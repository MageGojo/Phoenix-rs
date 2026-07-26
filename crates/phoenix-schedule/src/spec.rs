//! Schedule specs and the pure `next_run` core.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::{
    civil::{SECS_PER_DAY, SECS_PER_MINUTE},
    cron::CronExpr,
    error::ScheduleError,
};

/// When a scheduled job should fire.
///
/// Build via the DSL constructors: [`every_seconds`], [`every_minutes`],
/// [`every_hours`], [`daily_at`], and [`cron`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Spec {
    /// Every `interval_secs`, aligned to multiples of the interval since the
    /// Unix epoch (so `every_minutes(30)` fires at `:00` and `:30`).
    Every {
        /// Interval in seconds (>= 1).
        interval_secs: u64,
    },
    /// Once a day at the given wall-clock time.
    DailyAt {
        /// Hour 0-23.
        hour: u8,
        /// Minute 0-59.
        minute: u8,
    },
    /// A five-field cron expression.
    Cron(CronExpr),
}

/// Run every `seconds` seconds (clamped to at least 1).
#[must_use]
pub const fn every_seconds(seconds: u64) -> Spec {
    Spec::Every {
        interval_secs: if seconds == 0 { 1 } else { seconds },
    }
}

/// Run every `minutes` minutes (clamped to at least 1 minute).
#[must_use]
pub const fn every_minutes(minutes: u64) -> Spec {
    every_seconds(minutes.saturating_mul(60))
}

/// Run every `hours` hours (clamped to at least 1 hour).
#[must_use]
pub const fn every_hours(hours: u64) -> Spec {
    every_seconds(hours.saturating_mul(3600))
}

/// Run once a day at `HH:MM` wall-clock time.
///
/// # Panics
///
/// Panics when `time` is not a valid `HH:MM`. Use [`try_daily_at`] for a
/// fallible variant.
#[must_use]
pub fn daily_at(time: &str) -> Spec {
    match try_daily_at(time) {
        Ok(spec) => spec,
        Err(error) => panic!("{error}"),
    }
}

/// Fallible variant of [`daily_at`].
///
/// # Errors
///
/// Returns [`ScheduleError::InvalidDailyTime`] when `time` is not `HH:MM`
/// with hour 0-23 and minute 0-59.
pub fn try_daily_at(time: &str) -> Result<Spec, ScheduleError> {
    let invalid = || ScheduleError::InvalidDailyTime {
        value: time.to_owned(),
    };
    let (hour_text, minute_text) = time.split_once(':').ok_or_else(invalid)?;
    let parse = |text: &str| -> Option<u8> {
        if text.is_empty() || text.len() > 2 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        text.parse::<u8>().ok()
    };
    let hour = parse(hour_text).ok_or_else(invalid)?;
    let minute = parse(minute_text).ok_or_else(invalid)?;
    if hour > 23 || minute > 59 {
        return Err(invalid());
    }
    Ok(Spec::DailyAt { hour, minute })
}

/// Run on a standard five-field cron expression (`分 时 日 月 周`).
///
/// # Panics
///
/// Panics when the expression is invalid. Use [`try_cron`] for a fallible
/// variant.
#[must_use]
pub fn cron(expr: &str) -> Spec {
    match try_cron(expr) {
        Ok(spec) => spec,
        Err(error) => panic!("{error}"),
    }
}

/// Fallible variant of [`cron`].
///
/// # Errors
///
/// Returns [`ScheduleError::InvalidCron`] when the expression cannot be
/// parsed; see [`CronExpr::parse`] for the supported syntax.
pub fn try_cron(expr: &str) -> Result<Spec, ScheduleError> {
    Ok(Spec::Cron(CronExpr::parse(expr)?))
}

/// Pure core: first instant **strictly after** `after` at which `spec`
/// fires, interpreting wall-clock specs (`daily_at`, cron) in UTC.
///
/// Returns `None` only for unreachable cron expressions (e.g. `0 0 30 2 *`);
/// interval and `daily_at` specs always have a next run.
#[must_use]
pub fn next_run(after: SystemTime, spec: &Spec) -> Option<SystemTime> {
    next_run_with_offset(after, spec, 0)
}

