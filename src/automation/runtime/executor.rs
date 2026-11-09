//! Flow execution.
//!
//! v1 uses a deterministic breadth-first delivery queue. Each node processes
//! one delivery at a time; independent branches are represented in the queue.
//! A future release can replace the queue with per-node channels without
//! changing node types or persistence.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::automation::error::NodeExecutionError;
use crate::automation::flow::document::{FlowDocument, FlowNode, Wire};
use crate::automation::model::RunStatus;
use crate::automation::nodes::{NodeContext, NodeOutput, NodeRegistry, NoopLogger, NoopSecrets};
use crate::automation::runtime::bus::{MessageEnvelope, RunMetadata};
use crate::automation::runtime::context::{ContextScope, ContextStore};
use crate::automation::runtime::policy;
use crate::automation::runtime::subflow;
use crate::market_data::registry::DataSourceRegistry;

/// Optional execution observer used by the server to persist progress.
///
/// Observer failures are the implementer's responsibility; the runtime never
/// aborts a run because telemetry could not be written.
#[async_trait::async_trait]
pub trait ExecutionObserver: Send + Sync {
    /// Called before a node processes one delivery attempt.
    async fn node_started(&self, node_id: &str, attempt: u32, message: &Value);
    /// Called after one delivery attempt finishes.
    async fn node_finished(
        &self,
        node_id: &str,
        attempt: u32,
        output: Option<&NodeOutput>,
        error: Option<&NodeExecutionError>,
    );
    /// Called for every message emitted on an output port.
    async fn message(&self, node_id: &str, port: &str, message: &Value);
    /// Called for every durable context key written during a run.
    async fn context_changed(&self, _scope: &str, _scope_id: &str, _key: &str, _value: &Value) {}
}

/// Observer that discards all events.
pub struct NoopObserver;

#[async_trait::async_trait]
impl ExecutionObserver for NoopObserver {
    async fn node_started(&self, _node_id: &str, _attempt: u32, _message: &Value) {}
    async fn node_finished(
        &self,
        _node_id: &str,
        _attempt: u32,
        _output: Option<&NodeOutput>,
        _error: Option<&NodeExecutionError>,
    ) {
    }
    async fn message(&self, _node_id: &str, _port: &str, _message: &Value) {}
}

/// Loop-protection, subflow-depth, and quiescence limits.
#[derive(Debug, Clone, Copy)]
pub struct RunExecutionConfig {
    /// Maximum messages processed.
    pub max_messages: u64,
    /// Maximum messages for one node.
    pub max_messages_per_node: u64,
    /// Maximum wall-clock duration.
    pub max_duration: Duration,
    /// Maximum nested subflow depth.
    pub max_subflow_depth: u32,
}

impl Default for RunExecutionConfig {
    fn default() -> Self {
        Self {
            max_messages: 10_000,
            max_messages_per_node: 1_000,
            max_duration: Duration::from_secs(3_600),
            max_subflow_depth: subflow::DEFAULT_MAX_DEPTH,
        }
    }
}

/// The outcome of executing a flow.
#[derive(Debug, Clone)]
pub struct RunOutcome {
    /// Terminal status.
    pub status: RunStatus,
    /// Final run outputs.
    pub outputs: Value,
    /// Total messages processed.
    pub messages_total: u64,
    /// Terminal error, if any.
    pub error: Option<Value>,
}

/// Executes a published flow once with an empty context store.
#[allow(clippy::too_many_arguments)]
pub async fn execute_flow(
    flow: &FlowDocument,
    registry: &NodeRegistry,
    run: RunMetadata,
    pool: Option<&sqlx::PgPool>,
    market_data: &DataSourceRegistry,
    cancel: CancellationToken,
    config: RunExecutionConfig,
    observer: &dyn ExecutionObserver,
) -> RunOutcome {
    let context = ContextStore::new(
        run.flow_id.to_string(),
        "workspace".to_string(),
        BTreeMap::new(),
        BTreeMap::new(),
    );
    execute_flow_with_context(
        flow,
        registry,
        run,
        pool,
        market_data,
        cancel,
        config,
        observer,
        context,
    )
    .await
}

