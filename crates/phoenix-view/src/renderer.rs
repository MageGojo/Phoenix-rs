use std::{
    ffi::OsString,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{Mutex, mpsc},
};

use crate::{AssetManifest, AssetManifestError, Island, PageEnvelope, RendererManifest};

const RENDERER_PROTOCOL: u8 = 1;

#[derive(Clone, Debug)]
pub struct RendererConfig {
    program: PathBuf,
    args: Vec<OsString>,
    timeout: Duration,
    workers: usize,
    contract_hash: Option<String>,
    asset_version: Option<String>,
}

impl RendererConfig {
    /// Build a production renderer configuration from matching Vite
    /// manifests. The renderer entry is resolved below `renderer_root` and the
    /// worker handshake is pinned to the client asset version and contract.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid manifests or contract drift between the
    /// browser and renderer builds.
    pub fn production(
        assets: &AssetManifest,
        renderer: &RendererManifest,
        renderer_root: impl Into<PathBuf>,
    ) -> Result<Self, AssetManifestError> {
        assets.validate()?;
        renderer.validate()?;
        if assets.contract_hash != renderer.contract_hash {
            return Err(AssetManifestError::ContractMismatch {
                expected: assets.contract_hash.clone(),
                actual: renderer.contract_hash.clone(),
            });
        }
        Ok(Self::node(renderer_root.into().join(&renderer.entry))
            .with_contract_hash(&assets.contract_hash)
            .with_asset_version(&assets.version))
    }

    #[must_use]
    pub fn node(entrypoint: impl Into<PathBuf>) -> Self {
        Self {
            program: find_on_path("node").unwrap_or_else(|| PathBuf::from("node")),
            args: vec![entrypoint.into().into_os_string()],
            timeout: Duration::from_secs(2),
            workers: 1,
            contract_hash: None,
            asset_version: None,
        }
    }

    #[must_use]
    pub fn command(program: impl Into<PathBuf>, args: impl IntoIterator<Item = OsString>) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().collect(),
            timeout: Duration::from_secs(2),
            workers: 1,
            contract_hash: None,
            asset_version: None,
        }
    }

    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_workers(mut self, workers: usize) -> Self {
        self.workers = workers.max(1);
        self
    }

    #[must_use]
    pub fn with_contract_hash(mut self, contract_hash: impl Into<String>) -> Self {
        self.contract_hash = Some(contract_hash.into());
        self
    }

    #[must_use]
    pub fn with_asset_version(mut self, asset_version: impl Into<String>) -> Self {
        self.asset_version = Some(asset_version.into());
        self
    }
}

#[derive(Clone)]
pub struct NodeRenderer {
    inner: Arc<RendererInner>,
}

struct RendererInner {
    config: RendererConfig,
    workers: Vec<Mutex<Option<RendererProcess>>>,
    next_id: AtomicU64,
    next_worker: AtomicUsize,
    ready: AtomicUsize,
    active: AtomicUsize,
    rendered: AtomicU64,
    failures: AtomicU64,
    restarts: AtomicU64,
    timeouts: AtomicU64,
    shutting_down: AtomicBool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RendererHealth {
    pub configured_workers: usize,
    pub ready_workers: usize,
    pub active_requests: usize,
    pub rendered_requests: u64,
    pub failures: u64,
    pub restarts: u64,
    pub timeouts: u64,
    pub shutting_down: bool,
}

impl NodeRenderer {
    #[must_use]
    pub fn new(config: RendererConfig) -> Self {
        let workers = (0..config.workers).map(|_| Mutex::new(None)).collect();
        Self {
            inner: Arc::new(RendererInner {
                config,
                workers,
                next_id: AtomicU64::new(1),
                next_worker: AtomicUsize::new(0),
                ready: AtomicUsize::new(0),
                active: AtomicUsize::new(0),
                rendered: AtomicU64::new(0),
                failures: AtomicU64::new(0),
                restarts: AtomicU64::new(0),
                timeouts: AtomicU64::new(0),
                shutting_down: AtomicBool::new(false),
            }),
        }
    }

