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
//!
//! Two additions layer on the primitive without changing it:
//! `create_missing_assets` (FR-015/FR-016) resolves a missing asset through
//! an exact upstream lookup and creates it atomically before the fetch, and
//! `load_range` (FR-014) drives this operation once per calendar day with a
//! bounded window.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::asset::create::create_asset_with_identifier;
use crate::domain::asset::entity::{Asset, NewAsset};
use crate::domain::asset_identifier::read as asset_identifier_read;
use crate::domain::asset_reference_data::create as asset_reference_data_create;
use crate::domain::asset_reference_data::entity::NewAssetReferenceData;
use crate::domain::asset_reference_data::read as asset_reference_data_read;
use crate::domain::error::DomainError;
use crate::domain::id::AssetId;
use crate::market_data::audit::{finish_load_run, start_load_run};
use crate::market_data::error::MarketDataProviderError;
use crate::market_data::registry::{DataSourceRegistry, TickerLookupMatch, TickerLookupWalk};
use crate::market_data::types::{
    HistoricalRangeRequest, PriceBar, TickerLookupHit, TypedIdentifier,
};

/// Fact type written by the load operation for end-of-day OHLCV bars.
///
/// Implements FR-004 of spec 018 (the same fact type the predecessor's
/// refresh writes).
pub const FACT_TYPE_MARKET_PRICE: &str = "market_price";

/// The currency recorded on an auto-created asset when the upstream omits it.
///
/// Yahoo's search endpoint does not return a currency (spec 017 research), so
/// creation falls back to this value and records the provenance on the
/// report line (`currency_source: configured_default`).
///
/// Implements FR-015 of spec 018.
pub const DEFAULT_ASSET_CURRENCY: &str = "USD";

/// The classification of one requested identifier.
///
/// The vocabulary is closed: every requested identifier ends in exactly one
/// of these nine categories, and no category is used for a condition it does
/// not describe.
///
/// Implements FR-005 of spec 018.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum LoadOutcome {
    /// A reference-data row was written.
    Loaded,
    /// Nothing was written because the identical fact is already known
    /// (same asset, fact, as-of date and knowledge date).
    NoOp,
    /// No asset claims the identifier, so nothing was fetched.
    ///
    /// With `create_missing_assets` on, the identifier may still have been
    /// looked up upstream; `asset_creation` records that attempt.
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

/// What the asset-creation path did for one identifier.
///
/// `create_missing_assets` turns the otherwise-dead `not_resolved` line into
/// a lookup + create attempt. This type records that side effect without
/// adding a tenth outcome bucket: the line's `outcome` stays inside the
/// closed nine-category vocabulary.
///
/// Implements FR-016 of spec 018.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", tag = "status")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum AssetCreation {
    /// Creation was not asked for (`create_missing_assets` was off).
    #[default]
    NotRequested,
    /// A dry run would have attempted creation, but contacts nothing.
    WouldAttempt,
    /// This run created the asset and the identifier that names it.
    Created {
        /// The source that confirmed the exact symbol.
        source_id: String,
        /// Where the asset's currency came from.
        currency_source: CurrencySource,
    },
    /// A concurrent creator won the unique-key race; this run adopted its
    /// asset instead of failing (the mirror of the reference-data
    /// collision rule, research D12).
    Adopted {
        /// The adopted asset's id.
        asset_id: Uuid,
    },
    /// Creation was considered and deliberately refused.
    Declined {
        /// Why it was refused.
        reason: CreationDecline,
    },
    /// Creation was attempted and the store refused it.
    Failed {
        /// Credential-free error text.
        error: String,
    },
}

/// Where a created asset's currency came from.
///
/// Implements FR-015 of spec 018.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum CurrencySource {
    /// The upstream lookup returned a currency.
    Upstream,
    /// The upstream omitted it and the configured default was used.
    ConfiguredDefault,
}

/// Why the creation path refused to create an asset.
///
/// Every reason is a deliberate, reportable refusal — never a silent skip.
///
/// Implements FR-015 of spec 018.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum CreationDecline {
    /// The identifier's scheme is not `TICKER` / `TICKER:*`; v1 only
    /// creates ticker-named instruments.
    UnsupportedScheme,
    /// The identifier value is not a plausible ticker symbol.
    InvalidSymbol,
    /// Every source answered, but none returned the exact symbol.
    NoUpstreamMatch,
    /// Several exact symbol matches were returned and the venue could not
    /// disambiguate them.
    AmbiguousMatch,
    /// The exact match trades on a different venue than the identifier
    /// names, or on a venue the upstream code cannot be mapped to.
    VenueMismatch,
    /// The upstream quote type is not in the canonical asset-type map.
    UnsupportedAssetType,
    /// The lookup itself failed (a transient or infrastructure error).
    LookupFailed,
}