/// Executes a published flow once using a preloaded context store.
#[allow(clippy::too_many_arguments)]
pub async fn execute_flow_with_context(
    flow: &FlowDocument,
    registry: &NodeRegistry,
    run: RunMetadata,
    pool: Option<&sqlx::PgPool>,
    market_data: &DataSourceRegistry,
    cancel: CancellationToken,
    config: RunExecutionConfig,
    observer: &dyn ExecutionObserver,
    context: ContextStore,
) -> RunOutcome {
    let mut execution = Execution {
        flow,
        registry,
        pool,
        market_data,
        cancel,
        config,
        observer,
        run,
        started: Instant::now(),
        messages_total: 0,
        per_node: HashMap::new(),
        outputs: json!({}),
        partial: false,
    };
    let outcome = execution.run(&context).await;
    execution.drain_durable_context(&context, false).await;
    outcome
}

struct Execution<'a> {
    flow: &'a FlowDocument,
    registry: &'a NodeRegistry,
    pool: Option<&'a sqlx::PgPool>,
    market_data: &'a DataSourceRegistry,
    cancel: CancellationToken,
    config: RunExecutionConfig,
    observer: &'a dyn ExecutionObserver,
    run: RunMetadata,
    started: Instant,
    messages_total: u64,
    per_node: HashMap<String, u64>,
    outputs: Value,
    partial: bool,
}

impl<'a> Execution<'a> {
    async fn run(&mut self, context: &ContextStore) -> RunOutcome {
        let flow = self.flow;
        let mut queue: VecDeque<(String, MessageEnvelope)> = VecDeque::new();
        if let Some(trigger_node_id) = self.run.trigger_node_id.as_deref() {
            let Some(node) = flow.nodes.iter().find(|node| node.id == trigger_node_id) else {
                return RunOutcome {
                    status: RunStatus::Failed,
                    outputs: json!({}),
                    messages_total: 0,
                    error: Some(json!({
                        "code": "run.start_node_missing",
                        "message": format!("start node '{trigger_node_id}' was not found in the published flow")
                    })),
                };
            };
            if node.node_type != "inject" {
                return RunOutcome {
                    status: RunStatus::Failed,
                    outputs: json!({}),
                    messages_total: 0,
                    error: Some(json!({
                        "code": "run.start_node_invalid",
                        "message": format!("start node '{trigger_node_id}' is not an inject node")
                    })),
                };
            }
            queue.push_back((node.id.clone(), MessageEnvelope::new(json!({}))));
        } else {
            for node in &flow.nodes {
                if node.node_type == "inject" {
                    queue.push_back((node.id.clone(), MessageEnvelope::new(json!({}))));
                }
            }
            if queue.is_empty() {
                for node in &flow.nodes {
                    if node.node_type == "data_load" || node.node_type == "data_load_range" {
                        queue.push_back((node.id.clone(), MessageEnvelope::new(json!({}))));
                    }
                }
            }
        }
        let result = Box::pin(self.run_graph(
            flow.nodes.as_slice(),
            flow.wires.as_slice(),
            context,
            "",
            queue,
            0,
            false,
        ))
        .await;
        match result {
            Ok(_) => RunOutcome {
                status: if self.partial {
                    RunStatus::Partial
                } else {
                    RunStatus::Succeeded
                },
                outputs: self.outputs.clone(),
                messages_total: self.messages_total,
                error: None,
            },
            Err(outcome) => outcome,
        }
    }

