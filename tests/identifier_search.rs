//! Database-backed tests for the read-only identifier search.
//!
//! Implements the executable proof of the known-state half of
//! `specs/026-identifier-search/spec.md`: exact `(scheme, value)` matches,
//! equivalent-ticker matches across venue and case variants, and a
//! before/after row count proving the operation never writes.
//!
//! The tests use the shared `obolargus_core::common::tx_pool()` fixture,
//! which truncates every high-level table before each test and on drop. When
//! no test database is configured the tests skip as a passing no-op, per the
//! `obolargus_core/AGENTS.md` convention.

use std::sync::Arc;

use async_trait::async_trait;

use obolargus_core::common::*;
use obolargus_core::domain::asset::create::create_asset_with_identifier;
use obolargus_core::domain::asset::entity::NewAsset;
use obolargus_core::market_data::error::MarketDataProviderError;
use obolargus_core::market_data::identifier_search::{
    IdentifierSearchRequest, KnownMatchKind, search_identifiers,
};
use obolargus_core::market_data::registry::DataSourceRegistry;
use obolargus_core::market_data::trait_::MarketDataProvider;
use obolargus_core::market_data::types::{
    HistoricalRange, HistoricalRangeRequest, LatestBar, LatestBarRequest, TickerLookupHit,
    TickerLookupRequest, TickerLookupResult,
};

/// A provider that returns one scripted list of hits for any query.
struct SearchProvider {
    source_id: String,
    hits: Vec<TickerLookupHit>,
}

impl SearchProvider {
    fn new(hits: Vec<TickerLookupHit>) -> Self {
        Self {
            source_id: "search_source".to_string(),
            hits,
        }
    }
}

#[async_trait]
impl MarketDataProvider for SearchProvider {
    fn source_id(&self) -> &str {
        &self.source_id
    }

    fn display_name(&self) -> &str {
        "Search Source"
    }

    fn supported_schemes(&self) -> &[String] {
        static SCHEMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
        SCHEMES.get_or_init(|| vec!["TICKER".to_string()])
    }

    fn enabled(&self) -> bool {
        true
    }

    fn priority(&self) -> i32 {
        1
    }

    async fn get_latest_bar(
        &self,
        _request: LatestBarRequest,
    ) -> Result<LatestBar, MarketDataProviderError> {
        Err(MarketDataProviderError::Unsupported("search test".into()))
    }

    async fn get_historical_range(
        &self,
        _request: HistoricalRangeRequest,
    ) -> Result<HistoricalRange, MarketDataProviderError> {
        Err(MarketDataProviderError::Unsupported("search test".into()))
    }

    async fn lookup_ticker(
        &self,
        _request: TickerLookupRequest,
    ) -> Result<TickerLookupResult, MarketDataProviderError> {
        Ok(TickerLookupResult {
            hits: self.hits.clone(),
            source_id: self.source_id.clone(),
        })
    }
}

fn hit(symbol: &str, exchange: &str, name: &str) -> TickerLookupHit {
    TickerLookupHit {
        symbol: symbol.to_string(),
        short_name: Some(name.to_string()),
        long_name: None,
        quote_type: Some("EQUITY".to_string()),
        exchange: Some(exchange.to_string()),
        currency: Some("USD".to_string()),
        schemes: Vec::new(),
        relevance: Some(0.9),
    }
}

fn new_asset(name: &str) -> NewAsset {
    NewAsset {
        asset_type: "EQUITY".to_string(),
        display_name: name.to_string(),
        currency: "USD".to_string(),
        backed_by_portfolio_id: None,
        issuer_id: None,
    }
}

fn snapshot(hits: Vec<TickerLookupHit>) -> DataSourceRegistry {
    DataSourceRegistry::new(vec![Arc::new(SearchProvider::new(hits))]).expect("registry")
}

fn request(query: &str) -> IdentifierSearchRequest {
    IdentifierSearchRequest::new(query, None, None, None, None, None, None).expect("request")
}

async fn row_counts(pool: &sqlx::PgPool) -> (i64, i64) {
    let assets: i64 = sqlx::query_scalar("SELECT count(*) FROM assets")
        .fetch_one(pool)
        .await
        .expect("count assets");
    let identifiers: i64 = sqlx::query_scalar("SELECT count(*) FROM asset_identifiers")
        .fetch_one(pool)
        .await
        .expect("count identifiers");
    (assets, identifiers)
}

