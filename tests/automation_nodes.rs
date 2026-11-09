//! Node registry, descriptor, and generated-schema tests (US7).

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use obolargus_core::automation::docs::{node_types_document, node_types_json};
use obolargus_core::automation::error::NodeExecutionError;
use obolargus_core::automation::nodes::{
    NodeCategory, NodeContext, NodeOutput, WorkflowNode, default_registry,
};

#[test]
fn node_type_catalog_fixture_is_current() {
    let expected = include_str!("../schema/automation-node-types.json");
    assert_eq!(
        expected,
        node_types_json(),
        "regenerate the catalog fixture"
    );
}

#[test]
#[ignore = "regenerates the checked-in fixture"]
fn regenerate_node_type_catalog_fixture() {
    std::fs::write("schema/automation-node-types.json", node_types_json()).expect("write fixture");
}

#[test]
fn every_built_in_type_has_a_versioned_descriptor() {
    let registry = default_registry();
    let descriptors = registry.latest_descriptors();
    assert!(descriptors.len() >= 15);
    for descriptor in descriptors {
        assert_eq!(
            descriptor.key(),
            format!("{}@{}", descriptor.id, descriptor.version)
        );
        assert!(!descriptor.display_name.is_empty());
        assert!(!descriptor.docs.is_empty());
        assert_eq!(descriptor.config_schema["type"], "object");
    }
    let document = node_types_document();
    assert_eq!(document["schema_version"], 1);
    assert!(
        document["items"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct ExampleConfig {
    /// Greeting text.
    message: String,
}

struct ExampleNode {
    version: u32,
    deprecated: bool,
}

#[async_trait]
impl WorkflowNode for ExampleNode {
    fn descriptor(&self) -> obolargus_core::automation::nodes::NodeTypeDescriptor {
        let mut descriptor = obolargus_core::automation::nodes::common::descriptor::<ExampleConfig>(
            "example",
            self.version,
            NodeCategory::Transform,
            "Example",
            "Documented example node type.",
            "extension",
            None,
            vec![obolargus_core::automation::nodes::common::output_port()],
            Vec::new(),
            "Add a node type by implementing WorkflowNode and registering it.",
        );
        descriptor.deprecated = self.deprecated;
        descriptor
    }

    async fn execute(&self, ctx: NodeContext<'_>) -> Result<NodeOutput, NodeExecutionError> {
        let config: ExampleConfig = serde_json::from_value(ctx.config.clone())
            .map_err(|error| NodeExecutionError::new("node.config_invalid", error.to_string()))?;
        let mut output = NodeOutput::empty();
        output.push("out", json!({ "payload": config.message }));
        Ok(output)
    }
}

#[test]
fn new_node_types_register_without_runtime_changes() {
    let mut registry = default_registry();
    registry.register(Arc::new(ExampleNode {
        version: 1,
        deprecated: false,
    }));
    registry.register(Arc::new(ExampleNode {
        version: 2,
        deprecated: false,
    }));
    assert!(registry.get("example@1").is_some());
    assert!(registry.get("example@2").is_some());
    let latest = registry
        .latest_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.id == "example")
        .expect("latest example");
    assert_eq!(latest.version, 2);
}

#[test]
fn deprecated_types_are_excluded_from_the_palette() {
    let mut registry = default_registry();
    registry.register(Arc::new(ExampleNode {
        version: 1,
        deprecated: true,
    }));
    assert!(
        registry
            .latest_descriptors()
            .iter()
            .all(|descriptor| descriptor.id != "example")
    );
    assert!(registry.get("example@1").is_some());
}

#[test]
fn pinned_versions_resolve_exactly() {
    let mut registry = default_registry();
    registry.register(Arc::new(ExampleNode {
        version: 1,
        deprecated: false,
    }));
    registry.register(Arc::new(ExampleNode {
        version: 2,
        deprecated: true,
    }));
    assert!(registry.get("example@1").is_some());
    assert!(registry.get("example@2").is_some());
    assert!(registry.get("example@3").is_none());
    let latest = registry
        .latest_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.id == "example")
        .expect("latest example");
    assert_eq!(latest.version, 1);
}

#[test]
fn capability_allowlist_blocks_flows_that_need_denied_capabilities() {
    use obolargus_core::automation::flow::document::FlowDocument;
    use obolargus_core::automation::flow::validate::validate_flow;

    let flow: FlowDocument = serde_json::from_value(json!({
        "schema_version": 1,
        "name": "capability",
        "nodes": [
            { "id": "n1", "type": "inject", "type_version": 1, "config": { "repeat": "none", "once": true } },
            {
                "id": "n2",
                "type": "data_load",
                "type_version": 1,
                "config": { "identifiers": [{ "scheme": "TICKER", "value": "XNAS:AAPL" }], "as_of_date": "2026-01-02", "dry_run": true }
            }
        ],
        "wires": [
            { "id": "w1", "source": "n1", "source_port": "out", "target": "n2", "target_port": "in" }
        ]
    }))
    .expect("flow");

    let denied = default_registry().with_capabilities("read:market_data");
    let report = validate_flow(&flow, &denied);
    assert!(
        report
            .errors
            .iter()
            .any(|finding| finding.code == "node.capability_missing"),
        "{:?}",
        report.errors
    );

    let allowed = default_registry().with_capabilities("all");
    assert!(validate_flow(&flow, &allowed).is_valid());
}
