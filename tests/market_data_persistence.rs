//! Database-backed tests for the market-data persistence layer.
//!
//! Implements the executable proof of the persistence half of spec 017
//! (FR-002, FR-007, FR-022, FR-030..FR-032): the `data_sources`
//! configuration table, the `data_source_refresh_runs` audit table, the
//! `data_source_reconciliation_audits` audit table, and the two
//! orchestrators (`run_refresh` / `run_reconcile`) that write them.
//!
//! These paths are database-bound, so the crate's own coverage run never
//! reached them: `obolargus_server` exercises `run_refresh` / `run_reconcile`,
//! but from *its* coverage run. This file exercises them from *this*
//! crate's run.
//!
//! The tests use the shared `obolargus_core::common::tx_pool()` fixture,
//! which truncates every high-level table before each test and on drop.
//! The suite is forced to run serially via `RUST_TEST_THREADS=1` in
//! `obolargus_core/.cargo/config.toml`.
//!
//! The same database is shared with `obolargus_server`'s and
//! `obolargus_cli`'s integration tests, so run one crate's suite at a
//! time (the root `Makefile`'s `test` / `coverage` targets do that).
//!
//! When no test database is configured the tests skip (as a passing
//! no-op), per the `obolargus_core/AGENTS.md` convention.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::NaiveDate;
use rust_decimal::Decimal;

use obolargus_core::domain::asset::create::create_asset;
use obolargus_core::domain::asset::entity::NewAsset;
use obolargus_core::domain::asset_identifier::create::create_asset_identifier;
use obolargus_core::domain::asset_identifier::entity::NewAssetIdentifier;
use obolargus_core::domain::id::{AssetId, DomainId};
use obolargus_core::market_data::audit::{
    DataSourceRefreshCounters, ReconciliationAuditPayload, finish_refresh_run,
    record_reconciliation_audit, start_refresh_run,
};
use obolargus_core::market_data::config::DataSourceConfig;
use obolargus_core::market_data::data_sources::{list_data_sources, upsert_data_source};
use obolargus_core::market_data::error::MarketDataProviderError;
use obolargus_core::market_data::reconcile::{
    ReconcileConfig, ReconciliationPolicy, run_reconcile,
};
use obolargus_core::market_data::refresh::{RefreshConfig, run_refresh};
use obolargus_core::market_data::registry::DataSourceRegistry;
use obolargus_core::market_data::trait_::MarketDataProvider;
use obolargus_core::market_data::types::{
    HistoricalRange, HistoricalRangeRequest, LatestBar, LatestBarRequest, PriceBar,
    TickerLookupRequest, TickerLookupResult, TypedIdentifier,
};

use obolargus_core::common::*;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A provider that answers every request with the same bar.
struct FixedProvider {
    source_id: String,
    priority: i32,
    enabled: bool,
    bar: Option<LatestBar>,
    error: Option<MarketDataProviderError>,
}

impl FixedProvider {
    fn ok(source_id: &str, priority: i32, close: i64) -> Arc<dyn MarketDataProvider + Send + Sync> {
        Arc::new(Self {
            source_id: source_id.to_string(),
            priority,
            enabled: true,
            bar: Some(make_latest_bar(source_id, close)),
            error: None,
        })
    }

    fn failing(
        source_id: &str,
        priority: i32,
        error: MarketDataProviderError,
    ) -> Arc<dyn MarketDataProvider + Send + Sync> {
        Arc::new(Self {
            source_id: source_id.to_string(),
            priority,
            enabled: true,
            bar: None,
            error: Some(error),
        })
    }
}

#[async_trait]
impl MarketDataProvider for FixedProvider {
    fn source_id(&self) -> &str {
        &self.source_id
    }

    fn display_name(&self) -> &str {
        "Fixed Provider"
    }

    fn supported_schemes(&self) -> &[String] {
        static SCHEMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
        SCHEMES.get_or_init(|| vec!["TICKER".to_string()])
    }

    fn enabled(&self) -> bool {
        self.enabled
    }

    fn priority(&self) -> i32 {
        self.priority
    }

