use std::{
    ffi::OsString,
    future::Future,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::mpsc,
    time::Duration,
};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;
use tokio::process::{Child, Command};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

impl CommandSpec {
    #[must_use]
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
        }
    }

    #[must_use]
    pub fn arg(mut self, argument: impl Into<OsString>) -> Self {
        self.args.push(argument.into());
        self
    }

    #[must_use]
    pub fn args(mut self, arguments: impl IntoIterator<Item = impl Into<OsString>>) -> Self {
        self.args.extend(arguments.into_iter().map(Into::into));
        self
    }
}

#[derive(Clone, Debug)]
pub struct DevConfig {
    pub working_directory: PathBuf,
    pub rust: CommandSpec,
    pub vite: CommandSpec,
    pub client_build: CommandSpec,
    pub renderer_build: CommandSpec,
    /// Build browser and renderer bundles before each backend restart.
    pub build_frontend: bool,
    /// When true (default), watch Rust sources and restart `cargo run` on change.
    pub watch_rust: bool,
}

impl Default for DevConfig {
    fn default() -> Self {
        Self {
            working_directory: PathBuf::from("."),
            rust: CommandSpec::new("cargo").args(["run", "--", "serve"]),
            vite: CommandSpec::new("npm").args(["run", "dev", "--", "--strictPort"]),
            client_build: CommandSpec::new("npm").args(["run", "build:client"]),
            renderer_build: CommandSpec::new("npm").args(["run", "build:ssr"]),
            build_frontend: true,
            watch_rust: true,
        }
    }
}

impl DevConfig {
    #[must_use]
    pub fn working_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.working_directory = directory.into();
        self
    }

    #[must_use]
    pub fn rust(mut self, command: CommandSpec) -> Self {
        self.rust = command;
        self
    }

    #[must_use]
    pub fn vite(mut self, command: CommandSpec) -> Self {
        self.vite = command;
        self
    }

    #[must_use]
    pub fn client_build(mut self, command: CommandSpec) -> Self {
        self.client_build = command;
        self
    }

    #[must_use]
    pub fn renderer_build(mut self, command: CommandSpec) -> Self {
        self.renderer_build = command;
        self
    }

    #[must_use]
    pub const fn build_frontend(mut self, enabled: bool) -> Self {
        self.build_frontend = enabled;
        self
    }

    #[must_use]
    pub const fn watch_rust(mut self, enabled: bool) -> Self {
        self.watch_rust = enabled;
        self
    }
}

pub struct DevSupervisor {
    config: DevConfig,
}

impl DevSupervisor {
    #[must_use]
    pub fn new(config: DevConfig) -> Self {
        Self { config }
    }

    /// Run Rust and Vite together until Ctrl-C, or until Vite exits on its own.
    /// When `watch_rust` is enabled, Rust source changes restart the backend;
    /// compile/run failures wait for the next change instead of tearing down Vite.
    ///
    /// # Errors
    ///
    /// Returns an error when a child cannot start, signal registration fails,
    /// or the Vite process exits on its own.
    pub async fn run(self) -> Result<(), DevError> {
        self.run_with_shutdown(async { tokio::signal::ctrl_c().await.map_err(DevError::Signal) })
            .await
    }

