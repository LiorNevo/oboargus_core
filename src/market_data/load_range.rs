//! Range wrapper around the single-date load operation.
//!
//! Implements FR-014 of `specs/018-load-reference-data/spec.md`. Given a
//! `from`/`to` range and a window size `x`, it drives the 018 primitive
//! ([`run_identifier_load`]) once per **calendar day** in the range, with at
//! most `x` dates in flight at a time, and aggregates the per-date execution
//! reports into one deterministic range report.
//!
//! Every calendar day is attempted — there is no trading calendar and no
//! weekend filter. A date the upstream has no data for writes no
//! reference-data rows (the primitive's `no_data_for_date` outcome) and still
//! appears in the report, so the operator can see exactly which days
//! produced nothing and why.
//!
//! The primitive is used unchanged: this module never fetches, stores or
//! classifies on its own, and each date keeps its own
//! `reference_data_load_runs` audit row.

#![forbid(unsafe_code)]

use chrono::{NaiveDate, Utc};
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::market_data::error::MarketDataProviderError;
use crate::market_data::load::{
    IdentifierLoadConfig, IdentifierLoadReport, RunStatus, run_identifier_load,
    validate_as_of_date, validate_identifier_pair, validate_preferred_source,
};
use crate::market_data::registry::DataSourceRegistry;
use crate::market_data::types::TypedIdentifier;

/// The largest accepted concurrency window.
///
/// Keeps a range run from stampeding the database pool and the upstream
/// budget by accident. `x` is a scheduling window, not a promise of
/// provider-level parallelism.
///
/// Implements FR-014 of spec 018.
pub const MAX_CONCURRENCY: usize = 32;

/// Input of the range load operation.
///
/// Implements FR-014 of spec 018.
#[derive(Debug, Clone)]
pub struct IdentifierRangeLoadConfig {
    /// The batch as received (the primitive de-duplicates per date).
    pub identifiers: Vec<TypedIdentifier>,
    /// First date to attempt (inclusive).
    pub from: NaiveDate,
    /// Last date to attempt (inclusive).
    pub to: NaiveDate,
    /// The caller's preferred upstream source id (FR-013).
    pub preferred_source: Option<String>,
    /// Audit trigger: `"cli"`, `"api"` or `"test"`.
    pub trigger: String,
    /// When `true`, nothing is persisted for any date (FR-008).
    pub dry_run: bool,
    /// The size-`x` window: at most this many dates in flight.
    pub concurrency: usize,
    /// Passed through to every date's load (FR-014).
    pub create_missing_assets: bool,
    /// Passed through to every date's load (FR-015).
    pub default_currency: String,
}

/// One date's section of the range report.
///
/// Implements FR-014 of spec 018.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DateLoadSection {
    /// The date this section covers.
    pub date: NaiveDate,
    /// The complete single-date execution report for that date.
    pub report: IdentifierLoadReport,
}

/// The range report's summed counters.
///
/// The outcome counters are sums over the dates. `identifiers_attempted` is
/// therefore a **date-line count**, not a batch size: one identifier that is
/// attempted on five dates counts five.
///
/// Implements FR-014 / FR-016 of spec 018.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RangeLoadTotals {
    /// Unique identifier-lines that reached the fetch stage.
    pub identifiers_attempted: usize,
    /// A row was written.
    pub loaded: usize,
    /// The identical fact was already known.
    pub no_op: usize,
    /// No asset claims the identifier.
    pub not_resolved: usize,
    /// The source holds no value for that date.
    pub no_data_for_date: usize,
    /// No enabled source claims the identifier's scheme.
    pub unsupported_scheme: usize,
    /// The source refused the request.
    pub rejected: usize,
    /// A transient or infrastructure failure.
    pub source_unavailable: usize,
    /// The source's budget was exhausted.
    pub rate_limited: usize,
    /// The store refused the write.
    pub storage_failure: usize,
    /// Assets created by any date of the run (FR-016).
    pub assets_created: usize,
    /// Assets adopted after a concurrent creation race (FR-016).
    pub assets_adopted: usize,
}

