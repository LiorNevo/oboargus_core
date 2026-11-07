use serde_json::{Value, json};

use crate::data_dictionary::compat::{
    ChangeKind, check_compatibility, classify_changes, parse_document,
};
use crate::data_dictionary::export_system_schema;

fn baseline() -> Value {
    export_system_schema().expect("schema").document().clone()
}

fn definition_mut<'a>(document: &'a mut Value, name: &str) -> &'a mut Value {
    document
        .get_mut("$defs")
        .and_then(Value::as_object_mut)
        .expect("$defs")
        .get_mut(name)
        .unwrap_or_else(|| panic!("missing definition {name}"))
}

#[test]
fn added_optional_property_is_non_breaking() {
    let before = baseline();
    let mut after = before.clone();
    definition_mut(&mut after, "Portfolio")
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .expect("properties")
        .insert("nickname".to_string(), json!({ "type": "string" }));
    let changes = classify_changes(&before, &after);
    let change = changes
        .iter()
        .find(|change| change.path.ends_with("/nickname"))
        .expect("nickname change");
    assert_eq!(change.kind, ChangeKind::NonBreaking);
    check_compatibility(&before, &after).expect("non-breaking");
}

#[test]
fn added_definition_is_non_breaking() {
    let before = baseline();
    let mut after = before.clone();
    after
        .get_mut("$defs")
        .and_then(Value::as_object_mut)
        .expect("$defs")
        .insert("Example".to_string(), json!({ "type": "object" }));
    let changes = classify_changes(&before, &after);
    assert!(changes
        .iter()
        .any(|change| change.path == "/$defs/Example" && change.kind == ChangeKind::NonBreaking));
}

#[test]
fn removed_definition_is_breaking() {
    let before = baseline();
    let mut after = before.clone();
    after
        .get_mut("$defs")
        .and_then(Value::as_object_mut)
        .expect("$defs")
        .remove("Portfolio");
    let changes = classify_changes(&before, &after);
    assert!(
        changes
            .iter()
            .any(|change| change.path == "/$defs/Portfolio" && change.kind == ChangeKind::Breaking)
    );
    let error = check_compatibility(&before, &after).expect_err("breaking change");
    assert!(error.iter().any(|change| change.path == "/$defs/Portfolio"));
}

#[test]
fn changed_field_type_is_breaking() {
    let before = baseline();
    let mut after = before.clone();
    let field = definition_mut(&mut after, "Portfolio")
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .expect("properties")
        .get_mut("name")
        .expect("name");
    field
        .as_object_mut()
        .expect("field")
        .insert("type".to_string(), json!("integer"));
    let changes = classify_changes(&before, &after);
    assert!(
        changes
            .iter()
            .any(|change| change.kind == ChangeKind::Breaking)
    );
}

#[test]
fn added_required_property_is_breaking() {
    let before = baseline();
    let mut after = before.clone();
    definition_mut(&mut after, "Portfolio")
        .get_mut("required")
        .and_then(Value::as_array_mut)
        .expect("required")
        .push(json!("nickname"));
    let changes = classify_changes(&before, &after);
    assert!(changes.iter().any(|change| {
        change.kind == ChangeKind::Breaking && change.description.contains("nickname")
    }));
}

#[test]
fn removed_closed_enum_value_is_breaking_and_added_is_non_breaking() {
    let before = baseline();
    let mut after = before.clone();
    let enum_values = definition_mut(&mut after, "Event")
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .expect("properties")
        .get_mut("event_type")
        .and_then(|field| field.get_mut("enum"))
        .and_then(Value::as_array_mut)
        .expect("enum");
    enum_values.retain(|value| value.as_str() != Some("buy"));
    let changes = classify_changes(&before, &after);
    assert!(changes
        .iter()
        .any(|change| change.kind == ChangeKind::Breaking && change.description.contains("buy")));

    let mut added = before.clone();
    let enum_values = definition_mut(&mut added, "Event")
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .expect("properties")
        .get_mut("event_type")
        .and_then(|field| field.get_mut("enum"))
        .and_then(Value::as_array_mut)
        .expect("enum");
    enum_values.push(json!("reorganization"));
    let changes = classify_changes(&before, &added);
    assert!(
        changes
            .iter()
            .any(|change| change.kind == ChangeKind::NonBreaking
                && change.description.contains("reorganization"))
    );
}

#[test]
fn breaking_change_fails_unless_the_schema_version_changes() {
    let before = baseline();
    let mut after = before.clone();
    after
        .get_mut("$defs")
        .and_then(Value::as_object_mut)
        .expect("$defs")
        .remove("Portfolio");
    assert!(check_compatibility(&before, &after).is_err());

    after
        .as_object_mut()
        .expect("document")
        .insert("x-obolargus-schema-version".to_string(), json!("9.9.9"));
    check_compatibility(&before, &after).expect("version bump allows breaking change");
}

#[test]
fn baseline_document_parses() {
    let document =
        parse_document(r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","$defs":{}}"#)
            .expect("parse");
    assert!(document.get("$defs").is_some());
}
