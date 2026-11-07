use serde_json::json;

use crate::data_dictionary::validate_instance;

fn valid_portfolio() -> serde_json::Value {
    json!({
        "id": "00000000-0000-0000-0000-000000000001",
        "name": "Core",
        "purpose": "Long-only core holdings",
        "created_at": "2026-01-01T00:00:00Z",
        "created_by": "tester",
        "parent_id": null
    })
}

#[test]
fn valid_entity_passes() {
    validate_instance("Portfolio", &valid_portfolio()).expect("valid portfolio");
}

#[test]
fn missing_required_field_fails_with_pointer() {
    let mut value = valid_portfolio();
    value.as_object_mut().expect("object").remove("name");
    let errors = validate_instance("Portfolio", &value).expect_err("missing name");
    assert!(
        errors
            .iter()
            .any(|error| error.pointer == "/name" && error.rule == "required")
    );
}

#[test]
fn unknown_property_fails() {
    let mut value = valid_portfolio();
    value
        .as_object_mut()
        .expect("object")
        .insert("unexpected".to_string(), json!(true));
    let errors = validate_instance("Portfolio", &value).expect_err("unknown property");
    assert!(
        errors
            .iter()
            .any(|error| error.rule == "additionalProperties")
    );
}

#[test]
fn nullable_field_accepts_absent_and_null() {
    let mut absent = valid_portfolio();
    absent.as_object_mut().expect("object").remove("parent_id");
    validate_instance("Portfolio", &absent).expect("absent nullable field");
    let mut null = valid_portfolio();
    null.as_object_mut()
        .expect("object")
        .insert("parent_id".to_string(), serde_json::Value::Null);
    validate_instance("Portfolio", &null).expect("null nullable field");
}

#[test]
fn required_field_rejects_null() {
    let mut value = valid_portfolio();
    value
        .as_object_mut()
        .expect("object")
        .insert("name".to_string(), serde_json::Value::Null);
    let errors = validate_instance("Portfolio", &value).expect_err("null required field");
    assert!(errors.iter().any(|error| error.pointer == "/name"));
}

#[test]
fn wrong_id_format_fails() {
    let mut value = valid_portfolio();
    value
        .as_object_mut()
        .expect("object")
        .insert("id".to_string(), json!("not-a-uuid"));
    let errors = validate_instance("Portfolio", &value).expect_err("bad uuid");
    assert!(errors.iter().any(|error| error.pointer == "/id"));
}
