//! Deterministic value generation for factories.
//!
//! Self-contained on purpose: the word banks are small, the generator is a
//! seedable `SplitMix64`, and there is no dependency to keep current. What
//! matters for test data is that values look plausible, that uniqueness holds
//! where a column demands it, and that a failing seed can be replayed —
//! [`Faker::with_seed`] gives the last one.

/// Which language generated names and words are drawn from.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Locale {
    /// English names and lorem-style words.
    #[default]
    En,
    /// Simplified Chinese names, with English lorem for prose.
    ZhCn,
}

const EN_FIRST_NAMES: [&str; 16] = [
    "Ada", "Alan", "Grace", "Linus", "Barbara", "Ken", "Margaret", "Dennis", "Radia", "Bjarne",
    "Anita", "Guido", "Frances", "James", "Karen", "Niklaus",
];
const EN_LAST_NAMES: [&str; 16] = [
    "Lovelace",
    "Turing",
    "Hopper",
    "Torvalds",
    "Liskov",
    "Thompson",
    "Hamilton",
    "Ritchie",
    "Perlman",
    "Stroustrup",
    "Borg",
    "Rossum",
    "Allen",
    "Gosling",
    "Uhlenbeck",
    "Wirth",
];
const ZH_SURNAMES: [&str; 16] = [
    "王", "李", "张", "刘", "陈", "杨", "黄", "赵", "周", "吴", "徐", "孙", "马", "朱", "胡", "林",
];
const ZH_GIVEN_NAMES: [&str; 16] = [
    "伟", "芳", "娜", "敏", "静", "磊", "洋", "艳", "勇", "军", "杰", "娟", "涛", "明", "超",
    "秀英",
];
const WORDS: [&str; 24] = [
    "alpha", "beacon", "cadence", "delta", "ember", "forge", "gradient", "harbor", "index",
    "juniper", "kernel", "lattice", "meridian", "nimbus", "orbit", "pivot", "quartz", "ripple",
    "summit", "tangent", "umbra", "vector", "willow", "zenith",
];
const DOMAINS: [&str; 4] = ["example.com", "example.org", "test.local", "example.net"];

/// Seedable generator of plausible test values.
///
/// Cloning is deliberate: a clone continues the same sequence independently,
/// which is occasionally useful and never surprising, since nothing here is
/// meant to be cryptographically anything.
#[derive(Clone, Debug)]
pub struct Faker {
    state: u64,
    locale: Locale,
    /// Monotonic counter behind the `unique_*` helpers.
    sequence: u64,
}

impl Default for Faker {
    fn default() -> Self {
        Self::new()
    }
}

