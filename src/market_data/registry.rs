//! `DataSourceRegistry` — the consumer-facing façade over configured providers.
//!
//! Implements FR-001 / FR-002 / FR-003 / FR-004 / FR-005 / FR-006 / FR-007
//! of `specs/017-yahoo-data-source/spec.md`. The registry holds
//! `Box<dyn MarketDataProvider + Send + Sync>` trait objects and walks them
//! in `(priority ASC, source_id ASC)` order, applying the failover policy
//! (Transient / RateLimited fall over; the rest are final).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Serialize;

use crate::market_data::error::MarketDataProviderError;
use crate::market_data::trait_::MarketDataProvider;
use crate::market_data::types::{
    HistoricalRange, HistoricalRangeRequest, LatestBar, LatestBarRequest, TickerLookupHit,
    TickerLookupRequest, TickerLookupResult,
};

/// Lightweight, serializable description of one configured source.
///
/// Returned by [`DataSourceRegistry::enumerate_sources`] (FR-002).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SourceDescriptor {
    /// Stable id (e.g. `"yahoo_finance"`).
    pub source_id: String,
    /// Human-readable name.
    pub display_name: String,
    /// Enabled flag at the time of enumeration.
    pub enabled: bool,
    /// Numeric priority (lower = tried first).
    pub priority: i32,
    /// Identifier schemes this source can answer.
    pub supported_schemes: Vec<String>,
}

/// One source's failure during a registry walk.
///
/// The orchestrator records these in `data_source_refresh_runs.per_source_failed`
/// so the audit names the source that failed (FR-032).
///
/// Implements FR-007 / FR-032 of spec 017.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFailure {
    /// The stable id of the source that failed.
    pub source_id: String,
    /// The typed error it returned.
    pub error: MarketDataProviderError,
}

/// The outcome of a registry walk.
///
/// Carries the answer (when a source produced one), the typed error that
/// ended the walk (when it did not), and every source that failed along
/// the way. The failure list is what the refresh orchestrator records in
/// `data_source_refresh_runs.per_source_failed` (FR-032).
///
/// Implements FR-005 / FR-007 / FR-032 of spec 017.
#[derive(Debug, Clone)]
pub struct WalkOutcome {
    /// The answer, when a source produced one.
    pub bar: Option<LatestBar>,
    /// The typed error that ended the walk, when it produced no answer.
    pub error: Option<MarketDataProviderError>,
    /// Every source that failed during the walk, in the order consulted.
    pub failures: Vec<SourceFailure>,
}

/// One exact-symbol match produced by a lookup walk.
///
/// Implements FR-014 of `specs/018-load-reference-data/spec.md`.
#[derive(Debug, Clone, PartialEq)]
pub struct TickerLookupMatch {
    /// The source that returned the hit.
    pub source_id: String,
    /// The hit whose symbol matched the query exactly.
    pub hit: TickerLookupHit,
}

/// The outcome of a ticker-lookup walk.
///
/// `matches` carries every exact-symbol hit the first answering source
/// produced — zero (nothing recognized the symbol), one (unambiguous), or
/// several (the caller must refuse to guess).
///
/// Implements FR-014 of `specs/018-load-reference-data/spec.md`.
#[derive(Debug, Clone)]
pub struct TickerLookupWalk {
    /// Exact-symbol matches, in the answering source's order.
    pub matches: Vec<TickerLookupMatch>,
    /// The typed error that ended the walk, when it produced no match.
    pub error: Option<MarketDataProviderError>,
    /// Every source that failed during the walk, in the order consulted.
    pub failures: Vec<SourceFailure>,
}

/// The outcome of a ranged registry walk.
///
/// Mirrors [`WalkOutcome`] for historical-range requests, so the caller can
/// tell "no enabled source supports this scheme" apart from "a source was
/// tried and failed".
///
/// Implements FR-009 of `specs/018-load-reference-data/spec.md`.
#[derive(Debug, Clone)]
pub struct RangeWalkOutcome {
    /// The answer, when a source produced one.
    pub range: Option<HistoricalRange>,
    /// The typed error that ended the walk, when it produced no answer.
    pub error: Option<MarketDataProviderError>,
    /// Every source that failed during the walk, in the order consulted.
    pub failures: Vec<SourceFailure>,
}

/// Source-agnostic façade. Constructed at startup with one trait object per
/// configured source. Consumers call the registry, not individual providers.
///
/// Implements US3 of spec 017.
pub struct DataSourceRegistry {
    /// Enabled providers only, sorted by `(priority ASC, source_id ASC)`.
    /// Disabled providers are filtered out at construction time.
    enabled: Vec<Arc<dyn MarketDataProvider + Send + Sync>>,
    /// All providers (enabled or disabled), for enumeration. Sorted by
    /// `source_id ASC`.
    all: Vec<Arc<dyn MarketDataProvider + Send + Sync>>,
}

impl DataSourceRegistry {
    /// Construct from a vec of providers. Validates unique `source_id`
    /// (FR-003); fails fast on duplicates with a typed error.
    ///
    /// Disabled providers are filtered out of the walking list but kept
    /// for `enumerate_sources` (FR-002). If the enabled list is empty
    /// AND at least one provider was supplied, returns
    /// `Validation` (per the FR-005 reconciliation Edge Cases entry).
    pub fn new(
        providers: Vec<Arc<dyn MarketDataProvider + Send + Sync>>,
    ) -> Result<Self, MarketDataProviderError> {
        // Duplicate detection
        let mut ids_seen = std::collections::BTreeSet::new();
        for p in &providers {
            if !ids_seen.insert(p.source_id().to_string()) {
                return Err(MarketDataProviderError::Validation(format!(
                    "duplicate source_id '{}'",
                    p.source_id()
                )));
            }
        }
        // All providers (sorted by id ASC for enumeration)
        let mut all = providers;
        all.sort_by(|a, b| a.source_id().cmp(b.source_id()));

        // Enabled providers (sorted by (priority ASC, source_id ASC) for
        // walking; ties broken alphabetically)
        let mut enabled: Vec<Arc<dyn MarketDataProvider + Send + Sync>> =
            all.iter().filter(|p| p.enabled()).cloned().collect();
        enabled.sort_by(|a, b| {
            a.priority()
                .cmp(&b.priority())
                .then_with(|| a.source_id().cmp(b.source_id()))
        });

        Ok(Self { enabled, all })
    }

    /// List every configured source, enabled or not, in `source_id ASC`
    /// order (FR-002). Disabled sources appear with `enabled: false`.
    pub fn enumerate_sources(&self) -> Vec<SourceDescriptor> {
        self.all
            .iter()
            .map(|p| SourceDescriptor {
                source_id: p.source_id().to_string(),
                display_name: p.display_name().to_string(),
                enabled: p.enabled(),
                priority: p.priority(),
                supported_schemes: p.supported_schemes().to_vec(),
            })
            .collect()
    }

