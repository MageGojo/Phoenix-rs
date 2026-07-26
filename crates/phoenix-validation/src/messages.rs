//! Locale-aware message catalogs for validation errors.
//!
//! Every built-in rule renders its user-facing message through a process-wide
//! message registry. The registry ships with two complete catalogs — [`LOCALE_EN`]
//! (the default, byte-for-byte identical to the historical hard-coded messages)
//! and [`LOCALE_ZH_CN`] — and applications may register additional locales,
//! override single templates, or map raw field names to human-readable display
//! names. Only message texts are affected: the `rule` identifiers and the 422
//! error body shape (`{ message, errors: { field: [{ rule, message }] } }`)
//! never change.
//!
//! Templates use `{name}` placeholders. Every template may reference `{field}`
//! (the field display name); rule-specific parameters are named after the rule
//! constructor arguments, e.g. `{min}` for [`min_length`](crate::min_length)
//! and `{max}` for [`max_length`](crate::max_length).

use std::{
    borrow::Cow,
    collections::BTreeMap,
    sync::{OnceLock, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

/// Locale identifier of the built-in English catalog (the process default).
pub const LOCALE_EN: &str = "en";

/// Locale identifier of the built-in Simplified Chinese catalog.
pub const LOCALE_ZH_CN: &str = "zh-CN";

/// Rule identifiers of every built-in rule shipped by this crate.
///
/// The test suite asserts that each entry has a template in every built-in
/// locale, so adding a rule here without translating it fails the build.
pub const BUILT_IN_RULES: &[&str] = &["max_length", "min_length", "required", "string"];

const DEFAULT_INVALID_EN: &str = "The submitted data is invalid.";
const DEFAULT_INVALID_ZH_CN: &str = "提交的数据不合法。";

fn en_template(rule: &str) -> Option<&'static str> {
    match rule {
        "required" => Some("The {field} field is required."),
        "string" => Some("The {field} field must be a string."),
        "min_length" => Some("The {field} field must be at least {min} characters."),
        "max_length" => Some("The {field} field must not exceed {max} characters."),
        _ => None,
    }
}

fn zh_cn_template(rule: &str) -> Option<&'static str> {
    match rule {
        "required" => Some("{field} 不能为空。"),
        "string" => Some("{field} 必须是字符串。"),
        "min_length" => Some("{field} 长度不能小于 {min} 个字符。"),
        "max_length" => Some("{field} 长度不能超过 {max} 个字符。"),
        _ => None,
    }
}

/// A message catalog for one locale: per-rule templates plus the top-level
/// 422 message (`"The submitted data is invalid."` in English).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Messages {
    invalid: Option<Cow<'static, str>>,
    templates: BTreeMap<Cow<'static, str>, Cow<'static, str>>,
}

impl Messages {
    /// An empty catalog; unregistered rules fall back to English.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the message template for one rule identifier.
    #[must_use]
    pub fn rule(
        mut self,
        rule: impl Into<Cow<'static, str>>,
        template: impl Into<Cow<'static, str>>,
    ) -> Self {
        self.templates.insert(rule.into(), template.into());
        self
    }

    /// Set the top-level 422 message shown next to the field errors.
    #[must_use]
    pub fn invalid(mut self, message: impl Into<Cow<'static, str>>) -> Self {
        self.invalid = Some(message.into());
        self
    }

    /// The template registered for `rule`, if any.
    #[must_use]
    pub fn template(&self, rule: &str) -> Option<&str> {
        self.templates.get(rule).map(Cow::as_ref)
    }

    /// The top-level 422 message of this catalog, if set.
    #[must_use]
    pub fn invalid_message(&self) -> Option<&str> {
        self.invalid.as_deref()
    }

    /// Rule identifiers that have a template in this catalog.
    pub fn rules(&self) -> impl Iterator<Item = &str> {
        self.templates.keys().map(Cow::as_ref)
    }
}

/// The complete built-in catalog for `locale`, or `None` when the crate does
/// not ship one. Built-in locales: [`LOCALE_EN`] and [`LOCALE_ZH_CN`].
#[must_use]
pub fn builtin_locale(locale: &str) -> Option<Messages> {
    type Template = fn(&str) -> Option<&'static str>;
    let (invalid, template): (&'static str, Template) = match locale {
        LOCALE_EN => (DEFAULT_INVALID_EN, en_template),
        LOCALE_ZH_CN => (DEFAULT_INVALID_ZH_CN, zh_cn_template),
        _ => return None,
    };
    let mut messages = Messages::new().invalid(invalid);
    for rule in BUILT_IN_RULES {
        if let Some(text) = template(rule) {
            messages = messages.rule(*rule, text);
        }
    }
    Some(messages)
}

#[derive(Debug)]
struct Registry {
    locale: String,
    locales: BTreeMap<String, Messages>,
    field_names: BTreeMap<String, String>,
}

