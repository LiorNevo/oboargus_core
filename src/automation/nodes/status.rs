//! Status node: receives node status events.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::automation::error::NodeExecutionError;
use crate::automation::nodes::common::{descriptor, input_port, output_port};
use crate::automation::nodes::{NodeCategory, NodeContext, NodeOutput, WorkflowNode};

/// Status configuration.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default)]
pub struct StatusConfig {
    /// `flow` or `selected`.
    pub scope: String,
    /// Selected node ids.
    pub node_ids: Vec<String>,
}

impl Default for StatusConfig {
    fn default() -> Self {
        Self {
            scope: "flow".to_string(),
            node_ids: Vec::new(),
        }
    }
}

/// The status node.
pub struct StatusNode;

#[async_trait]
impl WorkflowNode for StatusNode {
    fn descriptor(&self) -> crate::automation::nodes::NodeTypeDescriptor {
        descriptor::<StatusConfig>(
            "status",
            1,
            NodeCategory::Error,
            "Status",
            "Receives status events from nodes in its scope.",
            "monitor_heart",
            Some(input_port()),
            vec![output_port()],
            Vec::new(),
            "Receives status events from nodes in its scope.\n\nThis is informational: a status node does not change the run status.",
        )
    }

    async fn execute(&self, ctx: NodeContext<'_>) -> Result<NodeOutput, NodeExecutionError> {
        let mut output = NodeOutput::empty();
        output.push("out", ctx.msg.clone());
        Ok(output)
    }
}