    /// Returns the count of enabled sources.
    ///
    /// Implements FR-002 of spec 017 (reconciliation requires >= 2).
    pub fn enabled_count(&self) -> usize {
        self.enabled.len()
    }

    /// "First success" walk. Walks enabled providers in
    /// `(priority, source_id)` order; calls the first one whose
    /// `supported_schemes` contains the request's scheme; on
    /// `Transient` / `RateLimited` falls over; on `NotFound` /
    /// `InvalidInput` / `Unsupported` / `Unauthorized` /
    /// `Infrastructure` returns immediately. If no enabled source
    /// matches the request's scheme, returns `Err(NotFound)`. If every
    /// enabled source returns a retryable error, surfaces the LAST
    /// encountered retryable error (preserved upstream message).
    ///
    /// Implements FR-005 of spec 017.
    pub async fn get_latest_bar(
        &self,
        request: LatestBarRequest,
    ) -> Result<LatestBar, MarketDataProviderError> {
        let outcome = self.walk_latest_bar(request).await;
        match (outcome.bar, outcome.error) {
            (Some(bar), _) => Ok(bar),
            (None, Some(error)) => Err(error),
            (None, None) => Err(MarketDataProviderError::Infrastructure(
                "registry walked all enabled candidates without a final answer".into(),
            )),
        }
    }

    /// The "first success" walk, plus the per-source failures it
    /// encountered along the way.
    ///
    /// The outcome always carries the failure list — including when the
    /// walk produced no answer — so the refresh orchestrator can record
    /// the failing source ids in `per_source_failed` (FR-032). The audit
    /// names the *source* that failed, not an opaque marker.
    ///
    /// Implements FR-005 / FR-007 / FR-032 of spec 017.
    pub async fn walk_latest_bar(&self, request: LatestBarRequest) -> WalkOutcome {
        let candidates = self.candidates_for(&request.identifier.scheme);
        if candidates.is_empty() {
            return WalkOutcome {
                bar: None,
                error: Some(MarketDataProviderError::NotFound(format!(
                    "no enabled source supports scheme '{}'",
                    request.identifier.scheme
                ))),
                failures: Vec::new(),
            };
        }
        let mut last_retryable: Option<MarketDataProviderError> = None;
        let mut failures: Vec<SourceFailure> = Vec::new();
        for provider in &candidates {
            match provider.get_latest_bar(request.clone()).await {
                Ok(bar) => {
                    return WalkOutcome {
                        bar: Some(bar),
                        error: None,
                        failures,
                    };
                }
                Err(e) if e.should_fallover() => {
                    tracing::warn!(
                        source_id = provider.source_id(),
                        error = %e,
                        "provider returned retryable error; falling over"
                    );
                    failures.push(SourceFailure {
                        source_id: provider.source_id().to_string(),
                        error: e.clone(),
                    });
                    last_retryable = Some(e);
                }
                Err(e) => {
                    tracing::info!(
                        source_id = provider.source_id(),
                        error = %e,
                        "provider returned final error; not falling over"
                    );
                    failures.push(SourceFailure {
                        source_id: provider.source_id().to_string(),
                        error: e.clone(),
                    });
                    return WalkOutcome {
                        bar: None,
                        error: Some(e),
                        failures,
                    };
                }
            }
        }
        WalkOutcome {
            bar: None,
            error: Some(last_retryable.unwrap_or_else(|| {
                MarketDataProviderError::Infrastructure(
                    "registry walked all enabled candidates without a final answer".into(),
                )
            })),
            failures,
        }
    }