    /// Run with a caller-provided shutdown future, primarily for integration
    /// with another application lifecycle or deterministic tests.
    ///
    /// # Errors
    ///
    /// Returns a spawn, wait, shutdown, watch, or early-child-exit error.
    pub async fn run_with_shutdown<F>(self, shutdown: F) -> Result<(), DevError>
    where
        F: Future<Output = Result<(), DevError>> + Send,
    {
        let cwd = self.config.working_directory.clone();
        let mut vite = spawn("Vite", &self.config.vite, &cwd)?;
        let mut changes = if self.config.watch_rust {
            Some(start_rust_watcher(&cwd)?)
        } else {
            None
        };
        tokio::pin!(shutdown);

        let result = async {
            loop {
                self.build_frontend(&cwd).await?;
                let mut rust = spawn("Rust", &self.config.rust, &cwd)?;
                if self.config.watch_rust {
                    eprintln!(
                        "px dev: watching application and React sources for rebuilds"
                    );
                }

                let event = tokio::select! {
                    result = rust.wait() => Event::Rust(result.map_err(DevError::Wait)?),
                    result = vite.wait() => Event::Vite(result.map_err(DevError::Wait)?),
                    result = &mut shutdown => Event::Shutdown(result),
                    changed = recv_change(&mut changes) => {
                        changed?;
                        Event::RustChanged
                    }
                };

                match event {
                    Event::Shutdown(result) => {
                        terminate(&mut rust).await?;
                        return result;
                    }
                    Event::Vite(status) => {
                        terminate(&mut rust).await?;
                        return Err(DevError::Exited {
                            process: "Vite",
                            status,
                        });
                    }
                    Event::RustChanged => {
                        eprintln!("px dev: Rust source changed — rebuilding…");
                        terminate(&mut rust).await?;
                        drain_changes(&mut changes, Duration::from_millis(400)).await;
                        continue;
                    }
                    Event::Rust(status) => {
                        if !self.config.watch_rust {
                            return Err(DevError::Exited {
                                process: "Rust",
                                status,
                            });
                        }
                        eprintln!(
                            "px dev: Rust process exited ({status}); waiting for source changes…"
                        );
                        let wait = tokio::select! {
                            result = vite.wait() => WaitWhileDown::Vite(result.map_err(DevError::Wait)?),
                            result = &mut shutdown => WaitWhileDown::Shutdown(result),
                            changed = recv_change(&mut changes) => {
                                changed?;
                                WaitWhileDown::Changed
                            }
                        };
                        match wait {
                            WaitWhileDown::Shutdown(result) => return result,
                            WaitWhileDown::Vite(status) => {
                                return Err(DevError::Exited {
                                    process: "Vite",
                                    status,
                                });
                            }
                            WaitWhileDown::Changed => {
                                drain_changes(&mut changes, Duration::from_millis(400)).await;
                                continue;
                            }
                        }
                    }
                }
            }
        }
        .await;

        terminate(&mut vite).await?;
        result
    }

    async fn build_frontend(&self, cwd: &Path) -> Result<(), DevError> {
        if !self.config.build_frontend {
            return Ok(());
        }
        run_to_completion("Client build", &self.config.client_build, cwd).await?;
        run_to_completion("Renderer build", &self.config.renderer_build, cwd).await
    }
}

enum Event {
    Rust(ExitStatus),
    Vite(ExitStatus),
    Shutdown(Result<(), DevError>),
    RustChanged,
}

enum WaitWhileDown {
    Vite(ExitStatus),
    Shutdown(Result<(), DevError>),
    Changed,
}

struct RustWatcher {
    _watcher: RecommendedWatcher,
    rx: UnboundedReceiver<()>,
}

fn start_rust_watcher(cwd: &Path) -> Result<RustWatcher, DevError> {
    let (tx, rx) = unbounded_channel();
    let (notify_tx, notify_rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |result| {
        let _ = notify_tx.send(result);
    })
    .map_err(DevError::Watch)?;

    for relative in ["app", "src", "routes", "config", "database", "views"] {
        let path = cwd.join(relative);
        if path.is_dir() {
            watcher
                .watch(&path, RecursiveMode::Recursive)
                .map_err(DevError::Watch)?;
        }
    }
    let cargo_toml = cwd.join("Cargo.toml");
    if cargo_toml.is_file() {
        watcher
            .watch(&cargo_toml, RecursiveMode::NonRecursive)
            .map_err(DevError::Watch)?;
    }

    let cwd = cwd.to_path_buf();
    std::thread::Builder::new()
        .name("px-dev-rust-watch".into())
        .spawn(move || watch_loop(cwd, notify_rx, tx))
        .map_err(|source| DevError::Spawn {
            process: "RustWatcher",
            source,
        })?;

    Ok(RustWatcher {
        _watcher: watcher,
        rx,
    })
}