#[tokio::test]
async fn an_exact_identifier_marks_the_candidate_known() {
    let Some((pool, _guard)) = tx_pool().await else {
        eprintln!("SKIPPED: no test database configured");
        return;
    };
    let asset = create_asset_with_identifier(&pool, new_asset("Apple Inc."), "TICKER:XNAS", "AAPL")
        .await
        .expect("create asset");

    let registry = snapshot(vec![hit("AAPL", "NMS", "Apple Inc.")]);
    let result = search_identifiers(&registry.snapshot(), &pool, &request("apple"))
        .await
        .expect("search");

    assert_eq!(result.results.len(), 1);
    let known = result.results[0].known.as_ref().expect("known asset");
    assert_eq!(known.asset_id, asset.id);
    assert_eq!(known.match_kind, KnownMatchKind::Exact);
    assert_eq!(known.matched_scheme, "TICKER:XNAS");
    assert_eq!(known.matched_value, "AAPL");
    assert_eq!(known.display_name, "Apple Inc.");
    assert_eq!(known.asset_type, "EQUITY");
    assert_eq!(known.currency, "USD");
}

#[tokio::test]
async fn a_bare_ticker_matches_a_venue_qualified_candidate() {
    let Some((pool, _guard)) = tx_pool().await else {
        eprintln!("SKIPPED: no test database configured");
        return;
    };
    let asset = create_asset_with_identifier(&pool, new_asset("Invesco QQQM"), "TICKER", "QQQM")
        .await
        .expect("create asset");

    let registry = snapshot(vec![hit("QQQM", "NMS", "Invesco QQQM")]);
    let result = search_identifiers(&registry.snapshot(), &pool, &request("qqqm"))
        .await
        .expect("search");

    assert_eq!(result.results.len(), 1);
    let candidate = &result.results[0];
    assert_eq!(candidate.identifiers[0].scheme, "TICKER:XNAS");
    let known = candidate.known.as_ref().expect("known asset");
    assert_eq!(known.asset_id, asset.id);
    assert_eq!(known.match_kind, KnownMatchKind::EquivalentTicker);
}

#[tokio::test]
async fn case_and_whitespace_variants_are_normalized_on_both_sides() {
    let Some((pool, _guard)) = tx_pool().await else {
        eprintln!("SKIPPED: no test database configured");
        return;
    };
    let asset =
        create_asset_with_identifier(&pool, new_asset("Invesco QQQM"), " ticker ", " qqqm ")
            .await
            .expect("create asset");

    let registry = snapshot(vec![hit("QQQM", "NMS", "Invesco QQQM")]);
    let result = search_identifiers(&registry.snapshot(), &pool, &request("qqqm"))
        .await
        .expect("search");

    let known = result.results[0]
        .known
        .as_ref()
        .expect("case variant known");
    assert_eq!(known.asset_id, asset.id);
    assert_eq!(known.match_kind, KnownMatchKind::EquivalentTicker);
}

#[tokio::test]
async fn an_ambiguous_equivalent_ticker_is_not_labelled_known() {
    let Some((pool, _guard)) = tx_pool().await else {
        eprintln!("SKIPPED: no test database configured");
        return;
    };
    create_asset_with_identifier(&pool, new_asset("First ABC"), "TICKER:XNYS", "ABC")
        .await
        .expect("first asset");
    create_asset_with_identifier(&pool, new_asset("Second ABC"), "TICKER:XASE", "ABC")
        .await
        .expect("second asset");

    let registry = snapshot(vec![hit("ABC", "NMS", "ABC")]);
    let result = search_identifiers(&registry.snapshot(), &pool, &request("abc"))
        .await
        .expect("search");

    assert!(
        result.results[0].known.is_none(),
        "two distinct assets share the value, so the match is ambiguous"
    );
}

#[tokio::test]
async fn search_performs_no_writes() {
    let Some((pool, _guard)) = tx_pool().await else {
        eprintln!("SKIPPED: no test database configured");
        return;
    };
    create_asset_with_identifier(&pool, new_asset("Apple Inc."), "TICKER:XNAS", "AAPL")
        .await
        .expect("create asset");

    let before = row_counts(&pool).await;
    let registry = snapshot(vec![
        hit("AAPL", "NMS", "Apple Inc."),
        hit("MSFT", "NMS", "Microsoft"),
    ]);
    let result = search_identifiers(&registry.snapshot(), &pool, &request("apple"))
        .await
        .expect("search");
    let after = row_counts(&pool).await;

    assert!(!result.results.is_empty());
    assert_eq!(before, after, "identifier search must not write rows");
}
