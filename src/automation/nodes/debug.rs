//! Debug node: shows a message without changing the flow.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::automation::error::NodeExecutionError;
use crate::automation::nodes::common::{descriptor, input_port};
use crate::automation::nodes::{NodeCategory, NodeContext, NodeOutput, WorkflowNode};

/// Debug configuration.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default)]
pub struct DebugConfig {
    /// `payload`, `msg`, or `expression`.
    pub target: String,
    /// Expression when target is `expression`.
    pub expression: Option<Value>,
    /// Emit to the debug sidebar.
    pub to_sidebar: bool,
    /// Emit to the run log.
    pub to_log: bool,
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            target: "payload".to_string(),
            expression: None,
            to_sidebar: true,
            to_log: false,
        }
    }
}

/// The debug node.
pub struct DebugNode;

#[async_trait]
impl WorkflowNode for DebugNode {
    fn descriptor(&self) -> crate::automation::nodes::NodeTypeDescriptor {
        descriptor::<DebugConfig>(
            "debug",
            1,
            NodeCategory::Flow,
            "Debug",
            "Displays a message in the debug sidebar and/or the run log.",
            "bug_report",
            Some(input_port()),
            Vec::new(),
            Vec::new(),
            "A sink node for inspecting messages. Choose the whole message, its payload, or an expression; optionally also write the value to the run log.\n\nIt never changes the message or the run status.",
        )
    }

    async fn execute(&self, ctx: NodeContext<'_>) -> Result<NodeOutput, NodeExecutionError> {
        let config: DebugConfig = serde_json::from_value(ctx.config.clone())
            .map_err(|error| NodeExecutionError::new("node.config_invalid", error.to_string()))?;
        let value = match config.target.as_str() {
            "payload" => ctx.msg.get("payload").cloned().unwrap_or(Value::Null),
            "msg" => ctx.msg.clone(),
            "expression" => match config.expression.as_ref() {
                Some(expression) => ctx.resolve(expression)?,
                None => Value::Null,
            },
            _ => ctx.msg.get("payload").cloned().unwrap_or(Value::Null),
        };
        if config.to_log {
            ctx.logger
                .log("debug", "debug node output", Some(value.clone()));
        }
        if config.to_sidebar {
            ctx.logger.log("debug", "debug", Some(value));
        }
        Ok(NodeOutput::empty())
    }
}
