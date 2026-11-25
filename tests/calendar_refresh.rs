//! Refresh skips an asset whose latest completed session is already stored.
//!
//! Skipped as a passing no-op when no test database is configured.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;

use obolargus_core::common::tx_pool;
use obolargus_core::domain::asset::create::create_asset_with_identifier;
use obolargus_core::domain::asset::entity::NewAsset;
use obolargus_core::domain::id::AssetId;
use obolargus_core::market_data::calendar::{
    current_session_candidate, latest_completed_session, seed_bundled_calendars,
};
use obolargus_core::market_data::error::MarketDataProviderError;
use obolargus_core::market_data::refresh::{
    RefreshConfig, run_refresh, run_refresh_including_current,
};
use obolargus_core::market_data::registry::DataSourceRegistry;
use obolargus_core::market_data::trait_::MarketDataProvider;
use obolargus_core::market_data::types::{
    HistoricalRange, HistoricalRangeRequest, LatestBar, LatestBarRequest, PriceBar,
    TickerLookupRequest, TickerLookupResult,
};
use obolargus_core::temporal::{
    Adjustment, FactInput, FactSubject, FactValue, KnownAt, MarketPriceValue, ValidAt,
    write_fact_outcome,
};

struct LatestProvider {
    calls: AtomicUsize,
    date: NaiveDate,
}

impl LatestProvider {
    fn new(date: NaiveDate) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            date,
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
impl MarketDataProvider for LatestProvider {
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
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(LatestBar {
            bar: bar(self.date),
            knowledge_date: self.date,
            source_id: "test".to_string(),
        })
    }

    async fn get_historical_range(
        &self,
        _request: HistoricalRangeRequest,
    ) -> Result<HistoricalRange, MarketDataProviderError> {
        Err(MarketDataProviderError::Unsupported(
            "not used in this test".into(),
        ))
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

async fn store(pool: &sqlx::PgPool, asset_id: AssetId, date: NaiveDate) {
    write_fact_outcome(
        pool,
        FactInput {
            subject: FactSubject::Asset(asset_id),
            fact_type: "market_price".to_string(),
            valid_at: ValidAt::midnight_utc(date),
            known_at: KnownAt::now(),
            source: "test".to_string(),
            value: FactValue::MarketPrice(MarketPriceValue {
                open: Decimal::new(100, 0),
                high: Decimal::new(101, 0),
                low: Decimal::new(99, 0),
                close: Decimal::new(100, 0),
                adjclose: Some(Decimal::new(100, 0)),
                volume: Some(Decimal::new(1_000_000, 0)),
                currency: "USD".to_string(),
                adjustment: Adjustment::Unadjusted,
            }),
        },
    )
    .await
    .expect("stored bar");
}

fn instant(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("timestamp")
        .with_timezone(&Utc)
}

#[tokio::test]
async fn a_stored_latest_session_skips_the_provider() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset = seed(&pool).await;
    let completed = latest_completed_session(&pool, asset, Utc::now())
        .await
        .expect("latest session")
        .expect("session");
    store(&pool, asset, completed).await;

    let provider = Arc::new(LatestProvider::new(completed));
    let registry = LatestProvider::registry(Arc::clone(&provider));
    let summary = run_refresh(
        &pool,
        &registry,
        RefreshConfig {
            assets: Some(vec![asset]),
            trigger: "test".into(),
        },
    )
    .await
    .expect("refresh");
    assert_eq!(provider.calls(), 0);
    assert_eq!(summary.assets_no_op, 1);
    assert_eq!(summary.assets_refreshed, 0);
    assert_eq!(summary.latest_completed_session, Some(completed));
    guard.cleanup().await;
}

#[tokio::test]
async fn the_current_session_refreshes_before_it_closes() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset = seed(&pool).await;
    let now = Utc::now();
    let Some(current) = current_session_candidate(&pool, asset, now)
        .await
        .expect("current session")
    else {
        guard.cleanup().await;
        return;
    };
    let Some(completed) = latest_completed_session(&pool, asset, now)
        .await
        .expect("latest session")
    else {
        guard.cleanup().await;
        return;
    };
    if current <= completed {
        guard.cleanup().await;
        return;
    }
    store(&pool, asset, completed).await;

    let provider = Arc::new(LatestProvider::new(current));
    let registry = LatestProvider::registry(Arc::clone(&provider));
    let summary = run_refresh_including_current(
        &pool,
        &registry,
        RefreshConfig {
            assets: Some(vec![asset]),
            trigger: "test".into(),
        },
    )
    .await
    .expect("refresh");
    assert_eq!(provider.calls(), 1);
    assert_eq!(summary.assets_refreshed, 1);
    assert_eq!(summary.assets_no_op, 0);
    assert_eq!(summary.latest_completed_session, Some(completed));
    guard.cleanup().await;
}

#[tokio::test]
async fn an_older_stored_session_refreshes_once() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset = seed(&pool).await;
    let completed = latest_completed_session(&pool, asset, Utc::now())
        .await
        .expect("latest session")
        .expect("session");
    let older = completed - chrono::Duration::days(1);
    store(&pool, asset, older).await;

    let provider = Arc::new(LatestProvider::new(completed));
    let registry = LatestProvider::registry(Arc::clone(&provider));
    let summary = run_refresh(
        &pool,
        &registry,
        RefreshConfig {
            assets: Some(vec![asset]),
            trigger: "test".into(),
        },
    )
    .await
    .expect("refresh");
    assert_eq!(provider.calls(), 1);
    assert_eq!(summary.assets_refreshed, 1);
    guard.cleanup().await;
}

#[tokio::test]
async fn a_holiday_is_not_a_missing_session() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset = seed(&pool).await;

    let before_close = latest_completed_session(&pool, asset, instant("2026-11-27T02:00:00Z"))
        .await
        .expect("before close")
        .expect("session");
    assert_eq!(before_close, date_value("2026-11-25"));

    let after_close = latest_completed_session(&pool, asset, instant("2026-11-27T22:00:00Z"))
        .await
        .expect("after close")
        .expect("session");
    assert_eq!(after_close, date_value("2026-11-27"));
    guard.cleanup().await;
}
