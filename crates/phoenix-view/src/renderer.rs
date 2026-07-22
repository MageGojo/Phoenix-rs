use std::{
    ffi::OsString,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};

use crate::{Island, PageEnvelope};

const RENDERER_PROTOCOL: u8 = 1;

#[derive(Clone, Debug)]
pub struct RendererConfig {
    program: PathBuf,
    args: Vec<OsString>,
    timeout: Duration,
}

impl RendererConfig {
    #[must_use]
    pub fn node(entrypoint: impl Into<PathBuf>) -> Self {
        Self {
            program: find_on_path("node").unwrap_or_else(|| PathBuf::from("node")),
            args: vec![entrypoint.into().into_os_string()],
            timeout: Duration::from_secs(2),
        }
    }

    #[must_use]
    pub fn command(program: impl Into<PathBuf>, args: impl IntoIterator<Item = OsString>) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().collect(),
            timeout: Duration::from_secs(2),
        }
    }

    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[derive(Clone)]
pub struct NodeRenderer {
    inner: Arc<RendererInner>,
}

struct RendererInner {
    config: RendererConfig,
    process: Mutex<Option<RendererProcess>>,
    next_id: AtomicU64,
}

impl NodeRenderer {
    #[must_use]
    pub fn new(config: RendererConfig) -> Self {
        Self {
            inner: Arc::new(RendererInner {
                config,
                process: Mutex::new(None),
                next_id: AtomicU64::new(1),
            }),
        }
    }

    /// Render one page through a persistent Node.js child process.
    ///
    /// Waiting for capacity is included in the timeout. A broken child is
    /// replaced once; renderer rejections are returned without changing mode.
    ///
    /// # Errors
    ///
    /// Returns an error when the process cannot start, times out, rejects the
    /// page, or violates the renderer protocol.
    pub async fn render(
        &self,
        envelope: &PageEnvelope,
        context: &RenderContext,
    ) -> Result<RenderResult, RendererError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let request = RendererRequest::render(id, envelope, context);
        if let Ok(result) =
            tokio::time::timeout(self.inner.config.timeout, self.render_request(&request)).await
        {
            result
        } else {
            self.inner.process.lock().await.take();
            Err(RendererError::Timeout(self.inner.config.timeout))
        }
    }

    async fn render_request(
        &self,
        request: &RendererRequest<'_>,
    ) -> Result<RenderResult, RendererError> {
        let mut slot = self.inner.process.lock().await;
        for attempt in 0..2 {
            if slot.is_none() {
                *slot = Some(RendererProcess::start(&self.inner.config).await?);
            }

            let response = slot
                .as_mut()
                .expect("renderer process was initialized")
                .exchange(request)
                .await;
            match response {
                Ok(response) => return response.into_result(request.id),
                Err(error) if attempt == 0 => {
                    slot.take();
                    let _ = error;
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("renderer retry loop always returns")
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

struct RendererProcess {
    _child: Child,
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
            _child: child,
            stdin,
            stdout: BufReader::new(stdout),
        };
        let hello = RendererRequest::hello(0);
        process.exchange(&hello).await?.into_result(hello.id)?;
        Ok(process)
    }

    async fn exchange(
        &mut self,
        request: &RendererRequest<'_>,
    ) -> Result<RendererResponse, RendererError> {
        let mut message = serde_json::to_vec(request).map_err(RendererError::Serialize)?;
        message.push(b'\n');
        self.stdin
            .write_all(&message)
            .await
            .map_err(RendererError::Io)?;
        self.stdin.flush().await.map_err(RendererError::Io)?;

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
}

#[derive(Serialize)]
struct RendererRequest<'a> {
    protocol: u8,
    id: u64,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    envelope: Option<&'a PageEnvelope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    locale: Option<&'a str>,
}

impl<'a> RendererRequest<'a> {
    const fn hello(id: u64) -> Self {
        Self {
            protocol: RENDERER_PROTOCOL,
            id,
            kind: "hello",
            envelope: None,
            url: None,
            locale: None,
        }
    }

    fn render(id: u64, envelope: &'a PageEnvelope, context: &'a RenderContext) -> Self {
        Self {
            protocol: RENDERER_PROTOCOL,
            id,
            kind: "render",
            envelope: Some(envelope),
            url: Some(&context.url),
            locale: Some(&context.locale),
        }
    }
}

#[derive(Deserialize)]
struct RendererResponse {
    protocol: u8,
    id: u64,
    ok: bool,
    #[serde(default)]
    html: String,
    #[serde(default)]
    head: Vec<String>,
    #[serde(default)]
    islands: Vec<Island>,
    error: Option<String>,
}

impl RendererResponse {
    fn into_result(self, expected_id: u64) -> Result<RenderResult, RendererError> {
        if self.protocol != RENDERER_PROTOCOL || self.id != expected_id {
            return Err(RendererError::ProtocolMismatch {
                expected_id,
                actual_id: self.id,
                protocol: self.protocol,
            });
        }
        if !self.ok {
            return Err(RendererError::Rejected(
                self.error
                    .unwrap_or_else(|| "renderer rejected the page".to_owned()),
            ));
        }
        Ok(RenderResult {
            html: self.html,
            head: self.head,
            islands: self.islands,
        })
    }
}

#[derive(Debug, Error)]
pub enum RendererError {
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
    #[error("SSR renderer rejected the page: {0}")]
    Rejected(String),
    #[error("SSR renderer exceeded its {0:?} deadline")]
    Timeout(Duration),
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

    fn node_fixture(source: &str) -> RendererConfig {
        RendererConfig::command(
            find_on_path("node").expect("Node.js is required for renderer tests"),
            [OsString::from("--eval"), OsString::from(source)],
        )
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
    }

    #[tokio::test]
    async fn restarts_once_when_the_child_exits_during_rendering() {
        let marker = std::env::temp_dir().join(format!(
            "phoenix-renderer-restart-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let marker_json = serde_json::to_string(&marker).unwrap();
        let source = format!(
            r"
              const fs = require('node:fs');
              const readline = require('node:readline');
              const marker = {marker_json};
              readline.createInterface({{input: process.stdin}}).on('line', line => {{
                const request = JSON.parse(line);
                if (request.kind === 'render' && !fs.existsSync(marker)) {{
                  fs.writeFileSync(marker, 'restart');
                  process.exit(1);
                }}
                console.log(JSON.stringify({{protocol: 1, id: request.id, ok: true,
                  html: request.kind === 'hello' ? '' : '<p>recovered</p>', head: []}}));
              }});
            "
        );
        let renderer = NodeRenderer::new(node_fixture(&source));

        let result = renderer
            .render(
                &PageEnvelope::new_for_test(json!({ "value": "restart" })),
                &RenderContext::new("/restart"),
            )
            .await
            .unwrap();

        assert_eq!(result.html, "<p>recovered</p>");
        std::fs::remove_file(marker).unwrap();
    }
}