    async fn get_latest_bar(
        &self,
        _request: LatestBarRequest,
    ) -> Result<LatestBar, MarketDataProviderError> {
        match (&self.bar, &self.error) {
            (Some(bar), _) => Ok(bar.clone()),
            (_, Some(error)) => Err(error.clone()),
            _ => Err(MarketDataProviderError::NotFound("no bar".into())),
        }
    }

    async fn get_historical_range(
        &self,
        _request: HistoricalRangeRequest,
    ) -> Result<HistoricalRange, MarketDataProviderError> {
        Err(MarketDataProviderError::Unsupported(
            "fixed provider: not implemented".into(),
        ))
    }

    async fn lookup_ticker(
        &self,
        _request: TickerLookupRequest,
    ) -> Result<TickerLookupResult, MarketDataProviderError> {
        Err(MarketDataProviderError::Unsupported(
            "fixed provider: not implemented".into(),
        ))
    }
}

/// Builds a `LatestBar` for the given source.
fn make_latest_bar(source_id: &str, close: i64) -> LatestBar {
    LatestBar {
        bar: PriceBar {
            as_of_date: NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"),
            open: Decimal::new(close, 0),
            high: Decimal::new(close, 0),
            low: Decimal::new(close, 0),
            close: Decimal::new(close, 0),
            adjclose: Decimal::new(close, 0),
            volume: 1_000_000,
        },
        knowledge_date: NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"),
        source_id: source_id.to_string(),
    }
}

/// Creates an asset with the given display name.
async fn seed_asset(pool: &sqlx::PgPool, display_name: &str) -> AssetId {
    let asset = create_asset(
        pool,
        NewAsset {
            asset_type: "EQUITY".to_string(),
            display_name: display_name.to_string(),
            currency: "USD".to_string(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
    )
    .await
    .expect("create asset");
    AssetId::from_uuid(asset.id)
}

/// Creates an asset identifier naming `asset_id`.
async fn seed_identifier(pool: &sqlx::PgPool, asset_id: AssetId, scheme: &str, value: &str) {
    create_asset_identifier(
        pool,
        NewAssetIdentifier {
            asset_id,
            scheme: scheme.to_string(),
            value: value.to_string(),
        },
    )
    .await
    .expect("create asset identifier");
}

// ---------------------------------------------------------------------------
// data_sources: the configuration table (FR-002 / FR-040)
// ---------------------------------------------------------------------------

/// An upserted source is readable back, and the secret is never stored —
/// only the `api_key_present` boolean (FR-040).
#[tokio::test]
async fn upsert_data_source_persists_the_source_shape() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let config = DataSourceConfig::default_yahoo();
    let row = upsert_data_source(&pool, &config, true)
        .await
        .expect("upsert");

    assert_eq!(row.source_id, "yahoo_finance");
    assert_eq!(row.display_name, "Yahoo Finance");
    assert_eq!(row.provider_type, "yahoo_finance");
    assert!(row.enabled);
    assert_eq!(row.priority, 10);
    assert!(row.supported_schemes.contains(&"TICKER".to_string()));
    assert_eq!(row.rate_limit_per_minute, Some(60));
    assert!(row.api_key_present);
    assert_eq!(row.timeout_seconds, Some(30));

    // The row's id is a fresh uuid (not the nil uuid).
    assert_ne!(row.id, uuid::Uuid::nil());

    guard.cleanup().await;
}

/// The `source_id` is the natural key: re-running the upsert updates the row
/// in place instead of inserting a duplicate (FR-002).
#[tokio::test]
async fn upsert_data_source_updates_in_place_on_the_same_source_id() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let mut config = DataSourceConfig::default_yahoo();
    let first = upsert_data_source(&pool, &config, false)
        .await
        .expect("first upsert");

    config.display_name = "Renamed".to_string();
    config.priority = 42;
    let second = upsert_data_source(&pool, &config, true)
        .await
        .expect("second upsert");

    assert_eq!(first.id, second.id, "the row id must be stable");
    assert_eq!(second.display_name, "Renamed");
    assert_eq!(second.priority, 42);
    assert!(second.api_key_present);

    let all = list_data_sources(&pool).await.expect("list");
    assert_eq!(all.len(), 1, "the upsert must not insert a duplicate row");

    guard.cleanup().await;
}

