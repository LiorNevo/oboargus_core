//! Reference validation for the generated schema.
//!
//! The validator covers the JSON Schema subset emitted by this crate plus the
//! named cross-field rules that JSON Schema cannot express.

use std::cmp::Ordering;

use serde_json::Value;

use crate::data_dictionary::document::SystemSchema;
use crate::data_dictionary::error::{DataDictionaryError, ValidationError};
use crate::temporal::catalog::{SubjectKind, fact_catalog};

/// Validates an instance against a named definition and its semantic rules.
pub fn validate_instance(definition: &str, instance: &Value) -> Result<(), Vec<ValidationError>> {
    let schema = SystemSchema::build().map_err(|error| vec![validation_from_error(error)])?;
    let Some(target) = schema
        .document()
        .get("$defs")
        .and_then(|definitions| definitions.get(definition))
    else {
        return Err(vec![ValidationError::new(
            "",
            format!("unknown_definition:{definition}"),
        )]);
    };
    let mut errors = Vec::new();
    validate_schema(target, instance, "", schema.document(), &mut errors);
    validate_semantics(definition, instance, &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn validation_from_error(error: DataDictionaryError) -> ValidationError {
    ValidationError::new("", error.to_string())
}

fn validate_schema(
    schema: &Value,
    instance: &Value,
    pointer: &str,
    root: &Value,
    errors: &mut Vec<ValidationError>,
) {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        if let Some(target) = resolve_ref(root, reference) {
            validate_schema(target, instance, pointer, root, errors);
        } else {
            errors.push(ValidationError::new(pointer, "unresolved_ref"));
        }
        return;
    }

    if let Some(variants) = schema.get("oneOf").and_then(Value::as_array) {
        let matches = variants
            .iter()
            .filter(|variant| {
                let mut trial = Vec::new();
                validate_schema(variant, instance, pointer, root, &mut trial);
                trial.is_empty()
            })
            .count();
        if matches != 1 {
            errors.push(ValidationError::new(pointer, "oneOf"));
        }
        return;
    }

    if let Some(expected) = schema.get("const")
        && instance != expected
    {
        errors.push(ValidationError::new(pointer, "const"));
    }

    if let Some(values) = schema.get("enum").and_then(Value::as_array)
        && !values.contains(instance)
    {
        errors.push(ValidationError::new(pointer, "enum"));
    }

    if let Some(expected) = schema.get("type")
        && !matches_type(expected, instance)
    {
        errors.push(ValidationError::new(pointer, "type"));
        return;
    }

    if instance.is_null() {
        return;
    }

    match instance {
        Value::Object(object) => validate_object(schema, object, pointer, root, errors),
        Value::Array(items) => {
            if let Some(item_schema) = schema.get("items") {
                for (index, item) in items.iter().enumerate() {
                    let child = format!("{pointer}/{index}");
                    validate_schema(item_schema, item, &child, root, errors);
                }
            }
        }
        Value::String(text) => validate_string(schema, text, pointer, errors),
        Value::Number(number) => validate_number(schema, number, pointer, errors),
        _ => {}
    }
}

fn validate_object(
    schema: &Value,
    object: &serde_json::Map<String, Value>,
    pointer: &str,
    root: &Value,
    errors: &mut Vec<ValidationError>,
) {
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for name in required.iter().filter_map(Value::as_str) {
            if !object.contains_key(name) {
                errors.push(ValidationError::new(
                    format!("{pointer}/{name}"),
                    "required",
                ));
            }
        }
    }
    let properties = schema.get("properties").and_then(Value::as_object);
    if schema.get("additionalProperties") == Some(&Value::Bool(false))
        && let Some(properties) = properties
    {
        for key in object.keys() {
            if !properties.contains_key(key) {
                errors.push(ValidationError::new(
                    format!("{pointer}/{key}"),
                    "additionalProperties",
                ));
            }
        }
    }
    if let Some(properties) = properties {
        for (key, value) in object {
            if let Some(property_schema) = properties.get(key) {
                let child = format!("{pointer}/{key}");
                validate_schema(property_schema, value, &child, root, errors);
            }
        }
    }
}

