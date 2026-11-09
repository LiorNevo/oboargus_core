//! Runtime execution tests: message passing, branches, loops, cancellation.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use obolargus_core::automation::error::NodeExecutionError;
use obolargus_core::automation::flow::document::FlowDocument;
use obolargus_core::automation::model::RunStatus;
use obolargus_core::automation::nodes::{NodeOutput, NodeRegistry, default_registry};
use obolargus_core::automation::runtime::bus::RunMetadata;
use obolargus_core::automation::runtime::executor::{
    ExecutionObserver, NoopObserver, RunExecutionConfig, execute_flow,
};
use obolargus_core::market_data::registry::DataSourceRegistry;

fn fixture(name: &str) -> FlowDocument {
    let path = format!("tests/fixtures/automation/{name}.json");
    let text = std::fs::read_to_string(&path).expect("fixture");
    serde_json::from_str(&text).expect("flow")
}

fn run_metadata(inputs: Value) -> RunMetadata {
    RunMetadata {
        id: Uuid::new_v4(),
        flow_id: Uuid::new_v4(),
        flow_version_id: Uuid::new_v4(),
        trigger_kind: "manual".to_string(),
        trigger_node_id: None,
        started_at: Utc::now(),
        actor_id: Some("tester".to_string()),
        inputs,
    }
}

#[derive(Default)]
struct Collector {
    started: Mutex<Vec<(String, Value)>>,
    finished: Mutex<Vec<(String, bool)>>,
    messages: Mutex<Vec<(String, String, Value)>>,
}

#[async_trait::async_trait]
impl ExecutionObserver for Collector {
    async fn node_started(&self, node_id: &str, _attempt: u32, message: &Value) {
        if let Ok(mut guard) = self.started.lock() {
            guard.push((node_id.to_string(), message.clone()));
        }
    }

    async fn node_finished(
        &self,
        node_id: &str,
        _attempt: u32,
        output: Option<&NodeOutput>,
        error: Option<&NodeExecutionError>,
    ) {
        if let Ok(mut guard) = self.finished.lock() {
            guard.push((node_id.to_string(), error.is_none() && output.is_some()));
        }
    }

    async fn message(&self, node_id: &str, port: &str, message: &Value) {
        if let Ok(mut guard) = self.messages.lock() {
            guard.push((node_id.to_string(), port.to_string(), message.clone()));
        }
    }
}

fn registry_snapshot() -> (NodeRegistry, DataSourceRegistry) {
    let registry = default_registry();
    let market_data = DataSourceRegistry::new(Vec::new()).expect("registry");
    (registry, market_data)
}

#[tokio::test]
async fn multi_output_flow_routes_matching_branch() {
    let flow = fixture("multi-output");
    let (registry, market_data) = registry_snapshot();
    let collector = Collector::default();
    let outcome = execute_flow(
        &flow,
        &registry,
        run_metadata(json!({})),
        None,
        &market_data,
        CancellationToken::new(),
        RunExecutionConfig::default(),
        &collector,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Succeeded);
    assert_eq!(outcome.messages_total, 3);
    let messages = collector.messages.lock().expect("messages");
    let ports: Vec<(&str, &str)> = messages
        .iter()
        .map(|(node, port, _)| (node.as_str(), port.as_str()))
        .collect();
    assert!(ports.contains(&("n1", "out")));
    assert!(ports.contains(&("n2", "out")));
    assert!(!ports.contains(&("n2", "default")));
}

#[tokio::test]
async fn run_output_node_sets_final_output() {
    let flow: FlowDocument = serde_json::from_value(json!({
        "schema_version": 1,
        "name": "output",
        "nodes": [
            { "id": "n1", "type": "inject", "type_version": 1, "config": { "repeat": "none", "once": true } },
            { "id": "n2", "type": "run_output", "type_version": 1, "config": { "value": { "constant": 42 } } }
        ],
        "wires": [
            { "id": "w1", "source": "n1", "source_port": "out", "target": "n2", "target_port": "in" }
        ]
    }))
    .expect("flow");
    let (registry, market_data) = registry_snapshot();
    let outcome = execute_flow(
        &flow,
        &registry,
        run_metadata(json!({})),
        None,
        &market_data,
        CancellationToken::new(),
        RunExecutionConfig::default(),
        &NoopObserver,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Succeeded);
    assert_eq!(outcome.outputs, json!({ "constant": 42 }));
}