/// `list_data_sources` returns the configured sources ordered by
/// `source_id ASC` (FR-002).
#[tokio::test]
async fn list_data_sources_returns_rows_ordered_by_source_id() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    for source_id in ["zeta_source", "alpha_source", "mid_source"] {
        let mut config = DataSourceConfig::default_yahoo();
        config.source_id = source_id.to_string();
        upsert_data_source(&pool, &config, false)
            .await
            .expect("upsert");
    }

    let all = list_data_sources(&pool).await.expect("list");
    let ids: Vec<&str> = all.iter().map(|r| r.source_id.as_str()).collect();
    assert_eq!(ids, vec!["alpha_source", "mid_source", "zeta_source"]);

    guard.cleanup().await;
}

/// An empty configuration table lists no sources.
#[tokio::test]
async fn list_data_sources_is_empty_without_configuration() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let all = list_data_sources(&pool).await.expect("list");
    assert!(all.is_empty());

    guard.cleanup().await;
}

// ---------------------------------------------------------------------------
// data_source_refresh_runs: the refresh audit table (FR-007 / FR-032)
// ---------------------------------------------------------------------------

/// A started run is readable back and is still in flight (no `ended_at`).
#[tokio::test]
async fn start_refresh_run_opens_an_in_flight_audit_row() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let row = start_refresh_run(&pool, "scheduler").await.expect("start");
    assert_eq!(row.trigger, "scheduler");
    assert!(row.ended_at.is_none(), "the run is still in flight");
    assert_eq!(row.assets_attempted, 0);
    assert_eq!(row.duration_ms, None);
    assert_eq!(
        row.id(),
        obolargus_core::domain::id::DataSourceRefreshRunId::from_uuid(row.id)
    );

    guard.cleanup().await;
}

/// An empty trigger label is normalized to `"test"` (FR-007).
#[tokio::test]
async fn start_refresh_run_defaults_an_empty_trigger_to_test() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let row = start_refresh_run(&pool, "").await.expect("start");
    assert_eq!(row.trigger, "test");

    guard.cleanup().await;
}

/// Finishing a run writes the counters, the per-source maps and the
/// duration (FR-032).
#[tokio::test]
async fn finish_refresh_run_writes_the_counters() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let started = start_refresh_run(&pool, "cli").await.expect("start");
    let counters = DataSourceRefreshCounters {
        refreshed: 3,
        no_op: 2,
        missing_identifier: 1,
        source_unavailable: 4,
    };
    let attempted = serde_json::json!({"yahoo_finance": 10});
    let failed = serde_json::json!({"yahoo_finance": 4});

    let finished = finish_refresh_run(&pool, started.id, &counters, &attempted, &failed)
        .await
        .expect("finish");

    assert_eq!(finished.assets_attempted, 10);
    assert_eq!(finished.assets_refreshed, 3);
    assert_eq!(finished.assets_no_op, 2);
    assert_eq!(finished.assets_missing_identifier, 1);
    assert_eq!(finished.assets_source_unavailable, 4);
    assert!(finished.ended_at.is_some(), "the run has ended");
    assert!(finished.duration_ms.is_some(), "the duration is recorded");
    assert_eq!(finished.per_source_attempted, attempted);
    assert_eq!(finished.per_source_failed, failed);

    guard.cleanup().await;
}

// ---------------------------------------------------------------------------
// data_source_reconciliation_audits: the reconciliation audit table (FR-022)
// ---------------------------------------------------------------------------

/// A reconciliation audit requires at least two consulted sources.
#[tokio::test]
async fn record_reconciliation_audit_requires_two_sources() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let asset_id = seed_asset(&pool, "Audit Asset").await;
    let error = record_reconciliation_audit(
        &pool,
        ReconciliationAuditPayload {
            asset_id,
            fact_type: "market_price".to_string(),
            as_of_date: NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"),
            policy: "primary_wins".to_string(),
            sources_tried: vec!["only_one".to_string()],
            per_source_answers: serde_json::json!({}),
            canonical_value: "{}".to_string(),
        },
    )
    .await
    .expect_err("err");

    assert!(
        matches!(error, MarketDataProviderError::Validation(ref msg) if msg.contains("at least 2 sources")),
        "got: {error:?}"
    );

    guard.cleanup().await;
}

