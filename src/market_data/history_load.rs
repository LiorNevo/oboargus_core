//! Bounded history loading for an existing asset.
//!
//! The chart requests a missing calendar window and expects one provider range
//! call, not one load per day. This module resolves the asset's preferred
//! identifier, walks the configured sources once for the window, stores every
//! returned bar through the shared bitemporal write path, and reports the
//! outcome with the same vocabulary used by the existing load operation.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::asset::read::get_asset;
use crate::domain::asset_identifier::read::list_asset_identifiers;
use crate::domain::id::AssetId;
use crate::market_data::audit::{LoadRunContext, finish_load_run, start_load_run_with_context};
use crate::market_data::data_domain::DataDomain;
use crate::market_data::error::MarketDataProviderError;
use crate::market_data::load::{DataSourceLoadCounters, RunStatus, StoreOutcome, store_price_bar};
use crate::market_data::registry::DataSourceRegistry;
use crate::market_data::types::{HistoricalRangeRequest, TypedIdentifier};

/// Largest accepted history window in calendar days.
///
/// Implements FR-035 of spec 031.
pub const MAX_HISTORY_WINDOW_DAYS: i64 = 366;

/// Input of one bounded history load.
///
/// Implements FR-035 of spec 031.
#[derive(Debug, Clone)]
pub struct HistoryLoadConfig {
    /// Existing asset.
    pub asset_id: AssetId,
    /// Inclusive first date.
    pub from: NaiveDate,
    /// Inclusive last date.
    pub to: NaiveDate,
    /// Optional preferred source.
    pub preferred_source: Option<String>,
    /// Audit trigger (`"chart"` from the API).
    pub trigger: String,
}

/// The execution report of one bounded history load.
///
/// Implements FR-035 / FR-043 of spec 031.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HistoryLoadReport {
    /// Report schema version.
    pub report_version: u32,
    /// Audit row id.
    pub run_id: Option<Uuid>,
    /// The loaded asset.
    pub asset_id: Uuid,
    /// Inclusive first date.
    pub from: NaiveDate,
    /// Inclusive last date.
    pub to: NaiveDate,
    /// Worst status across storage/source outcomes.
    pub status: RunStatus,
    /// Source that answered the range.
    pub source_id: Option<String>,
    /// Bars written.
    pub loaded: usize,
    /// Bars already known identically.
    pub no_op: usize,
    /// True when the source answered with no bars in the window.
    pub no_data: bool,
    /// Transient/infrastructure failures.
    pub source_unavailable: usize,
    /// Rate-limit outcomes.
    pub rate_limited: usize,
    /// Provider refusals.
    pub rejected: usize,
    /// Store refusals.
    pub storage_failure: usize,
    /// Wall-clock duration.
    pub duration_ms: u64,
    /// Safe explanation for partial/failed/no-data outcomes.
    pub message: Option<String>,
}

fn validate(config: &HistoryLoadConfig) -> Result<(), MarketDataProviderError> {
    let today = Utc::now().date_naive();
    if config.from > config.to {
        return Err(MarketDataProviderError::Validation(
            "the history range start is later than its end".into(),
        ));
    }
    if config.to > today {
        return Err(MarketDataProviderError::Validation(format!(
            "the history range ends after today ({today})"
        )));
    }
    let days = (config.to - config.from).num_days() + 1;
    if days > MAX_HISTORY_WINDOW_DAYS {
        return Err(MarketDataProviderError::Validation(format!(
            "the history range is {days} days; the maximum is {MAX_HISTORY_WINDOW_DAYS}"
        )));
    }
    Ok(())
}

fn preferred_identifier(
    identifiers: &[crate::domain::asset_identifier::entity::AssetIdentifier],
) -> Option<TypedIdentifier> {
    let chosen = identifiers
        .iter()
        .find(|identifier| identifier.scheme.to_ascii_uppercase().starts_with("TICKER"))
        .or_else(|| identifiers.first())?;
    Some(TypedIdentifier::new(
        chosen.scheme.clone(),
        chosen.value.clone(),
    ))
}

fn status_for(counters: &DataSourceLoadCounters) -> RunStatus {
    let failures = counters.rejected
        + counters.source_unavailable
        + counters.rate_limited
        + counters.storage_failure;
    let successes = counters.loaded + counters.no_op;
    if failures == 0 {
        RunStatus::Complete
    } else if successes > 0 {
        RunStatus::Partial
    } else {
        RunStatus::Failed
    }
}

