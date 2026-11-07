use serde_json::json;

use crate::data_dictionary::validate_instance;

#[test]
fn scalar_shapes_validate() {
    validate_instance("FactValue.text", &json!("hello")).expect("text");
    validate_instance("FactValue.decimal", &json!("123.45")).expect("decimal");
    validate_instance("FactValue.integer", &json!("42")).expect("integer");
    validate_instance("FactValue.boolean", &json!(true)).expect("boolean");
    validate_instance("FactValue.date", &json!("2026-01-01")).expect("date");
    validate_instance("FactValue.timestamp", &json!("2026-01-01T00:00:00Z")).expect("timestamp");
}

#[test]
fn decimal_precision_and_scale_are_enforced() {
    let errors = validate_instance("FactValue.decimal", &json!("1.2345678901234567890"))
        .expect_err("too many fractional digits");
    assert!(errors.iter().any(|error| error.rule == "pattern"));
}

#[test]
fn wrong_scalar_type_fails() {
    let errors = validate_instance("FactValue.integer", &json!("4.2")).expect_err("not integer");
    assert!(errors.iter().any(|error| error.rule == "pattern"));
    let errors = validate_instance("FactValue.boolean", &json!("true")).expect_err("not boolean");
    assert!(errors.iter().any(|error| error.rule == "type"));
}

#[test]
fn market_price_shape_validates() {
    let value = json!({
        "open": "10.0",
        "high": "12.0",
        "low": "9.0",
        "close": "11.0",
        "currency": "USD",
        "adjustment": "adjusted"
    });
    validate_instance("FactValue.market_price", &value).expect("market price");
}

#[test]
fn market_price_rejects_missing_required_field() {
    let value = json!({
        "open": "10.0",
        "high": "12.0",
        "low": "9.0",
        "currency": "USD",
        "adjustment": "adjusted"
    });
    let errors = validate_instance("FactValue.market_price", &value).expect_err("missing close");
    assert!(
        errors
            .iter()
            .any(|error| error.pointer == "/close" && error.rule == "required")
    );
}

#[test]
fn market_price_rejects_bad_currency() {
    let value = json!({
        "open": "10.0",
        "high": "12.0",
        "low": "9.0",
        "close": "11.0",
        "currency": "usd",
        "adjustment": "adjusted"
    });
    let errors = validate_instance("FactValue.market_price", &value).expect_err("bad currency");
    assert!(errors.iter().any(|error| error.pointer == "/currency"));
}

#[test]
fn market_price_rejects_negative_volume() {
    let value = json!({
        "open": "10.0",
        "high": "12.0",
        "low": "9.0",
        "close": "11.0",
        "currency": "USD",
        "adjustment": "adjusted",
        "volume": "-1"
    });
    let errors = validate_instance("FactValue.market_price", &value).expect_err("negative volume");
    assert!(
        errors
            .iter()
            .any(|error| error.pointer == "/volume" && error.rule == "minimum")
    );
}