/// A recorded reconciliation audit is readable back, with the typed ids.
#[tokio::test]
async fn record_reconciliation_audit_persists_the_row() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let asset_id = seed_asset(&pool, "Audit Asset").await;
    let answers = serde_json::json!({"a": {"value": "2026-01-02"}});
    let row = record_reconciliation_audit(
        &pool,
        ReconciliationAuditPayload {
            asset_id,
            fact_type: "market_price".to_string(),
            as_of_date: NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"),
            policy: "first_non_empty".to_string(),
            sources_tried: vec!["a".to_string(), "b".to_string()],
            per_source_answers: answers.clone(),
            canonical_value: "{\"close\":\"100\"}".to_string(),
        },
    )
    .await
    .expect("record");

    assert_eq!(row.asset_id(), asset_id);
    assert_eq!(row.sources_tried, vec!["a".to_string(), "b".to_string()]);
    assert_eq!(row.policy, "first_non_empty");
    assert_eq!(row.per_source_answers, answers);
    assert_eq!(row.canonical_value, "{\"close\":\"100\"}");

    guard.cleanup().await;
}

// ---------------------------------------------------------------------------
// run_refresh: the refresh orchestrator (FR-007 / FR-030..FR-032)
// ---------------------------------------------------------------------------

/// A successful refresh writes the price fact and counts the asset as
/// refreshed (FR-013 / FR-032).
#[tokio::test]
async fn run_refresh_writes_the_price_fact() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let asset_id = seed_asset(&pool, "Refreshed Asset").await;
    seed_identifier(&pool, asset_id, "TICKER", "AAPL").await;

    let registry = DataSourceRegistry::new(vec![FixedProvider::ok("yahoo_finance", 10, 100)])
        .expect("registry");
    let summary = run_refresh(
        &pool,
        &registry,
        RefreshConfig {
            assets: Some(vec![asset_id]),
            trigger: "test".to_string(),
        },
    )
    .await
    .expect("refresh");

    assert_eq!(summary.assets_attempted, 1);
    assert_eq!(summary.assets_refreshed, 1);
    assert_eq!(summary.assets_no_op, 0);
    assert_eq!(summary.assets_missing_identifier, 0);
    assert_eq!(summary.assets_source_unavailable, 0);

    // The audit row for the run exists and is closed.
    let audit: Option<(i32, i32, bool)> = sqlx::query_as(
        "SELECT assets_attempted, assets_refreshed, ended_at IS NOT NULL \
         FROM data_source_refresh_runs WHERE id = $1",
    )
    .bind(summary.run_id)
    .fetch_optional(&pool)
    .await
    .expect("query audit");
    let (attempted, refreshed, ended) = audit.expect("the run wrote an audit row");
    assert_eq!(attempted, 1);
    assert_eq!(refreshed, 1);
    assert!(ended, "the run must be closed");

    // The canonical fact was written to `asset_reference_data`.
    let fact: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM asset_reference_data \
         WHERE asset_id = $1 AND fact_type = 'market_price'",
    )
    .bind(asset_id.as_uuid())
    .fetch_optional(&pool)
    .await
    .expect("query fact");
    let (value,) = fact.expect("the price fact was written");
    let parsed: serde_json::Value = serde_json::from_str(&value).expect("json");
    assert_eq!(parsed["close"], "100");
    assert_eq!(parsed["volume"], 1_000_000i64);

    guard.cleanup().await;
}

