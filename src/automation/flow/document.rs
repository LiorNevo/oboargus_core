//! Canonical flow document.
//!
//! The document is the single source of truth for authoring, validation,
//! execution, and JSON interchange. It is versioned with `schema_version` and
//! validated against a generated JSON Schema.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Current flow document schema version.
pub const FLOW_SCHEMA_VERSION: u32 = 1;

/// A versioned automation flow document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FlowDocument {
    /// Document schema version.
    pub schema_version: u32,
    /// Display name.
    pub name: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// Tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Runtime inputs.
    #[serde(default)]
    pub inputs: Vec<RuntimeInput>,
    /// Nodes.
    #[serde(default)]
    pub nodes: Vec<FlowNode>,
    /// Wires.
    #[serde(default)]
    pub wires: Vec<Wire>,
    /// Subflows.
    #[serde(default)]
    pub subflows: Vec<Subflow>,
}

impl FlowDocument {
    /// Creates an empty document for a new flow.
    pub fn empty(name: impl Into<String>) -> Self {
        Self {
            schema_version: FLOW_SCHEMA_VERSION,
            name: name.into(),
            description: String::new(),
            tags: Vec::new(),
            inputs: Vec::new(),
            nodes: Vec::new(),
            wires: Vec::new(),
            subflows: Vec::new(),
        }
    }

    /// Returns a node by id.
    pub fn node(&self, id: &str) -> Option<&FlowNode> {
        self.nodes.iter().find(|node| node.id == id)
    }

    /// Returns a subflow by id.
    pub fn subflow(&self, id: &str) -> Option<&Subflow> {
        self.subflows.iter().find(|subflow| subflow.id == id)
    }

    /// Returns the wires leaving a node output.
    pub fn outgoing(&self, node_id: &str, port: &str) -> Vec<&Wire> {
        self.wires
            .iter()
            .filter(|wire| wire.source == node_id && wire.source_port == port)
            .collect()
    }

    /// Returns the wires entering a node.
    pub fn incoming(&self, node_id: &str) -> Vec<&Wire> {
        self.wires
            .iter()
            .filter(|wire| wire.target == node_id)
            .collect()
    }

    /// Returns a map of node id to node.
    pub fn node_map(&self) -> BTreeMap<&str, &FlowNode> {
        self.nodes
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect()
    }

    /// Returns the set of declared output ports for a node type.
    pub fn output_ports<'a>(
        &self,
        node: &'a FlowNode,
        descriptors: &'a BTreeMap<String, crate::automation::nodes::NodeTypeDescriptor>,
    ) -> Option<BTreeSet<&'a str>> {
        descriptors.get(&node.node_type_key()).map(|descriptor| {
            descriptor
                .outputs
                .iter()
                .map(|port| port.name.as_str())
                .collect()
        })
    }
}

/// A declared runtime input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeInput {
    /// Input name.
    pub name: String,
    /// JSON type name.
    #[serde(rename = "type")]
    pub input_type: String,
    /// Required flag.
    #[serde(default)]
    pub required: bool,
    /// Optional default.
    #[serde(default)]
    pub default: Option<Value>,
    /// Secret flag.
    #[serde(default)]
    pub secret: bool,
    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
}

/// A configured node instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FlowNode {
    /// Node id.
    pub id: String,
    /// Node type id.
    #[serde(rename = "type")]
    pub node_type: String,
    /// Node type version.
    pub type_version: u32,
    /// Display label.
    #[serde(default)]
    pub label: String,
    /// Canvas position.
    #[serde(default)]
    pub position: NodePosition,
    /// Node configuration.
    #[serde(default = "empty_object")]
    pub config: Value,
    /// Retry policy.
    #[serde(default)]
    pub retry: RetryPolicy,
    /// Timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
    /// Error policy.
    #[serde(default = "default_error_policy")]
    pub error_policy: String,
}

impl FlowNode {
    /// Key used by the node registry.
    pub fn node_type_key(&self) -> String {
        format!("{}@{}", self.node_type, self.type_version)
    }
}

/// Canvas position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct NodePosition {
    /// X coordinate.
    #[serde(default)]
    pub x: i32,
    /// Y coordinate.
    #[serde(default)]
    pub y: i32,
}

/// Retry policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RetryPolicy {
    /// Maximum attempts.
    #[serde(default = "default_attempts")]
    pub max_attempts: u32,
    /// Backoff base in seconds.
    #[serde(default)]
    pub backoff_seconds: u32,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 1,
            backoff_seconds: 0,
        }
    }
}

/// A wire from an output port to a target input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Wire {
    /// Wire id.
    pub id: String,
    /// Source node id.
    pub source: String,
    /// Source output port.
    #[serde(default = "default_out")]
    pub source_port: String,
    /// Target node id.
    pub target: String,
    /// Target input port.
    #[serde(default = "default_in")]
    pub target_port: String,
}

/// A reusable flow fragment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Subflow {
    /// Subflow id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Declared inputs.
    #[serde(default)]
    pub inputs: Vec<SubflowInput>,
    /// Declared outputs.
    #[serde(default)]
    pub outputs: Vec<SubflowOutput>,
    /// Internal nodes.
    #[serde(default)]
    pub nodes: Vec<FlowNode>,
    /// Internal wires.
    #[serde(default)]
    pub wires: Vec<Wire>,
}

/// A subflow input declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubflowInput {
    /// Input id.
    pub id: String,
    /// Display label.
    #[serde(default)]
    pub label: String,
}

/// A subflow output declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubflowOutput {
    /// Output id.
    pub id: String,
    /// Display label.
    #[serde(default)]
    pub label: String,
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

fn default_timeout() -> u32 {
    300
}

fn default_error_policy() -> String {
    "fail".to_string()
}

fn default_attempts() -> u32 {
    1
}

fn default_out() -> String {
    "out".to_string()
}

fn default_in() -> String {
    "in".to_string()
}