impl Default for Registry {
    fn default() -> Self {
        let mut locales = BTreeMap::new();
        for locale in [LOCALE_EN, LOCALE_ZH_CN] {
            if let Some(messages) = builtin_locale(locale) {
                locales.insert(locale.to_owned(), messages);
            }
        }
        Self {
            locale: LOCALE_EN.to_owned(),
            locales,
            field_names: BTreeMap::new(),
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

/// Set the process-wide locale used to render validation messages.
///
/// Typically called once during application start-up, e.g.
/// `phoenix_validation::set_locale(phoenix_validation::LOCALE_ZH_CN)`. Rules
/// missing from the active locale fall back to English, so a partial custom
/// locale is safe.
pub fn set_locale(locale: impl Into<String>) {
    write().locale = locale.into();
}

/// The identifier of the currently active locale (default: `"en"`).
#[must_use]
pub fn locale() -> String {
    read().locale.clone()
}

/// Register (or fully replace) an application-defined locale catalog.
///
/// Activate it with [`set_locale`]. Rules without a template in the catalog
/// fall back to the built-in English messages.
pub fn register_locale(locale: impl Into<String>, messages: Messages) {
    write().locales.insert(locale.into(), messages);
}

/// Override a single rule template inside one locale.
///
/// The locale is created on demand, seeded from the built-in catalog when one
/// exists, so overriding one message keeps every other translation intact.
pub fn override_message(
    locale: &str,
    rule: impl Into<Cow<'static, str>>,
    template: impl Into<Cow<'static, str>>,
) {
    let mut registry = write();
    let messages = registry
        .locales
        .entry(locale.to_owned())
        .or_insert_with(|| builtin_locale(locale).unwrap_or_default());
    messages.templates.insert(rule.into(), template.into());
}

/// Override the top-level 422 message (`"The submitted data is invalid."`)
/// for one locale.
pub fn override_invalid_message(locale: &str, message: impl Into<Cow<'static, str>>) {
    let mut registry = write();
    let messages = registry
        .locales
        .entry(locale.to_owned())
        .or_insert_with(|| builtin_locale(locale).unwrap_or_default());
    messages.invalid = Some(message.into());
}

/// Register a human-readable display name for a field, used wherever a
/// template references `{field}` (e.g. `email` → `邮箱`).
pub fn register_field_name(field: impl Into<String>, display: impl Into<String>) {
    write().field_names.insert(field.into(), display.into());
}

/// Register several field display names at once. See [`register_field_name`].
pub fn register_field_names<F, D>(pairs: impl IntoIterator<Item = (F, D)>)
where
    F: Into<String>,
    D: Into<String>,
{
    let mut registry = write();
    for (field, display) in pairs {
        registry.field_names.insert(field.into(), display.into());
    }
}

/// The display name registered for `field`, falling back to the raw name.
#[must_use]
pub fn field_display_name(field: &str) -> String {
    read()
        .field_names
        .get(field)
        .cloned()
        .unwrap_or_else(|| field.to_owned())
}

/// The localized top-level 422 message for the active locale.
#[must_use]
pub fn invalid_message() -> String {
    let registry = read();
    registry
        .locales
        .get(&registry.locale)
        .and_then(Messages::invalid_message)
        .or_else(|| {
            registry
                .locales
                .get(LOCALE_EN)
                .and_then(Messages::invalid_message)
        })
        .unwrap_or(DEFAULT_INVALID_EN)
        .to_owned()
}

/// Render the localized message for `rule`, interpolating `{field}` (mapped
/// through the display-name registry) and the given `params`.
///
/// Resolution order: active locale → registered `en` catalog → built-in
/// English templates. Returns `None` when the rule has no template anywhere,
/// which only happens for custom rules that never registered one — useful for
/// [`custom_rule`](crate::custom_rule) implementations that want to opt in to
/// the catalog while keeping their own fallback text.
#[must_use]
pub fn rule_message(rule: &str, field: &str, params: &[(&str, &str)]) -> Option<String> {
    let registry = read();
    let template = registry
        .locales
        .get(&registry.locale)
        .and_then(|messages| messages.template(rule))
        .or_else(|| {
            registry
                .locales
                .get(LOCALE_EN)
                .and_then(|messages| messages.template(rule))
        })
        .or_else(|| en_template(rule))?;
    let display = registry
        .field_names
        .get(field)
        .map_or(field, String::as_str);
    let mut rendered = template.replace("{field}", display);
    for (name, value) in params {
        rendered = rendered.replace(&format!("{{{name}}}"), value);
    }
    Some(rendered)
}

/// Message renderer for the crate's own rules: falls back to a generic text so
/// built-in rules can never produce an empty message.
pub(crate) fn builtin_message(rule: &str, field: &str, params: &[(&str, &str)]) -> String {
    rule_message(rule, field, params)
        .unwrap_or_else(|| format!("The {} field is invalid.", field_display_name(field)))
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    *write() = Registry::default();
}