/// One source's answer (or failure) during a registry walk.
///
/// Implements FR-006 of spec 018.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LoadIdentifierOutcome {
    /// The identifier as received (normalized scheme/value pair).
    pub identifier: TypedIdentifier,
    /// The claimed asset, when resolution succeeded.
    pub asset_id: Option<Uuid>,
    /// What the optional asset-creation path did for this line.
    ///
    /// `not_requested` unless `create_missing_assets` was on. Only a
    /// `not_resolved` line can carry a creation record other than
    /// `not_requested`; a created asset's line goes on to report the fetch
    /// outcome and carries `created` alongside it. Serde-defaulted so
    /// version-1 payloads written before the field existed still
    /// deserialize (FR-016).
    #[serde(default)]
    pub asset_creation: AssetCreation,
    /// Whether the line belongs to the fetch-attempted partition.
    ///
    /// `true` for every outcome except `not_resolved`. A `not_resolved` line
    /// may still have consulted a source for the optional asset-creation
    /// lookup (FR-015); that activity is recorded by `asset_creation`, not
    /// here, so the audit table's partition is unchanged (FR-006/FR-016).
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum RunStatus {
    /// No identifier ended in a failure bucket.
    Complete,
    /// Some, but not all, attempted identifiers ended in a failure bucket.
    Partial,
    /// Every attempted identifier ended in a failure bucket.
    Failed,
}

/// Per-run counters: one field per outcome bucket, plus the two creation
/// side-effect counters.
///
/// The fetch-attempted buckets partition `identifiers_attempted` exactly as
/// the audit table's `CHECK` constraint does; `not_resolved` is kept out of
/// that partition (it was never attempted). `assets_created` and
/// `assets_adopted` are not outcomes and are not part of either partition.
///
/// Implements FR-005 / FR-006 / FR-008 / FR-016 of spec 018.
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
    /// Assets created because the load could not resolve an identifier.
    ///
    /// A side-effect counter, not an outcome bucket (FR-016).
    pub assets_created: i64,
    /// Assets adopted after losing a concurrent creation race.
    ///
    /// A side-effect counter, not an outcome bucket (FR-016).
    pub assets_adopted: i64,
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
    /// When `true`, the load computes the report but persists nothing at
    /// all — no reference-data rows and no run record (FR-008).
    pub dry_run: bool,
    /// When `true`, an identifier that resolves to no asset is looked up
    /// upstream and — on an exact symbol match with a supported asset type —
    /// created (with its identifier) before the fetch (FR-015/FR-016).
    pub create_missing_assets: bool,
    /// The currency recorded on a created asset when the upstream omits one
    /// (FR-015). Callers pass [`DEFAULT_ASSET_CURRENCY`].
    pub default_currency: String,
}

