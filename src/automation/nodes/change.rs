//! Change node: mutates message properties and run/flow/global context.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::automation::error::NodeExecutionError;
use crate::automation::nodes::common::{descriptor, input_port, output_port};
use crate::automation::nodes::{NodeCategory, NodeContext, NodeOutput, WorkflowNode};
use crate::automation::runtime::context::ContextScope;

/// One change rule.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ChangeRule {
    /// `set`, `change`, or `delete`.
    pub action: String,
    /// `msg`, `flow`, `global`, or `run`.
    pub target: String,
    /// Dot path for `msg`.
    #[serde(default)]
    pub path: Option<String>,
    /// Key for context targets.
    #[serde(default)]
    pub key: Option<String>,
    /// Value literal or expression object.
    #[serde(default)]
    pub value: Option<Value>,
    /// Replacement source for `change`.
    #[serde(default)]
    pub from: Option<String>,
    /// Replacement target for `change`.
    #[serde(default)]
    pub to: Option<String>,
}

/// Change configuration.
#[derive(Debug, Clone, Deserialize, JsonSchema, Default)]
#[serde(default)]
pub struct ChangeConfig {
    /// Ordered rules.
    pub rules: Vec<ChangeRule>,
}

/// The change node.
pub struct ChangeNode;

#[async_trait]
impl WorkflowNode for ChangeNode {
    fn descriptor(&self) -> crate::automation::nodes::NodeTypeDescriptor {
        descriptor::<ChangeConfig>(
            "change",
            1,
            NodeCategory::Transform,
            "Change",
            "Sets, changes, or deletes message properties and run/flow/global context values.",
            "edit",
            Some(input_port()),
            vec![output_port()],
            vec!["read:context".to_string(), "write:context".to_string()],
            "Context writes are atomic per key and last-write-wins under concurrent runs.",
        )
    }

    async fn execute(&self, ctx: NodeContext<'_>) -> Result<NodeOutput, NodeExecutionError> {
        let config: ChangeConfig = serde_json::from_value(ctx.config.clone())
            .map_err(|error| NodeExecutionError::new("node.config_invalid", error.to_string()))?;
        let mut message = ctx.msg.clone();
        for rule in &config.rules {
            match rule.target.as_str() {
                "msg" => {
                    let path = rule.path.as_deref().ok_or_else(|| {
                        NodeExecutionError::new(
                            "node.config_invalid",
                            "change target 'msg' requires a path",
                        )
                    })?;
                    match rule.action.as_str() {
                        "set" => {
                            let value = ctx.resolve(rule.value.as_ref().unwrap_or(&Value::Null))?;
                            set_path(&mut message, path, value);
                        }
                        "change" => {
                            let from = rule.from.as_deref().unwrap_or_default();
                            let to = rule.to.as_deref().unwrap_or_default();
                            change_string(&mut message, path, from, to);
                        }
                        "delete" => delete_path(&mut message, path),
                        _ => {}
                    }
                }
                "flow" | "global" | "run" => {
                    let key = rule.key.as_deref().ok_or_else(|| {
                        NodeExecutionError::new(
                            "node.config_invalid",
                            "context change requires a key",
                        )
                    })?;
                    let scope = ContextScope::parse(rule.target.as_str()).ok_or_else(|| {
                        NodeExecutionError::new(
                            "node.config_invalid",
                            format!("unknown context target '{}'", rule.target),
                        )
                    })?;
                    match rule.action.as_str() {
                        "set" => {
                            let value = ctx.resolve(rule.value.as_ref().unwrap_or(&Value::Null))?;
                            ctx.context.set(scope, key, value)?;
                        }
                        "delete" => ctx.context.delete(scope, key)?,
                        _ => {}
                    }
                }
                other => {
                    return Err(NodeExecutionError::new(
                        "node.config_invalid",
                        format!("unknown change target '{other}'"),
                    ));
                }
            }
        }
        let mut output = NodeOutput::empty();
        output.push("out", message);
        Ok(output)
    }
}

fn set_path(value: &mut Value, path: &str, next: Value) {
    let segments: Vec<&str> = path
        .split('.')
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.is_empty() {
        *value = next;
        return;
    }
    let mut current = value;
    for segment in &segments[..segments.len() - 1] {
        if !current.is_object() {
            *current = Value::Object(serde_json::Map::new());
        }
        let Some(map) = current.as_object_mut() else {
            return;
        };
        current = map
            .entry((*segment).to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
    }
    if let Some(map) = current.as_object_mut() {
        map.insert(segments[segments.len() - 1].to_string(), next);
    }
}

fn delete_path(value: &mut Value, path: &str) {
    let segments: Vec<&str> = path
        .split('.')
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.is_empty() {
        *value = Value::Null;
        return;
    }
    let mut current = value;
    for segment in &segments[..segments.len() - 1] {
        let Some(next) = current.get_mut(*segment) else {
            return;
        };
        current = next;
    }
    if let Some(map) = current.as_object_mut() {
        map.remove(segments[segments.len() - 1]);
    }
}

fn change_string(value: &mut Value, path: &str, from: &str, to: &str) {
    let segments: Vec<&str> = path
        .split('.')
        .filter(|segment| !segment.is_empty())
        .collect();
    let mut current = value;
    for segment in &segments {
        let Some(next) = current.get_mut(*segment) else {
            return;
        };
        current = next;
    }
    if let Some(text) = current.as_str() {
        *current = Value::String(text.replace(from, to));
    }
}
