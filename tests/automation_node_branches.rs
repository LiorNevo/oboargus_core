//! Branch coverage for built-in node execution and subflow resolution.

use std::collections::BTreeMap;

use chrono::Utc;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use obolargus_core::automation::flow::document::{
    FlowNode, NodePosition, RetryPolicy, Subflow, SubflowInput, SubflowOutput, Wire,
};
use obolargus_core::automation::nodes::change::ChangeNode;
use obolargus_core::automation::nodes::data_load_range::DataLoadRangeNode;
use obolargus_core::automation::nodes::switch::SwitchNode;
use obolargus_core::automation::nodes::{
    NodeCategory, NodeContext, NodeOutput, NoopLogger, NoopSecrets, WorkflowNode,
};
use obolargus_core::automation::runtime::bus::RunMetadata;
use obolargus_core::automation::runtime::context::{ContextScope, ContextStore};
use obolargus_core::automation::runtime::subflow;
use obolargus_core::market_data::registry::DataSourceRegistry;

fn run_metadata() -> RunMetadata {
    RunMetadata {
        id: Uuid::new_v4(),
        flow_id: Uuid::new_v4(),
        flow_version_id: Uuid::new_v4(),
        trigger_kind: "manual".to_string(),
        trigger_node_id: None,
        started_at: Utc::now(),
        actor_id: None,
        inputs: json!({ "symbol": "AAPL", "rows": 5 }),
    }
}

struct Fixture {
    context: ContextStore,
    market_data: DataSourceRegistry,
}

impl Fixture {
    fn new() -> Self {
        Self {
            context: ContextStore::new("flow", "global", BTreeMap::new(), BTreeMap::new()),
            market_data: DataSourceRegistry::new(Vec::new()).expect("empty registry"),
        }
    }

    fn node<'a>(
        &'a self,
        config: Value,
        msg: Value,
        secrets: &'a NoopSecrets,
        logger: &'a NoopLogger,
    ) -> NodeContext<'a> {
        NodeContext {
            run: run_metadata(),
            node_id: "node-1",
            config,
            msg,
            context: &self.context,
            pool: None,
            market_data: &self.market_data,
            secrets,
            cancel: CancellationToken::new(),
            logger,
        }
    }
}

async fn execute(
    node: &dyn WorkflowNode,
    fixture: &Fixture,
    config: Value,
    msg: Value,
) -> Result<NodeOutput, obolargus_core::automation::error::NodeExecutionError> {
    let secrets = NoopSecrets;
    let logger = NoopLogger;
    node.execute(fixture.node(config, msg, &secrets, &logger))
        .await
}

#[test]
fn category_names_and_output_helpers_are_total() {
    assert_eq!(NodeCategory::Trigger.as_str(), "trigger");
    assert_eq!(NodeCategory::Data.as_str(), "data");
    assert_eq!(NodeCategory::Transform.as_str(), "transform");
    assert_eq!(NodeCategory::Flow.as_str(), "flow");
    assert_eq!(NodeCategory::Error.as_str(), "error");
    assert_eq!(NodeCategory::Notify.as_str(), "notify");
    assert_eq!(NodeCategory::Output.as_str(), "output");

    let mut output = NodeOutput::empty();
    assert!(output.is_empty());
    output = output.with_status(obolargus_core::automation::nodes::NodeStatusEvent {
        status: "running".to_string(),
        detail: Some(json!({ "step": 1 })),
    });
    assert_eq!(
        output.status.as_ref().map(|status| status.status.as_str()),
        Some("running")
    );
    output.push("out", json!({ "ok": true }));
    assert!(!output.is_empty());
    assert_eq!(output.messages["out"][0], json!({ "ok": true }));

    let secrets = NoopSecrets;
    assert_eq!(
        obolargus_core::automation::nodes::SecretResolver::resolve(&secrets, "missing"),
        None
    );
}

#[tokio::test]
async fn context_config_accessors_report_missing_and_wrong_types() {
    let fixture = Fixture::new();
    let secrets = NoopSecrets;
    let logger = NoopLogger;
    let ctx = fixture.node(
        json!({ "present": "value", "number": 3 }),
        json!({}),
        &secrets,
        &logger,
    );

    assert_eq!(
        ctx.config_field("present").expect("present"),
        &json!("value")
    );
    let missing = ctx.config_field("absent").expect_err("absent");
    assert_eq!(missing.code, "node.config_invalid");
    assert_eq!(
        ctx.resolved_string("present").expect("string"),
        "value".to_string()
    );
    let wrong_type = ctx.resolved_string("number").expect_err("number");
    assert_eq!(wrong_type.code, "node.config_invalid");
}