    /// Runs one graph level and returns messages emitted by structural output
    /// nodes. Top-level graphs do not contain structural nodes.
    #[allow(clippy::too_many_arguments, clippy::result_large_err)]
    async fn run_graph(
        &mut self,
        nodes: &'a [FlowNode],
        wires: &'a [Wire],
        context: &ContextStore,
        prefix: &str,
        mut queue: VecDeque<(String, MessageEnvelope)>,
        depth: u32,
        collect_output: bool,
    ) -> Result<Vec<Value>, RunOutcome> {
        let mut emitted = Vec::new();
        while let Some((node_id, envelope)) = queue.pop_front() {
            if self.cancel.is_cancelled() {
                return Err(self.failure(RunStatus::Cancelled, "run.cancelled", "run cancelled"));
            }
            if self.messages_total >= self.config.max_messages {
                return Err(self.failure(
                    RunStatus::Failed,
                    "run.loop_limit",
                    "maximum messages per run exceeded",
                ));
            }
            if self.started.elapsed() > self.config.max_duration {
                return Err(self.failure(
                    RunStatus::Failed,
                    "run.duration_limit",
                    "maximum run duration exceeded",
                ));
            }
            let Some(node) = nodes.iter().find(|node| node.id == node_id) else {
                return Err(self.failure(
                    RunStatus::Failed,
                    "run.internal",
                    format!("delivery references missing node '{node_id}'"),
                ));
            };
            let qualified = qualify(prefix, &node.id);
            let count = self.per_node.entry(qualified.clone()).or_insert(0);
            *count += 1;
            if *count > self.config.max_messages_per_node {
                return Err(self.failure(
                    RunStatus::Failed,
                    "run.loop_limit",
                    format!("node '{}' exceeded its message limit", node.id),
                ));
            }

            if node.node_type == subflow::INPUT_TYPE {
                self.messages_total += 1;
                self.observer
                    .node_started(&qualified, 1, &envelope.msg)
                    .await;
                let mut output = NodeOutput::empty();
                output.push("out", envelope.msg.clone());
                self.observer
                    .node_finished(&qualified, 1, Some(&output), None)
                    .await;
                self.observer
                    .message(&qualified, "out", &envelope.msg)
                    .await;
                enqueue_outgoing(wires, &node.id, "out", &envelope.msg, &mut queue);
                continue;
            }
            if node.node_type == subflow::OUTPUT_TYPE {
                self.messages_total += 1;
                self.observer
                    .node_started(&qualified, 1, &envelope.msg)
                    .await;
                self.observer
                    .node_finished(&qualified, 1, Some(&NodeOutput::empty()), None)
                    .await;
                if collect_output {
                    emitted.push(envelope.msg);
                }
                continue;
            }
            if node.node_type == subflow::NODE_TYPE {
                if depth >= self.config.max_subflow_depth {
                    return Err(self.failure(
                        RunStatus::Failed,
                        "run.subflow_depth",
                        format!(
                            "subflow '{}' exceeds the maximum depth of {}",
                            node.id, self.config.max_subflow_depth
                        ),
                    ));
                }
                let Some(subflow_id) = node.config.get("subflow_id").and_then(Value::as_str) else {
                    return Err(self.failure(
                        RunStatus::Failed,
                        "flow.subflow_missing",
                        format!("subflow node '{}' is missing config.subflow_id", node.id),
                    ));
                };
                let Some(definition) = self.flow.subflow(subflow_id) else {
                    return Err(self.failure(
                        RunStatus::Failed,
                        "flow.subflow_missing",
                        format!(
                            "subflow node '{}' references missing subflow '{}'",
                            node.id, subflow_id
                        ),
                    ));
                };
                let graph = match subflow::graph(definition) {
                    Ok(graph) => graph,
                    Err(error) => return Err(self.node_failure(&error)),
                };
                self.observer
                    .node_started(&qualified, 1, &envelope.msg)
                    .await;
                let child = context.child_flow(&qualified);
                let mut child_queue = VecDeque::new();
                child_queue.push_back((graph.input.id.clone(), envelope.clone()));
                let child_result = Box::pin(self.run_graph(
                    graph.subflow.nodes.as_slice(),
                    graph.subflow.wires.as_slice(),
                    &child,
                    &qualified,
                    child_queue,
                    depth + 1,
                    true,
                ))
                .await;
                let messages = match child_result {
                    Ok(messages) => match subflow::exit(definition, messages) {
                        Ok(messages) => messages,
                        Err(error) => return Err(self.node_failure(&error)),
                    },
                    Err(outcome) => return Err(outcome),
                };
                self.drain_durable_context(&child, true).await;
                let mut output = NodeOutput::empty();
                for message in &messages {
                    output.push("out", message.clone());
                }
                self.observer
                    .node_finished(&qualified, 1, Some(&output), None)
                    .await;
                for message in &messages {
                    self.observer.message(&qualified, "out", message).await;
                    enqueue_outgoing(wires, &node.id, "out", message, &mut queue);
                }
                self.deliver_event(
                    nodes,
                    node,
                    status_event(&node.id, "success"),
                    "status",
                    &mut queue,
                );
                self.deliver_event(
                    nodes,
                    node,
                    complete_event(&node.id),
                    "complete",
                    &mut queue,
                );
                continue;
            }

            let Some(worker) = self.registry.get(&node.node_type_key()) else {
                return Err(self.failure(
                    RunStatus::Failed,
                    "flow.unknown_type",
                    format!("unknown node type '{}'", node.node_type),
                ));
            };
            let descriptor = worker.descriptor();
            let denied = self.registry.denied_capabilities(&descriptor);
            if !denied.is_empty() {
                return Err(self.failure(
                    RunStatus::Failed,
                    "node.capability_missing",
                    format!(
                        "node '{}' requires capabilities this deployment does not allow: {}",
                        node.id,
                        denied.join(", ")
                    ),
                ));
            }

            let mut attempt = 1u32;
            let mut retries = 0u32;
            let result = loop {
                self.observer
                    .node_started(&qualified, attempt, &envelope.msg)
                    .await;
                let ctx = NodeContext {
                    run: self.run.clone(),
                    node_id: &qualified,
                    config: node.config.clone(),
                    msg: envelope.msg.clone(),
                    context,
                    pool: self.pool,
                    market_data: self.market_data,
                    secrets: &NoopSecrets,
                    cancel: self.cancel.clone(),
                    logger: &NoopLogger,
                };
                let attempt_result = tokio::time::timeout(
                    Duration::from_secs(node.timeout_seconds as u64),
                    worker.execute(ctx),
                )
                .await;
                let error = match attempt_result {
                    Ok(Ok(output)) => {
                        self.observer
                            .node_finished(&qualified, attempt, Some(&output), None)
                            .await;
                        break Ok(output);
                    }
                    Ok(Err(error)) => error,
                    Err(_) => node_timeout(node.timeout_seconds),
                };
                self.observer
                    .node_finished(&qualified, attempt, None, Some(&error))
                    .await;
                if policy::should_retry(node, attempt, &error) {
                    let backoff = policy::backoff_seconds(node, attempt);
                    if backoff > 0
                        && !sleep_or_cancel(Duration::from_secs(backoff), &self.cancel).await
                    {
                        return Err(self.failure(
                            RunStatus::Cancelled,
                            "run.cancelled",
                            "run cancelled",
                        ));
                    }
                    attempt += 1;
                    retries += 1;
                    continue;
                }
                break Err(error);
            };
            self.messages_total += 1;
            match result {
                Ok(output) => {
                    if let Some(value) = output.run_output.clone() {
                        self.outputs = value;
                    }
                    if let Some(status) = output.status.clone() {
                        self.deliver_event(
                            nodes,
                            node,
                            json!({
                                "status": status.status,
                                "detail": status.detail,
                                "payload": status.status,
                            }),
                            "status",
                            &mut queue,
                        );
                    }
                    for (port, messages) in output.messages {
                        for message in messages {
                            self.observer.message(&qualified, &port, &message).await;
                            enqueue_outgoing(wires, &node.id, &port, &message, &mut queue);
                        }
                    }
                    if !is_event_node(node) {
                        self.deliver_event(
                            nodes,
                            node,
                            status_event(&node.id, "success"),
                            "status",
                            &mut queue,
                        );
                        self.deliver_event(
                            nodes,
                            node,
                            complete_event(&node.id),
                            "complete",
                            &mut queue,
                        );
                    }
                }
                Err(error) => {
                    if !is_event_node(node) {
                        self.deliver_event(
                            nodes,
                            node,
                            status_event(&node.id, "error"),
                            "status",
                            &mut queue,
                        );
                    }
                    let caught = self.deliver_catch(nodes, node, &error, retries, &mut queue);
                    match node.error_policy.as_str() {
                        "continue" => {
                            self.partial = true;
                        }
                        "catch" if caught => {
                            self.partial = true;
                        }
                        "fail" if caught => {
                            self.partial = true;
                        }
                        _ => return Err(self.node_failure(&error)),
                    }
                }
            }
        }
        Ok(emitted)
    }