/// Like [`next_run`], but interprets wall-clock specs at a fixed offset of
/// `utc_offset_secs` seconds east of UTC (e.g. `8 * 3600` for UTC+8).
///
/// Interval specs ([`Spec::Every`]) are offset-independent.
#[must_use]
pub fn next_run_with_offset(
    after: SystemTime,
    spec: &Spec,
    utc_offset_secs: i32,
) -> Option<SystemTime> {
    let next = next_run_unix(unix_secs(after), spec, utc_offset_secs)?;
    Some(system_time_from_unix(next))
}

/// Unix-seconds version of [`next_run_with_offset`]; the actual pure core.
pub(crate) fn next_run_unix(after: i64, spec: &Spec, utc_offset_secs: i32) -> Option<i64> {
    match spec {
        Spec::Every { interval_secs } => {
            let interval = i64::try_from(*interval_secs).unwrap_or(i64::MAX).max(1);
            Some((after.div_euclid(interval) + 1).saturating_mul(interval))
        }
        Spec::DailyAt { hour, minute } => {
            let local = after + i64::from(utc_offset_secs);
            let day = local.div_euclid(SECS_PER_DAY);
            let target = i64::from(*hour) * 3600 + i64::from(*minute) * SECS_PER_MINUTE;
            let today = day * SECS_PER_DAY + target;
            let local_next = if today > local {
                today
            } else {
                today + SECS_PER_DAY
            };
            Some(local_next - i64::from(utc_offset_secs))
        }
        Spec::Cron(expr) => {
            let local = after + i64::from(utc_offset_secs);
            Some(expr.next_match(local)? - i64::from(utc_offset_secs))
        }
    }
}