#[tokio::test]
async fn change_node_sets_changes_and_deletes_message_paths() {
    let fixture = Fixture::new();
    let node = ChangeNode;
    let message = json!({ "payload": { "text": "a-b-a" }, "keep": true });

    let output = execute(
        &node,
        &fixture,
        json!({
            "rules": [
                { "action": "set", "target": "msg", "path": "payload.count", "value": 2 },
                { "action": "set", "target": "msg", "path": "missing.deep.value", "value": { "kind": "literal", "value": "made" } },
                { "action": "set", "target": "msg", "path": "payload.nothing" },
                { "action": "change", "target": "msg", "path": "payload.text", "from": "a", "to": "x" },
                { "action": "change", "target": "msg", "path": "payload.text", "from": "x" },
                { "action": "change", "target": "msg", "path": "ghost.text", "from": "a", "to": "b" },
                { "action": "change", "target": "msg", "path": "payload.count", "from": "2", "to": "3" },
                { "action": "delete", "target": "msg", "path": "payload.count" },
                { "action": "delete", "target": "msg", "path": "ghost.child" },
                { "action": "unknown", "target": "msg", "path": "keep" }
            ]
        }),
        message,
    )
    .await
    .expect("change output");
    let changed = &output.messages["out"][0];
    assert_eq!(changed["payload"]["text"], json!("-b-"));
    assert_eq!(changed["payload"]["nothing"], Value::Null);
    assert_eq!(changed["missing"]["deep"]["value"], json!("made"));
    assert_eq!(changed["keep"], json!(true));
    assert!(changed["payload"].get("count").is_none());

    let output = execute(
        &node,
        &fixture,
        json!({
            "rules": [
                { "action": "set", "target": "msg", "path": "", "value": "whole" }
            ]
        }),
        json!({ "payload": 1 }),
    )
    .await
    .expect("whole message");
    assert_eq!(output.messages["out"][0], json!("whole"));

    let output = execute(
        &node,
        &fixture,
        json!({
            "rules": [
                { "action": "delete", "target": "msg", "path": "" }
            ]
        }),
        json!({ "payload": 1 }),
    )
    .await
    .expect("whole delete");
    assert_eq!(output.messages["out"][0], Value::Null);
}

#[tokio::test]
async fn change_node_writes_context_and_validates_targets() {
    let fixture = Fixture::new();
    let node = ChangeNode;

    execute(
        &node,
        &fixture,
        json!({
            "rules": [
                { "action": "set", "target": "flow", "key": "flow-key", "value": 1 },
                { "action": "set", "target": "global", "key": "global-key", "value": 2 },
                { "action": "set", "target": "run", "key": "run-key", "value": 3 },
                { "action": "unknown", "target": "flow", "key": "flow-key" },
                { "action": "delete", "target": "flow", "key": "flow-key" },
                { "action": "delete", "target": "global", "key": "global-key" },
                { "action": "delete", "target": "run", "key": "run-key" }
            ]
        }),
        json!({}),
    )
    .await
    .expect("context changes");
    assert!(fixture.context.snapshot(ContextScope::Flow).is_empty());
    assert!(fixture.context.snapshot(ContextScope::Global).is_empty());
    assert!(fixture.context.snapshot(ContextScope::Run).is_empty());

    let error = execute(
        &node,
        &fixture,
        json!({ "rules": [{ "action": "set", "target": "msg", "value": 1 }] }),
        json!({}),
    )
    .await
    .expect_err("missing msg path");
    assert_eq!(error.code, "node.config_invalid");

    let error = execute(
        &node,
        &fixture,
        json!({ "rules": [{ "action": "set", "target": "flow", "value": 1 }] }),
        json!({}),
    )
    .await
    .expect_err("missing context key");
    assert_eq!(error.code, "node.config_invalid");

    let error = execute(
        &node,
        &fixture,
        json!({ "rules": [{ "action": "set", "target": "nowhere", "key": "k", "value": 1 }] }),
        json!({}),
    )
    .await
    .expect_err("unknown target");
    assert_eq!(error.code, "node.config_invalid");

    let error = execute(
        &node,
        &fixture,
        json!({ "rules": "not an array" }),
        json!({}),
    )
    .await
    .expect_err("invalid config");
    assert_eq!(error.code, "node.config_invalid", "rules must be an array");
}

