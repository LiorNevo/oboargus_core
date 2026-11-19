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

#[test]
fn bound_changes_are_classified_by_tightening() {
    let before = json!({
        "$defs": {
            "Thing": {
                "type": "string",
                "format": "date",
                "pattern": "^a$",
                "minLength": 2,
                "maxLength": 5,
                "minimum": 1,
                "maximum": 5
            }
        }
    });
    let mut after = before.clone();
    let thing = definition_mut(&mut after, "Thing");
    thing["format"] = json!("date-time");
    thing["pattern"] = json!("^b$");
    thing["minLength"] = json!(3);
    thing["maxLength"] = json!(6);
    thing["minimum"] = json!(0);
    thing["maximum"] = json!(4);
    let changes = classify_changes(&before, &after);
    let find = |suffix: &str| {
        changes
            .iter()
            .find(|change| change.path.ends_with(suffix))
            .unwrap_or_else(|| panic!("missing {suffix} in {changes:?}"))
    };
    assert_eq!(find("/format").kind, ChangeKind::Breaking);
    assert_eq!(find("/pattern").kind, ChangeKind::Breaking);
    assert_eq!(find("/minLength").kind, ChangeKind::Breaking);
    assert_eq!(find("/maxLength").kind, ChangeKind::NonBreaking);
    assert_eq!(find("/minimum").kind, ChangeKind::NonBreaking);
    assert_eq!(find("/maximum").kind, ChangeKind::Breaking);

    let mut added = before.clone();
    definition_mut(&mut added, "Thing")["minimum"] = json!(0);
    let changes = classify_changes(&before, &added);
    assert!(
        changes
            .iter()
            .any(|change| change.path.ends_with("/minimum")
                && change.kind == ChangeKind::NonBreaking)
    );

    let mut removed = before.clone();
    removed["$defs"]["Thing"]
        .as_object_mut()
        .expect("thing")
        .remove("maximum");
    let changes = classify_changes(&before, &removed);
    assert!(changes.iter().any(|change| {
        change.path.ends_with("/maximum")
            && change.kind == ChangeKind::Breaking
            && change.description.contains("added or removed")
    }));
}

#[test]
fn required_properties_and_removals_are_classified() {
    let before = json!({
        "$defs": {
            "Thing": {
                "required": ["a", "b"],
                "properties": {
                    "a": {"type": "string"},
                    "b": {"type": "string"}
                }
            }
        }
    });
    let mut after = before.clone();
    let thing = definition_mut(&mut after, "Thing");
    thing["required"] = json!(["a"]);
    thing["properties"]
        .as_object_mut()
        .expect("properties")
        .remove("b");
    let changes = classify_changes(&before, &after);
    assert!(changes.iter().any(|change| {
        change.path.ends_with("/required")
            && change.kind == ChangeKind::NonBreaking
            && change.description.contains('b')
    }));
    assert!(changes.iter().any(|change| {
        change.path.ends_with("/properties/b")
            && change.kind == ChangeKind::Breaking
            && change.description == "property removed"
    }));
}

#[test]
fn top_level_array_members_are_classified() {
    let before = json!({
        "x-obolargus-relationships": [
            {"from": "Asset", "to": "Portfolio"},
            {"from": "Portfolio", "to": "Asset"}
        ],
        "x-obolargus-invariants": ["first", "second"]
    });
    let after = json!({
        "x-obolargus-relationships": [{"from": "Asset", "to": "Portfolio"}],
        "x-obolargus-invariants": ["first", "third"]
    });
    let changes = classify_changes(&before, &after);
    assert!(changes.iter().any(|change| {
        change.path == "/x-obolargus-relationships"
            && change.kind == ChangeKind::Breaking
            && change.description.contains("Portfolio")
    }));
    assert!(changes.iter().any(|change| {
        change.path == "/x-obolargus-invariants"
            && change.kind == ChangeKind::Breaking
            && change.description.contains("second")
    }));
    assert!(changes.iter().any(|change| {
        change.path == "/x-obolargus-invariants"
            && change.kind == ChangeKind::NonBreaking
            && change.description.contains("third")
    }));

    // Missing or malformed members are ignored rather than treated as changes.
    let changes = classify_changes(&json!({}), &json!({}));
    assert!(changes.is_empty());
    let changes = classify_changes(
        &json!({"x-obolargus-relationships": "not an array"}),
        &json!({"x-obolargus-relationships": []}),
    );
    assert!(changes.is_empty());
}

#[test]
fn definitions_accessor_and_parse_errors() {
    let document = json!({"$defs": {"A": {"type": "string"}}});
    assert!(crate::data_dictionary::compat::definitions(&document).is_some());
    assert!(crate::data_dictionary::compat::definitions(&json!({})).is_none());
    assert!(parse_document("{").is_err());
}
