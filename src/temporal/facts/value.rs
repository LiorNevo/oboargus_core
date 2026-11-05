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
}