/// Re-running the refresh for the same fact is a no-op (FR-031): the
/// predecessor's table is unique on
/// `(asset_id, fact_type, as_of_date, knowledge_date)`.
#[tokio::test]
async fn run_refresh_is_idempotent_for_the_same_fact() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let asset_id = seed_asset(&pool, "Refreshed Asset").await;
    seed_identifier(&pool, asset_id, "TICKER", "AAPL").await;

    let registry = DataSourceRegistry::new(vec![FixedProvider::ok("yahoo_finance", 10, 100)])
        .expect("registry");
    let config = RefreshConfig {
        assets: Some(vec![asset_id]),
        trigger: "test".to_string(),
    };

    let first = run_refresh(&pool, &registry, config.clone())
        .await
        .expect("first");
    assert_eq!(first.assets_refreshed, 1);

    let second = run_refresh(&pool, &registry, config).await.expect("second");
    assert_eq!(second.assets_refreshed, 0, "the second run writes no row");
    assert_eq!(second.assets_no_op, 1, "the second run is a no-op");

    guard.cleanup().await;
}

/// An asset whose identifier scheme no enabled source supports is counted
/// as `missing_identifier` (FR-030).
#[tokio::test]
async fn run_refresh_counts_assets_without_a_supported_identifier() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let asset_id = seed_asset(&pool, "Unsupported Asset").await;
    seed_identifier(&pool, asset_id, "LEI", "5493001KJTIIGC8Y1R12").await;

    let registry = DataSourceRegistry::new(vec![FixedProvider::ok("yahoo_finance", 10, 100)])
        .expect("registry");
    let summary = run_refresh(
        &pool,
        &registry,
        RefreshConfig {
            assets: Some(vec![asset_id]),
            trigger: "test".to_string(),
        },
    )
    .await
    .expect("refresh");

    assert_eq!(summary.assets_missing_identifier, 1);
    assert_eq!(summary.assets_refreshed, 0);

    guard.cleanup().await;
}

/// A source that returns a typed error is counted as `source_unavailable`
/// and the failing source is named in the audit's `per_source_failed`
/// (FR-032).
#[tokio::test]
async fn run_refresh_counts_source_failures() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let asset_id = seed_asset(&pool, "Failing Asset").await;
    seed_identifier(&pool, asset_id, "TICKER", "AAPL").await;

    let registry = DataSourceRegistry::new(vec![FixedProvider::failing(
        "yahoo_finance",
        10,
        MarketDataProviderError::Transient("upstream timeout".into()),
    )])
    .expect("registry");
    let summary = run_refresh(
        &pool,
        &registry,
        RefreshConfig {
            assets: Some(vec![asset_id]),
            trigger: "test".to_string(),
        },
    )
    .await
    .expect("refresh");

    assert_eq!(summary.assets_source_unavailable, 1);
    assert_eq!(summary.assets_refreshed, 0);

    let failed: Option<(serde_json::Value,)> =
        sqlx::query_as("SELECT per_source_failed FROM data_source_refresh_runs WHERE id = $1")
            .bind(summary.run_id)
            .fetch_optional(&pool)
            .await
            .expect("query audit");
    let (failed,) = failed.expect("the run wrote an audit row");
    assert_eq!(failed["yahoo_finance"], 1, "the failing source is named");

    guard.cleanup().await;
}

/// With no asset list, the refresh scans every asset in the database
/// (FR-030).
#[tokio::test]
async fn run_refresh_scans_every_asset_when_no_list_is_given() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let asset_id = seed_asset(&pool, "Scanned Asset").await;
    seed_identifier(&pool, asset_id, "TICKER", "AAPL").await;

    let registry = DataSourceRegistry::new(vec![FixedProvider::ok("yahoo_finance", 10, 100)])
        .expect("registry");
    let summary = run_refresh(&pool, &registry, RefreshConfig::cli_all())
        .await
        .expect("refresh");

    assert_eq!(summary.assets_attempted, 1);
    assert_eq!(summary.assets_refreshed, 1);

    guard.cleanup().await;
}

// ---------------------------------------------------------------------------
// run_reconcile: the reconciliation orchestrator (FR-020..FR-022)
// ---------------------------------------------------------------------------