/// Signed Unix seconds for a [`SystemTime`] (negative before the epoch).
pub(crate) fn unix_secs(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
        Err(error) => -i64::try_from(error.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

/// [`SystemTime`] for signed Unix seconds.
pub(crate) fn system_time_from_unix(secs: i64) -> SystemTime {
    if secs >= 0 {
        UNIX_EPOCH + Duration::from_secs(secs.unsigned_abs())
    } else {
        UNIX_EPOCH - Duration::from_secs(secs.unsigned_abs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::civil::unix_from_civil;

    #[test]
    fn every_aligns_to_epoch_multiples() {
        let spec = every_minutes(30);
        let after = unix_from_civil(2026, 7, 26, 10, 5);
        assert_eq!(
            next_run_unix(after, &spec, 0),
            Some(unix_from_civil(2026, 7, 26, 10, 30))
        );
        // Exactly on a boundary → strictly after → next slot.
        let boundary = unix_from_civil(2026, 7, 26, 10, 30);
        assert_eq!(
            next_run_unix(boundary, &spec, 0),
            Some(unix_from_civil(2026, 7, 26, 11, 0))
        );
    }

    #[test]
    fn every_seconds_clamps_zero_to_one() {
        assert_eq!(every_seconds(0), Spec::Every { interval_secs: 1 });
        assert_eq!(next_run_unix(41, &every_seconds(0), 0), Some(42));
    }

    #[test]
    fn every_ignores_utc_offset() {
        let spec = every_hours(1);
        let after = unix_from_civil(2026, 7, 26, 10, 5);
        assert_eq!(
            next_run_unix(after, &spec, 8 * 3600),
            next_run_unix(after, &spec, 0)
        );
    }

    #[test]
    fn daily_at_before_and_after_target() {
        let spec = daily_at("03:00");
        assert_eq!(
            next_run_unix(unix_from_civil(2026, 7, 26, 1, 30), &spec, 0),
            Some(unix_from_civil(2026, 7, 26, 3, 0))
        );
        // At exactly 03:00 → strictly after → tomorrow.
        assert_eq!(
            next_run_unix(unix_from_civil(2026, 7, 26, 3, 0), &spec, 0),
            Some(unix_from_civil(2026, 7, 27, 3, 0))
        );
        // One second past 03:00 → tomorrow.
        assert_eq!(
            next_run_unix(unix_from_civil(2026, 7, 26, 3, 0) + 1, &spec, 0),
            Some(unix_from_civil(2026, 7, 27, 3, 0))
        );
    }

    #[test]
    fn daily_at_crosses_month_and_year() {
        let spec = daily_at("00:30");
        assert_eq!(
            next_run_unix(unix_from_civil(2026, 2, 28, 1, 0), &spec, 0),
            Some(unix_from_civil(2026, 3, 1, 0, 30)),
            "2026 is not a leap year"
        );
        assert_eq!(
            next_run_unix(unix_from_civil(2028, 2, 28, 1, 0), &spec, 0),
            Some(unix_from_civil(2028, 2, 29, 0, 30)),
            "2028 is a leap year"
        );
        assert_eq!(
            next_run_unix(unix_from_civil(2026, 12, 31, 1, 0), &spec, 0),
            Some(unix_from_civil(2027, 1, 1, 0, 30))
        );
    }

    #[test]
    fn daily_at_with_utc_offset() {
        // 03:00 at UTC+8 is 19:00 UTC on the previous day.
        let spec = daily_at("03:00");
        assert_eq!(
            next_run_unix(unix_from_civil(2026, 7, 26, 12, 0), &spec, 8 * 3600),
            Some(unix_from_civil(2026, 7, 26, 19, 0))
        );
    }

    #[test]
    fn cron_spec_delegates_to_parser() {
        let spec = cron("0 3 * * *");
        assert_eq!(
            next_run_unix(unix_from_civil(2026, 7, 26, 2, 0), &spec, 0),
            Some(unix_from_civil(2026, 7, 26, 3, 0))
        );
        // Unreachable cron → None.
        assert_eq!(
            next_run_unix(unix_from_civil(2026, 1, 1, 0, 0), &cron("0 0 30 2 *"), 0),
            None
        );
    }

    #[test]
    fn cron_with_utc_offset() {
        // Daily 03:00 at UTC+8 → 19:00 UTC.
        assert_eq!(
            next_run_unix(
                unix_from_civil(2026, 7, 26, 12, 0),
                &cron("0 3 * * *"),
                8 * 3600
            ),
            Some(unix_from_civil(2026, 7, 26, 19, 0))
        );
        // Negative offset (UTC-5): 03:00 local = 08:00 UTC.
        assert_eq!(
            next_run_unix(
                unix_from_civil(2026, 7, 26, 0, 0),
                &cron("0 3 * * *"),
                -5 * 3600
            ),
            Some(unix_from_civil(2026, 7, 26, 8, 0))
        );
    }

    #[test]
    fn system_time_round_trip() {
        let now = SystemTime::now();
        let secs = unix_secs(now);
        let restored = system_time_from_unix(secs);
        let difference = now
            .duration_since(restored)
            .expect("truncation loses sub-second precision only");
        assert!(difference < Duration::from_secs(1));
    }

    #[test]
    fn next_run_public_wrapper() {
        let after = system_time_from_unix(unix_from_civil(2026, 7, 26, 2, 59));
        let next = next_run(after, &daily_at("03:00")).expect("daily always has a next run");
        assert_eq!(unix_secs(next), unix_from_civil(2026, 7, 26, 3, 0));
    }

    #[test]
    fn daily_at_parsing_edges() {
        assert!(try_daily_at("00:00").is_ok());
        assert!(try_daily_at("23:59").is_ok());
        assert!(try_daily_at("3:05").is_ok(), "single-digit hour is allowed");
        for bad in [
            "24:00", "12:60", "3", "03:0a", ":30", "03:", "003:00", "-1:00", "03:005",
        ] {
            assert!(try_daily_at(bad).is_err(), "`{bad}` should be rejected");
        }
    }

    #[test]
    #[should_panic(expected = "invalid cron expression")]
    fn cron_constructor_panics_on_invalid_input() {
        let _ = cron("not a cron");
    }

    #[test]
    #[should_panic(expected = "invalid daily time")]
    fn daily_at_constructor_panics_on_invalid_input() {
        let _ = daily_at("25:00");
    }
}
