//! Subflow resolution and instance scoping.
//!
//! A subflow is a flow-local graph with exactly one structural input node and
//! one or more structural output nodes. The runtime expands an instance by
//! seeding the input node with the incoming message and forwarding messages
//! emitted by the output nodes.

use serde_json::Value;

use crate::automation::error::NodeExecutionError;
use crate::automation::flow::document::{FlowNode, Subflow};

/// Node type id for the structural subflow input node.
pub const INPUT_TYPE: &str = "subflow_input";
/// Node type id for the structural subflow output node.
pub const OUTPUT_TYPE: &str = "subflow_output";
/// Node type id for a subflow instance.
pub const NODE_TYPE: &str = "subflow";
/// Default maximum number of nested subflow instances.
pub const DEFAULT_MAX_DEPTH: u32 = 10;

/// A resolved subflow graph ready for execution.
#[derive(Debug, Clone)]
pub struct SubflowGraph<'a> {
    /// The subflow definition.
    pub subflow: &'a Subflow,
    /// The structural input node.
    pub input: &'a FlowNode,
    /// The structural output nodes.
    pub outputs: Vec<&'a FlowNode>,
}

/// Returns true for structural (non-executable) subflow node types.
pub fn is_structural(node_type: &str) -> bool {
    node_type == INPUT_TYPE || node_type == OUTPUT_TYPE
}

/// Returns the instance scope id used for subflow flow context.
pub fn instance_scope(flow_scope_id: &str, instance_id: &str) -> String {
    format!("{flow_scope_id}:{instance_id}")
}

/// Returns a node id that is unique within a run.
pub fn qualified_node_id(instance_id: &str, node_id: &str) -> String {
    format!("{instance_id}/{node_id}")
}

/// Resolves a subflow definition into its executable graph.
///
/// Validation rejects malformed definitions before a run starts; this function
/// repeats the checks so a direct runtime invocation fails safely.
pub fn graph(subflow: &Subflow) -> Result<SubflowGraph<'_>, NodeExecutionError> {
    let inputs: Vec<&FlowNode> = subflow
        .nodes
        .iter()
        .filter(|node| node.node_type == INPUT_TYPE)
        .collect();
    if inputs.len() != 1 {
        return Err(NodeExecutionError::new(
            "flow.subflow_io",
            format!(
                "subflow '{}' must contain exactly one subflow_input node",
                subflow.id
            ),
        ));
    }
    let input = inputs[0];
    if subflow.inputs.len() != 1 || subflow.inputs[0].id != input.id {
        return Err(NodeExecutionError::new(
            "flow.subflow_io",
            format!(
                "subflow '{}' input declaration must reference node '{}'",
                subflow.id, input.id
            ),
        ));
    }
    let outputs: Vec<&FlowNode> = subflow
        .nodes
        .iter()
        .filter(|node| node.node_type == OUTPUT_TYPE)
        .collect();
    if outputs.is_empty() {
        return Err(NodeExecutionError::new(
            "flow.subflow_io",
            format!(
                "subflow '{}' must contain at least one subflow_output node",
                subflow.id
            ),
        ));
    }
    for declared in &subflow.outputs {
        if !outputs.iter().any(|node| node.id == declared.id) {
            return Err(NodeExecutionError::new(
                "flow.subflow_io",
                format!(
                    "subflow '{}' output '{}' does not reference a subflow_output node",
                    subflow.id, declared.id
                ),
            ));
        }
    }
    if subflow.outputs.len() != outputs.len() {
        return Err(NodeExecutionError::new(
            "flow.subflow_io",
            format!(
                "subflow '{}' declares {} outputs but contains {} subflow_output nodes",
                subflow.id,
                subflow.outputs.len(),
                outputs.len()
            ),
        ));
    }
    // The output vectors borrow from `subflow`, so they share its lifetime.
    let output_nodes: Vec<&FlowNode> = outputs;
    Ok(SubflowGraph {
        subflow,
        input,
        outputs: output_nodes,
    })
}

/// Returns the subflow input message for an instance.
pub fn enter(subflow: &Subflow, message: Value) -> Result<Value, NodeExecutionError> {
    graph(subflow)?;
    Ok(message)
}

/// Collects output-node messages into one subflow result.
///
/// v1 exposes a single `out` port on the instance, so messages are
/// concatenated in output-node declaration order.
pub fn exit(_subflow: &Subflow, messages: Vec<Value>) -> Result<Vec<Value>, NodeExecutionError> {
    Ok(messages)
}
