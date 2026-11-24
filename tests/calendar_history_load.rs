//! Calendar-aware bounded history loading.
//!
//! Skipped as a passing no-op when no test database is configured.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::NaiveDate;
use rust_decimal::Decimal;

use obolargus_core::common::tx_pool;
use obolargus_core::domain::asset::create::create_asset_with_identifier;
use obolargus_core::domain::asset::entity::NewAsset;
use obolargus_core::domain::id::AssetId;
use obolargus_core::market_data::calendar::seed_bundled_calendars;
use obolargus_core::market_data::error::MarketDataProviderError;
use obolargus_core::market_data::history_load::{HistoryLoadConfig, run_asset_history_load};
use obolargus_core::market_data::registry::DataSourceRegistry;
use obolargus_core::market_data::trait_::MarketDataProvider;
use obolargus_core::market_data::types::{
    HistoricalRange, HistoricalRangeRequest, LatestBar, LatestBarRequest, PriceBar,
    TickerLookupRequest, TickerLookupResult,
};

struct RangeProvider {
    calls: AtomicUsize,
    bars: Vec<NaiveDate>,
}

impl RangeProvider {
    fn new(bars: &[&str]) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            bars: bars.iter().map(|date| date_value(date)).collect(),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn registry(provider: Arc<Self>) -> DataSourceRegistry {
        DataSourceRegistry::new(vec![provider]).expect("registry")
    }
}

fn date_value(value: &str) -> NaiveDate {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("date")
}

fn bar(date: NaiveDate) -> PriceBar {
    PriceBar {
        as_of_date: date,
        open: Decimal::new(100, 0),
        high: Decimal::new(101, 0),
        low: Decimal::new(99, 0),
        close: Decimal::new(100, 0),
        adjclose: Decimal::new(100, 0),
        volume: 1_000_000,
    }
}

#[async_trait]
impl MarketDataProvider for RangeProvider {
    fn source_id(&self) -> &str {
        "test"
    }

    fn display_name(&self) -> &str {
        "Test source"
    }

    fn supported_schemes(&self) -> &[String] {
        static SCHEMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
        SCHEMES.get_or_init(|| vec!["TICKER:*".to_string()])
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
        Err(MarketDataProviderError::Unsupported(
            "not used in this test".into(),
        ))
    }

    async fn get_historical_range(
        &self,
        request: HistoricalRangeRequest,
    ) -> Result<HistoricalRange, MarketDataProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(HistoricalRange {
            bars: self
                .bars
                .iter()
                .filter(|date| **date >= request.from && **date <= request.to)
                .copied()
                .map(bar)
                .collect(),
            knowledge_date: request.to,
            source_id: "test".to_string(),
        })
    }

    async fn lookup_ticker(
        &self,
        _request: TickerLookupRequest,
    ) -> Result<TickerLookupResult, MarketDataProviderError> {
        Err(MarketDataProviderError::Unsupported(
            "not used in this test".into(),
        ))
    }
}

async fn seed(pool: &sqlx::PgPool) -> AssetId {
    seed_bundled_calendars(pool).await.expect("seed calendars");
    create_asset_with_identifier(
        pool,
        NewAsset {
            asset_type: "stock".into(),
            display_name: "Test Equity".into(),
            currency: "USD".into(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
        "TICKER:XNAS",
        "TEST",
    )
    .await
    .expect("asset")
    .id()
}

fn config(asset_id: AssetId, from: &str, to: &str) -> HistoryLoadConfig {
    HistoryLoadConfig {
        asset_id,
        from: date_value(from),
        to: date_value(to),
        preferred_source: None,
        trigger: "test".into(),
        owner_id: "dev-user".into(),
    }
}

#[tokio::test]
async fn an_entirely_closed_window_returns_without_a_provider_call() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset = seed(&pool).await;
    let provider = Arc::new(RangeProvider::new(&[]));
    let registry = RangeProvider::registry(Arc::clone(&provider));
    let report =
        run_asset_history_load(&pool, &registry, config(asset, "2025-11-27", "2025-11-27"))
            .await
            .expect("history");
    assert_eq!(report.expected_sessions, 0);
    assert_eq!(report.closed_sessions, 1);
    assert_eq!(report.unexpected_sessions, 0);
    assert_eq!(report.calendar_mic.as_deref(), Some("XNAS"));
    assert!(!report.no_data);
    assert_eq!(provider.calls(), 0);
    assert_eq!(report.loaded, 0);
    guard.cleanup().await;
}

#[tokio::test]
async fn a_range_is_requested_once_and_closure_surprises_are_reported() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset = seed(&pool).await;
    let provider = Arc::new(RangeProvider::new(&[
        "2025-11-26",
        "2025-11-27",
        "2025-11-29",
    ]));
    let registry = RangeProvider::registry(Arc::clone(&provider));
    let report =
        run_asset_history_load(&pool, &registry, config(asset, "2025-11-26", "2025-11-29"))
            .await
            .expect("history");
    assert_eq!(report.expected_sessions, 2);
    assert_eq!(report.closed_sessions, 1);
    assert_eq!(report.unexpected_sessions, 2);
    assert_eq!(provider.calls(), 1, "one bounded range call");
    assert_eq!(report.loaded, 3);
    assert!(!report.no_data);
    guard.cleanup().await;
}

#[tokio::test]
async fn an_empty_source_response_stays_no_data() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset = seed(&pool).await;
    let provider = Arc::new(RangeProvider::new(&[]));
    let registry = RangeProvider::registry(Arc::clone(&provider));
    let report =
        run_asset_history_load(&pool, &registry, config(asset, "2025-11-26", "2025-11-28"))
            .await
            .expect("history");
    assert_eq!(report.expected_sessions, 2);
    assert_eq!(report.closed_sessions, 1);
    assert!(report.no_data);
    assert_eq!(provider.calls(), 1);
    guard.cleanup().await;
}
