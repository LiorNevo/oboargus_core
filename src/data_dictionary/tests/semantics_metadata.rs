use serde_json::Value;

use crate::data_dictionary::export_system_schema;
use crate::domain::model_description::model_description;
use crate::temporal::catalog::{StorageKind, fact_catalog};

fn definition<'a>(document: &'a Value, name: &str) -> &'a Value {
    document
        .get("$defs")
        .and_then(|definitions| definitions.get(name))
        .unwrap_or_else(|| panic!("missing definition {name}"))
}

#[test]
fn relationships_preserve_labels_and_cardinalities() {
    let schema = export_system_schema().expect("schema");
    let document = schema.document();
    let relationships = document
        .get("x-obolargus-relationships")
        .and_then(Value::as_array)
        .expect("relationships");
    let model = model_description();
    assert_eq!(relationships.len(), model.relationships.len());
    for (actual, expected) in relationships.iter().zip(&model.relationships) {
        assert_eq!(
            actual.get("from").and_then(Value::as_str),
            Some(expected.from)
        );
        assert_eq!(actual.get("to").and_then(Value::as_str), Some(expected.to));
        assert_eq!(
            actual.get("label").and_then(Value::as_str),
            Some(expected.label)
        );
        assert_eq!(
            actual.get("cardinality_from").and_then(Value::as_str),
            Some(expected.cardinality_from)
        );
        assert_eq!(
            actual.get("cardinality_to").and_then(Value::as_str),
            Some(expected.cardinality_to)
        );
    }
}

#[test]
fn invariants_are_preserved_in_order() {
    let schema = export_system_schema().expect("schema");
    let document = schema.document();
    let invariants = document
        .get("x-obolargus-invariants")
        .and_then(Value::as_array)
        .expect("invariants");
    let expected = model_description()
        .invariants
        .iter()
        .map(|value| Value::String((*value).to_string()))
        .collect::<Vec<_>>();
    assert_eq!(invariants, &expected);
}

#[test]
fn fact_type_metadata_matches_the_catalog() {
    let schema = export_system_schema().expect("schema");
    let document = schema.document();
    for entry in fact_catalog() {
        let metadata = definition(document, &format!("FactType.{}", entry.fact_type))
            .get("x-obolargus-fact-type")
            .expect("fact-type metadata");
        assert_eq!(
            metadata.get("fact_type").and_then(Value::as_str),
            Some(entry.fact_type)
        );
        assert_eq!(
            metadata.get("display_name").and_then(Value::as_str),
            Some(entry.display_name)
        );
        assert_eq!(
            metadata.get("storage_kind").and_then(Value::as_str),
            Some(entry.storage_kind.as_str())
        );
        assert_eq!(
            metadata.get("aggregation").and_then(Value::as_str),
            Some(entry.aggregation)
        );
        assert_eq!(metadata.get("unit").and_then(Value::as_str), entry.unit);
        assert_eq!(
            metadata.get("observation_kind").and_then(Value::as_str),
            entry.observation_kind
        );
        assert_eq!(
            metadata
                .get("interval_seconds")
                .and_then(Value::as_i64)
                .map(|value| value as i32),
            entry.interval_seconds
        );
        assert_eq!(
            metadata
                .get("schema_version")
                .and_then(Value::as_i64)
                .map(|value| value as i32),
            Some(entry.schema_version)
        );
        let subjects = metadata
            .get("subjects")
            .and_then(Value::as_array)
            .expect("subjects");
        let actual = subjects
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        let expected = entry
            .subjects
            .iter()
            .map(|subject| subject.as_str())
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }
}

#[test]
fn closed_vocabularies_use_enum() {
    let schema = export_system_schema().expect("schema");
    let document = schema.document();
    let event_type = definition(document, "Event")
        .get("properties")
        .and_then(|properties| properties.get("event_type"))
        .expect("event_type");
    assert_eq!(
        event_type
            .get("enum")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(8)
    );
    assert!(event_type.get("x-obolargus-known-values").is_none());

    let aggregates = definition(document, "FactField")
        .get("properties")
        .and_then(|properties| properties.get("aggregates"))
        .expect("aggregates");
    assert!(aggregates.get("enum").is_none());
    assert!(
        aggregates
            .get("items")
            .and_then(|items| items.get("enum"))
            .is_some()
    );
}

#[test]
fn open_vocabularies_use_known_values_without_enum() {
    let schema = export_system_schema().expect("schema");
    let document = schema.document();
    for (entity, field) in [
        ("AssetIdentifier", "scheme"),
        ("IssuerIdentifier", "scheme"),
        ("Rule", "category"),
        ("TemporalFact", "fact_type"),
        ("FactType", "fact_type"),
    ] {
        let property = definition(document, entity)
            .get("properties")
            .and_then(|properties| properties.get(field))
            .unwrap_or_else(|| panic!("missing {entity}.{field}"));
        assert!(
            property.get("enum").is_none(),
            "{entity}.{field} must stay open"
        );
        assert!(
            property
                .get("x-obolargus-known-values")
                .and_then(Value::as_array)
                .is_some(),
            "{entity}.{field} must list known values"
        );
    }
}

#[test]
fn every_entity_has_a_description() {
    let schema = export_system_schema().expect("schema");
    let document = schema.document();
    for entity in model_description().entities {
        let description = definition(document, entity.name)
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            !description.is_empty(),
            "{} has no description",
            entity.name
        );
    }
}

#[test]
fn market_price_fields_carry_catalog_metadata() {
    let schema = export_system_schema().expect("schema");
    let document = schema.document();
    let properties = definition(document, "FactValue.market_price")
        .get("properties")
        .and_then(Value::as_object)
        .expect("market price properties");
    let entry = fact_catalog()
        .iter()
        .find(|entry| entry.storage_kind == StorageKind::MarketObservation)
        .expect("market observation fact type");
    assert_eq!(properties.len(), entry.fields.len());
    for field in entry.fields {
        let property = properties.get(field.name).expect("field");
        assert_eq!(
            property.get("description").and_then(Value::as_str),
            Some(field.description)
        );
    }
}