    /// Start every configured worker and validate its protocol/contract
    /// handshake before accepting production traffic.
    ///
    /// # Errors
    ///
    /// Returns the first process, protocol, or identity error.
    pub async fn warm_up(&self) -> Result<(), RendererError> {
        self.ensure_running()?;
        for worker in &self.inner.workers {
            let mut slot = worker.lock().await;
            if slot.is_none() {
                *slot = Some(RendererProcess::start(&self.inner.config).await?);
                self.inner.ready.fetch_add(1, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn health(&self) -> RendererHealth {
        RendererHealth {
            configured_workers: self.inner.workers.len(),
            ready_workers: self.inner.ready.load(Ordering::Relaxed),
            active_requests: self.inner.active.load(Ordering::Relaxed),
            rendered_requests: self.inner.rendered.load(Ordering::Relaxed),
            failures: self.inner.failures.load(Ordering::Relaxed),
            restarts: self.inner.restarts.load(Ordering::Relaxed),
            timeouts: self.inner.timeouts.load(Ordering::Relaxed),
            shutting_down: self.inner.shutting_down.load(Ordering::Acquire),
        }
    }

    /// Render one page through the persistent worker pool.
    ///
    /// Capacity waiting is included in the deadline. A broken child is
    /// replaced once; application renderer rejections are not retried.
    ///
    /// # Errors
    ///
    /// Returns an error for shutdown, contract drift, process failures,
    /// timeout, rejection, or invalid protocol data.
    pub async fn render(
        &self,
        envelope: &PageEnvelope,
        context: &RenderContext,
    ) -> Result<RenderResult, RendererError> {
        self.ensure_compatible(envelope)?;
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let worker = self.next_worker();
        let request = RendererRequest::render(id, envelope.clone(), context);
        let _active = ActiveRequest::new(&self.inner.active);
        let result = tokio::time::timeout(
            self.inner.config.timeout,
            self.render_on_worker(worker, &request),
        )
        .await;
        match result {
            Ok(Ok(rendered)) => {
                self.inner.rendered.fetch_add(1, Ordering::Relaxed);
                Ok(rendered)
            }
            Ok(Err(error)) => {
                self.inner.failures.fetch_add(1, Ordering::Relaxed);
                Err(error)
            }
            Err(_) => {
                self.inner.timeouts.fetch_add(1, Ordering::Relaxed);
                self.inner.failures.fetch_add(1, Ordering::Relaxed);
                self.reset_worker(worker).await;
                Err(RendererError::Timeout(self.inner.config.timeout))
            }
        }
    }

    /// Start a framed streaming render. Each HTML chunk is delivered as it is
    /// produced, followed by one completion frame containing head/island data.
    ///
    /// # Errors
    ///
    /// Returns immediately for shutdown or page/build identity mismatches.
    pub fn render_stream(
        &self,
        envelope: &PageEnvelope,
        context: &RenderContext,
    ) -> Result<RendererStream, RendererError> {
        self.ensure_compatible(envelope)?;
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let worker = self.next_worker();
        let request = RendererRequest::stream(id, envelope.clone(), context);
        let (sender, receiver) = mpsc::channel(16);
        let renderer = self.clone();
        tokio::spawn(async move {
            let _active = ActiveRequest::new(&renderer.inner.active);
            let result = tokio::time::timeout(
                renderer.inner.config.timeout,
                renderer.stream_on_worker(worker, &request, &sender),
            )
            .await;
            match result {
                Ok(Ok(())) => {
                    renderer.inner.rendered.fetch_add(1, Ordering::Relaxed);
                }
                Ok(Err(error)) => {
                    renderer.inner.failures.fetch_add(1, Ordering::Relaxed);
                    renderer.reset_worker(worker).await;
                    let _ = sender.send(Err(error)).await;
                }
                Err(_) => {
                    renderer.inner.timeouts.fetch_add(1, Ordering::Relaxed);
                    renderer.inner.failures.fetch_add(1, Ordering::Relaxed);
                    renderer.reset_worker(worker).await;
                    let _ = sender
                        .send(Err(RendererError::Timeout(renderer.inner.config.timeout)))
                        .await;
                }
            }
        });
        Ok(RendererStream { receiver })
    }

    /// Stop accepting work and terminate all child processes.
    pub async fn shutdown(&self) {
        self.inner.shutting_down.store(true, Ordering::Release);
        for worker in &self.inner.workers {
            let mut slot = worker.lock().await;
            if let Some(mut process) = slot.take() {
                let _ = process.shutdown().await;
                self.inner.ready.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }

    fn ensure_running(&self) -> Result<(), RendererError> {
        if self.inner.shutting_down.load(Ordering::Acquire) {
            Err(RendererError::ShuttingDown)
        } else {
            Ok(())
        }
    }

    fn ensure_compatible(&self, envelope: &PageEnvelope) -> Result<(), RendererError> {
        self.ensure_running()?;
        if let Some(expected) = &self.inner.config.contract_hash
            && envelope.contract_hash.as_deref() != Some(expected)
        {
            return Err(RendererError::ContractMismatch {
                expected: expected.clone(),
                actual: envelope.contract_hash.clone(),
            });
        }
        if let Some(expected) = &self.inner.config.asset_version
            && envelope.asset_version.as_deref() != Some(expected)
        {
            return Err(RendererError::AssetVersionMismatch {
                expected: expected.clone(),
                actual: envelope.asset_version.clone(),
            });
        }
        Ok(())
    }

    fn next_worker(&self) -> usize {
        self.inner.next_worker.fetch_add(1, Ordering::Relaxed) % self.inner.workers.len()
    }

    async fn render_on_worker(
        &self,
        worker: usize,
        request: &RendererRequest,
    ) -> Result<RenderResult, RendererError> {
        let mut slot = self.inner.workers[worker].lock().await;
        for attempt in 0..2 {
            self.ensure_running()?;
            if slot.is_none() {
                *slot = Some(RendererProcess::start(&self.inner.config).await?);
                self.inner.ready.fetch_add(1, Ordering::Relaxed);
            }
            let response = slot
                .as_mut()
                .expect("renderer process was initialized")
                .exchange(request)
                .await;
            match response {
                Ok(response) => return response.into_render_result(request.id),
                Err(error) if attempt == 0 => {
                    slot.take();
                    self.inner.ready.fetch_sub(1, Ordering::Relaxed);
                    self.inner.restarts.fetch_add(1, Ordering::Relaxed);
                    let _ = error;
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("renderer retry loop always returns")
    }

    async fn stream_on_worker(
        &self,
        worker: usize,
        request: &RendererRequest,
        sender: &mpsc::Sender<Result<RenderFrame, RendererError>>,
    ) -> Result<(), RendererError> {
        let mut slot = self.inner.workers[worker].lock().await;
        self.ensure_running()?;
        if slot.is_none() {
            *slot = Some(RendererProcess::start(&self.inner.config).await?);
            self.inner.ready.fetch_add(1, Ordering::Relaxed);
        }
        slot.as_mut()
            .expect("renderer process was initialized")
            .stream_exchange(request, sender)
            .await
    }

    async fn reset_worker(&self, worker: usize) {
        let mut slot = self.inner.workers[worker].lock().await;
        if slot.take().is_some() {
            self.inner.ready.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

struct ActiveRequest<'a> {
    active: &'a AtomicUsize,
}

impl<'a> ActiveRequest<'a> {
    fn new(active: &'a AtomicUsize) -> Self {
        active.fetch_add(1, Ordering::Relaxed);
        Self { active }
    }
}

impl Drop for ActiveRequest<'_> {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Clone, Debug)]
pub struct RenderContext {
    pub url: String,
    pub locale: String,
}

impl RenderContext {
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            locale: "en".to_owned(),
        }
    }

    #[must_use]
    pub fn locale(mut self, locale: impl Into<String>) -> Self {
        self.locale = locale.into();
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderResult {
    pub html: String,
    pub head: Vec<String>,
    pub islands: Vec<Island>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RenderFrame {
    Chunk(String),
    Complete {
        head: Vec<String>,
        islands: Vec<Island>,
    },
}

pub struct RendererStream {
    receiver: mpsc::Receiver<Result<RenderFrame, RendererError>>,
}

impl RendererStream {
    pub async fn recv(&mut self) -> Option<Result<RenderFrame, RendererError>> {
        self.receiver.recv().await
    }
}

struct RendererProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl RendererProcess {
    async fn start(config: &RendererConfig) -> Result<Self, RendererError> {
        let mut child = Command::new(&config.program)
            .args(&config.args)
            .env_clear()
            .env("NODE_ENV", "production")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(RendererError::Start)?;
        let stdin = child.stdin.take().ok_or(RendererError::MissingPipe)?;
        let stdout = child.stdout.take().ok_or(RendererError::MissingPipe)?;
        let mut process = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        };
        let hello = RendererRequest::hello(0, config);
        let response = process.exchange(&hello).await?;
        response.verify_identity(hello.id, config)?;
        Ok(process)
    }

    async fn write(&mut self, request: &RendererRequest) -> Result<(), RendererError> {
        let mut message = serde_json::to_vec(request).map_err(RendererError::Serialize)?;
        message.push(b'\n');
        self.stdin
            .write_all(&message)
            .await
            .map_err(RendererError::Io)?;
        self.stdin.flush().await.map_err(RendererError::Io)
    }

    async fn read_response(&mut self) -> Result<RendererResponse, RendererError> {
        let mut line = String::new();
        let bytes = self
            .stdout
            .read_line(&mut line)
            .await
            .map_err(RendererError::Io)?;
        if bytes == 0 {
            return Err(RendererError::Exited);
        }
        serde_json::from_str(&line).map_err(RendererError::InvalidResponse)
    }

    async fn exchange(
        &mut self,
        request: &RendererRequest,
    ) -> Result<RendererResponse, RendererError> {
        self.write(request).await?;
        self.read_response().await
    }

    async fn stream_exchange(
        &mut self,
        request: &RendererRequest,
        sender: &mpsc::Sender<Result<RenderFrame, RendererError>>,
    ) -> Result<(), RendererError> {
        self.write(request).await?;
        loop {
            let response = self.read_response().await?;
            response.verify_protocol(request.id)?;
            match response.kind.as_deref() {
                Some("chunk") => {
                    let _ = sender
                        .send(Ok(RenderFrame::Chunk(response.chunk.unwrap_or_default())))
                        .await;
                }
                Some("complete") => {
                    if !response.ok {
                        return Err(response.rejection());
                    }
                    let _ = sender
                        .send(Ok(RenderFrame::Complete {
                            head: response.head,
                            islands: response.islands,
                        }))
                        .await;
                    return Ok(());
                }
                Some("error") => return Err(response.rejection()),
                kind => {
                    return Err(RendererError::InvalidStreamFrame(
                        kind.unwrap_or("missing").to_owned(),
                    ));
                }
            }
        }
    }

    async fn shutdown(&mut self) -> Result<(), std::io::Error> {
        if self.child.id().is_some() {
            self.child.kill().await?;
        }
        let _ = self.child.wait().await;
        Ok(())
    }
}

#[derive(Clone, Serialize)]
struct RendererRequest {
    protocol: u8,
    id: u64,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    envelope: Option<PageEnvelope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    locale: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    contract_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    asset_version: Option<String>,
}

impl RendererRequest {
    fn hello(id: u64, config: &RendererConfig) -> Self {
        Self {
            protocol: RENDERER_PROTOCOL,
            id,
            kind: "hello",
            envelope: None,
            url: None,
            locale: None,
            contract_hash: config.contract_hash.clone(),
            asset_version: config.asset_version.clone(),
        }
    }

    fn render(id: u64, envelope: PageEnvelope, context: &RenderContext) -> Self {
        Self::page(id, "render", envelope, context)
    }

    fn stream(id: u64, envelope: PageEnvelope, context: &RenderContext) -> Self {
        Self::page(id, "stream", envelope, context)
    }

    fn page(id: u64, kind: &'static str, envelope: PageEnvelope, context: &RenderContext) -> Self {
        Self {
            protocol: RENDERER_PROTOCOL,
            id,
            kind,
            contract_hash: envelope.contract_hash.clone(),
            asset_version: envelope.asset_version.clone(),
            envelope: Some(envelope),
            url: Some(context.url.clone()),
            locale: Some(context.locale.clone()),
        }
    }
}

#[derive(Deserialize)]
struct RendererResponse {
    protocol: u8,
    id: u64,
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    html: String,
    #[serde(default)]
    head: Vec<String>,
    #[serde(default)]
    islands: Vec<Island>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    chunk: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    contract_hash: Option<String>,
    #[serde(default)]
    asset_version: Option<String>,
}

impl RendererResponse {
    fn verify_protocol(&self, expected_id: u64) -> Result<(), RendererError> {
        if self.protocol == RENDERER_PROTOCOL && self.id == expected_id {
            Ok(())
        } else {
            Err(RendererError::ProtocolMismatch {
                expected_id,
                actual_id: self.id,
                protocol: self.protocol,
            })
        }
    }

    fn verify_identity(
        &self,
        expected_id: u64,
        config: &RendererConfig,
    ) -> Result<(), RendererError> {
        self.verify_protocol(expected_id)?;
        if !self.ok {
            return Err(self.rejection());
        }
        if let Some(expected) = &config.contract_hash
            && self.contract_hash.as_ref() != Some(expected)
        {
            return Err(RendererError::ContractMismatch {
                expected: expected.clone(),
                actual: self.contract_hash.clone(),
            });
        }
        if let Some(expected) = &config.asset_version
            && self.asset_version.as_ref() != Some(expected)
        {
            return Err(RendererError::AssetVersionMismatch {
                expected: expected.clone(),
                actual: self.asset_version.clone(),
            });
        }
        Ok(())
    }

    fn into_render_result(self, expected_id: u64) -> Result<RenderResult, RendererError> {
        self.verify_protocol(expected_id)?;
        if !self.ok {
            return Err(self.rejection());
        }
        Ok(RenderResult {
            html: self.html,
            head: self.head,
            islands: self.islands,
        })
    }

    fn rejection(&self) -> RendererError {
        RendererError::Rejected(
            self.error
                .clone()
                .unwrap_or_else(|| "renderer rejected the page".to_owned()),
        )
    }
}

#[derive(Debug, Error)]
pub enum RendererError {
    #[error("the SSR renderer is shutting down")]
    ShuttingDown,
    #[error("failed to start the SSR renderer: {0}")]
    Start(std::io::Error),
    #[error("SSR renderer standard streams are unavailable")]
    MissingPipe,
    #[error("SSR renderer I/O failed: {0}")]
    Io(std::io::Error),
    #[error("SSR renderer exited before responding")]
    Exited,
    #[error("SSR renderer request serialization failed: {0}")]
    Serialize(serde_json::Error),
    #[error("SSR renderer returned invalid JSON: {0}")]
    InvalidResponse(serde_json::Error),
    #[error(
        "SSR renderer protocol mismatch (protocol {protocol}, request {actual_id}, expected {expected_id})"
    )]
    ProtocolMismatch {
        expected_id: u64,
        actual_id: u64,
        protocol: u8,
    },
    #[error("SSR renderer returned an invalid streaming frame: {0}")]
    InvalidStreamFrame(String),
    #[error("SSR renderer rejected the page: {0}")]
    Rejected(String),
    #[error("SSR renderer exceeded its {0:?} deadline")]
    Timeout(Duration),
    #[error("contract hash mismatch (expected {expected}, actual {actual:?})")]
    ContractMismatch {
        expected: String,
        actual: Option<String>,
    },
    #[error("asset version mismatch (expected {expected}, actual {actual:?})")]
    AssetVersionMismatch {
        expected: String,
        actual: Option<String>,
    },
}

fn find_on_path(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|directory| directory.join(program))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Instant;

    fn node_fixture(source: &str) -> RendererConfig {
        RendererConfig::command(
            find_on_path("node").expect("Node.js is required for renderer tests"),
            [OsString::from("--eval"), OsString::from(source)],
        )
    }

    #[test]
    fn production_config_pins_renderer_to_client_manifest() {
        let assets = AssetManifest {
            schema: crate::ASSET_MANIFEST_SCHEMA,
            version: "sha256-client".to_owned(),
            contract_hash: "fnv1a-contract".to_owned(),
            public_path: "/assets/".to_owned(),
            entries: std::collections::HashMap::from([(
                "client".to_owned(),
                crate::AssetEntry {
                    file: "phoenix.js".to_owned(),
                    css: Vec::new(),
                    imports: Vec::new(),
                },
            )]),
        };
        let renderer = RendererManifest {
            schema: crate::ASSET_MANIFEST_SCHEMA,
            version: "sha256-renderer".to_owned(),
            contract_hash: "fnv1a-contract".to_owned(),
            entry: "renderer-a1.js".to_owned(),
        };
        let config = RendererConfig::production(&assets, &renderer, "public/ssr").unwrap();

        assert_eq!(config.asset_version.as_deref(), Some("sha256-client"));
        assert_eq!(config.contract_hash.as_deref(), Some("fnv1a-contract"));
        assert!(
            config.args[0]
                .to_string_lossy()
                .ends_with("public/ssr/renderer-a1.js")
        );
    }

    #[tokio::test]
    async fn reuses_one_process_for_multiple_dynamic_pages() {
        let source = r"
          const readline = require('node:readline');
          let count = 0;
          readline.createInterface({input: process.stdin}).on('line', line => {
            const request = JSON.parse(line); count += 1;
            console.log(JSON.stringify({protocol: 1, id: request.id, ok: true,
              html: request.kind === 'hello' ? '' : `<p>${request.envelope.props.value}:${count}</p>`, head: []}));
          });
        ";
        let renderer = NodeRenderer::new(node_fixture(source));
        let context = RenderContext::new("/dynamic");

        let first = renderer
            .render(
                &PageEnvelope::new_for_test(json!({ "value": "one" })),
                &context,
            )
            .await
            .unwrap();
        let second = renderer
            .render(
                &PageEnvelope::new_for_test(json!({ "value": "two" })),
                &context,
            )
            .await
            .unwrap();

        assert_eq!(first.html, "<p>one:2</p>");
        assert_eq!(second.html, "<p>two:3</p>");
    }

    #[tokio::test]
    async fn worker_pool_renders_concurrently_and_reports_health() {
        let source = r"
          const readline = require('node:readline');
          readline.createInterface({input: process.stdin}).on('line', line => {
            const request = JSON.parse(line);
            const response = () => console.log(JSON.stringify({protocol: 1, id: request.id,
              ok: true, html: '<p>done</p>', head: []}));
            if (request.kind === 'render') setTimeout(response, 120); else response();
          });
        ";
        let renderer = NodeRenderer::new(node_fixture(source).with_workers(2));
        renderer.warm_up().await.unwrap();
        assert_eq!(renderer.health().ready_workers, 2);
        let envelope = PageEnvelope::new_for_test(json!({}));
        let context = RenderContext::new("/pool");
        let started = Instant::now();
        let (left, right) = tokio::join!(
            renderer.render(&envelope, &context),
            renderer.render(&envelope, &context)
        );
        left.unwrap();
        right.unwrap();
        assert!(started.elapsed() < Duration::from_millis(220));
        assert_eq!(renderer.health().rendered_requests, 2);

        renderer.shutdown().await;
        let health = renderer.health();
        assert_eq!(health.ready_workers, 0);
        assert!(health.shutting_down);
        assert!(matches!(
            renderer.render(&envelope, &context).await,
            Err(RendererError::ShuttingDown)
        ));
    }

    #[tokio::test]
    async fn validates_contract_hash_during_handshake_and_render() {
        let source = r"
          const readline = require('node:readline');
          readline.createInterface({input: process.stdin}).on('line', line => {
            const request = JSON.parse(line);
            console.log(JSON.stringify({protocol: 1, id: request.id, ok: true,
              contract_hash: 'renderer-contract', html: ''}));
          });
        ";
        let renderer =
            NodeRenderer::new(node_fixture(source).with_contract_hash("server-contract"));
        assert!(matches!(
            renderer.warm_up().await,
            Err(RendererError::ContractMismatch { .. })
        ));

        let renderer = NodeRenderer::new(node_fixture(source));
        let mut envelope = PageEnvelope::new_for_test(json!({}));
        envelope.contract_hash = Some("wrong".to_owned());
        let strict =
            NodeRenderer::new(node_fixture(source).with_contract_hash("renderer-contract"));
        assert!(matches!(
            strict.render(&envelope, &RenderContext::new("/")).await,
            Err(RendererError::ContractMismatch { .. })
        ));
        renderer.shutdown().await;
    }

    #[tokio::test]
    async fn streams_html_chunks_and_completion_metadata() {
        let source = r"
          const readline = require('node:readline');
          readline.createInterface({input: process.stdin}).on('line', line => {
            const request = JSON.parse(line);
            if (request.kind === 'hello') {
              console.log(JSON.stringify({protocol: 1, id: request.id, ok: true})); return;
            }
            console.log(JSON.stringify({protocol: 1, id: request.id, ok: true, kind: 'chunk', chunk: '<h1>'}));
            console.log(JSON.stringify({protocol: 1, id: request.id, ok: true, kind: 'chunk', chunk: 'Hello</h1>'}));
            console.log(JSON.stringify({protocol: 1, id: request.id, ok: true, kind: 'complete',
              islands: [{id: 'counter', component: 'counter', props: {value: 1}}]}));
          });
        ";
        let renderer = NodeRenderer::new(node_fixture(source));
        let mut stream = renderer
            .render_stream(
                &PageEnvelope::new_for_test(json!({})),
                &RenderContext::new("/stream"),
            )
            .unwrap();
        assert_eq!(
            stream.recv().await.unwrap().unwrap(),
            RenderFrame::Chunk("<h1>".to_owned())
        );
        assert_eq!(
            stream.recv().await.unwrap().unwrap(),
            RenderFrame::Chunk("Hello</h1>".to_owned())
        );
        assert!(matches!(
            stream.recv().await.unwrap().unwrap(),
            RenderFrame::Complete { islands, .. } if islands.len() == 1
        ));
        assert!(stream.recv().await.is_none());
    }

    #[tokio::test]
    async fn times_out_without_hanging_the_request() {
        let source = r"
          const readline = require('node:readline');
          readline.createInterface({input: process.stdin}).on('line', () => {});
        ";
        let renderer =
            NodeRenderer::new(node_fixture(source).with_timeout(Duration::from_millis(100)));
        let result = renderer
            .render(
                &PageEnvelope::new_for_test(json!({ "value": "slow" })),
                &RenderContext::new("/slow"),
            )
            .await;

        assert!(matches!(result, Err(RendererError::Timeout(_))));
        assert_eq!(renderer.health().timeouts, 1);
    }
}
