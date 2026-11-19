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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model_description::FieldDescription;

    fn field(name: &'static str, type_domain: &'static str, nullable: bool) -> FieldDescription {
        FieldDescription {
            name,
            type_domain,
            aggregates: &["last"],
            nullable,
            constraints: &[],
        }
    }

    #[test]
    fn builds_properties_required_fields_and_metadata() {
        let model = ModelDescription {
            entities: vec![EntityDescription {
                name: "Portfolio",
                description: "A portfolio.",
                fields: vec![
                    field("name", "text", false),
                    field("parent_id", "uuid", true),
                    field("description", "text", false),
                ],
            }],
            relationships: Vec::new(),
            invariants: Vec::new(),
        };
        let definitions = build_entity_definitions(&model).expect("definitions");
        let definition = definitions.get("Portfolio").expect("portfolio");
        assert_eq!(definition["type"], "object");
        assert_eq!(definition["additionalProperties"], false);
        assert_eq!(definition["x-obolargus-table"], "portfolios");
        assert_eq!(definition["required"], json!(["description", "name"]));
        assert_eq!(
            definition["properties"]["name"]["x-obolargus-writable"],
            true
        );
        assert_eq!(
            definition["properties"]["description"]["x-obolargus-writable"],
            false
        );
        assert_eq!(
            definition["properties"]["name"]["x-obolargus-aggregates"],
            json!(["last"])
        );
        assert!(
            definition["properties"]["parent_id"]["type"]
                .as_array()
                .expect("nullable type")
                .contains(&json!("null"))
        );
    }

    #[test]
    fn unknown_tables_and_read_only_entities_are_flagged() {
        let model = ModelDescription {
            entities: vec![
                EntityDescription {
                    name: "FactType",
                    description: "A fact type.",
                    fields: vec![field("fact_type", "text", false)],
                },
                EntityDescription {
                    name: "Custom",
                    description: "A custom entity.",
                    fields: vec![field("value", "text", false)],
                },
            ],
            relationships: Vec::new(),
            invariants: Vec::new(),
        };
        let definitions = build_entity_definitions(&model).expect("definitions");
        assert_eq!(definitions["FactType"]["x-obolargus-table"], "fact_types");
        assert_eq!(
            definitions["FactType"]["properties"]["fact_type"]["x-obolargus-writable"],
            false
        );
        assert_eq!(definitions["Custom"]["x-obolargus-table"], "unknown");
    }

    #[test]
    fn every_entity_table_name_and_writable_field_is_recognized() {
        for (entity, table) in [
            ("Portfolio", "portfolios"),
            ("Asset", "assets"),
            ("Position", "positions"),
            ("Event", "events"),
            ("Issuer", "issuers"),
            ("AssetIdentifier", "asset_identifiers"),
            ("IssuerIdentifier", "issuer_identifiers"),
            ("Rule", "rules"),
            ("TemporalFact", "temporal_facts"),
            ("FactType", "fact_types"),
            ("FactField", "fact_type_fields"),
            ("MarketObservation", "market_observations"),
        ] {
            assert_eq!(table_name(entity), table);
        }
        assert!(is_writable("Asset", "display_name"));
        assert!(!is_writable("Asset", "created_at"));
        assert!(is_writable("Position", "quantity"));
        assert!(is_writable("Event", "event_type"));
        assert!(is_writable("Issuer", "name"));
        assert!(is_writable("AssetIdentifier", "scheme"));
        assert!(is_writable("IssuerIdentifier", "value"));
        assert!(is_writable("Rule", "expression"));
        assert!(is_writable("TemporalFact", "source"));
        assert!(is_writable("MarketObservation", "close"));
        assert!(!is_writable("FactType", "fact_type"));
    }

    #[test]
    fn missing_metadata_and_duplicates_are_rejected() {
        let missing_name = ModelDescription {
            entities: vec![EntityDescription {
                name: "Broken",
                description: "Broken.",
                fields: vec![field("", "text", false)],
            }],
            relationships: Vec::new(),
            invariants: Vec::new(),
        };
        assert!(matches!(
            build_entity_definitions(&missing_name).expect_err("missing name"),
            DataDictionaryError::MissingMetadata { .. }
        ));

        let missing_type = ModelDescription {
            entities: vec![EntityDescription {
                name: "Broken",
                description: "Broken.",
                fields: vec![field("value", "", false)],
            }],
            relationships: Vec::new(),
            invariants: Vec::new(),
        };
        assert!(matches!(
            build_entity_definitions(&missing_type).expect_err("missing type"),
            DataDictionaryError::MissingMetadata { .. }
        ));

        let duplicate = ModelDescription {
            entities: vec![
                EntityDescription {
                    name: "Twin",
                    description: "First.",
                    fields: Vec::new(),
                },
                EntityDescription {
                    name: "Twin",
                    description: "Second.",
                    fields: Vec::new(),
                },
            ],
            relationships: Vec::new(),
            invariants: Vec::new(),
        };
        assert!(matches!(
            build_entity_definitions(&duplicate).expect_err("duplicate"),
            DataDictionaryError::DuplicateDefinition { .. }
        ));

        let mut definitions = Map::new();
        insert_unique(&mut definitions, "Twin", json!({})).expect("first");
        assert!(matches!(
            insert_unique(&mut definitions, "Twin", json!({})).expect_err("duplicate"),
            DataDictionaryError::DuplicateDefinition { .. }
        ));
    }
}
