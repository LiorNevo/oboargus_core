//! Range wrapper around the single-date load operation.
//!
//! Implements FR-014 of `specs/018-load-reference-data/spec.md`. Given a
//! `from`/`to` range and a window size `x`, it drives the 018 primitive
//! ([`crate::market_data::run_identifier_load`]) once per **calendar day** in the range, with at
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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{NaiveDate, Utc};
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::market_data::audit::LoadRunContext;
use crate::market_data::data_domain::DataDomain;
use crate::market_data::error::MarketDataProviderError;
use crate::market_data::load::{
    IdentifierLoadConfig, IdentifierLoadReport, RunStatus, run_identifier_load_with_snapshot,
    validate_as_of_date, validate_identifier_pair, validate_preferred_source_with_snapshot,
};
use crate::market_data::registry::{DataSourceRegistry, RegistrySnapshot};
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

/// One date's summary line in the range report.
///
/// The full per-identifier report is persisted on that date's
/// `reference_data_load_runs` row before the summary is retained, so a range
/// keeps `concurrency × summaries` in memory instead of one full report per
/// date (NFR-001 / FR-013).
///
/// Implements FR-013 of `specs/034-production-hardening/spec.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DateLoadSummary {
    /// The date this summary covers.
    pub date: NaiveDate,
    /// That date's audit row id (`None` for a dry run).
    pub run_id: Option<Uuid>,
    /// That date's terminal status.
    pub status: RunStatus,
    /// Identifier-lines that reached the fetch stage.
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
    /// Assets created by this date.
    pub assets_created: usize,
    /// Assets adopted by this date.
    pub assets_adopted: usize,
    /// Per-source attempt counts for this date.
    pub per_source_attempted: std::collections::BTreeMap<String, usize>,
    /// Per-source failure counts for this date.
    pub per_source_failed: std::collections::BTreeMap<String, usize>,
    /// When the date's run started.
    pub started_at: chrono::DateTime<Utc>,
    /// When the date's run ended.
    pub ended_at: chrono::DateTime<Utc>,
    /// The date run's wall-clock duration.
    pub duration_ms: u64,
}

impl DateLoadSummary {
    /// Collapses a full per-date report into its bounded summary.
    ///
    /// Implements FR-013 of `specs/034-production-hardening/spec.md`.
    pub fn from_report(date: NaiveDate, report: IdentifierLoadReport) -> Self {
        Self {
            date,
            run_id: report.run_id,
            status: report.status,
            identifiers_attempted: report.identifiers_attempted,
            loaded: report.loaded,
            no_op: report.no_op,
            not_resolved: report.not_resolved,
            no_data_for_date: report.no_data_for_date,
            unsupported_scheme: report.unsupported_scheme,
            rejected: report.rejected,
            source_unavailable: report.source_unavailable,
            rate_limited: report.rate_limited,
            storage_failure: report.storage_failure,
            assets_created: report.assets_created,
            assets_adopted: report.assets_adopted,
            per_source_attempted: report.per_source_attempted,
            per_source_failed: report.per_source_failed,
            started_at: report.started_at,
            ended_at: report.ended_at,
            duration_ms: report.duration_ms,
        }
    }
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
    /// Data domain loaded by this range.
    pub data_domain: DataDomain,
    /// Client-generated (or server-generated) group id shared by every child.
    pub range_group_id: Uuid,
    /// First requested date.
    pub from: NaiveDate,
    /// Last requested date.
    pub to: NaiveDate,
    /// The size-`x` window the run used.
    pub concurrency: usize,
    /// The range's terminal status (`complete`, `partial`, `failed`,
    /// `cancelled`; `interrupted` only when read from history).
    pub status: RunStatus,
    /// The number of calendar days in `from..=to` (never the completed
    /// count — FR-017).
    pub dates_requested: usize,
    /// Dates whose outcome is durably recorded.
    pub dates_completed: usize,
    /// Dates whose outcome is a failure.
    pub dates_failed: usize,
    /// Dates whose section wrote at least one row (`loaded > 0`).
    pub dates_with_loads: usize,
    /// Dates whose section neither wrote nor confirmed a row
    /// (`loaded == 0` and `no_op == 0`) — weekends, holidays and gaps.
    pub dates_with_nothing_stored: usize,
    /// The dates' counters, summed.
    pub totals: RangeLoadTotals,
    /// One run-audit id per date, in date order (`null` for a dry run).
    pub run_ids: Vec<Option<Uuid>>,
    /// The per-date summaries, in ascending date order.
    pub dates: Vec<DateLoadSummary>,
    /// When the range run started.
    pub started_at: chrono::DateTime<Utc>,
    /// When the range run ended.
    pub ended_at: chrono::DateTime<Utc>,
    /// The range run's wall-clock duration.
    pub duration_ms: u64,
}

