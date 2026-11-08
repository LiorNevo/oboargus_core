//! Node type contract, registry, and built-in node implementations.

pub mod catch;
pub mod change;
pub mod common;
pub mod complete;
pub mod data_load;
pub mod data_load_range;
pub mod debug;
pub mod delay;
pub mod inject;
pub mod log;
pub mod notification;
pub mod run_output;
pub mod status;
pub mod subflow;
pub mod switch;
pub mod transform;

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::automation::error::NodeExecutionError;
use crate::automation::runtime::bus::RunMetadata;
use crate::automation::runtime::context::ContextStore;
use crate::market_data::registry::DataSourceRegistry;

/// Palette category for a node type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeCategory {
    /// Trigger/source nodes.
    Trigger,
    /// Data nodes.
    Data,
    /// Transformation nodes.
    Transform,
    /// Flow-control nodes.
    Flow,
    /// Error/status handling nodes.
    Error,
    /// Notification/log nodes.
    Notify,
    /// Sink/output nodes.
    Output,
}

impl NodeCategory {
    /// Display string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trigger => "trigger",
            Self::Data => "data",
            Self::Transform => "transform",
            Self::Flow => "flow",
            Self::Error => "error",
            Self::Notify => "notify",
            Self::Output => "output",
        }
    }
}

/// A node input or output port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortDescriptor {
    /// Stable port name.
    pub name: String,
    /// Display label.
    #[serde(default)]
    pub label: String,
}

/// A node type descriptor served to the editor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NodeTypeDescriptor {
    /// Stable type id.
    pub id: String,
    /// Immutable version.
    pub version: u32,
    /// Palette category.
    pub category: NodeCategory,
    /// Display name.
    pub display_name: String,
    /// Description.
    pub description: String,
    /// Icon name.
    pub icon: String,
    /// Optional input port.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<PortDescriptor>,
    /// Output ports.
    pub outputs: Vec<PortDescriptor>,
    /// JSON Schema for the node configuration.
    pub config_schema: Value,
    /// JSON Schema for the node output payload.
    pub output_schema: Value,
    /// Required capabilities.
    pub capabilities: Vec<String>,
    /// Deprecation flag.
    pub deprecated: bool,
    /// User-facing documentation.
    pub docs: String,
}

impl NodeTypeDescriptor {
    /// Registry key.
    pub fn key(&self) -> String {
        format!("{}@{}", self.id, self.version)
    }
}

/// A node's output messages keyed by output port.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeOutput {
    /// Messages per output port.
    pub messages: BTreeMap<String, Vec<Value>>,
    /// Final run output, set by a `run_output` node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_output: Option<Value>,
    /// Optional status event for `status` nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<NodeStatusEvent>,
}

impl NodeOutput {
    /// Creates an empty output.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Adds one message on a port.
    pub fn push(&mut self, port: &str, message: Value) {
        self.messages
            .entry(port.to_string())
            .or_default()
            .push(message);
    }

    /// Sets the status event emitted for this execution.
    pub fn with_status(mut self, status: NodeStatusEvent) -> Self {
        self.status = Some(status);
        self
    }

    /// Returns true when no messages were emitted.
    pub fn is_empty(&self) -> bool {
        self.messages.values().all(Vec::is_empty)
    }
}

/// A status event emitted by a node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeStatusEvent {
    /// Status text.
    pub status: String,
    /// Optional detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

/// Resolves named secrets from the existing configuration mechanism.
pub trait SecretResolver: Send + Sync {
    /// Resolves a secret by name.
    fn resolve(&self, name: &str) -> Option<String>;
}

/// A resolver that returns no secrets.
pub struct NoopSecrets;

impl SecretResolver for NoopSecrets {
    fn resolve(&self, _name: &str) -> Option<String> {
        None
    }
}

/// Structured node logger.
pub trait NodeLogger: Send + Sync {
    /// Writes one log entry.
    fn log(&self, level: &str, message: &str, fields: Option<Value>);
}

/// A logger that discards entries.
pub struct NoopLogger;

impl NodeLogger for NoopLogger {
    fn log(&self, _level: &str, _message: &str, _fields: Option<Value>) {}
}

/// Execution context passed to a node.
pub struct NodeContext<'a> {
    /// Run metadata.
    pub run: RunMetadata,
    /// Node id.
    pub node_id: &'a str,
    /// Resolved node configuration.
    pub config: Value,
    /// Input message.
    pub msg: Value,
    /// Run/flow/global context.
    pub context: &'a ContextStore,
    /// Database pool.
    pub pool: Option<&'a sqlx::PgPool>,
    /// Market-data registry.
    pub market_data: &'a DataSourceRegistry,
    /// Secret resolver.
    pub secrets: &'a dyn SecretResolver,
    /// Cancellation token.
    pub cancel: CancellationToken,
    /// Node logger.
    pub logger: &'a dyn NodeLogger,
}