#[tokio::test]
async fn switch_node_covers_every_operator_and_comparison() {
    let fixture = Fixture::new();
    let node = SwitchNode;
    let message = json!({ "payload": "alpha" });
    let config = json!({
        "property": "msg.payload",
        "check_all": true,
        "rules": [
            { "operator": "eq", "value": "alpha", "output": "eq" },
            { "operator": "neq", "value": "beta", "output": "neq" },
            { "operator": "lt", "value": "beta", "output": "lt" },
            { "operator": "lte", "value": "alpha", "output": "lte" },
            { "operator": "gt", "value": "aardvark", "output": "gt" },
            { "operator": "gte", "value": "alpha", "output": "gte" },
            { "operator": "contains", "value": "lph", "output": "contains-string" },
            { "operator": "regex", "value": "^al.*a$", "output": "regex" },
            { "operator": "is_not_null", "output": "not-null" },
            { "operator": "unknown", "value": "alpha", "output": "unknown" }
        ]
    });
    let output = execute(&node, &fixture, config, message)
        .await
        .expect("switch");
    for port in [
        "eq",
        "neq",
        "lt",
        "lte",
        "gt",
        "gte",
        "contains-string",
        "regex",
        "not-null",
    ] {
        assert!(output.messages.contains_key(port), "missing {port}");
    }
    assert!(!output.messages.contains_key("unknown"));

    let number_config = json!({
        "property": { "kind": "expression", "expression": "inputs.symbol" },
        "check_all": true,
        "rules": [
            { "operator": "lt", "value": 1, "output": "lt" },
            { "operator": "lte", "value": 1, "output": "lte" },
            { "operator": "gt", "value": 1, "output": "gt" },
            { "operator": "gte", "value": 1, "output": "gte" },
            { "operator": "contains", "value": "A", "output": "contains" },
            { "operator": "contains", "value": "A", "output": "contains-array" },
            { "operator": "contains", "value": "A", "output": "contains-object" },
            { "operator": "contains", "value": 1, "output": "contains-other" }
        ]
    });
    let output = execute(&node, &fixture, number_config, json!({}))
        .await
        .expect("switch");
    assert!(!output.messages.contains_key("lt"));
    assert!(!output.messages.contains_key("lte"));
    assert!(!output.messages.contains_key("gt"));
    assert!(!output.messages.contains_key("gte"));
    assert!(output.messages.contains_key("contains"));

    let numeric = execute(
        &node,
        &fixture,
        json!({
            "property": { "kind": "expression", "expression": "inputs.rows" },
            "check_all": true,
            "rules": [
                { "operator": "lt", "value": 7, "output": "lt" },
                { "operator": "lte", "value": 5, "output": "lte" },
                { "operator": "gt", "value": 3, "output": "gt" },
                { "operator": "gte", "value": 5, "output": "gte" }
            ]
        }),
        json!({}),
    )
    .await
    .expect("numeric comparisons");
    for port in ["lt", "lte", "gt", "gte"] {
        assert!(numeric.messages.contains_key(port), "missing {port}");
    }
}

#[tokio::test]
async fn switch_node_handles_collections_regex_failures_and_defaults() {
    let fixture = Fixture::new();
    let node = SwitchNode;

    let array = execute(
        &node,
        &fixture,
        json!({
            "property": "msg.payload",
            "check_all": true,
            "rules": [
                { "operator": "contains", "value": "b", "output": "array" },
                { "operator": "regex", "value": "[", "output": "bad-regex" }
            ]
        }),
        json!({ "payload": ["a", "b"] }),
    )
    .await
    .expect("array");
    assert!(array.messages.contains_key("array"));
    assert!(!array.messages.contains_key("bad-regex"));

    let object = execute(
        &node,
        &fixture,
        json!({
            "property": "msg.payload",
            "rules": [
                { "operator": "contains", "value": "key", "output": "object" },
                { "operator": "contains", "value": "missing", "output": "object-miss" }
            ]
        }),
        json!({ "payload": { "key": 1 } }),
    )
    .await
    .expect("object");
    assert!(object.messages.contains_key("object"));
    assert!(!object.messages.contains_key("object-miss"));

    let scalar = execute(
        &node,
        &fixture,
        json!({
            "property": "msg.payload",
            "rules": [
                { "operator": "contains", "value": "x", "output": "scalar" },
                { "operator": "regex", "value": "x", "output": "scalar-regex" }
            ]
        }),
        json!({ "payload": 7 }),
    )
    .await
    .expect("scalar");
    assert!(scalar.messages.contains_key("default"));
    assert!(!scalar.messages.contains_key("scalar"));
    assert!(!scalar.messages.contains_key("scalar-regex"));

    let no_default = execute(
        &node,
        &fixture,
        json!({
            "property": "msg.payload",
            "default_output": null,
            "rules": [{ "operator": "eq", "value": "other", "output": "eq" }]
        }),
        json!({ "payload": "alpha" }),
    )
    .await
    .expect("no default");
    assert!(no_default.messages.is_empty());

    let first_match = execute(
        &node,
        &fixture,
        json!({
            "property": "msg.payload",
            "check_all": false,
            "rules": [
                { "operator": "eq", "value": "alpha", "output": "first" },
                { "operator": "eq", "value": "alpha", "output": "second" }
            ]
        }),
        json!({ "payload": "alpha" }),
    )
    .await
    .expect("first match");
    assert!(first_match.messages.contains_key("first"));
    assert!(!first_match.messages.contains_key("second"));

    let null_subject = execute(
        &node,
        &fixture,
        json!({
            "property": "msg.payload",
            "rules": [{ "operator": "is_null", "output": "null" }]
        }),
        json!({ "payload": null }),
    )
    .await
    .expect("null");
    assert!(null_subject.messages.contains_key("null"));
}

