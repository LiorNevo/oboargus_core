//! Shared normalization of upstream lookup hits into the load vocabulary.
//!
//! The identifier search (spec 026) must show the exact identifier that the
//! reference-data load (spec 018) would resolve and create. Rather than
//! duplicate the mapping in two places, both operations use these helpers.
//!
//! Implements FR-007 / FR-011 / FR-019 of
//! `specs/026-identifier-search/spec.md`.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

use crate::market_data::types::{TickerLookupHit, TypedIdentifier};

/// Where a created asset's currency came from.
///
/// Implements FR-015 of spec 018.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum CurrencySource {
    /// The upstream lookup returned a currency.
    Upstream,
    /// The upstream omitted it and the configured default was used.
    ConfiguredDefault,
}

/// Whether a lookup hit trades on the identifier's venue.
///
/// Implements FR-015 of spec 018.
pub fn hit_matches_venue(hit: &TickerLookupHit, venue: &str) -> bool {
    exchange_code_mic(hit.exchange.as_deref()).is_some_and(|mic| mic.eq_ignore_ascii_case(venue))
}

/// Maps an upstream exchange code to a MIC.
///
/// The upstream search endpoint reports venue codes such as `NMS`, not the
/// MICs the identifier schemes use; an unmapped code is treated as
/// unverifiable, never as a match.
///
/// Implements FR-015 of spec 018.
pub fn exchange_code_mic(exchange: Option<&str>) -> Option<&'static str> {
    let code = exchange?.trim().to_ascii_uppercase();
    match code.as_str() {
        "NMS" | "NGM" | "NCM" | "NAS" => Some("XNAS"),
        "NYQ" | "NYS" => Some("XNYS"),
        "PCX" | "PSE" => Some("ARCX"),
        "ASE" | "AMEX" => Some("XASE"),
        "BTS" | "BATS" => Some("BATS"),
        "OTC" | "PNK" | "OQX" | "OB" => Some("OTC"),
        _ => None,
    }
}

/// Canonicalizes an identifier before lookup and storage.
///
/// Ticker values are case-insensitive in practice, so ticker-like schemes are
/// trimmed and uppercased. Other schemes keep their existing exact-match
/// semantics.
///
/// Implements FR-011 of `specs/026-identifier-search/spec.md`.
pub fn canonical_identifier(identifier: &TypedIdentifier) -> TypedIdentifier {
    let scheme = identifier.scheme.trim().to_uppercase();
    let value = if ticker_venue(&scheme).is_some() {
        identifier.value.trim().to_uppercase()
    } else {
        identifier.value.trim().to_string()
    };
    TypedIdentifier::new(scheme, value)
}

/// The venue an identifier's scheme names, if it is a ticker scheme.
///
/// `None` means "not a ticker scheme"; `Some(None)` means a bare `TICKER`;
/// `Some(Some(mic))` means a venue-qualified `TICKER:<MIC>`.
///
/// Implements FR-011 of `specs/026-identifier-search/spec.md`.
pub fn ticker_venue(scheme: &str) -> Option<Option<&str>> {
    let scheme = scheme.trim();
    if scheme.eq_ignore_ascii_case("TICKER") {
        return Some(None);
    }
    let (prefix, venue) = scheme.split_once(':')?;
    if prefix.eq_ignore_ascii_case("TICKER") && !venue.trim().is_empty() {
        Some(Some(venue.trim()))
    } else {
        None
    }
}

/// Whether a value is a plausible ticker symbol.
///
/// Implements FR-015 of spec 018.
pub fn is_plausible_symbol(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 20
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// Maps an upstream quote type onto the canonical asset-type vocabulary.
///
/// An unknown quote type is refused rather than guessed: a wrong asset type
/// is worse than an unresolved identifier.
///
/// Implements FR-015 of spec 018.
pub fn canonical_asset_type(quote_type: Option<&str>) -> Option<&'static str> {
    let quote_type = quote_type?.trim().to_ascii_uppercase();
    match quote_type.as_str() {
        "EQUITY" | "STOCK" => Some("EQUITY"),
        "ETF" => Some("ETF"),
        "MUTUALFUND" | "MUTUAL_FUND" | "MUTUAL FUND" => Some("MUTUALFUND"),
        "INDEX" => Some("INDEX"),
        "CURRENCY" => Some("CURRENCY"),
        "CRYPTOCURRENCY" | "CRYPTO" => Some("CRYPTO"),
        "FUTURE" => Some("FUTURE"),
        _ => None,
    }
}

/// Derives a created asset's display name.
///
/// Implements FR-015 of spec 018.
pub fn derive_display_name(hit: &TickerLookupHit) -> String {
    hit.short_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .or_else(|| {
            hit.long_name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
        })
        .unwrap_or_else(|| hit.symbol.trim())
        .to_string()
}

