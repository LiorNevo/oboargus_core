//! Shared provider budget: global/per-provider concurrency, bounded retries,
//! exponential backoff, and the `Retry-After` cap.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use obolargus_core::market_data::error::MarketDataProviderError;
use obolargus_core::market_data::registry::{DataSourceRegistry, ProviderBudget, RETRY_AFTER_CAP};
use obolargus_core::market_data::trait_::MarketDataProvider;
use obolargus_core::market_data::types::{
    HistoricalRange, HistoricalRangeRequest, LatestBar, LatestBarRequest, PriceBar,
    TickerLookupRequest, TickerLookupResult, TypedIdentifier,
};
use rust_decimal::Decimal;

struct Probe {
    id: String,
    calls: AtomicUsize,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
    fail_until: usize,
    delay: Duration,
}

impl Probe {
    fn new(
        id: &str,
        active: Arc<AtomicUsize>,
        max_active: Arc<AtomicUsize>,
        fail_until: usize,
        delay: Duration,
    ) -> Self {
        Self {
            id: id.to_string(),
            calls: AtomicUsize::new(0),
            active,
            max_active,
            fail_until,
            delay,
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl MarketDataProvider for Probe {
    fn source_id(&self) -> &str {
        &self.id
    }
    fn display_name(&self) -> &str {
        "Probe"
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
        request: HistoricalRangeRequest,
    ) -> Result<HistoricalRange, MarketDataProviderError> {
        let now_active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(now_active, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call <= self.fail_until {
            return Err(MarketDataProviderError::Transient("probe failure".into()));
        }
        Ok(HistoricalRange {
            bars: vec![PriceBar {
                as_of_date: request.from,
                open: Decimal::ONE,
                high: Decimal::ONE,
                low: Decimal::ONE,
                close: Decimal::ONE,
                adjclose: Decimal::ONE,
                volume: 1,
            }],
            knowledge_date: request.from,
            source_id: self.id.clone(),
        })
    }
    async fn lookup_ticker(
        &self,
        _request: TickerLookupRequest,
    ) -> Result<TickerLookupResult, MarketDataProviderError> {
        Err(MarketDataProviderError::Unsupported("unused".into()))
    }
}

fn request() -> HistoricalRangeRequest {
    HistoricalRangeRequest {
        identifier: TypedIdentifier::new("TICKER", "AAPL"),
        from: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).expect("date"),
        to: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).expect("date"),
    }
}

fn probe(id: &str) -> (Arc<Probe>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let probe = Arc::new(Probe::new(
        id,
        Arc::clone(&active),
        Arc::clone(&max_active),
        0,
        Duration::from_millis(1),
    ));
    (probe, active, max_active)
}

#[tokio::test]
async fn retries_stop_at_the_attempt_cap() {
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let probe = Arc::new(Probe::new(
        "retry_source",
        active,
        max_active,
        usize::MAX,
        Duration::from_millis(1),
    ));
    let probe_ref = Arc::clone(&probe);
    let registry = DataSourceRegistry::new_with_budget(
        vec![probe],
        ProviderBudget::new(1, 3, Duration::from_millis(5))
            .with_base_backoff(Duration::from_millis(1)),
    )
    .expect("registry");
    let error = registry
        .snapshot()
        .get_historical_range(request())
        .await
        .expect_err("budget exhausted");
    assert!(matches!(error, MarketDataProviderError::Transient(_)));
    assert_eq!(
        probe_ref.calls(),
        3,
        "the attempt cap includes the first try"
    );
}

#[tokio::test]
async fn a_retry_within_the_cap_recovers() {
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let probe = Arc::new(Probe::new(
        "recovering",
        active,
        max_active,
        1,
        Duration::from_millis(1),
    ));
    let probe_ref = Arc::clone(&probe);
    let registry = DataSourceRegistry::new_with_budget(
        vec![probe],
        ProviderBudget::new(1, 3, Duration::from_millis(5))
            .with_base_backoff(Duration::from_millis(1)),
    )
    .expect("registry");
    let range = registry
        .snapshot()
        .get_historical_range(request())
        .await
        .expect("second attempt succeeds");
    assert_eq!(range.source_id, "recovering");
    assert_eq!(probe_ref.calls(), 2);
}

#[test]
fn retry_delay_honors_retry_after_up_to_the_cap() {
    let budget = ProviderBudget::new(4, 3, Duration::from_secs(30));
    let honored = budget.retry_delay(
        &MarketDataProviderError::RateLimited {
            msg: "slow down".into(),
            retry_after: Some(Duration::from_secs(5)),
        },
        1,
    );
    assert_eq!(honored, Duration::from_secs(5));

    let capped = budget.retry_delay(
        &MarketDataProviderError::RateLimited {
            msg: "slow down".into(),
            retry_after: Some(Duration::from_secs(3_600)),
        },
        1,
    );
    assert_eq!(capped, RETRY_AFTER_CAP);
    assert_eq!(capped, Duration::from_secs(60));
}

#[test]
fn exponential_backoff_is_capped() {
    let budget = ProviderBudget::new(4, 5, Duration::from_millis(20))
        .with_base_backoff(Duration::from_millis(5));
    let transient = MarketDataProviderError::Transient("x".into());
    assert_eq!(budget.retry_delay(&transient, 1), Duration::from_millis(5));
    assert_eq!(budget.retry_delay(&transient, 2), Duration::from_millis(10));
    assert_eq!(
        budget.retry_delay(&transient, 3),
        Duration::from_millis(20),
        "the configured cap wins"
    );
    assert_eq!(budget.retry_delay(&transient, 9), Duration::from_millis(20));
}

#[tokio::test]
async fn per_provider_calls_are_serialized_when_the_budget_is_one() {
    let (probe, _, max_active) = probe("serial_source");
    let budget =
        Arc::new(ProviderBudget::new(8, 1, Duration::from_millis(5)).with_provider_concurrency(1));
    let calls = (0..8).map(|_| {
        let probe = Arc::clone(&probe);
        let budget = Arc::clone(&budget);
        async move {
            budget
                .run("serial_source", || probe.get_historical_range(request()))
                .await
        }
    });
    futures::future::join_all(calls).await;
    assert_eq!(max_active.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn the_global_semaphore_bounds_total_concurrency() {
    let global_active = Arc::new(AtomicUsize::new(0));
    let global_max = Arc::new(AtomicUsize::new(0));
    let first = Arc::new(Probe::new(
        "global_a",
        Arc::clone(&global_active),
        Arc::clone(&global_max),
        0,
        Duration::from_millis(25),
    ));
    let second = Arc::new(Probe::new(
        "global_b",
        Arc::clone(&global_active),
        Arc::clone(&global_max),
        0,
        Duration::from_millis(25),
    ));
    let registry = DataSourceRegistry::new_with_budget(
        vec![first, second],
        ProviderBudget::new(2, 1, Duration::from_millis(5)),
    )
    .expect("registry");
    let snapshot = registry.snapshot();
    let calls = (0..12).map(|_| snapshot.get_historical_range(request()));
    let started = Instant::now();
    futures::future::join_all(calls).await;
    assert!(
        global_max.load(Ordering::SeqCst) <= 2,
        "global max was {}",
        global_max.load(Ordering::SeqCst)
    );
    assert!(started.elapsed() >= Duration::from_millis(50));
}

#[test]
fn zero_configuration_is_clamped() {
    let budget = ProviderBudget::new(0, 0, Duration::ZERO);
    assert_eq!(budget.max_concurrency(), 1);
    assert_eq!(budget.max_attempts(), 1);
}
