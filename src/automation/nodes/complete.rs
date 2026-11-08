//! Complete node: receives successful-completion events.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::automation::error::NodeExecutionError;
use crate::automation::nodes::common::{descriptor, input_port, output_port};
use crate::automation::nodes::{NodeCategory, NodeContext, NodeOutput, WorkflowNode};

/// Complete configuration.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CompleteConfig {
    /// `flow` or `selected`.
    pub scope: String,
    /// Selected node ids.
    pub node_ids: Vec<String>,
}

impl Default for CompleteConfig {
    fn default() -> Self {
        Self {
            scope: "flow".to_string(),
            node_ids: Vec::new(),
        }
    }
}

/// The complete node.
pub struct CompleteNode;

#[async_trait]
impl WorkflowNode for CompleteNode {
    fn descriptor(&self) -> crate::automation::nodes::NodeTypeDescriptor {
        descriptor::<CompleteConfig>(
            "complete",
            1,
            NodeCategory::Error,
            "Complete",
            "Receives successful-completion events from nodes in its scope.",
            "task_alt",
            Some(input_port()),
            vec![output_port()],
            Vec::new(),
            "Informational: a complete node does not change the run status.",
        )
    }

    async fn execute(&self, ctx: NodeContext<'_>) -> Result<NodeOutput, NodeExecutionError> {
        let mut output = NodeOutput::empty();
        output.push("out", ctx.msg.clone());
        Ok(output)
    }
}