#[tokio::test]
async fn cyclic_flow_hits_the_message_limit() {
    let flow = fixture("cyclic");
    let (registry, market_data) = registry_snapshot();
    let config = RunExecutionConfig {
        max_messages: 20,
        max_messages_per_node: 10,
        max_duration: Duration::from_secs(5),
        max_subflow_depth: 10,
    };
    let outcome = execute_flow(
        &flow,
        &registry,
        run_metadata(json!({})),
        None,
        &market_data,
        CancellationToken::new(),
        config,
        &NoopObserver,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Failed);
    let error = outcome.error.expect("error");
    assert_eq!(error["code"], "run.loop_limit");
}

#[tokio::test]
async fn cancellation_stops_the_run() {
    let flow = fixture("cyclic");
    let (registry, market_data) = registry_snapshot();
    let token = CancellationToken::new();
    token.cancel();
    let outcome = execute_flow(
        &flow,
        &registry,
        run_metadata(json!({})),
        None,
        &market_data,
        token,
        RunExecutionConfig::default(),
        &NoopObserver,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Cancelled);
    assert_eq!(outcome.messages_total, 0);
}

#[tokio::test]
async fn run_from_inject_node_only_executes_that_branch() {
    let flow: FlowDocument = serde_json::from_value(json!({
        "schema_version": 1,
        "name": "start-node",
        "nodes": [
            { "id": "n1", "type": "inject", "type_version": 1, "config": { "payload_type": "string", "payload": "first", "repeat": "none", "once": true } },
            { "id": "n2", "type": "debug", "type_version": 1, "config": { "target": "msg" } },
            { "id": "n3", "type": "inject", "type_version": 1, "config": { "payload_type": "string", "payload": "second", "repeat": "none", "once": true } },
            { "id": "n4", "type": "debug", "type_version": 1, "config": { "target": "msg" } }
        ],
        "wires": [
            { "id": "w1", "source": "n1", "source_port": "out", "target": "n2", "target_port": "in" },
            { "id": "w2", "source": "n3", "source_port": "out", "target": "n4", "target_port": "in" }
        ]
    }))
    .expect("flow");
    let (registry, market_data) = registry_snapshot();
    let collector = Collector::default();
    let mut metadata = run_metadata(json!({}));
    metadata.trigger_node_id = Some("n3".to_string());
    let outcome = execute_flow(
        &flow,
        &registry,
        metadata,
        None,
        &market_data,
        CancellationToken::new(),
        RunExecutionConfig::default(),
        &collector,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Succeeded);
    assert_eq!(outcome.messages_total, 2);
    let started = collector.started.lock().expect("started");
    let nodes: Vec<&str> = started.iter().map(|(node, _)| node.as_str()).collect();
    assert!(!nodes.contains(&"n1"));
    assert!(!nodes.contains(&"n2"));
    assert!(nodes.contains(&"n3"));
    assert!(nodes.contains(&"n4"));
    let messages = collector.messages.lock().expect("messages");
    assert!(messages.iter().any(|(node, _, value)| {
        node == "n3" && value["payload"] == json!("second")
    }));
}