    /// Same walking policy as `get_latest_bar`, for `get_historical_range`.
    ///
    /// Implements FR-005 of spec 017.
    pub async fn get_historical_range(
        &self,
        request: HistoricalRangeRequest,
    ) -> Result<HistoricalRange, MarketDataProviderError> {
        let candidates = self.candidates_for(&request.identifier.scheme);
        if candidates.is_empty() {
            return Err(MarketDataProviderError::NotFound(format!(
                "no enabled source supports scheme '{}'",
                request.identifier.scheme
            )));
        }
        let mut last_retryable: Option<MarketDataProviderError> = None;
        for provider in &candidates {
            match provider.get_historical_range(request.clone()).await {
                Ok(range) => return Ok(range),
                Err(e) if e.should_fallover() => {
                    last_retryable = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_retryable.unwrap_or_else(|| {
            MarketDataProviderError::Infrastructure(
                "registry walked all enabled candidates without a final answer".into(),
            )
        }))
    }

    /// The ranged "first success" walk, plus the per-source failures it
    /// encountered along the way.
    ///
    /// Mirrors [`DataSourceRegistry::walk_latest_bar`] for ranged requests:
    /// the same scheme filter and the same fail-over rule, plus the
    /// per-source failure manifest the load's report needs (FR-006).
    ///
    /// When `preferred_source` names a configured source, that source is
    /// consulted **first** — a preference, not a restriction: the fail-over
    /// continues over the remaining enabled sources when it cannot answer
    /// (FR-013 of spec 018). An id that is not configured never reaches the
    /// walk; the caller rejects it as caller-input validation.
    ///
    /// The empty candidate set is signalled by exactly one contract:
    /// `range: None`, `error: Some(NotFound("no enabled source supports
    /// scheme '<scheme>'"))`, `failures: vec![]`.
    ///
    /// Implements FR-009 of spec 018.
    pub async fn walk_historical_range(
        &self,
        request: HistoricalRangeRequest,
        preferred_source: Option<&str>,
    ) -> RangeWalkOutcome {
        let mut candidates = self.candidates_for(&request.identifier.scheme);
        if candidates.is_empty() {
            return RangeWalkOutcome {
                range: None,
                error: Some(MarketDataProviderError::NotFound(format!(
                    "no enabled source supports scheme '{}'",
                    request.identifier.scheme
                ))),
                failures: Vec::new(),
            };
        }
        if let Some(preferred) = preferred_source {
            move_to_front(&mut candidates, preferred);
        }

        let mut last_retryable: Option<MarketDataProviderError> = None;
        let mut failures: Vec<SourceFailure> = Vec::new();
        for provider in &candidates {
            match provider.get_historical_range(request.clone()).await {
                Ok(range) => {
                    return RangeWalkOutcome {
                        range: Some(range),
                        error: None,
                        failures,
                    };
                }
                Err(e) if e.should_fallover() => {
                    tracing::warn!(
                        source_id = provider.source_id(),
                        error = %e,
                        "provider returned retryable error; falling over"
                    );
                    failures.push(SourceFailure {
                        source_id: provider.source_id().to_string(),
                        error: e.clone(),
                    });
                    last_retryable = Some(e);
                }
                Err(e) => {
                    tracing::info!(
                        source_id = provider.source_id(),
                        error = %e,
                        "provider returned final error; not falling over"
                    );
                    failures.push(SourceFailure {
                        source_id: provider.source_id().to_string(),
                        error: e.clone(),
                    });
                    return RangeWalkOutcome {
                        range: None,
                        error: Some(e),
                        failures,
                    };
                }
            }
        }
        RangeWalkOutcome {
            range: None,
            error: Some(last_retryable.unwrap_or_else(|| {
                MarketDataProviderError::Infrastructure(
                    "registry walked all enabled candidates without a final answer".into(),
                )
            })),
            failures,
        }
    }

    /// The ticker-lookup walk used by asset creation.
    ///
    /// Consulted sources are the scheme's candidates in the usual priority
    /// order, with `preferred_source` moved to the front (FR-013 of spec
    /// 018). Only **exact symbol matches** are returned: a hit whose
    /// `symbol` does not equal `query` (case-insensitively) is ignored, so
    /// a fuzzy upstream match can never be mistaken for the requested
    /// instrument.
    ///
    /// The walk stops at the first source that produced at least one exact
    /// match. When no source produced a match, `matches` is empty and
    /// `error` names the last retryable failure, or a `NotFound` when every
    /// consulted source answered but none recognized the symbol.
    ///
    /// Implements FR-014 of spec 018.
    pub async fn walk_ticker_lookup_exact(
        &self,
        query: &str,
        scheme: &str,
        preferred_source: Option<&str>,
    ) -> TickerLookupWalk {
        let mut candidates = self.candidates_for(scheme);
        if candidates.is_empty() {
            return TickerLookupWalk {
                matches: Vec::new(),
                error: Some(MarketDataProviderError::NotFound(format!(
                    "no enabled source supports scheme '{scheme}'"
                ))),
                failures: Vec::new(),
            };
        }
        if let Some(preferred) = preferred_source {
            move_to_front(&mut candidates, preferred);
        }

        let mut failures: Vec<SourceFailure> = Vec::new();
        let mut last_retryable: Option<MarketDataProviderError> = None;
        for provider in &candidates {
            let request = TickerLookupRequest {
                query: query.to_string(),
            };
            match provider.lookup_ticker(request).await {
                Ok(result) => {
                    let matches: Vec<TickerLookupMatch> = result
                        .hits
                        .into_iter()
                        .filter(|hit| hit.symbol.eq_ignore_ascii_case(query))
                        .map(|hit| TickerLookupMatch {
                            source_id: provider.source_id().to_string(),
                            hit,
                        })
                        .collect();
                    if !matches.is_empty() {
                        return TickerLookupWalk {
                            matches,
                            error: None,
                            failures,
                        };
                    }
                }
                Err(e) if e.should_fallover() => {
                    failures.push(SourceFailure {
                        source_id: provider.source_id().to_string(),
                        error: e.clone(),
                    });
                    last_retryable = Some(e);
                }
                Err(e) => {
                    failures.push(SourceFailure {
                        source_id: provider.source_id().to_string(),
                        error: e.clone(),
                    });
                    return TickerLookupWalk {
                        matches: Vec::new(),
                        error: Some(e),
                        failures,
                    };
                }
            }
        }

        TickerLookupWalk {
            matches: Vec::new(),
            error: Some(last_retryable.unwrap_or_else(|| {
                MarketDataProviderError::NotFound(format!(
                    "no enabled source recognizes symbol '{query}'"
                ))
            })),
            failures,
        }
    }

    /// "Every enabled source's answer" — never fails because of a single
    /// source's failure. Returns a map keyed by `source_id` whose values
    /// are `Result<…, MarketDataProviderError>`. Sequential in v1
    /// (research D-out-of-scope; parallel batch is a follow-on).
    ///
    /// Implements FR-006 of spec 017.
    pub async fn every_source_latest_bar(
        &self,
        request: LatestBarRequest,
    ) -> BTreeMap<String, Result<LatestBar, MarketDataProviderError>> {
        let mut out = BTreeMap::new();
        for provider in &self.enabled {
            let outcome = provider.get_latest_bar(request.clone()).await;
            out.insert(provider.source_id().to_string(), outcome);
        }
        out
    }

    /// "Every enabled source's answer" for ticker lookup. Mirrors
    /// `every_source_latest_bar`.
    ///
    /// Implements FR-006 / FR-012 of spec 017.
    pub async fn every_source_lookup_ticker(
        &self,
        request: TickerLookupRequest,
    ) -> BTreeMap<String, Result<TickerLookupResult, MarketDataProviderError>> {
        let mut out = BTreeMap::new();
        for provider in &self.enabled {
            let outcome = provider.lookup_ticker(request.clone()).await;
            out.insert(provider.source_id().to_string(), outcome);
        }
        out
    }

    /// Filters the enabled list to providers whose `supported_schemes`
    /// contains `scheme` (with `TICKER:*` matching any `TICKER:XXX`).
    fn candidates_for(&self, scheme: &str) -> Vec<Arc<dyn MarketDataProvider + Send + Sync>> {
        self.enabled
            .iter()
            .filter(|p| scheme_matches(p.supported_schemes(), scheme))
            .cloned()
            .collect()
    }
}

/// Moves the named source to the front of the candidate list.
///
/// A preferred source that is not among the candidates (disabled, or unable
/// to answer this scheme) is simply not present — the order is unchanged.
///
/// Implements FR-013 of `specs/018-load-reference-data/spec.md`.
fn move_to_front(
    candidates: &mut Vec<Arc<dyn MarketDataProvider + Send + Sync>>,
    preferred_source: &str,
) {
    if let Some(index) = candidates
        .iter()
        .position(|provider| provider.source_id() == preferred_source)
    {
        let preferred = candidates.remove(index);
        candidates.insert(0, preferred);
    }
}

fn scheme_matches(supported: &[String], scheme: &str) -> bool {
    for s in supported {
        if s == scheme {
            return true;
        }
        if let Some(prefix) = s.strip_suffix(":*")
            && scheme.starts_with(prefix)
            && scheme.contains(':')
        {
            return true;
        }
        if s == "*" {
            return true;
        }
    }
    false
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::market_data::config::DataSourceConfig;
    use async_trait::async_trait;
    use chrono::Utc;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use crate::market_data::error::MarketDataProviderError;
    use crate::market_data::types::{
        HistoricalRange, LatestBar, LatestBarRequest, PriceBar, TickerLookupHit,
        TickerLookupResult, TypedIdentifier,
    };

    /// A `MockMarketDataProvider` driven by per-method response maps.
    ///
    /// Implements the trait contract of FR-001 of spec 017; used to
    /// exercise the registry's failover behavior offline.
    pub struct MockMarketDataProvider {
        pub id: String,
        pub display: String,
        pub schemes: Vec<String>,
        pub priority: i32,
        pub enabled: bool,
        pub latest: HashMap<String, Result<LatestBar, MarketDataProviderError>>,
        /// Per-key responses for `get_historical_range`.
        pub historical: HashMap<String, Result<HistoricalRange, MarketDataProviderError>>,
        /// Per-query responses for `lookup_ticker`.
        pub lookup: HashMap<String, Result<TickerLookupResult, MarketDataProviderError>>,
    }

    impl MockMarketDataProvider {
        /// Constructs a mock provider with the given identity.
        ///
        /// Implements FR-001 of spec 017.
        pub fn new(id: &str, schemes: &[&str], priority: i32) -> Self {
            Self {
                id: id.to_string(),
                display: id.to_string(),
                schemes: schemes.iter().map(|s| s.to_string()).collect(),
                priority,
                enabled: true,
                latest: HashMap::new(),
                historical: HashMap::new(),
                lookup: HashMap::new(),
            }
        }
    }

    #[async_trait]
    impl MarketDataProvider for MockMarketDataProvider {
        fn source_id(&self) -> &str {
            &self.id
        }
        fn display_name(&self) -> &str {
            &self.display
        }
        fn supported_schemes(&self) -> &[String] {
            &self.schemes
        }
        fn enabled(&self) -> bool {
            self.enabled
        }
        fn priority(&self) -> i32 {
            self.priority
        }

        async fn get_latest_bar(
            &self,
            request: LatestBarRequest,
        ) -> Result<LatestBar, MarketDataProviderError> {
            let key = format!("{}:{}", request.identifier.scheme, request.identifier.value);
            match self.latest.get(&key) {
                Some(r) => match r {
                    Ok(bar) => Ok(bar.clone()),
                    Err(e) => Err(e.clone()),
                },
                None => Err(MarketDataProviderError::NotFound(format!(
                    "mock {key} not configured"
                ))),
            }
        }

        async fn get_historical_range(
            &self,
            request: crate::market_data::types::HistoricalRangeRequest,
        ) -> Result<HistoricalRange, MarketDataProviderError> {
            let key = format!("{}:{}", request.identifier.scheme, request.identifier.value);
            match self.historical.get(&key) {
                Some(r) => match r {
                    Ok(range) => Ok(range.clone()),
                    Err(e) => Err(e.clone()),
                },
                None => Err(MarketDataProviderError::Unsupported(
                    "mock: historical_range not implemented".into(),
                )),
            }
        }

        async fn lookup_ticker(
            &self,
            request: TickerLookupRequest,
        ) -> Result<TickerLookupResult, MarketDataProviderError> {
            match self.lookup.get(&request.query) {
                Some(r) => match r {
                    Ok(result) => Ok(result.clone()),
                    Err(e) => Err(e.clone()),
                },
                None => Err(MarketDataProviderError::Unsupported(
                    "mock: lookup not implemented".into(),
                )),
            }
        }
    }

    fn make_bar(source: &str) -> LatestBar {
        use chrono::NaiveDate;
        use rust_decimal::Decimal;
        LatestBar {
            bar: PriceBar {
                as_of_date: NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"),
                open: Decimal::new(100, 0),
                high: Decimal::new(101, 0),
                low: Decimal::new(99, 0),
                close: Decimal::new(100, 0),
                adjclose: Decimal::new(100, 0),
                volume: 1_000_000,
            },
            knowledge_date: NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"),
            source_id: source.to_string(),
        }
    }

    #[test]
    fn registry_new_rejects_duplicate_source_id() {
        let a: Arc<dyn MarketDataProvider + Send + Sync> =
            Arc::new(MockMarketDataProvider::new("a", &["TICKER"], 10));
        let b: Arc<dyn MarketDataProvider + Send + Sync> =
            Arc::new(MockMarketDataProvider::new("a", &["TICKER"], 20));
        let result = DataSourceRegistry::new(vec![a, b]);
        assert!(matches!(
            result,
            Err(MarketDataProviderError::Validation(ref msg)) if msg.contains("duplicate")
        ));
    }

    #[test]
    fn enumerate_lists_by_source_id_asc() {
        let a: Arc<dyn MarketDataProvider + Send + Sync> =
            Arc::new(MockMarketDataProvider::new("beta", &["TICKER"], 30));
        let b: Arc<dyn MarketDataProvider + Send + Sync> =
            Arc::new(MockMarketDataProvider::new("alpha", &["ISIN"], 10));
        let registry = DataSourceRegistry::new(vec![a, b]).expect("registry");
        let sources = registry.enumerate_sources();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].source_id, "alpha");
        assert_eq!(sources[1].source_id, "beta");
    }