fn validate_string(schema: &Value, text: &str, pointer: &str, errors: &mut Vec<ValidationError>) {
    if let Some(minimum) = schema.get("minLength").and_then(Value::as_u64)
        && text.chars().count() < minimum as usize
    {
        errors.push(ValidationError::new(pointer, "minLength"));
    }
    if let Some(maximum) = schema.get("maxLength").and_then(Value::as_u64)
        && text.chars().count() > maximum as usize
    {
        errors.push(ValidationError::new(pointer, "maxLength"));
    }
    if let Some(format) = schema.get("format").and_then(Value::as_str)
        && !matches_format(format, text)
    {
        errors.push(ValidationError::new(pointer, "format"));
    }
    if let Some(pattern) = schema.get("pattern").and_then(Value::as_str)
        && !matches_pattern(pattern, text)
    {
        errors.push(ValidationError::new(pointer, "pattern"));
    }
    if let Some(minimum) = schema.get("x-obolargus-minimum").and_then(Value::as_str)
        && decimal_cmp(text, minimum) == Some(Ordering::Less)
    {
        errors.push(ValidationError::new(pointer, "minimum"));
    }
    if let Some(maximum) = schema.get("x-obolargus-maximum").and_then(Value::as_str)
        && decimal_cmp(text, maximum) == Some(Ordering::Greater)
    {
        errors.push(ValidationError::new(pointer, "maximum"));
    }
}

fn validate_number(
    schema: &Value,
    number: &serde_json::Number,
    pointer: &str,
    errors: &mut Vec<ValidationError>,
) {
    if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64)
        && number.as_f64().is_some_and(|value| value < minimum)
    {
        errors.push(ValidationError::new(pointer, "minimum"));
    }
    if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64)
        && number.as_f64().is_some_and(|value| value > maximum)
    {
        errors.push(ValidationError::new(pointer, "maximum"));
    }
}

fn matches_type(expected: &Value, instance: &Value) -> bool {
    match expected {
        Value::String(name) => type_matches(name, instance),
        Value::Array(names) => names
            .iter()
            .filter_map(Value::as_str)
            .any(|name| type_matches(name, instance)),
        _ => true,
    }
}

fn type_matches(name: &str, instance: &Value) -> bool {
    match name {
        "object" => instance.is_object(),
        "array" => instance.is_array(),
        "string" => instance.is_string(),
        "integer" => instance.as_i64().is_some() || instance.as_u64().is_some(),
        "number" => instance.is_number(),
        "boolean" => instance.is_boolean(),
        "null" => instance.is_null(),
        _ => true,
    }
}

fn matches_format(format: &str, text: &str) -> bool {
    match format {
        "uuid" => uuid::Uuid::parse_str(text).is_ok(),
        "date" => chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").is_ok(),
        "date-time" => chrono::DateTime::parse_from_rfc3339(text).is_ok(),
        "int64" => matches_integer(text),
        _ => true,
    }
}

fn matches_pattern(pattern: &str, text: &str) -> bool {
    match pattern {
        crate::data_dictionary::types::DECIMAL_38_18_PATTERN => matches_decimal(text, 20, 18),
        crate::data_dictionary::types::DECIMAL_38_0_PATTERN => matches_decimal(text, 38, 0),
        crate::data_dictionary::types::INT64_PATTERN => matches_integer(text),
        crate::data_dictionary::types::CURRENCY_PATTERN => {
            text.len() == 3 && text.chars().all(|c| c.is_ascii_uppercase())
        }
        crate::data_dictionary::types::SHA256_PATTERN => {
            text.len() == 64
                && text
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        }
        _ => true,
    }
}

