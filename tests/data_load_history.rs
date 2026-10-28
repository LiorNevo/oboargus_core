//! Database-backed tests for the Data Load history and source-state control.
//!
//! Skipped as a passing no-op when no test database is configured.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use uuid::Uuid;

use obolargus_core::common::tx_pool;
use obolargus_core::domain::asset::create::create_asset;
use obolargus_core::domain::asset::entity::NewAsset;
use obolargus_core::domain::asset_identifier::create::create_asset_identifier;
use obolargus_core::domain::asset_identifier::entity::NewAssetIdentifier;
use obolargus_core::domain::id::AssetId;
use obolargus_core::market_data::audit::{
    LoadRunContext, finish_load_run, start_load_run_with_context,
};
use obolargus_core::market_data::config::DataSourceConfig;
use obolargus_core::market_data::data_domain::DataDomain;
use obolargus_core::market_data::data_sources::upsert_data_source;
use obolargus_core::market_data::error::MarketDataProviderError;
use obolargus_core::market_data::history::{
    HistoryFilter, get_history_detail, list_history, range_progress,
};
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
        eprintln!("SKIPPED: no test database configured");
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
            .all(|section| section.report.identifiers[0].stored.is_some())
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