    #[tokio::test]
    async fn get_latest_bar_walks_in_priority_order() {
        let mut a = MockMarketDataProvider::new("a_low_priority", &["TICKER"], 20);
        a.latest
            .insert("TICKER:AAPL".to_string(), Ok(make_bar("a_low_priority")));
        let mut b = MockMarketDataProvider::new("b_high_priority", &["TICKER"], 10);
        b.latest
            .insert("TICKER:AAPL".to_string(), Ok(make_bar("b_high_priority")));
        let a_arc: Arc<dyn MarketDataProvider + Send + Sync> = Arc::new(a);
        let b_arc: Arc<dyn MarketDataProvider + Send + Sync> = Arc::new(b);
        let registry = DataSourceRegistry::new(vec![a_arc, b_arc]).expect("registry");
        let request = LatestBarRequest::now(TypedIdentifier::new("TICKER", "AAPL"));
        let bar = registry.get_latest_bar(request).await.expect("bar");
        assert_eq!(bar.source_id, "b_high_priority");
    }

    #[tokio::test]
    async fn transient_error_falls_over_to_next_provider() {
        let mut a = MockMarketDataProvider::new("a_transient", &["TICKER"], 10);
        a.latest.insert(
            "TICKER:AAPL".to_string(),
            Err(MarketDataProviderError::Transient(
                "upstream timeout".into(),
            )),
        );
        let mut b = MockMarketDataProvider::new("b_ok", &["TICKER"], 20);
        b.latest
            .insert("TICKER:AAPL".to_string(), Ok(make_bar("b_ok")));
        let a_arc: Arc<dyn MarketDataProvider + Send + Sync> = Arc::new(a);
        let b_arc: Arc<dyn MarketDataProvider + Send + Sync> = Arc::new(b);
        let registry = DataSourceRegistry::new(vec![a_arc, b_arc]).expect("registry");
        let request = LatestBarRequest::now(TypedIdentifier::new("TICKER", "AAPL"));
        let bar = registry.get_latest_bar(request).await.expect("bar");
        assert_eq!(bar.source_id, "b_ok");
    }