fn matches_integer(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    !digits.is_empty()
        && digits.len() <= 19
        && digits.chars().all(|c| c.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'))
}

fn matches_decimal(text: &str, max_integer_digits: usize, max_fraction_digits: usize) -> bool {
    let unsigned = text.strip_prefix('-').unwrap_or(text);
    let mut parts = unsigned.split('.');
    let integer = parts.next().unwrap_or_default();
    let fraction = parts.next();
    if parts.next().is_some() {
        return false;
    }
    if integer.is_empty() || integer.len() > max_integer_digits {
        return false;
    }
    if !integer.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    if integer.len() > 1 && integer.starts_with('0') {
        return false;
    }
    if let Some(fraction) = fraction
        && (fraction.is_empty()
            || fraction.len() > max_fraction_digits
            || !fraction.chars().all(|c| c.is_ascii_digit()))
    {
        return false;
    }
    true
}

fn resolve_ref<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let name = reference.strip_prefix("#/$defs/")?;
    root.get("$defs")?.get(name)
}

fn validate_semantics(definition: &str, instance: &Value, errors: &mut Vec<ValidationError>) {
    match definition {
        "Event" => validate_event(instance, errors),
        "MarketObservation" => validate_observation(instance, errors),
        "TemporalFact" => validate_temporal_fact(instance, errors),
        "FactType" | "FactField" => validate_catalog_reference(instance, errors),
        _ => {
            if let Some(fact_type) = definition.strip_prefix("FactType.")
                && !catalog_contains(fact_type)
            {
                errors.push(ValidationError::new("/type", "unknown_fact_type"));
            }
        }
    }
}

fn validate_event(instance: &Value, errors: &mut Vec<ValidationError>) {
    let Some(object) = instance.as_object() else {
        return;
    };
    let event_type = object.get("event_type").and_then(Value::as_str);
    let destination = object.get("to_portfolio_id");
    if event_type == Some("transfer") && destination.is_none_or(Value::is_null) {
        errors.push(ValidationError::new(
            "/to_portfolio_id",
            "transfer_requires_destination",
        ));
    }
    if let (Some(destination), Some(source)) = (
        object.get("to_portfolio_id").and_then(Value::as_str),
        object.get("portfolio_id").and_then(Value::as_str),
    ) && destination == source
    {
        errors.push(ValidationError::new(
            "/to_portfolio_id",
            "transfer_not_self",
        ));
    }
    if let Some(quantity) = object.get("quantity_delta").and_then(Value::as_str)
        && decimal_cmp(quantity, "0") == Some(Ordering::Equal)
    {
        errors.push(ValidationError::new(
            "/quantity_delta",
            "quantity_delta_non_zero",
        ));
    }
}

