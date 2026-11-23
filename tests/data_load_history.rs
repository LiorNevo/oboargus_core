//! Database-backed tests for the Data Load history and source-state control.
//!
//! Skipped as a passing no-op when no test database is configured.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use uuid::Uuid;

use obolargus_core::common::tx_pool;
use obolargus_core::domain::asset::create::create_asset;
use obolargus_core::domain::asset::entity::NewAsset;
use obolargus_core::domain::asset_identifier::create::create_asset_identifier;
use obolargus_core::domain::asset_identifier::entity::NewAssetIdentifier;
use obolargus_core::domain::id::AssetId;
use obolargus_core::domain::id::DomainId;
use obolargus_core::market_data::audit::{
    LoadRunContext, finish_load_run, start_load_run_with_context,
};
use obolargus_core::market_data::config::DataSourceConfig;
use obolargus_core::market_data::data_domain::DataDomain;
use obolargus_core::market_data::data_sources::upsert_data_source;
use obolargus_core::market_data::error::MarketDataProviderError;
use obolargus_core::market_data::history::{
    HistoryFilter, get_history_detail, get_history_entry, list_history, range_progress,
};
use obolargus_core::market_data::history_load::{
    HistoryLoadConfig, MAX_HISTORY_WINDOW_DAYS, run_asset_history_load,
};
use obolargus_core::market_data::load::{DataSourceLoadCounters, RunStatus};
use obolargus_core::market_data::load_range::{
    IdentifierRangeLoadConfig, run_identifier_range_load_with_group,
};
use obolargus_core::market_data::registry::DataSourceRegistry;
use obolargus_core::market_data::source_state::set_source_enabled;
use obolargus_core::market_data::trait_::MarketDataProvider;
use obolargus_core::market_data::types::{
    HistoricalRange, HistoricalRangeRequest, LatestBar, LatestBarRequest, TickerLookupRequest,
    TickerLookupResult,
};
use obolargus_core::market_data::types::{PriceBar, TypedIdentifier};

struct Stub;

#[async_trait]
impl MarketDataProvider for Stub {
    fn source_id(&self) -> &str {
        "yahoo_finance"
    }
    fn display_name(&self) -> &str {
        "Stub"
    }
    fn supported_schemes(&self) -> &[String] {
        static SCHEMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
        SCHEMES.get_or_init(|| vec!["TICKER".to_string(), "TICKER:*".to_string()])
    }
    fn enabled(&self) -> bool {
        true
    }
    fn priority(&self) -> i32 {
        10
    }
    async fn get_latest_bar(
        &self,
        _request: LatestBarRequest,
    ) -> Result<LatestBar, MarketDataProviderError> {
        Err(MarketDataProviderError::Unsupported("stub".into()))
    }
    async fn get_historical_range(
        &self,
        request: HistoricalRangeRequest,
    ) -> Result<HistoricalRange, MarketDataProviderError> {
        Ok(HistoricalRange {
            bars: vec![PriceBar {
                as_of_date: request.from,
                open: Decimal::new(100, 0),
                high: Decimal::new(101, 0),
                low: Decimal::new(99, 0),
                close: Decimal::new(100, 0),
                adjclose: Decimal::new(100, 0),
                volume: 1_000,
            }],
            knowledge_date: request.from,
            source_id: "yahoo_finance".to_string(),
        })
    }
    async fn lookup_ticker(
        &self,
        _request: TickerLookupRequest,
    ) -> Result<TickerLookupResult, MarketDataProviderError> {
        Err(MarketDataProviderError::Unsupported("stub".into()))
    }
}

fn registry() -> DataSourceRegistry {
    DataSourceRegistry::new(vec![Arc::new(Stub)]).expect("registry")
}

struct FailingRange {
    error: MarketDataProviderError,
}

