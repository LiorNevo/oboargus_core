//! Persistence helpers for the market data audit tables.
//!
//! Implements FR-007 / FR-022 / FR-032 of `specs/017-yahoo-data-source/spec.md`.
//! The two audit tables are `data_source_refresh_runs` and
//! `data_source_reconciliation_audits` (defined in
//! `obolargus_core/migrations/0005_market_data_sources.sql`).

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::domain::id::DomainId;
use crate::domain::id::{AssetId, DataSourceRefreshRunId, ReconciliationAuditId};
use crate::market_data::error::MarketDataProviderError;

/// One row in `data_source_refresh_runs`.
///
/// Read shape (the row's id is a domain-typed `DataSourceRefreshRunId`).
///
/// Implements FR-007 / FR-032 of spec 017.
#[derive(Debug, Clone, PartialEq, Eq, FromRow, Serialize, Deserialize)]
pub struct RefreshRunRow {
    /// Row id.
    pub id: Uuid,
    /// Run started at.
    pub started_at: DateTime<Utc>,
    /// Run ended at (`None` while in flight).
    pub ended_at: Option<DateTime<Utc>>,
    /// Trigger label: `"scheduler"`, `"api"`, `"cli"`, or `"test"`.
    pub trigger: String,
    /// Total assets the run touched.
    ///
    /// The column is `INTEGER` (INT4) per the data model, so this is
    /// `i32` — not `i64`.
    pub assets_attempted: i32,
    /// Assets for which a new row was written.
    ///
    /// The column is `INTEGER` (INT4) per the data model.
    pub assets_refreshed: i32,
    /// Assets whose bar already matched the existing latest row.
    ///
    /// The column is `INTEGER` (INT4) per the data model.
    pub assets_no_op: i32,
    /// Assets with no supported identifier.
    ///
    /// The column is `INTEGER` (INT4) per the data model.
    pub assets_missing_identifier: i32,
    /// Assets where the source returned an error.
    ///
    /// The column is `INTEGER` (INT4) per the data model.
    pub assets_source_unavailable: i32,
    /// Per-source attempt counts (`{"yahoo_finance": 12, …}`).
    pub per_source_attempted: serde_json::Value,
    /// Per-source failure counts.
    pub per_source_failed: serde_json::Value,
    /// Wall-clock duration, in milliseconds (`None` while in flight).
    pub duration_ms: Option<i64>,
}

impl RefreshRunRow {
    /// Borrows the id as a typed `DataSourceRefreshRunId`.
    ///
    /// Implements FR-007 of spec 017.
    pub fn id(&self) -> DataSourceRefreshRunId {
        DataSourceRefreshRunId::from_uuid(self.id)
    }
}

/// Increment-only counters for a refresh run. Closed form: the four
/// non-overlapping outcome categories partition `assets_attempted`
/// (FR-032 + the SQL `CHECK`).
///
/// Implements FR-032 of spec 017.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataSourceRefreshCounters {
    /// Refresh succeeded and a new row was written.
    pub refreshed: i64,
    /// Source bar matched the existing latest row (idempotent re-run).
    pub no_op: i64,
    /// Asset had no supported identifier for any enabled source.
    pub missing_identifier: i64,
    /// Source returned a typed retryable error.
    pub source_unavailable: i64,
}

impl DataSourceRefreshCounters {
    /// Total assets across all four outcome categories.
    ///
    /// Implements FR-032 of spec 017.
    pub fn total(&self) -> i64 {
        self.refreshed + self.no_op + self.missing_identifier + self.source_unavailable
    }
}

/// One row in `data_source_reconciliation_audits`.
///
/// Implements FR-022 of spec 017.
#[derive(Debug, Clone, PartialEq, Eq, FromRow, Serialize, Deserialize)]
pub struct ReconciliationAuditRow {
    /// Row id.
    pub id: Uuid,
    /// Subject asset.
    pub asset_id: Uuid,
    /// Fact type (e.g. `"market_price"`).
    pub fact_type: String,
    /// Dual-temporal `as_of_date`.
    pub as_of_date: NaiveDate,
    /// `"primary_wins"` or `"first_non_empty"` (v1; `"majority"` deferred).
    pub policy: String,
    /// Ordered list of source ids.
    pub sources_tried: Vec<String>,
    /// Per-source answers or typed errors.
    pub per_source_answers: serde_json::Value,
    /// The chosen canonical value, as its serialized form.
    pub canonical_value: String,
    /// Audit timestamp.
    pub decided_at: DateTime<Utc>,
}

impl ReconciliationAuditRow {
    /// Borrows the id as a typed `ReconciliationAuditId`.
    ///
    /// Implements FR-022 of spec 017.
    pub fn id(&self) -> ReconciliationAuditId {
        ReconciliationAuditId::from_uuid(self.id)
    }