    /// Delivers an event to matching `status`/`complete` nodes in this graph.
    fn deliver_event(
        &self,
        nodes: &[FlowNode],
        source: &FlowNode,
        message: Value,
        target_type: &str,
        queue: &mut VecDeque<(String, MessageEnvelope)>,
    ) {
        for target in nodes
            .iter()
            .filter(|candidate| candidate.node_type == target_type)
            .filter(|candidate| event_scope_matches(candidate, &source.id))
        {
            queue.push_back((target.id.clone(), MessageEnvelope::new(message.clone())));
        }
    }

    /// Delivers a catch event and reports whether at least one catch node
    /// accepted it.
    fn deliver_catch(
        &self,
        nodes: &[FlowNode],
        source: &FlowNode,
        error: &NodeExecutionError,
        retry_count: u32,
        queue: &mut VecDeque<(String, MessageEnvelope)>,
    ) -> bool {
        let targets: Vec<&FlowNode> = nodes
            .iter()
            .filter(|candidate| candidate.node_type == "catch")
            .filter(|candidate| event_scope_matches(candidate, &source.id))
            .collect();
        if targets.is_empty() {
            return false;
        }
        let has_unconditional = targets
            .iter()
            .any(|candidate| !candidate.config_bool("uncaught_only"));
        let message = error_event(&source.id, error, retry_count);
        let mut delivered = false;
        for target in targets {
            if target.config_bool("uncaught_only") && has_unconditional {
                continue;
            }
            queue.push_back((target.id.clone(), MessageEnvelope::new(message.clone())));
            delivered = true;
        }
        delivered
    }

