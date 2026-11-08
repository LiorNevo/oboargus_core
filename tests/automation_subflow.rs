//! Subflow parsing, validation, expansion, and scoping tests (US4).

use std::sync::Mutex;

use chrono::Utc;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use obolargus_core::automation::error::NodeExecutionError;
use obolargus_core::automation::flow::document::{FlowDocument, FlowNode, Subflow};
use obolargus_core::automation::flow::validate::validate_flow;
use obolargus_core::automation::model::RunStatus;
use obolargus_core::automation::nodes::{NodeOutput, default_registry};
use obolargus_core::automation::runtime::bus::RunMetadata;
use obolargus_core::automation::runtime::executor::{
    ExecutionObserver, NoopObserver, RunExecutionConfig, execute_flow,
};
use obolargus_core::automation::runtime::subflow;
use obolargus_core::market_data::registry::DataSourceRegistry;

fn fixture(name: &str) -> FlowDocument {
    let path = format!("tests/fixtures/automation/{name}.json");
    let text = std::fs::read_to_string(&path).expect("fixture should exist");
    serde_json::from_str(&text).expect("fixture should parse")
}

fn metadata() -> RunMetadata {
    RunMetadata {
        id: Uuid::new_v4(),
        flow_id: Uuid::new_v4(),
        flow_version_id: Uuid::new_v4(),
        trigger_kind: "manual".to_string(),
        trigger_node_id: None,
        started_at: Utc::now(),
        actor_id: Some("tester".to_string()),
        inputs: json!({}),
    }
}

fn node(value: Value) -> FlowNode {
    serde_json::from_value(value).expect("node")
}

fn subflow_definition(id: &str, internal_type: &str, config: Value) -> Subflow {
    serde_json::from_value(json!({
        "id": id,
        "name": id,
        "inputs": [{ "id": "in", "label": "Input" }],
        "outputs": [{ "id": "out", "label": "Output" }],
        "nodes": [
            { "id": "in", "type": "subflow_input", "type_version": 1 },
            {
                "id": "s1",
                "type": internal_type,
                "type_version": 1,
                "config": config
            },
            { "id": "out", "type": "subflow_output", "type_version": 1 }
        ],
        "wires": [
            { "id": "sw1", "source": "in", "source_port": "out", "target": "s1", "target_port": "in" },
            { "id": "sw2", "source": "s1", "source_port": "out", "target": "out", "target_port": "in" }
        ]
    }))
    .expect("subflow")
}

fn instance_flow(subflows: Vec<Subflow>, instances: &[&str]) -> FlowDocument {
    let mut nodes = vec![node(json!({
        "id": "n1",
        "type": "inject",
        "type_version": 1,
        "config": { "repeat": "none", "once": true }
    }))];
    let mut wires = Vec::new();
    for (index, instance) in instances.iter().enumerate() {
        nodes.push(node(json!({
            "id": instance,
            "type": "subflow",
            "type_version": 1,
            "config": { "subflow_id": "sf1" }
        })));
        wires.push(json!({
            "id": format!("w{index}"),
            "source": "n1",
            "source_port": "out",
            "target": instance,
            "target_port": "in"
        }));
    }
    serde_json::from_value(json!({
        "schema_version": 1,
        "name": "subflow instances",
        "nodes": nodes,
        "wires": wires,
        "subflows": subflows
    }))
    .expect("flow")
}

#[derive(Default)]
struct Collector {
    started: Mutex<Vec<String>>,
    finished: Mutex<Vec<String>>,
    emitted: Mutex<Vec<(String, String, Value)>>,
    context: Mutex<Vec<(String, String, String, Value)>>,
}

#[async_trait::async_trait]
impl ExecutionObserver for Collector {
    async fn node_started(&self, node_id: &str, _attempt: u32, _message: &Value) {
        if let Ok(mut guard) = self.started.lock() {
            guard.push(node_id.to_string());
        }
    }