#[async_trait]
impl MarketDataProvider for FailingRange {
    fn source_id(&self) -> &str {
        "yahoo_finance"
    }
    fn display_name(&self) -> &str {
        "Failing range"
    }
    fn supported_schemes(&self) -> &[String] {
        static SCHEMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
        SCHEMES.get_or_init(|| vec!["TICKER".to_string(), "TICKER:*".to_string()])
    }
    fn enabled(&self) -> bool {
        true
    }
    fn priority(&self) -> i32 {
        10
    }
    async fn get_latest_bar(
        &self,
        _request: LatestBarRequest,
    ) -> Result<LatestBar, MarketDataProviderError> {
        Err(MarketDataProviderError::Unsupported("stub".into()))
    }
    async fn get_historical_range(
        &self,
        _request: HistoricalRangeRequest,
    ) -> Result<HistoricalRange, MarketDataProviderError> {
        Err(self.error.clone())
    }
    async fn lookup_ticker(
        &self,
        _request: TickerLookupRequest,
    ) -> Result<TickerLookupResult, MarketDataProviderError> {
        Err(MarketDataProviderError::Unsupported("stub".into()))
    }
}

fn failing_registry(error: MarketDataProviderError) -> DataSourceRegistry {
    DataSourceRegistry::new(vec![Arc::new(FailingRange { error })]).expect("registry")
}

