//! Additional flow-validation coverage for the structural, policy, trigger,
//! wire, and subflow branches that the happy-path fixtures do not reach.

use async_trait::async_trait;
use serde_json::{Value, json};

use obolargus_core::automation::error::NodeExecutionError;
use obolargus_core::automation::flow::document::{
    FlowDocument, FlowNode, NodePosition, RetryPolicy, RuntimeInput, Subflow, SubflowInput,
    SubflowOutput, Wire,
};
use obolargus_core::automation::flow::validate::validate_flow;
use obolargus_core::automation::nodes::{
    NodeCategory, NodeContext, NodeOutput, NodeRegistry, NodeTypeDescriptor, WorkflowNode,
    default_registry,
};

fn node(id: &str, node_type: &str, config: Value) -> FlowNode {
    FlowNode {
        id: id.to_string(),
        node_type: node_type.to_string(),
        type_version: 1,
        label: String::new(),
        position: NodePosition::default(),
        config,
        retry: RetryPolicy::default(),
        timeout_seconds: 300,
        error_policy: "fail".to_string(),
    }
}

fn wire(id: &str, source: &str, source_port: &str, target: &str, target_port: &str) -> Wire {
    Wire {
        id: id.to_string(),
        source: source.to_string(),
        source_port: source_port.to_string(),
        target: target.to_string(),
        target_port: target_port.to_string(),
    }
}

fn codes(document: &FlowDocument, registry: &NodeRegistry) -> (Vec<String>, Vec<String>) {
    let report = validate_flow(document, registry);
    (
        report
            .errors
            .iter()
            .map(|finding| finding.code.clone())
            .collect(),
        report
            .warnings
            .iter()
            .map(|finding| finding.code.clone())
            .collect(),
    )
}

fn has(codes: &[String], code: &str) -> bool {
    codes.iter().any(|candidate| candidate == code)
}

#[test]
fn document_limits_and_runtime_inputs_are_enforced() {
    let mut document = FlowDocument::empty("limits");
    document.description = "d".repeat(4_001);
    document.inputs = vec![
        RuntimeInput {
            name: "1starts-with-a-digit".to_string(),
            input_type: "string".to_string(),
            required: false,
            default: None,
            secret: false,
            description: None,
        },
        RuntimeInput {
            name: "duplicate".to_string(),
            input_type: "string".to_string(),
            required: false,
            default: None,
            secret: false,
            description: None,
        },
        RuntimeInput {
            name: "duplicate".to_string(),
            input_type: "unsupported".to_string(),
            required: false,
            default: None,
            secret: false,
            description: None,
        },
        RuntimeInput {
            name: "x".repeat(101),
            input_type: "array".to_string(),
            required: false,
            default: None,
            secret: false,
            description: Some("d".repeat(201)),
        },
        RuntimeInput {
            name: "_valid".to_string(),
            input_type: "object".to_string(),
            required: false,
            default: None,
            secret: false,
            description: Some("short".to_string()),
        },
    ];

    // Structural nodes are skipped by node validation, which keeps the
    // over-limit document cheap while still exercising the document guards.
    for index in 0..1_001 {
        document
            .nodes
            .push(node(&format!("s{index}"), "subflow_input", json!({})));
    }
    for index in 0..5_001 {
        document.wires.push(wire(
            &format!("w{index}"),
            "missing-source",
            "out",
            "missing-target",
            "in",
        ));
    }

    let (errors, _) = codes(&document, &default_registry());
    assert!(has(&errors, "flow.name_empty"));
    assert!(has(&errors, "flow.definition_too_large"));
    assert!(has(&errors, "flow.inputs_invalid"));
    assert!(has(&errors, "flow.dangling_wire"));
    assert!(
        errors
            .iter()
            .filter(|code| *code == "flow.inputs_invalid")
            .count()
            >= 5
    );
}

#[test]
fn node_identity_and_policy_limits_are_enforced() {
    let mut document = FlowDocument::empty("node policies");
    let mut empty_id = node("", "debug", json!({}));
    empty_id.label = "l".repeat(201);
    empty_id.retry = RetryPolicy {
        max_attempts: 0,
        backoff_seconds: 3_601,
    };
    empty_id.timeout_seconds = 0;
    document.nodes.push(empty_id);

    let mut long_id = node(&"n".repeat(101), "debug", json!({}));
    long_id.retry = RetryPolicy {
        max_attempts: 11,
        backoff_seconds: 0,
    };
    long_id.timeout_seconds = 86_401;
    long_id.error_policy = "explode".to_string();
    document.nodes.push(long_id);

    let (errors, _) = codes(&document, &default_registry());
    assert!(has(&errors, "flow.duplicate_id"));
    assert!(has(&errors, "node.config_invalid"));
    assert!(
        errors
            .iter()
            .filter(|code| *code == "node.policy_invalid")
            .count()
            >= 5
    );
}

