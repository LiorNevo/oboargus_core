//! The source-agnostic `MarketDataProvider` trait and per-provider input / output specs.
//!
//! Implements FR-001 / FR-002 / FR-005 / FR-010 / FR-041 of
//! `specs/017-yahoo-data-source/spec.md`.
//!
//! **Async-trait note**: this trait uses the `#[async_trait]` macro because
//! stable Rust does not yet allow `async fn` methods in a `dyn`-compatible
//! trait without expansion. Research D7 in `research.md` originally noted
//! "no `async_trait` macro" but that decision was incorrect — the
//! implementation requires it. The macro is widely used in the Rust
//! ecosystem and adds one transitive dependency (`async-trait = "0.1"`)
//! to `obolargus_core`. The previous decision would have forced the
//! registry to use enum dispatch or generic-based monomorphization,
//! both of which would explode the surface area.

#![forbid(unsafe_code)]

use std::fmt;

use async_trait::async_trait;

use crate::market_data::error::MarketDataProviderError;
use crate::market_data::types::{
    HistoricalRange, HistoricalRangeRequest, LatestBar, LatestBarRequest, TickerLookupRequest,
    TickerLookupResult,
};

/// A source of market data. Concrete implementations MUST be `Send + Sync`
/// so the registry can hold them behind a trait object and share them
/// across handlers / tasks.
///
/// Implements FR-001 / FR-002 / FR-005 / FR-010 of spec 017.
#[async_trait]
pub trait MarketDataProvider: Send + Sync {
    /// Stable, configuration-defined id (e.g. `"yahoo_finance"`).
    /// Used in audit rows, log lines, and configuration. MUST be unique
    /// across providers in a deployment (FR-003).
    fn source_id(&self) -> &str;

    /// Free-text label for operator-facing logs and UIs.
    fn display_name(&self) -> &str;

    /// The set of identifier schemes this provider can answer. Registry
    /// uses this to skip providers that cannot answer a given
    /// `(scheme, value)`. Examples: `["TICKER"]`, `["ISIN", "TICKER"]`,
    /// `["TICKER:*"]` (venue-qualified wildcards).
    fn supported_schemes(&self) -> &[String];

    /// Whether the provider is enabled for the current deployment.
    /// Registry skips disabled providers (FR-004).
    fn enabled(&self) -> bool;

    /// Numeric priority (lower = tried first). Ties broken by `source_id`
    /// ascending (alphabetical), per FR-002.
    fn priority(&self) -> i32;

    /// Fetch the latest available end-of-day bar for the given identifier.
    ///
    /// Errors: see [`MarketDataProviderError`]. Implementations MUST reject
    /// future-dated bars with [`MarketDataProviderError::InvalidInput`].
    async fn get_latest_bar(
        &self,
        request: LatestBarRequest,
    ) -> Result<LatestBar, MarketDataProviderError>;

    /// Fetch a historical range of end-of-day bars. Implementations MUST
    /// return bars in chronological order; bars whose `as_of_date >
    /// today()` MUST be rejected with
    /// [`MarketDataProviderError::InvalidInput`].
    async fn get_historical_range(
        &self,
        request: HistoricalRangeRequest,
    ) -> Result<HistoricalRange, MarketDataProviderError>;

    /// Look up an asset by free-text query (e.g. ticker or company-name
    /// fragment). Returns 0..N hits ordered by upstream relevance.
    /// Returns `Ok(…)` even when no hits match — an empty `hits` vec is
    /// **not** an error.
    async fn lookup_ticker(
        &self,
        request: TickerLookupRequest,
    ) -> Result<TickerLookupResult, MarketDataProviderError>;
}

impl fmt::Debug for dyn MarketDataProvider + Send + Sync {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MarketDataProvider {{ source_id: {}, display_name: {}, enabled: {}, priority: {} }}",
            self.source_id(),
            self.display_name(),
            self.enabled(),
            self.priority()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market_data::error::MarketDataProviderError;

    /// A minimal provider used to exercise the trait's ergonomics.
    struct Tiny;

    #[async_trait::async_trait]
    impl MarketDataProvider for Tiny {
        fn source_id(&self) -> &str {
            "tiny"
        }

        fn display_name(&self) -> &str {
            "Tiny Provider"
        }

        fn supported_schemes(&self) -> &[String] {
            &[]
        }

        fn enabled(&self) -> bool {
            true
        }

        fn priority(&self) -> i32 {
            5
        }

        async fn get_latest_bar(
            &self,
            _request: LatestBarRequest,
        ) -> Result<LatestBar, MarketDataProviderError> {
            Err(MarketDataProviderError::Unsupported("tiny".into()))
        }

        async fn get_historical_range(
            &self,
            _request: HistoricalRangeRequest,
        ) -> Result<HistoricalRange, MarketDataProviderError> {
            Err(MarketDataProviderError::Unsupported("tiny".into()))
        }

        async fn lookup_ticker(
            &self,
            _request: TickerLookupRequest,
        ) -> Result<TickerLookupResult, MarketDataProviderError> {
            Err(MarketDataProviderError::Unsupported("tiny".into()))
        }
    }

    /// The trait is object-safe: the `dyn MarketDataProvider + Send + Sync`
    /// binding must compile without a `where Self: Sized` constraint. This
    /// test is a build-check — passing means we can put concrete providers
    /// behind a trait object (which the registry does in practice).
    #[allow(dead_code)]
    fn _object_safety(_provider: Box<dyn MarketDataProvider + Send + Sync>) {
        let _ = _provider.source_id();
    }

    /// The `Debug` implementation names the provider's identity fields.
    #[test]
    fn debug_impl_names_the_identity_fields() {
        let provider: Box<dyn MarketDataProvider + Send + Sync> = Box::new(Tiny);
        let rendered = format!("{provider:?}");
        assert!(rendered.contains("source_id: tiny"), "got: {rendered}");
        assert!(
            rendered.contains("display_name: Tiny Provider"),
            "got: {rendered}"
        );
        assert!(rendered.contains("enabled: true"), "got: {rendered}");
        assert!(rendered.contains("priority: 5"), "got: {rendered}");
    }
}