impl NodeContext<'_> {
    /// Resolves a configuration field that may be a literal, reference,
    /// expression, or template object.
    pub fn resolve(&self, value: &Value) -> Result<Value, NodeExecutionError> {
        crate::automation::expression::resolve_value(value, &self.expression_context())
    }

    /// Builds the expression evaluation context.
    pub fn expression_context(&self) -> crate::automation::expression::EvalContext<'_> {
        crate::automation::expression::EvalContext {
            msg: &self.msg,
            inputs: &self.run.inputs,
            run: &self.run,
            context: self.context,
        }
    }

    /// Reads a required config field.
    pub fn config_field(&self, name: &str) -> Result<&Value, NodeExecutionError> {
        self.config.get(name).ok_or_else(|| {
            NodeExecutionError::new(
                "node.config_invalid",
                format!("missing required config field '{name}'"),
            )
        })
    }

    /// Reads a config field as a string after expression resolution.
    pub fn resolved_string(&self, name: &str) -> Result<String, NodeExecutionError> {
        let value = self.resolve(self.config_field(name)?)?;
        value.as_str().map(str::to_owned).ok_or_else(|| {
            NodeExecutionError::new(
                "node.config_invalid",
                format!("config field '{name}' must be a string"),
            )
        })
    }
}

/// One executable step.
#[async_trait]
pub trait WorkflowNode: Send + Sync {
    /// Descriptor served to the editor.
    fn descriptor(&self) -> NodeTypeDescriptor;

    /// Executes one input message.
    async fn execute(&self, ctx: NodeContext<'_>) -> Result<NodeOutput, NodeExecutionError>;
}

/// A registry of node types keyed by `id@version`.
///
/// An optional deployment capability allowlist can be attached. When present,
/// a node type may only be used if every capability it declares is allowed.
/// `None` means the deployment accepts every capability (the development
/// default); `all` in an explicit allowlist does the same.
#[derive(Default)]
pub struct NodeRegistry {
    nodes: BTreeMap<String, Arc<dyn WorkflowNode>>,
    capabilities: Option<std::collections::BTreeSet<String>>,
}

impl NodeRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Restricts the registry to a deployment capability allowlist.
    ///
    /// The value is the comma-separated `AUTOMATION_ENABLED_CAPABILITIES`
    /// form; an empty string disables every capability-bearing node type.
    pub fn with_capabilities(mut self, allowlist: &str) -> Self {
        let capabilities: std::collections::BTreeSet<String> = allowlist
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect();
        self.capabilities = Some(capabilities);
        self
    }

    /// Returns true when `capability` is allowed by this deployment.
    pub fn allows_capability(&self, capability: &str) -> bool {
        match &self.capabilities {
            None => true,
            Some(allowed) => allowed.contains("all") || allowed.contains(capability),
        }
    }

    /// Returns the descriptor capabilities the deployment does not allow.
    pub fn denied_capabilities(&self, descriptor: &NodeTypeDescriptor) -> Vec<String> {
        descriptor
            .capabilities
            .iter()
            .filter(|capability| !self.allows_capability(capability))
            .cloned()
            .collect()
    }

    /// Registers a node type.
    pub fn register(&mut self, node: Arc<dyn WorkflowNode>) {
        let descriptor = node.descriptor();
        self.nodes.insert(descriptor.key(), node);
    }

    /// Looks up an exact `id@version`.
    pub fn get(&self, key: &str) -> Option<Arc<dyn WorkflowNode>> {
        self.nodes.get(key).cloned()
    }

    /// Returns descriptors for every registered node type.
    pub fn descriptors(&self) -> Vec<NodeTypeDescriptor> {
        self.nodes.values().map(|node| node.descriptor()).collect()
    }

    /// Returns the latest non-deprecated descriptor for each id.
    pub fn latest_descriptors(&self) -> Vec<NodeTypeDescriptor> {
        let mut latest: BTreeMap<String, NodeTypeDescriptor> = BTreeMap::new();
        for descriptor in self.descriptors() {
            if descriptor.deprecated {
                continue;
            }
            latest
                .entry(descriptor.id.clone())
                .and_modify(|current| {
                    if descriptor.version > current.version {
                        *current = descriptor.clone();
                    }
                })
                .or_insert(descriptor);
        }
        latest.into_values().collect()
    }
}

/// Builds the built-in node registry.
pub fn default_registry() -> NodeRegistry {
    let mut registry = NodeRegistry::new();
    registry.register(Arc::new(inject::InjectNode));
    registry.register(Arc::new(debug::DebugNode));
    registry.register(Arc::new(switch::SwitchNode));
    registry.register(Arc::new(change::ChangeNode));
    registry.register(Arc::new(delay::DelayNode));
    registry.register(Arc::new(transform::TransformNode));
    registry.register(Arc::new(log::LogNode));
    registry.register(Arc::new(notification::NotificationNode));
    registry.register(Arc::new(catch::CatchNode));
    registry.register(Arc::new(status::StatusNode));
    registry.register(Arc::new(complete::CompleteNode));
    registry.register(Arc::new(subflow::SubflowNode));
    registry.register(Arc::new(run_output::RunOutputNode));
    registry.register(Arc::new(data_load::DataLoadNode));
    registry.register(Arc::new(data_load_range::DataLoadRangeNode));
    registry
}

/// Serializes a schemars schema to JSON.
pub fn schema_value<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).unwrap_or(Value::Null)
}