    #[tokio::test]
    async fn not_found_is_final_no_fallover() {
        let mut a = MockMarketDataProvider::new("a_nf", &["TICKER"], 10);
        a.latest.insert(
            "TICKER:AAPL".to_string(),
            Err(MarketDataProviderError::NotFound("AAPL not in a".into())),
        );
        let mut b = MockMarketDataProvider::new("b_ok", &["TICKER"], 20);
        b.latest
            .insert("TICKER:AAPL".to_string(), Ok(make_bar("b_ok")));
        let a_arc: Arc<dyn MarketDataProvider + Send + Sync> = Arc::new(a);
        let b_arc: Arc<dyn MarketDataProvider + Send + Sync> = Arc::new(b);
        let registry = DataSourceRegistry::new(vec![a_arc, b_arc]).expect("registry");
        let request = LatestBarRequest::now(TypedIdentifier::new("TICKER", "AAPL"));
        let err = registry.get_latest_bar(request).await.expect_err("err");
        assert!(matches!(err, MarketDataProviderError::NotFound(_)));
    }

    #[tokio::test]
    async fn no_source_supports_scheme_returns_not_found() {
        let a: Arc<dyn MarketDataProvider + Send + Sync> =
            Arc::new(MockMarketDataProvider::new("a", &["ISIN"], 10));
        let registry = DataSourceRegistry::new(vec![a]).expect("registry");
        let request = LatestBarRequest::now(TypedIdentifier::new("TICKER", "AAPL"));
        let err = registry.get_latest_bar(request).await.expect_err("err");
        assert!(
            matches!(err, MarketDataProviderError::NotFound(ref msg) if msg.contains("no enabled source"))
        );
    }

    #[tokio::test]
    async fn every_source_returns_per_source_map() {
        let mut a = MockMarketDataProvider::new("a", &["TICKER"], 10);
        a.latest
            .insert("TICKER:AAPL".to_string(), Ok(make_bar("a")));
        let mut b = MockMarketDataProvider::new("b", &["TICKER"], 20);
        b.latest.insert(
            "TICKER:AAPL".to_string(),
            Err(MarketDataProviderError::NotFound("not in b".into())),
        );
        let a_arc: Arc<dyn MarketDataProvider + Send + Sync> = Arc::new(a);
        let b_arc: Arc<dyn MarketDataProvider + Send + Sync> = Arc::new(b);
        let registry = DataSourceRegistry::new(vec![a_arc, b_arc]).expect("registry");
        let request = LatestBarRequest::now(TypedIdentifier::new("TICKER", "AAPL"));
        let map = registry.every_source_latest_bar(request).await;
        assert_eq!(map.len(), 2);
        assert!(map["a"].is_ok());
        assert!(map["b"].is_err());
    }

    #[test]
    fn scheme_matches_handles_wildcards() {
        assert!(scheme_matches(&["TICKER".to_string()], "TICKER"));
        assert!(!scheme_matches(&["TICKER".to_string()], "ISIN"));
        assert!(scheme_matches(&["TICKER:*".to_string()], "TICKER:XNAS"));
        assert!(!scheme_matches(&["TICKER:*".to_string()], "TICKER"));
        assert!(scheme_matches(&["*".to_string()], "anything"));
    }

    /// Builds a registry with a single source that answers `TICKER`.
    ///
    /// Shared with the `refresh` module's identifier-picker tests.
    pub(crate) fn registry_with_ticker_source() -> DataSourceRegistry {
        let provider: Arc<dyn MarketDataProvider + Send + Sync> =
            Arc::new(MockMarketDataProvider::new("mock", &["TICKER"], 10));
        DataSourceRegistry::new(vec![provider]).expect("registry")
    }

    // ---------- enabled_count / disabled-source filtering ----------

    /// `enabled_count` counts only the enabled providers; disabled ones stay
    /// in `enumerate_sources` (FR-002 / FR-004).
    #[test]
    fn enabled_count_counts_only_enabled_sources() {
        let mut enabled = MockMarketDataProvider::new("a_enabled", &["TICKER"], 10);
        enabled.enabled = true;
        let mut disabled = MockMarketDataProvider::new("b_disabled", &["TICKER"], 20);
        disabled.enabled = false;
        let registry = DataSourceRegistry::new(vec![
            Arc::new(enabled) as Arc<dyn MarketDataProvider + Send + Sync>,
            Arc::new(disabled) as Arc<dyn MarketDataProvider + Send + Sync>,
        ])
        .expect("registry");
        assert_eq!(registry.enabled_count(), 1);
        // The disabled source is still enumerated, flagged as disabled.
        let sources = registry.enumerate_sources();
        assert_eq!(sources.len(), 2);
        assert!(!sources[1].enabled, "the disabled source must be flagged");
    }

    /// A disabled provider is skipped by the walk even when it is the only
    /// one that could answer the request (FR-004).
    #[tokio::test]
    async fn disabled_sources_are_not_walked() {
        let mut disabled = MockMarketDataProvider::new("only", &["TICKER"], 10);
        disabled.enabled = false;
        disabled
            .latest
            .insert("TICKER:AAPL".to_string(), Ok(make_bar("only")));
        let registry = DataSourceRegistry::new(vec![
            Arc::new(disabled) as Arc<dyn MarketDataProvider + Send + Sync>
        ])
        .expect("registry");
        let request = LatestBarRequest::now(TypedIdentifier::new("TICKER", "AAPL"));
        let err = registry.get_latest_bar(request).await.expect_err("err");
        assert!(
            matches!(err, MarketDataProviderError::NotFound(ref msg) if msg.contains("no enabled source")),
            "got: {err:?}"
        );
    }

    // ---------- walk_latest_bar failure bookkeeping ----------