    async fn node_finished(
        &self,
        node_id: &str,
        _attempt: u32,
        _output: Option<&NodeOutput>,
        _error: Option<&NodeExecutionError>,
    ) {
        if let Ok(mut guard) = self.finished.lock() {
            guard.push(node_id.to_string());
        }
    }

    async fn message(&self, node_id: &str, port: &str, message: &Value) {
        if let Ok(mut guard) = self.emitted.lock() {
            guard.push((node_id.to_string(), port.to_string(), message.clone()));
        }
    }

    async fn context_changed(&self, scope: &str, scope_id: &str, key: &str, value: &Value) {
        if let Ok(mut guard) = self.context.lock() {
            guard.push((
                scope.to_string(),
                scope_id.to_string(),
                key.to_string(),
                value.clone(),
            ));
        }
    }
}

async fn run(
    flow: &FlowDocument,
    observer: &dyn ExecutionObserver,
) -> obolargus_core::automation::runtime::executor::RunOutcome {
    let registry = default_registry();
    let market_data = DataSourceRegistry::new(Vec::new()).expect("registry");
    execute_flow(
        flow,
        &registry,
        metadata(),
        None,
        &market_data,
        CancellationToken::new(),
        RunExecutionConfig::default(),
        observer,
    )
    .await
}

#[test]
fn subflow_fixture_parses_and_validates() {
    let flow = fixture("subflow");
    assert_eq!(flow.subflows.len(), 1);
    let report = validate_flow(&flow, &default_registry());
    assert!(report.is_valid(), "{:?}", report.errors);
}

#[test]
fn structural_nodes_resolve_input_and_outputs() {
    let flow = fixture("subflow");
    let graph = subflow::graph(&flow.subflows[0]).expect("graph");
    assert_eq!(graph.input.id, "in");
    assert_eq!(graph.outputs.len(), 1);
    assert_eq!(graph.outputs[0].id, "out");
}

#[test]
fn missing_structural_input_is_rejected() {
    let mut definition = subflow_definition("sf1", "change", json!({ "rules": [] }));
    definition
        .nodes
        .retain(|node| node.node_type != "subflow_input");
    let flow = instance_flow(vec![definition], &["a"]);
    let report = validate_flow(&flow, &default_registry());
    assert!(
        report
            .errors
            .iter()
            .any(|finding| finding.code == "flow.subflow_io")
    );
}

#[test]
fn output_declaration_must_reference_a_structural_output() {
    let mut definition = subflow_definition("sf1", "change", json!({ "rules": [] }));
    definition.outputs[0].id = "not-a-node".to_string();
    let flow = instance_flow(vec![definition], &["a"]);
    let report = validate_flow(&flow, &default_registry());
    assert!(
        report
            .errors
            .iter()
            .any(|finding| finding.code == "flow.subflow_io")
    );
}

#[test]
fn direct_recursion_is_rejected() {
    let mut definition = subflow_definition("sf1", "change", json!({ "rules": [] }));
    definition.nodes.push(node(json!({
        "id": "recursive",
        "type": "subflow",
        "type_version": 1,
        "config": { "subflow_id": "sf1" }
    })));
    let flow = instance_flow(vec![definition], &["a"]);
    let report = validate_flow(&flow, &default_registry());
    assert!(
        report
            .errors
            .iter()
            .any(|finding| finding.code == "flow.subflow_recursive")
    );
}

#[test]
fn indirect_recursion_is_rejected() {
    let mut first = subflow_definition("sf1", "change", json!({ "rules": [] }));
    let second = subflow_definition("sf2", "change", json!({ "rules": [] }));
    first.nodes.push(node(json!({
        "id": "to-second",
        "type": "subflow",
        "type_version": 1,
        "config": { "subflow_id": "sf2" }
    })));
    let mut second = second;
    second.nodes.push(node(json!({
        "id": "to-first",
        "type": "subflow",
        "type_version": 1,
        "config": { "subflow_id": "sf1" }
    })));
    let flow = instance_flow(vec![first, second], &["a"]);
    let report = validate_flow(&flow, &default_registry());
    assert!(
        report
            .errors
            .iter()
            .any(|finding| finding.code == "flow.subflow_recursive")
    );
}