#[tokio::test]
async fn switch_node_reports_configuration_and_expression_errors() {
    let fixture = Fixture::new();
    let node = SwitchNode;

    let error = execute(
        &node,
        &fixture,
        json!({ "rules": "not an array" }),
        json!({}),
    )
    .await
    .expect_err("invalid config");
    assert_eq!(error.code, "node.config_invalid");

    let error = execute(
        &node,
        &fixture,
        json!({ "property": "1 not a path", "rules": [] }),
        json!({}),
    )
    .await
    .expect_err("invalid path");
    assert_eq!(error.code, "expression.parse_error");
}

#[tokio::test]
async fn data_load_range_rejects_bad_identifiers_and_dates() {
    let fixture = Fixture::new();
    let node = DataLoadRangeNode;

    let error = execute(
        &node,
        &fixture,
        json!({ "identifiers": "AAPL", "from": "2026-01-01", "to": "2026-01-02" }),
        json!({}),
    )
    .await
    .expect_err("identifiers not an array");
    assert_eq!(error.code, "node.config_invalid");

    let error = execute(
        &node,
        &fixture,
        json!({
            "identifiers": [{ "value": "AAPL" }],
            "from": "2026-01-01",
            "to": "2026-01-02"
        }),
        json!({}),
    )
    .await
    .expect_err("missing scheme");
    assert_eq!(error.code, "node.config_invalid");

    let error = execute(
        &node,
        &fixture,
        json!({
            "identifiers": [{ "scheme": "ticker" }, "AAPL"],
            "from": "2026-01-01",
            "to": "2026-01-02"
        }),
        json!({}),
    )
    .await
    .expect_err("missing value");
    assert_eq!(error.code, "node.config_invalid");

    let error = execute(
        &node,
        &fixture,
        json!({
            "identifiers": [{ "scheme": "ticker", "value": "AAPL" }],
            "from": 20260101,
            "to": "2026-01-02"
        }),
        json!({}),
    )
    .await
    .expect_err("from not a string");
    assert_eq!(error.code, "node.config_invalid");

    let error = execute(
        &node,
        &fixture,
        json!({
            "identifiers": [{ "scheme": "ticker", "value": "AAPL" }],
            "from": "not-a-date",
            "to": "2026-01-02"
        }),
        json!({}),
    )
    .await
    .expect_err("invalid from");
    assert_eq!(error.code, "node.config_invalid");

    let error = execute(
        &node,
        &fixture,
        json!({
            "identifiers": [{ "scheme": "ticker", "value": "AAPL" }],
            "from": "2026-01-01",
            "to": "not-a-date"
        }),
        json!({}),
    )
    .await
    .expect_err("invalid to");
    assert_eq!(error.code, "node.config_invalid");

    let error = execute(&node, &fixture, json!({ "concurrency": "many" }), json!({}))
        .await
        .expect_err("invalid config");
    assert_eq!(error.code, "node.config_invalid");
}

#[tokio::test]
async fn data_load_range_reports_validation_and_dry_run_success() {
    let fixture = Fixture::new();
    let node = DataLoadRangeNode;

    let error = execute(
        &node,
        &fixture,
        json!({
            "identifiers": [],
            "from": "2026-01-01",
            "to": "2026-01-02"
        }),
        json!({}),
    )
    .await
    .expect_err("empty identifiers");
    assert_eq!(error.code, "node.config_invalid");

    let output = execute(
        &node,
        &fixture,
        json!({
            "identifiers": [{ "scheme": "ticker", "value": "AAPL" }],
            "from": "2026-01-01",
            "to": "2026-01-02",
            "dry_run": true,
            "concurrency": 0
        }),
        json!({ "payload": { "start": true } }),
    )
    .await
    .expect("dry run report");
    let message = &output.messages["out"][0];
    assert!(message["payload"]["report"].is_object());
    assert!(message["payload"]["totals"].is_object());
    assert!(message["payload"]["status"].is_string());
}

