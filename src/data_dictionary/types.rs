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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_domain_maps_to_a_schema() {
        assert_eq!(schema_for_domain("uuid")["format"], "uuid");
        assert_eq!(
            schema_for_domain("decimal")["pattern"],
            DECIMAL_38_18_PATTERN
        );
        assert_eq!(schema_for_domain("integer")["type"], "integer");
        assert_eq!(schema_for_domain("boolean")["type"], "boolean");
        assert_eq!(schema_for_domain("date")["format"], "date");
        assert_eq!(schema_for_domain("timestamp")["format"], "date-time");
        assert_eq!(schema_for_domain("text[]")["type"], "array");
        assert_eq!(schema_for_domain("unknown")["type"], "string");
    }

    #[test]
    fn data_types_choose_the_right_pattern() {
        assert_eq!(
            schema_for_data_type("decimal", Some(0))["pattern"],
            DECIMAL_38_0_PATTERN
        );
        assert_eq!(
            schema_for_data_type("decimal", Some(18))["pattern"],
            DECIMAL_38_18_PATTERN
        );
        assert_eq!(
            schema_for_data_type("decimal", None)["pattern"],
            DECIMAL_38_18_PATTERN
        );
        assert_eq!(schema_for_data_type("integer", None)["format"], "int64");
        assert_eq!(
            schema_for_data_type("integer", None)["pattern"],
            INT64_PATTERN
        );
        assert_eq!(schema_for_data_type("boolean", None)["type"], "boolean");
        assert_eq!(schema_for_data_type("date", None)["format"], "date");
        assert_eq!(
            schema_for_data_type("timestamp", None)["format"],
            "date-time"
        );
        assert_eq!(schema_for_data_type("text", None)["type"], "string");
    }

    #[test]
    fn constraints_are_applied_to_scalar_schemas() {
        use crate::domain::model_description::FieldConstraint;
        let mut schema = json!({"type": "string"});
        apply_constraints(
            &mut schema,
            &[
                FieldConstraint::Format { format: "date" },
                FieldConstraint::Pattern { pattern: "^a$" },
                FieldConstraint::MinLength { value: 1 },
                FieldConstraint::MaxLength { value: 5 },
                FieldConstraint::AllowedValues {
                    values: &["a", "b"],
                },
                FieldConstraint::KnownValues {
                    values: &["x", "y"],
                },
                FieldConstraint::CrossField { rule: "some_rule" },
            ],
        );
        assert_eq!(schema["format"], "date");
        assert_eq!(schema["pattern"], "^a$");
        assert_eq!(schema["minLength"], 1);
        assert_eq!(schema["maxLength"], 5);
        assert_eq!(schema["enum"], json!(["a", "b"]));
        assert_eq!(schema["x-obolargus-known-values"], json!(["x", "y"]));
        assert_eq!(
            schema["x-obolargus-cross-field-constraints"],
            json!(["some_rule"])
        );

        apply_constraints(
            &mut schema,
            &[FieldConstraint::CrossField {
                rule: "second_rule",
            }],
        );
        assert_eq!(
            schema["x-obolargus-cross-field-constraints"],
            json!(["some_rule", "second_rule"])
        );
    }

    #[test]
    fn constraints_handle_arrays_bounds_and_non_objects() {
        use crate::domain::model_description::FieldConstraint;
        let mut array = json!({"type": "array"});
        apply_constraints(
            &mut array,
            &[FieldConstraint::AllowedValues { values: &["a"] }],
        );
        assert_eq!(array["items"], json!({"type": "string", "enum": ["a"]}));

        let mut array = json!({"type": "array", "items": {"type": "string"}});
        apply_constraints(
            &mut array,
            &[FieldConstraint::AllowedValues { values: &["b"] }],
        );
        assert_eq!(array["items"]["enum"], json!(["b"]));

        let mut numeric = json!({"type": "integer"});
        apply_constraints(
            &mut numeric,
            &[
                FieldConstraint::Minimum { value: "1" },
                FieldConstraint::Maximum { value: "5" },
            ],
        );
        assert_eq!(numeric["minimum"], 1);
        assert_eq!(numeric["maximum"], 5);

        let mut decimal = json!({"type": "string"});
        apply_constraints(
            &mut decimal,
            &[
                FieldConstraint::Minimum { value: "1.5" },
                FieldConstraint::Maximum { value: "5.5" },
            ],
        );
        assert_eq!(decimal["x-obolargus-minimum"], "1.5");
        assert_eq!(decimal["x-obolargus-maximum"], "5.5");

        let mut integer_with_bad_bound = json!({"type": "integer"});
        apply_constraints(
            &mut integer_with_bad_bound,
            &[FieldConstraint::Minimum {
                value: "not-a-number",
            }],
        );
        assert_eq!(
            integer_with_bad_bound["x-obolargus-minimum"],
            "not-a-number"
        );

        let mut non_object = json!("not a schema");
        apply_constraints(&mut non_object, &[FieldConstraint::MinLength { value: 1 }]);
        assert_eq!(non_object, json!("not a schema"));
    }

    #[test]
    fn nullability_widens_every_type_shape() {
        let mut string = json!({"type": "string"});
        make_nullable(&mut string);
        assert_eq!(string["type"], json!(["string", "null"]));

        let mut already_nullable = json!({"type": ["string", "null"]});
        make_nullable(&mut already_nullable);
        assert_eq!(already_nullable["type"], json!(["string", "null"]));

        let mut array = json!({"type": ["string", "integer"]});
        make_nullable(&mut array);
        assert_eq!(array["type"], json!(["string", "integer", "null"]));

        let mut null_only = json!({"type": "null"});
        make_nullable(&mut null_only);
        assert_eq!(null_only["type"], "null");

        let mut object_type = json!({"type": {}});
        make_nullable(&mut object_type);
        assert_eq!(object_type["type"], json!({}));

        let mut no_type = json!({"format": "uuid"});
        make_nullable(&mut no_type);
        assert_eq!(no_type["format"], "uuid");

        let mut non_object = json!("x");
        make_nullable(&mut non_object);
        assert_eq!(non_object, json!("x"));
    }
}
