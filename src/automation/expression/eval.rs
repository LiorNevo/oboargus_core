//! lx_engine-backed expression adapter.
//!
//! Flow expressions are lx_engine rules (`LxRule` JSON). A small path
//! shorthand (`msg.payload.report.loaded`, `flow.last_date`, ...) is compiled
//! into nested `get` rules so node configuration stays readable. Template
//! strings interpolate `${...}` placeholders, each evaluated by lx_engine.
//!
//! This keeps one evaluation engine for automation and financial rules while
//! preserving a low-code authoring surface.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::automation::error::NodeExecutionError;
use crate::automation::runtime::bus::RunMetadata;
use crate::automation::runtime::context::{ContextScope, ContextStore};

/// Expression parse/evaluation error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpressionError {
    /// Stable code.
    pub code: &'static str,
    /// Human-readable message.
    pub message: String,
}

impl ExpressionError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ExpressionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ExpressionError {}

impl From<ExpressionError> for NodeExecutionError {
    fn from(value: ExpressionError) -> Self {
        NodeExecutionError::new(value.code, value.message)
    }
}

/// Evaluation context.
pub struct EvalContext<'a> {
    /// Input message.
    pub msg: &'a Value,
    /// Runtime inputs.
    pub inputs: &'a Value,
    /// Run metadata.
    pub run: &'a RunMetadata,
    /// Run/flow/global context.
    pub context: &'a ContextStore,
}

/// Evaluates an expression source.
///
/// The source is either:
/// - an lx_engine rule JSON document (`{"get": ...}`, `{">": [...]}`, ...),
/// - a path shorthand (`msg.payload.report.loaded`), or
/// - a string template (`"Loaded ${msg.payload.loaded} rows"`).
pub fn eval_str(source: &str, ctx: &EvalContext<'_>) -> Result<Value, ExpressionError> {
    if source.len() > 4096 {
        return Err(ExpressionError::new(
            "expression.limit_exceeded",
            "expression source exceeds 4 KiB",
        ));
    }
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return Err(ExpressionError::new(
            "expression.parse_error",
            "empty expression",
        ));
    }
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        let rule: Value = serde_json::from_str(trimmed)
            .map_err(|error| ExpressionError::new("expression.parse_error", error.to_string()))?;
        return eval_rule(&rule, ctx);
    }
    if trimmed.contains("${") {
        return render_template(trimmed, ctx);
    }
    let rule = path_rule(trimmed)?;
    eval_rule(&rule, ctx)
}

/// Resolves a configuration value that may be a literal, reference, expression,
/// or template object.
pub fn resolve_value(value: &Value, ctx: &EvalContext<'_>) -> Result<Value, NodeExecutionError> {
    let Some(object) = value.as_object() else {
        return Ok(value.clone());
    };
    let Some(kind) = object.get("kind").and_then(Value::as_str) else {
        return Ok(value.clone());
    };
    match kind {
        "literal" => Ok(object.get("value").cloned().unwrap_or(Value::Null)),
        "reference" => {
            let path = object.get("path").and_then(Value::as_str).ok_or_else(|| {
                NodeExecutionError::new(
                    "node.expression_invalid",
                    "reference expression requires a 'path' string",
                )
            })?;
            eval_str(path, ctx).map_err(Into::into)
        }
        "expression" => {
            let expression = object.get("expression").ok_or_else(|| {
                NodeExecutionError::new(
                    "node.expression_invalid",
                    "expression object requires an 'expression' value",
                )
            })?;
            match expression {
                Value::String(source) => eval_str(source, ctx).map_err(Into::into),
                rule => eval_rule(rule, ctx).map_err(Into::into),
            }
        }
        "template" => {
            let template = object
                .get("template")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    NodeExecutionError::new(
                        "node.expression_invalid",
                        "template object requires a 'template' string",
                    )
                })?;
            render_template(template, ctx).map_err(Into::into)
        }
        _ => Ok(value.clone()),
    }
}

/// Evaluates an lx_engine rule against the run context.
pub fn eval_rule(rule: &Value, ctx: &EvalContext<'_>) -> Result<Value, ExpressionError> {
    let input_data = json!({
        "msg": ctx.msg,
        "inputs": ctx.inputs,
        "run": run_value(ctx.run),
        "flow": map_value(ctx.context.snapshot(ContextScope::Flow)),
        "global": map_value(ctx.context.snapshot(ContextScope::Global)),
    });
    let inputs = json!({
        "rules": rule,
        "input_data": input_data,
        "timeout": 2,
        "reference_date": ctx.run.started_at.date_naive().to_string(),
    });
    let lx_inputs: lx_engine::input::inputs::LxInputs = serde_json::from_value(inputs)
        .map_err(|error| ExpressionError::new("expression.parse_error", error.to_string()))?;
    let outputs = lx_engine::run_rules(lx_inputs);
    let value = serde_json::to_value(&outputs)
        .map_err(|error| ExpressionError::new("expression.parse_error", error.to_string()))?;
    let value = unwrap_result(value);
    if value == Value::String("timeout".to_string()) {
        return Err(ExpressionError::new(
            "expression.limit_exceeded",
            "lx_engine evaluation timed out",
        ));
    }
    Ok(value)
}