/// The execution report returned by every load call.
///
/// Implements FR-006 / FR-007 of spec 018.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
    /// Assets created by this run (FR-016).
    pub assets_created: usize,
    /// Assets adopted after a concurrent creation race (FR-016).
    pub assets_adopted: usize,
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
/// A **dry run** (`config.dry_run`) persists nothing at all — no
/// reference-data rows and no run record — and its report carries an
/// empty run id (FR-008). A dry run needs no database pool; every other
/// mode does.
///
/// Implements FR-001..FR-006, FR-008 and FR-013 of spec 018.
pub async fn run_identifier_load(
    pool: Option<&sqlx::PgPool>,
    registry: &DataSourceRegistry,
    config: IdentifierLoadConfig,
) -> Result<IdentifierLoadReport, MarketDataProviderError> {
    validate_request(&config, registry)?;

    let requested_raw = config.identifiers.len();
    let (unique, duplicated) = dedupe_identifiers(config.identifiers.clone());

    let started_at = Utc::now();

    if config.dry_run {
        // A dry run persists nothing at all — no reference-data rows and no
        // run record — and its report carries an empty run id (FR-008).
        let report = dry_run_report(&unique, requested_raw, duplicated, &config);
        return Ok(report);
    }

    let pool = pool.ok_or_else(|| {
        MarketDataProviderError::Infrastructure(
            "the load operation needs a database pool (a dry run does not)".into(),
        )
    })?;

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
            &config,
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

/// Builds the report of a dry run.
///
/// The dry run resolves the identifiers and reports what *would* happen,
/// but persists nothing — no reference-data rows and no run record — and
/// carries an empty run id (FR-008, contract I-5).
///
/// Implements FR-008 of spec 018.
fn dry_run_report(
    unique: &[TypedIdentifier],
    requested_raw: usize,
    duplicated: usize,
    config: &IdentifierLoadConfig,
) -> IdentifierLoadReport {
    let now = Utc::now();
    let lines: Vec<LoadIdentifierOutcome> = unique
        .iter()
        .map(|identifier| LoadIdentifierOutcome {
            identifier: identifier.clone(),
            asset_id: None,
            asset_creation: if config.create_missing_assets
                && ticker_venue(&identifier.scheme).is_some()
            {
                // A dry run contacts nothing, so it can only say what it
                // *would* attempt — never that it would create (FR-008).
                AssetCreation::WouldAttempt
            } else {
                AssetCreation::NotRequested
            },
            attempted: false,
            outcome: LoadOutcome::NotResolved,
            source_id: None,
            per_source: Vec::new(),
        })
        .collect();

    IdentifierLoadReport {
        report_version: 1,
        run_id: None,
        requested_source: config.preferred_source.clone(),
        status: RunStatus::Complete,
        identifiers_requested_raw: requested_raw,
        identifiers_duplicated: duplicated,
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
        per_source_attempted: BTreeMap::new(),
        per_source_failed: BTreeMap::new(),
        identifiers: lines,
        started_at: now,
        ended_at: now,
        duration_ms: 0,
    }
}

/// Resolves, fetches, classifies and stores one identifier.
///
/// Implements FR-003..FR-005 and FR-015..FR-016 of spec 018.
#[allow(clippy::too_many_arguments)]
async fn process_identifier(
    pool: &sqlx::PgPool,
    registry: &DataSourceRegistry,
    identifier: &TypedIdentifier,
    config: &IdentifierLoadConfig,
    counters: &mut DataSourceLoadCounters,
    per_source_attempted: &mut BTreeMap<String, usize>,
    per_source_failed: &mut BTreeMap<String, usize>,
) -> LoadIdentifierOutcome {
    let as_of_date = config.as_of_date;

    let resolved = match asset_identifier_read::find_asset_by_identifier(
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
            return outcome_line(
                identifier,
                None,
                true,
                LoadOutcome::StorageFailure,
                None,
                Vec::new(),
                AssetCreation::NotRequested,
            );
        }
    };

    let mut creation = AssetCreation::NotRequested;
    let mut per_source: Vec<SourceOutcome> = Vec::new();

    let asset = match resolved {
        Some(asset) => asset,
        None if !config.create_missing_assets => {
            // No asset claims the identifier: the fetch registry is never
            // consulted, so the line was not attempted (FR-003 / D13).
            counters.not_resolved += 1;
            return outcome_line(
                identifier,
                None,
                false,
                LoadOutcome::NotResolved,
                None,
                Vec::new(),
                AssetCreation::NotRequested,
            );
        }
        None => {
            let attempt = attempt_asset_creation(
                pool,
                registry,
                identifier,
                config,
                counters,
                per_source_attempted,
                per_source_failed,
            )
            .await;
            per_source = attempt.per_source;
            creation = attempt.creation;

            match attempt.asset {
                Some(asset) => asset,
                None => {
                    let outcome = attempt.outcome.unwrap_or(LoadOutcome::NotResolved);
                    counters.increment(outcome);
                    // `not_resolved` stays outside the fetch-attempted
                    // partition even when a creation lookup consulted a
                    // source; a lookup that failed *is* an attempted line.
                    let attempted = outcome != LoadOutcome::NotResolved;
                    return outcome_line(
                        identifier, None, attempted, outcome, None, per_source, creation,
                    );
                }
            }
        }
    };

    let asset_id = AssetId(asset.id);
    let walk = registry
        .walk_historical_range(
            build_range_request(identifier, as_of_date),
            config.preferred_source.as_deref(),
        )
        .await;

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
        return outcome_line(
            identifier,
            Some(asset_id.0),
            true,
            outcome,
            None,
            per_source,
            creation,
        );
    };

    let bars = bars_for_requested_date(&range.bars, as_of_date);
    let Some(bar) = bars.first().cloned() else {
        counters.no_data_for_date += 1;
        return outcome_line(
            identifier,
            Some(asset_id.0),
            true,
            LoadOutcome::NoDataForDate,
            Some(range.source_id.clone()),
            per_source,
            creation,
        );
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

    outcome_line(
        identifier,
        Some(asset_id.0),
        true,
        outcome,
        Some(range.source_id.clone()),
        per_source,
        creation,
    )
}

/// Assembles one report line.
///
/// Implements FR-006 of spec 018.
#[allow(clippy::too_many_arguments)]
fn outcome_line(
    identifier: &TypedIdentifier,
    asset_id: Option<Uuid>,
    attempted: bool,
    outcome: LoadOutcome,
    source_id: Option<String>,
    per_source: Vec<SourceOutcome>,
    asset_creation: AssetCreation,
) -> LoadIdentifierOutcome {
    LoadIdentifierOutcome {
        identifier: identifier.clone(),
        asset_id,
        asset_creation,
        attempted,
        outcome,
        source_id,
        per_source,
    }
}

/// The outcome of the optional lookup-and-create step.
struct CreationAttempt {
    /// The asset to fetch for, when the step produced one.
    asset: Option<Asset>,
    /// What the step did, for the report line.
    creation: AssetCreation,
    /// The line's outcome, when creation consumed the line (a decline or a
    /// store failure). `None` means "continue to the fetch".
    outcome: Option<LoadOutcome>,
    /// The lookup's per-source manifest.
    per_source: Vec<SourceOutcome>,
}

