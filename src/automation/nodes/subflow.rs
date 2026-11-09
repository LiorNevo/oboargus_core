//! Subflow node: instantiates a reusable flow fragment.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::automation::error::NodeExecutionError;
use crate::automation::nodes::common::{descriptor, input_port, output_port};
use crate::automation::nodes::{NodeCategory, NodeContext, NodeOutput, WorkflowNode};

/// Subflow configuration.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SubflowConfig {
    /// Referenced subflow id.
    pub subflow_id: String,
}

/// The subflow node.
pub struct SubflowNode;

#[async_trait]
impl WorkflowNode for SubflowNode {
    fn descriptor(&self) -> crate::automation::nodes::NodeTypeDescriptor {
        descriptor::<SubflowConfig>(
            "subflow",
            1,
            NodeCategory::Flow,
            "Subflow",
            "Runs a reusable flow fragment as a node.",
            "account_tree",
            Some(input_port()),
            vec![output_port()],
            Vec::new(),
            "Runs a reusable flow fragment as a node.\n\nThe runtime expands the subflow with its own flow context and forwards its output.",
        )
    }

    async fn execute(&self, ctx: NodeContext<'_>) -> Result<NodeOutput, NodeExecutionError> {
        let mut output = NodeOutput::empty();
        output.push("out", ctx.msg.clone());
        Ok(output)
    }
}
