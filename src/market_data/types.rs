//! Per-call input / output structs for the source-agnostic market data interface.
//!
//! Implements FR-001 of `specs/017-yahoo-data-source/spec.md`.

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// A typed identifier naming an asset / issuer / portfolio outside the system.
///
/// The open vocabulary is the same as the predecessor's
/// `domain::asset_identifier::AssetIdentifier::scheme` (see spec 016,
/// FR-023..FR-026). Examples: `"TICKER"`, `"ISIN"`, `"CUSIP"`, and the
/// venue-qualified form `"TICKER:XNAS"`.
///
/// The wire surface of feature 018 calls this field the identifier's
/// **type**; the stored representation calls it the `scheme`. One concept,
/// two names (spec 018 FR-002).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TypedIdentifier {
    /// Scheme (open vocabulary; matched case-sensitively per registry rules).
    pub scheme: String,
    /// Value of the identifier (e.g. `"AAPL"`, `"US0378331005"`).
    pub value: String,
}

impl TypedIdentifier {
    /// Constructs a new `TypedIdentifier`.
    ///
    /// Implements FR-001 of spec 017.
    pub fn new(scheme: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            scheme: scheme.into(),
            value: value.into(),
        }
    }
}

/// Request for a single end-of-day bar.
///
/// Implements FR-001 of spec 017.
#[derive(Debug, Clone)]
pub struct LatestBarRequest {
    /// The asset's external identifier (the registry will route this).
    pub identifier: TypedIdentifier,
    /// Logical "current time" so tests can pin the clock.
    pub as_of_now: DateTime<Utc>,
}

impl LatestBarRequest {
    /// Convenience constructor for the common case (now is `Utc::now()`).
    ///
    /// Implements FR-001 of spec 017.
    pub fn now(identifier: TypedIdentifier) -> Self {
        Self {
            identifier,
            as_of_now: Utc::now(),
        }
    }
}

/// Request for a historical range of bars.
///
/// Implements FR-001 of spec 017.
#[derive(Debug, Clone)]
pub struct HistoricalRangeRequest {
    /// The asset's external identifier.
    pub identifier: TypedIdentifier,
    /// Inclusive lower bound (date).
    pub from: NaiveDate,
    /// Inclusive upper bound (date).
    pub to: NaiveDate,
}

/// Request for a free-text ticker / company-name lookup.
///
/// Implements FR-012 of spec 017 (US2).
#[derive(Debug, Clone)]
pub struct TickerLookupRequest {
    /// Free-text query (ticker or company name fragment).
    pub query: String,
}

/// A single end-of-day OHLCV bar.
///
/// Prices are `rust_decimal::Decimal` everywhere — never `f64` or `f32` —
/// per Constitution Principle X.
///
/// Implements FR-013 of spec 017.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceBar {
    /// Trading day the bar refers to.
    pub as_of_date: NaiveDate,
    /// Open price for the bar.
    pub open: Decimal,
    /// High price for the bar.
    pub high: Decimal,
    /// Low price for the bar.
    pub low: Decimal,
    /// Close price for the bar.
    pub close: Decimal,
    /// Adjusted close (split/dividend-adjusted; Yahoo's `adjclose`).
    pub adjclose: Decimal,
    /// Volume in shares / contracts.
    pub volume: u64,
}

/// Result of a `get_latest_bar` call.
///
/// Implements FR-001 / FR-013 of spec 017.
#[derive(Debug, Clone)]
pub struct LatestBar {
    /// The bar.
    pub bar: PriceBar,
    /// When the system learned this bar (typically `today()` at the
    /// `run_refresh` call site). Dual-temporal `knowledge_date`.
    pub knowledge_date: NaiveDate,
    /// Source id of the provider that returned this bar.
    pub source_id: String,
}

/// Result of a `get_historical_range` call.
///
/// Implements FR-001 of spec 017.
#[derive(Debug, Clone)]
pub struct HistoricalRange {
    /// Bars in chronological order.
    pub bars: Vec<PriceBar>,
    /// When the system learned these bars (typically `today()`).
    pub knowledge_date: NaiveDate,
    /// Source id of the provider that returned these bars.
    pub source_id: String,
}

/// One hit returned by a ticker lookup.
///
/// Implements FR-012 of spec 017 (US2).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TickerLookupHit {
    /// Raw ticker symbol (e.g. `"MSFT"`).
    pub symbol: String,
    /// Short name (if upstream provided one).
    pub short_name: Option<String>,
    /// Long name (if upstream provided one).
    pub long_name: Option<String>,
    /// Quote type (e.g. `"EQUITY"`, `"ETF"`).
    pub quote_type: Option<String>,
    /// Exchange MIC or similar (Yahoo uses values like `"NMS"` for NASDAQ).
    pub exchange: Option<String>,
    /// ISO 4217 currency. Yahoo's search endpoint does not provide this;
    /// requires a separate quote-summary call. Surfaces as `None` in v1.
    pub currency: Option<String>,
    /// Additional identifier schemes returned by the upstream (e.g. an ISIN).
    pub schemes: Vec<(String, String)>,
    /// Relevance score (Yahoo's `score`). Higher is more relevant.
    pub relevance: Option<f64>,
}

/// Result of a `lookup_ticker` call.
///
/// Implements FR-012 of spec 017 (US2).
#[derive(Debug, Clone)]
pub struct TickerLookupResult {
    /// Hits (0..N, ordered by upstream relevance).
    pub hits: Vec<TickerLookupHit>,
    /// Source id of the provider that returned these hits.
    pub source_id: String,
}