/// Drives a range load: validate → fan out over dates → aggregate.
///
/// Each date is one [`crate::market_data::run_identifier_load`] call, so the single-date
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
    let snapshot = registry.snapshot();
    run_identifier_range_load_with_snapshot(
        pool,
        &snapshot,
        config,
        Uuid::new_v4(),
        DataDomain::MarketData,
    )
    .await
}

/// Range load with a caller-supplied group id (the browser generates one so
/// it can poll progress before the response arrives).
/// Implements FR-026 / FR-028..FR-031 of spec 022.
pub async fn run_identifier_range_load_with_group(
    pool: Option<&sqlx::PgPool>,
    registry: &DataSourceRegistry,
    config: IdentifierRangeLoadConfig,
    group_id: Uuid,
    data_domain: DataDomain,
) -> Result<IdentifierRangeLoadReport, MarketDataProviderError> {
    run_identifier_range_load_with_group_cancellable(
        pool,
        registry,
        config,
        group_id,
        data_domain,
        None,
    )
    .await
}

/// Range load with an optional cooperative cancellation flag.
///
/// The flag is checked before each date starts. Dates already in flight are
/// allowed to finish so their audit rows are never left half-written.
///
/// Implements FR-014 of spec 018 and FR-031 of spec 022.
pub async fn run_identifier_range_load_with_group_cancellable(
    pool: Option<&sqlx::PgPool>,
    registry: &DataSourceRegistry,
    config: IdentifierRangeLoadConfig,
    group_id: Uuid,
    data_domain: DataDomain,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<IdentifierRangeLoadReport, MarketDataProviderError> {
    run_identifier_range_load_with_group_cancellable_owned(
        pool,
        registry,
        config,
        group_id,
        data_domain,
        cancel,
        "dev-user",
    )
    .await
}

/// Range load with a cooperative cancellation flag and an explicit owner.
///
/// The server passes the authenticated actor's owner id; legacy callers keep
/// the `dev-user` owner through the wrapper above.
///
/// Implements FR-016 / FR-018 of `specs/034-production-hardening/spec.md`.
#[allow(clippy::too_many_arguments)]
pub async fn run_identifier_range_load_with_group_cancellable_owned(
    pool: Option<&sqlx::PgPool>,
    registry: &DataSourceRegistry,
    config: IdentifierRangeLoadConfig,
    group_id: Uuid,
    data_domain: DataDomain,
    cancel: Option<Arc<AtomicBool>>,
    owner_id: &str,
) -> Result<IdentifierRangeLoadReport, MarketDataProviderError> {
    let snapshot = registry.snapshot();
    run_identifier_range_load_with_snapshot_cancellable(
        pool,
        &snapshot,
        config,
        group_id,
        data_domain,
        cancel,
        owner_id,
    )
    .await
}

/// Range load over an already-taken snapshot.
///
/// One snapshot is passed to every date, so enabling/disabling a source while
/// the range is in flight does not change the run's source set.
/// Implements FR-026 / FR-028..FR-031 of spec 022.
pub async fn run_identifier_range_load_with_snapshot(
    pool: Option<&sqlx::PgPool>,
    snapshot: &RegistrySnapshot,
    config: IdentifierRangeLoadConfig,
    group_id: Uuid,
    data_domain: DataDomain,
) -> Result<IdentifierRangeLoadReport, MarketDataProviderError> {
    run_identifier_range_load_with_snapshot_cancellable(
        pool,
        snapshot,
        config,
        group_id,
        data_domain,
        None,
        "dev-user",
    )
    .await
}

async fn run_identifier_range_load_with_snapshot_cancellable(
    pool: Option<&sqlx::PgPool>,
    snapshot: &RegistrySnapshot,
    config: IdentifierRangeLoadConfig,
    group_id: Uuid,
    data_domain: DataDomain,
    cancel: Option<Arc<AtomicBool>>,
    owner_id: &str,
) -> Result<IdentifierRangeLoadReport, MarketDataProviderError> {
    validate_range_request_with_snapshot(&config, snapshot)?;

    if let Some(pool) = pool {
        crate::temporal::observations::ensure_market_observation_partitions(
            pool,
            config.from,
            config.to,
        )
        .await
        .map_err(|error| {
            MarketDataProviderError::Infrastructure(format!("observation partitions: {error}"))
        })?;
    }

    let requested_dates = enumerate_dates(config.from, config.to);
    let dates_requested = requested_dates.len();
    let started_at = Utc::now();
    let range_from = config.from;
    let range_to = config.to;
    let concurrency = config.concurrency;

    // Each date collapses to a summary as soon as its child report exists, so
    // peak memory is `concurrency × identifiers`, never
    // `dates × identifiers` (NFR-001).
    let mut results: Vec<(NaiveDate, Result<DateLoadSummary, MarketDataProviderError>)> =
        stream::iter(requested_dates.into_iter().map(|date| {
            let per_date = config_for_date(&config, date);
            let per_date_context = LoadRunContext::range(
                data_domain,
                date,
                group_id,
                range_from,
                range_to,
                concurrency as i32,
            )
            .with_owner(owner_id);
            let cancel = cancel.clone();
            async move {
                if cancel
                    .as_ref()
                    .is_some_and(|flag| flag.load(Ordering::Relaxed))
                {
                    return None;
                }
                Some((
                    date,
                    run_identifier_load_with_snapshot(pool, snapshot, per_date, &per_date_context)
                        .await
                        .map(|report| DateLoadSummary::from_report(date, report)),
                ))
            }
        }))
        .buffer_unordered(concurrency)
        .filter_map(|item| async move { item })
        .collect()
        .await;

    // `buffer_unordered` yields in completion order; the report must be in
    // date order so two runs are comparable line for line.
    results.sort_by_key(|(date, _)| *date);

    let ended_at = Utc::now();
    let cancelled = cancel
        .as_ref()
        .is_some_and(|flag| flag.load(Ordering::Relaxed));

    let mut summaries: Vec<DateLoadSummary> = Vec::with_capacity(results.len());
    let mut first_error: Option<MarketDataProviderError> = None;
    for (_date, result) in results {
        match result {
            Ok(summary) => summaries.push(summary),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }

    if let Some(error) = first_error {
        // Dates already processed are persisted; retrying is safe. The
        // caller finalizes the group from those persisted dates.
        return Err(error);
    }

    Ok(build_range_report(
        config.from,
        config.to,
        config.concurrency,
        dates_requested,
        summaries,
        cancelled,
        started_at,
        ended_at,
        data_domain,
        group_id,
    ))
}

/// Validates the range request before any date is attempted.
///
/// Implements FR-002 / FR-014 of spec 018.
#[cfg(test)]
fn validate_range_request(
    config: &IdentifierRangeLoadConfig,
    registry: &DataSourceRegistry,
) -> Result<(), MarketDataProviderError> {
    validate_range_request_with_snapshot(config, &registry.snapshot())
}

fn validate_range_request_with_snapshot(
    config: &IdentifierRangeLoadConfig,
    snapshot: &RegistrySnapshot,
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
    validate_preferred_source_with_snapshot(config.preferred_source.as_deref(), snapshot)?;
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
/// Implements FR-014 of spec 018 and FR-013 / FR-017 of spec 034.
#[allow(clippy::too_many_arguments)]
fn build_range_report(
    from: NaiveDate,
    to: NaiveDate,
    concurrency: usize,
    dates_requested: usize,
    summaries: Vec<DateLoadSummary>,
    cancelled: bool,
    started_at: chrono::DateTime<Utc>,
    ended_at: chrono::DateTime<Utc>,
    data_domain: DataDomain,
    range_group_id: Uuid,
) -> IdentifierRangeLoadReport {
    let mut totals = RangeLoadTotals::default();
    let mut dates_with_loads = 0usize;
    let mut dates_with_nothing_stored = 0usize;
    let mut dates_failed = 0usize;

    for summary in &summaries {
        totals.identifiers_attempted += summary.identifiers_attempted;
        totals.loaded += summary.loaded;
        totals.no_op += summary.no_op;
        totals.not_resolved += summary.not_resolved;
        totals.no_data_for_date += summary.no_data_for_date;
        totals.unsupported_scheme += summary.unsupported_scheme;
        totals.rejected += summary.rejected;
        totals.source_unavailable += summary.source_unavailable;
        totals.rate_limited += summary.rate_limited;
        totals.storage_failure += summary.storage_failure;
        totals.assets_created += summary.assets_created;
        totals.assets_adopted += summary.assets_adopted;

        if summary.status == RunStatus::Failed {
            dates_failed += 1;
        }
        if summary.loaded > 0 {
            dates_with_loads += 1;
        }
        if summary.loaded == 0 && summary.no_op == 0 {
            dates_with_nothing_stored += 1;
        }
    }

    let dates_completed = summaries.len();
    let status = derive_range_status(dates_requested, &summaries, cancelled);
    let run_ids = summaries.iter().map(|summary| summary.run_id).collect();

    IdentifierRangeLoadReport {
        report_version: 1,
        data_domain,
        range_group_id,
        from,
        to,
        concurrency,
        status,
        dates_requested,
        dates_completed,
        dates_failed,
        dates_with_loads,
        dates_with_nothing_stored,
        totals,
        run_ids,
        dates: summaries,
        started_at,
        ended_at,
        duration_ms: (ended_at - started_at).num_milliseconds().max(0) as u64,
    }
}

/// Derives the range status from the persisted dates and the stop marker.
///
/// `complete` wins when every requested date persisted with no failed date,
/// even if a late stop arrived (FR-018). Otherwise an accepted stop with
/// missing dates is `cancelled`; no persisted date is `failed`; anything else
/// is `partial`.
///
/// Implements FR-017 of `specs/034-production-hardening/spec.md`.
fn derive_range_status(
    dates_requested: usize,
    summaries: &[DateLoadSummary],
    cancelled: bool,
) -> RunStatus {
    let dates_completed = summaries.len();
    let failed_dates = summaries
        .iter()
        .filter(|summary| summary.status == RunStatus::Failed)
        .count();
    let partial_dates = summaries
        .iter()
        .filter(|summary| summary.status == RunStatus::Partial)
        .count();

    if dates_completed == dates_requested && failed_dates == 0 && partial_dates == 0 {
        return RunStatus::Complete;
    }
    if cancelled && dates_completed < dates_requested {
        return RunStatus::Cancelled;
    }
    if dates_completed == 0 {
        return RunStatus::Failed;
    }
    RunStatus::Partial
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use async_trait::async_trait;
    use rust_decimal::Decimal;

    use crate::market_data::data_domain::DataDomain;
    use crate::market_data::load::DEFAULT_ASSET_CURRENCY;
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
        assert_eq!(report.dates_completed, 5);
        assert_eq!(report.dates.len(), 5);
        let dates: Vec<NaiveDate> = report.dates.iter().map(|summary| summary.date).collect();
        let mut sorted = dates.clone();
        sorted.sort();
        assert_eq!(dates, sorted, "summaries are in date order");
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
    async fn a_dry_run_keeps_the_full_requested_count_and_no_creation_effects() {
        let mut config = config("2026-10-01", "2026-10-02", 2);
        config.create_missing_assets = true;
        let report = run_identifier_range_load(None, &registry(), config)
            .await
            .expect("report");
        for summary in &report.dates {
            assert_eq!(
                summary.assets_created, 0,
                "a dry run never records a creation side effect"
            );
        }
        assert_eq!(report.totals.assets_created, 0);
    }

    #[tokio::test]
    async fn the_group_wrapper_returns_a_dry_run_report() {
        let report = run_identifier_range_load_with_group(
            None,
            &registry(),
            config("2026-10-01", "2026-10-03", 2),
            Uuid::new_v4(),
            DataDomain::MarketData,
        )
        .await
        .expect("report");
        assert_eq!(report.dates.len(), 3);
        assert_eq!(report.dates_requested, 3);
    }

    #[tokio::test]
    async fn cancellation_before_the_first_date_keeps_the_requested_range() {
        let cancel = Arc::new(AtomicBool::new(true));
        let report = run_identifier_range_load_with_group_cancellable(
            None,
            &registry(),
            config("2026-10-01", "2026-10-05", 2),
            Uuid::new_v4(),
            DataDomain::MarketData,
            Some(cancel),
        )
        .await
        .expect("report");
        assert_eq!(report.dates.len(), 0);
        assert_eq!(report.dates_requested, 5);
        assert_eq!(report.dates_completed, 0);
        assert_eq!(report.status, RunStatus::Cancelled);
    }

    #[tokio::test]
    async fn cancellable_runs_without_a_flag_behave_like_the_normal_path() {
        let report = run_identifier_range_load_with_group_cancellable(
            None,
            &registry(),
            config("2026-10-01", "2026-10-02", 1),
            Uuid::new_v4(),
            DataDomain::MarketData,
            None,
        )
        .await
        .expect("report");
        assert_eq!(report.dates.len(), 2);
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

    fn summary(status: RunStatus) -> DateLoadSummary {
        DateLoadSummary {
            date: date("2026-10-01"),
            run_id: None,
            status,
            identifiers_attempted: 0,
            loaded: 0,
            no_op: 0,
            not_resolved: 0,
            no_data_for_date: 0,
            unsupported_scheme: 0,
            rejected: 0,
            source_unavailable: 0,
            rate_limited: 0,
            storage_failure: 0,
            assets_created: 0,
            assets_adopted: 0,
            per_source_attempted: std::collections::BTreeMap::new(),
            per_source_failed: std::collections::BTreeMap::new(),
            started_at: Utc::now(),
            ended_at: Utc::now(),
            duration_ms: 0,
        }
    }

    #[test]
    fn the_range_status_is_derived_from_persisted_dates_and_the_stop_marker() {
        let three_complete = || {
            vec![
                summary(RunStatus::Complete),
                summary(RunStatus::Complete),
                summary(RunStatus::Complete),
            ]
        };
        assert_eq!(
            derive_range_status(3, &three_complete(), false),
            RunStatus::Complete
        );
        assert_eq!(
            derive_range_status(3, &three_complete(), true),
            RunStatus::Complete,
            "a late stop does not relabel a fully persisted run"
        );
        assert_eq!(
            derive_range_status(3, &[summary(RunStatus::Complete)], true),
            RunStatus::Cancelled
        );
        assert_eq!(derive_range_status(3, &[], false), RunStatus::Failed);
        assert_eq!(
            derive_range_status(3, &[summary(RunStatus::Complete)], false),
            RunStatus::Partial
        );
        assert_eq!(
            derive_range_status(
                3,
                &[
                    summary(RunStatus::Complete),
                    summary(RunStatus::Partial),
                    summary(RunStatus::Complete)
                ],
                false
            ),
            RunStatus::Partial
        );
        assert_eq!(
            derive_range_status(
                3,
                &[
                    summary(RunStatus::Failed),
                    summary(RunStatus::Complete),
                    summary(RunStatus::Complete)
                ],
                false
            ),
            RunStatus::Partial
        );
    }
}
