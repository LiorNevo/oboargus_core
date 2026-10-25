//! Batch reference-data loading by identifier and date.
//!
//! Implements FR-001..FR-009, FR-011..FR-013 of
//! `specs/018-load-reference-data/spec.md`. The module owns the shared
//! "load reference data" operation: given a list of typed identifiers and a
//! single as-of date, it resolves every identifier to its asset, fetches that
//! asset's end-of-day value for the date through the source-agnostic
//! registry (fail-over included), stores it through the canonical
//! dual-temporal write API, and returns a per-identifier execution report.
//!
//! The CLI (FR-011) and the API (FR-012) are thin witnesses of this one
//! operation — neither implements its own fetch/store loop.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::asset_identifier::read as asset_identifier_read;
use crate::domain::asset_reference_data::create as asset_reference_data_create;
use crate::domain::asset_reference_data::entity::NewAssetReferenceData;
use crate::domain::asset_reference_data::read as asset_reference_data_read;
use crate::domain::id::AssetId;
use crate::market_data::audit::{finish_load_run, start_load_run};
use crate::market_data::error::MarketDataProviderError;
use crate::market_data::registry::DataSourceRegistry;
use crate::market_data::types::{HistoricalRangeRequest, PriceBar, TypedIdentifier};

/// Fact type written by the load operation for end-of-day OHLCV bars.
///
/// Implements FR-004 of spec 018 (the same fact type the predecessor's
/// refresh writes).
pub const FACT_TYPE_MARKET_PRICE: &str = "market_price";

/// The classification of one requested identifier.
///
/// The vocabulary is closed: every requested identifier ends in exactly one
/// of these nine categories, and no category is used for a condition it does
/// not describe.
///
/// Implements FR-005 of spec 018.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadOutcome {
    /// A reference-data row was written.
    Loaded,
    /// Nothing was written because the identical fact is already known
    /// (same asset, fact, as-of date and knowledge date).
    NoOp,
    /// No asset claims the identifier; no source was contacted.
    NotResolved,
    /// The source answered authoritatively that it holds no value for that
    /// date — a non-trading day, or an upstream that does not recognize the
    /// identifier's symbol.
    NoDataForDate,
    /// No enabled source claims the identifier's scheme.
    UnsupportedScheme,
    /// The source refused or could not answer this identifier for a
    /// non-transient reason — invalid request, unsupported operation,
    /// credentials refused, or the source's own validation failure.
    Rejected,
    /// A transient or infrastructure failure.
    SourceUnavailable,
    /// The source's budget was exhausted.
    RateLimited,
    /// The reference-data store refused the write for a reason other than
    /// the idempotency collision.
    StorageFailure,
}

/// One source's answer (or failure) during a registry walk.
///
/// Implements FR-006 of spec 018.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceOutcome {
    /// The stable id of the source that was consulted.
    pub source_id: String,
    /// Whether the source answered successfully.
    pub ok: bool,
    /// The typed error's display form, present only when `ok` is `false`.
    ///
    /// Never carries credential material (FR-005 / FR-008).
    pub error: Option<String>,
}

/// The per-identifier result line of an execution report.
///
/// Implements FR-006 of spec 018.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadIdentifierOutcome {
    /// The identifier as received (normalized scheme/value pair).
    pub identifier: TypedIdentifier,
    /// The claimed asset, when resolution succeeded.
    pub asset_id: Option<Uuid>,
    /// Whether the load consulted the data-source registry for this line.
    ///
    /// `true` for every outcome except `not_resolved`; an
    /// `unsupported_scheme` line consulted the registry but contacted no
    /// source.
    pub attempted: bool,
    /// The chosen classification (the fixed vocabulary of FR-005).
    pub outcome: LoadOutcome,
    /// The source that **actually answered**, when one answered.
    ///
    /// This is the value stored on the reference-data row (FR-004); the
    /// caller's named preference lives on the report's `requested_source`.
    pub source_id: Option<String>,
    /// Per-source try/typed-error list (the manifest of FR-006).
    pub per_source: Vec<SourceOutcome>,
}

/// The run's overall status.
///
/// Absence outcomes — `not_resolved`, `no_data_for_date`,
/// `unsupported_scheme` — are not failures and never degrade the status.
///
/// Implements FR-006 of spec 018.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// No identifier ended in a failure bucket.
    Complete,
    /// Some, but not all, attempted identifiers ended in a failure bucket.
    Partial,
    /// Every attempted identifier ended in a failure bucket.
    Failed,
}

/// Per-run counters, one field per outcome bucket.
///
/// The fetch-attempted buckets partition `identifiers_attempted` exactly as
/// the audit table's `CHECK` constraint does; `not_resolved` is kept out of
/// that partition (it was never attempted).
///
/// Implements FR-005 / FR-006 / FR-008 of spec 018.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataSourceLoadCounters {
    /// A row was written.
    pub loaded: i64,
    /// The identical fact was already known.
    pub no_op: i64,
    /// No asset claims the identifier.
    pub not_resolved: i64,
    /// The source holds no value for that date.
    pub no_data_for_date: i64,
    /// No enabled source claims the identifier's scheme.
    pub unsupported_scheme: i64,
    /// The source refused the request.
    pub rejected: i64,
    /// A transient or infrastructure failure.
    pub source_unavailable: i64,
    /// The source's budget was exhausted.
    pub rate_limited: i64,
    /// The store refused the write.
    pub storage_failure: i64,
}

impl DataSourceLoadCounters {
    /// The number of identifiers that reached the fetch stage.
    ///
    /// Implements FR-005 of spec 018 (research D13: "attempted" means the
    /// data-source registry was consulted).
    pub fn total_attempted(&self) -> i64 {
        self.loaded
            + self.no_op
            + self.no_data_for_date
            + self.unsupported_scheme
            + self.rejected
            + self.source_unavailable
            + self.rate_limited
            + self.storage_failure
    }

    /// The number of unique identifiers the request carried.
    ///
    /// Implements FR-005 of spec 018.
    pub fn total_requested(&self) -> i64 {
        self.total_attempted() + self.not_resolved
    }

