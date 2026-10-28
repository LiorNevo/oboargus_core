//! Read model for the Data Load page's history.
//!
//! Reads the `data_load_history` view, reconstructs a completed run's report
//! from its audit row, and reports range progress. History is read-only.
//!
//! Implements FR-028..FR-031 of `specs/022-market-data-load-ui/spec.md`.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::market_data::audit::LoadRunRow;
use crate::market_data::data_domain::DataDomain;
use crate::market_data::error::MarketDataProviderError;
use crate::market_data::load::{IdentifierLoadReport, LoadIdentifierOutcome, RunStatus};
use crate::market_data::load_range::RangeLoadTotals;

/// Filters accepted by the history list.
#[derive(Debug, Clone, Default)]
/// Implements FR-026 / FR-028..FR-031 of spec 022.
pub struct HistoryFilter {
    /// Inclusive lower bound on `started_at`.
    pub from: Option<NaiveDate>,
    /// Inclusive upper bound on `started_at` (whole UTC day).
    pub to: Option<NaiveDate>,
    /// Data domain.
    pub domain: Option<String>,
    /// Requested source, answering source, or source-change source.
    pub source: Option<String>,
    /// Derived status (`complete`, `partial`, `failed`, `running`,
    /// `incomplete`, `source_enabled`, `source_disabled`).
    pub status: Option<String>,
    /// `load_run`, `range_run`, `source_change`, or `run`.
    pub entry_type: Option<String>,
    /// Page size (`1..=200`).
    pub limit: i64,
    /// Offset.
    pub offset: i64,
}

/// One history entry shown in the list.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// Implements FR-026 / FR-028..FR-031 of spec 022.
pub struct LoadHistoryEntry {
    /// Run id, range group id, or source-change id.
    pub entry_id: Uuid,
    /// `load_run`, `range_run`, or `source_change`.
    pub entry_type: String,
    /// Entry start.
    pub started_at: DateTime<Utc>,
    /// Entry end (`None` while running).
    pub ended_at: Option<DateTime<Utc>>,
    /// Data domain (null for source changes).
    pub data_domain: Option<String>,
    /// Requested source (null for source changes or Auto).
    pub requested_source: Option<String>,
    /// Distinct answering sources.
    pub answering_sources: Vec<String>,
    /// Identifiers as received.
    pub identifiers_requested_raw: i32,
    /// Duplicate entries.
    pub identifiers_duplicated: i32,
    /// Identifiers attempted.
    pub identifiers_attempted: i32,
    /// Loaded count.
    pub loaded: i32,
    /// No-op count.
    pub no_op: i32,
    /// Not-resolved count.
    pub not_resolved: i32,
    /// No-data count.
    pub no_data_for_date: i32,
    /// Unsupported-scheme count.
    pub unsupported_scheme: i32,
    /// Rejected count.
    pub rejected: i32,
    /// Source-unavailable count.
    pub source_unavailable: i32,
    /// Rate-limited count.
    pub rate_limited: i32,
    /// Storage-failure count.
    pub storage_failure: i32,
    /// Assets created.
    pub assets_created: i32,
    /// Assets adopted.
    pub assets_adopted: i32,
    /// Derived status.
    pub status: String,
    /// Running longer than the staleness interval.
    pub stale: bool,
    /// Duration in milliseconds.
    pub duration_ms: Option<i64>,
    /// First date covered.
    pub date_from: Option<NaiveDate>,
    /// Last date covered.
    pub date_to: Option<NaiveDate>,
    /// Dates requested.
    pub dates_requested: i32,
    /// Dates completed.
    pub dates_completed: i32,
    /// Dates with at least one stored row.
    pub dates_with_loads: i32,
    /// Window used by a range.
    pub range_concurrency: Option<i32>,
    /// Source-change source id.
    pub source_id: Option<String>,
    /// Source-change previous state.
    pub before_enabled: Option<bool>,
    /// Source-change new state.
    pub after_enabled: Option<bool>,
    /// Actor, when available.
    pub actor: Option<String>,
}