/// Looks the identifier up upstream and, on an exact match, creates the asset
/// and its identifier in one transaction.
///
/// Every refusal is reported — nothing is created on a fuzzy match, a
/// lookup failure, a venue mismatch or an unmapped asset type.
///
/// Implements FR-015 / FR-016 of spec 018.
#[allow(clippy::too_many_arguments)]
async fn attempt_asset_creation(
    pool: &sqlx::PgPool,
    registry: &DataSourceRegistry,
    identifier: &TypedIdentifier,
    config: &IdentifierLoadConfig,
    counters: &mut DataSourceLoadCounters,
    per_source_attempted: &mut BTreeMap<String, usize>,
    per_source_failed: &mut BTreeMap<String, usize>,
) -> CreationAttempt {
    let Some(venue) = ticker_venue(&identifier.scheme) else {
        return CreationAttempt::declined(CreationDecline::UnsupportedScheme);
    };
    if !is_plausible_symbol(&identifier.value) {
        return CreationAttempt::declined(CreationDecline::InvalidSymbol);
    }

    let walk = registry
        .walk_ticker_lookup_exact(
            &identifier.value,
            &identifier.scheme,
            config.preferred_source.as_deref(),
        )
        .await;

    for failure in &walk.failures {
        *per_source_failed
            .entry(failure.source_id.clone())
            .or_insert(0) += 1;
    }
    let mut per_source = lookup_manifest(&walk);

    if walk.matches.is_empty() {
        return match walk.error.as_ref() {
            Some(error) if !walk.failures.is_empty() => {
                let (outcome, reason) = match error {
                    MarketDataProviderError::NotFound(_) => {
                        (LoadOutcome::NotResolved, CreationDecline::NoUpstreamMatch)
                    }
                    other => (
                        map_provider_error_to_outcome(other),
                        CreationDecline::LookupFailed,
                    ),
                };
                CreationAttempt {
                    asset: None,
                    creation: AssetCreation::Declined { reason },
                    outcome: Some(outcome),
                    per_source,
                }
            }
            _ => CreationAttempt {
                asset: None,
                creation: AssetCreation::Declined {
                    reason: CreationDecline::NoUpstreamMatch,
                },
                outcome: Some(LoadOutcome::NotResolved),
                per_source,
            },
        };
    }

    let chosen = match choose_lookup_match(&walk.matches, venue) {
        Ok(chosen) => chosen,
        Err(reason) => {
            return CreationAttempt {
                asset: None,
                creation: AssetCreation::Declined { reason },
                outcome: Some(LoadOutcome::NotResolved),
                per_source,
            };
        }
    };

    *per_source_attempted
        .entry(chosen.source_id.clone())
        .or_insert(0) += 1;
    per_source.push(SourceOutcome {
        source_id: chosen.source_id.clone(),
        ok: true,
        error: None,
    });

    let Some(asset_type) = canonical_asset_type(chosen.hit.quote_type.as_deref()) else {
        return CreationAttempt {
            asset: None,
            creation: AssetCreation::Declined {
                reason: CreationDecline::UnsupportedAssetType,
            },
            outcome: Some(LoadOutcome::NotResolved),
            per_source,
        };
    };

    let (currency, currency_source) = derive_currency(&chosen.hit, &config.default_currency);
    let new_asset = NewAsset {
        asset_type: asset_type.to_string(),
        display_name: derive_display_name(&chosen.hit),
        currency,
        backed_by_portfolio_id: None,
        issuer_id: None,
    };

    match create_asset_with_identifier(pool, new_asset, &identifier.scheme, &identifier.value).await
    {
        Ok(asset) => {
            counters.assets_created += 1;
            CreationAttempt {
                asset: Some(asset),
                creation: AssetCreation::Created {
                    source_id: chosen.source_id.clone(),
                    currency_source,
                },
                outcome: None,
                per_source,
            }
        }
        Err(error) if is_identifier_uniqueness_violation(&error) => {
            // A concurrent creator won the unique-key race: adopt its asset
            // instead of failing (the mirror of research D12).
            match asset_identifier_read::find_asset_by_identifier(
                pool,
                &identifier.scheme,
                &identifier.value,
            )
            .await
            {
                Ok(Some(asset)) => {
                    let adopted_id = asset.id;
                    counters.assets_adopted += 1;
                    CreationAttempt {
                        asset: Some(asset),
                        creation: AssetCreation::Adopted {
                            asset_id: adopted_id,
                        },
                        outcome: None,
                        per_source,
                    }
                }
                Ok(None) | Err(_) => CreationAttempt {
                    asset: None,
                    creation: AssetCreation::Failed {
                        error: "identifier collision but no asset claims it".to_string(),
                    },
                    outcome: Some(LoadOutcome::StorageFailure),
                    per_source,
                },
            }
        }
        Err(error) => {
            tracing::warn!(
                identifier = ?identifier,
                error = %error,
                "asset creation failed"
            );
            CreationAttempt {
                asset: None,
                creation: AssetCreation::Failed {
                    error: error.to_string(),
                },
                outcome: Some(LoadOutcome::StorageFailure),
                per_source,
            }
        }
    }
}

impl CreationAttempt {
    /// A creation attempt that was refused before anything was contacted.
    fn declined(reason: CreationDecline) -> Self {
        Self {
            asset: None,
            creation: AssetCreation::Declined { reason },
            outcome: Some(LoadOutcome::NotResolved),
            per_source: Vec::new(),
        }
    }
}

/// Maps the lookup walk's failures to the report's per-source manifest.
///
/// Implements FR-006 / FR-015 of spec 018.
fn lookup_manifest(walk: &TickerLookupWalk) -> Vec<SourceOutcome> {
    walk.failures
        .iter()
        .map(|failure| SourceOutcome {
            source_id: failure.source_id.clone(),
            ok: false,
            error: Some(failure.error.to_string()),
        })
        .collect()
}

