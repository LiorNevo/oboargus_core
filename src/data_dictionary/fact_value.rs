//! Reusable fact-value shapes and the tagged `FactValue` union.

use serde_json::{Map, Value, json};

use crate::data_dictionary::error::DataDictionaryError;
use crate::data_dictionary::types;
use crate::temporal::catalog::{FactFieldDef, FactTypeDef, StorageKind};

/// Builds the reusable fact-value definitions.
pub fn build_value_definitions(
    catalog: &[FactTypeDef],
) -> Result<Map<String, Value>, DataDictionaryError> {
    let mut definitions = Map::new();
    for (name, schema) in scalar_shapes() {
        definitions.insert(name.to_string(), schema);
    }
    definitions.insert(
        "FactValue.market_price".to_string(),
        market_price_shape(catalog)?,
    );
    definitions.insert("FactValue".to_string(), value_union());
    Ok(definitions)
}

fn scalar_shapes() -> Vec<(&'static str, Value)> {
    vec![
        ("FactValue.text", json!({ "type": "string" })),
        (
            "FactValue.decimal",
            json!({ "type": "string", "pattern": types::DECIMAL_38_18_PATTERN }),
        ),
        (
            "FactValue.integer",
            json!({
                "type": "string",
                "format": "int64",
                "pattern": types::INT64_PATTERN
            }),
        ),
        ("FactValue.boolean", json!({ "type": "boolean" })),
        (
            "FactValue.date",
            json!({ "type": "string", "format": "date" }),
        ),
        (
            "FactValue.timestamp",
            json!({ "type": "string", "format": "date-time" }),
        ),
    ]
}

fn market_price_shape(catalog: &[FactTypeDef]) -> Result<Value, DataDictionaryError> {
    let definition = catalog
        .iter()
        .find(|entry| entry.storage_kind == StorageKind::MarketObservation)
        .ok_or(DataDictionaryError::EmptyCatalog)?;
    let mut properties = Map::new();
    let mut required = Vec::new();
    for field in definition.fields {
        properties.insert(field.name.to_string(), field_schema(field));
        if !field.nullable {
            required.push(field.name.to_string());
        }
    }
    required.sort();
    Ok(json!({
        "type": "object",
        "description": "Typed market-price observation payload.",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    }))
}

fn field_schema(field: &FactFieldDef) -> Value {
    let mut schema = types::schema_for_data_type(field.data_type, field.scale);
    if let Some(object) = schema.as_object_mut() {
        if field.name == "currency" {
            object.insert(
                "pattern".to_string(),
                Value::String(types::CURRENCY_PATTERN.to_string()),
            );
        }
        if let Some(unit) = field.unit {
            object.insert("x-obolargus-unit".to_string(), json!(unit));
        }
        if let Some(precision) = field.precision {
            object.insert("x-obolargus-precision".to_string(), json!(precision));
        }
        if let Some(scale) = field.scale {
            object.insert("x-obolargus-scale".to_string(), json!(scale));
        }
        if let Some(minimum) = field.min_value {
            object.insert("x-obolargus-minimum".to_string(), json!(minimum));
        }
        if let Some(maximum) = field.max_value {
            object.insert("x-obolargus-maximum".to_string(), json!(maximum));
        }
        if !field.allowed_values.is_empty() {
            object.insert(
                "enum".to_string(),
                Value::Array(
                    field
                        .allowed_values
                        .iter()
                        .map(|value| json!(value))
                        .collect(),
                ),
            );
        }
        object.insert(
            "x-obolargus-aggregates".to_string(),
            Value::Array(field.aggregates.iter().map(|value| json!(value)).collect()),
        );
        object.insert(
            "description".to_string(),
            Value::String(field.description.to_string()),
        );
    }
    if field.nullable {
        types::make_nullable(&mut schema);
    }
    schema
}

fn value_union() -> Value {
    let variants = [
        ("text", "text"),
        ("decimal", "decimal"),
        ("integer", "integer"),
        ("boolean", "boolean"),
        ("date", "date"),
        ("timestamp", "timestamp"),
        ("market_price", "market_price"),
    ];
    let one_of: Vec<Value> = variants
        .iter()
        .map(|(tag, shape)| {
            json!({
                "type": "object",
                "properties": {
                    "type": { "const": tag },
                    "value": { "$ref": format!("#/$defs/FactValue.{shape}") }
                },
                "required": ["type", "value"],
                "additionalProperties": false
            })
        })
        .collect();
    json!({ "oneOf": one_of })
}