/// One page of history entries.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// Implements FR-026 / FR-028..FR-031 of spec 022.
pub struct LoadHistoryPage {
    /// Matching entries.
    pub entries: Vec<LoadHistoryEntry>,
    /// Total matching entries, independent of paging.
    pub total: i64,
    /// Page size used.
    pub limit: i64,
    /// Offset used.
    pub offset: i64,
}

/// A completed single-date detail.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// Implements FR-026 / FR-028..FR-031 of spec 022.
pub struct LoadDetail {
    /// The reconstructed execution report.
    pub report: IdentifierLoadReport,
}

/// A source-change detail.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// Implements FR-026 / FR-028..FR-031 of spec 022.
pub struct SourceChangeDetail {
    /// Source id.
    pub source_id: String,
    /// State before.
    pub before_enabled: bool,
    /// State after.
    pub after_enabled: bool,
    /// When it changed.
    pub changed_at: DateTime<Utc>,
    /// Actor, when available.
    pub actor: Option<String>,
    /// Optional operator note.
    pub reason: Option<String>,
}

/// One date in a range detail.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// Implements FR-026 / FR-028..FR-031 of spec 022.
pub struct DateRunDetail {
    /// The date.
    pub date: NaiveDate,
    /// That date's report.
    pub report: IdentifierLoadReport,
}

/// A range detail.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// Implements FR-026 / FR-028..FR-031 of spec 022.
pub struct RangeRunDetail {
    /// First date.
    pub from: NaiveDate,
    /// Last date.
    pub to: NaiveDate,
    /// Window used.
    pub concurrency: usize,
    /// Worst status.
    pub status: RunStatus,
    /// Dates requested.
    pub dates_requested: usize,
    /// Dates completed.
    pub dates_completed: usize,
    /// Dates with loads.
    pub dates_with_loads: usize,
    /// Dates with nothing stored.
    pub dates_with_nothing_stored: usize,
    /// Summed counters.
    pub totals: RangeLoadTotals,
    /// Ascending date sections.
    pub dates: Vec<DateRunDetail>,
}

/// A range progress snapshot.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// Implements FR-026 / FR-028..FR-031 of spec 022.
pub struct RangeProgress {
    /// Group id.
    pub group_id: Uuid,
    /// `starting`, `running`, `complete`, `partial`, or `failed`.
    pub status: String,
    /// Dates requested (`None` before the first child exists).
    pub dates_requested: Option<usize>,
    /// Dates completed.
    pub dates_completed: usize,
    /// Completed dates whose status is failed.
    pub dates_failed: usize,
    /// Dates with loads.
    pub dates_with_loads: usize,
    /// Earliest child start.
    pub started_at: Option<DateTime<Utc>>,
    /// Latest child transition (or now while starting).
    pub updated_at: DateTime<Utc>,
}

/// Full detail for one history entry.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// Implements FR-026 / FR-028..FR-031 of spec 022.
pub struct LoadHistoryDetail {
    /// Summary entry.
    pub entry: LoadHistoryEntry,
    /// Single-date detail.
    pub load: Option<LoadDetail>,
    /// Range detail.
    pub range: Option<RangeRunDetail>,
    /// Source-change detail.
    pub source_change: Option<SourceChangeDetail>,
}

#[derive(Debug, Clone, FromRow)]
struct HistoryRow {
    entry_id: Uuid,
    entry_type: String,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    data_domain: Option<String>,
    requested_source: Option<String>,
    answering_sources: Vec<String>,
    identifiers_requested_raw: i32,
    identifiers_duplicated: i32,
    identifiers_attempted: i32,
    loaded: i32,
    no_op: i32,
    not_resolved: i32,
    no_data_for_date: i32,
    unsupported_scheme: i32,
    rejected: i32,
    source_unavailable: i32,
    rate_limited: i32,
    storage_failure: i32,
    assets_created: i32,
    assets_adopted: i32,
    status: String,
    duration_ms: Option<i64>,
    date_from: Option<NaiveDate>,
    date_to: Option<NaiveDate>,
    dates_requested: i32,
    dates_completed: i32,
    dates_with_loads: i32,
    range_concurrency: Option<i32>,
    source_id: Option<String>,
    before_enabled: Option<bool>,
    after_enabled: Option<bool>,
    actor: Option<String>,
}