/// Picks the one exact match, refusing to guess.
///
/// A venue-qualified identifier must agree with the match's exchange; a bare
/// `TICKER` must produce exactly one match.
///
/// Implements FR-015 of spec 018.
fn choose_lookup_match<'a>(
    matches: &'a [TickerLookupMatch],
    venue: Option<&str>,
) -> Result<&'a TickerLookupMatch, CreationDecline> {
    let candidate = if matches.len() == 1 {
        &matches[0]
    } else {
        match venue {
            None => return Err(CreationDecline::AmbiguousMatch),
            Some(venue) => {
                let mut filtered = matches
                    .iter()
                    .filter(|matched| hit_matches_venue(&matched.hit, venue));
                match (filtered.next(), filtered.next()) {
                    (None, _) => return Err(CreationDecline::VenueMismatch),
                    (Some(one), None) => one,
                    (Some(_), Some(_)) => return Err(CreationDecline::AmbiguousMatch),
                }
            }
        }
    };

    if let Some(venue) = venue
        && !hit_matches_venue(&candidate.hit, venue)
    {
        return Err(CreationDecline::VenueMismatch);
    }
    Ok(candidate)
}

/// Whether a lookup hit trades on the identifier's venue.
///
/// Implements FR-015 of spec 018.
fn hit_matches_venue(hit: &TickerLookupHit, venue: &str) -> bool {
    yahoo_exchange_mic(hit.exchange.as_deref()).is_some_and(|mic| mic.eq_ignore_ascii_case(venue))
}

/// Maps Yahoo's exchange code to a MIC.
///
/// The upstream search endpoint reports venue codes such as `NMS`, not the
/// MICs the identifier schemes use; an unmapped code is treated as
/// unverifiable, never as a match.
///
/// Implements FR-015 of spec 018.
fn yahoo_exchange_mic(exchange: Option<&str>) -> Option<&'static str> {
    let code = exchange?.trim().to_ascii_uppercase();
    match code.as_str() {
        "NMS" | "NGM" | "NCM" | "NAS" => Some("XNAS"),
        "NYQ" | "NYS" => Some("XNYS"),
        "PCX" | "PSE" => Some("ARCX"),
        "ASE" | "AMEX" => Some("XASE"),
        "BTS" | "BATS" => Some("BATS"),
        "OTC" | "PNK" | "OQX" | "OB" => Some("OTC"),
        _ => None,
    }
}

/// The venue an identifier's scheme names, if it is a ticker scheme.
///
/// `None` means "not a ticker scheme"; `Some(None)` means a bare `TICKER`;
/// `Some(Some(mic))` means a venue-qualified `TICKER:<MIC>`.
///
/// Implements FR-015 of spec 018.
fn ticker_venue(scheme: &str) -> Option<Option<&str>> {
    let scheme = scheme.trim();
    if scheme.eq_ignore_ascii_case("TICKER") {
        return Some(None);
    }
    let (prefix, venue) = scheme.split_once(':')?;
    if prefix.eq_ignore_ascii_case("TICKER") && !venue.trim().is_empty() {
        Some(Some(venue.trim()))
    } else {
        None
    }
}

/// Whether a value is a plausible ticker symbol.
///
/// Implements FR-015 of spec 018.
fn is_plausible_symbol(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 20
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// Maps an upstream quote type onto the canonical asset-type vocabulary.
///
/// An unknown quote type is refused rather than guessed: a wrong asset type
/// is worse than an unresolved identifier.
///
/// Implements FR-015 of spec 018.
fn canonical_asset_type(quote_type: Option<&str>) -> Option<&'static str> {
    let quote_type = quote_type?.trim().to_ascii_uppercase();
    match quote_type.as_str() {
        "EQUITY" | "STOCK" => Some("EQUITY"),
        "ETF" => Some("ETF"),
        "MUTUALFUND" | "MUTUAL_FUND" | "MUTUAL FUND" => Some("MUTUALFUND"),
        "INDEX" => Some("INDEX"),
        "CURRENCY" => Some("CURRENCY"),
        "CRYPTOCURRENCY" | "CRYPTO" => Some("CRYPTO"),
        "FUTURE" => Some("FUTURE"),
        _ => None,
    }
}

/// Derives a created asset's display name.
///
/// Implements FR-015 of spec 018.
fn derive_display_name(hit: &TickerLookupHit) -> String {
    hit.short_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .or_else(|| {
            hit.long_name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
        })
        .unwrap_or_else(|| hit.symbol.trim())
        .to_string()
}

/// Derives a created asset's currency and records its provenance.
///
/// Implements FR-015 of spec 018.
fn derive_currency(hit: &TickerLookupHit, default_currency: &str) -> (String, CurrencySource) {
    match hit
        .currency
        .as_deref()
        .map(str::trim)
        .filter(|currency| !currency.is_empty())
    {
        Some(currency) => (currency.to_ascii_uppercase(), CurrencySource::Upstream),
        None => (
            default_currency.trim().to_ascii_uppercase(),
            CurrencySource::ConfiguredDefault,
        ),
    }
}

/// Whether an error is the identifier-uniqueness race, not a real failure.
///
/// Implements FR-015 of spec 018.
fn is_identifier_uniqueness_violation(error: &DomainError) -> bool {
    matches!(
        error,
        DomainError::InvariantViolation { invariant, .. } if *invariant == "identifier_uniqueness"
    )
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
    if config.create_missing_assets && config.default_currency.trim().is_empty() {
        return Err(MarketDataProviderError::Validation(
            "the default asset currency is empty".into(),
        ));
    }
    Ok(())
}

