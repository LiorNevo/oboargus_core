use crate::data_dictionary::export_system_schema;
use crate::data_dictionary::version::{SYSTEM_SCHEMA_VERSION, model_version};

#[test]
fn schema_id_embeds_the_schema_version() {
    let schema = export_system_schema().expect("schema");
    let document = schema.document();
    let expected_id = format!("urn:obolargus:schema:system:{SYSTEM_SCHEMA_VERSION}");
    assert_eq!(
        document.get("$id").and_then(serde_json::Value::as_str),
        Some(expected_id.as_str())
    );
    assert_eq!(
        document
            .get("x-obolargus-schema-version")
            .and_then(serde_json::Value::as_str),
        Some(SYSTEM_SCHEMA_VERSION)
    );
}

#[test]
fn model_version_tracks_the_crate_version() {
    let schema = export_system_schema().expect("schema");
    assert_eq!(model_version(), env!("CARGO_PKG_VERSION"));
    assert_eq!(
        schema
            .document()
            .get("x-obolargus-model-version")
            .and_then(serde_json::Value::as_str),
        Some(model_version())
    );
}

#[test]
fn schema_version_is_semver() {
    let parts = SYSTEM_SCHEMA_VERSION.split('.').collect::<Vec<_>>();
    assert_eq!(parts.len(), 3);
    for part in parts {
        assert!(!part.is_empty());
        assert!(part.chars().all(|character| character.is_ascii_digit()));
    }
}
