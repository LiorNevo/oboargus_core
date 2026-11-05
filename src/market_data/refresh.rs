//! Refresh orchestrator: drives per-asset price refreshes through the registry.
//!
//! Implements FR-007 / FR-013 / FR-014 / FR-030..FR-032 of
//! `specs/017-yahoo-data-source/spec.md`. The orchestrator writes typed market
//! observations through the normalized fact surface and persists a
//! `data_source_refresh_runs` audit row per call.

use std::collections::HashMap;

use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use crate::domain::asset_identifier::read as asset_identifier_read;
use crate::domain::id::AssetId;
use crate::domain::id::DomainId;
use crate::market_data::audit::{DataSourceRefreshCounters, finish_refresh_run, start_refresh_run};
use crate::market_data::error::MarketDataProviderError;
use crate::market_data::registry::DataSourceRegistry;
use crate::market_data::types::{LatestBarRequest, TypedIdentifier};
use crate::temporal::{
    Adjustment, FactInput, FactSubject, FactValue, KnownAt, MarketPriceValue, ValidAt,
    write_fact_outcome,
};

/// Input for `run_refresh`.
///
/// Implements FR-030 of spec 017.
#[derive(Debug, Clone)]
pub struct RefreshConfig {
    /// Specific assets to refresh. `None` means "scan every asset with at
    /// least one identifier supported by any enabled source" (FR-030).
    pub assets: Option<Vec<AssetId>>,
    /// Audit `trigger` label (e.g. `"scheduler"`, `"api"`, `"cli"`).
    pub trigger: String,
}

impl RefreshConfig {
    /// Defaults to scanning all assets with `trigger = "cli"`.
    ///
    /// Implements FR-030 of spec 017.
    pub fn cli_all() -> Self {
        Self {
            assets: None,
            trigger: "cli".into(),
        }
    }
}

/// Result of `run_refresh`.
///
/// Implements FR-007 / FR-032 of spec 017.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshSummary {
    /// Run id (matches the audit row's id).
    pub run_id: Uuid,
    /// Total assets attempted.
    pub assets_attempted: i64,
    /// Assets successfully refreshed (a new row was written).
    pub assets_refreshed: i64,
    /// Assets whose bar matched the existing latest row (idempotent
    /// re-run per FR-031).
    pub assets_no_op: i64,
    /// Assets with no supported identifier for any enabled source.
    pub assets_missing_identifier: i64,
    /// Assets where the source returned a typed retryable error.
    pub assets_source_unavailable: i64,
}

/// Fact type written by `run_refresh` for end-of-day OHLCV bars.
///
/// Implements FR-013 of spec 017. The value is a typed
/// `MarketPriceValue` stored in `market_observations`.
pub const FACT_TYPE_MARKET_PRICE: &str = "market_price";

