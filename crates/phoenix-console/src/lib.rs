//! Application console: `myapp update` with a one-function command.
//! See `docs/QUEUE_MAIL_CONSOLE.md`.

#![forbid(unsafe_code)]

use std::{
    error::Error,
    fmt::{Display, Formatter},
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
};

/// Result type returned by console commands and [`Console::run`].
pub type CommandResult = Result<(), Box<dyn Error + Send + Sync>>;

/// Borrowed arguments for a single console invocation.
#[derive(Clone, Copy, Debug)]
pub struct CommandContext<'a> {
    /// Remaining argv after the binary name and command name.
    pub args: &'a [String],
    /// Application binary name used in help text.
    pub binary_name: &'a str,
}

type BoxedCommandFuture<'a> = Pin<Box<dyn Future<Output = CommandResult> + Send + 'a>>;
type CommandHandler =
    Box<dyn for<'a> Fn(CommandContext<'a>) -> BoxedCommandFuture<'a> + Send + Sync>;

/// One registered application command.
pub struct CommandEntry {
    name: &'static str,
    handler: CommandHandler,
}

impl CommandEntry {
    /// Wrap an async command function for registration.
    #[must_use]
    pub fn new<F>(name: &'static str, handler: F) -> Self
    where
        F: for<'a> Fn(CommandContext<'a>) -> BoxedCommandFuture<'a> + Send + Sync + 'static,
    {
        Self {
            name,
            handler: Box::new(handler),
        }
    }

    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }
}

/// Application CLI console built around a binary name and named commands.
pub struct Console {
    binary_name: String,
    about: Option<String>,
    serve: Option<CommandHandler>,
    commands: Vec<CommandEntry>,
}

impl Console {
    /// Create a console whose help text uses `binary_name`.
    #[must_use]
    pub fn new(binary_name: impl Into<String>) -> Self {
        Self {
            binary_name: binary_name.into(),
            about: None,
            serve: None,
            commands: Vec::new(),
        }
    }

    /// Short description printed above the usage block.
    #[must_use]
    pub fn about(mut self, about: impl Into<String>) -> Self {
        self.about = Some(about.into());
        self
    }

