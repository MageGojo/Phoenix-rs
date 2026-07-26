use std::{
    borrow::Cow,
    collections::BTreeMap,
    ops::{Deref, DerefMut},
};

use phoenix_http::{FromRequest, IntoResponse, Json, Request, Response, StatusCode};
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

pub mod messages;

pub use messages::{
    BUILT_IN_RULES, LOCALE_EN, LOCALE_ZH_CN, Messages, builtin_locale, field_display_name,
    invalid_message, locale, override_invalid_message, override_message, register_field_name,
    register_field_names, register_locale, rule_message, set_locale,
};

#[derive(Clone, Copy)]
pub struct RuleContext<'a> {
    pub field: &'a str,
    pub value: Option<&'a Value>,
    pub data: &'a Value,
}

pub trait Rule: Send + Sync + 'static {
    fn name(&self) -> Cow<'static, str>;

    /// Validate a field in the context of its full input payload.
    ///
    /// # Errors
    ///
    /// Returns a user-facing validation message when the field is invalid.
    fn validate(&self, context: RuleContext<'_>) -> Result<(), String>;
}

pub type BoxedRule = Box<dyn Rule>;

#[macro_export]
macro_rules! rules {
    ($($rule:expr),* $(,)?) => {{
        let rules: ::std::vec::Vec<$crate::BoxedRule> =
            ::std::vec![$(::std::boxed::Box::new($rule)),*];
        rules
    }};
}

pub struct CustomRule<F> {
    name: Cow<'static, str>,
    validate: F,
}

#[must_use]
pub fn custom_rule<F>(name: impl Into<Cow<'static, str>>, validate: F) -> CustomRule<F>
where
    F: for<'a> Fn(RuleContext<'a>) -> Result<(), String> + Send + Sync + 'static,
{
    CustomRule {
        name: name.into(),
        validate,
    }
}

impl<F> Rule for CustomRule<F>
where
    F: for<'a> Fn(RuleContext<'a>) -> Result<(), String> + Send + Sync + 'static,
{
    fn name(&self) -> Cow<'static, str> {
        self.name.clone()
    }

    fn validate(&self, context: RuleContext<'_>) -> Result<(), String> {
        (self.validate)(context)
    }
}

pub struct Validator<'a> {
    data: &'a Value,
    rules: BTreeMap<String, Vec<Box<dyn Rule>>>,
}

