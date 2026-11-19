//! Typed fact values, validation, and canonical hashing.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

use crate::temporal::catalog::{FactFieldDef, FactTypeDef, StorageKind};
use crate::temporal::error::TemporalError;
use crate::temporal::facts::FactSubject;
use crate::temporal::instant::ValidAt;

/// The adjustment basis of a price series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Adjustment {
    /// Split/dividend-adjusted prices.
    Adjusted,
    /// Raw, as-traded prices.
    Unadjusted,
    /// Not applicable (quotes or non-price observations).
    None,
}

impl Adjustment {
    /// The stable wire/SQL value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Adjusted => "adjusted",
            Self::Unadjusted => "unadjusted",
            Self::None => "none",
        }
    }

    /// Parses a stable wire/SQL value.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "adjusted" => Some(Self::Adjusted),
            "unadjusted" => Some(Self::Unadjusted),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

/// A typed market-price value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketPriceValue {
    /// Opening price.
    #[serde(with = "decimal_string")]
    pub open: Decimal,
    /// Highest price.
    #[serde(with = "decimal_string")]
    pub high: Decimal,
    /// Lowest price.
    #[serde(with = "decimal_string")]
    pub low: Decimal,
    /// Unadjusted close.
    #[serde(with = "decimal_string")]
    pub close: Decimal,
    /// Adjusted close, when supplied.
    #[serde(with = "optional_decimal_string", default)]
    pub adjclose: Option<Decimal>,
    /// Volume in shares/contracts, when supplied.
    #[serde(with = "optional_decimal_string", default)]
    pub volume: Option<Decimal>,
    /// ISO 4217 quote currency.
    pub currency: String,
    /// Adjustment basis.
    pub adjustment: Adjustment,
}

impl MarketPriceValue {
    /// Normalizes the case of the currency code.
    pub fn normalize(mut self) -> Self {
        self.currency = self.currency.trim().to_uppercase();
        self
    }

    /// The canonical representation used for hashing.
    pub fn canonical(&self) -> String {
        format!(
            "open={};high={};low={};close={};adjclose={};volume={};currency={};adjustment={}",
            self.open.normalize(),
            self.high.normalize(),
            self.low.normalize(),
            self.close.normalize(),
            self.adjclose
                .map(|v| v.normalize().to_string())
                .unwrap_or_default(),
            self.volume
                .map(|v| v.normalize().to_string())
                .unwrap_or_default(),
            self.currency,
            self.adjustment.as_str(),
        )
    }
}

/// The typed value of a fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FactValue {
    /// A text value.
    Text {
        /// The value.
        value: String,
    },
    /// A decimal value.
    Decimal {
        /// The value.
        #[serde(with = "decimal_string")]
        value: Decimal,
    },
    /// An integer value.
    Integer {
        /// The value.
        #[serde(with = "i64_string")]
        value: i64,
    },
    /// A boolean value.
    Boolean {
        /// The value.
        value: bool,
    },
    /// A date value.
    Date {
        /// The value.
        value: NaiveDate,
    },
    /// A timestamp value.
    Timestamp {
        /// The value.
        value: DateTime<Utc>,
    },
    /// A market-price observation.
    MarketPrice(MarketPriceValue),
}

impl FactValue {
    /// The catalog storage kind this value requires.
    pub fn storage_kind(&self) -> StorageKind {
        match self {
            Self::Text { .. } => StorageKind::Text,
            Self::Decimal { .. } => StorageKind::Decimal,
            Self::Integer { .. } => StorageKind::Integer,
            Self::Boolean { .. } => StorageKind::Boolean,
            Self::Date { .. } => StorageKind::Date,
            Self::Timestamp { .. } => StorageKind::Timestamp,
            Self::MarketPrice(_) => StorageKind::MarketObservation,
        }
    }

