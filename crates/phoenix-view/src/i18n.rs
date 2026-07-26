//! View-layer internationalization: translation catalogs and locale negotiation.
//!
//! This is the rendering-side companion to `phoenix-validation`'s message
//! catalogs. Both share the same mental model — a process-wide registry keyed by
//! locale, `{name}` placeholder templates, and English-style fallback — but the
//! two registries are independent: validation renders 422 error messages under a
//! single *active* process locale, while the view layer negotiates a *per
//! request* locale (from `Accept-Language`) and ships that locale's catalog to
//! the browser inside the page envelope.
//!
//! Register catalogs once at start-up, negotiate a locale per request with
//! [`negotiate_locale`] (or [`negotiate_locale_from_headers`]), then attach it
//! with [`Page::locale`](crate::Page::locale) so the `<html lang>` attribute,
//! the SSR renderer context, and [`PageEnvelope::translations`](crate::PageEnvelope::translations)
//! all agree.
//!
//! ```
//! use phoenix_view::i18n;
//!
//! i18n::register_translations("zh-CN", [("greeting", "你好，{name}！")]);
//! let locale = i18n::negotiate_locale("zh-CN,en;q=0.8", &["en", "zh-CN"], "en");
//! assert_eq!(locale, "zh-CN");
//! assert_eq!(
//!     i18n::translate(&locale, "greeting", &[("name", "Ada")]),
//!     "你好，Ada！"
//! );
//! ```

use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::BTreeMap,
    sync::{OnceLock, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use phoenix_http::{HeaderMap, header};

/// Locale identifier used as the process-wide fallback (`"en"`).
pub const DEFAULT_LOCALE: &str = "en";

/// A translation catalog for one locale: `key` -> message template.
///
/// Templates use `{name}` placeholders, interpolated by [`translate`]. Build a
/// catalog in memory and install it with [`register_locale`] (full replace), or
/// merge loose pairs into a locale with [`register_translations`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Translations {
    entries: BTreeMap<Cow<'static, str>, Cow<'static, str>>,
}

impl Translations {
    /// An empty catalog.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the template for one translation `key`.
    #[must_use]
    pub fn entry(
        mut self,
        key: impl Into<Cow<'static, str>>,
        template: impl Into<Cow<'static, str>>,
    ) -> Self {
        self.entries.insert(key.into(), template.into());
        self
    }

    /// The template registered for `key`, if any.
    #[must_use]
    pub fn template(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(Cow::as_ref)
    }

    /// Every translation key in this catalog, in sorted order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(Cow::as_ref)
    }

    /// Number of registered keys.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the catalog has no keys.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Debug)]
struct Registry {
    default_locale: String,
    locales: BTreeMap<String, Translations>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            default_locale: DEFAULT_LOCALE.to_owned(),
            locales: BTreeMap::new(),
        }
    }
}

fn registry() -> &'static RwLock<Registry> {
    static REGISTRY: OnceLock<RwLock<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(Registry::default()))
}

fn read() -> RwLockReadGuard<'static, Registry> {
    registry().read().unwrap_or_else(PoisonError::into_inner)
}

fn write() -> RwLockWriteGuard<'static, Registry> {
    registry().write().unwrap_or_else(PoisonError::into_inner)
}

/// Set the process-wide fallback locale (default: [`DEFAULT_LOCALE`]).
///
/// [`translate`] and [`translation_catalog`] fall back to this locale when the
/// requested locale is missing a key. Typically called once at start-up.
pub fn set_default_locale(locale: impl Into<String>) {
    write().default_locale = locale.into();
}

/// The identifier of the current fallback locale (default: `"en"`).
#[must_use]
pub fn default_locale() -> String {
    read().default_locale.clone()
}

/// Register (or fully replace) a locale's catalog.
///
/// Mirrors `phoenix_validation::register_locale`. Use [`register_translations`]
/// to merge into an existing catalog instead of replacing it.
pub fn register_locale(locale: impl Into<String>, translations: Translations) {
    write().locales.insert(locale.into(), translations);
}

/// Register or merge loose `key` -> `template` pairs into a locale, creating the
/// catalog on demand.
///
/// This is the primary in-memory registration entry point:
///
/// ```
/// use phoenix_view::i18n;
///
/// i18n::register_translations("en", [("greeting", "Hello, {name}!")]);
/// i18n::register_translations(
///     "zh-CN",
///     [("greeting", "你好，{name}！"), ("bye", "再见")],
/// );
/// ```
pub fn register_translations<K, V>(
    locale: impl Into<String>,
    entries: impl IntoIterator<Item = (K, V)>,
) where
    K: Into<Cow<'static, str>>,
    V: Into<Cow<'static, str>>,
{
    let mut registry = write();
    let catalog = registry.locales.entry(locale.into()).or_default();
    for (key, template) in entries {
        catalog.entries.insert(key.into(), template.into());
    }
}