impl Faker {
    /// A faker seeded from the clock — different data on every run.
    #[must_use]
    pub fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_nanos() & u128::from(u64::MAX)).unwrap_or(0)
            });
        Self::with_seed(nanos ^ u64::from(std::process::id()))
    }

    /// A faker with a fixed seed — the same data every run.
    ///
    /// Use this when a seeded fixture has to be reproducible, or to replay the
    /// exact data a failing test ran against.
    #[must_use]
    pub const fn with_seed(seed: u64) -> Self {
        Self {
            state: seed,
            locale: Locale::En,
            sequence: 0,
        }
    }

    /// Draw values from a different language.
    pub const fn set_locale(&mut self, locale: Locale) {
        self.locale = locale;
    }

    /// Builder form of [`Self::set_locale`].
    #[must_use]
    pub const fn locale(mut self, locale: Locale) -> Self {
        self.locale = locale;
        self
    }

    /// The next raw value in the sequence.
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `low..=high`. A reversed range is treated as `low..=low`.
    pub fn int_between(&mut self, low: i64, high: i64) -> i64 {
        if high <= low {
            return low;
        }
        let span = high.abs_diff(low).saturating_add(1);
        let offset = i64::try_from(self.next_u64() % span).unwrap_or(0);
        low.saturating_add(offset)
    }

    /// `true` roughly half the time.
    pub fn boolean(&mut self) -> bool {
        self.next_u64() % 2 == 0
    }

    /// One of `options`.
    ///
    /// # Panics
    ///
    /// Panics when `options` is empty — there is nothing meaningful to return,
    /// and a silent default would be a worse surprise inside a fixture.
    pub fn pick<'a, T>(&mut self, options: &'a [T]) -> &'a T {
        assert!(!options.is_empty(), "Faker::pick needs at least one option");
        let index = usize::try_from(self.next_u64() % options.len() as u64).unwrap_or(0);
        &options[index]
    }

    /// A lowercase word.
    pub fn word(&mut self) -> String {
        (*self.pick(&WORDS)).to_owned()
    }

    /// A given name in the configured locale.
    pub fn first_name(&mut self) -> String {
        match self.locale {
            Locale::En => (*self.pick(&EN_FIRST_NAMES)).to_owned(),
            Locale::ZhCn => (*self.pick(&ZH_GIVEN_NAMES)).to_owned(),
        }
    }

    /// A family name in the configured locale.
    pub fn last_name(&mut self) -> String {
        match self.locale {
            Locale::En => (*self.pick(&EN_LAST_NAMES)).to_owned(),
            Locale::ZhCn => (*self.pick(&ZH_SURNAMES)).to_owned(),
        }
    }

    /// A full name, ordered for the locale.
    pub fn name(&mut self) -> String {
        match self.locale {
            Locale::En => format!("{} {}", self.first_name(), self.last_name()),
            // Chinese names put the family name first, with no separator.
            Locale::ZhCn => format!("{}{}", self.last_name(), self.first_name()),
        }
    }

    /// A lowercase ASCII handle, safe for a username column in any locale.
    pub fn username(&mut self) -> String {
        let word = self.word();
        let number = self.int_between(10, 9999);
        format!("{word}{number}")
    }

    /// An email address. **May repeat** — use [`Self::unique_email`] for a
    /// column with a unique index.
    pub fn email(&mut self) -> String {
        let user = self.username();
        let domain = *self.pick(&DOMAINS);
        format!("{user}@{domain}")
    }

    /// An email address that never repeats within this faker.
    ///
    /// Uniqueness comes from a monotonic counter, not from randomness: with
    /// random local parts a few thousand rows will collide, and the failure
    /// shows up as a confusing unique-constraint error mid-seed.
    pub fn unique_email(&mut self) -> String {
        self.sequence += 1;
        let sequence = self.sequence;
        let word = self.word();
        let domain = *self.pick(&DOMAINS);
        format!("{word}{sequence}@{domain}")
    }

    /// A value that never repeats within this faker, with `prefix` in front.
    pub fn unique(&mut self, prefix: &str) -> String {
        self.sequence += 1;
        format!("{prefix}{}", self.sequence)
    }

    /// A capitalized sentence of roughly `words` words.
    pub fn sentence(&mut self, words: usize) -> String {
        let count = words.clamp(1, 64);
        let mut sentence = String::new();
        for index in 0..count {
            if index > 0 {
                sentence.push(' ');
            }
            let word = self.word();
            if index == 0 {
                let mut characters = word.chars();
                if let Some(first) = characters.next() {
                    sentence.extend(first.to_uppercase());
                    sentence.push_str(characters.as_str());
                }
            } else {
                sentence.push_str(&word);
            }
        }
        sentence.push('.');
        sentence
    }

    /// A paragraph of roughly `sentences` sentences.
    pub fn paragraph(&mut self, sentences: usize) -> String {
        let count = sentences.clamp(1, 32);
        let mut paragraph = String::new();
        for index in 0..count {
            if index > 0 {
                paragraph.push(' ');
            }
            let length = usize::try_from(self.int_between(6, 14)).unwrap_or(8);
            paragraph.push_str(&self.sentence(length));
        }
        paragraph
    }

    /// A URL on an example domain.
    pub fn url(&mut self) -> String {
        let domain = *self.pick(&DOMAINS);
        let path = self.word();
        format!("https://{domain}/{path}")
    }

    /// A mainland-China mobile number, always in a reserved test range.
    pub fn phone_cn(&mut self) -> String {
        // 138 0013 xxxx is inside the documentation range, so generated data
        // can never reach a real handset.
        format!("1380013{:04}", self.int_between(0, 9999))
    }

    /// An RFC 3339 timestamp within the last `days`, at second resolution.
    pub fn recent_datetime(&mut self, days: i64) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(0));
        let span = days.max(0).saturating_mul(86_400);
        let seconds = now.saturating_sub(self.int_between(0, span));
        format_rfc3339(seconds)
    }
}

/// Format Unix seconds as `YYYY-MM-DDTHH:MM:SSZ` with integer date math only.
fn format_rfc3339(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let time = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        (time % 3600) / 60,
        time % 60
    )
}

