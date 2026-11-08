//! Switch node: routes a message to one or more outputs.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::automation::error::NodeExecutionError;
use crate::automation::nodes::common::{descriptor, input_port, named_output, output_port};
use crate::automation::nodes::{NodeCategory, NodeContext, NodeOutput, WorkflowNode};

/// One switch rule.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SwitchRule {
    /// `eq`, `neq`, `lt`, `lte`, `gt`, `gte`, `contains`, `regex`,
    /// `is_null`, `is_not_null`.
    pub operator: String,
    /// Comparison value.
    #[serde(default)]
    pub value: Value,
    /// Output port name.
    pub output: String,
}

/// Switch configuration.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default)]
pub struct SwitchConfig {
    /// Property path or expression object.
    pub property: Value,
    /// Ordered rules.
    pub rules: Vec<SwitchRule>,
    /// Default output when no rule matches.
    pub default_output: Option<String>,
    /// Send to every matching output.
    pub check_all: bool,
}

impl Default for SwitchConfig {
    fn default() -> Self {
        Self {
            property: Value::String("msg.payload".to_string()),
            rules: Vec::new(),
            default_output: Some("default".to_string()),
            check_all: false,
        }
    }
}

/// The switch node.
pub struct SwitchNode;

#[async_trait]
impl WorkflowNode for SwitchNode {
    fn descriptor(&self) -> crate::automation::nodes::NodeTypeDescriptor {
        let mut outputs = vec![output_port()];
        outputs.push(named_output("default", "Default"));
        descriptor::<SwitchConfig>(
            "switch",
            1,
            NodeCategory::Flow,
            "Switch",
            "Routes a message to one or more outputs according to ordered rules.",
            "call_split",
            Some(input_port()),
            outputs,
            Vec::new(),
            "Rules are evaluated in order. `check_all` allows a message to reach multiple matching outputs.",
        )
    }

    async fn execute(&self, ctx: NodeContext<'_>) -> Result<NodeOutput, NodeExecutionError> {
        let config: SwitchConfig = serde_json::from_value(ctx.config.clone())
            .map_err(|error| NodeExecutionError::new("node.config_invalid", error.to_string()))?;
        let subject = match &config.property {
            Value::String(path) => {
                crate::automation::expression::eval_str(path, &ctx.expression_context())
                    .map_err(|error| NodeExecutionError::new(error.code, error.message))?
            }
            value => ctx.resolve(value)?,
        };
        let mut output = NodeOutput::empty();
        let mut matched = false;
        for rule in &config.rules {
            let expected = ctx.resolve(&rule.value)?;
            if matches(&subject, &rule.operator, &expected) {
                output.push(&rule.output, ctx.msg.clone());
                matched = true;
                if !config.check_all {
                    break;
                }
            }
        }
        if !matched && let Some(port) = &config.default_output {
            output.push(port, ctx.msg.clone());
        }
        Ok(output)
    }
}

fn matches(subject: &Value, operator: &str, expected: &Value) -> bool {
    match operator {
        "eq" => subject == expected,
        "neq" => subject != expected,
        "lt" => compare(subject, expected).is_some_and(|value| value < 0),
        "lte" => compare(subject, expected).is_some_and(|value| value <= 0),
        "gt" => compare(subject, expected).is_some_and(|value| value > 0),
        "gte" => compare(subject, expected).is_some_and(|value| value >= 0),
        "contains" => match subject {
            Value::String(text) => expected
                .as_str()
                .is_some_and(|needle| text.contains(needle)),
            Value::Array(items) => items.contains(expected),
            Value::Object(object) => expected
                .as_str()
                .is_some_and(|key| object.contains_key(key)),
            _ => false,
        },
        "regex" => match (subject.as_str(), expected.as_str()) {
            (Some(text), Some(pattern)) => regex::Regex::new(pattern)
                .map(|regex| regex.is_match(text))
                .unwrap_or(false),
            _ => false,
        },
        "is_null" => subject.is_null(),
        "is_not_null" => !subject.is_null(),
        _ => false,
    }
}

fn compare(left: &Value, right: &Value) -> Option<i32> {
    if let (Some(left), Some(right)) = (left.as_f64(), right.as_f64()) {
        return Some(if left < right {
            -1
        } else if left > right {
            1
        } else {
            0
        });
    }
    match (left.as_str(), right.as_str()) {
        (Some(left), Some(right)) => Some(if left < right {
            -1
        } else if left > right {
            1
        } else {
            0
        }),
        _ => None,
    }
}
