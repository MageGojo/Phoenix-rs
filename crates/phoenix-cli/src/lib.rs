use std::{
    ffi::OsString,
    future::Future,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
};

use thiserror::Error;
use tokio::process::{Child, Command};

mod scaffold;

pub use scaffold::{
    ControllerOptions, DependencySource, GenerateOptions, ModelOptions, NewProjectOptions,
    ProjectGenerator, ScaffoldError, create_project,
};

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
}

impl Default for DevConfig {
    fn default() -> Self {
        Self {
            working_directory: PathBuf::from("."),
            rust: CommandSpec::new("cargo").arg("run"),
            vite: CommandSpec::new("npm").args(["run", "dev", "--", "--strictPort"]),
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
}

pub struct DevSupervisor {
    config: DevConfig,
}

impl DevSupervisor {
    #[must_use]
    pub fn new(config: DevConfig) -> Self {
        Self { config }
    }

    /// Run Rust and Vite together until Ctrl-C, or until either process exits.
    /// Both children are terminated and reaped before this function returns.
    ///
    /// # Errors
    ///
    /// Returns an error when a child cannot start, signal registration fails,
    /// or either development process exits on its own.
    pub async fn run(self) -> Result<(), DevError> {
        self.run_with_shutdown(async { tokio::signal::ctrl_c().await.map_err(DevError::Signal) })
            .await
    }

    /// Run with a caller-provided shutdown future, primarily for integration
    /// with another application lifecycle or deterministic tests.
    ///
    /// # Errors
    ///
    /// Returns a spawn, wait, shutdown, or early-child-exit error.
    pub async fn run_with_shutdown<F>(self, shutdown: F) -> Result<(), DevError>
    where
        F: Future<Output = Result<(), DevError>> + Send,
    {
        let mut rust = spawn("Rust", &self.config.rust, &self.config.working_directory)?;
        let mut vite = match spawn("Vite", &self.config.vite, &self.config.working_directory) {
            Ok(vite) => vite,
            Err(error) => {
                terminate(&mut rust).await?;
                return Err(error);
            }
        };
        tokio::pin!(shutdown);

        let event = tokio::select! {
            result = rust.wait() => Event::Rust(result.map_err(DevError::Wait)?),
            result = vite.wait() => Event::Vite(result.map_err(DevError::Wait)?),
            result = &mut shutdown => Event::Shutdown(result),
        };
        terminate(&mut rust).await?;
        terminate(&mut vite).await?;

        match event {
            Event::Shutdown(result) => result,
            Event::Rust(status) => Err(DevError::Exited {
                process: "Rust",
                status,
            }),
            Event::Vite(status) => Err(DevError::Exited {
                process: "Vite",
                status,
            }),
        }
    }
}

enum Event {
    Rust(ExitStatus),
    Vite(ExitStatus),
    Shutdown(Result<(), DevError>),
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

#[cfg(unix)]
async fn terminate(child: &mut Child) -> Result<(), DevError> {
    use nix::{
        sys::signal::{Signal, killpg},
        unistd::Pid,
    };
    use std::time::Duration;

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
    #[error("the {process} development process exited early with {status}")]
    Exited {
        process: &'static str,
        status: ExitStatus,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn shell(script: &str) -> CommandSpec {
        CommandSpec::new("sh").args(["-c", script])
    }

    #[tokio::test]
    async fn shutdown_stops_and_reaps_both_processes() {
        let supervisor = DevSupervisor::new(
            DevConfig::default()
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
    async fn one_failed_process_stops_the_other() {
        let supervisor = DevSupervisor::new(
            DevConfig::default()
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
}
