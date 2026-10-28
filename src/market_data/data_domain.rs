//! Data-domain vocabulary for the Data Load page.
//!
//! The first release loads market/reference data. Holdings is declared so the
//! page, source catalog, and history are structurally ready for it without a
//! redesign, but it is not loadable yet.
//!
//! Implements FR-026 of `specs/022-market-data-load-ui/spec.md`.

#![forbid(unsafe_code)]

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A kind of data the system can load.
///
/// Wire values are `market_data` and `holdings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// Implements FR-026 / FR-028..FR-031 of spec 022.
pub enum DataDomain {
    /// Market and reference data (the loadable domain in this release).
    MarketData,
    /// Holdings (declared but not loadable in this release).
    Holdings,
}

impl DataDomain {
    /// The stable wire value.
    /// Implements FR-026 / FR-028..FR-031 of spec 022.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MarketData => "market_data",
            Self::Holdings => "holdings",
        }
    }

    /// Whether this domain can be loaded in the current release.
    /// Implements FR-026 / FR-028..FR-031 of spec 022.
    pub fn is_loadable(self) -> bool {
        matches!(self, Self::MarketData)
    }

    /// Human-readable label for the page.
    /// Implements FR-026 of spec 022.
    pub fn label(self) -> &'static str {
        match self {
            Self::MarketData => "Market & reference data",
            Self::Holdings => "Holdings",
        }
    }

    /// Why the domain cannot be loaded, when it cannot.
    /// Implements FR-026 of spec 022.
    pub fn unavailable_reason(self) -> Option<&'static str> {
        match self {
            Self::MarketData => None,
            Self::Holdings => Some("Holdings loading is not available in this release."),
        }
    }
}

impl fmt::Display for DataDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DataDomain {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "market_data" => Ok(Self::MarketData),
            "holdings" => Ok(Self::Holdings),
            other => Err(format!("unknown data domain '{other}'")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_data_is_loadable() {
        assert!(DataDomain::MarketData.is_loadable());
        assert_eq!(DataDomain::MarketData.unavailable_reason(), None);
    }

    #[test]
    fn holdings_is_declared_but_unavailable() {
        assert!(!DataDomain::Holdings.is_loadable());
        assert!(DataDomain::Holdings.unavailable_reason().is_some());
    }

    #[test]
    fn wire_values_round_trip() {
        assert_eq!(
            DataDomain::from_str("market_data").expect("market data"),
            DataDomain::MarketData
        );
        assert_eq!(
            DataDomain::from_str("holdings").expect("holdings"),
            DataDomain::Holdings
        );
        assert!(DataDomain::from_str("unknown").is_err());
        assert_eq!(
            serde_json::to_string(&DataDomain::MarketData).expect("serialize"),
            "\"market_data\""
        );
    }
}
