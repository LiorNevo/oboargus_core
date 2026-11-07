use serde_json::Value;

use crate::data_dictionary::export_system_schema;

const ARTIFACT: &str = include_str!("../../../schema/system-data-dictionary.json");

fn first_differing_definition(left: &Value, right: &Value) -> String {
    let left_definitions = left.get("$defs").and_then(Value::as_object);
    let right_definitions = right.get("$defs").and_then(Value::as_object);
    if let (Some(left_definitions), Some(right_definitions)) = (left_definitions, right_definitions)
    {
        for name in left_definitions
            .keys()
            .chain(right_definitions.keys())
            .collect::<std::collections::BTreeSet<_>>()
        {
            if left_definitions.get(name) != right_definitions.get(name) {
                return name.to_string();
            }
        }
    }
    "<document>".to_string()
}

#[test]
fn committed_artifact_matches_a_fresh_export() {
    let schema = export_system_schema().expect("schema");
    if ARTIFACT != schema.canonical_json() {
        let committed: Value = serde_json::from_str(ARTIFACT).expect("committed artifact");
        let fresh: Value =
            serde_json::from_str(schema.canonical_json()).expect("fresh canonical document");
        let differing = first_differing_definition(&committed, &fresh);
        panic!(
            "committed schema artifact is stale; first differing definition: {differing}; \
             run scripts/schema.sh --update"
        );
    }
}

#[test]
fn committed_artifact_is_canonical_json() {
    assert!(ARTIFACT.starts_with("{\n  \"$defs\""));
    assert!(ARTIFACT.ends_with("}\n"));
    assert!(!ARTIFACT.ends_with("\n\n"));
    let parsed: Value = serde_json::from_str(ARTIFACT).expect("artifact is JSON");
    assert!(parsed.get("$defs").and_then(Value::as_object).is_some());
}