type ChangeRow = (
    String,
    bool,
    bool,
    DateTime<Utc>,
    Option<String>,
    Option<String>,
);

type ProgressAggregate = (
    i64,
    i64,
    i64,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    Option<NaiveDate>,
    Option<NaiveDate>,
);

type ProgressChild = (bool, Option<DateTime<Utc>>, i32, i32, i32, i32, i32);

const HISTORY_SELECT: &str = "SELECT entry_id, entry_type, started_at, ended_at, \
    data_domain, requested_source, answering_sources, identifiers_requested_raw, \
    identifiers_duplicated, identifiers_attempted, loaded, no_op, not_resolved, \
    no_data_for_date, unsupported_scheme, rejected, source_unavailable, \
    rate_limited, storage_failure, assets_created, assets_adopted, status, \
    duration_ms, date_from, date_to, dates_requested, dates_completed, \
    dates_with_loads, range_concurrency, source_id, \
    before_enabled, after_enabled, actor FROM data_load_history";

const HISTORY_FILTER: &str = " WHERE ($1::date IS NULL OR started_at >= $1::date) \
    AND ($2::date IS NULL OR started_at < ($2::date + 1)) \
    AND ($3::text IS NULL OR data_domain = $3) \
    AND ($4::text IS NULL OR requested_source = $4 OR $4 = ANY(answering_sources) \
         OR (entry_type = 'source_change' AND source_id = $4)) \
    AND ($5::text IS NULL \
         OR ($5 = 'incomplete' AND status = 'running' AND started_at < NOW() - ($7::double precision * INTERVAL '1 second')) \
         OR ($5 = 'running' AND status = 'running' AND started_at >= NOW() - ($7::double precision * INTERVAL '1 second')) \
         OR ($5 NOT IN ('incomplete', 'running') AND status = $5)) \
    AND ($6::text IS NULL OR entry_type = $6 \
         OR ($6 = 'run' AND entry_type IN ('load_run', 'range_run')))";

/// Lists history entries, newest first.
/// Implements FR-026 / FR-028..FR-031 of spec 022.
pub async fn list_history(
    pool: &sqlx::PgPool,
    filter: &HistoryFilter,
    stale_after_seconds: i64,
) -> Result<LoadHistoryPage, MarketDataProviderError> {
    let limit = filter.limit.clamp(1, 200);
    let offset = filter.offset.max(0);
    let sql = format!(
        "{HISTORY_SELECT}{HISTORY_FILTER} ORDER BY started_at DESC, entry_id DESC LIMIT $8 OFFSET $9"
    );
    let rows: Vec<HistoryRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(filter.from)
        .bind(filter.to)
        .bind(&filter.domain)
        .bind(&filter.source)
        .bind(&filter.status)
        .bind(&filter.entry_type)
        .bind(stale_after_seconds)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
        .map_err(|e| MarketDataProviderError::Infrastructure(format!("list_history: {e}")))?;

    let count_sql = format!("SELECT COUNT(*) FROM data_load_history{HISTORY_FILTER}");
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(count_sql))
        .bind(filter.from)
        .bind(filter.to)
        .bind(&filter.domain)
        .bind(&filter.source)
        .bind(&filter.status)
        .bind(&filter.entry_type)
        .bind(stale_after_seconds)
        .fetch_one(pool)
        .await
        .map_err(|e| MarketDataProviderError::Infrastructure(format!("count_history: {e}")))?;

    let now = Utc::now();
    let entries = rows
        .into_iter()
        .map(|row| row.into_entry(now, stale_after_seconds))
        .collect();
    Ok(LoadHistoryPage {
        entries,
        total,
        limit,
        offset,
    })
}

