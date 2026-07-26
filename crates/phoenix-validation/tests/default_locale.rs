//! Backward-compatibility guarantee: a process that never touches the locale
//! API gets byte-for-byte the same messages and 422 body as before the
//! localization feature existed. Runs as its own binary so no other test can
//! mutate the process-wide registry first.

use phoenix_http::{IntoResponse, Response, StatusCode};
use phoenix_validation::{
    ValidatedRejection, Validator, max_length, min_length, required, rules, string,
};
use serde_json::Value;

#[test]
fn untouched_process_keeps_historical_english_output() {
    assert_eq!(phoenix_validation::locale(), "en");

    let data = serde_json::json!({ "nick": 7, "bio": "ab", "title": "abcdef" });
    let errors = Validator::new(&data)
        .field("name", rules![required()])
        .field("nick", rules![string()])
        .field("bio", rules![min_length(3)])
        .field("title", rules![max_length(5)])
        .validate()
        .expect_err("every rule fails");

    let message = |field: &str| {
        let errors = errors.get(field).expect("field has errors");
        assert_eq!(errors.len(), 1);
        errors[0].message.clone()
    };
    assert_eq!(message("name"), "The name field is required.");
    assert_eq!(message("nick"), "The nick field must be a string.");
    assert_eq!(
        message("bio"),
        "The bio field must be at least 3 characters."
    );
    assert_eq!(
        message("title"),
        "The title field must not exceed 5 characters."
    );

    let rejection: ValidatedRejection<Response> = ValidatedRejection::Validation(errors);
    let response = rejection.into_response();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = serde_json::from_slice(response.body()).expect("JSON body");
    assert_eq!(body["message"], "The submitted data is invalid.");
    assert_eq!(body["errors"]["name"][0]["rule"], "required");
    assert_eq!(
        body["errors"]["name"][0]["message"],
        "The name field is required."
    );
}