#[tokio::test]
async fn history_list_filters_and_detail() {
    let Some((pool, _guard)) = tx_pool().await else {
        eprintln!("SKIPPED: no test database configured");
        return;
    };

    let date = NaiveDate::from_ymd_opt(2026, 10, 1).expect("date");
    let run = start_load_run_with_context(
        &pool,
        "test",
        Some("yahoo_finance"),
        &LoadRunContext::market_data(Some(date)),
    )
    .await
    .expect("start");
    let empty = serde_json::json!([]);
    finish_load_run(
        &pool,
        run.id,
        0,
        0,
        &Default::default(),
        &serde_json::json!({}),
        &serde_json::json!({}),
        &empty,
    )
    .await
    .expect("finish");

    let change_id: Uuid = sqlx::query_scalar(
        "INSERT INTO data_source_state_changes
            (source_id, before_enabled, after_enabled, reason)
         VALUES ('yahoo_finance', true, false, 'test change')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .expect("change");

    let page = list_history(
        &pool,
        &HistoryFilter {
            limit: 50,
            ..Default::default()
        },
        7200,
    )
    .await
    .expect("list");
    assert!(page.total >= 2);
    assert_eq!(page.entries[0].entry_type, "source_change");

    let filtered = list_history(
        &pool,
        &HistoryFilter {
            entry_type: Some("source_change".to_string()),
            limit: 50,
            ..Default::default()
        },
        7200,
    )
    .await
    .expect("filtered");
    assert_eq!(filtered.total, 1);
    assert_eq!(filtered.entries[0].entry_id, change_id);

    let detail = get_history_detail(&pool, run.id, 7200)
        .await
        .expect("detail");
    assert_eq!(
        detail.load.expect("load").report.data_domain,
        DataDomain::MarketData
    );

    let change = get_history_detail(&pool, change_id, 7200)
        .await
        .expect("change");
    assert_eq!(
        change
            .source_change
            .expect("source change")
            .reason
            .as_deref(),
        Some("test change")
    );

    let progress = range_progress(&pool, Uuid::new_v4())
        .await
        .expect("progress");
    assert_eq!(progress.status, "starting");
}

#[tokio::test]
async fn source_toggle_persists_before_runtime_update() {
    let Some((pool, _guard)) = tx_pool().await else {
        return;
    };

    let source = DataSourceConfig::default_yahoo();
    let inserted = upsert_data_source(&pool, &source, false)
        .await
        .expect("upsert");
    assert!(inserted.enabled);

    let registry = registry();
    let disabled = set_source_enabled(
        &pool,
        &registry,
        &source.source_id,
        false,
        Some("test"),
        Some("outage"),
    )
    .await
    .expect("disable");
    assert!(disabled.changed);
    assert!(!disabled.source.enabled);
    assert!(!registry.catalog(None)[0].enabled);
    let change = disabled.change.expect("audit row");
    assert_eq!(change.actor.as_deref(), Some("test"));
    assert_eq!(change.reason.as_deref(), Some("outage"));

    let no_op = set_source_enabled(&pool, &registry, &source.source_id, false, None, None)
        .await
        .expect("no-op");
    assert!(!no_op.changed);
    assert!(no_op.change.is_none());

    let enabled = set_source_enabled(&pool, &registry, &source.source_id, true, None, None)
        .await
        .expect("enable");
    assert!(enabled.changed);
    assert!(enabled.source.enabled);
    assert!(registry.catalog(Some(DataDomain::MarketData))[0].selectable_for_domain);
}

#[tokio::test]
async fn source_toggle_reports_unknown_and_unpersisted_sources() {
    let Some((pool, _guard)) = tx_pool().await else {
        return;
    };
    let registry = registry();

    assert!(matches!(
        set_source_enabled(&pool, &registry, "not-a-source", false, None, None)
            .await
            .expect_err("unknown"),
        MarketDataProviderError::NotFound(_)
    ));
    assert!(matches!(
        set_source_enabled(&pool, &registry, "yahoo_finance", false, None, None)
            .await
            .expect_err("unpersisted"),
        MarketDataProviderError::NotFound(_)
    ));
}

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

#[tokio::test]
async fn range_group_metadata_and_progress_are_durable() {
    let Some((pool, _guard)) = tx_pool().await else {
        eprintln!("SKIPPED: no test database configured");
        return;
    };
    let _asset = seed_asset(&pool, "TICKER:XNAS", "AAPL").await;
    let date_from = NaiveDate::from_ymd_opt(2026, 10, 1).expect("from");
    let date_to = NaiveDate::from_ymd_opt(2026, 10, 2).expect("to");
    let group_id = Uuid::new_v4();
    let report = run_identifier_range_load_with_group(
        Some(&pool),
        &registry(),
        IdentifierRangeLoadConfig {
            identifiers: vec![TypedIdentifier::new("TICKER:XNAS", "AAPL")],
            from: date_from,
            to: date_to,
            preferred_source: None,
            trigger: "test".into(),
            dry_run: false,
            concurrency: 2,
            create_missing_assets: false,
            default_currency: "USD".into(),
        },
        group_id,
        DataDomain::MarketData,
    )
    .await
    .expect("range report");

    assert_eq!(report.range_group_id, group_id);
    assert_eq!(report.dates.len(), 2);
    assert!(
        report
            .dates
            .iter()
            .all(|summary| summary.loaded == 1 && summary.run_id.is_some()),
        "each date persisted one row and carries its audit id"
    );

    let rows: Vec<(String, NaiveDate, NaiveDate, i32, Vec<String>)> = sqlx::query_as(
        "SELECT data_domain, range_from, range_to, range_concurrency, answering_sources
         FROM reference_data_load_runs WHERE range_group_id = $1 ORDER BY as_of_date",
    )
    .bind(group_id)
    .fetch_all(&pool)
    .await
    .expect("rows");
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.0 == "market_data"));
    assert!(
        rows.iter()
            .all(|row| row.1 == date_from && row.2 == date_to)
    );
    assert!(rows.iter().all(|row| row.3 == 2));
    assert!(
        rows.iter()
            .all(|row| row.4 == vec!["yahoo_finance".to_string()])
    );

    let progress = range_progress(&pool, group_id).await.expect("progress");
    assert_eq!(progress.status, "complete");
    assert_eq!(progress.dates_requested, Some(2));
    assert_eq!(progress.dates_completed, 2);
    assert_eq!(progress.dates_with_loads, 2);
}

fn history_config(
    asset_id: obolargus_core::domain::id::AssetId,
    from: NaiveDate,
    to: NaiveDate,
) -> HistoryLoadConfig {
    HistoryLoadConfig {
        asset_id,
        from,
        to,
        preferred_source: Some("yahoo_finance".to_string()),
        trigger: "chart".to_string(),
        owner_id: "dev-user".to_string(),
    }
}