/// Days since the Unix epoch to a civil date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = u32::try_from(day_of_year - (153 * shifted_month + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    })
    .unwrap_or(1);
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_replays_the_same_data() {
        let mut left = Faker::with_seed(42);
        let mut right = Faker::with_seed(42);
        for _ in 0..32 {
            assert_eq!(left.name(), right.name());
            assert_eq!(left.unique_email(), right.unique_email());
            assert_eq!(left.int_between(1, 1_000), right.int_between(1, 1_000));
        }

        let mut other = Faker::with_seed(43);
        assert_ne!(
            Faker::with_seed(42).sentence(8),
            other.sentence(8),
            "a different seed gives different data"
        );
    }

    #[test]
    fn unique_values_do_not_repeat() {
        let mut faker = Faker::with_seed(7);
        let emails: std::collections::HashSet<String> =
            (0..5_000).map(|_| faker.unique_email()).collect();
        assert_eq!(emails.len(), 5_000, "a unique column must not collide");

        let mut faker = Faker::with_seed(7);
        let codes: std::collections::HashSet<String> =
            (0..1_000).map(|_| faker.unique("SKU-")).collect();
        assert_eq!(codes.len(), 1_000);
        assert!(codes.iter().all(|code| code.starts_with("SKU-")));
    }

    #[test]
    fn ranges_are_inclusive_and_survive_bad_input() {
        let mut faker = Faker::with_seed(1);
        for _ in 0..500 {
            let value = faker.int_between(5, 7);
            assert!((5..=7).contains(&value), "got {value}");
        }
        assert_eq!(faker.int_between(3, 3), 3);
        assert_eq!(
            faker.int_between(9, 2),
            9,
            "a reversed range yields its low"
        );
        // Extreme bounds must not overflow.
        let value = faker.int_between(i64::MIN, i64::MAX);
        assert!(value >= i64::MIN);
    }

    #[test]
    fn locales_change_the_shape_of_a_name() {
        let mut english = Faker::with_seed(3);
        let name = english.name();
        assert!(name.contains(' '), "English names are `First Last`: {name}");
        assert!(name.is_ascii());

        let mut chinese = Faker::with_seed(3).locale(Locale::ZhCn);
        let name = chinese.name();
        assert!(
            !name.contains(' '),
            "Chinese names have no separator: {name}"
        );
        assert!(!name.is_ascii());
        assert!(name.chars().count() <= 4, "{name}");

        // Usernames and emails stay ASCII whatever the locale, because the
        // columns they land in usually are.
        assert!(chinese.username().is_ascii());
        assert!(chinese.unique_email().is_ascii());
    }

    #[test]
    fn prose_has_the_requested_shape() {
        let mut faker = Faker::with_seed(11);
        let sentence = faker.sentence(5);
        assert_eq!(sentence.split(' ').count(), 5, "{sentence}");
        assert!(sentence.ends_with('.'), "{sentence}");
        assert!(
            sentence.chars().next().is_some_and(char::is_uppercase),
            "{sentence}"
        );
        // Absurd inputs are clamped rather than allocating forever.
        assert!(faker.sentence(0).split(' ').count() == 1);
        assert!(faker.paragraph(0).ends_with('.'));
        assert!(faker.paragraph(3).matches('.').count() == 3);
    }

    #[test]
    fn timestamps_are_rfc3339_and_within_the_window() {
        assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339(1_785_000_000), "2026-07-25T17:20:00Z");
        // A leap day, to check the calendar math rather than the formatting.
        assert_eq!(format_rfc3339(1_709_164_800), "2024-02-29T00:00:00Z");

        let mut faker = Faker::with_seed(5);
        let stamp = faker.recent_datetime(30);
        assert_eq!(stamp.len(), 20, "{stamp}");
        assert!(stamp.ends_with('Z'), "{stamp}");
        assert!(stamp.contains('T'), "{stamp}");
    }

    #[test]
    fn generated_contact_details_cannot_reach_anyone_real() {
        let mut faker = Faker::with_seed(9);
        for _ in 0..100 {
            let email = faker.unique_email();
            assert!(
                DOMAINS.iter().any(|domain| email.ends_with(domain)),
                "generated mail must stay in reserved domains: {email}"
            );
            let phone = faker.phone_cn();
            assert!(
                phone.starts_with("1380013"),
                "generated numbers must stay in the documentation range: {phone}"
            );
            assert_eq!(phone.len(), 11);
            let url = faker.url();
            assert!(
                DOMAINS
                    .iter()
                    .any(|domain| url.starts_with(&format!("https://{domain}/"))),
                "generated links must stay in reserved domains: {url}"
            );
        }
    }
}