    /// The number of identifiers that ended in a failure bucket.
    ///
    /// Implements FR-006 of spec 018.
    pub fn total_failed(&self) -> i64 {
        self.rejected + self.source_unavailable + self.rate_limited + self.storage_failure
    }

    /// Adds one line to the bucket its outcome belongs to.
    ///
    /// Implements FR-005 of spec 018.
    pub fn increment(&mut self, outcome: LoadOutcome) {
        match outcome {
            LoadOutcome::Loaded => self.loaded += 1,
            LoadOutcome::NoOp => self.no_op += 1,
            LoadOutcome::NotResolved => self.not_resolved += 1,
            LoadOutcome::NoDataForDate => self.no_data_for_date += 1,
            LoadOutcome::UnsupportedScheme => self.unsupported_scheme += 1,
            LoadOutcome::Rejected => self.rejected += 1,
            LoadOutcome::SourceUnavailable => self.source_unavailable += 1,
            LoadOutcome::RateLimited => self.rate_limited += 1,
            LoadOutcome::StorageFailure => self.storage_failure += 1,
        }
    }
}

/// Input of the shared load operation.
///
/// Implements FR-002 / FR-009 / FR-013 of spec 018.
#[derive(Debug, Clone)]
pub struct IdentifierLoadConfig {
    /// The batch as received (post-parse, pre-dedup; de-duplication happens
    /// inside the operation).
    pub identifiers: Vec<TypedIdentifier>,
    /// The single as-of date (validated ≤ today before any DB call).
    pub as_of_date: NaiveDate,
    /// The caller's preferred upstream source id (FR-013; `None` when
    /// unnamed). One selection per request — never per identifier and never
    /// per type.
    pub preferred_source: Option<String>,
    /// Audit trigger: `"cli"`, `"api"` or `"test"`.
    pub trigger: String,
}

/// The execution report returned by every load call.
///
/// Implements FR-006 / FR-007 of spec 018.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentifierLoadReport {
    /// The report schema version (FR-007). Currently `1`.
    pub report_version: u32,
    /// Run audit row's id (`None` only for a dry run).
    pub run_id: Option<Uuid>,
    /// The caller-named preferred source (FR-013); `None` when unnamed —
    /// preserved verbatim even when a different source actually answered.
    pub requested_source: Option<String>,
    /// The run's overall status.
    pub status: RunStatus,
    /// The identifiers as received, before de-duplication.
    pub identifiers_requested_raw: usize,
    /// The repeated entries in the request.
    pub identifiers_duplicated: usize,
    /// The unique identifiers that reached the fetch stage.
    pub identifiers_attempted: usize,
    /// The nine outcome counters.
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
    /// Per-source attempt counts.
    pub per_source_attempted: BTreeMap<String, usize>,
    /// Per-source failure counts.
    pub per_source_failed: BTreeMap<String, usize>,
    /// One line per *unique* requested identifier (duplicates collapsed).
    pub identifiers: Vec<LoadIdentifierOutcome>,
    /// When the run started.
    pub started_at: chrono::DateTime<Utc>,
    /// When the run ended.
    pub ended_at: chrono::DateTime<Utc>,
    /// The run's wall-clock duration.
    pub duration_ms: u64,
}

/// The result of the store step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoreOutcome {
    /// A row was written.
    Loaded,
    /// The identical fact was already known — nothing was written.
    NoOp,
    /// The store refused the write.
    StorageFailure,
}

/// Drives the whole batch load: validate → resolve → fetch → store → report.
///
/// Identifiers are processed sequentially, one source call in flight at a
/// time (NFR-003). Per-identifier failures are outcomes, not errors: only
/// caller-input validation and an audit-write failure return `Err`.
///
/// Implements FR-001..FR-006, FR-008 and FR-013 of spec 018.
pub async fn run_identifier_load(
    pool: &sqlx::PgPool,
    registry: &DataSourceRegistry,
    config: IdentifierLoadConfig,
) -> Result<IdentifierLoadReport, MarketDataProviderError> {
    validate_request(&config, registry)?;

    let requested_raw = config.identifiers.len();
    let (unique, duplicated) = dedupe_identifiers(config.identifiers.clone());

    let started_at = Utc::now();
    let run_row = start_load_run(pool, &config.trigger, config.preferred_source.as_deref()).await?;

    let mut counters = DataSourceLoadCounters::default();
    let mut per_source_attempted: BTreeMap<String, usize> = BTreeMap::new();
    let mut per_source_failed: BTreeMap<String, usize> = BTreeMap::new();
    let mut lines: Vec<LoadIdentifierOutcome> = Vec::with_capacity(unique.len());

    for identifier in &unique {
        let line = process_identifier(
            pool,
            registry,
            identifier,
            config.as_of_date,
            config.preferred_source.as_deref(),
            &mut counters,
            &mut per_source_attempted,
            &mut per_source_failed,
        )
        .await;
        lines.push(line);
    }

    let ended_at = Utc::now();
    let per_identifier = serde_json::to_value(&lines)
        .map_err(|e| MarketDataProviderError::Infrastructure(format!("json encode lines: {e}")))?;
    let per_source_attempted_value = serde_json::to_value(&per_source_attempted)
        .map_err(|e| MarketDataProviderError::Infrastructure(format!("json encode: {e}")))?;
    let per_source_failed_value = serde_json::to_value(&per_source_failed)
        .map_err(|e| MarketDataProviderError::Infrastructure(format!("json encode: {e}")))?;

    finish_load_run(
        pool,
        run_row.id,
        requested_raw as i32,
        duplicated as i32,
        &counters,
        &per_source_attempted_value,
        &per_source_failed_value,
        &per_identifier,
    )
    .await?;

    Ok(build_report(
        &counters,
        lines,
        Some(run_row.id),
        config.preferred_source.clone(),
        requested_raw,
        duplicated,
        per_source_attempted,
        per_source_failed,
        started_at,
        ended_at,
    ))
}