impl HistoryRow {
    fn into_entry(self, now: DateTime<Utc>, stale_after_seconds: i64) -> LoadHistoryEntry {
        let stale = self.ended_at.is_none()
            && self.started_at < now - chrono::Duration::seconds(stale_after_seconds);
        LoadHistoryEntry {
            entry_id: self.entry_id,
            entry_type: self.entry_type,
            started_at: self.started_at,
            ended_at: self.ended_at,
            data_domain: self.data_domain,
            requested_source: self.requested_source,
            answering_sources: self.answering_sources,
            identifiers_requested_raw: self.identifiers_requested_raw,
            identifiers_duplicated: self.identifiers_duplicated,
            identifiers_attempted: self.identifiers_attempted,
            loaded: self.loaded,
            no_op: self.no_op,
            not_resolved: self.not_resolved,
            no_data_for_date: self.no_data_for_date,
            unsupported_scheme: self.unsupported_scheme,
            rejected: self.rejected,
            source_unavailable: self.source_unavailable,
            rate_limited: self.rate_limited,
            storage_failure: self.storage_failure,
            assets_created: self.assets_created,
            assets_adopted: self.assets_adopted,
            status: self.status,
            stale,
            duration_ms: self.duration_ms,
            date_from: self.date_from,
            date_to: self.date_to,
            dates_requested: self.dates_requested,
            dates_completed: self.dates_completed,
            dates_with_loads: self.dates_with_loads,
            range_concurrency: self.range_concurrency,
            source_id: self.source_id,
            before_enabled: self.before_enabled,
            after_enabled: self.after_enabled,
            actor: self.actor,
        }
    }
}

/// Reads the summary entry for one id.
/// Implements FR-026 / FR-028..FR-031 of spec 022.
pub async fn get_history_entry(
    pool: &sqlx::PgPool,
    entry_id: Uuid,
    stale_after_seconds: i64,
) -> Result<LoadHistoryEntry, MarketDataProviderError> {
    let sql = format!("{HISTORY_SELECT} WHERE entry_id = $1");
    let row: Option<HistoryRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(entry_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| MarketDataProviderError::Infrastructure(format!("get_history_entry: {e}")))?;
    row.map(|row| row.into_entry(Utc::now(), stale_after_seconds))
        .ok_or_else(|| MarketDataProviderError::NotFound(format!("history entry '{entry_id}'")))
}

/// Reads full detail for one entry.
/// Implements FR-026 / FR-028..FR-031 of spec 022.
pub async fn get_history_detail(
    pool: &sqlx::PgPool,
    entry_id: Uuid,
    stale_after_seconds: i64,
) -> Result<LoadHistoryDetail, MarketDataProviderError> {
    let entry = get_history_entry(pool, entry_id, stale_after_seconds).await?;
    match entry.entry_type.as_str() {
        "load_run" => {
            if entry.ended_at.is_none() {
                return Ok(LoadHistoryDetail {
                    entry,
                    load: None,
                    range: None,
                    source_change: None,
                });
            }
            let row = get_run_row(pool, entry_id).await?;
            let report = report_from_row(&row, &entry.status)?;
            Ok(LoadHistoryDetail {
                entry,
                load: Some(LoadDetail { report }),
                range: None,
                source_change: None,
            })
        }
        "range_run" => {
            let rows = get_group_rows(pool, entry_id).await?;
            let range = range_from_rows(&entry, rows)?;
            Ok(LoadHistoryDetail {
                entry,
                load: None,
                range: Some(range),
                source_change: None,
            })
        }
        "source_change" => {
            let change = get_change(pool, entry_id).await?;
            Ok(LoadHistoryDetail {
                entry,
                load: None,
                range: None,
                source_change: Some(change),
            })
        }
        other => Err(MarketDataProviderError::NotFound(format!(
            "unsupported history entry type '{other}'"
        ))),
    }
}

async fn get_run_row(
    pool: &sqlx::PgPool,
    run_id: Uuid,
) -> Result<LoadRunRow, MarketDataProviderError> {
    crate::market_data::audit::get_load_run(pool, run_id).await
}

async fn get_group_rows(
    pool: &sqlx::PgPool,
    group_id: Uuid,
) -> Result<Vec<LoadRunRow>, MarketDataProviderError> {
    let rows: Vec<LoadRunRow> = sqlx::query_as(
        "SELECT id, started_at, ended_at, trigger, requested_source, \
         identifiers_requested_raw, identifiers_duplicated, identifiers_attempted, \
         loaded, no_op, not_resolved, no_data_for_date, unsupported_scheme, \
         rejected, source_unavailable, rate_limited, storage_failure, \
         assets_created, assets_adopted, per_source_attempted, per_source_failed, \
         per_identifier, duration_ms, data_domain, as_of_date, range_group_id, \
         range_from, range_to, range_concurrency, answering_sources \
         FROM reference_data_load_runs WHERE range_group_id = $1 \
         ORDER BY as_of_date ASC, started_at ASC",
    )
    .bind(group_id)
    .fetch_all(pool)
    .await
    .map_err(|e| MarketDataProviderError::Infrastructure(format!("get_group_rows: {e}")))?;
    Ok(rows)
}