/// Drives a refresh run over the configured assets.
///
/// Writes `AssetReferenceData` rows and a `data_source_refresh_runs` audit
/// row. The run is idempotent in the latest-`knowledge_date`-wins sense
/// (research D6): re-running for the same trading day does not produce
/// duplicate canonical rows because the predecessor's write function uses
/// `INSERT … VALUES (…, today(), …)` and the read-back resolves "current"
/// to the latest knowledge.
///
/// Implements FR-007 / FR-013 / FR-014 / FR-030..FR-032 of spec 017.
pub async fn run_refresh(
    pool: &sqlx::PgPool,
    registry: &DataSourceRegistry,
    config: RefreshConfig,
) -> Result<RefreshSummary, MarketDataProviderError> {
    let run_row = start_refresh_run(pool, &config.trigger).await?;

    let assets_to_process: Vec<AssetId> = match config.assets.clone() {
        Some(list) => list,
        None => scan_assets_with_supported_identifier(pool, registry).await?,
    };

    let mut counters = DataSourceRefreshCounters::default();
    let mut per_source_attempted: HashMap<String, i64> = HashMap::new();
    let mut per_source_failed: HashMap<String, i64> = HashMap::new();

    for asset_id in assets_to_process {
        let identifier = match asset_identifier_read::list_asset_identifiers(pool, asset_id).await {
            Ok(ids) => pick_supported_identifier(&ids, registry),
            Err(_) => {
                counters.missing_identifier += 1;
                continue;
            }
        };

        let typed_id = match identifier {
            Some(id) => id,
            None => {
                counters.missing_identifier += 1;
                continue;
            }
        };

        let request = LatestBarRequest {
            identifier: typed_id,
            as_of_now: Utc::now(),
        };

        let outcome = registry.walk_latest_bar(request).await;

        // Record every source that failed during the walk, under its own
        // source id (FR-032).
        for failure in &outcome.failures {
            *per_source_failed
                .entry(failure.source_id.clone())
                .or_insert(0) += 1;
        }

        let Some(latest) = outcome.bar else {
            tracing::warn!(
                asset_id = %asset_id,
                error = ?outcome.error,
                "refresh failed"
            );
            counters.source_unavailable += 1;
            continue;
        };

        {
            *per_source_attempted
                .entry(latest.source_id.clone())
                .or_insert(0) += 1;

            let currency = asset_currency(pool, asset_id).await;
            let value = FactValue::MarketPrice(MarketPriceValue {
                open: latest.bar.open,
                high: latest.bar.high,
                low: latest.bar.low,
                close: latest.bar.close,
                adjclose: Some(latest.bar.adjclose),
                volume: Some(rust_decimal::Decimal::from(latest.bar.volume)),
                currency,
                adjustment: Adjustment::Unadjusted,
            });
            let input = FactInput {
                subject: FactSubject::Asset(asset_id),
                fact_type: FACT_TYPE_MARKET_PRICE.to_string(),
                valid_at: ValidAt::midnight_utc(latest.bar.as_of_date),
                known_at: KnownAt::now(),
                source: latest.source_id.clone(),
                value,
            };
            match write_fact_outcome(pool, input).await {
                Ok((_, true)) => counters.refreshed += 1,
                Ok((_, false)) => counters.no_op += 1,
                Err(e) => {
                    tracing::warn!(asset_id = %asset_id, error = %e, "market observation write failed");
                    counters.source_unavailable += 1;
                    *per_source_failed.entry(latest.source_id).or_insert(0) += 1;
                }
            }
        }
    }

    let per_source_attempted_value =
        serde_json::to_value(&per_source_attempted).unwrap_or_else(|_| json!({}));
    let per_source_failed_value =
        serde_json::to_value(&per_source_failed).unwrap_or_else(|_| json!({}));

    let final_row = finish_refresh_run(
        pool,
        run_row.id,
        &counters,
        &per_source_attempted_value,
        &per_source_failed_value,
    )
    .await?;

    Ok(RefreshSummary {
        run_id: final_row.id,
        assets_attempted: counters.total(),
        assets_refreshed: counters.refreshed,
        assets_no_op: counters.no_op,
        assets_missing_identifier: counters.missing_identifier,
        assets_source_unavailable: counters.source_unavailable,
    })
}

/// Reads the asset's reporting currency, defaulting to USD for a legacy row.
async fn asset_currency(pool: &sqlx::PgPool, asset_id: AssetId) -> String {
    let currency: Option<String> = sqlx::query_scalar("SELECT currency FROM assets WHERE id = $1")
        .bind(asset_id.as_uuid())
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    currency.unwrap_or_else(|| "USD".to_string())
}

/// Picks the first identifier whose scheme is supported by at least one
/// enabled source. Returns `None` if none qualify.
fn pick_supported_identifier(
    identifiers: &[crate::domain::asset_identifier::entity::AssetIdentifier],
    registry: &DataSourceRegistry,
) -> Option<TypedIdentifier> {
    let sources = registry.enumerate_sources();
    for id in identifiers {
        for src in &sources {
            if !src.enabled {
                continue;
            }
            if scheme_matches(&src.supported_schemes, &id.scheme) {
                return Some(TypedIdentifier::new(&id.scheme, &id.value));
            }
        }
    }
    None
}