    async fn drain_durable_context(&self, context: &ContextStore, flow_only: bool) {
        let flow_scope = context.flow_scope_id().to_string();
        for key in context.take_dirty_flow() {
            if let Some(value) = context.get(ContextScope::Flow, &key) {
                self.observer
                    .context_changed("flow", &flow_scope, &key, &value)
                    .await;
            }
        }
        if flow_only {
            return;
        }
        let global_scope = context.global_scope_id().to_string();
        for key in context.take_dirty_global() {
            if let Some(value) = context.get(ContextScope::Global, &key) {
                self.observer
                    .context_changed("global", &global_scope, &key, &value)
                    .await;
            }
        }
    }

    fn failure(&self, status: RunStatus, code: &str, message: impl Into<String>) -> RunOutcome {
        RunOutcome {
            status,
            outputs: self.outputs.clone(),
            messages_total: self.messages_total,
            error: Some(json!({ "code": code, "message": message.into() })),
        }
    }

    fn node_failure(&self, error: &NodeExecutionError) -> RunOutcome {
        RunOutcome {
            status: RunStatus::Failed,
            outputs: self.outputs.clone(),
            messages_total: self.messages_total,
            error: Some(error_value(error)),
        }
    }
}

trait NodeConfigExt {
    fn config_bool(&self, key: &str) -> bool;
}

impl NodeConfigExt for FlowNode {
    fn config_bool(&self, key: &str) -> bool {
        self.config
            .get(key)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
}

fn is_event_node(node: &FlowNode) -> bool {
    matches!(node.node_type.as_str(), "catch" | "status" | "complete")
}

fn qualify(prefix: &str, node_id: &str) -> String {
    if prefix.is_empty() {
        node_id.to_string()
    } else {
        format!("{prefix}/{node_id}")
    }
}

fn enqueue_outgoing(
    wires: &[Wire],
    node_id: &str,
    port: &str,
    message: &Value,
    queue: &mut VecDeque<(String, MessageEnvelope)>,
) {
    for wire in wires
        .iter()
        .filter(|wire| wire.source == node_id && wire.source_port == port)
    {
        // Each wire receives an independent clone with its own message id.
        let envelope = MessageEnvelope::new(message.clone()).clone_for_wire();
        queue.push_back((wire.target.clone(), envelope));
    }
}

fn node_timeout(seconds: u32) -> NodeExecutionError {
    NodeExecutionError::retryable(
        "node.timeout",
        format!("node timed out after {seconds} seconds"),
    )
}

/// Sleeps for `duration` unless the run is cancelled. Returns false when
/// cancelled.
async fn sleep_or_cancel(duration: Duration, cancel: &CancellationToken) -> bool {
    tokio::select! {
        () = tokio::time::sleep(duration) => true,
        () = cancel.cancelled() => false,
    }
}

fn status_event(source: &str, status: &str) -> Value {
    json!({
        "status": status,
        "status_detail": { "source": source, "status": status },
        "payload": status,
    })
}

fn complete_event(source: &str) -> Value {
    json!({
        "complete": { "source": source },
        "payload": "complete",
    })
}

fn error_event(source: &str, error: &NodeExecutionError, retry_count: u32) -> Value {
    json!({
        "error": {
            "source": source,
            "code": error.code,
            "message": error.message,
            "retry_count": retry_count,
            "retryable": error.retryable,
        },
        "payload": error.message,
    })
}

fn event_scope_matches(candidate: &FlowNode, source_id: &str) -> bool {
    let scope = candidate
        .config
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or("flow");
    match scope {
        "flow" => true,
        "selected" => candidate
            .config
            .get("node_ids")
            .and_then(Value::as_array)
            .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(source_id))),
        _ => false,
    }
}

fn error_value(error: &NodeExecutionError) -> Value {
    serde_json::to_value(error)
        .unwrap_or_else(|_| json!({ "code": error.code, "message": error.message }))
}