async fn get_change(
    pool: &sqlx::PgPool,
    change_id: Uuid,
) -> Result<SourceChangeDetail, MarketDataProviderError> {
    let row: Option<ChangeRow> = sqlx::query_as(
        "SELECT source_id, before_enabled, after_enabled, changed_at, actor, reason \
         FROM data_source_state_changes WHERE id = $1",
    )
    .bind(change_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| MarketDataProviderError::Infrastructure(format!("get_change: {e}")))?;
    let (source_id, before_enabled, after_enabled, changed_at, actor, reason) = row
        .ok_or_else(|| MarketDataProviderError::NotFound(format!("source change '{change_id}'")))?;
    Ok(SourceChangeDetail {
        source_id,
        before_enabled,
        after_enabled,
        changed_at,
        actor,
        reason,
    })
}

fn report_from_row(
    row: &LoadRunRow,
    status: &str,
) -> Result<IdentifierLoadReport, MarketDataProviderError> {
    let identifiers: Vec<LoadIdentifierOutcome> =
        serde_json::from_value(row.per_identifier.clone()).map_err(|e| {
            MarketDataProviderError::Infrastructure(format!("decode per_identifier: {e}"))
        })?;
    let per_source_attempted: BTreeMap<String, usize> =
        serde_json::from_value(row.per_source_attempted.clone()).unwrap_or_default();
    let per_source_failed: BTreeMap<String, usize> =
        serde_json::from_value(row.per_source_failed.clone()).unwrap_or_default();
    let data_domain = row
        .data_domain
        .parse::<DataDomain>()
        .map_err(MarketDataProviderError::Validation)?;
    Ok(IdentifierLoadReport {
        report_version: 1,
        run_id: Some(row.id),
        data_domain,
        as_of_date: row.as_of_date,
        requested_source: row.requested_source.clone(),
        status: status_from_str(status),
        identifiers_requested_raw: row.identifiers_requested_raw.max(0) as usize,
        identifiers_duplicated: row.identifiers_duplicated.max(0) as usize,
        identifiers_attempted: row.identifiers_attempted.max(0) as usize,
        loaded: row.loaded.max(0) as usize,
        no_op: row.no_op.max(0) as usize,
        not_resolved: row.not_resolved.max(0) as usize,
        no_data_for_date: row.no_data_for_date.max(0) as usize,
        unsupported_scheme: row.unsupported_scheme.max(0) as usize,
        rejected: row.rejected.max(0) as usize,
        source_unavailable: row.source_unavailable.max(0) as usize,
        rate_limited: row.rate_limited.max(0) as usize,
        storage_failure: row.storage_failure.max(0) as usize,
        assets_created: row.assets_created.max(0) as usize,
        assets_adopted: row.assets_adopted.max(0) as usize,
        per_source_attempted,
        per_source_failed,
        identifiers,
        started_at: row.started_at,
        ended_at: row.ended_at.unwrap_or_else(Utc::now),
        duration_ms: row.duration_ms.unwrap_or(0).max(0) as u64,
    })
}