/// Resolves, fetches, classifies and stores one identifier.
///
/// Implements FR-003..FR-005 of spec 018.
#[allow(clippy::too_many_arguments)]
async fn process_identifier(
    pool: &sqlx::PgPool,
    registry: &DataSourceRegistry,
    identifier: &TypedIdentifier,
    as_of_date: NaiveDate,
    preferred_source: Option<&str>,
    counters: &mut DataSourceLoadCounters,
    per_source_attempted: &mut BTreeMap<String, usize>,
    per_source_failed: &mut BTreeMap<String, usize>,
) -> LoadIdentifierOutcome {
    let asset = match asset_identifier_read::find_asset_by_identifier(
        pool,
        &identifier.scheme,
        &identifier.value,
    )
    .await
    {
        Ok(asset) => asset,
        Err(error) => {
            tracing::warn!(
                identifier = ?identifier,
                error = %error,
                "identifier lookup failed"
            );
            counters.storage_failure += 1;
            return LoadIdentifierOutcome {
                identifier: identifier.clone(),
                asset_id: None,
                attempted: false,
                outcome: LoadOutcome::StorageFailure,
                source_id: None,
                per_source: Vec::new(),
            };
        }
    };

    let Some(asset) = asset else {
        // No asset claims the identifier: the registry is never consulted,
        // so the line was not attempted (FR-003 / research D13).
        counters.not_resolved += 1;
        return LoadIdentifierOutcome {
            identifier: identifier.clone(),
            asset_id: None,
            attempted: false,
            outcome: LoadOutcome::NotResolved,
            source_id: None,
            per_source: Vec::new(),
        };
    };

    let asset_id = AssetId(asset.id);
    let walk = registry
        .walk_historical_range(
            build_range_request(identifier, as_of_date),
            preferred_source,
        )
        .await;

    let mut per_source: Vec<SourceOutcome> = Vec::new();
    for failure in &walk.failures {
        *per_source_failed
            .entry(failure.source_id.clone())
            .or_insert(0) += 1;
        per_source.push(SourceOutcome {
            source_id: failure.source_id.clone(),
            ok: false,
            error: Some(failure.error.to_string()),
        });
    }

    if let Some(range) = &walk.range {
        *per_source_attempted
            .entry(range.source_id.clone())
            .or_insert(0) += 1;
        per_source.push(SourceOutcome {
            source_id: range.source_id.clone(),
            ok: true,
            error: None,
        });
    }

    let Some(range) = walk.range else {
        let outcome = classify_walk_failure(&walk);
        counters.increment(outcome);
        return LoadIdentifierOutcome {
            identifier: identifier.clone(),
            asset_id: Some(asset_id.0),
            attempted: true,
            outcome,
            source_id: None,
            per_source,
        };
    };

    let bars = bars_for_requested_date(&range.bars, as_of_date);
    let Some(bar) = bars.first().cloned() else {
        counters.no_data_for_date += 1;
        return LoadIdentifierOutcome {
            identifier: identifier.clone(),
            asset_id: Some(asset_id.0),
            attempted: true,
            outcome: LoadOutcome::NoDataForDate,
            source_id: Some(range.source_id.clone()),
            per_source,
        };
    };

    let stored = store_bar(
        pool,
        asset_id,
        bar,
        range.knowledge_date,
        &range.source_id,
        as_of_date,
    )
    .await;

    let outcome = match stored {
        StoreOutcome::Loaded => LoadOutcome::Loaded,
        StoreOutcome::NoOp => LoadOutcome::NoOp,
        StoreOutcome::StorageFailure => LoadOutcome::StorageFailure,
    };
    counters.increment(outcome);

    LoadIdentifierOutcome {
        identifier: identifier.clone(),
        asset_id: Some(asset_id.0),
        attempted: true,
        outcome,
        source_id: Some(range.source_id.clone()),
        per_source,
    }
}

/// Validates the caller's input before anything is contacted.
///
/// Implements FR-002 / FR-013 of spec 018.
fn validate_request(
    config: &IdentifierLoadConfig,
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
    validate_as_of_date(config.as_of_date)?;
    validate_preferred_source(config.preferred_source.as_deref(), registry)?;
    Ok(())
}

/// Rejects an identifier with an empty type or value.
///
/// Implements FR-002 of spec 018.
fn validate_identifier_pair(identifier: &TypedIdentifier) -> Result<(), MarketDataProviderError> {
    if identifier.scheme.trim().is_empty() {
        return Err(MarketDataProviderError::Validation(
            "an identifier's type is empty".into(),
        ));
    }
    if identifier.value.trim().is_empty() {
        return Err(MarketDataProviderError::Validation(format!(
            "identifier '{}' has an empty value",
            identifier.scheme
        )));
    }
    Ok(())
}

/// Rejects an as-of date later than today (UTC).
///
/// Implements FR-002 of spec 018.
fn validate_as_of_date(as_of_date: NaiveDate) -> Result<(), MarketDataProviderError> {
    let today = Utc::now().date_naive();
    if as_of_date > today {
        return Err(MarketDataProviderError::Validation(format!(
            "as-of date {as_of_date} is later than today ({today})"
        )));
    }
    Ok(())
}

/// Rejects a named preferred source that is not configured at all.
///
/// A configured-but-disabled source is not a validation error — it is
/// skipped like any other disabled source (FR-010 / FR-013).
///
/// Implements FR-002 / FR-013 of spec 018.
fn validate_preferred_source(
    preferred_source: Option<&str>,
    registry: &DataSourceRegistry,
) -> Result<(), MarketDataProviderError> {
    let Some(source_id) = preferred_source else {
        return Ok(());
    };
    let known = registry
        .enumerate_sources()
        .iter()
        .any(|source| source.source_id == source_id);
    if !known {
        return Err(MarketDataProviderError::Validation(format!(
            "preferred source '{source_id}' is not configured"
        )));
    }
    Ok(())
}