fn validate_observation(instance: &Value, errors: &mut Vec<ValidationError>) {
    let Some(object) = instance.as_object() else {
        return;
    };
    let retracted = object
        .get("retracted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let reason = object.get("retraction_reason");
    let has_reason = reason.is_some_and(|value| !value.is_null());
    if retracted != has_reason {
        errors.push(ValidationError::new(
            "/retraction_reason",
            "retraction_reason_iff_retracted",
        ));
    }
    if retracted {
        return;
    }
    let kind = object.get("kind").and_then(Value::as_str);
    let present = |name: &str| object.get(name).is_some_and(|value| !value.is_null());
    if kind == Some("bar") {
        for field in ["open", "high", "low", "close"] {
            if !present(field) {
                errors.push(ValidationError::new(
                    format!("/{field}"),
                    "market_observation_bar_shape",
                ));
            }
        }
    }
    if kind == Some("quote") && !present("price") {
        errors.push(ValidationError::new(
            "/price",
            "market_observation_quote_shape",
        ));
    }
    if !present("price") && !present("close") {
        errors.push(ValidationError::new(
            "/price",
            "market_observation_has_value",
        ));
    }
    validate_ohlc(object, errors);
    if let Some(volume) = object.get("volume").and_then(Value::as_str)
        && decimal_cmp(volume, "0") == Some(Ordering::Less)
    {
        errors.push(ValidationError::new(
            "/volume",
            "market_observation_ohlc_bounds",
        ));
    }
}

fn validate_ohlc(object: &serde_json::Map<String, Value>, errors: &mut Vec<ValidationError>) {
    let read = |name: &str| object.get(name).and_then(Value::as_str);
    let (Some(high), Some(low)) = (read("high"), read("low")) else {
        return;
    };
    if decimal_cmp(high, low) == Some(Ordering::Less) {
        errors.push(ValidationError::new(
            "/high",
            "market_observation_ohlc_bounds",
        ));
    }
    for field in ["open", "close"] {
        let Some(value) = read(field) else {
            continue;
        };
        if decimal_cmp(value, low) == Some(Ordering::Less)
            || decimal_cmp(value, high) == Some(Ordering::Greater)
        {
            errors.push(ValidationError::new(
                format!("/{field}"),
                "market_observation_ohlc_bounds",
            ));
        }
    }
}

fn validate_temporal_fact(instance: &Value, errors: &mut Vec<ValidationError>) {
    let Some(object) = instance.as_object() else {
        return;
    };
    let retracted = object
        .get("retracted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let has_reason = object
        .get("retraction_reason")
        .is_some_and(|value| !value.is_null());
    if retracted != has_reason {
        errors.push(ValidationError::new(
            "/retraction_reason",
            "retraction_reason_iff_retracted",
        ));
    }
    validate_catalog_reference(instance, errors);
    if let (Some(fact_type), Some(subject_kind)) = (
        object.get("fact_type").and_then(Value::as_str),
        object.get("subject_kind").and_then(Value::as_str),
    ) && let Some(subject) = SubjectKind::from_wire(subject_kind)
    {
        let applies = fact_catalog()
            .iter()
            .find(|entry| entry.fact_type == fact_type)
            .is_some_and(|entry| entry.applies_to(subject));
        if !applies && catalog_contains(fact_type) {
            errors.push(ValidationError::new("/subject_kind", "invalid_subject"));
        }
    }
}

fn validate_catalog_reference(instance: &Value, errors: &mut Vec<ValidationError>) {
    let Some(object) = instance.as_object() else {
        return;
    };
    if let Some(fact_type) = object.get("fact_type").and_then(Value::as_str)
        && !catalog_contains(fact_type)
    {
        errors.push(ValidationError::new("/fact_type", "unknown_fact_type"));
    }
}

fn catalog_contains(fact_type: &str) -> bool {
    fact_catalog()
        .iter()
        .any(|entry| entry.fact_type == fact_type)
}

/// Compares two decimal strings without converting through floating point.
pub fn decimal_cmp(left: &str, right: &str) -> Option<Ordering> {
    let (left_negative, left_integer, left_fraction) = decimal_parts(left)?;
    let (right_negative, right_integer, right_fraction) = decimal_parts(right)?;
    let left_zero = is_zero(&left_integer, &left_fraction);
    let right_zero = is_zero(&right_integer, &right_fraction);
    if left_zero && right_zero {
        return Some(Ordering::Equal);
    }
    if left_negative != right_negative {
        return Some(if left_negative {
            Ordering::Less
        } else {
            Ordering::Greater
        });
    }
    let magnitude = compare_magnitude(
        &left_integer,
        &left_fraction,
        &right_integer,
        &right_fraction,
    );
    Some(if left_negative {
        magnitude.reverse()
    } else {
        magnitude
    })
}

fn decimal_parts(value: &str) -> Option<(bool, String, String)> {
    let (negative, unsigned) = match value.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, value),
    };
    let mut parts = unsigned.split('.');
    let integer = parts.next()?;
    let fraction = parts.next().unwrap_or("");
    if parts.next().is_some() {
        return None;
    }
    if integer.is_empty()
        || !integer.chars().all(|c| c.is_ascii_digit())
        || !fraction.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    Some((
        negative,
        integer.trim_start_matches('0').to_string(),
        fraction.trim_end_matches('0').to_string(),
    ))
}

fn is_zero(integer: &str, fraction: &str) -> bool {
    integer.is_empty() && fraction.is_empty()
}

fn compare_magnitude(
    left_integer: &str,
    left_fraction: &str,
    right_integer: &str,
    right_fraction: &str,
) -> Ordering {
    match left_integer.len().cmp(&right_integer.len()) {
        Ordering::Equal => {}
        other => return other,
    }
    match left_integer.cmp(right_integer) {
        Ordering::Equal => {}
        other => return other,
    }
    let width = left_fraction.len().max(right_fraction.len());
    let left = format!("{left_fraction:0<width$}");
    let right = format!("{right_fraction:0<width$}");
    left.cmp(&right)
}
