//! Self-contained five-field cron expression parser and matcher.
//!
//! Supported syntax per field (`分 时 日 月 周`):
//!
//! - `*` — every value
//! - `N` — a single value
//! - `A-B` — an inclusive range
//! - `*/S`, `A-B/S`, `A/S` — steps (`A/S` means `A..=max` stepping by `S`)
//! - `X,Y,Z` — comma-separated list of any of the above
//!
//! Day-of-week accepts both `0` and `7` for Sunday. Month / weekday names are
//! **not** supported. When both day-of-month and day-of-week are restricted
//! (neither is `*`), a day matches when **either** field matches — standard
//! Vixie cron semantics.

use crate::{
    civil::{SECS_PER_DAY, SECS_PER_MINUTE, civil_from_days, weekday_from_days},
    error::ScheduleError,
};

/// Upper bound (in days) for the next-match search.
///
/// The rarest reachable schedule is Feb 29: after 2096-02-29 the next leap
/// year is 2104 (2100 is not a leap year), an eight-year gap. Nine years of
/// days covers every reachable expression; anything further is unreachable
/// (e.g. `0 0 30 2 *`).
const MAX_SEARCH_DAYS: i64 = 366 * 9;

/// Parsed five-field cron expression (minute precision).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CronExpr {
    /// Bitmask over minutes 0-59.
    minutes: u64,
    /// Bitmask over hours 0-23.
    hours: u64,
    /// Bitmask over days-of-month 1-31.
    days_of_month: u64,
    /// Bitmask over months 1-12.
    months: u64,
    /// Bitmask over weekdays 0-6 (`0 = Sunday`; `7` folded onto `0`).
    days_of_week: u64,
    /// Whether the day-of-month field was something other than `*`.
    dom_restricted: bool,
    /// Whether the day-of-week field was something other than `*`.
    dow_restricted: bool,
}

impl CronExpr {
    /// Parse a standard five-field cron expression.
    ///
    /// # Errors
    ///
    /// Returns [`ScheduleError::InvalidCron`] when the expression does not
    /// have exactly five whitespace-separated fields or a field contains
    /// out-of-range values, empty list items, reversed ranges, a zero step,
    /// or unsupported characters.
    pub fn parse(expr: &str) -> Result<Self, ScheduleError> {
        let invalid = |reason: &str| ScheduleError::InvalidCron {
            expr: expr.to_owned(),
            reason: reason.to_owned(),
        };

        let fields = expr.split_whitespace().collect::<Vec<_>>();
        let [minute, hour, dom, month, dow] = fields.as_slice() else {
            return Err(invalid("expected exactly 5 fields (分 时 日 月 周)"));
        };

        let minutes = parse_field(minute, 0, 59).map_err(|reason| invalid(&reason))?;
        let hours = parse_field(hour, 0, 23).map_err(|reason| invalid(&reason))?;
        let days_of_month = parse_field(dom, 1, 31).map_err(|reason| invalid(&reason))?;
        let months = parse_field(month, 1, 12).map_err(|reason| invalid(&reason))?;
        // Day-of-week allows 0-7; bit 7 (Sunday) is folded onto bit 0 below.
        let days_of_week = parse_field(dow, 0, 7).map_err(|reason| invalid(&reason))?;
        let days_of_week = fold_sunday(days_of_week);

        Ok(Self {
            minutes,
            hours,
            days_of_month,
            months,
            days_of_week,
            dom_restricted: *dom != "*",
            dow_restricted: *dow != "*",
        })
    }

    /// Whether the civil day `(year, month, day)` satisfies the date fields.
    fn day_matches(self, days_since_epoch: i64, month: u8, day: u8) -> bool {
        if self.months & (1 << month) == 0 {
            return false;
        }
        let by_day_of_month = self.days_of_month & (1 << day) != 0;
        let by_weekday = self.days_of_week & (1 << weekday_from_days(days_since_epoch)) != 0;
        match (self.dom_restricted, self.dow_restricted) {
            (false, false) => true,
            (true, false) => by_day_of_month,
            (false, true) => by_weekday,
            // Vixie cron: union when both fields are restricted.
            (true, true) => by_day_of_month || by_weekday,
        }
    }

