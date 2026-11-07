//! Fact-type definitions built from the authoritative catalog.

use serde_json::{Map, Value, json};

use crate::data_dictionary::entity::insert_unique;
use crate::data_dictionary::error::DataDictionaryError;
use crate::temporal::catalog::{FactTypeDef, StorageKind};

/// Builds one definition per catalog fact type.
pub fn build_fact_type_definitions(
    catalog: &[FactTypeDef],
) -> Result<Map<String, Value>, DataDictionaryError> {
    let mut definitions = Map::new();
    for entry in catalog {
        let definition = build_fact_type_definition(entry);
        insert_unique(
            &mut definitions,
            &format!("FactType.{}", entry.fact_type),
            definition,
        )?;
    }
    Ok(definitions)
}

fn build_fact_type_definition(entry: &FactTypeDef) -> Value {
    let value_shape = match entry.storage_kind {
        StorageKind::MarketObservation => "FactValue.market_price".to_string(),
        other => format!("FactValue.{}", other.as_str()),
    };
    json!({
        "type": "object",
        "description": entry.description,
        "properties": {
            "type": { "const": entry.fact_type },
            "value": { "$ref": format!("#/$defs/{value_shape}") }
        },
        "required": ["type", "value"],
        "additionalProperties": false,
        "x-obolargus-fact-type": {
            "fact_type": entry.fact_type,
            "display_name": entry.display_name,
            "subjects": entry.subjects.iter().map(|subject| subject.as_str()).collect::<Vec<_>>(),
            "storage_kind": entry.storage_kind.as_str(),
            "aggregation": entry.aggregation,
            "unit": entry.unit,
            "observation_kind": entry.observation_kind,
            "interval_seconds": entry.interval_seconds,
            "schema_version": entry.schema_version
        }
    })
}