/// Derives a created asset's currency and records its provenance.
///
/// Implements FR-015 of spec 018.
pub fn derive_currency(hit: &TickerLookupHit, default_currency: &str) -> (String, CurrencySource) {
    match hit
        .currency
        .as_deref()
        .map(str::trim)
        .filter(|currency| !currency.is_empty())
    {
        Some(currency) => (currency.to_ascii_uppercase(), CurrencySource::Upstream),
        None => (
            default_currency.trim().to_ascii_uppercase(),
            CurrencySource::ConfiguredDefault,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup_hit(symbol: &str, exchange: &str) -> TickerLookupHit {
        TickerLookupHit {
            symbol: symbol.to_string(),
            short_name: Some("Short Name".to_string()),
            long_name: Some("A Longer Name".to_string()),
            quote_type: Some("ETF".to_string()),
            exchange: Some(exchange.to_string()),
            currency: None,
            schemes: Vec::new(),
            relevance: Some(1.0),
        }
    }

    /// Only ticker schemes are eligible, and a venue qualifier is retained.
    #[test]
    fn ticker_venue_classifies_schemes() {
        assert_eq!(ticker_venue("TICKER"), Some(None));
        assert_eq!(ticker_venue("ticker"), Some(None));
        assert_eq!(ticker_venue("TICKER:XNAS"), Some(Some("XNAS")));
        assert_eq!(ticker_venue("ticker:xnas"), Some(Some("xnas")));
        assert_eq!(ticker_venue("ISIN"), None);
        assert_eq!(ticker_venue("CUSIP"), None);
        assert_eq!(ticker_venue("TICKER:"), None);
    }

    /// A plausible symbol is short and ASCII; anything else is refused.
    #[test]
    fn symbols_are_validated_conservatively() {
        assert!(is_plausible_symbol("AAPL"));
        assert!(is_plausible_symbol("BRK.B"));
        assert!(is_plausible_symbol("RDS-A"));
        assert!(!is_plausible_symbol(""));
        assert!(!is_plausible_symbol("   "));
        assert!(!is_plausible_symbol("AAPL; DROP TABLE"));
        assert!(!is_plausible_symbol(&"A".repeat(21)));
    }

    /// The asset-type map is canonical and closed.
    #[test]
    fn asset_types_map_to_a_closed_vocabulary() {
        assert_eq!(canonical_asset_type(Some("EQUITY")), Some("EQUITY"));
        assert_eq!(canonical_asset_type(Some("etf")), Some("ETF"));
        assert_eq!(canonical_asset_type(Some("MUTUALFUND")), Some("MUTUALFUND"));
        assert_eq!(canonical_asset_type(Some("INDEX")), Some("INDEX"));
        assert_eq!(canonical_asset_type(None), None);
        assert_eq!(canonical_asset_type(Some("SOMETHING_NEW")), None);
    }

    /// Display name prefers the short name, then the long name, then the
    /// symbol.
    #[test]
    fn display_name_falls_back_in_order() {
        let mut hit = lookup_hit("QQQM", "NMS");
        assert_eq!(derive_display_name(&hit), "Short Name");
        hit.short_name = Some("  ".to_string());
        assert_eq!(derive_display_name(&hit), "A Longer Name");
        hit.long_name = None;
        assert_eq!(derive_display_name(&hit), "QQQM");
    }

    /// Currency provenance is recorded, never guessed silently.
    #[test]
    fn currency_records_its_provenance() {
        let mut hit = lookup_hit("QQQM", "NMS");
        assert_eq!(
            derive_currency(&hit, "usd"),
            ("USD".to_string(), CurrencySource::ConfiguredDefault)
        );
        hit.currency = Some("eur".to_string());
        assert_eq!(
            derive_currency(&hit, "USD"),
            ("EUR".to_string(), CurrencySource::Upstream)
        );
    }

    /// Upstream exchange codes map onto MICs; unknown codes do not.
    #[test]
    fn exchanges_map_to_mics() {
        assert_eq!(exchange_code_mic(Some("NMS")), Some("XNAS"));
        assert_eq!(exchange_code_mic(Some("nyq")), Some("XNYS"));
        assert_eq!(exchange_code_mic(Some("PCX")), Some("ARCX"));
        assert_eq!(exchange_code_mic(Some("GER")), None);
        assert_eq!(exchange_code_mic(None), None);
    }

    /// A hit matches a venue only through a known MIC.
    #[test]
    fn hit_matches_a_known_venue() {
        let hit = lookup_hit("AAPL", "NMS");
        assert!(hit_matches_venue(&hit, "XNAS"));
        assert!(hit_matches_venue(&hit, "xnas"));
        assert!(!hit_matches_venue(&hit, "XNYS"));
        let unmapped = lookup_hit("AAPL", "GER");
        assert!(!hit_matches_venue(&unmapped, "GER"));
    }

    /// Canonicalization uppercases ticker schemes only.
    #[test]
    fn identifiers_are_canonicalized_by_scheme() {
        let ticker = canonical_identifier(&TypedIdentifier::new(" ticker:xnas ", " aapl "));
        assert_eq!(ticker, TypedIdentifier::new("TICKER:XNAS", "AAPL"));
        let isin = canonical_identifier(&TypedIdentifier::new(" isin ", " us0378331005 "));
        assert_eq!(isin, TypedIdentifier::new("ISIN", "us0378331005"));
    }
}