#[test]
fn every_trigger_and_data_load_branch_is_reported() {
    let mut document = FlowDocument::empty("trigger branches");
    document.nodes = vec![
        node(
            "interval",
            "inject",
            json!({ "repeat": "interval", "interval_seconds": 0 }),
        ),
        node("cron-missing", "inject", json!({ "repeat": "cron" })),
        node(
            "cron-valid",
            "inject",
            json!({ "repeat": "cron", "cron_expression": "0 * * * * *", "timezone": "UTC" }),
        ),
        node(
            "cron-bad-zone",
            "inject",
            json!({ "repeat": "cron", "cron": "0 * * * * *", "timezone": "Mars/Olympus" }),
        ),
        node("unknown", "inject", json!({ "repeat": "hourly" })),
        node("payload", "inject", json!({ "payload_type": "date" })),
        node(
            "secret-array",
            "inject",
            json!({ "payload": [{ "nested": { "api_key": "abc" } }] }),
        ),
        node(
            "secret-key",
            "inject",
            json!({ "payload": { "Authorization": "Bearer x" } }),
        ),
        node(
            "data-load",
            "data_load",
            json!({
                "identifiers": {},
                "as_of_date": "  ",
                "create_missing_assets": true,
                "default_currency": "   "
            }),
        ),
        node(
            "data-load-range",
            "data_load_range",
            json!({
                "identifiers": ["AAPL"],
                "from": { "kind": "reference", "path": "inputs.from" },
                "to": []
            }),
        ),
    ];

    let (errors, _) = codes(&document, &default_registry());
    assert!(has(&errors, "flow.trigger_invalid"));
    assert!(
        errors
            .iter()
            .filter(|code| *code == "flow.trigger_invalid")
            .count()
            >= 5
    );
    assert!(
        errors
            .iter()
            .filter(|code| *code == "flow.secret_in_config")
            .count()
            >= 2
    );
    assert!(
        errors
            .iter()
            .filter(|code| *code == "node.config_invalid")
            .count()
            >= 4
    );
}

#[test]
fn wire_validation_covers_ports_structure_and_duplicates() {
    let mut document = FlowDocument::empty("wire branches");
    document.nodes = vec![
        node("n1", "inject", json!({ "repeat": "none" })),
        node("n2", "debug", json!({})),
        node("n3", "change", json!({ "rules": [] })),
        node("si", "subflow_input", json!({})),
        node("so", "subflow_output", json!({})),
        node("n4", "inject", json!({ "repeat": "none" })),
    ];
    document.wires = vec![
        wire("w-missing-source", "ghost", "out", "n2", "in"),
        wire("w-missing-target", "n1", "out", "ghost", "in"),
        wire("w-self", "n1", "out", "n1", "in"),
        wire("w-structural-target", "n1", "out", "si", "in"),
        wire("w-structural-source", "so", "out", "n2", "in"),
        wire("w-unknown-output", "n1", "bogus", "n2", "in"),
        wire("w-bad-target-port", "n1", "out", "n2", "other"),
        wire("w-dup-a", "n1", "out", "n2", "in"),
        wire("w-dup-b", "n1", "out", "n2", "in"),
    ];

    let (errors, warnings) = codes(&document, &default_registry());
    assert!(
        errors
            .iter()
            .filter(|code| *code == "flow.dangling_wire")
            .count()
            >= 4
    );
    assert!(has(&errors, "flow.self_wire"));
    assert!(
        errors
            .iter()
            .filter(|code| *code == "flow.subflow_io")
            .count()
            >= 2
    );
    assert!(has(&errors, "flow.duplicate_wire"));
    assert!(has(&warnings, "flow.no_input"));
    assert!(has(&warnings, "flow.output_unused"));
}