/// De-duplicates the batch, keeping the first occurrence of each identifier.
///
/// Implements FR-006 of spec 018 (research D10).
fn dedupe_identifiers(identifiers: Vec<TypedIdentifier>) -> (Vec<TypedIdentifier>, usize) {
    let mut seen: std::collections::BTreeSet<(String, String)> = std::collections::BTreeSet::new();
    let mut unique: Vec<TypedIdentifier> = Vec::with_capacity(identifiers.len());
    let mut duplicates = 0usize;
    for identifier in identifiers {
        let key = (identifier.scheme.clone(), identifier.value.clone());
        if seen.insert(key) {
            unique.push(identifier);
        } else {
            duplicates += 1;
        }
    }
    (unique, duplicates)
}

/// Builds the one-day range request for the fetch stage.
///
/// Implements FR-001 of spec 018 (research D3: `from == to == as_of_date`).
fn build_range_request(
    identifier: &TypedIdentifier,
    as_of_date: NaiveDate,
) -> HistoricalRangeRequest {
    HistoricalRangeRequest {
        identifier: identifier.clone(),
        from: as_of_date,
        to: as_of_date,
    }
}

/// Keeps only the bars that belong to the requested date.
///
/// A source that answers a one-day range with a wider window must not cause
/// the wrong day to be stored (research D3).
///
/// Implements FR-001 of spec 018.
fn bars_for_requested_date(bars: &[PriceBar], as_of_date: NaiveDate) -> Vec<PriceBar> {
    bars.iter()
        .filter(|bar| bar.as_of_date == as_of_date)
        .cloned()
        .collect()
}

/// Classifies a walk that produced no answer.
///
/// Implements FR-005 of spec 018 (research D4).
fn classify_walk_failure(walk: &crate::market_data::registry::RangeWalkOutcome) -> LoadOutcome {
    let Some(error) = &walk.error else {
        return LoadOutcome::SourceUnavailable;
    };
    // The empty-candidate case is signalled by a `NotFound` error with an
    // empty failure list (contract §1.5).
    if walk.failures.is_empty() && matches!(error, MarketDataProviderError::NotFound(_)) {
        return LoadOutcome::UnsupportedScheme;
    }
    map_provider_error_to_outcome(error)
}

/// Maps a typed provider error to exactly one outcome.
///
/// Implements FR-005 of spec 018 (research D4).
fn map_provider_error_to_outcome(error: &MarketDataProviderError) -> LoadOutcome {
    match error {
        MarketDataProviderError::NotFound(_) => LoadOutcome::NoDataForDate,
        MarketDataProviderError::InvalidInput(_)
        | MarketDataProviderError::Unsupported(_)
        | MarketDataProviderError::Unauthorized(_)
        | MarketDataProviderError::Validation(_) => LoadOutcome::Rejected,
        MarketDataProviderError::Transient(_) | MarketDataProviderError::Infrastructure(_) => {
            LoadOutcome::SourceUnavailable
        }
        MarketDataProviderError::RateLimited { .. } => LoadOutcome::RateLimited,
    }
}

/// Writes the fetched bar through the canonical dual-temporal write API.
///
/// A collision with the identical existing fact is an idempotent `no-op`
/// (research D12); any other store failure is a `storage_failure`.
///
/// Implements FR-004 of spec 018.
async fn store_bar(
    pool: &sqlx::PgPool,
    asset_id: AssetId,
    bar: PriceBar,
    knowledge_date: NaiveDate,
    source_id: &str,
    as_of_date: NaiveDate,
) -> StoreOutcome {
    // The idempotence pre-check (research D12): the dual-temporal unique key
    // is `(asset_id, fact_type, as_of_date, knowledge_date)`, so a same-day
    // re-load must not write a second row for it.
    if is_already_known(pool, asset_id, as_of_date, knowledge_date).await {
        return StoreOutcome::NoOp;
    }

    let value_payload = match serde_json::to_string(&serialize_bar(&bar)) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::warn!(error = %error, "could not encode the bar payload");
            return StoreOutcome::StorageFailure;
        }
    };

    let new_row = NewAssetReferenceData {
        asset_id,
        fact_type: FACT_TYPE_MARKET_PRICE.to_string(),
        as_of_date: bar.as_of_date,
        knowledge_date,
        value: value_payload,
        source: source_id.to_string(),
    };

    match asset_reference_data_create::create_asset_reference_data(pool, new_row).await {
        Ok(_) => StoreOutcome::Loaded,
        Err(error) => {
            if is_unique_violation(&error) {
                // A concurrent load wrote the identical fact first: re-read
                // the current row to tell an idempotent no-op from a real
                // storage failure (research D12).
                return if current_row_carries_knowledge_date(
                    pool,
                    asset_id,
                    as_of_date,
                    knowledge_date,
                )
                .await
                {
                    StoreOutcome::NoOp
                } else {
                    StoreOutcome::StorageFailure
                };
            }
            tracing::warn!(asset_id = %asset_id, error = %error, "reference-data write failed");
            StoreOutcome::StorageFailure
        }
    }
}

/// Whether the current row for the fact already carries the knowledge date.
///
/// Implements FR-004 of spec 018 (research D12).
async fn is_already_known(
    pool: &sqlx::PgPool,
    asset_id: AssetId,
    as_of_date: NaiveDate,
    knowledge_date: NaiveDate,
) -> bool {
    match asset_reference_data_read::get_current_asset_reference_data(
        pool,
        asset_id,
        FACT_TYPE_MARKET_PRICE,
        as_of_date,
    )
    .await
    {
        Ok(Some(existing)) => existing.knowledge_date >= knowledge_date,
        Ok(None) => false,
        Err(error) => {
            tracing::warn!(
                asset_id = %asset_id,
                error = %error,
                "could not read the current reference-data row; assuming the fact is new"
            );
            false
        }
    }
}

/// Whether the current row for the fact carries the intended knowledge date.
///
/// Implements FR-004 of spec 018 (research D12).
async fn current_row_carries_knowledge_date(
    pool: &sqlx::PgPool,
    asset_id: AssetId,
    as_of_date: NaiveDate,
    knowledge_date: NaiveDate,
) -> bool {
    match asset_reference_data_read::get_current_asset_reference_data(
        pool,
        asset_id,
        FACT_TYPE_MARKET_PRICE,
        as_of_date,
    )
    .await
    {
        Ok(Some(existing)) => existing.knowledge_date == knowledge_date,
        Ok(None) => false,
        Err(error) => {
            tracing::warn!(
                asset_id = %asset_id,
                error = %error,
                "could not re-read the current reference-data row after a collision"
            );
            false
        }
    }
}

