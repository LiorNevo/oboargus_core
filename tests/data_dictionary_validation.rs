//! Schema and semantic validation branches for the system data dictionary.

use std::cmp::Ordering;

use serde_json::{Value, json};

use obolargus_core::data_dictionary::validate::decimal_cmp;
use obolargus_core::data_dictionary::validate_instance;

fn asset() -> Value {
    json!({
        "id": "00000000-0000-0000-0000-000000000001",
        "asset_type": "stock",
        "display_name": "Example",
        "currency": "USD",
        "created_at": "2026-01-01T00:00:00Z"
    })
}

fn fact_field() -> Value {
    json!({
        "fact_type": "market_price",
        "field_name": "close",
        "data_type": "decimal",
        "description": "Closing price",
        "nullable": false,
        "ordinal": 1,
        "aggregates": ["last"],
        "precision": 10,
        "scale": 2
    })
}

fn observation(kind: &str) -> Value {
    json!({
        "id": "00000000-0000-0000-0000-000000000020",
        "asset_id": "00000000-0000-0000-0000-000000000002",
        "kind": kind,
        "interval_seconds": 60,
        "valid_at": "2026-01-01T00:00:00Z",
        "known_at": "2026-01-01T00:00:00Z",
        "source": "test",
        "content_hash": "0000000000000000000000000000000000000000000000000000000000000000",
        "currency": "USD",
        "adjustment": "adjusted",
        "retracted": false
    })
}

#[test]
fn unknown_definitions_are_reported() {
    let errors = validate_instance("NoSuchDefinition", &json!({})).expect_err("unknown");
    assert_eq!(errors[0].rule, "unknown_definition:NoSuchDefinition");
}

#[test]
fn schema_primitives_report_enum_const_format_and_bounds() {
    // oneOf: exactly one variant matches.
    validate_instance("FactValue", &json!({ "type": "text", "value": "x" })).expect("text");
    let errors = validate_instance("FactValue", &json!({ "type": "bogus" })).expect_err("oneOf");
    assert!(errors.iter().any(|error| error.rule == "oneOf"));

    // const: the typed variant requires its own tag.
    let errors = validate_instance(
        "FactType.market_price",
        &json!({ "type": "nav", "value": "1.0" }),
    )
    .expect_err("const");
    assert!(errors.iter().any(|error| error.rule == "const"));

    // enum.
    let mut event = json!({
        "id": "00000000-0000-0000-0000-000000000010",
        "event_type": "bogus",
        "effective_date": "2026-01-01",
        "quantity_delta": "1",
        "recorded_at": "2026-01-01T00:00:00Z",
        "portfolio_id": "00000000-0000-0000-0000-000000000001",
        "asset_id": "00000000-0000-0000-0000-000000000002"
    });
    let errors = validate_instance("Event", &event).expect_err("enum");
    assert!(errors.iter().any(|error| error.rule == "enum"));

    event
        .as_object_mut()
        .expect("object")
        .insert("event_type".to_string(), json!("buy"));
    event
        .as_object_mut()
        .expect("object")
        .insert("quantity_delta".to_string(), json!("1.2.3"));
    let errors = validate_instance("Event", &event).expect_err("pattern");
    assert!(errors.iter().any(|error| error.rule == "pattern"));

    // minLength and format/pattern on strings.
    let mut invalid_asset = asset();
    invalid_asset
        .as_object_mut()
        .expect("object")
        .insert("display_name".to_string(), json!(""));
    let errors = validate_instance("Asset", &invalid_asset).expect_err("minLength");
    assert!(errors.iter().any(|error| error.rule == "minLength"));

    let mut invalid_asset = asset();
    invalid_asset
        .as_object_mut()
        .expect("object")
        .insert("currency".to_string(), json!("usd"));
    invalid_asset
        .as_object_mut()
        .expect("object")
        .insert("id".to_string(), json!("not-a-uuid"));
    let errors = validate_instance("Asset", &invalid_asset).expect_err("format and pattern");
    assert!(errors.iter().any(|error| error.rule == "pattern"));
    assert!(errors.iter().any(|error| error.rule == "format"));
}

#[test]
fn array_items_and_numeric_bounds_are_validated() {
    validate_instance("FactField", &fact_field()).expect("valid field");

    let mut field = fact_field();
    field
        .as_object_mut()
        .expect("object")
        .insert("aggregates".to_string(), json!(["bogus"]));
    let errors = validate_instance("FactField", &field).expect_err("item enum");
    assert!(errors.iter().any(|error| error.pointer == "/aggregates/0"));

    let mut field = fact_field();
    field
        .as_object_mut()
        .expect("object")
        .insert("ordinal".to_string(), json!(0));
    let errors = validate_instance("FactField", &field).expect_err("ordinal minimum");
    assert!(
        errors
            .iter()
            .any(|error| error.pointer == "/ordinal" && error.rule == "minimum")
    );

    let mut field = fact_field();
    field
        .as_object_mut()
        .expect("object")
        .insert("precision".to_string(), json!(39));
    let errors = validate_instance("FactField", &field).expect_err("precision maximum");
    assert!(
        errors
            .iter()
            .any(|error| error.pointer == "/precision" && error.rule == "maximum")
    );

    let mut field = fact_field();
    field
        .as_object_mut()
        .expect("object")
        .insert("scale".to_string(), json!(-1));
    let errors = validate_instance("FactField", &field).expect_err("scale minimum");
    assert!(
        errors
            .iter()
            .any(|error| error.pointer == "/scale" && error.rule == "minimum")
    );

    let mut field = fact_field();
    field
        .as_object_mut()
        .expect("object")
        .insert("precision".to_string(), json!("many"));
    let errors = validate_instance("FactField", &field).expect_err("type");
    assert!(errors.iter().any(|error| error.pointer == "/precision"));
}