/// Register or override a single `key` -> `template` pair in a locale.
pub fn register_translation(
    locale: impl Into<String>,
    key: impl Into<Cow<'static, str>>,
    template: impl Into<Cow<'static, str>>,
) {
    let mut registry = write();
    registry
        .locales
        .entry(locale.into())
        .or_default()
        .entries
        .insert(key.into(), template.into());
}

/// Render the template registered for (`locale`, `key`), interpolating the
/// `{name}` placeholders named in `params`.
///
/// Resolution order: the requested `locale` -> the default locale
/// ([`default_locale`]) -> the raw `key`. A key that is registered nowhere is
/// returned verbatim, so missing translations degrade to a stable identifier
/// rather than an empty string.
#[must_use]
pub fn translate(locale: &str, key: &str, params: &[(&str, &str)]) -> String {
    let registry = read();
    let template = registry
        .locales
        .get(locale)
        .and_then(|catalog| catalog.template(key))
        .or_else(|| {
            if registry.default_locale == locale {
                None
            } else {
                registry
                    .locales
                    .get(&registry.default_locale)
                    .and_then(|catalog| catalog.template(key))
            }
        });
    let mut rendered = template.unwrap_or(key).to_owned();
    for (name, value) in params {
        rendered = rendered.replace(&format!("{{{name}}}"), value);
    }
    rendered
}

/// The effective catalog shipped to the browser for `locale`: the default
/// locale's entries overlaid by the requested locale's entries.
///
/// Merging the default locale as a base gives the client full key coverage with
/// graceful fallback, matching [`translate`]'s server-side resolution. Used to
/// seed [`PageEnvelope::translations`](crate::PageEnvelope::translations).
#[must_use]
pub fn translation_catalog(locale: &str) -> BTreeMap<String, String> {
    let registry = read();
    let mut merged = BTreeMap::new();
    if registry.default_locale != locale
        && let Some(base) = registry.locales.get(&registry.default_locale)
    {
        for (key, template) in &base.entries {
            merged.insert(key.clone().into_owned(), template.clone().into_owned());
        }
    }
    if let Some(catalog) = registry.locales.get(locale) {
        for (key, template) in &catalog.entries {
            merged.insert(key.clone().into_owned(), template.clone().into_owned());
        }
    }
    merged
}

/// Every registered locale identifier, in sorted order.
#[must_use]
pub fn available_locales() -> Vec<String> {
    read().locales.keys().cloned().collect()
}

/// Negotiate the best available locale from an HTTP `Accept-Language` header.
///
/// Language ranges are ordered by their `q` value (default `1.0`; a `q=0` range
/// is rejected). Each range is matched against `available` first by exact tag
/// (case-insensitive), then by primary language subtag, so `zh` matches
/// `zh-CN`, `zh-CN` matches `zh`, and `en-US` matches `en`. The wildcard `*`
/// selects `default`. Returns `default` when nothing matches, including for an
/// empty or unparsable header.
///
/// The returned string preserves the casing of the matched `available` entry.
#[must_use]
pub fn negotiate_locale(accept_language: &str, available: &[&str], default: &str) -> String {
    let mut ranges: Vec<(String, f32, usize)> = accept_language
        .split(',')
        .enumerate()
        .filter_map(|(index, part)| parse_language_range(part, index))
        .collect();
    // Highest quality first; ties keep the header's original order.
    ranges.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(Ordering::Equal)
            .then(left.2.cmp(&right.2))
    });

    for (range, _quality, _index) in &ranges {
        if range == "*" {
            return default.to_owned();
        }
        if let Some(found) = available
            .iter()
            .find(|candidate| candidate.eq_ignore_ascii_case(range))
        {
            return (*found).to_owned();
        }
        if let Some(found) = available
            .iter()
            .find(|candidate| primary_subtag_matches(range, candidate))
        {
            return (*found).to_owned();
        }
    }
    default.to_owned()
}

/// Negotiate a locale from a request's `Accept-Language` header.
///
/// Reads the header through phoenix-http's read-only [`HeaderMap`] and forwards
/// to [`negotiate_locale`]. A missing or non-ASCII header negotiates to
/// `default`.
#[must_use]
pub fn negotiate_locale_from_headers(
    headers: &HeaderMap,
    available: &[&str],
    default: &str,
) -> String {
    let accept_language = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    negotiate_locale(accept_language, available, default)
}

fn parse_language_range(part: &str, index: usize) -> Option<(String, f32, usize)> {
    let mut segments = part.split(';');
    let tag = segments.next()?.trim();
    if tag.is_empty() {
        return None;
    }
    let mut quality = 1.0_f32;
    for segment in segments {
        if let Some(value) = segment.trim().strip_prefix("q=") {
            quality = value.trim().parse().unwrap_or(0.0);
        }
    }
    if quality <= 0.0 {
        return None;
    }
    Some((tag.to_ascii_lowercase(), quality, index))
}