    /// A retryable error is recorded in `failures` and the walk continues to
    /// the next candidate (FR-005 / FR-007).
    #[tokio::test]
    async fn walk_latest_bar_records_retryable_failures() {
        let mut a = MockMarketDataProvider::new("a_retryable", &["TICKER"], 10);
        a.latest.insert(
            "TICKER:AAPL".to_string(),
            Err(MarketDataProviderError::Transient("boom".into())),
        );
        let mut b = MockMarketDataProvider::new("b_ok", &["TICKER"], 20);
        b.latest
            .insert("TICKER:AAPL".to_string(), Ok(make_bar("b_ok")));
        let registry = DataSourceRegistry::new(vec![
            Arc::new(a) as Arc<dyn MarketDataProvider + Send + Sync>,
            Arc::new(b) as Arc<dyn MarketDataProvider + Send + Sync>,
        ])
        .expect("registry");
        let request = LatestBarRequest::now(TypedIdentifier::new("TICKER", "AAPL"));
        let outcome = registry.walk_latest_bar(request).await;
        assert!(outcome.bar.is_some(), "the walk should have produced a bar");
        assert_eq!(outcome.failures.len(), 1);
        assert_eq!(outcome.failures[0].source_id, "a_retryable");
        assert!(outcome.error.is_none());
    }

    /// A final (non-retryable) error ends the walk and is recorded in
    /// `failures` with the typed error (FR-005 / FR-007).
    #[tokio::test]
    async fn walk_latest_bar_records_final_failure() {
        let mut a = MockMarketDataProvider::new("a_final", &["TICKER"], 10);
        a.latest.insert(
            "TICKER:AAPL".to_string(),
            Err(MarketDataProviderError::Unauthorized("no key".into())),
        );
        let mut b = MockMarketDataProvider::new("b_ok", &["TICKER"], 20);
        b.latest
            .insert("TICKER:AAPL".to_string(), Ok(make_bar("b_ok")));
        let registry = DataSourceRegistry::new(vec![
            Arc::new(a) as Arc<dyn MarketDataProvider + Send + Sync>,
            Arc::new(b) as Arc<dyn MarketDataProvider + Send + Sync>,
        ])
        .expect("registry");
        let request = LatestBarRequest::now(TypedIdentifier::new("TICKER", "AAPL"));
        let outcome = registry.walk_latest_bar(request).await;
        assert!(outcome.bar.is_none(), "a final error must stop the walk");
        assert_eq!(outcome.failures.len(), 1);
        assert_eq!(outcome.failures[0].source_id, "a_final");
        assert!(matches!(
            outcome.error,
            Some(MarketDataProviderError::Unauthorized(_))
        ));
    }

    /// When every enabled candidate returns a retryable error, the walk
    /// surfaces the LAST retryable error and keeps every failure (FR-005).
    #[tokio::test]
    async fn walk_latest_bar_surfaces_last_retryable_error() {
        let mut a = MockMarketDataProvider::new("a_first", &["TICKER"], 10);
        a.latest.insert(
            "TICKER:AAPL".to_string(),
            Err(MarketDataProviderError::Transient("first".into())),
        );
        let mut b = MockMarketDataProvider::new("b_last", &["TICKER"], 20);
        b.latest.insert(
            "TICKER:AAPL".to_string(),
            Err(MarketDataProviderError::RateLimited {
                msg: "budget exhausted".into(),
                retry_after: None,
            }),
        );
        let registry = DataSourceRegistry::new(vec![
            Arc::new(a) as Arc<dyn MarketDataProvider + Send + Sync>,
            Arc::new(b) as Arc<dyn MarketDataProvider + Send + Sync>,
        ])
        .expect("registry");
        let request = LatestBarRequest::now(TypedIdentifier::new("TICKER", "AAPL"));
        let outcome = registry.walk_latest_bar(request).await;
        assert!(outcome.bar.is_none());
        assert_eq!(outcome.failures.len(), 2);
        assert!(matches!(
            outcome.error,
            Some(MarketDataProviderError::RateLimited { .. })
        ));
    }

    // ---------- get_historical_range ----------

    /// The historical walk returns the first successful candidate's range.
    #[tokio::test]
    async fn get_historical_range_returns_first_success() {
        let mut a = MockMarketDataProvider::new("a_ok", &["TICKER"], 10);
        a.historical
            .insert("TICKER:AAPL".to_string(), Ok(make_range("a_ok", 2)));
        let registry = DataSourceRegistry::new(vec![
            Arc::new(a) as Arc<dyn MarketDataProvider + Send + Sync>
        ])
        .expect("registry");
        let range = registry
            .get_historical_range(historical_request())
            .await
            .expect("range");
        assert_eq!(range.source_id, "a_ok");
        assert_eq!(range.bars.len(), 2);
    }

    /// No enabled source supports the scheme → `NotFound` (FR-005).
    #[tokio::test]
    async fn get_historical_range_without_candidates_returns_not_found() {
        let a: Arc<dyn MarketDataProvider + Send + Sync> =
            Arc::new(MockMarketDataProvider::new("a", &["ISIN"], 10));
        let registry = DataSourceRegistry::new(vec![a]).expect("registry");
        let err = registry
            .get_historical_range(historical_request())
            .await
            .expect_err("err");
        assert!(
            matches!(err, MarketDataProviderError::NotFound(ref msg) if msg.contains("no enabled source")),
            "got: {err:?}"
        );
    }

    /// A retryable error falls over to the next candidate (FR-005).
    #[tokio::test]
    async fn get_historical_range_falls_over_on_retryable_error() {
        let mut a = MockMarketDataProvider::new("a_retryable", &["TICKER"], 10);
        a.historical.insert(
            "TICKER:AAPL".to_string(),
            Err(MarketDataProviderError::Transient("boom".into())),
        );
        let mut b = MockMarketDataProvider::new("b_ok", &["TICKER"], 20);
        b.historical
            .insert("TICKER:AAPL".to_string(), Ok(make_range("b_ok", 1)));
        let registry = DataSourceRegistry::new(vec![
            Arc::new(a) as Arc<dyn MarketDataProvider + Send + Sync>,
            Arc::new(b) as Arc<dyn MarketDataProvider + Send + Sync>,
        ])
        .expect("registry");
        let range = registry
            .get_historical_range(historical_request())
            .await
            .expect("range");
        assert_eq!(range.source_id, "b_ok");
    }

    /// A final error ends the historical walk immediately (FR-005).
    #[tokio::test]
    async fn get_historical_range_final_error_stops_the_walk() {
        let mut a = MockMarketDataProvider::new("a_final", &["TICKER"], 10);
        a.historical.insert(
            "TICKER:AAPL".to_string(),
            Err(MarketDataProviderError::InvalidInput("bad range".into())),
        );
        let mut b = MockMarketDataProvider::new("b_ok", &["TICKER"], 20);
        b.historical
            .insert("TICKER:AAPL".to_string(), Ok(make_range("b_ok", 1)));
        let registry = DataSourceRegistry::new(vec![
            Arc::new(a) as Arc<dyn MarketDataProvider + Send + Sync>,
            Arc::new(b) as Arc<dyn MarketDataProvider + Send + Sync>,
        ])
        .expect("registry");
        let err = registry
            .get_historical_range(historical_request())
            .await
            .expect_err("err");
        assert!(matches!(err, MarketDataProviderError::InvalidInput(_)));
    }

