use phoenix::prelude::{Validator, custom_rule, required, rules};
use serde_json::json;

#[test]
fn closure_based_custom_rules_receive_the_field_and_full_payload() {
    let payload = json!({
        "password": "same-value",
        "password_confirmation": "different-value"
    });
    let confirmed = custom_rule("confirmed", |context| {
        let confirmation = context.data.get("password_confirmation");
        if context.value == confirmation {
            Ok(())
        } else {
            Err(format!(
                "The {} confirmation does not match.",
                context.field
            ))
        }
    });

    let errors = Validator::new(&payload)
        .field("password", rules![required(), confirmed])
        .validate()
        .expect_err("mismatched values should fail");

    let password_errors = errors.get("password").expect("field error should exist");
    assert_eq!(password_errors[0].rule, "confirmed");
    assert_eq!(
        password_errors[0].message,
        "The password confirmation does not match."
    );
}
