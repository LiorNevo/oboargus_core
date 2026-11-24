//! Calendar-aware bulk loading: exact closures skip the provider.
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
use obolargus_core::market_data::calendar::seed_bundled_calendars;
use obolargus_core::market_data::error::MarketDataProviderError;
use obolargus_core::market_data::load::{IdentifierLoadConfig, LoadOutcome, run_identifier_load};
use obolargus_core::market_data::load_range::{
    IdentifierRangeLoadConfig, run_identifier_range_load,
};
use obolargus_core::market_data::registry::DataSourceRegistry;
use obolargus_core::market_data::trait_::MarketDataProvider;
use obolargus_core::market_data::types::{
    HistoricalRange, HistoricalRangeRequest, LatestBar, LatestBarRequest, PriceBar,
    TickerLookupRequest, TickerLookupResult, TypedIdentifier,
};

struct CountingProvider {
    calls: AtomicUsize,
}

impl CountingProvider {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn registry(provider: Arc<Self>) -> DataSourceRegistry {
        DataSourceRegistry::new(vec![provider]).expect("registry")
    }
}

#[async_trait]
impl MarketDataProvider for CountingProvider {
    fn source_id(&self) -> &str {
        "test"
    }

    fn display_name(&self) -> &str {
        "Test source"
    }

    fn supported_schemes(&self) -> &[String] {
        static SCHEMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
        SCHEMES.get_or_init(|| vec!["TICKER:*".to_string(), "ISIN".to_string()])
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
            bars: vec![PriceBar {
                as_of_date: request.from,
                open: Decimal::new(100, 0),
                high: Decimal::new(101, 0),
                low: Decimal::new(99, 0),
                close: Decimal::new(100, 0),
                adjclose: Decimal::new(100, 0),
                volume: 1_000_000,
            }],
            knowledge_date: request.from,
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

fn date(value: &str) -> NaiveDate {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("date")
}

fn single_config(identifier: TypedIdentifier, as_of: NaiveDate) -> IdentifierLoadConfig {
    IdentifierLoadConfig {
        identifiers: vec![identifier],
        as_of_date: as_of,
        preferred_source: None,
        trigger: "test".into(),
        dry_run: false,
        create_missing_assets: false,
        default_currency: "USD".into(),
    }
}

async fn seed(pool: &sqlx::PgPool) -> uuid::Uuid {
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
    .0
}

#[tokio::test]
async fn a_single_date_holiday_is_closed_without_a_provider_call() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    seed(&pool).await;
    let provider = Arc::new(CountingProvider::new());
    let registry = CountingProvider::registry(Arc::clone(&provider));
    let report = run_identifier_load(
        Some(&pool),
        &registry,
        single_config(
            TypedIdentifier::new("TICKER:XNAS".to_string(), "TEST".to_string()),
            date("2025-11-27"),
        ),
    )
    .await
    .expect("load");
    assert_eq!(report.identifiers[0].outcome, LoadOutcome::CalendarClosed);
    assert_eq!(report.calendar_closed, 1);
    assert_eq!(provider.calls(), 0);
    assert_eq!(report.identifiers_attempted, 0);
    guard.cleanup().await;
}

#[tokio::test]
async fn an_open_date_still_reaches_the_provider() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    seed(&pool).await;
    let provider = Arc::new(CountingProvider::new());
    let registry = CountingProvider::registry(Arc::clone(&provider));
    let report = run_identifier_load(
        Some(&pool),
        &registry,
        single_config(
            TypedIdentifier::new("TICKER:XNAS".to_string(), "TEST".to_string()),
            date("2025-11-26"),
        ),
    )
    .await
    .expect("load");
    assert_eq!(report.identifiers[0].outcome, LoadOutcome::Loaded);
    assert_eq!(report.calendar_closed, 0);
    assert_eq!(provider.calls(), 1);
    guard.cleanup().await;
}

#[tokio::test]
async fn a_range_reuses_the_calendar_and_skips_only_closures() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    seed(&pool).await;
    let provider = Arc::new(CountingProvider::new());
    let registry = CountingProvider::registry(Arc::clone(&provider));
    let report = run_identifier_range_load(
        Some(&pool),
        &registry,
        IdentifierRangeLoadConfig {
            identifiers: vec![TypedIdentifier::new(
                "TICKER:XNAS".to_string(),
                "TEST".to_string(),
            )],
            from: date("2025-11-26"),
            to: date("2025-11-28"),
            preferred_source: None,
            trigger: "test".into(),
            dry_run: false,
            concurrency: 1,
            create_missing_assets: false,
            default_currency: "USD".into(),
        },
    )
    .await
    .expect("range");
    assert_eq!(report.totals.calendar_closed, 1);
    assert_eq!(report.dates_fully_closed, 1);
    assert_eq!(provider.calls(), 2, "only the two open dates fetch");
    let closed = report
        .dates
        .iter()
        .find(|summary| summary.date == date("2025-11-27"))
        .expect("closed summary");
    assert_eq!(closed.calendar_closed, 1);
    assert_eq!(closed.identifiers_attempted, 0);
    guard.cleanup().await;
}

#[tokio::test]
async fn an_unresolved_venue_keeps_every_day_behavior() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    seed_bundled_calendars(&pool).await.expect("seed calendars");
    create_asset_with_identifier(
        &pool,
        NewAsset {
            asset_type: "stock".into(),
            display_name: "No venue".into(),
            currency: "USD".into(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
        "ISIN",
        "US0000000001",
    )
    .await
    .expect("asset");
    let provider = Arc::new(CountingProvider::new());
    let registry = CountingProvider::registry(Arc::clone(&provider));
    let report = run_identifier_load(
        Some(&pool),
        &registry,
        single_config(
            TypedIdentifier::new("ISIN".to_string(), "US0000000001".to_string()),
            date("2025-11-27"),
        ),
    )
    .await
    .expect("load");
    assert_eq!(report.calendar_closed, 0);
    assert_eq!(provider.calls(), 1, "fallback keeps the provider call");
    guard.cleanup().await;
}