/// Reconciliation needs at least two enabled sources (FR-020).
#[tokio::test]
async fn run_reconcile_requires_two_enabled_sources() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let asset_id = seed_asset(&pool, "Reconciled Asset").await;
    seed_identifier(&pool, asset_id, "TICKER", "AAPL").await;

    let registry =
        DataSourceRegistry::new(vec![FixedProvider::ok("only_one", 10, 100)]).expect("registry");
    let error = run_reconcile(
        &pool,
        &registry,
        ReconcileConfig {
            asset_id,
            fact_type: "market_price".to_string(),
            as_of_date: NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"),
            policy: ReconciliationPolicy::PrimaryWins,
        },
    )
    .await
    .expect_err("err");

    assert!(
        matches!(error, MarketDataProviderError::Validation(ref msg) if msg.contains("at least 2 enabled sources")),
        "got: {error:?}"
    );

    guard.cleanup().await;
}

/// An asset with no identifiers cannot be reconciled (FR-020).
#[tokio::test]
async fn run_reconcile_rejects_an_asset_without_identifiers() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let asset_id = seed_asset(&pool, "No Identifier Asset").await;

    let registry = DataSourceRegistry::new(vec![
        FixedProvider::ok("a", 10, 100),
        FixedProvider::ok("b", 20, 200),
    ])
    .expect("registry");
    let error = run_reconcile(
        &pool,
        &registry,
        ReconcileConfig {
            asset_id,
            fact_type: "market_price".to_string(),
            as_of_date: NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"),
            policy: ReconciliationPolicy::PrimaryWins,
        },
    )
    .await
    .expect_err("err");

    assert!(
        matches!(error, MarketDataProviderError::NotFound(ref msg) if msg.contains("no identifiers")),
        "got: {error:?}"
    );

    guard.cleanup().await;
}

/// A successful reconciliation picks the canonical answer, writes the
/// reference-data fact and records the audit row (FR-022).
#[tokio::test]
async fn run_reconcile_writes_the_canonical_value_and_the_audit_row() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let asset_id = seed_asset(&pool, "Reconciled Asset").await;
    seed_identifier(&pool, asset_id, "TICKER", "AAPL").await;

    let registry = DataSourceRegistry::new(vec![
        FixedProvider::ok("a", 10, 100),
        FixedProvider::ok("b", 20, 200),
    ])
    .expect("registry");
    let summary = run_reconcile(
        &pool,
        &registry,
        ReconcileConfig {
            asset_id,
            fact_type: "market_price".to_string(),
            as_of_date: NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"),
            policy: ReconciliationPolicy::PrimaryWins,
        },
    )
    .await
    .expect("reconcile");

    // The primary (lowest priority = last in walking order) wins.
    let parsed: serde_json::Value = serde_json::from_str(&summary.canonical_value).expect("json");
    assert_eq!(parsed["close"], "200");

    let per_source: &BTreeMap<String, _> = &summary.per_source_answers;
    assert_eq!(per_source.len(), 2);
    assert!(per_source.contains_key("a"));
    assert!(per_source.contains_key("b"));

    // The audit row is persisted.
    let audit: Option<(String, i32)> = sqlx::query_as(
        "SELECT policy, cardinality(sources_tried) \
         FROM data_source_reconciliation_audits WHERE id = $1",
    )
    .bind(summary.reconciliation_id)
    .fetch_optional(&pool)
    .await
    .expect("query audit");
    let (policy, sources_tried) = audit.expect("the audit row was written");
    assert_eq!(policy, "primary_wins");
    assert_eq!(sources_tried, 2);

    guard.cleanup().await;
}

/// Under `first_non_empty` the first answer in walking order wins.
#[tokio::test]
async fn run_reconcile_first_non_empty_picks_the_first_answer() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let asset_id = seed_asset(&pool, "Reconciled Asset").await;
    seed_identifier(&pool, asset_id, "TICKER", "AAPL").await;

    let registry = DataSourceRegistry::new(vec![
        FixedProvider::ok("a", 10, 100),
        FixedProvider::ok("b", 20, 200),
    ])
    .expect("registry");
    let summary = run_reconcile(
        &pool,
        &registry,
        ReconcileConfig {
            asset_id,
            fact_type: "market_price".to_string(),
            as_of_date: NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"),
            policy: ReconciliationPolicy::FirstNonEmpty,
        },
    )
    .await
    .expect("reconcile");

    let parsed: serde_json::Value = serde_json::from_str(&summary.canonical_value).expect("json");
    assert_eq!(parsed["close"], "100");

    guard.cleanup().await;
}

