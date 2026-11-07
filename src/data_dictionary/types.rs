//! Mapping from domain and catalog types to JSON Schema keywords.

use serde_json::{Map, Value, json};

use crate::domain::model_description::FieldConstraint;

/// Exact decimal string pattern for `NUMERIC(38,18)`.
pub const DECIMAL_38_18_PATTERN: &str = r"^-?(?:0|[1-9][0-9]{0,19})(?:\.[0-9]{1,18})?$";
/// Exact decimal string pattern for `NUMERIC(38,0)`.
pub const DECIMAL_38_0_PATTERN: &str = r"^-?(?:0|[1-9][0-9]{0,37})$";
/// Exact string pattern for a `BIGINT` value.
pub const INT64_PATTERN: &str = r"^-?(?:0|[1-9][0-9]{0,18})$";
/// ISO 4217 currency pattern.
pub const CURRENCY_PATTERN: &str = "^[A-Z]{3}$";
/// Lowercase SHA-256 hex pattern.
pub const SHA256_PATTERN: &str = "^[0-9a-f]{64}$";

/// Returns the base schema for a `FieldDescription.type_domain`.
pub fn schema_for_domain(type_domain: &str) -> Value {
    match type_domain {
        "uuid" => json!({ "type": "string", "format": "uuid" }),
        "decimal" => json!({ "type": "string", "pattern": DECIMAL_38_18_PATTERN }),
        "integer" => json!({ "type": "integer" }),
        "boolean" => json!({ "type": "boolean" }),
        "date" => json!({ "type": "string", "format": "date" }),
        "timestamp" => json!({ "type": "string", "format": "date-time" }),
        "text[]" => json!({ "type": "array", "items": { "type": "string" } }),
        _ => json!({ "type": "string" }),
    }
}

/// Returns the schema for a catalog `FactFieldDef.data_type`.
pub fn schema_for_data_type(data_type: &str, scale: Option<i32>) -> Value {
    match data_type {
        "decimal" => {
            let pattern = if scale == Some(0) {
                DECIMAL_38_0_PATTERN
            } else {
                DECIMAL_38_18_PATTERN
            };
            json!({ "type": "string", "pattern": pattern })
        }
        "integer" => json!({
            "type": "string",
            "format": "int64",
            "pattern": INT64_PATTERN
        }),
        "boolean" => json!({ "type": "boolean" }),
        "date" => json!({ "type": "string", "format": "date" }),
        "timestamp" => json!({ "type": "string", "format": "date-time" }),
        _ => json!({ "type": "string" }),
    }
}

/// Applies domain constraints to a field schema.
pub fn apply_constraints(schema: &mut Value, constraints: &[FieldConstraint]) {
    let Some(object) = schema.as_object_mut() else {
        return;
    };
    for constraint in constraints {
        match constraint {
            FieldConstraint::Format { format } => {
                object.insert("format".to_string(), Value::String((*format).to_string()));
            }
            FieldConstraint::Pattern { pattern } => {
                object.insert("pattern".to_string(), Value::String((*pattern).to_string()));
            }
            FieldConstraint::MinLength { value } => {
                object.insert("minLength".to_string(), json!(value));
            }
            FieldConstraint::MaxLength { value } => {
                object.insert("maxLength".to_string(), json!(value));
            }
            FieldConstraint::Minimum { value } => {
                apply_bound(object, "minimum", value);
            }
            FieldConstraint::Maximum { value } => {
                apply_bound(object, "maximum", value);
            }
            FieldConstraint::AllowedValues { values } => {
                let allowed = Value::Array(values.iter().map(|v| json!(v)).collect());
                if object.get("type").and_then(Value::as_str) == Some("array") {
                    let items = object
                        .entry("items".to_string())
                        .or_insert_with(|| json!({ "type": "string" }));
                    if let Some(items) = items.as_object_mut() {
                        items.insert("enum".to_string(), allowed);
                    }
                } else {
                    object.insert("enum".to_string(), allowed);
                }
            }
            FieldConstraint::KnownValues { values } => {
                object.insert(
                    "x-obolargus-known-values".to_string(),
                    Value::Array(values.iter().map(|v| json!(v)).collect()),
                );
            }
            FieldConstraint::CrossField { rule } => {
                let entry = object
                    .entry("x-obolargus-cross-field-constraints".to_string())
                    .or_insert_with(|| Value::Array(Vec::new()));
                if let Some(array) = entry.as_array_mut() {
                    array.push(json!(rule));
                }
            }
        }
    }
}

fn apply_bound(object: &mut Map<String, Value>, keyword: &str, value: &str) {
    if object.get("type").and_then(Value::as_str) == Some("integer")
        && let Ok(number) = value.parse::<i64>()
    {
        object.insert(keyword.to_string(), json!(number));
        return;
    }
    object.insert(
        format!("x-obolargus-{keyword}"),
        Value::String(value.to_string()),
    );
}

/// Adds `null` to a field schema's type union.
pub fn make_nullable(schema: &mut Value) {
    let Some(object) = schema.as_object_mut() else {
        return;
    };
    let Some(type_value) = object.get_mut("type") else {
        return;
    };
    match type_value {
        Value::String(current) => {
            if current != "null" {
                *type_value = json!([current.clone(), "null"]);
            }
        }
        Value::Array(types) if !types.iter().any(|value| value == "null") => {
            types.push(json!("null"));
        }
        _ => {}
    }
}
