use serde_json::{Value, json};

use crate::data_dictionary::validate_instance;

fn event(event_type: &str, destination: Option<&str>, quantity: &str) -> Value {
    json!({
        "id": "00000000-0000-0000-0000-000000000010",
        "event_type": event_type,
        "effective_date": "2026-01-01",
        "quantity_delta": quantity,
        "recorded_at": "2026-01-01T00:00:00Z",
        "portfolio_id": "00000000-0000-0000-0000-000000000001",
        "asset_id": "00000000-0000-0000-0000-000000000002",
        "to_portfolio_id": destination
    })
}

#[test]
fn transfer_requires_destination() {
    let errors =
        validate_instance("Event", &event("transfer", None, "1")).expect_err("missing destination");
    assert!(
        errors
            .iter()
            .any(|error| error.rule == "transfer_requires_destination")
    );
}

#[test]
fn transfer_rejects_self() {
    let errors = validate_instance(
        "Event",
        &event(
            "transfer",
            Some("00000000-0000-0000-0000-000000000001"),
            "1",
        ),
    )
    .expect_err("self transfer");
    assert!(errors.iter().any(|error| error.rule == "transfer_not_self"));
}

#[test]
fn quantity_delta_must_be_non_zero() {
    let errors = validate_instance("Event", &event("buy", None, "0")).expect_err("zero quantity");
    assert!(
        errors
            .iter()
            .any(|error| error.rule == "quantity_delta_non_zero")
    );
}

fn observation(high: &str, low: &str) -> Value {
    json!({
        "id": "00000000-0000-0000-0000-000000000020",
        "asset_id": "00000000-0000-0000-0000-000000000002",
        "kind": "bar",
        "interval_seconds": 60,
        "valid_at": "2026-01-01T00:00:00Z",
        "known_at": "2026-01-01T00:00:00Z",
        "source": "test",
        "content_hash": "0000000000000000000000000000000000000000000000000000000000000000",
        "open": "10.0",
        "high": high,
        "low": low,
        "close": "10.5",
        "currency": "USD",
        "adjustment": "adjusted",
        "retracted": false
    })
}

#[test]
fn ohlc_bounds_are_enforced() {
    let errors = validate_instance("MarketObservation", &observation("9.0", "10.0"))
        .expect_err("high below low");
    assert!(
        errors
            .iter()
            .any(|error| error.rule == "market_observation_ohlc_bounds")
    );
}

#[test]
fn bar_shape_is_enforced() {
    let mut value = observation("12.0", "9.0");
    value.as_object_mut().expect("object").remove("open");
    let errors = validate_instance("MarketObservation", &value).expect_err("missing open");
    assert!(
        errors
            .iter()
            .any(|error| error.rule == "market_observation_bar_shape")
    );
}

#[test]
fn retraction_reason_iff_retracted() {
    let value = json!({
        "id": "00000000-0000-0000-0000-000000000030",
        "subject_kind": "asset",
        "subject_id": "00000000-0000-0000-0000-000000000002",
        "fact_type": "name",
        "valid_at": "2026-01-01T00:00:00Z",
        "known_at": "2026-01-01T00:00:00Z",
        "source": "test",
        "retracted": true,
        "content_hash": "0000000000000000000000000000000000000000000000000000000000000000"
    });
    let errors = validate_instance("TemporalFact", &value).expect_err("missing reason");
    assert!(
        errors
            .iter()
            .any(|error| error.rule == "retraction_reason_iff_retracted")
    );
}

#[test]
fn unknown_fact_type_and_invalid_subject_fail() {
    let mut value = json!({
        "id": "00000000-0000-0000-0000-000000000031",
        "subject_kind": "issuer",
        "subject_id": "00000000-0000-0000-0000-000000000003",
        "fact_type": "market_price",
        "valid_at": "2026-01-01T00:00:00Z",
        "known_at": "2026-01-01T00:00:00Z",
        "source": "test",
        "retracted": false,
        "content_hash": "0000000000000000000000000000000000000000000000000000000000000000"
    });
    let errors = validate_instance("TemporalFact", &value).expect_err("invalid subject");
    assert!(errors.iter().any(|error| error.rule == "invalid_subject"));

    value
        .as_object_mut()
        .expect("object")
        .insert("fact_type".to_string(), json!("does_not_exist"));
    let errors = validate_instance("TemporalFact", &value).expect_err("unknown fact type");
    assert!(errors.iter().any(|error| error.rule == "unknown_fact_type"));
}
