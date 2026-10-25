//! Database-backed tests for the batch reference-data load operation.
//!
//! Implements the executable proof of the persistence half of spec 018
//! (FR-003, FR-004, FR-006, FR-008): the `reference_data_load_runs` audit
//! table, the shared load operation, and the canonical dual-temporal write
//! path it uses.
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

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::NaiveDate;
use rust_decimal::Decimal;

use obolargus_core::domain::asset::create::create_asset;
use obolargus_core::domain::asset::entity::NewAsset;
use obolargus_core::domain::asset_identifier::create::create_asset_identifier;
use obolargus_core::domain::asset_identifier::entity::NewAssetIdentifier;
use obolargus_core::domain::asset_reference_data::read::get_current_asset_reference_data;
use obolargus_core::domain::id::AssetId;
use obolargus_core::market_data::audit::{finish_load_run, get_load_run, start_load_run};
use obolargus_core::market_data::error::MarketDataProviderError;
use obolargus_core::market_data::load::{
    DataSourceLoadCounters, IdentifierLoadConfig, LoadOutcome, RunStatus, run_identifier_load,
};
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

/// A provider that answers a scripted historical range per identifier.
struct ScriptedProvider {
    source_id: String,
    schemes: Vec<String>,
    priority: i32,
    enabled: bool,
    answers: HashMap<String, Result<HistoricalRange, MarketDataProviderError>>,
}

impl ScriptedProvider {
    fn new(source_id: &str, schemes: &[&str], priority: i32) -> Self {
        Self {
            source_id: source_id.to_string(),
            schemes: schemes.iter().map(|s| s.to_string()).collect(),
            priority,
            enabled: true,
            answers: HashMap::new(),
        }
    }

    fn answers(&mut self, key: &str, range: HistoricalRange) {
        self.answers.insert(key.to_string(), Ok(range));
    }

    /// Scripts a failure for one identifier.
    #[allow(dead_code)]
    fn fails(&mut self, key: &str, error: MarketDataProviderError) {
        self.answers.insert(key.to_string(), Err(error));
    }

    fn into_arc(self) -> Arc<dyn MarketDataProvider + Send + Sync> {
        Arc::new(self)
    }
}

#[async_trait]
impl MarketDataProvider for ScriptedProvider {
    fn source_id(&self) -> &str {
        &self.source_id
    }

    fn display_name(&self) -> &str {
        &self.source_id
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
        _request: LatestBarRequest,
    ) -> Result<LatestBar, MarketDataProviderError> {
        Err(MarketDataProviderError::Unsupported(
            "mock: latest bar not implemented".into(),
        ))
    }

    async fn get_historical_range(
        &self,
        request: HistoricalRangeRequest,
    ) -> Result<HistoricalRange, MarketDataProviderError> {
        let key = format!("{}:{}", request.identifier.scheme, request.identifier.value);
        match self.answers.get(&key) {
            Some(Ok(range)) => Ok(range.clone()),
            Some(Err(error)) => Err(error.clone()),
            None => Err(MarketDataProviderError::NotFound(format!(
                "mock {key} not configured"
            ))),
        }
    }

    async fn lookup_ticker(
        &self,
        _request: TickerLookupRequest,
    ) -> Result<TickerLookupResult, MarketDataProviderError> {
        Err(MarketDataProviderError::Unsupported(
            "mock: lookup not implemented".into(),
        ))
    }
}

/// Builds a historical range carrying one bar for `date`.
fn range_for(source: &str, date: NaiveDate) -> HistoricalRange {
    HistoricalRange {
        bars: vec![PriceBar {
            as_of_date: date,
            open: Decimal::new(100, 0),
            high: Decimal::new(101, 0),
            low: Decimal::new(99, 0),
            close: Decimal::new(100, 0),
            adjclose: Decimal::new(100, 0),
            volume: 1_000_000,
        }],
        knowledge_date: date,
        source_id: source.to_string(),
    }
}