    /// The canonical representation used for hashing.
    pub fn canonical(&self) -> String {
        match self {
            Self::Text { value } => format!("text:{}:{value}", value.len()),
            Self::Decimal { value } => format!("decimal:{}", value.normalize()),
            Self::Integer { value } => format!("integer:{value}"),
            Self::Boolean { value } => format!("boolean:{value}"),
            Self::Date { value } => format!("date:{value}"),
            Self::Timestamp { value } => format!("timestamp:{value}"),
            Self::MarketPrice(value) => format!("market_price:{}", value.canonical()),
        }
    }

    /// Validates this value against a fact type definition.
    pub fn validate(&self, def: &FactTypeDef) -> Result<(), TemporalError> {
        if self.storage_kind() != def.storage_kind {
            return Err(TemporalError::InvalidFactValue {
                fact_type: def.fact_type.to_string(),
                detail: format!(
                    "expected {} value, got {}",
                    def.storage_kind.as_str(),
                    self.storage_kind().as_str()
                ),
            });
        }

        if let Self::MarketPrice(value) = self {
            return validate_market_price(value);
        }

        let field = def
            .fields
            .first()
            .ok_or_else(|| TemporalError::InvalidFactValue {
                fact_type: def.fact_type.to_string(),
                detail: "fact type has no declared fields".to_string(),
            })?;
        validate_scalar(def, field, self)
    }
}

fn validate_scalar(
    def: &FactTypeDef,
    field: &FactFieldDef,
    value: &FactValue,
) -> Result<(), TemporalError> {
    let invalid = |detail: String| TemporalError::InvalidFactValue {
        fact_type: def.fact_type.to_string(),
        detail,
    };

    match (field.data_type, value) {
        ("text", FactValue::Text { value }) => {
            if value.trim().is_empty() {
                return Err(invalid("value must not be empty".to_string()));
            }
            if !field.allowed_values.is_empty() && !field.allowed_values.contains(&value.as_str()) {
                return Err(invalid(format!(
                    "value {value:?} is not one of {:?}",
                    field.allowed_values
                )));
            }
        }
        ("decimal", FactValue::Decimal { value }) => {
            check_decimal_range(field, *value).map_err(invalid)?;
            if let Some(scale) = field.scale
                && i32::try_from(value.scale()).unwrap_or(i32::MAX) > scale
            {
                return Err(invalid(format!(
                    "scale {} exceeds declared scale {scale}",
                    value.scale()
                )));
            }
        }
        ("integer", FactValue::Integer { .. }) => {}
        ("boolean", FactValue::Boolean { .. }) => {}
        ("date", FactValue::Date { .. }) => {}
        ("timestamp", FactValue::Timestamp { .. }) => {}
        (expected, actual) => {
            return Err(invalid(format!(
                "field type {expected} does not accept {}",
                actual.storage_kind().as_str()
            )));
        }
    }
    Ok(())
}

fn check_decimal_range(field: &FactFieldDef, value: Decimal) -> Result<(), String> {
    if let Some(min) = field.min_value
        && let Ok(min) = Decimal::from_str(min)
        && value < min
    {
        return Err(format!("value {value} is below minimum {min}"));
    }
    if let Some(max) = field.max_value
        && let Ok(max) = Decimal::from_str(max)
        && value > max
    {
        return Err(format!("value {value} is above maximum {max}"));
    }
    Ok(())
}

fn validate_market_price(value: &MarketPriceValue) -> Result<(), TemporalError> {
    let invalid = |detail: String| TemporalError::InvalidFactValue {
        fact_type: "market_price".to_string(),
        detail,
    };

    for (name, price) in [
        ("open", value.open),
        ("high", value.high),
        ("low", value.low),
        ("close", value.close),
    ] {
        if price < Decimal::ZERO {
            return Err(invalid(format!("{name} must not be negative")));
        }
    }
    if value.high < value.low {
        return Err(invalid(
            "high must be greater than or equal to low".to_string(),
        ));
    }
    if value.open < value.low || value.open > value.high {
        return Err(invalid("open must be between low and high".to_string()));
    }
    if value.close < value.low || value.close > value.high {
        return Err(invalid("close must be between low and high".to_string()));
    }
    if let Some(adjclose) = value.adjclose
        && adjclose < Decimal::ZERO
    {
        return Err(invalid("adjclose must not be negative".to_string()));
    }
    if let Some(volume) = value.volume
        && volume < Decimal::ZERO
    {
        return Err(invalid("volume must not be negative".to_string()));
    }
    if value.currency.len() != 3 || !value.currency.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(invalid(
            "currency must be a three-letter ISO 4217 code".to_string(),
        ));
    }
    Ok(())
}