fn unwrap_result(value: Value) -> Value {
    match value {
        Value::Object(mut object) if object.len() == 1 && object.contains_key("result") => {
            object.remove("result").unwrap_or(Value::Null)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(unwrap_result).collect()),
        other => other,
    }
}

fn render_template(source: &str, ctx: &EvalContext<'_>) -> Result<Value, ExpressionError> {
    let mut output = String::new();
    let mut chars = source.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        if ch == '\\' {
            if let Some((_, '$')) = chars.peek().copied() {
                chars.next();
                if let Some((_, '{')) = chars.peek().copied() {
                    output.push_str("${");
                    chars.next();
                    continue;
                }
            }
            output.push(ch);
            continue;
        }
        if ch == '$'
            && let Some((_, '{')) = chars.peek().copied()
        {
            chars.next();
            let mut inner = String::new();
            let mut closed = false;
            for (_, next) in chars.by_ref() {
                if next == '}' {
                    closed = true;
                    break;
                }
                inner.push(next);
            }
            if !closed {
                return Err(ExpressionError::new(
                    "expression.parse_error",
                    "unterminated template placeholder",
                ));
            }
            let value = eval_str(inner.trim(), ctx)?;
            output.push_str(&to_string_value(&value));
            continue;
        }
        output.push(ch);
    }
    Ok(Value::String(output))
}

/// Compiles a path shorthand into nested lx_engine `get` rules.
fn path_rule(path: &str) -> Result<Value, ExpressionError> {
    let segments = parse_path(path)?;
    let mut rule = json!({ "get": { "selector": segments[0] } });
    for segment in segments.iter().skip(1) {
        rule = json!({
            "get": {
                "selector": segment,
                "from": rule
            }
        });
    }
    Ok(rule)
}

fn parse_path(path: &str) -> Result<Vec<Value>, ExpressionError> {
    let mut segments = Vec::new();
    let mut chars = path.chars().peekable();
    let mut current = String::new();
    let mut root = true;
    while let Some(ch) = chars.next() {
        match ch {
            '.' => {
                if current.is_empty() {
                    return Err(ExpressionError::new(
                        "expression.parse_error",
                        "empty path segment",
                    ));
                }
                segments.push(Value::String(current.clone()));
                current.clear();
                root = false;
            }
            '[' => {
                if !current.is_empty() {
                    segments.push(Value::String(current.clone()));
                    current.clear();
                }
                let mut index = String::new();
                let mut closed = false;
                for next in chars.by_ref() {
                    if next == ']' {
                        closed = true;
                        break;
                    }
                    index.push(next);
                }
                if !closed {
                    return Err(ExpressionError::new(
                        "expression.parse_error",
                        "unterminated path index",
                    ));
                }
                let index = index.trim();
                if index.starts_with('"') && index.ends_with('"') {
                    segments.push(Value::String(index.trim_matches('"').to_string()));
                } else if let Ok(number) = index.parse::<i64>() {
                    segments.push(Value::Number(number.into()));
                } else {
                    return Err(ExpressionError::new(
                        "expression.parse_error",
                        "path index must be a string or integer",
                    ));
                }
                root = false;
            }
            _ => {
                if root && !(ch.is_ascii_alphabetic() || ch == '_') {
                    return Err(ExpressionError::new(
                        "expression.parse_error",
                        "path must start with an identifier",
                    ));
                }
                current.push(ch);
            }
        }
    }
    if !current.is_empty() {
        segments.push(Value::String(current));
    }
    if segments.is_empty() {
        return Err(ExpressionError::new("expression.parse_error", "empty path"));
    }
    Ok(segments)
}

fn run_value(run: &RunMetadata) -> Value {
    let mut map = Map::new();
    map.insert("id".to_string(), Value::String(run.id.to_string()));
    map.insert(
        "flow_id".to_string(),
        Value::String(run.flow_id.to_string()),
    );
    map.insert(
        "flow_version_id".to_string(),
        Value::String(run.flow_version_id.to_string()),
    );
    map.insert(
        "trigger_kind".to_string(),
        Value::String(run.trigger_kind.clone()),
    );
    map.insert(
        "trigger_node_id".to_string(),
        run.trigger_node_id
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    map.insert(
        "started_at".to_string(),
        Value::String(run.started_at.to_rfc3339()),
    );
    map.insert(
        "actor_id".to_string(),
        run.actor_id
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    Value::Object(map)
}

fn map_value(map: BTreeMap<String, Value>) -> Value {
    Value::Object(map.into_iter().collect())
}

fn to_string_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