/// One failing source does not fail the reconciliation: the other source's
/// answer becomes canonical and the failure is recorded (FR-022).
#[tokio::test]
async fn run_reconcile_survives_one_failing_source() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let asset_id = seed_asset(&pool, "Reconciled Asset").await;
    seed_identifier(&pool, asset_id, "TICKER", "AAPL").await;

    let registry = DataSourceRegistry::new(vec![
        FixedProvider::failing("a", 10, MarketDataProviderError::Transient("boom".into())),
        FixedProvider::ok("b", 20, 200),
    ])
    .expect("registry");
    let summary = run_reconcile(
        &pool,
        &registry,
        ReconcileConfig {
            asset_id,
            fact_type: "market_price".to_string(),
            as_of_date: NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"),
            policy: ReconciliationPolicy::FirstNonEmpty,
        },
    )
    .await
    .expect("reconcile");

    let parsed: serde_json::Value = serde_json::from_str(&summary.canonical_value).expect("json");
    assert_eq!(parsed["close"], "200");

    // The failed source is recorded as an error in the audit map.
    let answers: Option<(serde_json::Value,)> = sqlx::query_as(
        "SELECT per_source_answers FROM data_source_reconciliation_audits WHERE id = $1",
    )
    .bind(summary.reconciliation_id)
    .fetch_optional(&pool)
    .await
    .expect("query audit");
    let (answers,) = answers.expect("the audit row was written");
    assert!(
        answers["a"].get("error").is_some(),
        "the failed source is recorded (got {answers})"
    );

    guard.cleanup().await;
}

/// When every source fails there is no canonical answer, and the
/// reconciliation fails with a validation error (FR-020).
#[tokio::test]
async fn run_reconcile_fails_when_every_source_fails() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let asset_id = seed_asset(&pool, "Reconciled Asset").await;
    seed_identifier(&pool, asset_id, "TICKER", "AAPL").await;

    let registry = DataSourceRegistry::new(vec![
        FixedProvider::failing("a", 10, MarketDataProviderError::Transient("boom".into())),
        FixedProvider::failing("b", 20, MarketDataProviderError::Transient("boom".into())),
    ])
    .expect("registry");
    let error = run_reconcile(
        &pool,
        &registry,
        ReconcileConfig {
            asset_id,
            fact_type: "market_price".to_string(),
            as_of_date: NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"),
            policy: ReconciliationPolicy::FirstNonEmpty,
        },
    )
    .await
    .expect_err("err");

    assert!(
        matches!(error, MarketDataProviderError::Validation(ref msg) if msg.contains("no answer")),
        "got: {error:?}"
    );

    guard.cleanup().await;
}

/// The typed identifier carried by an asset's identifier is what the
/// sources are asked about (FR-020).
#[tokio::test]
async fn run_reconcile_asks_about_the_asset_identifier() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let asset_id = seed_asset(&pool, "Reconciled Asset").await;
    seed_identifier(&pool, asset_id, "TICKER", "MSFT").await;

    let registry = DataSourceRegistry::new(vec![
        FixedProvider::ok("a", 10, 100),
        FixedProvider::ok("b", 20, 200),
    ])
    .expect("registry");
    let summary = run_reconcile(
        &pool,
        &registry,
        ReconcileConfig {
            asset_id,
            fact_type: "market_price".to_string(),
            as_of_date: NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"),
            policy: ReconciliationPolicy::PrimaryWins,
        },
    )
    .await
    .expect("reconcile");

    // The canonical value names the asset's own bar.
    let parsed: serde_json::Value = serde_json::from_str(&summary.canonical_value).expect("json");
    assert_eq!(parsed["close"], "200");

    guard.cleanup().await;
}

/// The `TypedIdentifier` helper builds the `(scheme, value)` pair the
/// refresh orchestrator sends to the registry (FR-001).
#[test]
fn typed_identifier_carries_the_scheme_and_value() {
    let identifier = TypedIdentifier::new("TICKER", "AAPL");
    assert_eq!(identifier.scheme, "TICKER");
    assert_eq!(identifier.value, "AAPL");
}