/// Computes the canonical content hash for a fact value.
pub fn content_hash(
    subject: FactSubject,
    fact_type: &str,
    valid_at: ValidAt,
    source: &str,
    value: &FactValue,
) -> String {
    hash_parts(&[
        subject.kind().as_str(),
        &subject.id().to_string(),
        fact_type,
        &valid_at.to_string(),
        source,
        &value.canonical(),
    ])
}

/// Computes the canonical content hash for a retraction belief.
pub fn retraction_hash(
    subject: FactSubject,
    fact_type: &str,
    valid_at: ValidAt,
    source: &str,
    reason: &str,
) -> String {
    hash_parts(&[
        subject.kind().as_str(),
        &subject.id().to_string(),
        fact_type,
        &valid_at.to_string(),
        source,
        "retract",
        reason,
    ])
}

pub(crate) fn hash_parts(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

mod decimal_string {
    use super::*;

    pub fn serialize<S>(value: &Decimal, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.normalize().to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Decimal, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Decimal::from_str(&raw).map_err(D::Error::custom)
    }
}

mod optional_decimal_string {
    use super::*;

    pub fn serialize<S>(value: &Option<Decimal>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(value) => serializer.serialize_some(&value.normalize().to_string()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Decimal>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = Option::<String>::deserialize(deserializer)?;
        match raw {
            Some(raw) => Decimal::from_str(&raw).map(Some).map_err(D::Error::custom),
            None => Ok(None),
        }
    }
}

mod i64_string {
    use super::*;

    pub fn serialize<S>(value: &i64, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<i64, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        i64::from_str(&raw).map_err(D::Error::custom)
    }
}

impl fmt::Display for Adjustment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::id::DomainId;

    fn price(close: &str) -> MarketPriceValue {
        let close = Decimal::from_str(close).expect("decimal");
        MarketPriceValue {
            open: close,
            high: close,
            low: close,
            close,
            adjclose: None,
            volume: Some(Decimal::ONE),
            currency: "usd".into(),
            adjustment: Adjustment::Unadjusted,
        }
    }

    #[test]
    fn market_price_rejects_inverted_bounds() {
        let mut value = price("100");
        value.low = Decimal::from_str("101").expect("decimal");
        let def = crate::temporal::catalog::fact_type("market_price").expect("catalog");
        let error = FactValue::MarketPrice(value)
            .validate(def)
            .expect_err("bounds");
        assert!(error.to_string().contains("high"));
    }

    #[test]
    fn decimal_normalization_makes_the_hash_format_independent() {
        let subject =
            FactSubject::Asset(crate::domain::id::AssetId::from_uuid(uuid::Uuid::new_v4()));
        let valid_at = ValidAt::midnight_utc(NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"));
        let a = FactValue::Decimal {
            value: Decimal::from_str("1.0").expect("decimal"),
        };
        let b = FactValue::Decimal {
            value: Decimal::from_str("1.00").expect("decimal"),
        };
        assert_eq!(
            content_hash(subject, "nav", valid_at, "test", &a),
            content_hash(subject, "nav", valid_at, "test", &b)
        );
    }

    #[test]
    fn market_price_wire_shape_is_tagged() {
        let value = FactValue::MarketPrice(price("100"));
        let json = serde_json::to_value(&value).expect("json");
        assert_eq!(json["type"], "market_price");
        assert_eq!(json["close"], "100");
        assert_eq!(json["currency"], "usd");
    }

    #[test]
    fn adjustment_wire_values_round_trip() {
        for adjustment in [
            Adjustment::Adjusted,
            Adjustment::Unadjusted,
            Adjustment::None,
        ] {
            assert_eq!(Adjustment::from_wire(adjustment.as_str()), Some(adjustment));
            assert_eq!(adjustment.to_string(), adjustment.as_str());
        }
        assert_eq!(Adjustment::from_wire("split"), None);
        assert_eq!(
            serde_json::to_value(Adjustment::Unadjusted).expect("json"),
            serde_json::json!("unadjusted")
        );
    }

    #[test]
    fn market_price_normalizes_currency_and_canonicalizes() {
        let value = price("100");
        let normalized = value.clone().normalize();
        assert_eq!(normalized.currency, "USD");
        let canonical = normalized.canonical();
        assert!(canonical.contains("open=100"));
        assert!(canonical.contains("adjclose="));
        assert!(canonical.contains("volume=1"));
        assert!(canonical.contains("currency=USD"));
        assert!(canonical.contains("adjustment=unadjusted"));

        let mut without_optionals = normalized;
        without_optionals.adjclose = None;
        without_optionals.volume = None;
        let canonical = without_optionals.canonical();
        assert!(canonical.contains("adjclose=;"));
        assert!(canonical.contains("volume=;"));
    }

    #[test]
    fn storage_kind_covers_every_value_variant() {
        let values = [
            (FactValue::Text { value: "x".into() }, StorageKind::Text),
            (
                FactValue::Decimal {
                    value: Decimal::ONE,
                },
                StorageKind::Decimal,
            ),
            (FactValue::Integer { value: 1 }, StorageKind::Integer),
            (FactValue::Boolean { value: true }, StorageKind::Boolean),
            (
                FactValue::Date {
                    value: NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"),
                },
                StorageKind::Date,
            ),
            (
                FactValue::Timestamp {
                    value: DateTime::from_timestamp(0, 0).expect("epoch"),
                },
                StorageKind::Timestamp,
            ),
            (
                FactValue::MarketPrice(price("1")),
                StorageKind::MarketObservation,
            ),
        ];
        for (value, kind) in values {
            assert_eq!(value.storage_kind(), kind);
        }
    }

    #[test]
    fn canonical_representation_covers_every_value_variant() {
        let text = FactValue::Text {
            value: "abc".into(),
        };
        assert_eq!(text.canonical(), "text:3:abc");
        assert_eq!(
            FactValue::Decimal {
                value: Decimal::from_str("1.50").expect("decimal"),
            }
            .canonical(),
            "decimal:1.5"
        );
        assert_eq!(FactValue::Integer { value: -7 }.canonical(), "integer:-7");
        assert_eq!(
            FactValue::Boolean { value: false }.canonical(),
            "boolean:false"
        );
        assert_eq!(
            FactValue::Date {
                value: NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"),
            }
            .canonical(),
            "date:2026-01-02"
        );
        assert!(
            FactValue::Timestamp {
                value: DateTime::from_timestamp(0, 0).expect("epoch"),
            }
            .canonical()
            .starts_with("timestamp:1970-01-01")
        );
        assert!(
            FactValue::MarketPrice(price("100"))
                .canonical()
                .starts_with("market_price:")
        );
    }

    #[test]
    fn scalar_validation_accepts_matching_catalog_types() {
        let cases = [
            (
                "name",
                FactValue::Text {
                    value: "Apple".into(),
                },
            ),
            (
                "nav",
                FactValue::Decimal {
                    value: Decimal::ONE,
                },
            ),
            ("rank", FactValue::Integer { value: 1 }),
            ("is_active", FactValue::Boolean { value: true }),
            (
                "inception_date",
                FactValue::Date {
                    value: NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"),
                },
            ),
            (
                "last_verified_at",
                FactValue::Timestamp {
                    value: DateTime::from_timestamp(0, 0).expect("epoch"),
                },
            ),
            ("market_price", FactValue::MarketPrice(price("100"))),
        ];
        for (fact_type, value) in cases {
            let def = crate::temporal::catalog::fact_type(fact_type).expect("catalog");
            value.validate(def).expect("valid value");
        }
    }

    #[test]
    fn scalar_validation_rejects_wrong_type_and_empty_text() {
        let def = crate::temporal::catalog::fact_type("nav").expect("catalog");
        let mismatch = FactValue::Text {
            value: "not a decimal".into(),
        }
        .validate(def)
        .expect_err("type mismatch");
        assert!(mismatch.to_string().contains("expected decimal"));

        let def = crate::temporal::catalog::fact_type("name").expect("catalog");
        let empty = FactValue::Text {
            value: "   ".into(),
        }
        .validate(def)
        .expect_err("empty text");
        assert!(empty.to_string().contains("must not be empty"));
    }

    static CUSTOM_FIELDS: &[FactFieldDef] = &[FactFieldDef {
        name: "value",
        data_type: "decimal",
        nullable: false,
        unit: None,
        precision: Some(6),
        scale: Some(2),
        min_value: Some("0"),
        max_value: Some("10"),
        allowed_values: &[],
        aggregates: &[],
        description: "Bounded decimal.",
    }];

    static CUSTOM_TEXT_FIELDS: &[FactFieldDef] = &[FactFieldDef {
        name: "value",
        data_type: "text",
        nullable: false,
        unit: None,
        precision: None,
        scale: None,
        min_value: None,
        max_value: None,
        allowed_values: &["alpha", "beta"],
        aggregates: &[],
        description: "Closed vocabulary.",
    }];

    fn custom_def(fields: &'static [FactFieldDef], kind: StorageKind) -> FactTypeDef {
        FactTypeDef {
            fact_type: "custom",
            display_name: "Custom",
            description: "Test definition.",
            storage_kind: kind,
            aggregation: "last",
            unit: None,
            observation_kind: None,
            interval_seconds: None,
            schema_version: 1,
            subjects: &[],
            fields,
        }
    }

    #[test]
    fn decimal_validation_enforces_range_and_scale() {
        let def = custom_def(CUSTOM_FIELDS, StorageKind::Decimal);
        FactValue::Decimal {
            value: Decimal::from_str("5").expect("decimal"),
        }
        .validate(&def)
        .expect("in range");

        let too_large = FactValue::Decimal {
            value: Decimal::from_str("11").expect("decimal"),
        }
        .validate(&def)
        .expect_err("maximum");
        assert!(too_large.to_string().contains("above maximum"));

        let too_small = FactValue::Decimal {
            value: Decimal::from_str("-1").expect("decimal"),
        }
        .validate(&def)
        .expect_err("minimum");
        assert!(too_small.to_string().contains("below minimum"));

        let too_precise = FactValue::Decimal {
            value: Decimal::from_str("1.234").expect("decimal"),
        }
        .validate(&def)
        .expect_err("scale");
        assert!(too_precise.to_string().contains("scale"));
    }

    #[test]
    fn text_validation_enforces_the_allowed_vocabulary() {
        let def = custom_def(CUSTOM_TEXT_FIELDS, StorageKind::Text);
        FactValue::Text {
            value: "alpha".into(),
        }
        .validate(&def)
        .expect("allowed");
        let rejected = FactValue::Text {
            value: "gamma".into(),
        }
        .validate(&def)
        .expect_err("not allowed");
        assert!(rejected.to_string().contains("not one of"));
    }

    #[test]
    fn a_fact_type_without_fields_is_invalid() {
        let def = custom_def(&[], StorageKind::Text);
        let error = FactValue::Text { value: "x".into() }
            .validate(&def)
            .expect_err("no fields");
        assert!(error.to_string().contains("no declared fields"));
    }

    #[test]
    fn market_price_validation_rejects_each_invariant() {
        let def = crate::temporal::catalog::fact_type("market_price").expect("catalog");

        let mut negative = price("100");
        negative.open = Decimal::NEGATIVE_ONE;
        assert!(
            FactValue::MarketPrice(negative)
                .validate(def)
                .expect_err("negative")
                .to_string()
                .contains("open")
        );

        let mut negative = price("100");
        negative.high = Decimal::NEGATIVE_ONE;
        assert!(FactValue::MarketPrice(negative).validate(def).is_err());

        let mut negative = price("100");
        negative.low = Decimal::NEGATIVE_ONE;
        assert!(FactValue::MarketPrice(negative).validate(def).is_err());

        let mut negative = price("100");
        negative.close = Decimal::NEGATIVE_ONE;
        assert!(FactValue::MarketPrice(negative).validate(def).is_err());

        let mut open_outside = price("100");
        open_outside.open = Decimal::from_str("101").expect("decimal");
        assert!(
            FactValue::MarketPrice(open_outside)
                .validate(def)
                .expect_err("open")
                .to_string()
                .contains("open must be between")
        );

        let mut close_outside = price("100");
        close_outside.close = Decimal::from_str("99").expect("decimal");
        assert!(
            FactValue::MarketPrice(close_outside)
                .validate(def)
                .expect_err("close")
                .to_string()
                .contains("close must be between")
        );

        let mut negative_adjclose = price("100");
        negative_adjclose.adjclose = Some(Decimal::NEGATIVE_ONE);
        assert!(
            FactValue::MarketPrice(negative_adjclose)
                .validate(def)
                .is_err()
        );

        let mut negative_volume = price("100");
        negative_volume.volume = Some(Decimal::NEGATIVE_ONE);
        assert!(
            FactValue::MarketPrice(negative_volume)
                .validate(def)
                .is_err()
        );

        let mut short_currency = price("100");
        short_currency.currency = "US".into();
        assert!(
            FactValue::MarketPrice(short_currency)
                .validate(def)
                .is_err()
        );

        let mut numeric_currency = price("100");
        numeric_currency.currency = "U1D".into();
        assert!(
            FactValue::MarketPrice(numeric_currency)
                .validate(def)
                .is_err()
        );
    }

    #[test]
    fn every_value_variant_round_trips_through_json() {
        let values = vec![
            FactValue::Text {
                value: "abc".into(),
            },
            FactValue::Decimal {
                value: Decimal::from_str("1.50").expect("decimal"),
            },
            FactValue::Integer { value: -3 },
            FactValue::Boolean { value: true },
            FactValue::Date {
                value: NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"),
            },
            FactValue::Timestamp {
                value: DateTime::from_timestamp(0, 0).expect("epoch"),
            },
            FactValue::MarketPrice(price("100")),
        ];
        for value in values {
            let json = serde_json::to_value(&value).expect("serialize");
            let round_trip: FactValue = serde_json::from_value(json).expect("deserialize");
            assert_eq!(round_trip, value);
        }
    }

    #[test]
    fn malformed_number_strings_are_rejected_on_deserialize() {
        let bad_decimal = serde_json::from_value::<FactValue>(serde_json::json!({
            "type": "decimal",
            "value": "not-a-number"
        }));
        assert!(bad_decimal.is_err());

        let bad_integer = serde_json::from_value::<FactValue>(serde_json::json!({
            "type": "integer",
            "value": "not-a-number"
        }));
        assert!(bad_integer.is_err());

        let bad_optional = serde_json::from_value::<FactValue>(serde_json::json!({
            "type": "market_price",
            "open": "1",
            "high": "1",
            "low": "1",
            "close": "1",
            "adjclose": "nope",
            "currency": "USD",
            "adjustment": "unadjusted"
        }));
        assert!(bad_optional.is_err());
    }

    #[test]
    fn content_and_retraction_hashes_are_distinct_and_stable() {
        let subject =
            FactSubject::Asset(crate::domain::id::AssetId::from_uuid(uuid::Uuid::new_v4()));
        let valid_at = ValidAt::midnight_utc(NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"));
        let value = FactValue::Text {
            value: "abc".into(),
        };
        let content = content_hash(subject, "name", valid_at, "src", &value);
        assert_eq!(
            content,
            content_hash(subject, "name", valid_at, "src", &value)
        );
        let retraction = retraction_hash(subject, "name", valid_at, "src", "gone");
        assert_ne!(content, retraction);
        assert_eq!(
            retraction,
            retraction_hash(subject, "name", valid_at, "src", "gone")
        );
    }
}
