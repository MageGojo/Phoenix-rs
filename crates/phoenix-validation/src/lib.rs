use std::{borrow::Cow, collections::BTreeMap};

use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

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
            .ok_or_else(|| format!("The {} field is required.", context.field))
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
            Some(_) => Err(format!("The {} field must be a string.", context.field)),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MinLength(usize);

#[must_use]
pub const fn min_length(length: usize) -> MinLength {
    MinLength(length)
}

impl Rule for MinLength {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("min_length")
    }

    fn validate(&self, context: RuleContext<'_>) -> Result<(), String> {
        if let Some(Value::String(value)) = context.value
            && value.chars().count() < self.0
        {
            return Err(format!(
                "The {} field must be at least {} characters.",
                context.field, self.0
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
