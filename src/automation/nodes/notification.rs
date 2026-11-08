//! Notification node: emits a run notification.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::automation::error::NodeExecutionError;
use crate::automation::nodes::common::{descriptor, input_port, output_port};
use crate::automation::nodes::{NodeCategory, NodeContext, NodeOutput, WorkflowNode};

/// Notification configuration.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default)]
pub struct NotificationConfig {
    /// Notification message.
    pub message: String,
    /// Level.
    pub level: String,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            message: String::new(),
            level: "notice".to_string(),
        }
    }
}

/// The notification node.
pub struct NotificationNode;

#[async_trait]
impl WorkflowNode for NotificationNode {
    fn descriptor(&self) -> crate::automation::nodes::NodeTypeDescriptor {
        descriptor::<NotificationConfig>(
            "notification",
            1,
            NodeCategory::Notify,
            "Notification",
            "Emits a notification for the run view and subscribers.",
            "notifications",
            Some(input_port()),
            vec![output_port()],
            Vec::new(),
            "v1 delivers notifications inside the run and its SSE stream; external channels are out of scope.",
        )
    }

    async fn execute(&self, ctx: NodeContext<'_>) -> Result<NodeOutput, NodeExecutionError> {
        let config: NotificationConfig = serde_json::from_value(ctx.config.clone())
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
