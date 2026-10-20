//! Persistence for the `data_sources` configuration table.
//!
//! Implements FR-002 / FR-040 of `specs/017-yahoo-data-source/spec.md`.
//! The table records the *shape* of each configured source — never the
//! credential value. `api_key_present` is a boolean; the secret itself is
//! never written, never logged, and never returned by any query.

use sqlx::FromRow;
use uuid::Uuid;

use crate::market_data::config::DataSourceConfig;
use crate::market_data::error::MarketDataProviderError;

/// One row in `data_sources`.
///
/// Implements FR-002 / FR-040 of spec 017.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct DataSourceRow {
    /// Row id.
    pub id: Uuid,
    /// Stable source id.
    pub source_id: String,
    /// Human-readable label.
    pub display_name: String,
    /// Provider discriminator.
    pub provider_type: String,
    /// Whether the source is enabled.
    pub enabled: bool,
    /// Numeric priority (lower = tried first).
    pub priority: i32,
    /// Identifier schemes the source can answer.
    pub supported_schemes: Vec<String>,
    /// Per-source rate-limit budget (requests per minute), if any.
    pub rate_limit_per_minute: Option<i32>,
    /// Whether a credential was present at startup. Never the value.
    pub api_key_present: bool,
    /// Per-source `User-Agent` override, if any.
    pub user_agent: Option<String>,
    /// Per-source upstream timeout, if any.
    pub timeout_seconds: Option<i32>,
}

/// Upserts one configured source into `data_sources`.
///
/// The `source_id` is the natural key: re-running with the same source
/// updates the row in place rather than inserting a duplicate.
///
/// Implements FR-002 of spec 017.
pub async fn upsert_data_source(
    pool: &sqlx::PgPool,
    config: &DataSourceConfig,
    api_key_present: bool,
) -> Result<DataSourceRow, MarketDataProviderError> {
    let id = Uuid::new_v4();
    let row: DataSourceRow = sqlx::query_as(
        "INSERT INTO data_sources
            (id, source_id, display_name, provider_type, enabled, priority, supported_schemes,
             rate_limit_per_minute, api_key_present, user_agent, timeout_seconds, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NOW(), NOW())
         ON CONFLICT (source_id) DO UPDATE SET
            display_name = EXCLUDED.display_name,
            provider_type = EXCLUDED.provider_type,
            enabled = EXCLUDED.enabled,
            priority = EXCLUDED.priority,
            supported_schemes = EXCLUDED.supported_schemes,
            rate_limit_per_minute = EXCLUDED.rate_limit_per_minute,
            api_key_present = EXCLUDED.api_key_present,
            user_agent = EXCLUDED.user_agent,
            timeout_seconds = EXCLUDED.timeout_seconds,
            updated_at = NOW()
         RETURNING id, source_id, display_name, provider_type, enabled, priority,
                   supported_schemes, rate_limit_per_minute, api_key_present, user_agent,
                   timeout_seconds",
    )
    .bind(id)
    .bind(&config.source_id)
    .bind(&config.display_name)
    .bind(&config.provider_type)
    .bind(config.enabled)
    .bind(config.priority)
    .bind(&config.supported_schemes)
    .bind(config.rate_limit_per_minute.map(|v| v as i32))
    .bind(api_key_present)
    .bind(&config.user_agent)
    .bind(config.timeout_seconds.map(|t| t as i32))
    .fetch_one(pool)
    .await
    .map_err(|e| MarketDataProviderError::Infrastructure(format!("upsert_data_source: {e}")))?;

    Ok(row)
}

/// Reads every configured source, ordered by `source_id ASC`.
///
/// Implements FR-002 of spec 017.
pub async fn list_data_sources(
    pool: &sqlx::PgPool,
) -> Result<Vec<DataSourceRow>, MarketDataProviderError> {
    let rows = sqlx::query_as(
        "SELECT id, source_id, display_name, provider_type, enabled, priority,
                supported_schemes, rate_limit_per_minute, api_key_present, user_agent,
                timeout_seconds
         FROM data_sources ORDER BY source_id ASC",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| MarketDataProviderError::Infrastructure(format!("list_data_sources: {e}")))?;

    Ok(rows)
}