/// Whether the error is a unique-constraint violation.
///
/// Implements FR-004 of spec 018 (research D12: SQLSTATE 23505).
fn is_unique_violation(error: &crate::domain::error::DomainError) -> bool {
    let rendered = error.to_string();
    rendered.contains("23505") || rendered.contains("duplicate key")
}

/// Assembles the execution report.
///
/// Implements FR-006 of spec 018.
#[allow(clippy::too_many_arguments)]
fn build_report(
    counters: &DataSourceLoadCounters,
    lines: Vec<LoadIdentifierOutcome>,
    run_id: Option<Uuid>,
    requested_source: Option<String>,
    identifiers_requested_raw: usize,
    identifiers_duplicated: usize,
    per_source_attempted: BTreeMap<String, usize>,
    per_source_failed: BTreeMap<String, usize>,
    started_at: chrono::DateTime<Utc>,
    ended_at: chrono::DateTime<Utc>,
) -> IdentifierLoadReport {
    IdentifierLoadReport {
        report_version: 1,
        run_id,
        requested_source,
        status: compute_run_status(counters),
        identifiers_requested_raw,
        identifiers_duplicated,
        identifiers_attempted: counters.total_attempted() as usize,
        loaded: counters.loaded as usize,
        no_op: counters.no_op as usize,
        not_resolved: counters.not_resolved as usize,
        no_data_for_date: counters.no_data_for_date as usize,
        unsupported_scheme: counters.unsupported_scheme as usize,
        rejected: counters.rejected as usize,
        source_unavailable: counters.source_unavailable as usize,
        rate_limited: counters.rate_limited as usize,
        storage_failure: counters.storage_failure as usize,
        per_source_attempted,
        per_source_failed,
        identifiers: lines,
        started_at,
        ended_at,
        duration_ms: (ended_at - started_at).num_milliseconds().max(0) as u64,
    }
}

/// Derives the run status from the failure buckets alone.
///
/// Implements FR-006 of spec 018 (research D14).
fn compute_run_status(counters: &DataSourceLoadCounters) -> RunStatus {
    let attempted = counters.total_attempted();
    let failed = counters.total_failed();
    if failed == 0 {
        return RunStatus::Complete;
    }
    if failed == attempted {
        return RunStatus::Failed;
    }
    RunStatus::Partial
}