    /// Borrows the asset id as a typed `AssetId`.
    ///
    /// Implements FR-022 of spec 017.
    pub fn asset_id(&self) -> AssetId {
        AssetId::from_uuid(self.asset_id)
    }
}

/// Opens a new refresh-run audit row.
///
/// Returns the new row's id; the caller later calls
/// [`finish_refresh_run`] to set `ended_at` and the counters.
///
/// Implements FR-007 / FR-032 of spec 017.
pub async fn start_refresh_run(
    pool: &sqlx::PgPool,
    trigger: &str,
) -> Result<RefreshRunRow, MarketDataProviderError> {
    let id = Uuid::new_v4();
    let trigger_label = if trigger.is_empty() { "test" } else { trigger };
    sqlx::query(
        "INSERT INTO data_source_refresh_runs (id, started_at, trigger, assets_attempted, assets_refreshed, assets_no_op, assets_missing_identifier, assets_source_unavailable, per_source_attempted, per_source_failed)
         VALUES ($1, NOW(), $2, 0, 0, 0, 0, 0, '{}'::jsonb, '{}'::jsonb)",
    )
    .bind(id)
    .bind(trigger_label)
    .execute(pool)
    .await
    .map_err(|e| MarketDataProviderError::Infrastructure(format!("start_refresh_run: {e}")))?;

    let row: RefreshRunRow = sqlx::query_as(
        "SELECT id, started_at, ended_at, trigger, assets_attempted, assets_refreshed, assets_no_op, assets_missing_identifier, assets_source_unavailable, per_source_attempted, per_source_failed, duration_ms \
         FROM data_source_refresh_runs WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .map_err(|e| MarketDataProviderError::Infrastructure(format!("start_refresh_run read: {e}")))?;
    Ok(row)
}

/// Updates an in-flight refresh-run row with the final counters.
///
/// Implements FR-032 of spec 017.
pub async fn finish_refresh_run(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    counters: &DataSourceRefreshCounters,
    per_source_attempted: &serde_json::Value,
    per_source_failed: &serde_json::Value,
) -> Result<RefreshRunRow, MarketDataProviderError> {
    sqlx::query(
        "UPDATE data_source_refresh_runs
         SET ended_at = NOW(),
             assets_attempted = $2,
             assets_refreshed = $3,
             assets_no_op = $4,
             assets_missing_identifier = $5,
             assets_source_unavailable = $6,
             per_source_attempted = $7,
             per_source_failed = $8,
             duration_ms = (EXTRACT(EPOCH FROM (NOW() - started_at)) * 1000)::bigint
         WHERE id = $1",
    )
    .bind(run_id)
    // The counter columns are `INTEGER` (INT4) per the data model, so the
    // in-memory `i64` counters are narrowed on the way in.
    .bind(counters.total() as i32)
    .bind(counters.refreshed as i32)
    .bind(counters.no_op as i32)
    .bind(counters.missing_identifier as i32)
    .bind(counters.source_unavailable as i32)
    .bind(per_source_attempted)
    .bind(per_source_failed)
    .execute(pool)
    .await
    .map_err(|e| MarketDataProviderError::Infrastructure(format!("finish_refresh_run: {e}")))?;

    let row: RefreshRunRow = sqlx::query_as(
        "SELECT id, started_at, ended_at, trigger, assets_attempted, assets_refreshed, assets_no_op, assets_missing_identifier, assets_source_unavailable, per_source_attempted, per_source_failed, duration_ms \
         FROM data_source_refresh_runs WHERE id = $1",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await
    .map_err(|e| MarketDataProviderError::Infrastructure(format!("finish_refresh_run read: {e}")))?;
    Ok(row)
}

/// One row in `reference_data_load_runs`.
///
/// Implements FR-008 of `specs/018-load-reference-data/spec.md`.
#[derive(Debug, Clone, PartialEq, Eq, FromRow, Serialize, Deserialize)]
pub struct LoadRunRow {
    /// Row id.
    pub id: Uuid,
    /// Run started at.
    pub started_at: DateTime<Utc>,
    /// Run ended at (`None` while in flight).
    pub ended_at: Option<DateTime<Utc>>,
    /// Trigger label: `"cli"`, `"api"`, or `"test"`.
    pub trigger: String,
    /// The caller's named preferred source id (`None` when unnamed).
    ///
    /// Preserved verbatim even when a different source actually answered
    /// (FR-013 of spec 018).
    pub requested_source: Option<String>,
    /// The identifiers as received, before de-duplication.
    pub identifiers_requested_raw: i32,
    /// The repeated entries in the request.
    pub identifiers_duplicated: i32,
    /// The unique identifiers that reached the fetch stage.
    pub identifiers_attempted: i32,
    /// A row was written.
    pub loaded: i32,
    /// The identical fact was already known.
    pub no_op: i32,
    /// No asset claims the identifier.
    pub not_resolved: i32,
    /// The source holds no value for that date.
    pub no_data_for_date: i32,
    /// No enabled source claims the identifier's scheme.
    pub unsupported_scheme: i32,
    /// The source refused the request.
    pub rejected: i32,
    /// A transient or infrastructure failure.
    pub source_unavailable: i32,
    /// The source's budget was exhausted.
    pub rate_limited: i32,
    /// The store refused the write.
    pub storage_failure: i32,
    /// Per-source attempt counts (`{"yahoo_finance": 3, …}`).
    pub per_source_attempted: serde_json::Value,
    /// Per-source failure counts.
    pub per_source_failed: serde_json::Value,
    /// The per-identifier outcome lines.
    pub per_identifier: serde_json::Value,
    /// Wall-clock duration, in milliseconds (`None` while in flight).
    pub duration_ms: Option<i64>,
}

/// Opens a new load-run audit row.
///
/// Returns the new row; the caller later calls [`finish_load_run`] to set
/// `ended_at` and the counters.
///
/// Implements FR-008 of `specs/018-load-reference-data/spec.md`.
pub async fn start_load_run(
    pool: &sqlx::PgPool,
    trigger: &str,
    preferred_source: Option<&str>,
) -> Result<LoadRunRow, MarketDataProviderError> {
    let id = Uuid::new_v4();
    let trigger_label = if trigger.is_empty() { "test" } else { trigger };
    sqlx::query(
        "INSERT INTO reference_data_load_runs (id, started_at, trigger, requested_source, identifiers_requested_raw, identifiers_duplicated, identifiers_attempted, loaded, no_op, not_resolved, no_data_for_date, unsupported_scheme, rejected, source_unavailable, rate_limited, storage_failure, per_source_attempted, per_source_failed, per_identifier) \
         VALUES ($1, NOW(), $2, $3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, '{}'::jsonb, '{}'::jsonb, '[]'::jsonb)",
    )
    .bind(id)
    .bind(trigger_label)
    .bind(preferred_source)
    .execute(pool)
    .await
    .map_err(|e| MarketDataProviderError::Infrastructure(format!("start_load_run: {e}")))?;

    let row: LoadRunRow = sqlx::query_as(
        "SELECT id, started_at, ended_at, trigger, requested_source, identifiers_requested_raw, identifiers_duplicated, identifiers_attempted, loaded, no_op, not_resolved, no_data_for_date, unsupported_scheme, rejected, source_unavailable, rate_limited, storage_failure, per_source_attempted, per_source_failed, per_identifier, duration_ms \
         FROM reference_data_load_runs WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .map_err(|e| MarketDataProviderError::Infrastructure(format!("start_load_run read: {e}")))?;
    Ok(row)
}

/// Updates an in-flight load-run row with the final counters and lines.
///
/// `identifiers_requested_raw` and `identifiers_duplicated` are passed
/// separately: the raw count includes the repeated entries, and the unique
/// count (raw − duplicated) is what the partition `CHECK` constrains.
///
/// Implements FR-008 of `specs/018-load-reference-data/spec.md`.
#[allow(clippy::too_many_arguments)]
pub async fn finish_load_run(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    identifiers_requested_raw: i32,
    identifiers_duplicated: i32,
    counters: &crate::market_data::load::DataSourceLoadCounters,
    per_source_attempted: &serde_json::Value,
    per_source_failed: &serde_json::Value,
    per_identifier: &serde_json::Value,
) -> Result<LoadRunRow, MarketDataProviderError> {
    sqlx::query(
        "UPDATE reference_data_load_runs \
         SET ended_at = NOW(), \
             identifiers_requested_raw = $2, \
             identifiers_duplicated = $3, \
             identifiers_attempted = $4, \
             loaded = $5, \
             no_op = $6, \
             not_resolved = $7, \
             no_data_for_date = $8, \
             unsupported_scheme = $9, \
             rejected = $10, \
             source_unavailable = $11, \
             rate_limited = $12, \
             storage_failure = $13, \
             per_source_attempted = $14, \
             per_source_failed = $15, \
             per_identifier = $16, \
             duration_ms = (EXTRACT(EPOCH FROM (NOW() - started_at)) * 1000)::bigint \
         WHERE id = $1",
    )
    .bind(run_id)
    .bind(identifiers_requested_raw)
    .bind(identifiers_duplicated)
    .bind(counters.total_attempted() as i32)
    .bind(counters.loaded as i32)
    .bind(counters.no_op as i32)
    .bind(counters.not_resolved as i32)
    .bind(counters.no_data_for_date as i32)
    .bind(counters.unsupported_scheme as i32)
    .bind(counters.rejected as i32)
    .bind(counters.source_unavailable as i32)
    .bind(counters.rate_limited as i32)
    .bind(counters.storage_failure as i32)
    .bind(per_source_attempted)
    .bind(per_source_failed)
    .bind(per_identifier)
    .execute(pool)
    .await
    .map_err(|e| MarketDataProviderError::Infrastructure(format!("finish_load_run: {e}")))?;

    let row: LoadRunRow = sqlx::query_as(
        "SELECT id, started_at, ended_at, trigger, requested_source, identifiers_requested_raw, identifiers_duplicated, identifiers_attempted, loaded, no_op, not_resolved, no_data_for_date, unsupported_scheme, rejected, source_unavailable, rate_limited, storage_failure, per_source_attempted, per_source_failed, per_identifier, duration_ms \
         FROM reference_data_load_runs WHERE id = $1",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await
    .map_err(|e| MarketDataProviderError::Infrastructure(format!("finish_load_run read: {e}")))?;
    Ok(row)
}

/// Reads a load-run audit row by id.
///
/// Implements FR-008 of `specs/018-load-reference-data/spec.md`.
pub async fn get_load_run(
    pool: &sqlx::PgPool,
    run_id: Uuid,
) -> Result<LoadRunRow, MarketDataProviderError> {
    let row: Option<LoadRunRow> = sqlx::query_as(
        "SELECT id, started_at, ended_at, trigger, requested_source, identifiers_requested_raw, identifiers_duplicated, identifiers_attempted, loaded, no_op, not_resolved, no_data_for_date, unsupported_scheme, rejected, source_unavailable, rate_limited, storage_failure, per_source_attempted, per_source_failed, per_identifier, duration_ms \
         FROM reference_data_load_runs WHERE id = $1",
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| MarketDataProviderError::Infrastructure(format!("get_load_run: {e}")))?;

    match row {
        Some(row) => Ok(row),
        None => Err(MarketDataProviderError::NotFound(format!(
            "load run '{run_id}' does not exist"
        ))),
    }
}

/// The payload of a reconciliation audit row.
///
/// Bundles the audited fields so the write helper keeps a small argument
/// list. Implements FR-022 of spec 017.
#[derive(Debug, Clone)]
pub struct ReconciliationAuditPayload {
    /// Subject asset.
    pub asset_id: AssetId,
    /// Fact type (e.g. `"market_price"`).
    pub fact_type: String,
    /// Dual-temporal `as_of_date`.
    pub as_of_date: NaiveDate,
    /// Policy label (`"primary_wins"`, `"first_non_empty"`).
    pub policy: String,
    /// Ordered list of source ids that were consulted.
    pub sources_tried: Vec<String>,
    /// Per-source answers or typed errors.
    pub per_source_answers: serde_json::Value,
    /// The chosen canonical value, as its serialized form.
    pub canonical_value: String,
}

/// Persists a reconciliation audit row.
///
/// Implements FR-022 of spec 017.
pub async fn record_reconciliation_audit(
    pool: &sqlx::PgPool,
    payload: ReconciliationAuditPayload,
) -> Result<ReconciliationAuditRow, MarketDataProviderError> {
    if payload.sources_tried.len() < 2 {
        return Err(MarketDataProviderError::Validation(
            "reconciliation requires at least 2 sources in sources_tried".into(),
        ));
    }
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO data_source_reconciliation_audits \
         (id, asset_id, fact_type, as_of_date, policy, sources_tried, per_source_answers, canonical_value, decided_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW())",
    )
    .bind(id)
    .bind(payload.asset_id.as_uuid())
    .bind(&payload.fact_type)
    .bind(payload.as_of_date)
    .bind(&payload.policy)
    .bind(&payload.sources_tried)
    .bind(&payload.per_source_answers)
    .bind(&payload.canonical_value)
    .execute(pool)
    .await
    .map_err(|e| MarketDataProviderError::Infrastructure(format!("record_reconciliation_audit: {e}")))?;

    let row: ReconciliationAuditRow = sqlx::query_as(
        "SELECT id, asset_id, fact_type, as_of_date, policy, sources_tried, per_source_answers, canonical_value, decided_at \
         FROM data_source_reconciliation_audits WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .map_err(|e| MarketDataProviderError::Infrastructure(format!("record_reconciliation_audit read: {e}")))?;
    Ok(row)
}