    /// Every candidate retryable → the last retryable error is surfaced.
    #[tokio::test]
    async fn get_historical_range_surfaces_last_retryable_error() {
        let mut a = MockMarketDataProvider::new("a_first", &["TICKER"], 10);
        a.historical.insert(
            "TICKER:AAPL".to_string(),
            Err(MarketDataProviderError::Transient("first".into())),
        );
        let mut b = MockMarketDataProvider::new("b_last", &["TICKER"], 20);
        b.historical.insert(
            "TICKER:AAPL".to_string(),
            Err(MarketDataProviderError::Transient("last".into())),
        );
        let registry = DataSourceRegistry::new(vec![
            Arc::new(a) as Arc<dyn MarketDataProvider + Send + Sync>,
            Arc::new(b) as Arc<dyn MarketDataProvider + Send + Sync>,
        ])
        .expect("registry");
        let err = registry
            .get_historical_range(historical_request())
            .await
            .expect_err("err");
        assert!(
            matches!(err, MarketDataProviderError::Transient(ref msg) if msg == "last"),
            "got: {err:?}"
        );
    }

    // ---------- walk_historical_range ----------

    /// The ranged walk consults the named preferred source first, and the
    /// fail-over continues over the remaining sources (FR-013).
    #[tokio::test]
    async fn walk_historical_range_consults_the_preferred_source_first() {
        let mut preferred = MockMarketDataProvider::new("preferred", &["TICKER"], 20);
        preferred
            .historical
            .insert("TICKER:AAPL".to_string(), Ok(make_range("preferred", 1)));
        let mut normal = MockMarketDataProvider::new("normal", &["TICKER"], 10);
        normal
            .historical
            .insert("TICKER:AAPL".to_string(), Ok(make_range("normal", 1)));
        let registry = DataSourceRegistry::new(vec![
            Arc::new(normal) as Arc<dyn MarketDataProvider + Send + Sync>,
            Arc::new(preferred) as Arc<dyn MarketDataProvider + Send + Sync>,
        ])
        .expect("registry");

        let outcome = registry
            .walk_historical_range(historical_request(), Some("preferred"))
            .await;
        assert_eq!(
            outcome.range.expect("range").source_id,
            "preferred",
            "the named source must be consulted first"
        );
    }

    /// A preferred source that cannot answer is fell over, and the failure
    /// stays in the manifest (FR-013).
    #[tokio::test]
    async fn walk_historical_range_falls_over_from_the_preferred_source() {
        let mut preferred = MockMarketDataProvider::new("preferred", &["TICKER"], 20);
        preferred.historical.insert(
            "TICKER:AAPL".to_string(),
            Err(MarketDataProviderError::Transient("boom".into())),
        );
        let mut normal = MockMarketDataProvider::new("normal", &["TICKER"], 10);
        normal
            .historical
            .insert("TICKER:AAPL".to_string(), Ok(make_range("normal", 1)));
        let registry = DataSourceRegistry::new(vec![
            Arc::new(normal) as Arc<dyn MarketDataProvider + Send + Sync>,
            Arc::new(preferred) as Arc<dyn MarketDataProvider + Send + Sync>,
        ])
        .expect("registry");

        let outcome = registry
            .walk_historical_range(historical_request(), Some("preferred"))
            .await;
        assert_eq!(outcome.range.expect("range").source_id, "normal");
        assert_eq!(outcome.failures.len(), 1);
        assert_eq!(outcome.failures[0].source_id, "preferred");
    }

    /// An empty candidate set is signalled by exactly one contract:
    /// `range: None`, a `NotFound` error and an empty failure list.
    #[tokio::test]
    async fn walk_historical_range_without_candidates_reports_the_empty_contract() {
        let a: Arc<dyn MarketDataProvider + Send + Sync> =
            Arc::new(MockMarketDataProvider::new("a", &["ISIN"], 10));
        let registry = DataSourceRegistry::new(vec![a]).expect("registry");
        let outcome = registry
            .walk_historical_range(historical_request(), None)
            .await;
        assert!(outcome.range.is_none());
        assert!(outcome.failures.is_empty());
        assert!(matches!(
            outcome.error,
            Some(MarketDataProviderError::NotFound(ref msg)) if msg.contains("no enabled source")
        ));
    }

    /// The ranged walk records the failing source in its manifest.
    #[tokio::test]
    async fn walk_historical_range_records_the_failing_source() {
        let mut failing = MockMarketDataProvider::new("failing", &["TICKER"], 10);
        failing.historical.insert(
            "TICKER:AAPL".to_string(),
            Err(MarketDataProviderError::Transient("boom".into())),
        );
        let mut ok = MockMarketDataProvider::new("ok", &["TICKER"], 20);
        ok.historical
            .insert("TICKER:AAPL".to_string(), Ok(make_range("ok", 1)));
        let registry = DataSourceRegistry::new(vec![
            Arc::new(failing) as Arc<dyn MarketDataProvider + Send + Sync>,
            Arc::new(ok) as Arc<dyn MarketDataProvider + Send + Sync>,
        ])
        .expect("registry");
        let outcome = registry
            .walk_historical_range(historical_request(), None)
            .await;
        assert_eq!(outcome.range.expect("range").source_id, "ok");
        assert_eq!(outcome.failures.len(), 1);
        assert_eq!(outcome.failures[0].source_id, "failing");
    }

    // ---------- every_source_lookup_ticker ----------

    /// The lookup map carries every enabled source's answer, including the
    /// failures (FR-006 / FR-012).
    #[tokio::test]
    async fn every_source_lookup_ticker_returns_per_source_map() {
        let mut a = MockMarketDataProvider::new("a", &["TICKER"], 10);
        a.lookup.insert(
            "MSFT".to_string(),
            Ok(TickerLookupResult {
                hits: vec![make_hit("MSFT", 0.9)],
                source_id: "a".to_string(),
            }),
        );
        let mut b = MockMarketDataProvider::new("b", &["TICKER"], 20);
        b.lookup.insert(
            "MSFT".to_string(),
            Err(MarketDataProviderError::NotFound("no hit".into())),
        );
        let registry = DataSourceRegistry::new(vec![
            Arc::new(a) as Arc<dyn MarketDataProvider + Send + Sync>,
            Arc::new(b) as Arc<dyn MarketDataProvider + Send + Sync>,
        ])
        .expect("registry");
        let map = registry
            .every_source_lookup_ticker(TickerLookupRequest {
                query: "MSFT".to_string(),
            })
            .await;
        assert_eq!(map.len(), 2);
        assert_eq!(map["a"].as_ref().expect("a").hits.len(), 1);
        assert!(map["b"].is_err());
    }

