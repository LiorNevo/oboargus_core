use crate::data_dictionary::export_system_schema;
use crate::temporal::catalog::{StorageKind, fact_catalog};

#[test]
fn fact_field_metadata_matches_the_catalog() {
    let schema = export_system_schema().expect("schema");
    let definitions = schema.document().get("$defs").expect("$defs");
    let market_price = definitions
        .get("FactValue.market_price")
        .expect("market price shape");
    let properties = market_price.get("properties").expect("properties");
    for entry in fact_catalog() {
        if entry.storage_kind != StorageKind::MarketObservation {
            continue;
        }
        for field in entry.fields {
            let property = properties.get(field.name).expect("field property");
            if let Some(unit) = field.unit {
                assert_eq!(
                    property
                        .get("x-obolargus-unit")
                        .and_then(|value| value.as_str()),
                    Some(unit)
                );
            }
            if let Some(precision) = field.precision {
                assert_eq!(
                    property
                        .get("x-obolargus-precision")
                        .and_then(|value| value.as_i64()),
                    Some(precision as i64)
                );
            }
            if let Some(scale) = field.scale {
                assert_eq!(
                    property
                        .get("x-obolargus-scale")
                        .and_then(|value| value.as_i64()),
                    Some(scale as i64)
                );
            }
            if let Some(minimum) = field.min_value {
                assert_eq!(
                    property
                        .get("x-obolargus-minimum")
                        .and_then(|value| value.as_str()),
                    Some(minimum)
                );
            }
        }
    }
}