    /// Register the built-in `serve` command that starts the HTTP application.
    #[must_use]
    pub fn serve<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(CommandContext<'_>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = CommandResult> + Send + 'static,
    {
        self.serve = Some(Box::new(move |ctx| {
            let args = ctx.args.to_vec();
            let binary_name = ctx.binary_name.to_owned();
            let future = handler(CommandContext {
                args: &args,
                binary_name: &binary_name,
            });
            Box::pin(future)
        }));
        self
    }

    /// Append user-registered commands from [`commands!`] or a manual list.
    #[must_use]
    pub fn commands(mut self, commands: impl IntoIterator<Item = CommandEntry>) -> Self {
        self.commands.extend(commands);
        self
    }

    /// Parse process argv and dispatch.
    ///
    /// # Errors
    ///
    /// Returns an error when a packaged release root cannot be entered, for
    /// unknown commands, or when a command fails.
    pub async fn run(self) -> CommandResult {
        enter_packaged_release_root()?;
        self.run_argv(std::env::args()).await
    }

    /// Parse an explicit argv (including argv0) and dispatch.
    ///
    /// # Errors
    ///
    /// Returns an error for unknown commands or when a command fails.
    pub async fn run_argv(
        self,
        argv: impl IntoIterator<Item = impl Into<String>>,
    ) -> CommandResult {
        let argv = argv.into_iter().map(Into::into).collect::<Vec<_>>();
        let binary_name = self.binary_name.as_str();
        let command = argv.get(1).map(String::as_str);

        match command {
            None | Some("help" | "--help" | "-h") => {
                print!("{}", self.usage());
                Ok(())
            }
            Some("serve") => {
                let Some(serve) = &self.serve else {
                    return Err(Box::new(ConsoleError::ServeUnavailable));
                };
                let ctx = CommandContext {
                    args: argv.get(2..).unwrap_or(&[]),
                    binary_name,
                };
                serve(ctx).await
            }
            Some(name) => {
                let ctx = CommandContext {
                    args: argv.get(2..).unwrap_or(&[]),
                    binary_name,
                };
                let Some(entry) = self.commands.iter().find(|entry| entry.name == name) else {
                    return Err(Box::new(ConsoleError::UnknownCommand {
                        command: name.to_owned(),
                        usage: self.usage(),
                    }));
                };
                (entry.handler)(ctx).await
            }
        }
    }

    fn usage(&self) -> String {
        let mut lines = Vec::new();
        if let Some(about) = &self.about {
            lines.push(about.clone());
            lines.push(String::new());
        }
        lines.push(format!(
            "Usage:\n  {} <command> [args...]\n",
            self.binary_name
        ));
        lines.push("Commands:".to_owned());
        if self.serve.is_some() {
            lines.push("  serve    Start the HTTP server".to_owned());
        }
        let mut names = self
            .commands
            .iter()
            .map(CommandEntry::name)
            .collect::<Vec<_>>();
        names.sort_unstable();
        for name in names {
            lines.push(format!("  {name}"));
        }
        lines.push(String::new());
        lines.push(format!("Run `{} help` for this message.", self.binary_name));
        lines.push(String::new());
        lines.join("\n")
    }
}

/// Enter the release root when the executable lives in the conventional
/// `<release>/bin/<application>` layout.
///
/// Phoenix application configuration, static assets, and the Node renderer all
/// use paths relative to the application root. Process managers normally set
/// their working directory to `<deploy>/current`, but a release binary should
/// behave the same when it is started directly from its `bin/` directory.
fn enter_packaged_release_root() -> std::io::Result<Option<PathBuf>> {
    let Ok(executable) = std::env::current_exe() else {
        return Ok(None);
    };
    let Some(root) = packaged_release_root(&executable) else {
        return Ok(None);
    };
    std::env::set_current_dir(&root)?;
    Ok(Some(root))
}

fn packaged_release_root(executable: &Path) -> Option<PathBuf> {
    let bin = executable.parent()?;
    if bin.file_name().and_then(|name| name.to_str()) != Some("bin") {
        return None;
    }
    let root = bin.parent()?;
    if root.join("manifest.toml").is_file()
        && root.join("config").is_dir()
        && root.join("public").is_dir()
    {
        Some(root.to_path_buf())
    } else {
        None
    }
}

enum ConsoleError {
    UnknownCommand { command: String, usage: String },
    ServeUnavailable,
}

impl Display for ConsoleError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownCommand { command, usage } => {
                write!(formatter, "unknown command `{command}`\n\n{usage}")
            }
            Self::ServeUnavailable => {
                write!(
                    formatter,
                    "command `serve` is not configured; call Console::serve first"
                )
            }
        }
    }
}

impl std::fmt::Debug for ConsoleError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self, formatter)
    }
}