fn scheme_matches(supported: &[String], scheme: &str) -> bool {
    for s in supported {
        if s == scheme {
            return true;
        }
        if let Some(prefix) = s.strip_suffix(":*")
            && scheme.starts_with(prefix)
            && scheme.contains(':')
        {
            return true;
        }
        if s == "*" {
            return true;
        }
    }
    false
}

/// Returns the assets that have at least one identifier scheme supported by
/// any enabled source. Implements the "scan all" path of `RefreshConfig`.
async fn scan_assets_with_supported_identifier(
    pool: &sqlx::PgPool,
    _registry: &DataSourceRegistry,
) -> Result<Vec<AssetId>, MarketDataProviderError> {
    // Pull every Asset row; the identifier filtering is done per-asset by
    // `pick_supported_identifier`. The scan is sequential in v1; parallel
    // is a follow-on (research D-out-of-scope, tasks T065).
    let rows: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM assets")
        .fetch_all(pool)
        .await
        .map_err(|e| MarketDataProviderError::Infrastructure(format!("scan assets: {e}")))?;
    Ok(rows
        .into_iter()
        .map(|(id,)| AssetId::from_uuid(id))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_matches_handles_wildcards() {
        assert!(scheme_matches(&["TICKER".to_string()], "TICKER"));
        assert!(scheme_matches(&["TICKER:*".to_string()], "TICKER:XNAS"));
        assert!(scheme_matches(&["*".to_string()], "anything"));
        assert!(!scheme_matches(&["ISIN".to_string()], "TICKER"));
    }

    #[test]
    fn refresh_summary_counters_partition_total() {
        let c = DataSourceRefreshCounters {
            refreshed: 1,
            no_op: 2,
            missing_identifier: 3,
            source_unavailable: 4,
        };
        assert_eq!(c.total(), 10);
    }

    /// The identifier picker prefers an identifier whose scheme an enabled
    /// source supports, and skips the ones no source can answer.
    #[test]
    fn pick_supported_identifier_skips_unsupported_schemes() {
        let registry = crate::market_data::registry::tests::registry_with_ticker_source();
        let identifiers = vec![
            identifier("LEI", "5493001KJTIIGC8Y1R12"),
            identifier("TICKER", "AAPL"),
        ];
        let picked = pick_supported_identifier(&identifiers, &registry);
        assert_eq!(
            picked,
            Some(TypedIdentifier::new("TICKER", "AAPL")),
            "the TICKER identifier should be picked"
        );
    }

    /// When no identifier's scheme is supported, the picker returns `None`
    /// (the asset is counted as `missing_identifier`).
    #[test]
    fn pick_supported_identifier_returns_none_when_nothing_matches() {
        let registry = crate::market_data::registry::tests::registry_with_ticker_source();
        let identifiers = vec![identifier("LEI", "5493001KJTIIGC8Y1R12")];
        assert_eq!(pick_supported_identifier(&identifiers, &registry), None);
    }

    /// Builds an `AssetIdentifier` fixture.
    fn identifier(
        scheme: &str,
        value: &str,
    ) -> crate::domain::asset_identifier::entity::AssetIdentifier {
        crate::domain::asset_identifier::entity::AssetIdentifier {
            id: uuid::Uuid::nil(),
            asset_id: uuid::Uuid::nil(),
            scheme: scheme.to_string(),
            value: value.to_string(),
            created_at: chrono::Utc::now(),
        }
    }

    /// `RefreshConfig::cli_all` scans every asset and labels the audit run
    /// `"cli"` (FR-030).
    #[test]
    fn cli_all_scans_every_asset() {
        let config = RefreshConfig::cli_all();
        assert!(config.assets.is_none(), "cli_all must not pin a list");
        assert_eq!(config.trigger, "cli");
    }
}
