//! `schedule:run` / `schedule:work` as `phoenix-console` commands.

use std::{sync::Arc, time::SystemTime};

use phoenix_console::CommandEntry;
use phoenix_queue::ShutdownSignal;

use crate::schedule::Schedule;

/// Build the `schedule:run` and `schedule:work` console commands for an
/// application schedule.
///
/// Register them next to the app's own commands:
///
/// ```ignore
/// Console::new(env!("CARGO_PKG_NAME"))
///     .commands(commands::registry())
///     .commands(phoenix_schedule::console_commands(schedule))
///     .run()
///     .await
/// ```
///
/// - `schedule:run` executes one round of due jobs and exits — pair it with
///   an external crontab entry that fires every minute (`px schedule:run`
///   forwards here).
/// - `schedule:work` stays resident, checking at least once per minute, and
///   shuts down gracefully on Ctrl-C via the `phoenix-queue`
///   [`ShutdownSignal`] mechanism.
#[must_use]
pub fn console_commands(schedule: Arc<Schedule>) -> Vec<CommandEntry> {
    let run_schedule = Arc::clone(&schedule);
    let run = CommandEntry::new("schedule:run", move |_ctx| {
        let schedule = Arc::clone(&run_schedule);
        Box::pin(async move {
            let summary = schedule.run_due(SystemTime::now()).await;
            println!(
                "schedule:run — due: {}, completed: {}, failed: {}, skipped (overlap): {}",
                summary.due, summary.completed, summary.failed, summary.skipped
            );
            Ok(())
        })
    });

    let work = CommandEntry::new("schedule:work", move |_ctx| {
        let schedule = Arc::clone(&schedule);
        Box::pin(async move {
            let signal = ShutdownSignal::new();
            let token = signal.token();
            tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    signal.shutdown();
                }
            });
            println!(
                "schedule:work — {} job(s) registered; checking at least once per minute (Ctrl-C to stop)",
                schedule.len()
            );
            schedule.work(token).await;
            println!("schedule:work — stopped");
            Ok(())
        })
    });

    vec![run, work]
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use phoenix_console::Console;

    use super::*;
    use crate::spec::every_seconds;

    #[tokio::test]
    async fn schedule_run_executes_due_jobs_via_console() {
        let ran = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&ran);
        let schedule = Arc::new(Schedule::new().job("tick", every_seconds(60), move || {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
            }
        }));

        Console::new("demo")
            .commands(console_commands(schedule))
            .run_argv(["demo", "schedule:run"])
            .await
            .expect("schedule:run succeeds");
        assert_eq!(ran.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn commands_are_named_after_laravel_counterparts() {
        let schedule = Arc::new(Schedule::new());
        let entries = console_commands(schedule);
        let names = entries.iter().map(CommandEntry::name).collect::<Vec<_>>();
        assert_eq!(names, ["schedule:run", "schedule:work"]);
    }
}