fn primary_subtag_matches(range: &str, available: &str) -> bool {
    let range_primary = range.split('-').next().unwrap_or(range);
    let available_primary = available.split('-').next().unwrap_or(available);
    !range_primary.is_empty() && range_primary.eq_ignore_ascii_case(available_primary)
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    *write() = Registry::default();
}

/// Serialize registry-mutating tests across the whole crate's test binary.
///
/// The registry is process-global, so tests that register catalogs or read them
/// back must not interleave. Pure [`negotiate_locale`] tests need no guard.
#[cfg(test)]
pub(crate) fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiates_by_quality_then_exact_match() {
        // en has higher q than zh-CN, so it wins despite being listed second.
        assert_eq!(
            negotiate_locale("zh-CN;q=0.8, en;q=0.9", &["en", "zh-CN"], "en"),
            "en"
        );
        // Exact match wins and keeps the available entry's casing.
        assert_eq!(negotiate_locale("zh-CN", &["en", "zh-CN"], "en"), "zh-CN");
    }

    #[test]
    fn negotiates_by_primary_subtag_in_both_directions() {
        // Range "zh" matches the more specific available "zh-CN".
        assert_eq!(negotiate_locale("zh", &["en", "zh-CN"], "en"), "zh-CN");
        // Range "zh-CN" matches the less specific available "zh".
        assert_eq!(negotiate_locale("zh-CN", &["en", "zh"], "en"), "zh");
        // Range "en-US" matches available "en".
        assert_eq!(negotiate_locale("en-US", &["en", "zh-CN"], "zh-CN"), "en");
    }

    #[test]
    fn ties_keep_header_order_and_reject_zero_quality() {
        // Equal quality: first listed wins.
        assert_eq!(
            negotiate_locale("zh-CN,en", &["en", "zh-CN"], "en"),
            "zh-CN"
        );
        // q=0 explicitly rejects a range even though it is otherwise available.
        assert_eq!(
            negotiate_locale("zh-CN;q=0,en", &["en", "zh-CN"], "en"),
            "en"
        );
    }

    #[test]
    fn falls_back_to_default_for_wildcard_empty_and_unmatched() {
        assert_eq!(negotiate_locale("*", &["en", "zh-CN"], "en"), "en");
        assert_eq!(negotiate_locale("", &["en", "zh-CN"], "en"), "en");
        assert_eq!(negotiate_locale("fr, de", &["en", "zh-CN"], "en"), "en");
    }

    #[test]
    fn negotiates_from_request_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT_LANGUAGE,
            "zh-CN,zh;q=0.9,en;q=0.5".parse().unwrap(),
        );
        assert_eq!(
            negotiate_locale_from_headers(&headers, &["en", "zh-CN"], "en"),
            "zh-CN"
        );
        // A request without the header negotiates to the default.
        assert_eq!(
            negotiate_locale_from_headers(&HeaderMap::new(), &["en", "zh-CN"], "en"),
            "en"
        );
    }

    #[test]
    fn translate_interpolates_and_falls_back() {
        let _guard = test_guard();
        reset_for_tests();
        register_translations("en", [("greeting", "Hello, {name}!"), ("bye", "Bye")]);
        register_translations("zh-CN", [("greeting", "你好，{name}！")]);

        // Interpolates named placeholders.
        assert_eq!(
            translate("zh-CN", "greeting", &[("name", "Ada")]),
            "你好，Ada！"
        );
        // Missing key in zh-CN falls back to the default (en) catalog.
        assert_eq!(translate("zh-CN", "bye", &[]), "Bye");
        // Unknown key everywhere falls back to the raw key.
        assert_eq!(translate("zh-CN", "unknown", &[]), "unknown");
    }

    #[test]
    fn translation_catalog_merges_default_under_locale() {
        let _guard = test_guard();
        reset_for_tests();
        register_translations("en", [("greeting", "Hello"), ("bye", "Bye")]);
        register_translations("zh-CN", [("greeting", "你好")]);

        let catalog = translation_catalog("zh-CN");
        // Locale entry overrides the default; default-only keys remain available.
        assert_eq!(catalog.get("greeting").map(String::as_str), Some("你好"));
        assert_eq!(catalog.get("bye").map(String::as_str), Some("Bye"));

        assert_eq!(
            available_locales(),
            vec!["en".to_owned(), "zh-CN".to_owned()]
        );
    }

    #[test]
    fn custom_default_locale_changes_fallback() {
        let _guard = test_guard();
        reset_for_tests();
        set_default_locale("zh-CN");
        register_locale(
            "zh-CN",
            Translations::new().entry("greeting", "你好，{name}！"),
        );
        // No "fr" catalog: falls back to the configured default locale zh-CN.
        assert_eq!(
            translate("fr", "greeting", &[("name", "Ada")]),
            "你好，Ada！"
        );
        assert_eq!(default_locale(), "zh-CN");
        reset_for_tests();
    }
}
