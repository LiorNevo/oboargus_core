//! Entity definitions built from the authoritative domain model.

use serde_json::{Map, Value, json};

use crate::data_dictionary::error::DataDictionaryError;
use crate::data_dictionary::types;
use crate::domain::model_description::{EntityDescription, ModelDescription};

/// Builds one JSON Schema definition per persisted domain entity.
pub fn build_entity_definitions(
    model: &ModelDescription,
) -> Result<Map<String, Value>, DataDictionaryError> {
    let mut definitions = Map::new();
    for entity in &model.entities {
        let definition = build_entity_definition(entity)?;
        insert_unique(&mut definitions, entity.name, definition)?;
    }
    Ok(definitions)
}

fn build_entity_definition(entity: &EntityDescription) -> Result<Value, DataDictionaryError> {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for field in &entity.fields {
        if field.name.is_empty() || field.type_domain.is_empty() {
            return Err(DataDictionaryError::MissingMetadata {
                entity: entity.name.to_string(),
                field: field.name.to_string(),
            });
        }
        let mut schema = types::schema_for_domain(field.type_domain);
        types::apply_constraints(&mut schema, field.constraints);
        if field.nullable {
            types::make_nullable(&mut schema);
        } else {
            required.push(field.name.to_string());
        }
        if let Some(object) = schema.as_object_mut() {
            object.insert(
                "x-obolargus-writable".to_string(),
                json!(is_writable(entity.name, field.name)),
            );
            object.insert(
                "x-obolargus-aggregates".to_string(),
                Value::Array(field.aggregates.iter().map(|value| json!(value)).collect()),
            );
        }
        properties.insert(field.name.to_string(), schema);
    }
    required.sort();
    Ok(json!({
        "type": "object",
        "description": entity.description,
        "properties": properties,
        "required": required,
        "additionalProperties": false,
        "x-obolargus-entity": entity.name,
        "x-obolargus-table": table_name(entity.name),
    }))
}

/// Inserts a definition and rejects a duplicate name.
pub(crate) fn insert_unique(
    definitions: &mut Map<String, Value>,
    name: &str,
    definition: Value,
) -> Result<(), DataDictionaryError> {
    if definitions.contains_key(name) {
        return Err(DataDictionaryError::DuplicateDefinition {
            name: name.to_string(),
        });
    }
    definitions.insert(name.to_string(), definition);
    Ok(())
}

fn table_name(entity: &str) -> &'static str {
    match entity {
        "Portfolio" => "portfolios",
        "Asset" => "assets",
        "Position" => "positions",
        "Event" => "events",
        "Issuer" => "issuers",
        "AssetIdentifier" => "asset_identifiers",
        "IssuerIdentifier" => "issuer_identifiers",
        "Rule" => "rules",
        "TemporalFact" => "temporal_facts",
        "FactType" => "fact_types",
        "FactField" => "fact_type_fields",
        "MarketObservation" => "market_observations",
        _ => "unknown",
    }
}

fn is_writable(entity: &str, field: &str) -> bool {
    match entity {
        "Portfolio" => matches!(field, "name" | "purpose" | "parent_id"),
        "Asset" => matches!(
            field,
            "asset_type" | "display_name" | "currency" | "backed_by_portfolio_id" | "issuer_id"
        ),
        "Position" => matches!(field, "portfolio_id" | "asset_id" | "quantity" | "as_of"),
        "Event" => matches!(
            field,
            "event_type"
                | "effective_date"
                | "quantity_delta"
                | "portfolio_id"
                | "asset_id"
                | "to_portfolio_id"
        ),
        "Issuer" => matches!(
            field,
            "name" | "issuer_type" | "legal_jurisdiction" | "parent_id"
        ),
        "AssetIdentifier" => matches!(field, "asset_id" | "scheme" | "value"),
        "IssuerIdentifier" => matches!(field, "issuer_id" | "scheme" | "value"),
        "Rule" => matches!(field, "name" | "category" | "expression" | "description"),
        "TemporalFact" => matches!(
            field,
            "fact_type" | "source" | "retracted" | "retraction_reason"
        ),
        "MarketObservation" => matches!(
            field,
            "asset_id"
                | "kind"
                | "interval_seconds"
                | "source"
                | "open"
                | "high"
                | "low"
                | "close"
                | "adjclose"
                | "price"
                | "volume"
                | "currency"
                | "adjustment"
                | "sequence"
                | "retracted"
                | "retraction_reason"
        ),
        _ => false,
    }
}
