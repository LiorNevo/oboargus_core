//! Durable enable/disable control for already-configured sources.
//!
//! The persisted `data_sources.enabled` column is the authority; the runtime
//! registry is updated only after the transaction commits.
//!
//! Implements FR-032..FR-035 of `specs/022-market-data-load-ui/spec.md`.

#![forbid(unsafe_code)]

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::market_data::error::MarketDataProviderError;
use crate::market_data::registry::{DataSourceRegistry, SourceCatalogEntry};

/// One appended source-state change.
#[derive(Debug, Clone, PartialEq, Eq, FromRow, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// Implements FR-026 / FR-028..FR-031 of spec 022.
pub struct SourceStateChange {
    /// Audit row id.
    pub id: Uuid,
    /// Stable source id.
    pub source_id: String,
    /// Server time of the change.
    pub changed_at: DateTime<Utc>,
    /// State before the change.
    pub before_enabled: bool,
    /// State after the change.
    pub after_enabled: bool,
    /// Actor, when an identity is available.
    pub actor: Option<String>,
    /// Optional operator note.
    pub reason: Option<String>,
}

/// The result of a toggle request.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// Implements FR-026 / FR-028..FR-031 of spec 022.
pub struct SourceStateChangeResult {
    /// Whether the stored state actually changed.
    pub changed: bool,
    /// The source's catalog entry after the request.
    pub source: SourceCatalogEntry,
    /// The audit row, when a change was written.
    pub change: Option<SourceStateChange>,
}

/// Enables or disables one configured source.
///
/// A request for the state the source already has is an idempotent no-op:
/// no audit row is written and `changed` is `false`.
///
/// Implements FR-032..FR-035 of spec 022.
pub async fn set_source_enabled(
    pool: &sqlx::PgPool,
    registry: &DataSourceRegistry,
    source_id: &str,
    enabled: bool,
    actor: Option<&str>,
    reason: Option<&str>,
) -> Result<SourceStateChangeResult, MarketDataProviderError> {
    // Reject an id that is not in the configured catalog before touching the
    // database. This is the same 404 the page would see on a stale catalog.
    if !registry
        .catalog(None)
        .iter()
        .any(|entry| entry.source_id == source_id)
    {
        return Err(MarketDataProviderError::NotFound(format!(
            "source '{source_id}' is not configured"
        )));
    }

    let mut tx = pool.begin().await.map_err(|e| {
        MarketDataProviderError::Infrastructure(format!("source toggle begin: {e}"))
    })?;

    let current: Option<bool> =
        sqlx::query_scalar("SELECT enabled FROM data_sources WHERE source_id = $1 FOR UPDATE")
            .bind(source_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| {
                MarketDataProviderError::Infrastructure(format!("source toggle read: {e}"))
            })?;

    let current = current.ok_or_else(|| {
        MarketDataProviderError::NotFound(format!("source '{source_id}' has no persisted row"))
    })?;

    if current == enabled {
        tx.commit().await.map_err(|e| {
            MarketDataProviderError::Infrastructure(format!("source toggle commit: {e}"))
        })?;
        let source = catalog_entry(registry, source_id)?;
        return Ok(SourceStateChangeResult {
            changed: false,
            source,
            change: None,
        });
    }

    sqlx::query("UPDATE data_sources SET enabled = $2, updated_at = NOW() WHERE source_id = $1")
        .bind(source_id)
        .bind(enabled)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            MarketDataProviderError::Infrastructure(format!("source toggle update: {e}"))
        })?;

    let change: SourceStateChange = sqlx::query_as(
        "INSERT INTO data_source_state_changes
            (source_id, changed_at, before_enabled, after_enabled, actor, reason)
         VALUES ($1, NOW(), $2, $3, $4, $5)
         RETURNING id, source_id, changed_at, before_enabled, after_enabled, actor, reason",
    )
    .bind(source_id)
    .bind(current)
    .bind(enabled)
    .bind(actor)
    .bind(reason)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| MarketDataProviderError::Infrastructure(format!("source toggle audit: {e}")))?;

    tx.commit().await.map_err(|e| {
        MarketDataProviderError::Infrastructure(format!("source toggle commit: {e}"))
    })?;

    // The database is authoritative; if this in-process update ever failed,
    // the next request or restart would converge from the persisted row.
    registry.set_enabled(source_id, enabled)?;
    let source = catalog_entry(registry, source_id)?;

    Ok(SourceStateChangeResult {
        changed: true,
        source,
        change: Some(change),
    })
}

fn catalog_entry(
    registry: &DataSourceRegistry,
    source_id: &str,
) -> Result<SourceCatalogEntry, MarketDataProviderError> {
    registry
        .catalog(None)
        .into_iter()
        .find(|entry| entry.source_id == source_id)
        .ok_or_else(|| {
            MarketDataProviderError::NotFound(format!("source '{source_id}' is not configured"))
        })
}
