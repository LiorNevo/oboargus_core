//! Document-level semantic metadata.

use serde_json::{Map, Value, json};

use crate::domain::model_description::ModelDescription;
use crate::temporal::catalog::FactTypeDef;

/// Cross-field rules that JSON Schema cannot express structurally.
pub const CROSS_FIELD_RULES: &[(&str, &str, &str)] = &[
    (
        "transfer_requires_destination",
        "Event",
        "A transfer event requires to_portfolio_id.",
    ),
    (
        "transfer_not_self",
        "Event",
        "A transfer destination must differ from the source portfolio.",
    ),
    (
        "quantity_delta_non_zero",
        "Event",
        "An event quantity delta must be non-zero.",
    ),
    (
        "market_observation_ohlc_bounds",
        "MarketObservation",
        "high >= low, and open/close lie within [low, high].",
    ),
    (
        "market_observation_bar_shape",
        "MarketObservation",
        "A non-retracted bar carries open, high, low, and close.",
    ),
    (
        "market_observation_quote_shape",
        "MarketObservation",
        "A non-retracted quote carries price.",
    ),
    (
        "market_observation_has_value",
        "MarketObservation",
        "A non-retracted observation carries price or close.",
    ),
    (
        "retraction_reason_iff_retracted",
        "TemporalFact/MarketObservation",
        "A retraction reason is present exactly when retracted is true.",
    ),
];

/// The document description.
pub const DOCUMENT_DESCRIPTION: &str = "The complete Obolargus data dictionary: persisted domain entities, the normalized fact catalog, and typed fact values.";

/// Returns the relationship descriptors in authoritative order.
pub fn build_relationships(model: &ModelDescription) -> Value {
    Value::Array(
        model
            .relationships
            .iter()
            .map(|relationship| {
                json!({
                    "from": relationship.from,
                    "to": relationship.to,
                    "label": relationship.label,
                    "cardinality_from": relationship.cardinality_from,
                    "cardinality_to": relationship.cardinality_to
                })
            })
            .collect(),
    )
}

/// Returns the invariant strings in authoritative order.
pub fn build_invariants(model: &ModelDescription) -> Value {
    Value::Array(
        model
            .invariants
            .iter()
            .map(|invariant| json!(invariant))
            .collect(),
    )
}

/// Returns the named cross-field rules.
pub fn build_cross_field_constraints() -> Value {
    Value::Array(
        CROSS_FIELD_RULES
            .iter()
            .map(|(name, applies_to, description)| {
                json!({
                    "name": name,
                    "applies_to": applies_to,
                    "description": description
                })
            })
            .collect(),
    )
}

/// Adds catalog fact-type names to the `fact_type` fields as open known values.
pub fn apply_fact_type_known_values(definitions: &mut Map<String, Value>, catalog: &[FactTypeDef]) {
    let known: Vec<Value> = catalog.iter().map(|entry| json!(entry.fact_type)).collect();
    for definition_name in ["TemporalFact", "FactType", "FactField"] {
        let Some(definition) = definitions.get_mut(definition_name) else {
            continue;
        };
        let Some(properties) = definition
            .get_mut("properties")
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        let Some(field) = properties
            .get_mut("fact_type")
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        field.insert(
            "x-obolargus-known-values".to_string(),
            Value::Array(known.clone()),
        );
    }
}