/// Creates an asset carrying one identifier, and returns its id.
async fn seed_asset(pool: &sqlx::PgPool, scheme: &str, value: &str) -> uuid::Uuid {
    let asset = create_asset(
        pool,
        NewAsset {
            asset_type: "Equity".to_string(),
            display_name: format!("Test {value}"),
            currency: "USD".to_string(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
    )
    .await
    .expect("asset");

    create_asset_identifier(
        pool,
        NewAssetIdentifier {
            asset_id: AssetId(asset.id),
            scheme: scheme.to_string(),
            value: value.to_string(),
        },
    )
    .await
    .expect("identifier");

    asset.id
}

/// Counts the stored market-price rows for an asset and date.
async fn count_rows(pool: &sqlx::PgPool, asset_id: uuid::Uuid, date: NaiveDate) -> i64 {
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM asset_reference_data WHERE asset_id = $1 AND fact_type = 'market_price' AND as_of_date = $2")
            .bind(asset_id)
            .bind(date)
            .fetch_one(pool)
            .await
            .expect("count");
    count
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// A valid request stores one row per resolved asset and records the run.
#[tokio::test]
async fn valid_request_stores_one_row_per_resolved_asset() {
    let Some((pool, _guard)) = tx_pool().await else {
        eprintln!("SKIPPED: no test database configured");
        return;
    };

    let date = NaiveDate::from_ymd_opt(2026, 10, 24).expect("date");
    let asset_id = seed_asset(&pool, "TICKER:XNAS", "AAPL").await;

    let mut provider = ScriptedProvider::new("yahoo_finance", &["TICKER:*"], 10);
    provider.answers("TICKER:XNAS:AAPL", range_for("yahoo_finance", date));
    let registry = DataSourceRegistry::new(vec![provider.into_arc()]).expect("registry");

    let report = run_identifier_load(
        Some(&pool),
        &registry,
        IdentifierLoadConfig {
            identifiers: vec![TypedIdentifier::new("TICKER:XNAS", "AAPL")],
            as_of_date: date,
            preferred_source: None,
            trigger: "test".into(),
            dry_run: false,
        },
    )
    .await
    .expect("report");

    assert_eq!(report.status, RunStatus::Complete);
    assert_eq!(report.loaded, 1);
    assert_eq!(report.identifiers[0].outcome, LoadOutcome::Loaded);
    assert_eq!(count_rows(&pool, asset_id, date).await, 1);

    // The stored row carries the answering source and the requested date.
    let stored = get_current_asset_reference_data(&pool, AssetId(asset_id), "market_price", date)
        .await
        .expect("read")
        .expect("row");
    assert_eq!(stored.source, "yahoo_finance");
    assert_eq!(stored.as_of_date, date);
}

/// An unknown identifier is skipped and reported; the others still load.
#[tokio::test]
async fn unknown_identifier_is_not_resolved_and_others_still_load() {
    let Some((pool, _guard)) = tx_pool().await else {
        eprintln!("SKIPPED: no test database configured");
        return;
    };

    let date = NaiveDate::from_ymd_opt(2026, 10, 24).expect("date");
    let asset_id = seed_asset(&pool, "TICKER:XNAS", "AAPL").await;

    let mut provider = ScriptedProvider::new("yahoo_finance", &["TICKER:*"], 10);
    provider.answers("TICKER:XNAS:AAPL", range_for("yahoo_finance", date));
    let registry = DataSourceRegistry::new(vec![provider.into_arc()]).expect("registry");

    let report = run_identifier_load(
        Some(&pool),
        &registry,
        IdentifierLoadConfig {
            identifiers: vec![
                TypedIdentifier::new("TICKER:XNAS", "AAPL"),
                TypedIdentifier::new("TICKER:XNAS", "GHOST"),
            ],
            as_of_date: date,
            preferred_source: None,
            trigger: "test".into(),
            dry_run: false,
        },
    )
    .await
    .expect("report");

    assert_eq!(report.not_resolved, 1);
    assert_eq!(report.loaded, 1);
    assert_eq!(report.identifiers[1].outcome, LoadOutcome::NotResolved);
    assert!(!report.identifiers[1].attempted);
    assert_eq!(count_rows(&pool, asset_id, date).await, 1);
}

/// A same-day re-load reports `no_op` and stores nothing new.
#[tokio::test]
async fn same_day_reload_reports_no_op_and_stores_nothing() {
    let Some((pool, _guard)) = tx_pool().await else {
        eprintln!("SKIPPED: no test database configured");
        return;
    };

    let date = NaiveDate::from_ymd_opt(2026, 10, 24).expect("date");
    let asset_id = seed_asset(&pool, "TICKER:XNAS", "AAPL").await;

    let mut provider = ScriptedProvider::new("yahoo_finance", &["TICKER:*"], 10);
    provider.answers("TICKER:XNAS:AAPL", range_for("yahoo_finance", date));
    let registry = DataSourceRegistry::new(vec![provider.into_arc()]).expect("registry");

    let config = IdentifierLoadConfig {
        identifiers: vec![TypedIdentifier::new("TICKER:XNAS", "AAPL")],
        as_of_date: date,
        preferred_source: None,
        trigger: "test".into(),
        dry_run: false,
    };

    let first = run_identifier_load(Some(&pool), &registry, config.clone())
        .await
        .expect("first");
    assert_eq!(first.loaded, 1);

    let second = run_identifier_load(Some(&pool), &registry, config)
        .await
        .expect("second");
    assert_eq!(second.no_op, 1);
    assert_eq!(second.identifiers[0].outcome, LoadOutcome::NoOp);
    assert_eq!(count_rows(&pool, asset_id, date).await, 1);
}

/// The load writes through the canonical dual-temporal write API.
#[tokio::test]
async fn load_writes_through_the_canonical_api() {
    let Some((pool, _guard)) = tx_pool().await else {
        eprintln!("SKIPPED: no test database configured");
        return;
    };

    let date = NaiveDate::from_ymd_opt(2026, 10, 24).expect("date");
    let asset_id = seed_asset(&pool, "TICKER:XNAS", "AAPL").await;

    let mut provider = ScriptedProvider::new("yahoo_finance", &["TICKER:*"], 10);
    provider.answers("TICKER:XNAS:AAPL", range_for("yahoo_finance", date));
    let registry = DataSourceRegistry::new(vec![provider.into_arc()]).expect("registry");

    run_identifier_load(
        Some(&pool),
        &registry,
        IdentifierLoadConfig {
            identifiers: vec![TypedIdentifier::new("TICKER:XNAS", "AAPL")],
            as_of_date: date,
            preferred_source: None,
            trigger: "test".into(),
            dry_run: false,
        },
    )
    .await
    .expect("report");

    let stored = get_current_asset_reference_data(&pool, AssetId(asset_id), "market_price", date)
        .await
        .expect("read")
        .expect("row");
    let value: serde_json::Value = serde_json::from_str(&stored.value).expect("json");
    assert_eq!(value["close"], "100");
    assert_eq!(value["volume"], 1_000_000i64);
    assert!(value["open"].is_string(), "prices are decimal strings");
}

/// The audit row round-trips its counters and the requested source.
#[tokio::test]
async fn start_and_finish_load_run_round_trips() {
    let Some((pool, _guard)) = tx_pool().await else {
        eprintln!("SKIPPED: no test database configured");
        return;
    };

    let run = start_load_run(&pool, "test", Some("yahoo_finance"))
        .await
        .expect("start");
    assert_eq!(run.requested_source.as_deref(), Some("yahoo_finance"));
    assert!(run.ended_at.is_none(), "a new run is in flight");

    let counters = DataSourceLoadCounters {
        loaded: 1,
        not_resolved: 1,
        ..Default::default()
    };
    let per_identifier = serde_json::json!([{"outcome": "loaded"}]);

    let finished = finish_load_run(
        &pool,
        run.id,
        2,
        0,
        &counters,
        &serde_json::json!({"yahoo_finance": 1}),
        &serde_json::json!({}),
        &per_identifier,
    )
    .await
    .expect("finish");

    assert!(finished.ended_at.is_some());
    assert_eq!(finished.loaded, 1);
    assert_eq!(finished.not_resolved, 1);

    let read = get_load_run(&pool, run.id).await.expect("read");
    assert_eq!(read.id, run.id);
    assert_eq!(read.requested_source.as_deref(), Some("yahoo_finance"));
    assert_eq!(read.per_identifier, per_identifier);
}

/// An interrupted run stays in flight and is not readable as completed.
#[tokio::test]
async fn interrupted_run_leaves_an_in_flight_record() {
    let Some((pool, _guard)) = tx_pool().await else {
        eprintln!("SKIPPED: no test database configured");
        return;
    };

    let run = start_load_run(&pool, "test", None).await.expect("start");
    let read = get_load_run(&pool, run.id).await.expect("read");
    assert!(
        read.ended_at.is_none(),
        "an interrupted run is not a completed record"
    );

    // A later run of the same request is unaffected by the orphan.
    let date = NaiveDate::from_ymd_opt(2026, 10, 24).expect("date");
    seed_asset(&pool, "TICKER:XNAS", "AAPL").await;
    let mut provider = ScriptedProvider::new("yahoo_finance", &["TICKER:*"], 10);
    provider.answers("TICKER:XNAS:AAPL", range_for("yahoo_finance", date));
    let registry = DataSourceRegistry::new(vec![provider.into_arc()]).expect("registry");

    let report = run_identifier_load(
        Some(&pool),
        &registry,
        IdentifierLoadConfig {
            identifiers: vec![TypedIdentifier::new("TICKER:XNAS", "AAPL")],
            as_of_date: date,
            preferred_source: None,
            trigger: "test".into(),
            dry_run: false,
        },
    )
    .await
    .expect("report");

    assert_eq!(report.loaded, 1);
    let completed = get_load_run(&pool, report.run_id.expect("run id"))
        .await
        .expect("read");
    assert!(completed.ended_at.is_some(), "the new run is complete");
}

/// A re-load on a later day stores a second row and keeps the history.
#[tokio::test]
async fn later_day_reload_stores_a_second_row_and_keeps_history() {
    let Some((pool, _guard)) = tx_pool().await else {
        eprintln!("SKIPPED: no test database configured");
        return;
    };

    let date = NaiveDate::from_ymd_opt(2026, 10, 24).expect("date");
    let later = NaiveDate::from_ymd_opt(2026, 10, 27).expect("date");
    let asset_id = seed_asset(&pool, "TICKER:XNAS", "AAPL").await;

    let mut provider = ScriptedProvider::new("yahoo_finance", &["TICKER:*"], 10);
    provider.answers("TICKER:XNAS:AAPL", range_for("yahoo_finance", date));
    let registry = DataSourceRegistry::new(vec![provider.into_arc()]).expect("registry");

    run_identifier_load(
        Some(&pool),
        &registry,
        IdentifierLoadConfig {
            identifiers: vec![TypedIdentifier::new("TICKER:XNAS", "AAPL")],
            as_of_date: date,
            preferred_source: None,
            trigger: "test".into(),
            dry_run: false,
        },
    )
    .await
    .expect("first run");

    // The later run carries a later knowledge date, so it is a new row.
    let mut later_provider = ScriptedProvider::new("yahoo_finance", &["TICKER:*"], 10);
    later_provider.answers("TICKER:XNAS:AAPL", range_for("yahoo_finance", later));
    let later_registry =
        DataSourceRegistry::new(vec![later_provider.into_arc()]).expect("registry");

    let report = run_identifier_load(
        Some(&pool),
        &later_registry,
        IdentifierLoadConfig {
            identifiers: vec![TypedIdentifier::new("TICKER:XNAS", "AAPL")],
            as_of_date: later,
            preferred_source: None,
            trigger: "test".into(),
            dry_run: false,
        },
    )
    .await
    .expect("later run");

    assert_eq!(report.loaded, 1);
    assert_eq!(count_rows(&pool, asset_id, later).await, 1);
    // The earlier row remains readable as history.
    assert_eq!(count_rows(&pool, asset_id, date).await, 1);
}

/// Two concurrent same-day loads leave exactly one row for the key: the
/// loser reports `no_op`, never an error (research D12).
#[tokio::test]
async fn concurrent_same_day_loads_leave_one_row() {
    let Some((pool, _guard)) = tx_pool().await else {
        eprintln!("SKIPPED: no test database configured");
        return;
    };

    let date = NaiveDate::from_ymd_opt(2026, 10, 24).expect("date");
    let asset_id = seed_asset(&pool, "TICKER:XNAS", "AAPL").await;

    let mut provider = ScriptedProvider::new("yahoo_finance", &["TICKER:*"], 10);
    provider.answers("TICKER:XNAS:AAPL", range_for("yahoo_finance", date));
    let registry =
        std::sync::Arc::new(DataSourceRegistry::new(vec![provider.into_arc()]).expect("registry"));

    let config = IdentifierLoadConfig {
        identifiers: vec![TypedIdentifier::new("TICKER:XNAS", "AAPL")],
        as_of_date: date,
        preferred_source: None,
        trigger: "test".into(),
        dry_run: false,
    };

    let first = {
        let pool = pool.clone();
        let registry = registry.clone();
        let config = config.clone();
        tokio::spawn(async move { run_identifier_load(Some(&pool), &registry, config).await })
    };
    let second = {
        let pool = pool.clone();
        let registry = registry.clone();
        let config = config.clone();
        tokio::spawn(async move { run_identifier_load(Some(&pool), &registry, config).await })
    };

    let first = first.await.expect("join").expect("first report");
    let second = second.await.expect("join").expect("second report");

    // Exactly one row exists for the dual-temporal key.
    assert_eq!(count_rows(&pool, asset_id, date).await, 1);
    // Neither run failed: the loser is an idempotent no-op.
    assert!(first.identifiers[0].outcome != LoadOutcome::StorageFailure);
    assert!(second.identifiers[0].outcome != LoadOutcome::StorageFailure);
    assert!(
        first.identifiers[0].outcome == LoadOutcome::Loaded
            || second.identifiers[0].outcome == LoadOutcome::Loaded,
        "one of the two runs wrote the row"
    );
}

/// The stored run record agrees with the report's counters.
#[tokio::test]
async fn stored_counters_equal_the_report_counters() {
    let Some((pool, _guard)) = tx_pool().await else {
        eprintln!("SKIPPED: no test database configured");
        return;
    };

    let date = NaiveDate::from_ymd_opt(2026, 10, 24).expect("date");
    seed_asset(&pool, "TICKER:XNAS", "AAPL").await;

    let mut provider = ScriptedProvider::new("yahoo_finance", &["TICKER:*"], 10);
    provider.answers("TICKER:XNAS:AAPL", range_for("yahoo_finance", date));
    let registry = DataSourceRegistry::new(vec![provider.into_arc()]).expect("registry");

    let report = run_identifier_load(
        Some(&pool),
        &registry,
        IdentifierLoadConfig {
            identifiers: vec![TypedIdentifier::new("TICKER:XNAS", "AAPL")],
            as_of_date: date,
            preferred_source: None,
            trigger: "test".into(),
            dry_run: false,
        },
    )
    .await
    .expect("report");

    let run_id = report.run_id.expect("run id");
    let stored = get_load_run(&pool, run_id).await.expect("read");
    assert_eq!(stored.loaded as usize, report.loaded);
    assert_eq!(stored.not_resolved as usize, report.not_resolved);
    assert_eq!(
        stored.identifiers_attempted as usize,
        report.identifiers_attempted
    );
}

/// The persisted per-identifier payload round-trips every line.
#[tokio::test]
async fn per_identifier_outcomes_round_trip_through_jsonb() {
    let Some((pool, _guard)) = tx_pool().await else {
        eprintln!("SKIPPED: no test database configured");
        return;
    };

    let date = NaiveDate::from_ymd_opt(2026, 10, 24).expect("date");
    seed_asset(&pool, "TICKER:XNAS", "AAPL").await;

    let mut provider = ScriptedProvider::new("yahoo_finance", &["TICKER:*"], 10);
    provider.answers("TICKER:XNAS:AAPL", range_for("yahoo_finance", date));
    let registry = DataSourceRegistry::new(vec![provider.into_arc()]).expect("registry");

    let report = run_identifier_load(
        Some(&pool),
        &registry,
        IdentifierLoadConfig {
            identifiers: vec![TypedIdentifier::new("TICKER:XNAS", "AAPL")],
            as_of_date: date,
            preferred_source: None,
            trigger: "test".into(),
            dry_run: false,
        },
    )
    .await
    .expect("report");

    let run_id = report.run_id.expect("run id");
    let stored = get_load_run(&pool, run_id).await.expect("read");
    let lines = stored.per_identifier.as_array().expect("array");
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["outcome"], "loaded");
    assert_eq!(lines[0]["source_id"], "yahoo_finance");
    assert_eq!(lines[0]["attempted"], true);
}
