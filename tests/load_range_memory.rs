//! Bounded range memory: the report keeps one summary per date and the
//! original requested count, never one full per-identifier report per date.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use chrono::NaiveDate;
use obolargus_core::market_data::data_domain::DataDomain;
use obolargus_core::market_data::error::MarketDataProviderError;
use obolargus_core::market_data::load::RunStatus;
use obolargus_core::market_data::load_range::{
    IdentifierRangeLoadConfig, run_identifier_range_load,
    run_identifier_range_load_with_group_cancellable,
};
use obolargus_core::market_data::registry::DataSourceRegistry;
use obolargus_core::market_data::trait_::MarketDataProvider;
use obolargus_core::market_data::types::{
    HistoricalRange, HistoricalRangeRequest, LatestBar, LatestBarRequest, TickerLookupRequest,
    TickerLookupResult, TypedIdentifier,
};
use uuid::Uuid;

struct SilentProvider;

#[async_trait::async_trait]
impl MarketDataProvider for SilentProvider {
    fn source_id(&self) -> &str {
        "mock"
    }
    fn display_name(&self) -> &str {
        "mock"
    }
    fn supported_schemes(&self) -> &[String] {
        static SCHEMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
        SCHEMES.get_or_init(|| vec!["TICKER".to_string()])
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
        Err(MarketDataProviderError::Unsupported("unused".into()))
    }
    async fn get_historical_range(
        &self,
        _request: HistoricalRangeRequest,
    ) -> Result<HistoricalRange, MarketDataProviderError> {
        Err(MarketDataProviderError::Unsupported(
            "dry run contacts nothing".into(),
        ))
    }
    async fn lookup_ticker(
        &self,
        _request: TickerLookupRequest,
    ) -> Result<TickerLookupResult, MarketDataProviderError> {
        Err(MarketDataProviderError::Unsupported("unused".into()))
    }
}

fn date(value: &str) -> NaiveDate {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("date")
}

fn registry() -> DataSourceRegistry {
    DataSourceRegistry::new(vec![
        Arc::new(SilentProvider) as Arc<dyn MarketDataProvider + Send + Sync>
    ])
    .expect("registry")
}

fn config(
    from: &str,
    to: &str,
    concurrency: usize,
    identifiers: usize,
) -> IdentifierRangeLoadConfig {
    IdentifierRangeLoadConfig {
        identifiers: (0..identifiers)
            .map(|index| TypedIdentifier::new("TICKER", format!("T{index}")))
            .collect(),
        from: date(from),
        to: date(to),
        preferred_source: None,
        trigger: "test".into(),
        dry_run: true,
        concurrency,
        create_missing_assets: false,
        default_currency: "USD".into(),
    }
}

#[tokio::test]
async fn a_multi_year_dry_run_keeps_one_summary_per_date_and_the_full_count() {
    let report =
        run_identifier_range_load(None, &registry(), config("2020-01-01", "2024-12-30", 8, 10))
            .await
            .expect("report");
    assert_eq!(report.dates_requested, 1_826);
    assert_eq!(report.dates_completed, 1_826);
    assert_eq!(report.dates.len(), 1_826);
    assert_eq!(report.status, RunStatus::Complete);
    assert_eq!(report.run_ids.len(), 1_826);
}

#[tokio::test]
async fn a_cancelled_run_preserves_the_requested_range() {
    let cancel = Arc::new(AtomicBool::new(true));
    let report = run_identifier_range_load_with_group_cancellable(
        None,
        &registry(),
        config("2020-01-01", "2024-12-30", 8, 10),
        Uuid::new_v4(),
        DataDomain::MarketData,
        Some(cancel),
    )
    .await
    .expect("report");
    assert_eq!(report.dates_requested, 1_826);
    assert_eq!(report.dates_completed, 0);
    assert!(report.dates.is_empty());
    assert_eq!(report.status, RunStatus::Cancelled);
}

#[tokio::test]
async fn date_summaries_carry_counters_but_no_per_identifier_payload() {
    let report =
        run_identifier_range_load(None, &registry(), config("2026-10-01", "2026-10-02", 1, 3))
            .await
            .expect("report");
    let summary = report.dates.first().expect("summary");
    assert_eq!(summary.date, date("2026-10-01"));
    assert_eq!(summary.identifiers_attempted, 0);
    assert_eq!(summary.status, RunStatus::Complete);

    let json = serde_json::to_value(summary).expect("json");
    assert!(json.get("identifiers").is_none());
    assert!(json.get("report").is_none());
    assert!(json.get("per_source_attempted").is_some());
    assert!(json.get("duration_ms").is_some());
}

#[tokio::test]
async fn a_long_run_with_many_identifiers_does_not_retain_per_identifier_lines() {
    // The report type itself proves the property: a range report's `dates`
    // is `Vec<DateLoadSummary>`, and a summary has no identifier vector.
    let report = run_identifier_range_load(
        None,
        &registry(),
        config("2026-01-01", "2026-06-30", 4, 500),
    )
    .await
    .expect("report");
    assert_eq!(report.dates_requested, 181);
    assert_eq!(report.dates.len(), 181);
    let payload = serde_json::to_vec(&report).expect("json");
    // 181 summaries of counters stay well below the hundreds of thousands of
    // identifier lines a full per-date retention would produce.
    assert!(
        payload.len() < 400_000,
        "range report grew to {} bytes",
        payload.len()
    );
}