#[tokio::test]
async fn bounded_history_load_writes_bars_and_is_idempotent() {
    let Some((pool, _guard)) = tx_pool().await else {
        return;
    };
    let asset = seed_asset(&pool, "TICKER:XNAS", "HIST").await;
    let asset_id = obolargus_core::domain::id::AssetId::from_uuid(asset);
    let from = NaiveDate::from_ymd_opt(2026, 10, 1).expect("from");
    let to = NaiveDate::from_ymd_opt(2026, 10, 2).expect("to");

    let first = run_asset_history_load(&pool, &registry(), history_config(asset_id, from, to))
        .await
        .expect("first load");
    assert_eq!(first.report_version, 1);
    assert_eq!(first.status, RunStatus::Complete);
    assert_eq!(first.loaded, 1);
    assert_eq!(first.no_op, 0);
    assert!(!first.no_data);
    assert_eq!(first.source_id.as_deref(), Some("yahoo_finance"));
    assert_eq!(first.asset_id, asset);
    assert_eq!(first.from, from);
    assert_eq!(first.to, to);
    assert!(first.run_id.is_some());
    assert!(first.message.is_none());

    let second = run_asset_history_load(&pool, &registry(), history_config(asset_id, from, to))
        .await
        .expect("second load");
    assert_eq!(second.status, RunStatus::Complete);
    assert_eq!(second.loaded, 0);
    assert_eq!(second.no_op, 1);

    let run = first.run_id.expect("run id");
    let row: (
        Option<NaiveDate>,
        Option<NaiveDate>,
        Option<i32>,
        String,
        Vec<String>,
    ) = sqlx::query_as(
        "SELECT range_from, range_to, range_concurrency, data_domain, answering_sources \
             FROM reference_data_load_runs WHERE id = $1",
    )
    .bind(run)
    .fetch_one(&pool)
    .await
    .expect("audit row");
    assert_eq!(row.0, Some(from));
    assert_eq!(row.1, Some(to));
    assert_eq!(row.2, None);
    assert_eq!(row.3, "market_data");
    assert_eq!(row.4, vec!["yahoo_finance".to_string()]);
}