#[test]
fn semantic_rules_cover_observations_and_catalog_references() {
    let errors = validate_instance("MarketObservation", &observation("quote"))
        .expect_err("quote needs a price");
    assert!(
        errors
            .iter()
            .any(|error| error.rule == "market_observation_quote_shape")
    );
    assert!(
        errors
            .iter()
            .any(|error| error.rule == "market_observation_has_value")
    );

    let mut value = observation("quote");
    value
        .as_object_mut()
        .expect("object")
        .insert("price".to_string(), json!("10.0"));
    validate_instance("MarketObservation", &value).expect("quote with price");

    let mut value = observation("bar");
    value
        .as_object_mut()
        .expect("object")
        .insert("open".to_string(), json!("12.0"));
    value
        .as_object_mut()
        .expect("object")
        .insert("high".to_string(), json!("11.0"));
    value
        .as_object_mut()
        .expect("object")
        .insert("low".to_string(), json!("10.0"));
    value
        .as_object_mut()
        .expect("object")
        .insert("close".to_string(), json!("11.0"));
    let errors = validate_instance("MarketObservation", &value).expect_err("open above high");
    assert!(errors.iter().any(|error| error.pointer == "/open"));

    let mut value = observation("bar");
    value
        .as_object_mut()
        .expect("object")
        .insert("high".to_string(), json!("11.0"));
    value
        .as_object_mut()
        .expect("object")
        .insert("retracted".to_string(), json!(true));
    let errors = validate_instance("MarketObservation", &value).expect_err("retraction reason");
    assert!(
        errors
            .iter()
            .any(|error| error.rule == "retraction_reason_iff_retracted")
    );

    // A retracted observation with a reason skips the price-shape rules.
    let mut value = observation("bar");
    value
        .as_object_mut()
        .expect("object")
        .insert("retracted".to_string(), json!(true));
    value
        .as_object_mut()
        .expect("object")
        .insert("retraction_reason".to_string(), json!("restated"));
    validate_instance("MarketObservation", &value).expect("retracted with reason");

    // A reason without the retracted flag is also rejected.
    let mut value = observation("quote");
    value
        .as_object_mut()
        .expect("object")
        .insert("price".to_string(), json!("10.0"));
    value
        .as_object_mut()
        .expect("object")
        .insert("retraction_reason".to_string(), json!("restated"));
    let errors = validate_instance("MarketObservation", &value).expect_err("reason without flag");
    assert!(
        errors
            .iter()
            .any(|error| error.rule == "retraction_reason_iff_retracted")
    );

    let mut field = fact_field();
    field
        .as_object_mut()
        .expect("object")
        .insert("fact_type".to_string(), json!("not_real"));
    let errors = validate_instance("FactField", &field).expect_err("unknown fact type");
    assert!(
        errors
            .iter()
            .any(|error| error.pointer == "/fact_type" && error.rule == "unknown_fact_type")
    );

    // Semantic validators tolerate non-object instances.
    let _ = validate_instance("Event", &json!([]));
    let _ = validate_instance("FactField", &json!("x"));
    let _ = validate_instance("MarketObservation", &json!(null));
    let _ = validate_instance("TemporalFact", &json!(1));
}

#[test]
fn decimal_comparison_is_exact_and_rejects_malformed_values() {
    assert_eq!(decimal_cmp("1.0", "1"), Some(Ordering::Equal));
    assert_eq!(decimal_cmp("0.0", "0"), Some(Ordering::Equal));
    assert_eq!(decimal_cmp("-0", "0"), Some(Ordering::Equal));
    assert_eq!(decimal_cmp("2", "10"), Some(Ordering::Less));
    assert_eq!(decimal_cmp("10", "2"), Some(Ordering::Greater));
    assert_eq!(decimal_cmp("-1", "0"), Some(Ordering::Less));
    assert_eq!(decimal_cmp("0", "-1"), Some(Ordering::Greater));
    assert_eq!(decimal_cmp("-2", "-10"), Some(Ordering::Greater));
    assert_eq!(decimal_cmp("-10", "-2"), Some(Ordering::Less));
    assert_eq!(decimal_cmp("1.25", "1.3"), Some(Ordering::Less));
    assert_eq!(decimal_cmp("1.3", "1.25"), Some(Ordering::Greater));
    assert_eq!(decimal_cmp("1.20", "1.2"), Some(Ordering::Equal));
    assert_eq!(decimal_cmp("abc", "1"), None);
    assert_eq!(decimal_cmp("1", "abc"), None);
    assert_eq!(decimal_cmp("1.2.3", "1"), None);
    assert_eq!(decimal_cmp("", "0"), None);
}