#[tokio::test]
async fn message_flows_through_a_subflow_to_its_output_wire() {
    let flow = fixture("subflow");
    let collector = Collector::default();
    let outcome = run(&flow, &collector).await;
    assert_eq!(outcome.status, RunStatus::Succeeded, "{:?}", outcome.error);
    let started = collector.started.lock().expect("started");
    assert!(started.iter().any(|node| node == "n2/s1"));
    assert!(started.iter().any(|node| node == "n2/in"));
    assert!(started.iter().any(|node| node == "n2/out"));
    assert!(started.iter().any(|node| node == "n3"));
    let emitted = collector.emitted.lock().expect("emitted");
    assert!(
        emitted
            .iter()
            .any(|(node, port, _)| node == "n2" && port == "out")
    );
}

#[tokio::test]
async fn two_instances_share_the_same_definition() {
    let flow = instance_flow(
        vec![subflow_definition("sf1", "change", json!({ "rules": [] }))],
        &["a", "b"],
    );
    let collector = Collector::default();
    let outcome = run(&flow, &collector).await;
    assert_eq!(outcome.status, RunStatus::Succeeded, "{:?}", outcome.error);
    let started = collector.started.lock().expect("started");
    assert!(started.iter().any(|node| node == "a/s1"));
    assert!(started.iter().any(|node| node == "b/s1"));
}

#[tokio::test]
async fn subflow_context_uses_an_instance_scope() {
    let flow = instance_flow(
        vec![subflow_definition(
            "sf1",
            "change",
            json!({
                "rules": [{ "action": "set", "target": "flow", "key": "seen", "value": true }]
            }),
        )],
        &["a"],
    );
    let collector = Collector::default();
    let outcome = run(&flow, &collector).await;
    assert_eq!(outcome.status, RunStatus::Succeeded, "{:?}", outcome.error);
    let writes = collector.context.lock().expect("context");
    assert!(writes.iter().any(|(scope, scope_id, key, value)| {
        scope == "flow" && scope_id.ends_with(":a") && key == "seen" && value == &json!(true)
    }));
}

#[tokio::test]
async fn subflow_depth_limit_is_enforced() {
    let inner = subflow_definition("sf2", "change", json!({ "rules": [] }));
    let mut outer = subflow_definition("sf1", "change", json!({ "rules": [] }));
    outer
        .nodes
        .retain(|node| node.id != "s1" && node.id != "out");
    outer.wires.clear();
    outer.nodes.push(node(json!({
        "id": "nested",
        "type": "subflow",
        "type_version": 1,
        "config": { "subflow_id": "sf2" }
    })));
    outer.nodes.push(node(json!({
        "id": "out",
        "type": "subflow_output",
        "type_version": 1
    })));
    outer.wires.push(
        serde_json::from_value(json!({
            "id": "ow1",
            "source": "in",
            "source_port": "out",
            "target": "nested",
            "target_port": "in"
        }))
        .expect("wire"),
    );
    outer.wires.push(
        serde_json::from_value(json!({
            "id": "ow2",
            "source": "nested",
            "source_port": "out",
            "target": "out",
            "target_port": "in"
        }))
        .expect("wire"),
    );
    let flow = instance_flow(vec![outer, inner], &["a"]);
    assert!(validate_flow(&flow, &default_registry()).is_valid());
    let registry = default_registry();
    let market_data = DataSourceRegistry::new(Vec::new()).expect("registry");
    let outcome = execute_flow(
        &flow,
        &registry,
        metadata(),
        None,
        &market_data,
        CancellationToken::new(),
        RunExecutionConfig {
            max_subflow_depth: 1,
            ..RunExecutionConfig::default()
        },
        &NoopObserver,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.error.expect("error")["code"], "run.subflow_depth");
}
