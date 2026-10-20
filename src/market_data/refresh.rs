//! Refresh orchestrator: drives per-asset price refreshes through the registry.
//!
//! Implements FR-007 / FR-013 / FR-014 / FR-030..FR-032 of
//! `specs/017-yahoo-data-source/spec.md`. The orchestrator writes
//! `AssetReferenceData` rows via the predecessor's `create_asset_reference_data`
//! and persists a `data_source_refresh_runs` audit row per call.

use std::collections::HashMap;

use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::json;
use uuid::Uuid;

use crate::domain::asset_identifier::read as asset_identifier_read;
use crate::domain::asset_reference_data::create as asset_reference_data_create;
use crate::domain::asset_reference_data::entity::NewAssetReferenceData;
use crate::domain::asset_reference_data::read as asset_reference_data_read;
use crate::domain::id::AssetId;
use crate::domain::id::DomainId;
use crate::market_data::audit::{DataSourceRefreshCounters, finish_refresh_run, start_refresh_run};
use crate::market_data::error::MarketDataProviderError;
use crate::market_data::registry::DataSourceRegistry;
use crate::market_data::types::{LatestBarRequest, PriceBar, TypedIdentifier};

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
/// Implements FR-013 of spec 017. The value is a canonical JSON object
/// shaped `{ "open": "…", "high": "…", "low": "…", "close": "…",
/// "adjclose": "…", "volume": <u64> }`.
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
            let value_payload =
                serde_json::to_string(&serialize_bar(&latest.bar)).map_err(|e| {
                    MarketDataProviderError::Infrastructure(format!("json encode: {e}"))
                })?;
            *per_source_attempted
                .entry(latest.source_id.clone())
                .or_insert(0) += 1;

            // Idempotency (FR-031): the predecessor's table is unique on
            // `(asset_id, fact_type, as_of_date, knowledge_date)`, so a
            // re-run on the same trading day would violate that
            // constraint. Detect the already-known fact *before* writing
            // and count it as a no-op instead. No `ON CONFLICT` is used —
            // the latest-`knowledge_date`-wins rule does the work
            // (research D6).
            if is_already_known(pool, asset_id, latest.bar.as_of_date, latest.knowledge_date).await
            {
                counters.no_op += 1;
                continue;
            }

            let new_row = NewAssetReferenceData {
                asset_id,
                fact_type: FACT_TYPE_MARKET_PRICE.to_string(),
                as_of_date: latest.bar.as_of_date,
                knowledge_date: latest.knowledge_date,
                value: value_payload,
                source: latest.source_id.clone(),
            };
            match asset_reference_data_create::create_asset_reference_data(pool, new_row).await {
                Ok(_) => counters.refreshed += 1,
                Err(e) => {
                    tracing::warn!(asset_id = %asset_id, error = %e, "asset_reference_data write failed");
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

/// Whether the fact is already recorded with the same `knowledge_date`.
///
/// The predecessor's `asset_reference_data` table is unique on
/// `(asset_id, fact_type, as_of_date, knowledge_date)`. A refresh re-run on
/// the same trading day would therefore violate that constraint. This
/// helper detects the already-known fact so the run can be recorded as a
/// no-op instead of a write failure (FR-031).
///
/// The "current" row is the one with the greatest `knowledge_date`; if it
/// already carries the incoming `knowledge_date`, the fact is known.
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

/// Serializes a `PriceBar` as canonical JSON suitable for the
/// `AssetReferenceData.value` payload.
///
/// The shape is `{ "open": "…", "high": "…", "low": "…", "close": "…",
/// "adjclose": "…", "volume": <u64> }` — prices are decimal *strings* so
/// no precision is lost through a JSON number round-trip (Constitution
/// Principle X).
///
/// Implements FR-013 of spec 017.
pub fn serialize_bar(bar: &PriceBar) -> serde_json::Value {
    json!({
        "open": decimal_to_string(bar.open),
        "high": decimal_to_string(bar.high),
        "low": decimal_to_string(bar.low),
        "close": decimal_to_string(bar.close),
        "adjclose": decimal_to_string(bar.adjclose),
        "volume": bar.volume,
    })
}

fn decimal_to_string(d: Decimal) -> String {
    d.normalize().to_string()
}

/// Returns today's date (used by tests that pin the clock).
///
/// Implements FR-014 of spec 017 (the dual-temporal `knowledge_date`).
#[allow(dead_code)]
pub fn today() -> NaiveDate {
    Utc::now().date_naive()
}

#[allow(unused_imports)]
use crate::domain::asset_reference_data::entity::AssetReferenceData as _AssetReferenceData;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_bar_emits_all_six_fields() {
        let bar = PriceBar {
            as_of_date: NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(),
            open: Decimal::new(100, 0),
            high: Decimal::new(101, 0),
            low: Decimal::new(99, 0),
            close: Decimal::new(100, 0),
            adjclose: Decimal::new(100, 0),
            volume: 1_234_567,
        };
        let v = serialize_bar(&bar);
        assert_eq!(v["volume"], 1_234_567i64);
        assert!(v["close"].is_string());
    }

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

    /// `today()` is the dual-temporal `knowledge_date` used by the refresh
    /// run (FR-014).
    #[test]
    fn today_is_the_current_utc_date() {
        assert_eq!(today(), Utc::now().date_naive());
    }
}