impl Error for ConsoleError {}
/// Register application commands without clap boilerplate.
///
/// # Examples
///
/// ```ignore
/// commands! {
///     update,
///     "sync-users" => sync_users,
/// }
/// ```
///
/// Generates `pub fn registry() -> Vec<CommandEntry>` in the enclosing module.
#[macro_export]
macro_rules! commands {
    (@push $entries:ident;) => {};
    (@push $entries:ident; $name:ident) => {
        $entries.push($crate::CommandEntry::new(
            stringify!($name),
            |__ctx| ::std::boxed::Box::pin($name(__ctx)),
        ));
    };
    (@push $entries:ident; $name:ident,) => {
        $crate::commands!(@push $entries; $name);
    };
    (@push $entries:ident; $name:ident, $($rest:tt)*) => {
        $crate::commands!(@push $entries; $name);
        $crate::commands!(@push $entries; $($rest)*);
    };
    (@push $entries:ident; $alias:literal => $handler:ident) => {
        $entries.push($crate::CommandEntry::new(
            $alias,
            |__ctx| ::std::boxed::Box::pin($handler(__ctx)),
        ));
    };
    (@push $entries:ident; $alias:literal => $handler:ident,) => {
        $crate::commands!(@push $entries; $alias => $handler);
    };
    (@push $entries:ident; $alias:literal => $handler:ident, $($rest:tt)*) => {
        $crate::commands!(@push $entries; $alias => $handler);
        $crate::commands!(@push $entries; $($rest)*);
    };
    () => {
        /// Registered application console commands.
        #[must_use]
        pub fn registry() -> Vec<$crate::CommandEntry> {
            Vec::new()
        }
    };
    ($($item:tt)*) => {
        /// Registered application console commands.
        #[must_use]
        #[allow(clippy::vec_init_then_push)]
        pub fn registry() -> Vec<$crate::CommandEntry> {
            let mut __phoenix_commands = Vec::new();
            $crate::commands!(@push __phoenix_commands; $($item)*);
            __phoenix_commands
        }
    };
}

#[must_use]
pub const fn crate_name() -> &'static str {
    "phoenix-console"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    #[allow(clippy::unused_async)]
    async fn update(_ctx: CommandContext<'_>) -> CommandResult {
        Ok(())
    }

    commands! {
        update,
    }

    #[tokio::test]
    async fn registered_update_command_runs() {
        let result = Console::new("demo")
            .about("Demo")
            .commands(registry())
            .run_argv(["demo", "update"])
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn unknown_command_fails() {
        let result = Console::new("demo")
            .commands(registry())
            .run_argv(["demo", "nope"])
            .await;
        let error = result.expect_err("unknown command should fail");
        assert!(error.to_string().contains("unknown command `nope`"));
    }

    #[tokio::test]
    async fn help_does_not_panic() {
        Console::new("demo")
            .about("Demo app")
            .serve(|_ctx| async move { Ok(()) })
            .commands(registry())
            .run_argv(["demo", "help"])
            .await
            .expect("help should succeed");
        Console::new("demo")
            .run_argv(["demo"])
            .await
            .expect("empty argv should print usage");
    }

    #[tokio::test]
    async fn serve_invokes_handler() {
        let called = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&called);
        Console::new("demo")
            .serve(move |_ctx| {
                let flag = Arc::clone(&flag);
                async move {
                    flag.store(true, Ordering::SeqCst);
                    Ok(())
                }
            })
            .run_argv(["demo", "serve"])
            .await
            .expect("serve should succeed");
        assert!(called.load(Ordering::SeqCst));
    }

    #[test]
    fn discovers_packaged_release_root_from_bin_executable() {
        let root = temporary_directory("packaged-root");
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::create_dir_all(root.join("public")).unwrap();
        fs::write(root.join("manifest.toml"), "schema_version = 1\n").unwrap();

        assert_eq!(
            packaged_release_root(&root.join("bin/demo")),
            Some(root.clone())
        );
        assert_eq!(packaged_release_root(&root.join("demo")), None);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ignores_incomplete_bin_layouts() {
        let root = temporary_directory("incomplete-root");
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::create_dir_all(root.join("public")).unwrap();

        assert_eq!(packaged_release_root(&root.join("bin/demo")), None);

        fs::remove_dir_all(root).unwrap();
    }

    fn temporary_directory(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "phoenix-console-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn command_receives_remaining_args() {
        #[allow(clippy::unused_async)]
        async fn echo(ctx: CommandContext<'_>) -> CommandResult {
            assert_eq!(ctx.binary_name, "demo");
            assert_eq!(ctx.args, ["a".to_owned(), "b".to_owned()]);
            Ok(())
        }

        let entry = CommandEntry::new("echo", |ctx| Box::pin(echo(ctx)));
        Console::new("demo")
            .commands([entry])
            .run_argv(["demo", "echo", "a", "b"])
            .await
            .expect("echo should succeed");
    }
}
