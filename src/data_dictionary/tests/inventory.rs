use crate::data_dictionary::export_system_schema;
use crate::temporal::catalog::fact_catalog;

#[test]
fn document_contains_every_expected_definition() {
    let schema = export_system_schema().expect("schema");
    let definitions = schema
        .document()
        .get("$defs")
        .and_then(|value| value.as_object())
        .expect("$defs");
    for name in [
        "Portfolio",
        "Asset",
        "Position",
        "Event",
        "Issuer",
        "AssetIdentifier",
        "IssuerIdentifier",
        "Rule",
        "TemporalFact",
        "FactType",
        "FactField",
        "MarketObservation",
        "FactValue.text",
        "FactValue.decimal",
        "FactValue.integer",
        "FactValue.boolean",
        "FactValue.date",
        "FactValue.timestamp",
        "FactValue.market_price",
        "FactValue",
    ] {
        assert!(definitions.contains_key(name), "missing definition {name}");
    }
    for entry in fact_catalog() {
        assert!(
            definitions.contains_key(&format!("FactType.{}", entry.fact_type)),
            "missing fact type {}",
            entry.fact_type
        );
    }
}