fn flow_node(id: &str, node_type: &str) -> FlowNode {
    FlowNode {
        id: id.to_string(),
        node_type: node_type.to_string(),
        type_version: 1,
        label: String::new(),
        position: NodePosition::default(),
        config: json!({}),
        retry: RetryPolicy::default(),
        timeout_seconds: 300,
        error_policy: "fail".to_string(),
    }
}

fn subflow(
    inputs: Vec<SubflowInput>,
    outputs: Vec<SubflowOutput>,
    nodes: Vec<FlowNode>,
) -> Subflow {
    Subflow {
        id: "sub".to_string(),
        name: "Sub".to_string(),
        inputs,
        outputs,
        nodes,
        wires: Vec::<Wire>::new(),
    }
}

#[test]
fn subflow_helpers_and_graph_errors() {
    assert!(subflow::is_structural("subflow_input"));
    assert!(subflow::is_structural("subflow_output"));
    assert!(!subflow::is_structural("subflow"));
    assert_eq!(subflow::instance_scope("flow", "instance"), "flow:instance");
    assert_eq!(
        subflow::qualified_node_id("instance", "node"),
        "instance/node"
    );

    let missing_input = subflow(Vec::new(), Vec::new(), Vec::new());
    let error = subflow::graph(&missing_input).expect_err("no input");
    assert_eq!(error.code, "flow.subflow_io");

    let duplicate_input = subflow(
        Vec::new(),
        Vec::new(),
        vec![
            flow_node("in-a", "subflow_input"),
            flow_node("in-b", "subflow_input"),
        ],
    );
    let error = subflow::graph(&duplicate_input).expect_err("two inputs");
    assert_eq!(error.code, "flow.subflow_io");

    let mismatched_declaration = subflow(
        vec![SubflowInput {
            id: "other".to_string(),
            label: String::new(),
        }],
        Vec::new(),
        vec![flow_node("in", "subflow_input")],
    );
    let error = subflow::graph(&mismatched_declaration).expect_err("input mismatch");
    assert_eq!(error.code, "flow.subflow_io");

    let missing_output = subflow(
        vec![SubflowInput {
            id: "in".to_string(),
            label: String::new(),
        }],
        Vec::new(),
        vec![flow_node("in", "subflow_input")],
    );
    let error = subflow::graph(&missing_output).expect_err("no output");
    assert_eq!(error.code, "flow.subflow_io");

    let unknown_output = subflow(
        vec![SubflowInput {
            id: "in".to_string(),
            label: String::new(),
        }],
        vec![SubflowOutput {
            id: "ghost".to_string(),
            label: String::new(),
        }],
        vec![
            flow_node("in", "subflow_input"),
            flow_node("out", "subflow_output"),
        ],
    );
    let error = subflow::graph(&unknown_output).expect_err("unknown output");
    assert_eq!(error.code, "flow.subflow_io");

    let count_mismatch = subflow(
        vec![SubflowInput {
            id: "in".to_string(),
            label: String::new(),
        }],
        vec![SubflowOutput {
            id: "out-a".to_string(),
            label: String::new(),
        }],
        vec![
            flow_node("in", "subflow_input"),
            flow_node("out-a", "subflow_output"),
            flow_node("out-b", "subflow_output"),
        ],
    );
    let error = subflow::graph(&count_mismatch).expect_err("output count");
    assert_eq!(error.code, "flow.subflow_io");

    let valid = subflow(
        vec![SubflowInput {
            id: "in".to_string(),
            label: String::new(),
        }],
        vec![SubflowOutput {
            id: "out".to_string(),
            label: String::new(),
        }],
        vec![
            flow_node("in", "subflow_input"),
            flow_node("out", "subflow_output"),
        ],
    );
    let graph = subflow::graph(&valid).expect("valid graph");
    assert_eq!(graph.input.id, "in");
    assert_eq!(graph.outputs.len(), 1);
    assert_eq!(graph.subflow.id, "sub");
    assert_eq!(
        subflow::enter(&valid, json!({ "payload": 1 })).expect("enter"),
        json!({ "payload": 1 })
    );
    assert_eq!(
        subflow::exit(&valid, vec![json!(1), json!(2)]).expect("exit"),
        vec![json!(1), json!(2)]
    );
}
