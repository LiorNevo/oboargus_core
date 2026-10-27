//! Update the `value`/`source` of a reference-data record.
//!
//! Per Assumption 11, this appends a NEW belief with a later `known_at`; the
//! original record is preserved. Since 021 the knowledge coordinate is a
//! timestamp, so two corrections on the same calendar day no longer collide.

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::PgPool;

use crate::domain::asset_reference_data::create::create_asset_reference_data_with_known_at;
use crate::domain::asset_reference_data::entity::{AssetReferenceData, NewAssetReferenceData};
use crate::domain::error::DomainError;
use crate::domain::id::AssetId;
use crate::domain::id::DomainId;
use crate::domain::time_support::{now_micros, one_microsecond_after, utc_midnight};

/// Appends a correction and returns the new belief.
///
/// Implements FR-018 of spec 016-implement-domain-model and FR-010/FR-011 of
/// spec 021-bitemporal-market-data.
pub async fn update_asset_reference_value(
    pool: &PgPool,
    asset_id: AssetId,
    fact_type: &str,
    as_of_date: NaiveDate,
    new_value: String,
    new_source: String,
) -> Result<AssetReferenceData, DomainError> {
    let known_at = next_known_at(pool, asset_id, fact_type, as_of_date, &new_source).await?;
    create_asset_reference_data_with_known_at(
        pool,
        NewAssetReferenceData {
            asset_id,
            fact_type: fact_type.to_string(),
            as_of_date,
            // Superseded by the explicit `known_at` below; kept because the
            // input type still carries the day-granular field.
            knowledge_date: known_at.date_naive(),
            value: new_value,
            source: new_source,
        },
        known_at,
    )
    .await
}

/// The next transaction instant for this identity: now, or one microsecond
/// past the current winner when two writes land in the same microsecond.
async fn next_known_at(
    pool: &PgPool,
    asset_id: AssetId,
    fact_type: &str,
    as_of_date: NaiveDate,
    source: &str,
) -> Result<DateTime<Utc>, DomainError> {
    let current: Option<(DateTime<Utc>,)> = sqlx::query_as(
        r#"
        SELECT known_at FROM asset_reference_data
        WHERE asset_id = $1 AND fact_type = $2 AND valid_at = $3 AND source = $4
        ORDER BY known_at DESC, id DESC LIMIT 1
        "#,
    )
    .bind(asset_id.as_uuid())
    .bind(fact_type)
    .bind(utc_midnight(as_of_date))
    .bind(source)
    .fetch_optional(pool)
    .await?;

    let now = now_micros();
    Ok(match current {
        Some((winner,)) if now <= winner => one_microsecond_after(winner),
        _ => now,
    })
}