/// Serializes a `PriceBar` as canonical JSON for the reference-data payload.
///
/// The shape is `{ "open": "…", "high": "…", "low": "…", "close": "…",
/// "adjclose": "…", "volume": <u64> }` — prices are decimal *strings* so no
/// precision is lost through a JSON number round-trip (Constitution
/// Principle X).
///
/// Implements FR-004 of spec 018.
pub fn serialize_bar(bar: &PriceBar) -> serde_json::Value {
    serde_json::json!({
        "open": bar.open.normalize().to_string(),
        "high": bar.high.normalize().to_string(),
        "low": bar.low.normalize().to_string(),
        "close": bar.close.normalize().to_string(),
        "adjclose": bar.adjclose.normalize().to_string(),
        "volume": bar.volume,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::sync::Arc;

    use async_trait::async_trait;
    use rust_decimal::Decimal;

    use crate::market_data::trait_::MarketDataProvider;
    use crate::market_data::types::{
        HistoricalRange, HistoricalRangeRequest, LatestBar, LatestBarRequest, TickerLookupRequest,
        TickerLookupResult,
    };

    /// A provider whose per-identifier historical answers are scripted.
    ///
    /// The load's tests need to answer *per identifier* — the registry's
    /// existing mock cannot script an error for one value and a success for
    /// another in the same source.
    struct MockLoadProvider {
        id: String,
        schemes: Vec<String>,
        priority: i32,
        enabled: bool,
        historical: HashMap<String, Result<HistoricalRange, MarketDataProviderError>>,
        /// The identifiers this provider was asked for, in order.
        asked: std::sync::Mutex<Vec<String>>,
    }

    impl MockLoadProvider {
        fn new(id: &str, schemes: &[&str], priority: i32) -> Self {
            Self {
                id: id.to_string(),
                schemes: schemes.iter().map(|s| s.to_string()).collect(),
                priority,
                enabled: true,
                historical: HashMap::new(),
                asked: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn answers(&mut self, key: &str, range: HistoricalRange) {
            self.historical.insert(key.to_string(), Ok(range));
        }

        fn fails(&mut self, key: &str, error: MarketDataProviderError) {
            self.historical.insert(key.to_string(), Err(error));
        }
    }

    #[async_trait]
    impl MarketDataProvider for MockLoadProvider {
        fn source_id(&self) -> &str {
            &self.id
        }

        fn display_name(&self) -> &str {
            &self.id
        }

        fn supported_schemes(&self) -> &[String] {
            &self.schemes
        }

        fn enabled(&self) -> bool {
            self.enabled
        }

        fn priority(&self) -> i32 {
            self.priority
        }

        async fn get_latest_bar(
            &self,
            _request: LatestBarRequest,
        ) -> Result<LatestBar, MarketDataProviderError> {
            Err(MarketDataProviderError::Unsupported(
                "mock: latest bar not implemented".into(),
            ))
        }

        async fn get_historical_range(
            &self,
            request: HistoricalRangeRequest,
        ) -> Result<HistoricalRange, MarketDataProviderError> {
            let key = format!("{}:{}", request.identifier.scheme, request.identifier.value);
            self.asked.lock().expect("asked lock").push(key.clone());
            match self.historical.get(&key) {
                Some(Ok(range)) => Ok(range.clone()),
                Some(Err(error)) => Err(error.clone()),
                None => Err(MarketDataProviderError::NotFound(format!(
                    "mock {key} not configured"
                ))),
            }
        }

        async fn lookup_ticker(
            &self,
            _request: TickerLookupRequest,
        ) -> Result<TickerLookupResult, MarketDataProviderError> {
            Err(MarketDataProviderError::Unsupported(
                "mock: lookup not implemented".into(),
            ))
        }
    }

    // ---------- fixtures ----------

    fn bar_for(date: NaiveDate) -> PriceBar {
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

    fn range_for(source: &str, date: NaiveDate) -> HistoricalRange {
        HistoricalRange {
            bars: vec![bar_for(date)],
            knowledge_date: date,
            source_id: source.to_string(),
        }
    }

    fn date(ymd: &str) -> NaiveDate {
        NaiveDate::parse_from_str(ymd, "%Y-%m-%d").expect("date")
    }

    fn registry_with(
        providers: Vec<Arc<dyn MarketDataProvider + Send + Sync>>,
    ) -> DataSourceRegistry {
        DataSourceRegistry::new(providers).expect("registry")
    }

    // ---------- T027: the outcome vocabulary's wire names ----------

    /// Every `LoadOutcome` variant serializes to exactly its contract name.
    #[test]
    fn load_outcome_serializes_to_contract_names() {
        let cases = [
            (LoadOutcome::Loaded, "loaded"),
            (LoadOutcome::NoOp, "no_op"),
            (LoadOutcome::NotResolved, "not_resolved"),
            (LoadOutcome::NoDataForDate, "no_data_for_date"),
            (LoadOutcome::UnsupportedScheme, "unsupported_scheme"),
            (LoadOutcome::Rejected, "rejected"),
            (LoadOutcome::SourceUnavailable, "source_unavailable"),
            (LoadOutcome::RateLimited, "rate_limited"),
            (LoadOutcome::StorageFailure, "storage_failure"),
        ];
        for (outcome, expected) in cases {
            let json = serde_json::to_string(&outcome).expect("serialize");
            assert_eq!(json, format!("\"{expected}\""));
        }
    }

    // ---------- T028: the counters satisfy the audit partition ----------

    /// The counters partition exactly as the audit table's `CHECK` does.
    #[test]
    fn counters_satisfy_the_audit_check_partition() {
        let counters = DataSourceLoadCounters {
            loaded: 3,
            no_op: 2,
            not_resolved: 1,
            no_data_for_date: 4,
            unsupported_scheme: 1,
            rejected: 1,
            source_unavailable: 2,
            rate_limited: 1,
            storage_failure: 1,
        };

        // The fetch-attempted partition: every bucket except `not_resolved`.
        assert_eq!(counters.total_attempted(), 3 + 2 + 4 + 1 + 1 + 2 + 1 + 1);
        // The unique count adds the never-attempted bucket.
        assert_eq!(counters.total_requested(), counters.total_attempted() + 1);
        // The failure buckets are the four that degrade the status.
        assert_eq!(counters.total_failed(), 1 + 2 + 1 + 1);
    }

    // ---------- T030/T034..T037: the error taxonomy ----------

    /// `NotFound` maps to `no_data_for_date`.
    #[test]
    fn provider_not_found_maps_to_no_data_for_date() {
        let error = MarketDataProviderError::NotFound("no bar".into());
        assert_eq!(
            map_provider_error_to_outcome(&error),
            LoadOutcome::NoDataForDate
        );
    }

    /// A provider refusal maps to `rejected` — never to `source_unavailable`.
    #[test]
    fn provider_refusals_map_to_rejected() {
        let cases = [
            MarketDataProviderError::InvalidInput("bad".into()),
            MarketDataProviderError::Unsupported("nope".into()),
            MarketDataProviderError::Unauthorized("no key".into()),
            MarketDataProviderError::Validation("invalid".into()),
        ];
        for error in cases {
            assert_eq!(map_provider_error_to_outcome(&error), LoadOutcome::Rejected);
        }
    }

    /// A transient or infrastructure failure maps to `source_unavailable`.
    #[test]
    fn provider_transient_maps_to_source_unavailable() {
        for error in [
            MarketDataProviderError::Transient("timeout".into()),
            MarketDataProviderError::Infrastructure("db down".into()),
        ] {
            assert_eq!(
                map_provider_error_to_outcome(&error),
                LoadOutcome::SourceUnavailable
            );
        }
    }

    /// A rate-limited source maps to `rate_limited`.
    #[test]
    fn provider_rate_limited_maps_to_rate_limited() {
        let error = MarketDataProviderError::RateLimited {
            msg: "budget exhausted".into(),
            retry_after: None,
        };
        assert_eq!(
            map_provider_error_to_outcome(&error),
            LoadOutcome::RateLimited
        );
    }

    // ---------- T052/T056: the classification helpers ----------

    /// An empty candidate set is the `unsupported_scheme` case, not a
    /// generic failure (contract §1.5).
    #[test]
    fn empty_candidate_set_classifies_as_unsupported_scheme() {
        let walk = crate::market_data::registry::RangeWalkOutcome {
            range: None,
            error: Some(MarketDataProviderError::NotFound(
                "no enabled source supports scheme 'ISIN'".into(),
            )),
            failures: Vec::new(),
        };
        assert_eq!(classify_walk_failure(&walk), LoadOutcome::UnsupportedScheme);
    }

    /// A walk that tried a source and failed is not `unsupported_scheme`.
    #[test]
    fn a_failed_source_is_not_unsupported_scheme() {
        let walk = crate::market_data::registry::RangeWalkOutcome {
            range: None,
            error: Some(MarketDataProviderError::NotFound("no bar".into())),
            failures: vec![crate::market_data::registry::SourceFailure {
                source_id: "mock".into(),
                error: MarketDataProviderError::NotFound("no bar".into()),
            }],
        };
        assert_eq!(classify_walk_failure(&walk), LoadOutcome::NoDataForDate);
    }

    /// A blank identifier is rejected by the pair validation.
    #[test]
    fn validate_identifier_pair_rejects_blank_parts() {
        assert!(validate_identifier_pair(&TypedIdentifier::new("", "AAPL")).is_err());
        assert!(validate_identifier_pair(&TypedIdentifier::new("TICKER", "  ")).is_err());
        assert!(validate_identifier_pair(&TypedIdentifier::new("TICKER", "AAPL")).is_ok());
    }

    /// A future date is rejected; today and the past are accepted.
    #[test]
    fn validate_as_of_date_rejects_the_future_only() {
        let today = Utc::now().date_naive();
        assert!(validate_as_of_date(today).is_ok());
        assert!(validate_as_of_date(today - chrono::Duration::days(30)).is_ok());
        assert!(validate_as_of_date(today + chrono::Duration::days(1)).is_err());
    }

    /// An empty batch is a validation error.
    #[test]
    fn empty_identifier_list_is_a_validation_error() {
        let registry = registry_with(vec![Arc::new(MockLoadProvider::new(
            "mock",
            &["TICKER"],
            10,
        ))]);
        let config = IdentifierLoadConfig {
            identifiers: Vec::new(),
            as_of_date: Utc::now().date_naive(),
            preferred_source: None,
            trigger: "test".into(),
        };
        let error = validate_request(&config, &registry).expect_err("validation error");
        assert!(matches!(error, MarketDataProviderError::Validation(_)));
    }

    /// An unknown named source is a validation error.
    #[test]
    fn unknown_named_source_is_a_validation_error() {
        let registry = registry_with(vec![Arc::new(MockLoadProvider::new(
            "mock",
            &["TICKER"],
            10,
        ))]);
        let config = IdentifierLoadConfig {
            identifiers: vec![TypedIdentifier::new("TICKER", "AAPL")],
            as_of_date: Utc::now().date_naive(),
            preferred_source: Some("not_configured".into()),
            trigger: "test".into(),
        };
        let error = validate_request(&config, &registry).expect_err("validation error");
        assert!(matches!(error, MarketDataProviderError::Validation(_)));
    }

    /// A configured named source passes validation.
    #[test]
    fn a_configured_named_source_passes_validation() {
        let registry = registry_with(vec![Arc::new(MockLoadProvider::new(
            "mock",
            &["TICKER"],
            10,
        ))]);
        let config = IdentifierLoadConfig {
            identifiers: vec![TypedIdentifier::new("TICKER", "AAPL")],
            as_of_date: Utc::now().date_naive(),
            preferred_source: Some("mock".into()),
            trigger: "test".into(),
        };
        assert!(validate_request(&config, &registry).is_ok());
    }

    // ---------- T053/T054: the batch helpers ----------

    /// De-duplication is stable and first-occurrence-wins.
    #[test]
    fn dedupe_identifiers_keeps_the_first_occurrence() {
        let (unique, duplicates) = dedupe_identifiers(vec![
            TypedIdentifier::new("TICKER", "AAPL"),
            TypedIdentifier::new("TICKER", "MSFT"),
            TypedIdentifier::new("TICKER", "AAPL"),
        ]);
        assert_eq!(unique.len(), 2);
        assert_eq!(duplicates, 1);
        assert_eq!(unique[0], TypedIdentifier::new("TICKER", "AAPL"));
        assert_eq!(unique[1], TypedIdentifier::new("TICKER", "MSFT"));
    }

    /// The range request asks for the one requested day only.
    #[test]
    fn build_range_request_asks_for_one_day() {
        let request =
            build_range_request(&TypedIdentifier::new("TICKER", "AAPL"), date("2026-10-24"));
        assert_eq!(request.from, date("2026-10-24"));
        assert_eq!(request.to, date("2026-10-24"));
    }

    /// Bars outside the requested day are discarded.
    #[test]
    fn bars_for_requested_date_keeps_only_the_requested_day() {
        let bars = vec![bar_for(date("2026-10-23")), bar_for(date("2026-10-24"))];
        let kept = bars_for_requested_date(&bars, date("2026-10-24"));
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].as_of_date, date("2026-10-24"));
    }

    // ---------- T095/T100/T104: the run status ----------

    /// The status is derived from the failure buckets alone.
    #[test]
    fn run_status_reflects_the_failure_buckets() {
        let complete = DataSourceLoadCounters {
            loaded: 2,
            not_resolved: 1,
            no_data_for_date: 1,
            unsupported_scheme: 1,
            ..Default::default()
        };
        assert_eq!(compute_run_status(&complete), RunStatus::Complete);

        // Some, but not all, attempted identifiers failed → partial.
        let partial = DataSourceLoadCounters {
            loaded: 2,
            not_resolved: 1,
            no_data_for_date: 1,
            unsupported_scheme: 1,
            source_unavailable: 1,
            ..Default::default()
        };
        assert_eq!(compute_run_status(&partial), RunStatus::Partial);

        // Every attempted identifier failed → failed.
        let all_failed = DataSourceLoadCounters {
            source_unavailable: 2,
            ..Default::default()
        };
        assert_eq!(compute_run_status(&all_failed), RunStatus::Failed);
    }

    /// The status vocabulary is closed — three values, no "all failed".
    #[test]
    fn run_status_vocabulary_is_closed() {
        let names: Vec<String> = [RunStatus::Complete, RunStatus::Partial, RunStatus::Failed]
            .iter()
            .map(|status| serde_json::to_string(status).expect("serialize"))
            .collect();
        assert_eq!(names, vec!["\"complete\"", "\"partial\"", "\"failed\""]);
    }

    // ---------- the walk outcomes the load classifies ----------

    /// The named preferred source is consulted first, and the report keeps
    /// the requested and the answering source apart (FR-013 / FR-004).
    #[tokio::test]
    async fn named_preferred_source_is_tried_first() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 24).expect("date");
        let mut preferred = MockLoadProvider::new("preferred", &["TICKER"], 20);
        preferred.answers("TICKER:AAPL", range_for("preferred", date));
        let mut normal = MockLoadProvider::new("normal", &["TICKER"], 10);
        normal.answers("TICKER:AAPL", range_for("normal", date));

        let registry = registry_with(vec![
            Arc::new(normal) as Arc<dyn MarketDataProvider + Send + Sync>,
            Arc::new(preferred) as Arc<dyn MarketDataProvider + Send + Sync>,
        ]);

        let walk = registry
            .walk_historical_range(
                HistoricalRangeRequest {
                    identifier: TypedIdentifier::new("TICKER", "AAPL"),
                    from: date,
                    to: date,
                },
                Some("preferred"),
            )
            .await;

        assert_eq!(
            walk.range.expect("range").source_id,
            "preferred",
            "the named source answers first"
        );
    }

    /// A named source that cannot answer is fell over, and its failure
    /// stays in the manifest (FR-013).
    #[tokio::test]
    async fn named_source_that_cannot_answer_falls_over() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 24).expect("date");
        let mut preferred = MockLoadProvider::new("preferred", &["TICKER"], 20);
        preferred.fails(
            "TICKER:AAPL",
            MarketDataProviderError::Transient("boom".into()),
        );
        let mut normal = MockLoadProvider::new("normal", &["TICKER"], 10);
        normal.answers("TICKER:AAPL", range_for("normal", date));

        let registry = registry_with(vec![
            Arc::new(normal) as Arc<dyn MarketDataProvider + Send + Sync>,
            Arc::new(preferred) as Arc<dyn MarketDataProvider + Send + Sync>,
        ]);

        let walk = registry
            .walk_historical_range(
                HistoricalRangeRequest {
                    identifier: TypedIdentifier::new("TICKER", "AAPL"),
                    from: date,
                    to: date,
                },
                Some("preferred"),
            )
            .await;

        assert_eq!(walk.range.expect("range").source_id, "normal");
        assert_eq!(walk.failures.len(), 1);
        assert_eq!(walk.failures[0].source_id, "preferred");
    }

    /// A disabled source is never consulted, even when it is named.
    #[tokio::test]
    async fn disabled_named_source_is_skipped() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 24).expect("date");
        let mut disabled = MockLoadProvider::new("disabled", &["TICKER"], 10);
        disabled.enabled = false;
        disabled.answers("TICKER:AAPL", range_for("disabled", date));
        let mut normal = MockLoadProvider::new("normal", &["TICKER"], 20);
        normal.answers("TICKER:AAPL", range_for("normal", date));

        let registry = registry_with(vec![
            Arc::new(disabled) as Arc<dyn MarketDataProvider + Send + Sync>,
            Arc::new(normal) as Arc<dyn MarketDataProvider + Send + Sync>,
        ]);

        let walk = registry
            .walk_historical_range(
                HistoricalRangeRequest {
                    identifier: TypedIdentifier::new("TICKER", "AAPL"),
                    from: date,
                    to: date,
                },
                Some("disabled"),
            )
            .await;

        assert_eq!(walk.range.expect("range").source_id, "normal");
        assert!(
            walk.failures.is_empty(),
            "a disabled source is never contacted"
        );
    }

    /// A rate-limited identifier does not abort the rest of the batch.
    #[tokio::test]
    async fn rate_limited_identifier_does_not_abort_the_batch() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 24).expect("date");
        let mut provider = MockLoadProvider::new("mock", &["TICKER"], 10);
        provider.fails(
            "TICKER:AAPL",
            MarketDataProviderError::RateLimited {
                msg: "budget exhausted".into(),
                retry_after: None,
            },
        );
        provider.answers("TICKER:MSFT", range_for("mock", date));

        let registry = registry_with(vec![
            Arc::new(provider) as Arc<dyn MarketDataProvider + Send + Sync>
        ]);

        let request = |value: &str| HistoricalRangeRequest {
            identifier: TypedIdentifier::new("TICKER", value),
            from: date,
            to: date,
        };

        let limited = registry.walk_historical_range(request("AAPL"), None).await;
        assert_eq!(
            map_provider_error_to_outcome(&limited.error.expect("error")),
            LoadOutcome::RateLimited
        );

        let answered = registry.walk_historical_range(request("MSFT"), None).await;
        assert!(answered.range.is_some(), "the batch continues");
    }

    // ---------- T027 (report): the report carries its schema version ----------

    /// The report carries the contract's schema version.
    #[test]
    fn report_carries_the_schema_version() {
        let counters = DataSourceLoadCounters::default();
        let now = Utc::now();
        let report = build_report(
            &counters,
            Vec::new(),
            None,
            None,
            0,
            0,
            BTreeMap::new(),
            BTreeMap::new(),
            now,
            now,
        );
        assert_eq!(report.report_version, 1);
        let json = serde_json::to_value(&report).expect("serialize");
        assert_eq!(json["report_version"], 1);
    }

    /// The report's duration is never negative.
    #[test]
    fn report_duration_is_never_negative() {
        let counters = DataSourceLoadCounters::default();
        let start = Utc::now();
        let report = build_report(
            &counters,
            Vec::new(),
            None,
            None,
            0,
            0,
            BTreeMap::new(),
            BTreeMap::new(),
            start,
            start,
        );
        assert_eq!(report.duration_ms, 0);
    }

    // ---------- T100: the report's requested vs answering source ----------

    /// The report keeps the caller's named source and the answering source
    /// apart (FR-004 / FR-013).
    #[test]
    fn report_keeps_requested_and_answering_sources_apart() {
        let counters = DataSourceLoadCounters::default();
        let now = Utc::now();
        let lines = vec![LoadIdentifierOutcome {
            identifier: TypedIdentifier::new("TICKER", "AAPL"),
            asset_id: Some(Uuid::nil()),
            attempted: true,
            outcome: LoadOutcome::Loaded,
            source_id: Some("secondary".into()),
            per_source: vec![
                SourceOutcome {
                    source_id: "preferred".into(),
                    ok: false,
                    error: Some("transient error: boom".into()),
                },
                SourceOutcome {
                    source_id: "secondary".into(),
                    ok: true,
                    error: None,
                },
            ],
        }];
        let report = build_report(
            &counters,
            lines,
            None,
            Some("preferred".into()),
            1,
            0,
            BTreeMap::new(),
            BTreeMap::new(),
            now,
            now,
        );
        assert_eq!(report.requested_source.as_deref(), Some("preferred"));
        assert_eq!(
            report.identifiers[0].source_id.as_deref(),
            Some("secondary")
        );
        assert_eq!(report.identifiers[0].per_source[0].source_id, "preferred");
    }
}