fn watch_loop(
    cwd: PathBuf,
    notify_rx: mpsc::Receiver<Result<notify::Event, notify::Error>>,
    tx: UnboundedSender<()>,
) {
    while let Ok(result) = notify_rx.recv() {
        let Ok(event) = result else {
            continue;
        };
        if is_relevant_event(&cwd, &event) {
            let _ = tx.send(());
        }
    }
}

fn is_relevant_event(cwd: &Path, event: &notify::Event) -> bool {
    match event.kind {
        EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_) => {}
        _ => return false,
    }
    event
        .paths
        .iter()
        .any(|path| is_watched_rust_path(cwd, path))
}

fn is_watched_rust_path(cwd: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(cwd) else {
        return false;
    };
    let mut components = relative.components();
    let Some(std::path::Component::Normal(first)) = components.next() else {
        return path.file_name().is_some_and(|name| name == "Cargo.toml");
    };
    let first = first.to_string_lossy();
    matches!(
        first.as_ref(),
        "app" | "src" | "routes" | "config" | "database" | "views" | "Cargo.toml"
    ) && !relative.components().any(
        |component| matches!(component, std::path::Component::Normal(name) if name == "target"),
    )
}

async fn recv_change(changes: &mut Option<RustWatcher>) -> Result<(), DevError> {
    match changes.as_mut() {
        Some(watcher) => watcher.rx.recv().await.ok_or(DevError::WatchClosed),
        None => std::future::pending().await,
    }
}

async fn drain_changes(changes: &mut Option<RustWatcher>, window: Duration) {
    let Some(watcher) = changes.as_mut() else {
        return;
    };
    let deadline = tokio::time::Instant::now() + window;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, watcher.rx.recv()).await {
            Ok(Some(())) => continue,
            _ => break,
        }
    }
}

fn spawn(label: &'static str, spec: &CommandSpec, cwd: &Path) -> Result<Child, DevError> {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .current_dir(cwd)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    command.spawn().map_err(|source| DevError::Spawn {
        process: label,
        source,
    })
}

async fn run_to_completion(
    label: &'static str,
    spec: &CommandSpec,
    cwd: &Path,
) -> Result<(), DevError> {
    let mut child = spawn(label, spec, cwd)?;
    let status = child.wait().await.map_err(DevError::Wait)?;
    if status.success() {
        Ok(())
    } else {
        Err(DevError::Exited {
            process: label,
            status,
        })
    }
}

#[cfg(unix)]
async fn terminate(child: &mut Child) -> Result<(), DevError> {
    use nix::{
        sys::signal::{Signal, killpg},
        unistd::Pid,
    };

    let Some(id) = child.id() else {
        let _ = child.wait().await.map_err(DevError::Wait)?;
        return Ok(());
    };
    let process_group = Pid::from_raw(i32::try_from(id).map_err(|_| DevError::InvalidProcessId)?);
    if let Err(error) = killpg(process_group, Signal::SIGTERM)
        && error != nix::errno::Errno::ESRCH
    {
        return Err(DevError::SignalProcess(error));
    }
    if tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .is_err()
    {
        if let Err(error) = killpg(process_group, Signal::SIGKILL)
            && error != nix::errno::Errno::ESRCH
        {
            return Err(DevError::SignalProcess(error));
        }
        let _ = child.wait().await.map_err(DevError::Wait)?;
    }
    Ok(())
}

#[cfg(not(unix))]
async fn terminate(child: &mut Child) -> Result<(), DevError> {
    if child.id().is_some() {
        child.start_kill().map_err(DevError::Shutdown)?;
    }
    let _ = child.wait().await.map_err(DevError::Wait)?;
    Ok(())
}

