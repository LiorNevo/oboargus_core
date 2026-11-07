use crate::data_dictionary::export_system_schema;

#[test]
fn document_has_required_envelope_members() {
    let schema = export_system_schema().expect("schema");
    let document = schema.document();
    assert_eq!(
        document.get("$schema").and_then(|value| value.as_str()),
        Some("https://json-schema.org/draft/2020-12/schema")
    );
    assert!(
        document
            .get("$id")
            .and_then(|value| value.as_str())
            .is_some()
    );
    assert!(
        document
            .get("title")
            .and_then(|value| value.as_str())
            .is_some()
    );
    assert!(
        document
            .get("description")
            .and_then(|value| value.as_str())
            .is_some()
    );
    assert!(
        document
            .get("x-obolargus-schema-version")
            .and_then(|value| value.as_str())
            .is_some()
    );
    assert!(
        document
            .get("x-obolargus-model-version")
            .and_then(|value| value.as_str())
            .is_some()
    );
    assert!(
        document
            .get("x-obolargus-relationships")
            .and_then(|value| value.as_array())
            .is_some()
    );
    assert!(
        document
            .get("x-obolargus-invariants")
            .and_then(|value| value.as_array())
            .is_some()
    );
    assert!(
        document
            .get("x-obolargus-cross-field-constraints")
            .and_then(|value| value.as_array())
            .is_some()
    );
    assert!(
        document
            .get("$defs")
            .and_then(|value| value.as_object())
            .is_some()
    );
}