#[test]
fn subflow_declarations_and_internal_wires_are_checked() {
    let mut document = FlowDocument::empty("subflow branches");
    document.nodes.push(node("instance", "subflow", json!({})));
    document.nodes.push(node(
        "dangling-instance",
        "subflow",
        json!({ "subflow_id": "gone" }),
    ));

    let mut bad = Subflow {
        id: "bad".to_string(),
        name: "Bad subflow".to_string(),
        inputs: Vec::new(),
        outputs: vec![SubflowOutput {
            id: "ghost".to_string(),
            label: String::new(),
        }],
        nodes: vec![
            node("dup", "change", json!({ "rules": [] })),
            node("dup", "change", json!({ "rules": [] })),
            node("no-config", "subflow", json!({})),
            node("missing-target", "subflow", json!({ "subflow_id": "gone" })),
            node("out-a", "subflow_output", json!({})),
            node("out-b", "subflow_output", json!({})),
        ],
        wires: vec![
            wire("sw-missing-source", "ghost", "out", "dup", "in"),
            wire("sw-missing-target", "dup", "out", "ghost", "in"),
            wire("sw-targets-input", "out-a", "out", "out-b", "in"),
            wire("sw-self", "out-b", "out", "out-b", "in"),
            wire("sw-dup-a", "dup", "out", "out-a", "in"),
            wire("sw-dup-b", "dup", "out", "out-a", "in"),
            wire("sw-bad-port", "dup", "bogus", "out-a", "in"),
            wire("sw-bad-target-port", "dup", "out", "out-a", "sideways"),
        ],
    };
    // Every wire above targets an output node (which is structural) except the
    // self and duplicate pair; add an input node so the structural-input
    // branch is reachable too.
    bad.nodes.push(node("in", "subflow_input", json!({})));
    bad.inputs.push(SubflowInput {
        id: "mismatch".to_string(),
        label: String::new(),
    });
    bad.wires
        .push(wire("sw-targets-input-2", "dup", "out", "in", "in"));
    bad.wires
        .push(wire("sw-input-bad-port", "in", "sideways", "dup", "in"));
    document.subflows.push(bad);

    let (errors, _) = codes(&document, &default_registry());
    assert!(has(&errors, "flow.subflow_missing"));
    assert!(has(&errors, "flow.subflow_io"));
    assert!(has(&errors, "flow.dangling_wire"));
    assert!(has(&errors, "flow.self_wire"));
    assert!(has(&errors, "flow.duplicate_id"));
    assert!(has(&errors, "flow.duplicate_wire"));
}

#[test]
fn mutually_recursive_subflows_are_rejected() {
    let mut document = FlowDocument::empty("recursive");
    let subflow = |id: &str, target: &str| Subflow {
        id: id.to_string(),
        name: id.to_string(),
        inputs: vec![SubflowInput {
            id: "in".to_string(),
            label: String::new(),
        }],
        outputs: vec![SubflowOutput {
            id: "out".to_string(),
            label: String::new(),
        }],
        nodes: vec![
            node("in", "subflow_input", json!({})),
            node("next", "subflow", json!({ "subflow_id": target })),
            node("out", "subflow_output", json!({})),
        ],
        wires: vec![
            wire("a", "in", "out", "next", "in"),
            wire("b", "next", "out", "out", "in"),
        ],
    };
    document.subflows.push(subflow("a", "b"));
    document.subflows.push(subflow("b", "a"));

    let (errors, _) = codes(&document, &default_registry());
    assert!(
        errors
            .iter()
            .filter(|code| *code == "flow.subflow_recursive")
            .count()
            >= 2
    );
}

struct LegacyNode;

#[async_trait]
impl WorkflowNode for LegacyNode {
    fn descriptor(&self) -> NodeTypeDescriptor {
        NodeTypeDescriptor {
            id: "legacy".to_string(),
            version: 1,
            category: NodeCategory::Transform,
            display_name: "Legacy".to_string(),
            description: "Deprecated test node".to_string(),
            icon: "history".to_string(),
            input: None,
            outputs: Vec::new(),
            config_schema: json!({ "type": 5 }),
            output_schema: json!({}),
            capabilities: vec!["write:legacy".to_string()],
            deprecated: true,
            docs: String::new(),
        }
    }

    async fn execute(&self, _ctx: NodeContext<'_>) -> Result<NodeOutput, NodeExecutionError> {
        Ok(NodeOutput::empty())
    }
}

#[test]
fn deprecated_capability_and_schema_branches_are_reported() {
    let mut registry = NodeRegistry::new().with_capabilities("");
    registry.register(std::sync::Arc::new(LegacyNode));

    let mut document = FlowDocument::empty("legacy");
    document.nodes.push(node("legacy-1", "legacy", json!({})));

    let (errors, warnings) = codes(&document, &registry);
    assert!(has(&warnings, "node.deprecated"));
    assert!(has(&errors, "node.capability_missing"));
    assert!(has(&errors, "node.config_invalid"));
}

#[test]
fn structurally_typed_nodes_skip_generic_validation() {
    let mut document = FlowDocument::empty("structural");
    document
        .nodes
        .push(node("in", "subflow_input", json!({ "anything": true })));
    document
        .nodes
        .push(node("out", "subflow_output", json!({ "anything": true })));

    let report = validate_flow(&document, &default_registry());
    assert!(report.errors.is_empty(), "{:?}", report.errors);
}
