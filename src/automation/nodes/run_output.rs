//! Run output node: sets the final output of a run.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::automation::error::NodeExecutionError;
use crate::automation::nodes::common::{descriptor, input_port};
use crate::automation::nodes::{NodeCategory, NodeContext, NodeOutput, WorkflowNode};

/// Run output configuration.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default)]
pub struct RunOutputConfig {
    /// Output value: literal or expression object.
    pub value: Value,
}

impl Default for RunOutputConfig {
    fn default() -> Self {
        Self { value: Value::Null }
    }
}

/// The run output node.
pub struct RunOutputNode;

#[async_trait]
impl WorkflowNode for RunOutputNode {
    fn descriptor(&self) -> crate::automation::nodes::NodeTypeDescriptor {
        descriptor::<RunOutputConfig>(
            "run_output",
            1,
            NodeCategory::Output,
            "Run output",
            "Sets the final output of the run.",
            "output",
            Some(input_port()),
            Vec::new(),
            vec!["write:run_output".to_string()],
            "Sets the final output of the run.\n\nIf multiple run output nodes fire, the last completed one wins and a warning is logged.",
        )
    }

    async fn execute(&self, ctx: NodeContext<'_>) -> Result<NodeOutput, NodeExecutionError> {
        let config: RunOutputConfig = serde_json::from_value(ctx.config.clone())
            .map_err(|error| NodeExecutionError::new("node.config_invalid", error.to_string()))?;
        let value = ctx.resolve(&config.value)?;
        let mut output = NodeOutput::empty();
        output.run_output = Some(value);
        Ok(output)
    }
}
