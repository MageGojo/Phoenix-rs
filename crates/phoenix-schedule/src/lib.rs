//! Laravel-style task scheduler for Phoenix.
//!
//! Declare jobs with a fluent DSL, then run them either one round at a time
//! (`schedule:run`, driven by an external crontab) or with a resident loop
//! (`schedule:work`):
//!
//! ```
//! use phoenix_schedule::{Schedule, cron, daily_at, every_minutes};
//!
//! let schedule = Schedule::new()
//!     .job("sitemap", every_minutes(30), || async {
//!         // regenerate the sitemap …
//!     })
//!     .job("nightly-report", cron("0 3 * * *"), || async {
//!         Ok::<(), std::io::Error>(())
//!     })
//!     .job("digest", daily_at("08:30"), || async {});
//! assert_eq!(schedule.len(), 3);
//! ```
//!
//! The pure core is [`next_run`]: given an instant and a [`Spec`], it returns
//! the next fire time with no side effects. Cron expressions are parsed by a
//! self-contained five-field parser ([`cron`] / [`try_cron`]) supporting
//! `*`, numbers, `,` lists, `-` ranges, and `/` steps, with Sunday accepted
//! as both `0` and `7`.
//!
//! Overlap protection defaults to **per process**: a job whose previous
//! execution is still running is skipped (logged via `tracing`). Inject a
//! distributed [`ScheduleLock`] with [`Schedule::with_lock`] (e.g.
//! `phoenix_redis::RedisScheduleLock`) to make overlap protection span
//! processes / machines. See `docs/SCHEDULE.md`.

#![forbid(unsafe_code)]

mod civil;
mod commands;
mod cron;
mod error;
mod lock;
mod schedule;
mod spec;

pub use commands::console_commands;
pub use cron::CronExpr;
pub use error::ScheduleError;
pub use lock::{BoxLockFuture, InProcessLock, LockGuard, ScheduleLock};
// Re-exported so applications can share one graceful-shutdown mechanism
// between queue workers and the scheduler.
pub use phoenix_queue::{ShutdownSignal, ShutdownToken};
pub use schedule::{
    BoxTaskFuture, DEFAULT_LOCK_TTL, RunSummary, Schedule, ScheduledTask, TaskOutcome, TaskResult,
};
pub use spec::{
    Spec, cron, daily_at, every_hours, every_minutes, every_seconds, next_run,
    next_run_with_offset, try_cron, try_daily_at,
};

/// Crate identity helper, mirroring the other Phoenix crates.
#[must_use]
pub const fn crate_name() -> &'static str {
    "phoenix-schedule"
}