/// Rejects an identifier with an empty type or value.
///
/// Implements FR-002 of spec 018.
pub(crate) fn validate_identifier_pair(
    identifier: &TypedIdentifier,
) -> Result<(), MarketDataProviderError> {
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
pub(crate) fn validate_as_of_date(as_of_date: NaiveDate) -> Result<(), MarketDataProviderError> {
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
pub(crate) fn validate_preferred_source(
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
        assets_created: counters.assets_created as usize,
        assets_adopted: counters.assets_adopted as usize,
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
            assets_created: 0,
            assets_adopted: 0,
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
            dry_run: false,
            create_missing_assets: false,
            default_currency: crate::market_data::load::DEFAULT_ASSET_CURRENCY.to_string(),
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
            dry_run: false,
            create_missing_assets: false,
            default_currency: crate::market_data::load::DEFAULT_ASSET_CURRENCY.to_string(),
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
            dry_run: false,
            create_missing_assets: false,
            default_currency: crate::market_data::load::DEFAULT_ASSET_CURRENCY.to_string(),
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

    /// Every outcome increments exactly its own bucket.
    #[test]
    fn counters_increment_exactly_one_bucket_each() {
        let mut counters = DataSourceLoadCounters::default();
        counters.increment(LoadOutcome::Loaded);
        counters.increment(LoadOutcome::NoOp);
        counters.increment(LoadOutcome::NotResolved);
        counters.increment(LoadOutcome::NoDataForDate);
        counters.increment(LoadOutcome::UnsupportedScheme);
        counters.increment(LoadOutcome::Rejected);
        counters.increment(LoadOutcome::SourceUnavailable);
        counters.increment(LoadOutcome::RateLimited);
        counters.increment(LoadOutcome::StorageFailure);

        assert_eq!(counters.loaded, 1);
        assert_eq!(counters.no_op, 1);
        assert_eq!(counters.not_resolved, 1);
        assert_eq!(counters.no_data_for_date, 1);
        assert_eq!(counters.unsupported_scheme, 1);
        assert_eq!(counters.rejected, 1);
        assert_eq!(counters.source_unavailable, 1);
        assert_eq!(counters.rate_limited, 1);
        assert_eq!(counters.storage_failure, 1);
        // Nine identifiers: eight reached the fetch stage, one did not
        // (`not_resolved` is outside the attempted partition).
        assert_eq!(counters.total_attempted(), 8);
        assert_eq!(counters.total_requested(), 9);
        assert_eq!(counters.total_failed(), 4);
    }

    /// The counters start empty.
    #[test]
    fn counters_start_at_zero() {
        let counters = DataSourceLoadCounters::default();
        assert_eq!(counters.total_attempted(), 0);
        assert_eq!(counters.total_requested(), 0);
        assert_eq!(counters.total_failed(), 0);
    }

    /// The report's counters come from the run's counters, and the
    /// requested/duplicated sizes are carried through unchanged.
    #[test]
    fn report_counters_come_from_the_run_counters() {
        let counters = DataSourceLoadCounters {
            loaded: 2,
            no_op: 1,
            not_resolved: 1,
            no_data_for_date: 1,
            unsupported_scheme: 1,
            rejected: 1,
            source_unavailable: 1,
            rate_limited: 1,
            storage_failure: 1,
            assets_created: 0,
            assets_adopted: 0,
        };
        let now = Utc::now();
        let report = build_report(
            &counters,
            Vec::new(),
            Some(Uuid::nil()),
            None,
            11,
            2,
            BTreeMap::new(),
            BTreeMap::new(),
            now,
            now,
        );
        assert_eq!(report.loaded, 2);
        assert_eq!(report.no_op, 1);
        assert_eq!(report.not_resolved, 1);
        assert_eq!(report.no_data_for_date, 1);
        assert_eq!(report.unsupported_scheme, 1);
        assert_eq!(report.rejected, 1);
        assert_eq!(report.source_unavailable, 1);
        assert_eq!(report.rate_limited, 1);
        assert_eq!(report.storage_failure, 1);
        assert_eq!(report.identifiers_attempted, 9);
        assert_eq!(report.identifiers_requested_raw, 11);
        assert_eq!(report.identifiers_duplicated, 2);
        assert_eq!(report.status, RunStatus::Partial);
        assert_eq!(report.run_id, Some(Uuid::nil()));
    }

    /// The report's per-source maps are carried through unchanged.
    #[test]
    fn report_carries_the_per_source_maps() {
        let counters = DataSourceLoadCounters::default();
        let now = Utc::now();
        let mut attempted = BTreeMap::new();
        attempted.insert("yahoo_finance".to_string(), 3usize);
        let mut failed = BTreeMap::new();
        failed.insert("yahoo_finance".to_string(), 1usize);
        let report = build_report(
            &counters,
            Vec::new(),
            None,
            None,
            0,
            0,
            attempted,
            failed,
            now,
            now,
        );
        assert_eq!(report.per_source_attempted.get("yahoo_finance"), Some(&3));
        assert_eq!(report.per_source_failed.get("yahoo_finance"), Some(&1));
    }

    /// A scheme no enabled source supports is the `unsupported_scheme` case:
    /// the registry was consulted (attempted) but no source was contacted.
    #[tokio::test]
    async fn unsupported_scheme_yields_unsupported_scheme() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 24).expect("date");
        let provider = MockLoadProvider::new("ticker_source", &["TICKER"], 10);
        let registry = registry_with(vec![
            Arc::new(provider) as Arc<dyn MarketDataProvider + Send + Sync>
        ]);

        let walk = registry
            .walk_historical_range(
                HistoricalRangeRequest {
                    identifier: TypedIdentifier::new("ISIN", "US0378331005"),
                    from: date,
                    to: date,
                },
                None,
            )
            .await;

        assert!(walk.failures.is_empty(), "no source was contacted");
        assert_eq!(classify_walk_failure(&walk), LoadOutcome::UnsupportedScheme);
    }

    /// An upstream that answers "no value for that date" is the
    /// `no_data_for_date` case.
    #[tokio::test]
    async fn upstream_not_found_yields_no_data_for_date() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 24).expect("date");
        let mut provider = MockLoadProvider::new("mock", &["TICKER"], 10);
        provider.fails(
            "TICKER:AAPL",
            MarketDataProviderError::NotFound("no session for that date".into()),
        );
        let registry = registry_with(vec![
            Arc::new(provider) as Arc<dyn MarketDataProvider + Send + Sync>
        ]);

        let walk = registry
            .walk_historical_range(
                HistoricalRangeRequest {
                    identifier: TypedIdentifier::new("TICKER", "AAPL"),
                    from: date,
                    to: date,
                },
                None,
            )
            .await;

        assert_eq!(classify_walk_failure(&walk), LoadOutcome::NoDataForDate);
    }

    /// A dry run persists nothing and carries an empty run id (FR-008).
    #[tokio::test]
    async fn a_dry_run_persists_nothing() {
        let registry = registry_with(vec![Arc::new(MockLoadProvider::new(
            "mock",
            &["TICKER"],
            10,
        ))]);
        let config = IdentifierLoadConfig {
            identifiers: vec![TypedIdentifier::new("TICKER", "AAPL")],
            as_of_date: Utc::now().date_naive(),
            preferred_source: None,
            trigger: "cli".into(),
            dry_run: true,
            create_missing_assets: false,
            default_currency: crate::market_data::load::DEFAULT_ASSET_CURRENCY.to_string(),
        };

        // No pool is needed for a dry run.
        let report = run_identifier_load(None, &registry, config)
            .await
            .expect("report");
        assert!(report.run_id.is_none(), "a dry run has no run record");
        assert_eq!(report.identifiers.len(), 1);
    }

    /// A non-dry run without a pool is an infrastructure error, not a
    /// silent no-op.
    #[tokio::test]
    async fn a_run_without_a_pool_is_an_error() {
        let registry = registry_with(vec![Arc::new(MockLoadProvider::new(
            "mock",
            &["TICKER"],
            10,
        ))]);
        let config = IdentifierLoadConfig {
            identifiers: vec![TypedIdentifier::new("TICKER", "AAPL")],
            as_of_date: Utc::now().date_naive(),
            preferred_source: None,
            trigger: "cli".into(),
            dry_run: false,
            create_missing_assets: false,
            default_currency: crate::market_data::load::DEFAULT_ASSET_CURRENCY.to_string(),
        };
        let error = run_identifier_load(None, &registry, config)
            .await
            .expect_err("err");
        assert!(matches!(error, MarketDataProviderError::Infrastructure(_)));
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
            asset_creation: AssetCreation::NotRequested,
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
    // ---------- FR-015/FR-016: the creation helpers ----------

    fn lookup_hit(symbol: &str, exchange: &str) -> TickerLookupHit {
        TickerLookupHit {
            symbol: symbol.to_string(),
            short_name: Some("Short Name".to_string()),
            long_name: Some("A Longer Name".to_string()),
            quote_type: Some("ETF".to_string()),
            exchange: Some(exchange.to_string()),
            currency: None,
            schemes: Vec::new(),
            relevance: Some(1.0),
        }
    }

    /// Only ticker schemes are eligible, and a venue qualifier is retained.
    #[test]
    fn ticker_venue_classifies_schemes() {
        assert_eq!(ticker_venue("TICKER"), Some(None));
        assert_eq!(ticker_venue("ticker"), Some(None));
        assert_eq!(ticker_venue("TICKER:XNAS"), Some(Some("XNAS")));
        assert_eq!(ticker_venue("ticker:xnas"), Some(Some("xnas")));
        assert_eq!(ticker_venue("ISIN"), None);
        assert_eq!(ticker_venue("CUSIP"), None);
        assert_eq!(ticker_venue("TICKER:"), None);
    }

    /// A plausible symbol is short and ASCII; anything else is refused.
    #[test]
    fn symbols_are_validated_conservatively() {
        assert!(is_plausible_symbol("AAPL"));
        assert!(is_plausible_symbol("BRK.B"));
        assert!(is_plausible_symbol("RDS-A"));
        assert!(!is_plausible_symbol(""));
        assert!(!is_plausible_symbol("   "));
        assert!(!is_plausible_symbol("AAPL; DROP TABLE"));
        assert!(!is_plausible_symbol(&"A".repeat(21)));
    }

    /// The asset-type map is canonical and closed.
    #[test]
    fn asset_types_map_to_a_closed_vocabulary() {
        assert_eq!(canonical_asset_type(Some("EQUITY")), Some("EQUITY"));
        assert_eq!(canonical_asset_type(Some("etf")), Some("ETF"));
        assert_eq!(canonical_asset_type(Some("MUTUALFUND")), Some("MUTUALFUND"));
        assert_eq!(canonical_asset_type(Some("INDEX")), Some("INDEX"));
        assert_eq!(canonical_asset_type(None), None);
        assert_eq!(canonical_asset_type(Some("SOMETHING_NEW")), None);
    }

    /// Display name prefers the short name, then the long name, then the
    /// symbol.
    #[test]
    fn display_name_falls_back_in_order() {
        let mut hit = lookup_hit("QQQM", "NMS");
        assert_eq!(derive_display_name(&hit), "Short Name");
        hit.short_name = Some("  ".to_string());
        assert_eq!(derive_display_name(&hit), "A Longer Name");
        hit.long_name = None;
        assert_eq!(derive_display_name(&hit), "QQQM");
    }

    /// Currency provenance is recorded, never guessed silently.
    #[test]
    fn currency_records_its_provenance() {
        let mut hit = lookup_hit("QQQM", "NMS");
        assert_eq!(
            derive_currency(&hit, "usd"),
            ("USD".to_string(), CurrencySource::ConfiguredDefault)
        );
        hit.currency = Some("eur".to_string());
        assert_eq!(
            derive_currency(&hit, "USD"),
            ("EUR".to_string(), CurrencySource::Upstream)
        );
    }

    /// A bare ticker must resolve unambiguously.
    #[test]
    fn an_ambiguous_bare_ticker_is_refused() {
        let matches = vec![
            TickerLookupMatch {
                source_id: "a".into(),
                hit: lookup_hit("ABC", "NMS"),
            },
            TickerLookupMatch {
                source_id: "a".into(),
                hit: lookup_hit("ABC", "NYQ"),
            },
        ];
        assert_eq!(
            choose_lookup_match(&matches, None).expect_err("ambiguous"),
            CreationDecline::AmbiguousMatch
        );
    }

    /// A venue qualifier disambiguates, and a foreign venue is refused.
    #[test]
    fn a_venue_qualifier_selects_or_refuses() {
        let matches = vec![
            TickerLookupMatch {
                source_id: "a".into(),
                hit: lookup_hit("ABC", "NMS"),
            },
            TickerLookupMatch {
                source_id: "a".into(),
                hit: lookup_hit("ABC", "NYQ"),
            },
        ];
        let chosen = choose_lookup_match(&matches, Some("XNYS")).expect("chosen");
        assert_eq!(chosen.hit.exchange.as_deref(), Some("NYQ"));

        assert_eq!(
            choose_lookup_match(&matches, Some("XLON")).expect_err("mismatch"),
            CreationDecline::VenueMismatch
        );
    }

    /// Yahoo's exchange codes map onto MICs; unknown codes do not.
    #[test]
    fn yahoo_exchanges_map_to_mics() {
        assert_eq!(yahoo_exchange_mic(Some("NMS")), Some("XNAS"));
        assert_eq!(yahoo_exchange_mic(Some("nyq")), Some("XNYS"));
        assert_eq!(yahoo_exchange_mic(Some("PCX")), Some("ARCX"));
        assert_eq!(yahoo_exchange_mic(Some("GER")), None);
        assert_eq!(yahoo_exchange_mic(None), None);
    }

    /// The creation record serializes to the contract names.
    #[test]
    fn asset_creation_serializes_to_contract_names() {
        let cases = [
            (
                AssetCreation::NotRequested,
                serde_json::json!({"status": "not_requested"}),
            ),
            (
                AssetCreation::WouldAttempt,
                serde_json::json!({"status": "would_attempt"}),
            ),
            (
                AssetCreation::Declined {
                    reason: CreationDecline::NoUpstreamMatch,
                },
                serde_json::json!({"status": "declined", "reason": "no_upstream_match"}),
            ),
            (
                AssetCreation::Adopted {
                    asset_id: Uuid::nil(),
                },
                serde_json::json!({"status": "adopted", "asset_id": Uuid::nil()}),
            ),
        ];
        for (creation, expected) in cases {
            assert_eq!(
                serde_json::to_value(&creation).expect("serialize"),
                expected
            );
        }
    }

    /// A version-1 payload written before the field existed still parses.
    #[test]
    fn a_line_without_asset_creation_deserializes_as_not_requested() {
        let line = serde_json::json!({
            "identifier": {"scheme": "TICKER", "value": "AAPL"},
            "asset_id": null,
            "attempted": false,
            "outcome": "not_resolved",
            "source_id": null,
            "per_source": []
        });
        let parsed: LoadIdentifierOutcome = serde_json::from_value(line).expect("deserialize");
        assert_eq!(parsed.asset_creation, AssetCreation::NotRequested);
    }
}
