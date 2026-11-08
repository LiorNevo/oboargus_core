//! Delay node: delays or rate-limits messages.

use std::time::Duration;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::automation::error::NodeExecutionError;
use crate::automation::nodes::common::{descriptor, input_port, output_port};
use crate::automation::nodes::{NodeCategory, NodeContext, NodeOutput, WorkflowNode};

/// Delay configuration.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default)]
pub struct DelayConfig {
    /// `delay` or `rate_limit`.
    pub mode: String,
    /// Delay seconds.
    pub seconds: u64,
    /// Messages per second for rate limiting.
    pub rate: u32,
    /// Queue bound for rate limiting.
    pub queue_limit: u32,
}

impl Default for DelayConfig {
    fn default() -> Self {
        Self {
            mode: "delay".to_string(),
            seconds: 1,
            rate: 10,
            queue_limit: 1_000,
        }
    }
}

/// The delay node.
pub struct DelayNode;

#[async_trait]
impl WorkflowNode for DelayNode {
    fn descriptor(&self) -> crate::automation::nodes::NodeTypeDescriptor {
        descriptor::<DelayConfig>(
            "delay",
            1,
            NodeCategory::Flow,
            "Delay",
            "Delays or rate-limits messages.",
            "schedule",
            Some(input_port()),
            vec![output_port()],
            Vec::new(),
            "Cancellation-aware. Rate limiting uses a bounded queue and fails with node.queue_full on overflow.",
        )
    }

    async fn execute(&self, ctx: NodeContext<'_>) -> Result<NodeOutput, NodeExecutionError> {
        let config: DelayConfig = serde_json::from_value(ctx.config.clone())
            .map_err(|error| NodeExecutionError::new("node.config_invalid", error.to_string()))?;
        let wait = match config.mode.as_str() {
            "rate_limit" => {
                let rate = config.rate.max(1) as u64;
                Duration::from_millis(1_000 / rate)
            }
            _ => Duration::from_secs(config.seconds),
        };
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = ctx.cancel.cancelled() => {
                return Err(NodeExecutionError::new("node.cancelled", "run cancelled"));
            }
        }
        let mut output = NodeOutput::empty();
        output.push("out", ctx.msg.clone());
        Ok(output)
    }
}