    // ---------- walk_ticker_lookup_exact ----------

    /// Only exact-symbol hits come back, and the walk stops at the first
    /// source that has one.
    #[tokio::test]
    async fn exact_lookup_ignores_fuzzy_hits_and_stops_at_the_first_match() {
        let mut a = MockMarketDataProvider::new("a", &["TICKER"], 10);
        a.lookup.insert(
            "QQQM".to_string(),
            Ok(TickerLookupResult {
                hits: vec![make_hit("QQQ", 0.99)],
                source_id: "a".to_string(),
            }),
        );
        let mut b = MockMarketDataProvider::new("b", &["TICKER"], 20);
        b.lookup.insert(
            "QQQM".to_string(),
            Ok(TickerLookupResult {
                hits: vec![make_hit("QQQM", 0.5)],
                source_id: "b".to_string(),
            }),
        );
        let registry = DataSourceRegistry::new(vec![
            Arc::new(a) as Arc<dyn MarketDataProvider + Send + Sync>,
            Arc::new(b) as Arc<dyn MarketDataProvider + Send + Sync>,
        ])
        .expect("registry");

        let walk = registry
            .walk_ticker_lookup_exact("QQQM", "TICKER", None)
            .await;
        assert_eq!(walk.matches.len(), 1);
        assert_eq!(walk.matches[0].source_id, "b");
        assert_eq!(walk.matches[0].hit.symbol, "QQQM");
        assert!(walk.failures.is_empty());
    }

    /// The named preferred source is consulted first.
    #[tokio::test]
    async fn exact_lookup_honours_the_preferred_source() {
        let mut a = MockMarketDataProvider::new("a", &["TICKER"], 10);
        a.lookup.insert(
            "AAPL".to_string(),
            Ok(TickerLookupResult {
                hits: vec![make_hit("AAPL", 0.9)],
                source_id: "a".to_string(),
            }),
        );
        let mut b = MockMarketDataProvider::new("b", &["TICKER"], 20);
        b.lookup.insert(
            "AAPL".to_string(),
            Ok(TickerLookupResult {
                hits: vec![make_hit("AAPL", 0.9)],
                source_id: "b".to_string(),
            }),
        );
        let registry = DataSourceRegistry::new(vec![
            Arc::new(a) as Arc<dyn MarketDataProvider + Send + Sync>,
            Arc::new(b) as Arc<dyn MarketDataProvider + Send + Sync>,
        ])
        .expect("registry");

        let walk = registry
            .walk_ticker_lookup_exact("AAPL", "TICKER", Some("b"))
            .await;
        assert_eq!(walk.matches.len(), 1);
        assert_eq!(walk.matches[0].source_id, "b");
    }

    /// No exact hit and no failures is an authoritative "not recognized".
    #[tokio::test]
    async fn exact_lookup_without_a_match_reports_not_found() {
        let mut a = MockMarketDataProvider::new("a", &["TICKER"], 10);
        a.lookup.insert(
            "QQQM".to_string(),
            Ok(TickerLookupResult {
                hits: vec![make_hit("QQQ", 0.99)],
                source_id: "a".to_string(),
            }),
        );
        let registry = DataSourceRegistry::new(vec![
            Arc::new(a) as Arc<dyn MarketDataProvider + Send + Sync>
        ])
        .expect("registry");

        let walk = registry
            .walk_ticker_lookup_exact("QQQM", "TICKER", None)
            .await;
        assert!(walk.matches.is_empty());
        assert!(walk.failures.is_empty());
        assert!(matches!(
            walk.error,
            Some(MarketDataProviderError::NotFound(_))
        ));
    }

    /// A scheme with no candidate source is signalled without contacting
    /// anything.
    #[tokio::test]
    async fn exact_lookup_without_a_candidate_is_not_found() {
        let provider = MockMarketDataProvider::new("a", &["TICKER"], 10);
        let registry = DataSourceRegistry::new(vec![
            Arc::new(provider) as Arc<dyn MarketDataProvider + Send + Sync>
        ])
        .expect("registry");

        let walk = registry
            .walk_ticker_lookup_exact("US0378331005", "ISIN", None)
            .await;
        assert!(walk.matches.is_empty());
        assert!(walk.failures.is_empty());
        assert!(matches!(
            walk.error,
            Some(MarketDataProviderError::NotFound(_))
        ));
    }

    // ---------- fixtures ----------

    /// A historical request for `TICKER:AAPL` over a fixed date window.
    fn historical_request() -> crate::market_data::types::HistoricalRangeRequest {
        crate::market_data::types::HistoricalRangeRequest {
            identifier: TypedIdentifier::new("TICKER", "AAPL"),
            from: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).expect("date"),
            to: chrono::NaiveDate::from_ymd_opt(2026, 1, 31).expect("date"),
        }
    }

    /// A historical range with `bars` synthetic bars.
    fn make_range(source: &str, bars: usize) -> HistoricalRange {
        use chrono::NaiveDate;
        use rust_decimal::Decimal;
        HistoricalRange {
            bars: (0..bars)
                .map(|i| PriceBar {
                    as_of_date: NaiveDate::from_ymd_opt(2026, 1, 2).expect("date")
                        + chrono::Duration::days(i as i64),
                    open: Decimal::new(100, 0),
                    high: Decimal::new(101, 0),
                    low: Decimal::new(99, 0),
                    close: Decimal::new(100, 0),
                    adjclose: Decimal::new(100, 0),
                    volume: 1_000_000,
                })
                .collect(),
            knowledge_date: NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"),
            source_id: source.to_string(),
        }
    }

    /// A single ticker-lookup hit.
    fn make_hit(symbol: &str, relevance: f64) -> TickerLookupHit {
        TickerLookupHit {
            symbol: symbol.to_string(),
            short_name: Some("Short".to_string()),
            long_name: Some("Long Name".to_string()),
            quote_type: Some("EQUITY".to_string()),
            exchange: Some("NMS".to_string()),
            currency: None,
            schemes: vec![("ISIN".to_string(), "US0378331005".to_string())],
            relevance: Some(relevance),
        }
    }

    // Suppress unused-import warning when #[cfg(test)] is dormant
    #[allow(dead_code)]
    fn _unused(_m: &Mutex<()>, _cfg: &DataSourceConfig, _u: &Utc) {}
}
