//! Scheduler error types.

use thiserror::Error;

/// Errors produced while building schedule specs.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ScheduleError {
    /// The cron expression could not be parsed.
    #[error("invalid cron expression `{expr}`: {reason}")]
    InvalidCron {
        /// Original expression text.
        expr: String,
        /// Human-readable parse failure.
        reason: String,
    },
    /// The `daily_at` time was not a valid `HH:MM` wall-clock time.
    #[error("invalid daily time `{value}`: expected `HH:MM` between 00:00 and 23:59")]
    InvalidDailyTime {
        /// Original time text.
        value: String,
    },
}
