use phoenix::prelude::{Validate, ValidationErrors, Validator, max_length, required, rules, string};
use serde::Deserialize;

#[phoenix::contract(input)]
#[derive(Debug, Deserialize)]
pub struct StoreNoteRequest {
    pub name: String,
}

impl Validate for StoreNoteRequest {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let data = serde_json::json!({ "name": self.name });
        Validator::new(&data)
            .field("name", rules![required(), string(), max_length(255)])
            .validate()
    }
}
