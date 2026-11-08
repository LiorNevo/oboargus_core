//! Inject node: emits a configured message.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::automation::error::NodeExecutionError;
use crate::automation::nodes::common::{descriptor, output_port};
use crate::automation::nodes::{NodeCategory, NodeContext, NodeOutput, WorkflowNode};

/// Inject configuration.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default)]
pub struct InjectConfig {
    /// `none`, `string`, `number`, `boolean`, `json`, `timestamp`.
    pub payload_type: String,
    /// Payload literal or expression object.
    pub payload: Value,
    /// `none`, `interval`, `cron`.
    pub repeat: String,
    /// Interval seconds.
    pub interval_seconds: Option<u64>,
    /// Cron expression.
    pub cron_expression: Option<String>,
    /// IANA timezone.
    pub timezone: String,
    /// Fire once at run start.
    pub once: bool,
    /// Enabled state.
    pub enabled: bool,
}

impl Default for InjectConfig {
    fn default() -> Self {
        Self {
            payload_type: "none".to_string(),
            payload: Value::Null,
            repeat: "none".to_string(),
            interval_seconds: None,
            cron_expression: None,
            timezone: "UTC".to_string(),
            once: true,
            enabled: true,
        }
    }
}

/// The inject node.
pub struct InjectNode;

#[async_trait]
impl WorkflowNode for InjectNode {
    fn descriptor(&self) -> crate::automation::nodes::NodeTypeDescriptor {
        descriptor::<InjectConfig>(
            "inject",
            1,
            NodeCategory::Trigger,
            "Inject",
            "Emits a configured message manually, on an interval, or on a cron schedule.",
            "bolt",
            None,
            vec![output_port()],
            Vec::new(),
            "Starts a flow with a configured payload. Repeating injects are reconciled into durable triggers on publish.",
        )
    }

    async fn execute(&self, ctx: NodeContext<'_>) -> Result<NodeOutput, NodeExecutionError> {
        let config: InjectConfig = serde_json::from_value(ctx.config.clone())
            .map_err(|error| NodeExecutionError::new("node.config_invalid", error.to_string()))?;
        if !config.enabled {
            return Ok(NodeOutput::empty());
        }
        let payload = match config.payload_type.as_str() {
            "none" => Value::Null,
            "timestamp" => Value::String(ctx.run.started_at.to_rfc3339()),
            "json" | "string" | "number" | "boolean" => ctx.resolve(&config.payload)?,
            _ => ctx.resolve(&config.payload)?,
        };
        let mut message = json!({ "payload": payload });
        if let Value::Object(map) = &mut message {
            map.insert(
                "_msgid".to_string(),
                Value::String(uuid::Uuid::new_v4().to_string()),
            );
        }
        let mut output = NodeOutput::empty();
        output.push("out", message);
        Ok(output)
    }
}