#[derive(Debug, Error)]
pub enum DevError {
    #[error("failed to start the {process} development process: {source}")]
    Spawn {
        process: &'static str,
        source: std::io::Error,
    },
    #[error("failed while waiting for a development process: {0}")]
    Wait(std::io::Error),
    #[error("failed to stop a development process: {0}")]
    Shutdown(std::io::Error),
    #[cfg(unix)]
    #[error("failed to signal a development process group: {0}")]
    SignalProcess(nix::errno::Errno),
    #[error("a development process returned an invalid process id")]
    InvalidProcessId,
    #[error("failed to listen for Ctrl-C: {0}")]
    Signal(std::io::Error),
    #[error("failed to watch Rust sources for reload: {0}")]
    Watch(#[from] notify::Error),
    #[error("Rust file watcher stopped unexpectedly")]
    WatchClosed,
    #[error("the {process} development process exited early with {status}")]
    Exited {
        process: &'static str,
        status: ExitStatus,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::Duration};

    fn shell(script: &str) -> CommandSpec {
        CommandSpec::new("sh").args(["-c", script])
    }

    #[test]
    fn default_dev_config_runs_serve_with_watch() {
        let config = DevConfig::default();
        assert_eq!(config.rust.program, PathBuf::from("cargo"));
        assert!(config.watch_rust);
        assert_eq!(
            config.rust.args,
            vec![
                OsString::from("run"),
                OsString::from("--"),
                OsString::from("serve")
            ]
        );
    }

    #[test]
    fn watched_paths_cover_app_and_react_sources() {
        let cwd = PathBuf::from("/app");
        assert!(is_watched_rust_path(
            &cwd,
            &cwd.join("app/controllers/home.rs")
        ));
        assert!(is_watched_rust_path(&cwd, &cwd.join("Cargo.toml")));
        assert!(is_watched_rust_path(
            &cwd,
            &cwd.join("views/pages/home.tsx")
        ));
        assert!(!is_watched_rust_path(
            &cwd,
            &cwd.join("target/debug/my-app")
        ));
    }

    #[tokio::test]
    async fn shutdown_stops_and_reaps_both_processes() {
        let supervisor = DevSupervisor::new(
            DevConfig::default()
                .build_frontend(false)
                .watch_rust(false)
                .rust(shell("sleep 10 & wait"))
                .vite(shell("sleep 10 & wait")),
        );
        supervisor
            .run_with_shutdown(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(())
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn one_failed_process_stops_the_other_without_watch() {
        let supervisor = DevSupervisor::new(
            DevConfig::default()
                .build_frontend(false)
                .watch_rust(false)
                .rust(shell("exit 7"))
                .vite(shell("sleep 10")),
        );
        let result = supervisor.run_with_shutdown(std::future::pending()).await;

        assert!(matches!(
            result,
            Err(DevError::Exited {
                process: "Rust",
                status,
            }) if status.code() == Some(7)
        ));
    }

    #[tokio::test]
    async fn rust_exit_with_watch_waits_for_shutdown_without_failing() {
        let supervisor = DevSupervisor::new(
            DevConfig::default()
                .build_frontend(false)
                .watch_rust(true)
                .rust(shell("exit 1"))
                .vite(shell("sleep 10 & wait")),
        );
        supervisor
            .run_with_shutdown(async {
                tokio::time::sleep(Duration::from_millis(80)).await;
                Ok(())
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn frontend_builds_run_before_the_backend() {
        let marker = std::env::temp_dir().join(format!(
            "phoenix-dev-build-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let marker = marker.to_string_lossy().into_owned();
        let supervisor = DevSupervisor::new(
            DevConfig::default()
                .watch_rust(false)
                .client_build(shell(&format!("printf client > {marker}")))
                .renderer_build(shell(&format!("printf renderer >> {marker}")))
                .rust(shell("sleep 10"))
                .vite(shell("sleep 10")),
        );
        supervisor
            .run_with_shutdown(async {
                tokio::time::sleep(Duration::from_millis(80)).await;
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(fs::read_to_string(&marker).unwrap(), "clientrenderer");
        fs::remove_file(marker).unwrap();
    }
}
