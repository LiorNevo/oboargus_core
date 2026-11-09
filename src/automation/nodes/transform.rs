//! Transform node: evaluates an lx_engine expression or a JSON mapping.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::automation::error::NodeExecutionError;
use crate::automation::nodes::common::{descriptor, input_port, output_port, with_payload};
use crate::automation::nodes::{NodeCategory, NodeContext, NodeOutput, WorkflowNode};

/// Transform configuration.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(default)]
pub struct TransformConfig {
    /// lx_engine rule, path, or template.
    pub expression: Option<Value>,
    /// JSON mapping whose values are literals or expression objects.
    pub mapping: Option<Value>,
}

/// The transform node.
pub struct TransformNode;

#[async_trait]
impl WorkflowNode for TransformNode {
    fn descriptor(&self) -> crate::automation::nodes::NodeTypeDescriptor {
        descriptor::<TransformConfig>(
            "transform",
            1,
            NodeCategory::Transform,
            "Transform",
            "Builds a new payload with an lx_engine expression or a JSON mapping.",
            "transform",
            Some(input_port()),
            vec![output_port()],
            vec!["read:context".to_string()],
            "Builds a new message payload from an lx_engine expression or a JSON mapping.\n\nMapping values may use ${...} templates or expression objects. If neither is configured, the incoming payload is passed through.",
        )
    }

    async fn execute(&self, ctx: NodeContext<'_>) -> Result<NodeOutput, NodeExecutionError> {
        let config: TransformConfig = serde_json::from_value(ctx.config.clone())
            .map_err(|error| NodeExecutionError::new("node.config_invalid", error.to_string()))?;
        let result = if let Some(expression) = &config.expression {
            ctx.resolve(expression)?
        } else if let Some(mapping) = &config.mapping {
            resolve_mapping(mapping, &ctx)?
        } else {
            ctx.msg.get("payload").cloned().unwrap_or(Value::Null)
        };
        let mut output = NodeOutput::empty();
        output.push("out", with_payload(ctx.msg.clone(), result));
        Ok(output)
    }
}

fn resolve_mapping(value: &Value, ctx: &NodeContext<'_>) -> Result<Value, NodeExecutionError> {
    match value {
        Value::Object(object) if object.contains_key("kind") => ctx.resolve(value),
        Value::Object(object) => {
            let mut mapped = Map::new();
            for (key, value) in object {
                mapped.insert(key.clone(), resolve_mapping(value, ctx)?);
            }
            Ok(Value::Object(mapped))
        }
        Value::Array(items) => items
            .iter()
            .map(|item| resolve_mapping(item, ctx))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::String(text) if text.contains("${") => {
            crate::automation::expression::eval_str(text, &ctx.expression_context())
                .map_err(Into::into)
        }
        other => Ok(other.clone()),
    }
}