impl<'a> Validator<'a> {
    #[must_use]
    pub fn new(data: &'a Value) -> Self {
        Self {
            data,
            rules: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn field<I>(mut self, field: impl Into<String>, rules: I) -> Self
    where
        I: IntoIterator<Item = BoxedRule>,
    {
        self.rules.entry(field.into()).or_default().extend(rules);
        self
    }

    /// Run every registered rule and collect field-level errors.
    ///
    /// # Errors
    ///
    /// Returns all [`ValidationErrors`] when one or more rules fail.
    pub fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = BTreeMap::new();

        for (field, rules) in &self.rules {
            let value = value_at_path(self.data, field);
            for rule in rules {
                if let Err(message) = rule.validate(RuleContext {
                    field,
                    value,
                    data: self.data,
                }) {
                    errors
                        .entry(field.clone())
                        .or_insert_with(Vec::new)
                        .push(ValidationError {
                            rule: rule.name().into_owned(),
                            message,
                        });
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationErrors { fields: errors })
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ValidationError {
    pub rule: String,
    pub message: String,
}

#[derive(Clone, Debug, Error, Serialize, PartialEq, Eq)]
#[error("validation failed")]
pub struct ValidationErrors {
    fields: BTreeMap<String, Vec<ValidationError>>,
}

pub trait Validate: Send + Sync + 'static {
    /// Validate a deserialized request DTO.
    ///
    /// # Errors
    ///
    /// Returns stable field-level errors when the DTO is not valid.
    fn validate(&self) -> Result<(), ValidationErrors>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Validated<T>(pub T);

impl<T> Deref for Validated<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Validated<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Debug)]
pub enum ValidatedRejection<R> {
    Extract(R),
    Validation(ValidationErrors),
}

impl<R> IntoResponse for ValidatedRejection<R>
where
    R: IntoResponse,
{
    fn into_response(self) -> Response {
        match self {
            Self::Extract(rejection) => rejection.into_response(),
            Self::Validation(errors) => {
                #[derive(Serialize)]
                struct ValidationBody<'a> {
                    message: String,
                    errors: &'a BTreeMap<String, Vec<ValidationError>>,
                }

                Json(ValidationBody {
                    message: messages::invalid_message(),
                    errors: errors.fields(),
                })
                .into_response()
                .with_status(StatusCode::UNPROCESSABLE_ENTITY)
            }
        }
    }
}

impl<E, T> FromRequest for Validated<E>
where
    E: FromRequest + Deref<Target = T>,
    T: Validate,
{
    type Rejection = ValidatedRejection<E::Rejection>;

    fn from_request(request: &Request) -> Result<Self, Self::Rejection> {
        let extracted = E::from_request(request).map_err(ValidatedRejection::Extract)?;
        extracted
            .deref()
            .validate()
            .map_err(ValidatedRejection::Validation)?;
        Ok(Self(extracted))
    }
}

impl ValidationErrors {
    #[must_use]
    pub fn get(&self, field: &str) -> Option<&[ValidationError]> {
        self.fields.get(field).map(Vec::as_slice)
    }

    #[must_use]
    pub fn fields(&self) -> &BTreeMap<String, Vec<ValidationError>> {
        &self.fields
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Required;

#[must_use]
pub const fn required() -> Required {
    Required
}

impl Rule for Required {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("required")
    }

    fn validate(&self, context: RuleContext<'_>) -> Result<(), String> {
        let valid = match context.value {
            None | Some(Value::Null) => false,
            Some(Value::String(value)) => !value.trim().is_empty(),
            Some(Value::Array(value)) => !value.is_empty(),
            Some(Value::Object(value)) => !value.is_empty(),
            Some(_) => true,
        };
        valid
            .then_some(())
            .ok_or_else(|| messages::builtin_message("required", context.field, &[]))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct StringRule;

#[must_use]
pub const fn string() -> StringRule {
    StringRule
}

impl Rule for StringRule {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("string")
    }

    fn validate(&self, context: RuleContext<'_>) -> Result<(), String> {
        match context.value {
            None | Some(Value::Null | Value::String(_)) => Ok(()),
            Some(_) => Err(messages::builtin_message("string", context.field, &[])),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MinLength(usize);

#[must_use]
pub const fn min_length(length: usize) -> MinLength {
    MinLength(length)
}

#[derive(Clone, Copy, Debug)]
pub struct MaxLength(usize);

#[must_use]
pub const fn max_length(length: usize) -> MaxLength {
    MaxLength(length)
}

impl Rule for MaxLength {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("max_length")
    }

    fn validate(&self, context: RuleContext<'_>) -> Result<(), String> {
        if let Some(Value::String(value)) = context.value
            && value.chars().count() > self.0
        {
            let max = self.0.to_string();
            return Err(messages::builtin_message(
                "max_length",
                context.field,
                &[("max", max.as_str())],
            ));
        }
        Ok(())
    }
}

impl Rule for MinLength {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("min_length")
    }

    fn validate(&self, context: RuleContext<'_>) -> Result<(), String> {
        if let Some(Value::String(value)) = context.value
            && value.chars().count() < self.0
        {
            let min = self.0.to_string();
            return Err(messages::builtin_message(
                "min_length",
                context.field,
                &[("min", min.as_str())],
            ));
        }
        Ok(())
    }
}

fn value_at_path<'a>(data: &'a Value, field: &str) -> Option<&'a Value> {
    field
        .split('.')
        .try_fold(data, |current, segment| current.get(segment))
}

#[cfg(test)]
mod tests {
    use phoenix_http::{Bytes, Handler, HeaderMap, HeaderValue, Method, Request, header, typed};
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    struct CreateUserInput {
        name: String,
    }

    impl Validate for CreateUserInput {
        fn validate(&self) -> Result<(), ValidationErrors> {
            let data = serde_json::json!({ "name": self.name });
            Validator::new(&data)
                .field("name", rules![required(), string(), min_length(3)])
                .validate()
        }
    }

    #[tokio::test]
    async fn validated_extractor_returns_dto_or_field_errors() {
        let handler = typed(
            |Validated(Json(input)): Validated<Json<CreateUserInput>>| async move {
                Json(serde_json::json!({ "name": input.name }))
            },
        );
        let request = |body: &'static [u8]| {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            Request::from_parts(
                Method::POST,
                "/users".parse().expect("valid URI"),
                headers,
                Bytes::from_static(body),
            )
        };

        let response = handler.call(request(br#"{"name":"Ada"}"#)).await;
        assert_eq!(response.status(), StatusCode::OK);

        let response = handler.call(request(br#"{"name":"A"}"#)).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body: Value = serde_json::from_slice(response.body()).expect("validation JSON");
        assert_eq!(body["errors"]["name"][0]["rule"], "min_length");
    }

    mod locales {
        use std::{
            collections::BTreeSet,
            sync::{Mutex, MutexGuard, PoisonError},
        };

        use super::*;

        /// Serializes tests that touch the process-wide message registry and
        /// resets it to pristine defaults before each one.
        fn serial() -> MutexGuard<'static, ()> {
            static GUARD: Mutex<()> = Mutex::new(());
            let guard = GUARD.lock().unwrap_or_else(PoisonError::into_inner);
            messages::reset_for_tests();
            guard
        }

        fn fail_all_rules() -> ValidationErrors {
            let data = serde_json::json!({ "nick": 7, "bio": "ab", "title": "abcdef" });
            Validator::new(&data)
                .field("name", rules![required()])
                .field("nick", rules![string()])
                .field("bio", rules![min_length(3)])
                .field("title", rules![max_length(5)])
                .validate()
                .expect_err("every rule fails")
        }

        fn only_message(errors: &ValidationErrors, field: &str) -> String {
            let errors = errors.get(field).expect("field has errors");
            assert_eq!(errors.len(), 1);
            errors[0].message.clone()
        }

        #[test]
        fn built_in_locales_cover_every_rule_without_gaps() {
            let expected: BTreeSet<&str> = BUILT_IN_RULES.iter().copied().collect();
            assert_eq!(expected.len(), BUILT_IN_RULES.len(), "no duplicate rules");
            for locale in [LOCALE_EN, LOCALE_ZH_CN] {
                let catalog = builtin_locale(locale).expect("built-in locale exists");
                let translated: BTreeSet<&str> = catalog.rules().collect();
                assert_eq!(
                    translated, expected,
                    "locale {locale} must translate exactly the built-in rules"
                );
                assert!(
                    catalog.invalid_message().is_some(),
                    "locale {locale} must translate the top-level invalid message"
                );
            }
            // Every built-in rule constructor reports a name listed in BUILT_IN_RULES.
            for rule in rules![required(), string(), min_length(1), max_length(1)] {
                assert!(
                    BUILT_IN_RULES.contains(&rule.name().as_ref()),
                    "rule {} missing from BUILT_IN_RULES",
                    rule.name()
                );
            }
            assert!(builtin_locale("xx").is_none());
        }

        #[test]
        fn default_locale_keeps_english_messages() {
            let _guard = serial();
            assert_eq!(locale(), LOCALE_EN);
            let errors = fail_all_rules();
            assert_eq!(only_message(&errors, "name"), "The name field is required.");
            assert_eq!(
                only_message(&errors, "nick"),
                "The nick field must be a string."
            );
            assert_eq!(
                only_message(&errors, "bio"),
                "The bio field must be at least 3 characters."
            );
            assert_eq!(
                only_message(&errors, "title"),
                "The title field must not exceed 5 characters."
            );
            assert_eq!(invalid_message(), "The submitted data is invalid.");
        }

        #[test]
        fn zh_cn_locale_translates_every_built_in_rule() {
            let _guard = serial();
            set_locale(LOCALE_ZH_CN);
            let errors = fail_all_rules();
            assert_eq!(only_message(&errors, "name"), "name 不能为空。");
            assert_eq!(only_message(&errors, "nick"), "nick 必须是字符串。");
            assert_eq!(only_message(&errors, "bio"), "bio 长度不能小于 3 个字符。");
            assert_eq!(
                only_message(&errors, "title"),
                "title 长度不能超过 5 个字符。"
            );
            assert_eq!(invalid_message(), "提交的数据不合法。");
        }

        #[test]
        fn field_display_names_apply_with_fallback() {
            let _guard = serial();
            set_locale(LOCALE_ZH_CN);
            register_field_name("email", "邮箱");
            register_field_names([("password", "密码")]);
            assert_eq!(field_display_name("email"), "邮箱");
            assert_eq!(field_display_name("unknown"), "unknown");
            let data = serde_json::json!({ "password": "abc" });
            let errors = Validator::new(&data)
                .field("email", rules![required()])
                .field("password", rules![min_length(8)])
                .validate()
                .expect_err("both fields fail");
            assert_eq!(only_message(&errors, "email"), "邮箱 不能为空。");
            assert_eq!(
                only_message(&errors, "password"),
                "密码 长度不能小于 8 个字符。"
            );
            // Rule identifiers stay stable regardless of display names.
            assert_eq!(errors.get("email").expect("errors")[0].rule, "required");
        }

        #[test]
        fn single_message_and_invalid_overrides() {
            let _guard = serial();
            set_locale(LOCALE_ZH_CN);
            override_message(LOCALE_ZH_CN, "required", "请填写 {field}！");
            override_invalid_message(LOCALE_ZH_CN, "数据校验未通过。");
            let data = serde_json::json!({ "bio": "ab" });
            let errors = Validator::new(&data)
                .field("name", rules![required()])
                .field("bio", rules![min_length(3)])
                .validate()
                .expect_err("fails");
            assert_eq!(only_message(&errors, "name"), "请填写 name！");
            // Untouched templates keep their built-in translation.
            assert_eq!(only_message(&errors, "bio"), "bio 长度不能小于 3 个字符。");
            assert_eq!(invalid_message(), "数据校验未通过。");
        }

        #[test]
        fn custom_locale_falls_back_to_english_for_missing_rules() {
            let _guard = serial();
            register_locale(
                "pirate",
                Messages::new().rule("required", "Arr, {field} be missing!"),
            );
            set_locale("pirate");
            assert_eq!(locale(), "pirate");
            let data = serde_json::json!({ "bio": "ab" });
            let errors = Validator::new(&data)
                .field("name", rules![required()])
                .field("bio", rules![min_length(3)])
                .validate()
                .expect_err("fails");
            assert_eq!(only_message(&errors, "name"), "Arr, name be missing!");
            assert_eq!(
                only_message(&errors, "bio"),
                "The bio field must be at least 3 characters."
            );
            assert_eq!(invalid_message(), "The submitted data is invalid.");
        }

        #[test]
        fn rule_message_interpolates_placeholders() {
            let _guard = serial();
            set_locale(LOCALE_ZH_CN);
            register_field_name("title", "标题");
            assert_eq!(
                rule_message("max_length", "title", &[("max", "10")]).as_deref(),
                Some("标题 长度不能超过 10 个字符。")
            );
            assert_eq!(rule_message("no_such_rule", "title", &[]), None);
        }

        #[test]
        fn rejection_body_localizes_top_level_message_only() {
            let _guard = serial();
            set_locale(LOCALE_ZH_CN);
            let data = serde_json::json!({});
            let errors = Validator::new(&data)
                .field("name", rules![required()])
                .validate()
                .expect_err("fails");
            let rejection: ValidatedRejection<Response> = ValidatedRejection::Validation(errors);
            let response = rejection.into_response();
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let body: Value = serde_json::from_slice(response.body()).expect("JSON body");
            assert_eq!(body["message"], "提交的数据不合法。");
            assert_eq!(body["errors"]["name"][0]["rule"], "required");
            assert_eq!(body["errors"]["name"][0]["message"], "name 不能为空。");
        }
    }
}
