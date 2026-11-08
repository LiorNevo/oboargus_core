//! Log node: writes a structured run log entry.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::automation::error::NodeExecutionError;
use crate::automation::nodes::common::{descriptor, input_port, output_port};
use crate::automation::nodes::{NodeCategory, NodeContext, NodeOutput, WorkflowNode};

/// Log configuration.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default)]
pub struct LogConfig {
    /// Log level.
    pub level: String,
    /// Message template.
    pub message: String,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            message: String::new(),
        }
    }
}

/// The log node.
pub struct LogNode;

#[async_trait]
impl WorkflowNode for LogNode {
    fn descriptor(&self) -> crate::automation::nodes::NodeTypeDescriptor {
        descriptor::<LogConfig>(
            "log",
            1,
            NodeCategory::Notify,
            "Log",
            "Writes a structured entry to the run log.",
            "article",
            Some(input_port()),
            vec![output_port()],
            Vec::new(),
            "Message templates use ${...} expressions. Secrets are redacted by the runtime.",
        )
    }

    async fn execute(&self, ctx: NodeContext<'_>) -> Result<NodeOutput, NodeExecutionError> {
        let config: LogConfig = serde_json::from_value(ctx.config.clone())
            .map_err(|error| NodeExecutionError::new("node.config_invalid", error.to_string()))?;
        let message =
            crate::automation::expression::eval_str(&config.message, &ctx.expression_context())
                .map(|value| match value {
                    serde_json::Value::String(text) => text,
                    other => other.to_string(),
                })
                .unwrap_or_else(|_| config.message.clone());
        ctx.logger.log(&config.level, &message, None);
        let mut output = NodeOutput::empty();
        output.push("out", ctx.msg.clone());
        Ok(output)
    }
}
