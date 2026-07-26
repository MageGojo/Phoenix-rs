//! Pure proleptic-Gregorian calendar math (no external time crates).
//!
//! Days are counted from the Unix epoch (1970-01-01). The conversion
//! algorithms follow Howard Hinnant's `days_from_civil` / `civil_from_days`
//! and are exact for the whole `i64` day range used here.

/// Seconds per minute.
pub(crate) const SECS_PER_MINUTE: i64 = 60;
/// Seconds per day.
pub(crate) const SECS_PER_DAY: i64 = 86_400;

/// Whether `year` is a leap year in the Gregorian calendar.
#[cfg(test)]
#[must_use]
pub(crate) const fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Number of days in `month` (1-12) of `year`.
#[cfg(test)]
#[must_use]
pub(crate) const fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Days since 1970-01-01 for a civil date.
#[cfg(test)]
#[must_use]
pub(crate) fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = (i64::from(month) + 9) % 12;
    let day_of_year = (153 * month_prime + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Civil `(year, month, day)` for days since 1970-01-01.
#[must_use]
pub(crate) fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = year + i64::from(month <= 2);
    (
        i32::try_from(year).unwrap_or(if year > 0 { i32::MAX } else { i32::MIN }),
        u8::try_from(month).unwrap_or(1),
        u8::try_from(day).unwrap_or(1),
    )
}

/// Weekday for days since 1970-01-01, with `0 = Sunday` .. `6 = Saturday`.
///
/// 1970-01-01 was a Thursday (`4`).
#[must_use]
pub(crate) fn weekday_from_days(days: i64) -> u8 {
    u8::try_from((days + 4).rem_euclid(7)).unwrap_or(0)
}

/// Unix seconds for a civil wall-clock instant.
#[cfg(test)]
#[must_use]
pub(crate) fn unix_from_civil(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> i64 {
    days_from_civil(year, month, day) * SECS_PER_DAY
        + i64::from(hour) * 3600
        + i64::from(minute) * SECS_PER_MINUTE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_day_zero_and_thursday() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(weekday_from_days(0), 4);
    }

    #[test]
    fn roundtrips_across_leap_boundaries() {
        for &(year, month, day) in &[
            (2000, 2, 29),
            (2024, 2, 29),
            (2024, 3, 1),
            (2025, 2, 28),
            (2100, 2, 28),
            (2100, 3, 1),
            (1999, 12, 31),
            (2038, 1, 19),
        ] {
            let days = days_from_civil(year, month, day);
            assert_eq!(civil_from_days(days), (year, month, day));
        }
    }

    #[test]
    fn leap_year_rules() {
        assert!(is_leap_year(2000));
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(1900));
        assert!(!is_leap_year(2100));
        assert!(!is_leap_year(2025));
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2025, 2), 28);
        assert_eq!(days_in_month(2100, 2), 28);
        assert_eq!(days_in_month(2025, 4), 30);
        assert_eq!(days_in_month(2025, 12), 31);
    }

    #[test]
    fn known_weekdays() {
        // 2026-07-26 is a Sunday.
        assert_eq!(weekday_from_days(days_from_civil(2026, 7, 26)), 0);
        // 2024-02-29 is a Thursday.
        assert_eq!(weekday_from_days(days_from_civil(2024, 2, 29)), 4);
        // 2000-01-01 is a Saturday.
        assert_eq!(weekday_from_days(days_from_civil(2000, 1, 1)), 6);
    }

    #[test]
    fn unix_from_civil_matches_known_timestamps() {
        assert_eq!(unix_from_civil(1970, 1, 1, 0, 0), 0);
        assert_eq!(unix_from_civil(1970, 1, 2, 0, 1), 86_460);
        // 2026-07-26 00:00:00 UTC.
        assert_eq!(unix_from_civil(2026, 7, 26, 0, 0), 1_785_024_000);
    }
}