/// The execution report returned by a range load.
///
/// Implements FR-014 of spec 018.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct IdentifierRangeLoadReport {
    /// The report schema version (FR-007). Currently `1`.
    pub report_version: u32,
    /// First requested date.
    pub from: NaiveDate,
    /// Last requested date.
    pub to: NaiveDate,
    /// The size-`x` window the run used.
    pub concurrency: usize,
    /// The worst status across the dates (`failed` > `partial` > `complete`).
    pub status: RunStatus,
    /// The number of calendar days in `from..=to`.
    pub dates_requested: usize,
    /// Dates whose section wrote at least one row (`loaded > 0`).
    pub dates_with_loads: usize,
    /// Dates whose section neither wrote nor confirmed a row
    /// (`loaded == 0` and `no_op == 0`) — weekends, holidays and gaps.
    pub dates_with_nothing_stored: usize,
    /// The dates' counters, summed.
    pub totals: RangeLoadTotals,
    /// One run-audit id per date, in date order (`null` for a dry run).
    pub run_ids: Vec<Option<Uuid>>,
    /// The per-date sections, in ascending date order.
    pub dates: Vec<DateLoadSection>,
    /// When the range run started.
    pub started_at: chrono::DateTime<Utc>,
    /// When the range run ended.
    pub ended_at: chrono::DateTime<Utc>,
    /// The range run's wall-clock duration.
    pub duration_ms: u64,
}