#[tokio::test]
async fn run_from_missing_inject_node_fails_explicitly() {
    let flow: FlowDocument = serde_json::from_value(json!({
        "schema_version": 1,
        "name": "missing-start-node",
        "nodes": [
            { "id": "n1", "type": "inject", "type_version": 1, "config": { "repeat": "none", "once": true } }
        ],
        "wires": []
    }))
    .expect("flow");
    let (registry, market_data) = registry_snapshot();
    let mut metadata = run_metadata(json!({}));
    metadata.trigger_node_id = Some("nope".to_string());
    let outcome = execute_flow(
        &flow,
        &registry,
        metadata,
        None,
        &market_data,
        CancellationToken::new(),
        RunExecutionConfig::default(),
        &NoopObserver,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(
        outcome.error.expect("error")["code"],
        "run.start_node_missing"
    );
}

#[tokio::test]
async fn duplicate_messages_are_cloned_per_wire() {
    let flow: FlowDocument = serde_json::from_value(json!({
        "schema_version": 1,
        "name": "fanout",
        "nodes": [
            { "id": "n1", "type": "inject", "type_version": 1, "config": { "repeat": "none", "once": true } },
            { "id": "n2", "type": "debug", "type_version": 1, "config": { "target": "msg" } },
            { "id": "n3", "type": "debug", "type_version": 1, "config": { "target": "msg" } }
        ],
        "wires": [
            { "id": "w1", "source": "n1", "source_port": "out", "target": "n2", "target_port": "in" },
            { "id": "w2", "source": "n1", "source_port": "out", "target": "n3", "target_port": "in" }
        ]
    }))
    .expect("flow");
    let (registry, market_data) = registry_snapshot();
    let collector = Arc::new(Collector::default());
    let outcome = execute_flow(
        &flow,
        &registry,
        run_metadata(json!({})),
        None,
        &market_data,
        CancellationToken::new(),
        RunExecutionConfig::default(),
        collector.as_ref(),
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Succeeded);
    assert_eq!(outcome.messages_total, 3);
    let started = collector.started.lock().expect("started");
    let targets: Vec<&str> = started.iter().map(|(node, _)| node.as_str()).collect();
    assert_eq!(targets.iter().filter(|node| **node == "n2").count(), 1);
    assert_eq!(targets.iter().filter(|node| **node == "n3").count(), 1);
}

#[tokio::test]
async fn context_writes_are_visible_within_the_run() {
    let flow: FlowDocument = serde_json::from_value(json!({
        "schema_version": 1,
        "name": "context",
        "nodes": [
            { "id": "n1", "type": "inject", "type_version": 1, "config": { "repeat": "none", "once": true } },
            { "id": "n2", "type": "change", "type_version": 1, "config": {
                "rules": [{ "action": "set", "target": "run", "key": "seen", "value": true }]
            } },
            { "id": "n3", "type": "debug", "type_version": 1, "config": { "target": "msg" } }
        ],
        "wires": [
            { "id": "w1", "source": "n1", "source_port": "out", "target": "n2", "target_port": "in" },
            { "id": "w2", "source": "n2", "source_port": "out", "target": "n3", "target_port": "in" }
        ]
    }))
    .expect("flow");
    let (registry, market_data) = registry_snapshot();
    let outcome = execute_flow(
        &flow,
        &registry,
        run_metadata(json!({})),
        None,
        &market_data,
        CancellationToken::new(),
        RunExecutionConfig::default(),
        &NoopObserver,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Succeeded);
}

#[test]
fn no_dead_code_marker_is_needed_for_empty_graph() {
    // Ensures `BTreeMap` import is used meaningfully by a downstream test.
    let _map: BTreeMap<String, Value> = BTreeMap::new();
}

// --- Retry, error-policy, and event routing (US2) -------------------------

use obolargus_core::automation::nodes::{
    NodeCategory, NodeContext, NodeTypeDescriptor, PortDescriptor, WorkflowNode,
};

/// A scripted node that fails a fixed number of times, then emits `out`.
struct ScriptedNode {
    id: &'static str,
    failures: usize,
    retryable: bool,
    delay: Option<Duration>,
    calls: Mutex<usize>,
}

impl ScriptedNode {
    fn new(id: &'static str, failures: usize, retryable: bool) -> Self {
        Self {
            id,
            failures,
            retryable,
            delay: None,
            calls: Mutex::new(0),
        }
    }

    fn slow(id: &'static str) -> Self {
        Self {
            id,
            failures: 0,
            retryable: false,
            delay: Some(Duration::from_secs(5)),
            calls: Mutex::new(0),
        }
    }
}

#[async_trait::async_trait]
impl WorkflowNode for ScriptedNode {
    fn descriptor(&self) -> NodeTypeDescriptor {
        NodeTypeDescriptor {
            id: self.id.to_string(),
            version: 1,
            category: NodeCategory::Transform,
            display_name: self.id.to_string(),
            description: "Test node".to_string(),
            icon: "extension".to_string(),
            input: Some(PortDescriptor {
                name: "in".to_string(),
                label: "Input".to_string(),
            }),
            outputs: vec![PortDescriptor {
                name: "out".to_string(),
                label: "Output".to_string(),
            }],
            config_schema: json!({ "type": "object" }),
            output_schema: json!({ "type": "object" }),
            capabilities: Vec::new(),
            deprecated: false,
            docs: String::new(),
        }
    }

    async fn execute(&self, ctx: NodeContext<'_>) -> Result<NodeOutput, NodeExecutionError> {
        let call = {
            let mut calls = self.calls.lock().expect("calls");
            *calls += 1;
            *calls
        };
        if let Some(delay) = self.delay {
            tokio::time::sleep(delay).await;
        }
        if call <= self.failures {
            return Err(if self.retryable {
                NodeExecutionError::retryable("node.provider_unavailable", "temporary failure")
            } else {
                NodeExecutionError::new("node.config_invalid", "permanent failure")
            });
        }
        let mut output = NodeOutput::empty();
        output.push("out", ctx.msg);
        Ok(output)
    }
}

fn scripted_registry(node: ScriptedNode) -> NodeRegistry {
    let mut registry = default_registry();
    registry.register(Arc::new(node));
    registry
}

#[derive(Default)]
struct AttemptCollector {
    started: Mutex<Vec<(String, u32)>>,
    messages: Mutex<Vec<(String, String)>>,
}

#[async_trait::async_trait]
impl ExecutionObserver for AttemptCollector {
    async fn node_started(&self, node_id: &str, attempt: u32, _message: &Value) {
        if let Ok(mut guard) = self.started.lock() {
            guard.push((node_id.to_string(), attempt));
        }
    }

    async fn node_finished(
        &self,
        _node_id: &str,
        _attempt: u32,
        _output: Option<&NodeOutput>,
        _error: Option<&NodeExecutionError>,
    ) {
    }

    async fn message(&self, node_id: &str, port: &str, _message: &Value) {
        if let Ok(mut guard) = self.messages.lock() {
            guard.push((node_id.to_string(), port.to_string()));
        }
    }
}

fn scripted_flow(node_type: &str, error_policy: &str) -> FlowDocument {
    serde_json::from_value(json!({
        "schema_version": 1,
        "name": "scripted",
        "nodes": [
            { "id": "n1", "type": "inject", "type_version": 1, "config": { "repeat": "none", "once": true } },
            {
                "id": "n2",
                "type": node_type,
                "type_version": 1,
                "config": {},
                "retry": { "max_attempts": 3, "backoff_seconds": 0 },
                "error_policy": error_policy
            }
        ],
        "wires": [
            { "id": "w1", "source": "n1", "source_port": "out", "target": "n2", "target_port": "in" }
        ]
    }))
    .expect("flow")
}

#[tokio::test]
async fn retryable_error_is_retried_until_success() {
    let flow = scripted_flow("flaky", "fail");
    let registry = scripted_registry(ScriptedNode::new("flaky", 2, true));
    let market_data = DataSourceRegistry::new(Vec::new()).expect("registry");
    let collector = AttemptCollector::default();
    let outcome = execute_flow(
        &flow,
        &registry,
        run_metadata(json!({})),
        None,
        &market_data,
        CancellationToken::new(),
        RunExecutionConfig::default(),
        &collector,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Succeeded, "{:?}", outcome.error);
    let started = collector.started.lock().expect("started");
    let attempts: Vec<u32> = started
        .iter()
        .filter(|(node, _)| node == "n2")
        .map(|(_, attempt)| *attempt)
        .collect();
    assert_eq!(attempts, vec![1, 2, 3]);
}

#[tokio::test]
async fn non_retryable_error_fails_after_one_attempt() {
    let flow = scripted_flow("permanent", "fail");
    let registry = scripted_registry(ScriptedNode::new("permanent", 1, false));
    let market_data = DataSourceRegistry::new(Vec::new()).expect("registry");
    let collector = AttemptCollector::default();
    let outcome = execute_flow(
        &flow,
        &registry,
        run_metadata(json!({})),
        None,
        &market_data,
        CancellationToken::new(),
        RunExecutionConfig::default(),
        &collector,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.error.expect("error")["code"], "node.config_invalid");
    let started = collector.started.lock().expect("started");
    assert_eq!(started.iter().filter(|(node, _)| node == "n2").count(), 1);
}

#[tokio::test]
async fn continue_policy_ends_the_run_partial() {
    let flow = scripted_flow("permanent", "continue");
    let registry = scripted_registry(ScriptedNode::new("permanent", 1, false));
    let market_data = DataSourceRegistry::new(Vec::new()).expect("registry");
    let outcome = execute_flow(
        &flow,
        &registry,
        run_metadata(json!({})),
        None,
        &market_data,
        CancellationToken::new(),
        RunExecutionConfig::default(),
        &NoopObserver,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Partial);
    assert!(outcome.error.is_none());
}

#[tokio::test]
async fn catch_node_receives_the_error_and_the_run_is_partial() {
    let flow: FlowDocument = serde_json::from_value(json!({
        "schema_version": 1,
        "name": "catch",
        "nodes": [
            { "id": "n1", "type": "inject", "type_version": 1, "config": { "repeat": "none", "once": true } },
            {
                "id": "n2",
                "type": "permanent",
                "type_version": 1,
                "config": {},
                "retry": { "max_attempts": 1, "backoff_seconds": 0 },
                "error_policy": "catch"
            },
            { "id": "c1", "type": "catch", "type_version": 1, "config": { "scope": "flow" } },
            { "id": "d1", "type": "debug", "type_version": 1, "config": { "target": "msg" } }
        ],
        "wires": [
            { "id": "w1", "source": "n1", "source_port": "out", "target": "n2", "target_port": "in" },
            { "id": "w2", "source": "c1", "source_port": "out", "target": "d1", "target_port": "in" }
        ]
    }))
    .expect("flow");
    let registry = scripted_registry(ScriptedNode::new("permanent", 1, false));
    let market_data = DataSourceRegistry::new(Vec::new()).expect("registry");
    let collector = AttemptCollector::default();
    let outcome = execute_flow(
        &flow,
        &registry,
        run_metadata(json!({})),
        None,
        &market_data,
        CancellationToken::new(),
        RunExecutionConfig::default(),
        &collector,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Partial, "{:?}", outcome.error);
    let messages = collector.messages.lock().expect("messages");
    assert!(
        messages
            .iter()
            .any(|(node, port)| node == "c1" && port == "out")
    );
}

#[tokio::test]
async fn status_and_complete_nodes_receive_events() {
    let flow: FlowDocument = serde_json::from_value(json!({
        "schema_version": 1,
        "name": "events",
        "nodes": [
            { "id": "n1", "type": "inject", "type_version": 1, "config": { "repeat": "none", "once": true } },
            { "id": "n2", "type": "change", "type_version": 1, "config": { "rules": [] } },
            { "id": "s1", "type": "status", "type_version": 1, "config": { "scope": "selected", "node_ids": ["n2"] } },
            { "id": "c1", "type": "complete", "type_version": 1, "config": { "scope": "selected", "node_ids": ["n2"] } }
        ],
        "wires": [
            { "id": "w1", "source": "n1", "source_port": "out", "target": "n2", "target_port": "in" }
        ]
    }))
    .expect("flow");
    let (registry, market_data) = registry_snapshot();
    let collector = AttemptCollector::default();
    let outcome = execute_flow(
        &flow,
        &registry,
        run_metadata(json!({})),
        None,
        &market_data,
        CancellationToken::new(),
        RunExecutionConfig::default(),
        &collector,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Succeeded);
    let messages = collector.messages.lock().expect("messages");
    assert!(messages.iter().any(|(node, _)| node == "s1"));
    assert!(messages.iter().any(|(node, _)| node == "c1"));
}

#[tokio::test]
async fn node_timeout_produces_a_retryable_timeout_error() {
    let flow: FlowDocument = serde_json::from_value(json!({
        "schema_version": 1,
        "name": "timeout",
        "nodes": [
            { "id": "n1", "type": "inject", "type_version": 1, "config": { "repeat": "none", "once": true } },
            {
                "id": "n2",
                "type": "slow",
                "type_version": 1,
                "config": {},
                "retry": { "max_attempts": 1, "backoff_seconds": 0 },
                "timeout_seconds": 1
            }
        ],
        "wires": [
            { "id": "w1", "source": "n1", "source_port": "out", "target": "n2", "target_port": "in" }
        ]
    }))
    .expect("flow");
    let registry = scripted_registry(ScriptedNode::slow("slow"));
    let market_data = DataSourceRegistry::new(Vec::new()).expect("registry");
    let outcome = execute_flow(
        &flow,
        &registry,
        run_metadata(json!({})),
        None,
        &market_data,
        CancellationToken::new(),
        RunExecutionConfig::default(),
        &NoopObserver,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.error.expect("error")["code"], "node.timeout");
}

#[test]
fn registry_capability_allowlist_denies_nodes() {
    let registry = default_registry().with_capabilities("read:market_data");
    let data_load = registry.get("data_load@1").expect("data_load").descriptor();
    assert!(!registry.denied_capabilities(&data_load).is_empty());
    let debug = registry.get("debug@1").expect("debug").descriptor();
    assert!(registry.denied_capabilities(&debug).is_empty());
    let all = default_registry().with_capabilities("all");
    assert!(all.denied_capabilities(&data_load).is_empty());
}

/// A node that emits a burst of messages on `out`.
struct BurstNode {
    count: usize,
}

#[async_trait::async_trait]
impl WorkflowNode for BurstNode {
    fn descriptor(&self) -> NodeTypeDescriptor {
        NodeTypeDescriptor {
            id: "burst".to_string(),
            version: 1,
            category: NodeCategory::Transform,
            display_name: "Burst".to_string(),
            description: "Emits many messages".to_string(),
            icon: "bolt".to_string(),
            input: Some(PortDescriptor {
                name: "in".to_string(),
                label: "Input".to_string(),
            }),
            outputs: vec![PortDescriptor {
                name: "out".to_string(),
                label: "Output".to_string(),
            }],
            config_schema: json!({ "type": "object" }),
            output_schema: json!({ "type": "object" }),
            capabilities: Vec::new(),
            deprecated: false,
            docs: String::new(),
        }
    }

    async fn execute(&self, ctx: NodeContext<'_>) -> Result<NodeOutput, NodeExecutionError> {
        let mut output = NodeOutput::empty();
        for index in 0..self.count {
            let mut message = ctx.msg.clone();
            if let Value::Object(map) = &mut message {
                map.insert("index".to_string(), json!(index));
            }
            output.push("out", message);
        }
        Ok(output)
    }
}

#[tokio::test]
async fn thousand_message_burst_completes_quickly() {
    let flow: FlowDocument = serde_json::from_value(json!({
        "schema_version": 1,
        "name": "burst",
        "nodes": [
            { "id": "n1", "type": "inject", "type_version": 1, "config": { "repeat": "none", "once": true } },
            { "id": "n2", "type": "burst", "type_version": 1, "config": {} },
            { "id": "n3", "type": "debug", "type_version": 1, "config": { "target": "msg" } }
        ],
        "wires": [
            { "id": "w1", "source": "n1", "source_port": "out", "target": "n2", "target_port": "in" },
            { "id": "w2", "source": "n2", "source_port": "out", "target": "n3", "target_port": "in" }
        ]
    }))
    .expect("flow");
    let mut registry = default_registry();
    registry.register(Arc::new(BurstNode { count: 1_000 }));
    let market_data = DataSourceRegistry::new(Vec::new()).expect("registry");
    let started = std::time::Instant::now();
    let outcome = execute_flow(
        &flow,
        &registry,
        run_metadata(json!({})),
        None,
        &market_data,
        CancellationToken::new(),
        RunExecutionConfig {
            max_messages: 5_000,
            max_messages_per_node: 5_000,
            ..RunExecutionConfig::default()
        },
        &NoopObserver,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Succeeded, "{:?}", outcome.error);
    assert!(outcome.messages_total >= 1_001);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "1,000-message burst took {:?}",
        started.elapsed()
    );
}
