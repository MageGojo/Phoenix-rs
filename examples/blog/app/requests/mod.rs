use std::borrow::Cow;

use phoenix::prelude::{Rule, RuleContext, Validator, min_length, required, rules, string};
use serde_json::Value;

pub struct NotReservedUser;

impl Rule for NotReservedUser {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("not_reserved")
    }

    fn validate(&self, context: RuleContext<'_>) -> Result<(), String> {
        let reserved = context
            .value
            .and_then(Value::as_str)
            .is_some_and(|user| ["admin", "root"].contains(&user.to_ascii_lowercase().as_str()));

        if reserved {
            Err("The user field contains a reserved name.".to_owned())
        } else {
            Ok(())
        }
    }
}

#[must_use]
pub fn registration_validator(data: &Value) -> Validator<'_> {
    Validator::new(data)
        .field("user", rules![required(), string(), NotReservedUser])
        .field("password", rules![required(), string(), min_length(8)])
}