/// Drives a range load: validate → fan out over dates → aggregate.
///
/// Each date is one [`run_identifier_load`] call, so the single-date
/// contract (outcomes, audit row, idempotence) is inherited unchanged. At
/// most `config.concurrency` dates are in flight; the results are re-sorted
/// by date before aggregation, so two runs of the same request produce
/// comparable reports.
///
/// A hard error from any date (audit-write or infrastructure failure) aborts
/// the call with the **earliest** date's error. Completed dates stay
/// persisted, and retrying is safe because a same-day reload is a `no-op`.
///
/// Implements FR-014 of spec 018.
pub async fn run_identifier_range_load(
    pool: Option<&sqlx::PgPool>,
    registry: &DataSourceRegistry,
    config: IdentifierRangeLoadConfig,
) -> Result<IdentifierRangeLoadReport, MarketDataProviderError> {
    validate_range_request(&config, registry)?;

    let dates = enumerate_dates(config.from, config.to);
    let started_at = Utc::now();

    let mut results: Vec<(
        NaiveDate,
        Result<IdentifierLoadReport, MarketDataProviderError>,
    )> = stream::iter(dates.into_iter().map(|date| {
        let per_date = config_for_date(&config, date);
        async move { (date, run_identifier_load(pool, registry, per_date).await) }
    }))
    .buffer_unordered(config.concurrency)
    .collect()
    .await;

    // `buffer_unordered` yields in completion order; the report must be in
    // date order so two runs are comparable line for line.
    results.sort_by_key(|(date, _)| *date);

    let ended_at = Utc::now();

    let mut sections: Vec<DateLoadSection> = Vec::with_capacity(results.len());
    let mut first_error: Option<MarketDataProviderError> = None;
    for (date, result) in results {
        match result {
            Ok(report) => sections.push(DateLoadSection { date, report }),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }
    if let Some(error) = first_error {
        return Err(error);
    }

    Ok(build_range_report(
        config.from,
        config.to,
        config.concurrency,
        sections,
        started_at,
        ended_at,
    ))
}

/// Validates the range request before any date is attempted.
///
/// Implements FR-002 / FR-014 of spec 018.
fn validate_range_request(
    config: &IdentifierRangeLoadConfig,
    registry: &DataSourceRegistry,
) -> Result<(), MarketDataProviderError> {
    if config.identifiers.is_empty() {
        return Err(MarketDataProviderError::Validation(
            "the identifier list is empty".into(),
        ));
    }
    for identifier in &config.identifiers {
        validate_identifier_pair(identifier)?;
    }
    if config.from > config.to {
        return Err(MarketDataProviderError::Validation(format!(
            "range start {} is later than range end {}",
            config.from, config.to
        )));
    }
    validate_as_of_date(config.from)?;
    validate_as_of_date(config.to)?;
    if config.concurrency == 0 || config.concurrency > MAX_CONCURRENCY {
        return Err(MarketDataProviderError::Validation(format!(
            "concurrency must be between 1 and {MAX_CONCURRENCY}, got {}",
            config.concurrency
        )));
    }
    validate_preferred_source(config.preferred_source.as_deref(), registry)?;
    if config.create_missing_assets && config.default_currency.trim().is_empty() {
        return Err(MarketDataProviderError::Validation(
            "the default asset currency is empty".into(),
        ));
    }
    Ok(())
}

/// Every calendar day in `from..=to`, inclusive.
///
/// Implements FR-014 of spec 018 (no trading calendar: every day is tried).
fn enumerate_dates(from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
    let mut dates = Vec::new();
    let mut current = from;
    loop {
        dates.push(current);
        if current >= to {
            break;
        }
        match current.succ_opt() {
            Some(next) => current = next,
            // Unreachable while `from <= to`, but never loop forever.
            None => break,
        }
    }
    dates
}

/// Builds one date's single-date config.
///
/// Implements FR-014 of spec 018.
fn config_for_date(config: &IdentifierRangeLoadConfig, date: NaiveDate) -> IdentifierLoadConfig {
    IdentifierLoadConfig {
        identifiers: config.identifiers.clone(),
        as_of_date: date,
        preferred_source: config.preferred_source.clone(),
        trigger: config.trigger.clone(),
        dry_run: config.dry_run,
        create_missing_assets: config.create_missing_assets,
        default_currency: config.default_currency.clone(),
    }
}

/// Assembles the range report and its rollups.
///
/// Implements FR-014 of spec 018.
fn build_range_report(
    from: NaiveDate,
    to: NaiveDate,
    concurrency: usize,
    sections: Vec<DateLoadSection>,
    started_at: chrono::DateTime<Utc>,
    ended_at: chrono::DateTime<Utc>,
) -> IdentifierRangeLoadReport {
    let mut totals = RangeLoadTotals::default();
    let mut dates_with_loads = 0usize;
    let mut dates_with_nothing_stored = 0usize;

    for section in &sections {
        let report = &section.report;
        totals.identifiers_attempted += report.identifiers_attempted;
        totals.loaded += report.loaded;
        totals.no_op += report.no_op;
        totals.not_resolved += report.not_resolved;
        totals.no_data_for_date += report.no_data_for_date;
        totals.unsupported_scheme += report.unsupported_scheme;
        totals.rejected += report.rejected;
        totals.source_unavailable += report.source_unavailable;
        totals.rate_limited += report.rate_limited;
        totals.storage_failure += report.storage_failure;
        totals.assets_created += report.assets_created;
        totals.assets_adopted += report.assets_adopted;

        if report.loaded > 0 {
            dates_with_loads += 1;
        }
        if report.loaded == 0 && report.no_op == 0 {
            dates_with_nothing_stored += 1;
        }
    }

    let status = aggregate_status(&sections);
    let run_ids = sections
        .iter()
        .map(|section| section.report.run_id)
        .collect();

    IdentifierRangeLoadReport {
        report_version: 1,
        from,
        to,
        concurrency,
        status,
        dates_requested: sections.len(),
        dates_with_loads,
        dates_with_nothing_stored,
        totals,
        run_ids,
        dates: sections,
        started_at,
        ended_at,
        duration_ms: (ended_at - started_at).num_milliseconds().max(0) as u64,
    }
}

/// The worst date status wins.
///
/// Implements FR-014 of spec 018.
fn aggregate_status(sections: &[DateLoadSection]) -> RunStatus {
    if sections
        .iter()
        .any(|section| section.report.status == RunStatus::Failed)
    {
        return RunStatus::Failed;
    }
    if sections
        .iter()
        .any(|section| section.report.status == RunStatus::Partial)
    {
        return RunStatus::Partial;
    }
    RunStatus::Complete
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use async_trait::async_trait;
    use rust_decimal::Decimal;

    use crate::market_data::load::{DEFAULT_ASSET_CURRENCY, LoadOutcome};
    use crate::market_data::trait_::MarketDataProvider;
    use crate::market_data::types::{
        HistoricalRange, HistoricalRangeRequest, LatestBar, LatestBarRequest, PriceBar,
        TickerLookupRequest, TickerLookupResult,
    };

    struct SilentProvider;

    #[async_trait]
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
            request: HistoricalRangeRequest,
        ) -> Result<HistoricalRange, MarketDataProviderError> {
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
                source_id: "mock".into(),
            })
        }
        async fn lookup_ticker(
            &self,
            _request: TickerLookupRequest,
        ) -> Result<TickerLookupResult, MarketDataProviderError> {
            Err(MarketDataProviderError::Unsupported("unused".into()))
        }
    }

    fn date(ymd: &str) -> NaiveDate {
        NaiveDate::parse_from_str(ymd, "%Y-%m-%d").expect("date")
    }

    fn registry() -> DataSourceRegistry {
        DataSourceRegistry::new(vec![
            Arc::new(SilentProvider) as Arc<dyn MarketDataProvider + Send + Sync>
        ])
        .expect("registry")
    }

    fn config(from: &str, to: &str, concurrency: usize) -> IdentifierRangeLoadConfig {
        IdentifierRangeLoadConfig {
            identifiers: vec![TypedIdentifier::new("TICKER", "AAPL")],
            from: date(from),
            to: date(to),
            preferred_source: None,
            trigger: "test".into(),
            dry_run: true,
            concurrency,
            create_missing_assets: false,
            default_currency: DEFAULT_ASSET_CURRENCY.into(),
        }
    }

    #[test]
    fn every_calendar_day_is_enumerated_including_weekends() {
        // 2026-10-24 is a Saturday, 2026-10-25 a Sunday.
        let dates = enumerate_dates(date("2026-10-24"), date("2026-10-26"));
        assert_eq!(dates.len(), 3);
        assert_eq!(dates[0], date("2026-10-24"));
        assert_eq!(dates[2], date("2026-10-26"));
    }

    #[test]
    fn a_single_day_range_enumerates_one_date() {
        let dates = enumerate_dates(date("2026-10-25"), date("2026-10-25"));
        assert_eq!(dates, vec![date("2026-10-25")]);
    }

    #[test]
    fn a_reversed_range_is_a_validation_error() {
        let registry = registry();
        let error = validate_range_request(&config("2026-10-26", "2026-10-24", 2), &registry)
            .expect_err("validation error");
        assert!(matches!(error, MarketDataProviderError::Validation(_)));
    }

    #[test]
    fn a_future_end_date_is_a_validation_error() {
        let future = (Utc::now().date_naive() + chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        let registry = registry();
        let config = config("2026-10-01", &future, 2);
        let error = validate_range_request(&config, &registry).expect_err("validation error");
        assert!(matches!(error, MarketDataProviderError::Validation(_)));
    }

    #[test]
    fn a_zero_or_oversized_window_is_a_validation_error() {
        let registry = registry();
        for window in [0, MAX_CONCURRENCY + 1] {
            let error =
                validate_range_request(&config("2026-10-01", "2026-10-02", window), &registry)
                    .expect_err("validation error");
            assert!(matches!(error, MarketDataProviderError::Validation(_)));
        }
        assert!(validate_range_request(&config("2026-10-01", "2026-10-02", 1), &registry).is_ok());
        assert!(
            validate_range_request(
                &config("2026-10-01", "2026-10-02", MAX_CONCURRENCY),
                &registry
            )
            .is_ok()
        );
    }

    #[test]
    fn an_empty_batch_is_a_validation_error() {
        let registry = registry();
        let mut config = config("2026-10-01", "2026-10-02", 2);
        config.identifiers.clear();
        let error = validate_range_request(&config, &registry).expect_err("validation error");
        assert!(matches!(error, MarketDataProviderError::Validation(_)));
    }

    #[tokio::test]
    async fn a_dry_run_returns_one_ordered_section_per_date() {
        let report =
            run_identifier_range_load(None, &registry(), config("2026-10-01", "2026-10-05", 2))
                .await
                .expect("report");

        assert_eq!(report.dates_requested, 5);
        assert_eq!(report.dates.len(), 5);
        let dates: Vec<NaiveDate> = report.dates.iter().map(|section| section.date).collect();
        let mut sorted = dates.clone();
        sorted.sort();
        assert_eq!(dates, sorted, "sections are in date order");
        assert_eq!(report.status, RunStatus::Complete);
        assert_eq!(report.run_ids.len(), 5);
        assert!(report.run_ids.iter().all(Option::is_none));
        assert_eq!(report.dates_with_loads, 0);
        assert_eq!(report.dates_with_nothing_stored, 5);
        assert_eq!(report.totals.loaded, 0);
    }

    #[tokio::test]
    async fn a_window_of_one_and_a_window_of_many_agree() {
        let registry = registry();
        let serial =
            run_identifier_range_load(None, &registry, config("2026-10-01", "2026-10-06", 1))
                .await
                .expect("serial");
        let wide =
            run_identifier_range_load(None, &registry, config("2026-10-01", "2026-10-06", 6))
                .await
                .expect("wide");

        assert_eq!(serial.dates.len(), wide.dates.len());
        assert_eq!(serial.totals, wide.totals);
        let serial_dates: Vec<NaiveDate> = serial.dates.iter().map(|s| s.date).collect();
        let wide_dates: Vec<NaiveDate> = wide.dates.iter().map(|s| s.date).collect();
        assert_eq!(serial_dates, wide_dates);
    }

    #[tokio::test]
    async fn a_dry_run_marks_creation_as_would_attempt_only_when_eligible() {
        let mut config = config("2026-10-01", "2026-10-02", 2);
        config.create_missing_assets = true;
        let report = run_identifier_range_load(None, &registry(), config)
            .await
            .expect("report");
        for section in &report.dates {
            for line in &section.report.identifiers {
                assert_eq!(
                    line.asset_creation,
                    crate::market_data::load::AssetCreation::WouldAttempt
                );
                assert_eq!(line.outcome, LoadOutcome::NotResolved);
            }
        }
        assert_eq!(report.totals.assets_created, 0);
    }

    #[tokio::test]
    async fn a_non_dry_run_without_a_pool_is_an_error() {
        let mut config = config("2026-10-01", "2026-10-02", 2);
        config.dry_run = false;
        let error = run_identifier_range_load(None, &registry(), config)
            .await
            .expect_err("infrastructure error");
        assert!(matches!(error, MarketDataProviderError::Infrastructure(_)));
    }
}
