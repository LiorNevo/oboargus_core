//! Catch node: receives node errors in its scope.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::automation::error::NodeExecutionError;
use crate::automation::nodes::common::{descriptor, input_port, output_port};
use crate::automation::nodes::{NodeCategory, NodeContext, NodeOutput, WorkflowNode};

/// Catch configuration.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CatchConfig {
    /// `flow` or `selected`.
    pub scope: String,
    /// Selected node ids.
    pub node_ids: Vec<String>,
    /// Only uncaught errors.
    pub uncaught_only: bool,
}

impl Default for CatchConfig {
    fn default() -> Self {
        Self {
            scope: "flow".to_string(),
            node_ids: Vec::new(),
            uncaught_only: false,
        }
    }
}

/// The catch node.
pub struct CatchNode;

#[async_trait]
impl WorkflowNode for CatchNode {
    fn descriptor(&self) -> crate::automation::nodes::NodeTypeDescriptor {
        descriptor::<CatchConfig>(
            "catch",
            1,
            NodeCategory::Error,
            "Catch",
            "Receives node errors in its scope and routes them to a recovery path.",
            "error",
            Some(input_port()),
            vec![output_port()],
            Vec::new(),
            "Receives errors from nodes in its scope and routes them to a recovery path.\n\nThe input message carries `error.source`, `error.code`, `error.message`, and `error.retry_count`.",
        )
    }

    async fn execute(&self, ctx: NodeContext<'_>) -> Result<NodeOutput, NodeExecutionError> {
        let mut output = NodeOutput::empty();
        output.push("out", ctx.msg.clone());
        Ok(output)
    }
}