    /// First wall-clock time (Unix seconds, minute-aligned) strictly after
    /// `after` that matches, or `None` when the expression is unreachable
    /// (e.g. `0 0 30 2 *`).
    #[must_use]
    pub(crate) fn next_match(self, after: i64) -> Option<i64> {
        // First minute boundary strictly after `after`.
        let start = (after.div_euclid(SECS_PER_MINUTE) + 1) * SECS_PER_MINUTE;
        let start_day = start.div_euclid(SECS_PER_DAY);
        let start_minute_of_day = (start - start_day * SECS_PER_DAY) / SECS_PER_MINUTE;

        for offset in 0..=MAX_SEARCH_DAYS {
            let day_index = start_day + offset;
            let (_, month, day) = civil_from_days(day_index);
            if !self.day_matches(day_index, month, day) {
                continue;
            }
            let earliest = if offset == 0 { start_minute_of_day } else { 0 };
            if let Some(minute_of_day) = self.first_minute_of_day(earliest) {
                return Some(day_index * SECS_PER_DAY + minute_of_day * SECS_PER_MINUTE);
            }
        }
        None
    }

    /// Smallest matching minute-of-day `>= earliest`, if any.
    fn first_minute_of_day(self, earliest: i64) -> Option<i64> {
        for hour in 0..24_i64 {
            if self.hours & (1 << hour) == 0 || hour * 60 + 59 < earliest {
                continue;
            }
            for minute in 0..60_i64 {
                if self.minutes & (1 << minute) != 0 && hour * 60 + minute >= earliest {
                    return Some(hour * 60 + minute);
                }
            }
        }
        None
    }
}

/// Fold day-of-week bit 7 (alternate Sunday) onto bit 0.
const fn fold_sunday(mask: u64) -> u64 {
    (mask & 0x7F) | ((mask >> 7) & 1)
}

/// Parse one cron field into a bitmask over `min..=max`.
fn parse_field(text: &str, min: u8, max: u8) -> Result<u64, String> {
    if text.is_empty() {
        return Err("empty field".to_owned());
    }
    let mut mask = 0_u64;
    for item in text.split(',') {
        mask |= parse_item(item, min, max)?;
    }
    Ok(mask)
}

/// Parse one comma-separated item: `*`, `N`, `A-B`, optionally `/S`.
fn parse_item(item: &str, min: u8, max: u8) -> Result<u64, String> {
    if item.is_empty() {
        return Err("empty list item (double comma?)".to_owned());
    }

    let (base, step) = match item.split_once('/') {
        Some((base, step_text)) => {
            let step = parse_number(step_text, "step")?;
            if step == 0 {
                return Err(format!("step must be >= 1 in `{item}`"));
            }
            (base, step)
        }
        None => (item, 1),
    };

    let (start, end) = if base == "*" {
        (min, max)
    } else if let Some((low, high)) = base.split_once('-') {
        let low = parse_number(low, "range start")?;
        let high = parse_number(high, "range end")?;
        if low > high {
            return Err(format!("range start exceeds range end in `{item}`"));
        }
        (low, high)
    } else {
        let value = parse_number(base, "value")?;
        // `N/S` (Vixie extension) means `N..=max` stepping by `S`.
        if step > 1 {
            (value, max)
        } else {
            (value, value)
        }
    };

    if start < min || end > max {
        return Err(format!(
            "value out of range in `{item}` (allowed {min}-{max})"
        ));
    }

    let mut mask = 0_u64;
    let mut value = start;
    loop {
        mask |= 1 << value;
        match value.checked_add(step) {
            Some(next) if next <= end => value = next,
            _ => break,
        }
    }
    Ok(mask)
}

