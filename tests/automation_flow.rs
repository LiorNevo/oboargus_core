//! Flow document, schema, and structural validation tests (US1).

use obolargus_core::automation::flow::document::{FLOW_SCHEMA_VERSION, FlowDocument};
use obolargus_core::automation::flow::schema::flow_schema;
use obolargus_core::automation::flow::validate::validate_flow;
use obolargus_core::automation::nodes::default_registry;

fn fixture(name: &str) -> FlowDocument {
    let path = format!("tests/fixtures/automation/{name}.json");
    let text = std::fs::read_to_string(&path).expect("fixture should exist");
    serde_json::from_str(&text).expect("fixture should parse")
}

#[test]
fn document_round_trips_unchanged() {
    let document = fixture("data-loading");
    let encoded = serde_json::to_value(&document).expect("serialize");
    let decoded: FlowDocument = serde_json::from_value(encoded.clone()).expect("deserialize");
    assert_eq!(document, decoded);
    assert_eq!(document.schema_version, FLOW_SCHEMA_VERSION);
    assert_eq!(document.nodes.len(), 3);
    assert_eq!(document.wires.len(), 2);
}

#[test]
fn schema_is_generated_and_versioned() {
    let schema = flow_schema();
    assert_eq!(schema["title"], "FlowDocument");
    assert_eq!(schema["type"], "object");
    assert!(schema["properties"]["schema_version"].is_object());
    assert!(schema["properties"]["nodes"].is_object());
}

#[test]
fn fixtures_validate_against_the_built_in_registry() {
    let registry = default_registry();
    for name in [
        "data-loading",
        "multi-output",
        "cyclic",
        "subflow",
        "context",
    ] {
        let document = fixture(name);
        let report = validate_flow(&document, &registry);
        assert!(
            report.is_valid(),
            "fixture {name} should be valid: {:?}",
            report.errors
        );
    }
}

#[test]
fn dangling_wire_is_rejected() {
    let mut document = fixture("multi-output");
    document
        .wires
        .push(obolargus_core::automation::flow::document::Wire {
            id: "w9".to_string(),
            source: "n1".to_string(),
            source_port: "out".to_string(),
            target: "missing".to_string(),
            target_port: "in".to_string(),
        });
    let report = validate_flow(&document, &default_registry());
    assert!(
        report
            .errors
            .iter()
            .any(|finding| finding.code == "flow.dangling_wire")
    );
}

#[test]
fn duplicate_and_self_wires_are_rejected() {
    let mut document = fixture("multi-output");
    document.wires.push(document.wires[0].clone());
    document
        .wires
        .push(obolargus_core::automation::flow::document::Wire {
            id: "w-self".to_string(),
            source: "n3".to_string(),
            source_port: "out".to_string(),
            target: "n3".to_string(),
            target_port: "in".to_string(),
        });
    let report = validate_flow(&document, &default_registry());
    assert!(
        report
            .errors
            .iter()
            .any(|finding| finding.code == "flow.duplicate_wire")
    );
    assert!(
        report
            .errors
            .iter()
            .any(|finding| finding.code == "flow.self_wire")
    );
}

#[test]
fn unknown_node_type_is_rejected() {
    let mut document = fixture("multi-output");
    document.nodes[1].node_type = "not_a_node".to_string();
    let report = validate_flow(&document, &default_registry());
    assert!(
        report
            .errors
            .iter()
            .any(|finding| finding.code == "flow.unknown_type")
    );
}

#[test]
fn empty_flow_and_empty_name_are_rejected() {
    let document = FlowDocument::empty("");
    let report = validate_flow(&document, &default_registry());
    let codes: Vec<&str> = report
        .errors
        .iter()
        .map(|finding| finding.code.as_str())
        .collect();
    assert!(codes.contains(&"flow.empty"));
    assert!(codes.contains(&"flow.name_empty"));
}

#[test]
fn invalid_node_config_is_rejected() {
    let mut document = fixture("multi-output");
    document.nodes[1].config = serde_json::json!({ "rules": "not-an-array" });
    let report = validate_flow(&document, &default_registry());
    assert!(
        report
            .errors
            .iter()
            .any(|finding| finding.code == "node.config_invalid")
    );
}

#[test]
fn unknown_schema_version_is_rejected() {
    let mut document = fixture("multi-output");
    document.schema_version = 99;
    let report = validate_flow(&document, &default_registry());
    assert!(
        report
            .errors
            .iter()
            .any(|finding| finding.code == "flow.schema_version")
    );
}

#[test]
fn recursive_subflow_is_rejected() {
    let mut document = fixture("subflow");
    document.subflows[0].nodes.push(
        serde_json::from_value(serde_json::json!({
            "id": "s2",
            "type": "subflow",
            "type_version": 1,
            "config": { "subflow_id": "sf1" }
        }))
        .expect("node"),
    );
    let report = validate_flow(&document, &default_registry());
    assert!(
        report
            .errors
            .iter()
            .any(|finding| finding.code == "flow.subflow_recursive")
    );
}

#[test]
fn unknown_versions_and_invalid_triggers_are_reported() {
    let mut flow = fixture("data-loading");
    flow.nodes[0].type_version = 99;
    let report = validate_flow(&flow, &default_registry());
    assert!(
        report
            .errors
            .iter()
            .any(|finding| finding.code == "flow.unknown_version"),
        "{:?}",
        report.errors
    );

    let mut flow = fixture("data-loading");
    flow.nodes[0].config = serde_json::json!({
        "repeat": "cron",
        "cron_expression": "not a cron",
        "timezone": "UTC"
    });
    let report = validate_flow(&flow, &default_registry());
    assert!(
        report
            .errors
            .iter()
            .any(|finding| finding.code == "flow.trigger_invalid"),
        "{:?}",
        report.errors
    );
}

#[test]
fn embedded_secrets_are_rejected() {
    let mut flow = fixture("data-loading");
    flow.nodes[1].config = serde_json::json!({
        "identifiers": [],
        "as_of_date": "2026-01-02",
        "api_key": "super-secret"
    });
    let report = validate_flow(&flow, &default_registry());
    assert!(
        report
            .errors
            .iter()
            .any(|finding| finding.code == "flow.secret_in_config"),
        "{:?}",
        report.errors
    );
}