fn range_from_rows(
    entry: &LoadHistoryEntry,
    rows: Vec<LoadRunRow>,
) -> Result<RangeRunDetail, MarketDataProviderError> {
    let mut dates = Vec::with_capacity(rows.len());
    let mut totals = RangeLoadTotals::default();
    let mut dates_with_loads = 0usize;
    let mut dates_with_nothing_stored = 0usize;
    for row in rows.into_iter().filter(|row| row.ended_at.is_some()) {
        let status = child_status(&row);
        let report = report_from_row(&row, status)?;
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
        let date = row
            .as_of_date
            .or(report.as_of_date)
            .unwrap_or(row.started_at.date_naive());
        dates.push(DateRunDetail { date, report });
    }
    let from = entry
        .date_from
        .or_else(|| dates.first().map(|d| d.date))
        .ok_or_else(|| MarketDataProviderError::NotFound("range has no dates".into()))?;
    let to = entry
        .date_to
        .or_else(|| dates.last().map(|d| d.date))
        .unwrap_or(from);
    Ok(RangeRunDetail {
        from,
        to,
        concurrency: entry.range_concurrency.unwrap_or(1).max(1) as usize,
        status: status_from_str(&entry.status),
        dates_requested: entry.dates_requested.max(0) as usize,
        dates_completed: entry.dates_completed.max(0) as usize,
        dates_with_loads,
        dates_with_nothing_stored,
        totals,
        dates,
    })
}

fn child_status(row: &LoadRunRow) -> &'static str {
    let failed = row.rejected + row.source_unavailable + row.rate_limited + row.storage_failure;
    if row.ended_at.is_none() {
        "running"
    } else if failed == 0 {
        "complete"
    } else if failed == row.identifiers_attempted {
        "failed"
    } else {
        "partial"
    }
}

fn status_from_str(status: &str) -> RunStatus {
    match status {
        "failed" => RunStatus::Failed,
        "partial" => RunStatus::Partial,
        _ => RunStatus::Complete,
    }
}

/// Reads progress for a range group.
/// Implements FR-026 / FR-028..FR-031 of spec 022.
pub async fn range_progress(
    pool: &sqlx::PgPool,
    group_id: Uuid,
) -> Result<RangeProgress, MarketDataProviderError> {
    let row: Option<ProgressAggregate> = sqlx::query_as(
        "SELECT COUNT(*), \
                    COUNT(*) FILTER (WHERE ended_at IS NOT NULL), \
                    COUNT(*) FILTER (WHERE loaded > 0), \
                    MIN(started_at), MAX(COALESCE(ended_at, started_at)), \
                    MIN(range_from), MAX(range_to) \
             FROM reference_data_load_runs WHERE range_group_id = $1",
    )
    .bind(group_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| MarketDataProviderError::Infrastructure(format!("range_progress: {e}")))?;

    let Some((total, completed, with_loads, started_at, updated_at, from, to)) = row else {
        return Ok(starting_progress(group_id));
    };
    if total == 0 {
        return Ok(starting_progress(group_id));
    }

    let children: Vec<ProgressChild> = sqlx::query_as(
        "SELECT (ended_at IS NOT NULL), ended_at, rejected, source_unavailable, \
                rate_limited, storage_failure, identifiers_attempted \
         FROM reference_data_load_runs WHERE range_group_id = $1",
    )
    .bind(group_id)
    .fetch_all(pool)
    .await
    .map_err(|e| MarketDataProviderError::Infrastructure(format!("range_progress: {e}")))?;

    let mut dates_failed = 0usize;
    let mut running = false;
    let mut failed = false;
    let mut partial = false;
    for (ended, _, rejected, unavailable, limited, storage, attempted) in children {
        if !ended {
            running = true;
            continue;
        }
        let failures = rejected + unavailable + limited + storage;
        if failures > 0 && failures == attempted {
            failed = true;
            dates_failed += 1;
        } else if failures > 0 {
            partial = true;
        }
    }
    let status = if running {
        "running"
    } else if failed {
        "failed"
    } else if partial {
        "partial"
    } else {
        "complete"
    };
    let dates_requested = match (from, to) {
        (Some(from), Some(to)) => Some((to - from).num_days().max(0) as usize + 1),
        _ => Some(total as usize),
    };
    Ok(RangeProgress {
        group_id,
        status: status.to_string(),
        dates_requested,
        dates_completed: completed as usize,
        dates_failed,
        dates_with_loads: with_loads as usize,
        started_at,
        updated_at: updated_at.unwrap_or_else(Utc::now),
    })
}

fn starting_progress(group_id: Uuid) -> RangeProgress {
    RangeProgress {
        group_id,
        status: "starting".to_string(),
        dates_requested: None,
        dates_completed: 0,
        dates_failed: 0,
        dates_with_loads: 0,
        started_at: None,
        updated_at: Utc::now(),
    }
}