fn parse_number(text: &str, what: &str) -> Result<u8, String> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("invalid {what} `{text}` (digits only)"));
    }
    text.parse::<u8>()
        .map_err(|_| format!("invalid {what} `{text}` (too large)"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::civil::unix_from_civil;

    fn expr(text: &str) -> CronExpr {
        CronExpr::parse(text).expect("valid cron expression")
    }

    fn next_civil(text: &str, after: (i32, u8, u8, u8, u8)) -> Option<(i32, u8, u8, u8, u8)> {
        let (year, month, day, hour, minute) = after;
        let after = unix_from_civil(year, month, day, hour, minute);
        let next = expr(text).next_match(after)?;
        let days = next.div_euclid(SECS_PER_DAY);
        let (year, month, day) = civil_from_days(days);
        let minute_of_day = (next - days * SECS_PER_DAY) / SECS_PER_MINUTE;
        Some((
            year,
            month,
            day,
            u8::try_from(minute_of_day / 60).unwrap(),
            u8::try_from(minute_of_day % 60).unwrap(),
        ))
    }

    #[test]
    fn daily_at_three() {
        assert_eq!(
            next_civil("0 3 * * *", (2026, 7, 26, 2, 59)),
            Some((2026, 7, 26, 3, 0))
        );
        assert_eq!(
            next_civil("0 3 * * *", (2026, 7, 26, 3, 0)),
            Some((2026, 7, 27, 3, 0)),
            "next match is strictly after `after`"
        );
    }

    #[test]
    fn every_fifteen_minutes() {
        assert_eq!(
            next_civil("*/15 * * * *", (2026, 7, 26, 10, 0)),
            Some((2026, 7, 26, 10, 15))
        );
        assert_eq!(
            next_civil("*/15 * * * *", (2026, 7, 26, 10, 46)),
            Some((2026, 7, 26, 11, 0))
        );
        assert_eq!(
            next_civil("*/15 * * * *", (2026, 7, 26, 23, 46)),
            Some((2026, 7, 27, 0, 0))
        );
    }

    #[test]
    fn seconds_are_rounded_up_to_the_next_minute() {
        // 10:00:30 → the 10:00 slot has already begun; next match is 10:01.
        let after = unix_from_civil(2026, 7, 26, 10, 0) + 30;
        let next = expr("* * * * *").next_match(after).unwrap();
        assert_eq!(next, unix_from_civil(2026, 7, 26, 10, 1));
    }

    #[test]
    fn month_rollover_to_a_thirty_one_day_month() {
        // April has no 31st: from Apr 1 the next 31st is May 31.
        assert_eq!(
            next_civil("0 0 31 * *", (2026, 4, 1, 0, 0)),
            Some((2026, 5, 31, 0, 0))
        );
    }

    #[test]
    fn year_rollover() {
        assert_eq!(
            next_civil("0 0 1 1 *", (2026, 6, 15, 12, 0)),
            Some((2027, 1, 1, 0, 0))
        );
        assert_eq!(
            next_civil("30 23 31 12 *", (2026, 12, 31, 23, 30)),
            Some((2027, 12, 31, 23, 30))
        );
    }

    #[test]
    fn leap_day_waits_for_a_leap_year() {
        assert_eq!(
            next_civil("0 0 29 2 *", (2025, 3, 1, 0, 0)),
            Some((2028, 2, 29, 0, 0))
        );
        // Century rule: after 2096-02-29 the next leap year is 2104.
        assert_eq!(
            next_civil("0 0 29 2 *", (2096, 3, 1, 0, 0)),
            Some((2104, 2, 29, 0, 0))
        );
    }

    #[test]
    fn unreachable_expressions_return_none() {
        assert_eq!(next_civil("0 0 30 2 *", (2026, 1, 1, 0, 0)), None);
        assert_eq!(next_civil("0 0 31 4 *", (2026, 1, 1, 0, 0)), None);
        assert_eq!(next_civil("0 0 31 2,4,6,9,11 *", (2026, 1, 1, 0, 0)), None);
    }

    #[test]
    fn sunday_zero_and_seven_are_equivalent() {
        let by_zero = expr("0 9 * * 0");
        let by_seven = expr("0 9 * * 7");
        let after = unix_from_civil(2026, 7, 20, 0, 0); // Monday
        assert_eq!(by_zero.next_match(after), by_seven.next_match(after));
        // 2026-07-26 is a Sunday.
        assert_eq!(
            next_civil("0 9 * * 7", (2026, 7, 20, 0, 0)),
            Some((2026, 7, 26, 9, 0))
        );
    }

    #[test]
    fn weekday_range_business_hours() {
        // Friday 17:00 → next is Monday 09:00 (2026-07-24 is a Friday).
        assert_eq!(
            next_civil("0 9-17 * * 1-5", (2026, 7, 24, 17, 0)),
            Some((2026, 7, 27, 9, 0))
        );
        assert_eq!(
            next_civil("0 9-17 * * 1-5", (2026, 7, 24, 9, 30)),
            Some((2026, 7, 24, 10, 0))
        );
    }

    #[test]
    fn dom_and_dow_are_a_union_when_both_restricted() {
        // "0 0 13 * 5": the 13th of the month OR any Friday.
        // 2026-07-26 is a Sunday; the next Friday is 2026-07-31.
        assert_eq!(
            next_civil("0 0 13 * 5", (2026, 7, 26, 0, 0)),
            Some((2026, 7, 31, 0, 0))
        );
        // From Aug 12 the day-of-month leg wins (Aug 13 is a Thursday).
        assert_eq!(
            next_civil("0 0 13 * 5", (2026, 8, 12, 0, 0)),
            Some((2026, 8, 13, 0, 0))
        );
    }

    #[test]
    fn dow_only_restriction_ignores_dom() {
        // Plain "* * * * 1" must not fire on non-Mondays.
        assert_eq!(
            next_civil("0 0 * * 1", (2026, 7, 26, 0, 30)),
            Some((2026, 7, 27, 0, 0))
        );
    }

    #[test]
    fn lists_ranges_and_value_steps() {
        assert_eq!(
            next_civil("0 0 1,15 * *", (2026, 7, 2, 0, 0)),
            Some((2026, 7, 15, 0, 0))
        );
        // `10/20` = 10,30,50.
        assert_eq!(
            next_civil("10/20 * * * *", (2026, 7, 26, 10, 31)),
            Some((2026, 7, 26, 10, 50))
        );
        assert_eq!(
            next_civil("0-30/10 * * * *", (2026, 7, 26, 10, 21)),
            Some((2026, 7, 26, 10, 30))
        );
        assert_eq!(
            next_civil("0-30/10 * * * *", (2026, 7, 26, 10, 31)),
            Some((2026, 7, 26, 11, 0))
        );
    }

    #[test]
    fn parse_rejects_bad_expressions() {
        for bad in [
            "",
            "* * * *",
            "* * * * * *",
            "60 * * * *",
            "* 24 * * *",
            "* * 0 * *",
            "* * 32 * *",
            "* * * 13 *",
            "* * * * 8",
            "*/0 * * * *",
            "5-1 * * * *",
            "1,,2 * * * *",
            "a * * * *",
            "1.5 * * * *",
            "-5 * * * *",
            "* * * * mon",
        ] {
            assert!(
                CronExpr::parse(bad).is_err(),
                "`{bad}` should fail to parse"
            );
        }
    }

    #[test]
    fn parse_accepts_boundary_values() {
        assert!(CronExpr::parse("59 23 31 12 7").is_ok());
        assert!(CronExpr::parse("0 0 1 1 0").is_ok());
        assert!(CronExpr::parse("*/1 */1 */1 */1 */1").is_ok());
    }
}
