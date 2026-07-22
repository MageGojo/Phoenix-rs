use std::borrow::Cow;

use phoenix::prelude::{
    Rule, RuleContext, Validate, ValidationErrors, Validator, max_length, min_length, required,
    rules, string,
};
use serde::Deserialize;
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

#[phoenix::contract(input)]
#[derive(Debug, Deserialize)]
pub struct StoreMemberInput {
    pub name: String,
}

impl Validate for StoreMemberInput {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let data = serde_json::json!({ "name": self.name });
        Validator::new(&data)
            .field(
                "name",
                rules![required(), string(), min_length(1), max_length(40)],
            )
            .validate()
    }
}