#[tokio::test]
async fn bounded_history_load_validates_inputs_and_assets() {
    let Some((pool, _guard)) = tx_pool().await else {
        return;
    };
    let asset = seed_asset(&pool, "TICKER:XNAS", "VALID").await;
    let asset_id = obolargus_core::domain::id::AssetId::from_uuid(asset);
    let from = NaiveDate::from_ymd_opt(2026, 10, 1).expect("from");
    let to = NaiveDate::from_ymd_opt(2026, 10, 2).expect("to");

    assert!(
        run_asset_history_load(&pool, &registry(), history_config(asset_id, to, from),)
            .await
            .is_err()
    );
    assert!(
        run_asset_history_load(
            &pool,
            &registry(),
            history_config(
                asset_id,
                from,
                Utc::now().date_naive() + chrono::Duration::days(1)
            ),
        )
        .await
        .is_err()
    );
    assert!(
        run_asset_history_load(
            &pool,
            &registry(),
            history_config(
                asset_id,
                from - chrono::Duration::days(MAX_HISTORY_WINDOW_DAYS),
                to,
            ),
        )
        .await
        .is_err()
    );
    assert!(
        run_asset_history_load(
            &pool,
            &registry(),
            history_config(
                obolargus_core::domain::id::AssetId::from_uuid(Uuid::new_v4()),
                from,
                to,
            ),
        )
        .await
        .is_err()
    );

    let without_identifier = create_asset(
        &pool,
        NewAsset {
            asset_type: "Equity".to_string(),
            display_name: "No Identifier History".to_string(),
            currency: "USD".to_string(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
    )
    .await
    .expect("asset");
    let error = run_asset_history_load(
        &pool,
        &registry(),
        history_config(
            obolargus_core::domain::id::AssetId::from_uuid(without_identifier.id),
            from,
            to,
        ),
    )
    .await
    .expect_err("no identifier");
    assert!(matches!(error, MarketDataProviderError::Validation(_)));
}

#[tokio::test]
async fn bounded_history_load_classifies_provider_failures() {
    let Some((pool, _guard)) = tx_pool().await else {
        return;
    };
    let asset = seed_asset(&pool, "TICKER:XNAS", "FAIL").await;
    let asset_id = obolargus_core::domain::id::AssetId::from_uuid(asset);
    let from = NaiveDate::from_ymd_opt(2026, 10, 1).expect("from");
    let to = NaiveDate::from_ymd_opt(2026, 10, 2).expect("to");

    let missing = run_asset_history_load(
        &pool,
        &failing_registry(MarketDataProviderError::NotFound("no bars".into())),
        history_config(asset_id, from, to),
    )
    .await
    .expect("not found is data absence");
    assert!(missing.no_data);
    assert_eq!(missing.loaded, 0);
    assert!(missing.message.is_some());

    let limited = run_asset_history_load(
        &pool,
        &failing_registry(MarketDataProviderError::RateLimited {
            msg: "slow down".into(),
            retry_after: None,
        }),
        history_config(asset_id, from, to),
    )
    .await
    .expect("rate limited report");
    assert_eq!(limited.rate_limited, 1);
    assert_eq!(limited.status, RunStatus::Failed);

    let unavailable = run_asset_history_load(
        &pool,
        &failing_registry(MarketDataProviderError::Transient("down".into())),
        history_config(asset_id, from, to),
    )
    .await
    .expect("transient report");
    assert_eq!(unavailable.source_unavailable, 1);
    assert_eq!(unavailable.status, RunStatus::Failed);

    let rejected = run_asset_history_load(
        &pool,
        &failing_registry(MarketDataProviderError::Validation("bad request".into())),
        history_config(asset_id, from, to),
    )
    .await
    .expect("validation report");
    assert_eq!(rejected.rejected, 1);
    assert_eq!(rejected.status, RunStatus::Failed);
}

fn counters(loaded: i64, rejected: i64) -> DataSourceLoadCounters {
    DataSourceLoadCounters {
        loaded,
        rejected,
        ..DataSourceLoadCounters::default()
    }
}

fn identifier_lines(source: &str) -> serde_json::Value {
    serde_json::json!([{
        "identifier": {"scheme": "TICKER", "value": "AAPL"},
        "asset_id": null,
        "asset_creation": {"status": "not_requested"},
        "attempted": true,
        "outcome": "loaded",
        "source_id": source,
        "per_source": [],
        "stored": null
    }])
}

#[tokio::test]
async fn history_filters_entries_and_reports_progress() {
    let Some((pool, _guard)) = tx_pool().await else {
        return;
    };

    // A completed single-date run with an answering source.
    let completed = start_load_run_with_context(
        &pool,
        "test",
        None,
        &LoadRunContext::market_data(Some(NaiveDate::from_ymd_opt(2026, 9, 30).expect("date"))),
    )
    .await
    .expect("completed run");
    finish_load_run(
        &pool,
        completed.id,
        counters(1, 0).total_requested() as i32,
        0,
        &counters(1, 0),
        &serde_json::json!({"yahoo_finance": 1}),
        &serde_json::json!({}),
        &identifier_lines("yahoo_finance"),
    )
    .await
    .expect("finish completed");
    sqlx::query(
        "UPDATE reference_data_load_runs \
         SET started_at = '2026-09-30T12:00:00Z', ended_at = '2026-09-30T12:00:02Z' \
         WHERE id = $1",
    )
    .bind(completed.id)
    .execute(&pool)
    .await
    .expect("date the completed run");

    // An in-flight run, dated earlier so it is stale for short windows.
    let running = start_load_run_with_context(
        &pool,
        "test",
        None,
        &LoadRunContext::market_data(Some(NaiveDate::from_ymd_opt(2026, 10, 1).expect("date"))),
    )
    .await
    .expect("running run");
    sqlx::query(
        "UPDATE reference_data_load_runs \
         SET started_at = NOW() - INTERVAL '2 hours' WHERE id = $1",
    )
    .bind(running.id)
    .execute(&pool)
    .await
    .expect("age the running run");

    let all = list_history(
        &pool,
        &HistoryFilter {
            limit: 50,
            ..Default::default()
        },
        60,
    )
    .await
    .expect("all");
    assert_eq!(all.total, 2);
    assert!(all.entries.iter().any(|entry| entry.stale));
    assert!(
        all.entries
            .iter()
            .any(|entry| { entry.answering_sources == vec!["yahoo_finance".to_string()] })
    );

    let by_date = list_history(
        &pool,
        &HistoryFilter {
            from: Some(NaiveDate::from_ymd_opt(2026, 9, 30).expect("date")),
            to: Some(NaiveDate::from_ymd_opt(2026, 9, 30).expect("date")),
            limit: 50,
            ..Default::default()
        },
        60,
    )
    .await
    .expect("date filter");
    assert_eq!(by_date.total, 1);
    assert_eq!(by_date.entries[0].entry_id, completed.id);

    let by_domain = list_history(
        &pool,
        &HistoryFilter {
            domain: Some("market_data".to_string()),
            limit: 50,
            ..Default::default()
        },
        60,
    )
    .await
    .expect("domain filter");
    assert_eq!(by_domain.total, 2);

    let by_source = list_history(
        &pool,
        &HistoryFilter {
            source: Some("yahoo_finance".to_string()),
            limit: 50,
            ..Default::default()
        },
        60,
    )
    .await
    .expect("source filter");
    assert_eq!(by_source.total, 1);
    assert_eq!(by_source.entries[0].entry_id, completed.id);

    let complete = list_history(
        &pool,
        &HistoryFilter {
            status: Some("complete".to_string()),
            limit: 50,
            ..Default::default()
        },
        60,
    )
    .await
    .expect("complete filter");
    assert_eq!(complete.total, 1);

    let incomplete = list_history(
        &pool,
        &HistoryFilter {
            status: Some("incomplete".to_string()),
            limit: 50,
            ..Default::default()
        },
        60,
    )
    .await
    .expect("incomplete filter");
    assert_eq!(incomplete.total, 1);
    assert_eq!(incomplete.entries[0].entry_id, running.id);

    let still_running = list_history(
        &pool,
        &HistoryFilter {
            status: Some("running".to_string()),
            limit: 50,
            ..Default::default()
        },
        86_400,
    )
    .await
    .expect("running filter");
    assert_eq!(still_running.total, 1);

    let not_running_yet = list_history(
        &pool,
        &HistoryFilter {
            status: Some("running".to_string()),
            limit: 50,
            ..Default::default()
        },
        60,
    )
    .await
    .expect("short running filter");
    assert_eq!(not_running_yet.total, 0);

    let run_type = list_history(
        &pool,
        &HistoryFilter {
            entry_type: Some("run".to_string()),
            limit: 50,
            ..Default::default()
        },
        60,
    )
    .await
    .expect("run type filter");
    assert_eq!(run_type.total, 2);

    let paged = list_history(
        &pool,
        &HistoryFilter {
            limit: 1,
            offset: 1,
            ..Default::default()
        },
        60,
    )
    .await
    .expect("paged");
    assert_eq!(paged.total, 2);
    assert_eq!(paged.entries.len(), 1);
    assert_eq!(paged.limit, 1);
    assert_eq!(paged.offset, 1);

    assert_eq!(
        get_history_entry(&pool, running.id, 60)
            .await
            .expect("entry")
            .entry_id,
        running.id
    );
    assert!(matches!(
        get_history_entry(&pool, Uuid::new_v4(), 60)
            .await
            .expect_err("missing entry"),
        MarketDataProviderError::NotFound(_)
    ));
    let running_detail = get_history_detail(&pool, running.id, 60)
        .await
        .expect("running detail");
    assert!(running_detail.load.is_none());
    let completed_detail = get_history_detail(&pool, completed.id, 60)
        .await
        .expect("completed detail");
    assert!(completed_detail.load.is_some());

    // Range progress: one failed child plus one still running.
    let group_running = Uuid::new_v4();
    let from = NaiveDate::from_ymd_opt(2026, 10, 1).expect("from");
    let to = NaiveDate::from_ymd_opt(2026, 10, 2).expect("to");
    let failed_child = start_load_run_with_context(
        &pool,
        "test",
        None,
        &LoadRunContext::market_data_range(from, group_running, from, to, 2),
    )
    .await
    .expect("failed child");
    finish_load_run(
        &pool,
        failed_child.id,
        counters(0, 1).total_requested() as i32,
        0,
        &counters(0, 1),
        &serde_json::json!({"yahoo_finance": 1}),
        &serde_json::json!({"yahoo_finance": 1}),
        &serde_json::json!([]),
    )
    .await
    .expect("finish failed child");
    start_load_run_with_context(
        &pool,
        "test",
        None,
        &LoadRunContext::market_data_range(to, group_running, from, to, 2),
    )
    .await
    .expect("running child");

    let progress = range_progress(&pool, group_running)
        .await
        .expect("running progress");
    assert_eq!(progress.status, "running");
    assert_eq!(progress.dates_requested, Some(2));
    assert_eq!(progress.dates_completed, 1);
    assert_eq!(progress.dates_failed, 1);
    assert_eq!(progress.dates_with_loads, 0);

    let range_detail = get_history_detail(&pool, group_running, 60)
        .await
        .expect("range detail");
    assert!(range_detail.range.is_some());
    assert!(range_detail.load.is_none());

    // Failed range.
    let group_failed = Uuid::new_v4();
    let failed = start_load_run_with_context(
        &pool,
        "test",
        None,
        &LoadRunContext::market_data_range(from, group_failed, from, from, 1),
    )
    .await
    .expect("failed run");
    finish_load_run(
        &pool,
        failed.id,
        counters(0, 1).total_requested() as i32,
        0,
        &counters(0, 1),
        &serde_json::json!({}),
        &serde_json::json!({}),
        &serde_json::json!([]),
    )
    .await
    .expect("finish failed");
    let progress = range_progress(&pool, group_failed)
        .await
        .expect("failed progress");
    assert_eq!(progress.status, "failed");

    // Partially failed range.
    let group_partial = Uuid::new_v4();
    let partial = start_load_run_with_context(
        &pool,
        "test",
        None,
        &LoadRunContext::market_data_range(from, group_partial, from, from, 1),
    )
    .await
    .expect("partial run");
    finish_load_run(
        &pool,
        partial.id,
        counters(1, 1).total_requested() as i32,
        0,
        &counters(1, 1),
        &serde_json::json!({}),
        &serde_json::json!({}),
        &serde_json::json!([]),
    )
    .await
    .expect("finish partial");
    let progress = range_progress(&pool, group_partial)
        .await
        .expect("partial progress");
    assert_eq!(progress.status, "partial");

    // Complete range.
    let group_complete = Uuid::new_v4();
    let complete = start_load_run_with_context(
        &pool,
        "test",
        None,
        &LoadRunContext::market_data_range(from, group_complete, from, from, 1),
    )
    .await
    .expect("complete run");
    finish_load_run(
        &pool,
        complete.id,
        counters(1, 0).total_requested() as i32,
        0,
        &counters(1, 0),
        &serde_json::json!({}),
        &serde_json::json!({}),
        &serde_json::json!([]),
    )
    .await
    .expect("finish complete");
    let progress = range_progress(&pool, group_complete)
        .await
        .expect("complete progress");
    assert_eq!(progress.status, "complete");
    assert_eq!(progress.dates_with_loads, 1);
}