fn classify_walk_failure(
    error: &MarketDataProviderError,
    counters: &mut DataSourceLoadCounters,
) -> bool {
    match error {
        MarketDataProviderError::NotFound(_) => {
            counters.no_data_for_date += 1;
            true
        }
        MarketDataProviderError::RateLimited { .. } => {
            counters.rate_limited += 1;
            false
        }
        MarketDataProviderError::Transient(_) | MarketDataProviderError::Infrastructure(_) => {
            counters.source_unavailable += 1;
            false
        }
        _ => {
            counters.rejected += 1;
            false
        }
    }
}

/// Loads one bounded window for an existing asset.
///
/// Implements FR-035 / FR-039 / FR-043 of spec 031. It does not create
/// assets, identifiers, or providers.
pub async fn run_asset_history_load(
    pool: &sqlx::PgPool,
    registry: &DataSourceRegistry,
    config: HistoryLoadConfig,
) -> Result<HistoryLoadReport, MarketDataProviderError> {
    validate(&config)?;
    let asset = get_asset(pool, config.asset_id).await.map_err(|error| {
        MarketDataProviderError::Validation(format!("asset lookup failed: {error}"))
    })?;
    let identifiers = list_asset_identifiers(pool, config.asset_id)
        .await
        .map_err(|error| {
            MarketDataProviderError::Infrastructure(format!(
                "asset identifier lookup failed: {error}"
            ))
        })?;
    let identifier = preferred_identifier(&identifiers).ok_or_else(|| {
        MarketDataProviderError::Validation(
            "the asset has no identifier that a source can resolve".into(),
        )
    })?;

    let started_at = Utc::now();
    let context = LoadRunContext {
        data_domain: DataDomain::MarketData,
        as_of_date: None,
        range_group_id: None,
        range_from: Some(config.from),
        range_to: Some(config.to),
        range_concurrency: None,
    };
    let run = start_load_run_with_context(
        pool,
        &config.trigger,
        config.preferred_source.as_deref(),
        &context,
    )
    .await?;

    let snapshot = registry.snapshot();
    let walk = snapshot
        .walk_historical_range(
            HistoricalRangeRequest {
                identifier: identifier.clone(),
                from: config.from,
                to: config.to,
            },
            config.preferred_source.as_deref(),
        )
        .await;

    let mut counters = DataSourceLoadCounters::default();
    let mut per_source_attempted: BTreeMap<String, i64> = BTreeMap::new();
    let mut per_source_failed: BTreeMap<String, i64> = BTreeMap::new();
    let mut source_id: Option<String> = None;
    let mut no_data = false;
    let mut message: Option<String> = None;

    for failure in &walk.failures {
        *per_source_failed
            .entry(failure.source_id.clone())
            .or_insert(0) += 1;
    }

    if let Some(range) = walk.range {
        source_id = Some(range.source_id.clone());
        *per_source_attempted
            .entry(range.source_id.clone())
            .or_insert(0) += 1;
        let today = Utc::now().date_naive();
        let bars: Vec<_> = range
            .bars
            .into_iter()
            .filter(|bar| {
                bar.as_of_date >= config.from
                    && bar.as_of_date <= config.to
                    && bar.as_of_date <= today
            })
            .collect();
        if bars.is_empty() {
            counters.no_data_for_date += 1;
            no_data = true;
            message = Some("The source returned no bars for this period.".to_string());
        } else {
            for bar in bars {
                let as_of_date = bar.as_of_date;
                let (outcome, _) = store_price_bar(
                    pool,
                    config.asset_id,
                    bar,
                    &range.source_id,
                    as_of_date,
                    &asset.currency,
                )
                .await;
                match outcome {
                    StoreOutcome::Loaded => counters.loaded += 1,
                    StoreOutcome::NoOp => counters.no_op += 1,
                    StoreOutcome::StorageFailure => counters.storage_failure += 1,
                }
            }
        }
    } else if let Some(error) = &walk.error {
        let absent = classify_walk_failure(error, &mut counters);
        no_data = absent;
        message = Some(error.to_string());
    } else {
        counters.source_unavailable += 1;
        message = Some("No configured source answered the history request.".to_string());
    }

    let status = status_for(&counters);
    let per_source_attempted_value =
        serde_json::to_value(&per_source_attempted).map_err(|error| {
            MarketDataProviderError::Infrastructure(format!("json encode: {error}"))
        })?;
    let per_source_failed_value = serde_json::to_value(&per_source_failed).map_err(|error| {
        MarketDataProviderError::Infrastructure(format!("json encode: {error}"))
    })?;
    let per_identifier = serde_json::json!([{
        "identifier": { "scheme": identifier.scheme, "value": identifier.value },
        "asset_id": config.asset_id.0,
        "source_id": source_id,
        "loaded": counters.loaded,
        "no_op": counters.no_op,
        "no_data": no_data,
    }]);

    // The audit table requires requested - duplicated = attempted. A history
    // run reports bar-level counts, so the request count is the number of
    // attempted bars/source outcomes, not the single input identifier.
    finish_load_run(
        pool,
        run.id,
        counters.total_attempted() as i32,
        0,
        &counters,
        &per_source_attempted_value,
        &per_source_failed_value,
        &per_identifier,
    )
    .await?;

    let ended_at = Utc::now();
    Ok(HistoryLoadReport {
        report_version: 1,
        run_id: Some(run.id),
        asset_id: config.asset_id.0,
        from: config.from,
        to: config.to,
        status,
        source_id,
        loaded: counters.loaded as usize,
        no_op: counters.no_op as usize,
        no_data,
        source_unavailable: counters.source_unavailable as usize,
        rate_limited: counters.rate_limited as usize,
        rejected: counters.rejected as usize,
        storage_failure: counters.storage_failure as usize,
        duration_ms: (ended_at - started_at).num_milliseconds().max(0) as u64,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn identifier(
        scheme: &str,
        value: &str,
    ) -> crate::domain::asset_identifier::entity::AssetIdentifier {
        crate::domain::asset_identifier::entity::AssetIdentifier {
            id: uuid::Uuid::from_u128(1),
            asset_id: uuid::Uuid::from_u128(2),
            scheme: scheme.to_string(),
            value: value.to_string(),
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        }
    }

    #[test]
    fn validates_range_order_future_and_window_size() {
        let today = Utc::now().date_naive();
        let base = HistoryLoadConfig {
            asset_id: AssetId(uuid::Uuid::from_u128(1)),
            from: today - chrono::Duration::days(1),
            to: today,
            preferred_source: None,
            trigger: "test".into(),
        };
        assert!(validate(&base).is_ok());
        assert!(
            validate(&HistoryLoadConfig {
                from: today,
                to: today - chrono::Duration::days(1),
                ..base.clone()
            })
            .is_err()
        );
        assert!(
            validate(&HistoryLoadConfig {
                to: today + chrono::Duration::days(1),
                ..base.clone()
            })
            .is_err()
        );
        assert!(
            validate(&HistoryLoadConfig {
                from: today - chrono::Duration::days(MAX_HISTORY_WINDOW_DAYS),
                to: today,
                ..base
            })
            .is_err()
        );
    }

    #[test]
    fn prefers_a_ticker_identifier() {
        let chosen = preferred_identifier(&[
            identifier("ISIN", "US0378331005"),
            identifier("TICKER:XNAS", "AAPL"),
        ])
        .expect("identifier");
        assert_eq!(chosen.scheme, "TICKER:XNAS");
        assert_eq!(chosen.value, "AAPL");
        assert!(preferred_identifier(&[]).is_none());
    }

    #[test]
    fn computes_run_status_from_failures_and_successes() {
        let mut counters = DataSourceLoadCounters {
            loaded: 2,
            ..DataSourceLoadCounters::default()
        };
        assert_eq!(status_for(&counters), RunStatus::Complete);
        counters.storage_failure = 1;
        assert_eq!(status_for(&counters), RunStatus::Partial);
        counters.loaded = 0;
        counters.no_op = 0;
        assert_eq!(status_for(&counters), RunStatus::Failed);
    }

    #[test]
    fn classifies_absence_and_failures() {
        let mut counters = DataSourceLoadCounters::default();
        assert!(classify_walk_failure(
            &MarketDataProviderError::NotFound("no bars".into()),
            &mut counters
        ));
        assert_eq!(counters.no_data_for_date, 1);
        assert!(!classify_walk_failure(
            &MarketDataProviderError::RateLimited {
                msg: "slow down".into(),
                retry_after: None,
            },
            &mut counters
        ));
        assert_eq!(counters.rate_limited, 1);
        assert!(!classify_walk_failure(
            &MarketDataProviderError::Transient("down".into()),
            &mut counters
        ));
        assert_eq!(counters.source_unavailable, 1);
    }
}
