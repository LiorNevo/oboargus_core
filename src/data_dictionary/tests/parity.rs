use std::collections::BTreeSet;

use serde_json::Value;

use crate::data_dictionary::export_system_schema;
use crate::data_dictionary::parity::check_code_parity;
use crate::domain::model_description::model_description;
use crate::temporal::catalog::fact_catalog;

#[test]
fn code_parity_holds_for_every_entity_field_and_fact_type() {
    let schema = export_system_schema().expect("schema");
    check_code_parity(&schema).expect("code parity");
}

#[test]
fn definitions_contain_no_unexpected_entities_or_fact_types() {
    let schema = export_system_schema().expect("schema");
    let definitions = schema
        .document()
        .get("$defs")
        .and_then(Value::as_object)
        .expect("$defs");

    let expected_entities = model_description()
        .entities
        .iter()
        .map(|entity| entity.name.to_string())
        .collect::<BTreeSet<_>>();
    let expected_fact_types = fact_catalog()
        .iter()
        .map(|entry| format!("FactType.{}", entry.fact_type))
        .collect::<BTreeSet<_>>();
    let expected_value_shapes = [
        "FactValue",
        "FactValue.text",
        "FactValue.decimal",
        "FactValue.integer",
        "FactValue.boolean",
        "FactValue.date",
        "FactValue.timestamp",
        "FactValue.market_price",
    ]
    .iter()
    .map(|name| (*name).to_string())
    .collect::<BTreeSet<_>>();

    let actual = definitions.keys().cloned().collect::<BTreeSet<_>>();
    let expected = expected_entities
        .union(&expected_fact_types)
        .cloned()
        .collect::<BTreeSet<_>>()
        .union(&expected_value_shapes)
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected);
}

#[test]
fn every_entity_field_appears_exactly_once() {
    let schema = export_system_schema().expect("schema");
    let definitions = schema
        .document()
        .get("$defs")
        .and_then(Value::as_object)
        .expect("$defs");
    for entity in model_description().entities {
        let properties = definitions
            .get(entity.name)
            .and_then(|definition| definition.get("properties"))
            .and_then(Value::as_object)
            .expect("entity properties");
        assert_eq!(properties.len(), entity.fields.len(), "{}", entity.name);
        for field in entity.fields {
            assert!(
                properties.contains_key(field.name),
                "missing {}.{}",
                entity.name,
                field.name
            );
        }
    }
}

#[test]
fn every_fact_field_appears_exactly_once() {
    let schema = export_system_schema().expect("schema");
    let definitions = schema
        .document()
        .get("$defs")
        .and_then(Value::as_object)
        .expect("$defs");
    for entry in fact_catalog() {
        let definition = definitions
            .get(&format!("FactType.{}", entry.fact_type))
            .expect("fact type definition");
        let value_ref = definition
            .get("properties")
            .and_then(|properties| properties.get("value"))
            .and_then(|value| value.get("$ref"))
            .and_then(Value::as_str)
            .expect("value reference");
        let shape = value_ref
            .strip_prefix("#/$defs/")
            .expect("definition reference");
        let properties = definitions
            .get(shape)
            .and_then(|definition| definition.get("properties"))
            .and_then(Value::as_object);
        if let Some(properties) = properties {
            for field in entry.fields {
                assert!(
                    properties.contains_key(field.name),
                    "missing {}.{}",
                    entry.fact_type,
                    field.name
                );
            }
        }
    }
}
