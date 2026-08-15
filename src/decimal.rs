//! Precise decimal parsing for monetary values.
//!
//! Financial math in Obolargus uses [`rust_decimal::Decimal`] — never
//! floating-point — to preserve precision. This module provides the small,
//! verified entry point the domain layers build on.

use std::fmt;
use std::str::FromStr;

use rust_decimal::Decimal;

/// Errors produced when parsing a decimal value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecimalParseError {
    /// The input is not a syntactically valid decimal number.
    InvalidInput,
}

impl fmt::Display for DecimalParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput => write!(f, "input is not a valid decimal number"),
        }
    }
}

impl std::error::Error for DecimalParseError {}

/// Parses a monetary value into a precise [`Decimal`].
///
/// Rejects syntactically invalid inputs (for example `"abc"`, `"NaN"`, or an
/// empty string) and returns a typed [`DecimalParseError`] instead of
/// panicking or producing an approximation.
///
/// # Examples
///
/// ```
/// use obolargus_core::decimal::parse_decimal;
///
/// let value = parse_decimal("123.45").unwrap();
/// assert_eq!(value.to_string(), "123.45");
///
/// assert!(parse_decimal("not-a-number").is_err());
/// ```
pub fn parse_decimal(value: &str) -> Result<Decimal, DecimalParseError> {
    Decimal::from_str(value).map_err(|_| DecimalParseError::InvalidInput)
}

#[cfg(test)]
mod tests {
    use super::parse_decimal;

    #[test]
    fn parses_valid_integer() {
        assert_eq!(parse_decimal("0").unwrap().to_string(), "0");
        assert_eq!(parse_decimal("42").unwrap().to_string(), "42");
    }

    #[test]
    fn parses_valid_fractional_value() {
        let value = parse_decimal("123.45").unwrap();
        assert_eq!(value.to_string(), "123.45");
        assert_eq!(value.scale(), 2);
    }

    #[test]
    fn parses_negative_value() {
        assert_eq!(parse_decimal("-7.50").unwrap().to_string(), "-7.50");
    }

    #[test]
    fn rejects_non_numeric_input() {
        assert_eq!(
            parse_decimal("abc").unwrap_err().to_string(),
            "input is not a valid decimal number"
        );
    }

    #[test]
    fn rejects_nan_like_input() {
        assert!(parse_decimal("NaN").is_err());
        assert!(parse_decimal("Infinity").is_err());
    }

    #[test]
    fn rejects_empty_and_blank_input() {
        assert!(parse_decimal("").is_err());
        assert!(parse_decimal("   ").is_err());
    }

    #[test]
    fn preserves_precision_over_floating_point() {
        let value = parse_decimal("0.1").unwrap() + parse_decimal("0.2").unwrap();
        assert_eq!(value.to_string(), "0.3");
    }
}
