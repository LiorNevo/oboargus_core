use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::domain::asset_reference_data::entity::{AssetReferenceData, NewAssetReferenceData};
use crate::domain::error::DomainError;
use crate::domain::id::DomainId;
use crate::domain::id::new_uuid;
use crate::domain::time_support::utc_midnight;

/// Appends one belief about an asset, with day-granular time coordinates.
///
/// The `as_of_date` becomes `valid_at` at midnight UTC and `knowledge_date`
/// becomes `known_at` at midnight UTC. Corrections that need sub-day
/// transaction time use [`create_asset_reference_data_with_known_at`].
///
/// Implements FR-018 of spec 016-implement-domain-model and FR-001/FR-009 of
/// spec 021-bitemporal-market-data.
pub async fn create_asset_reference_data(
    pool: &PgPool,
    input: NewAssetReferenceData,
) -> Result<AssetReferenceData, DomainError> {
    let known_at = utc_midnight(input.knowledge_date);
    create_asset_reference_data_with_known_at(pool, input, known_at).await
}

/// Appends one belief about an asset with an explicit `known_at`.
///
/// Used by same-day corrections and by market-data ingestion, where the
/// transaction time is "when we recorded it", not a calendar date.
pub async fn create_asset_reference_data_with_known_at(
    pool: &PgPool,
    input: NewAssetReferenceData,
    known_at: DateTime<Utc>,
) -> Result<AssetReferenceData, DomainError> {
    if input.fact_type.trim().is_empty() {
        return Err(DomainError::Validation(
            "fact_type must not be empty".to_string(),
        ));
    }
    if input.source.trim().is_empty() {
        return Err(DomainError::Validation(
            "source must not be empty".to_string(),
        ));
    }

    let id = new_uuid();
    let valid_at = utc_midnight(input.as_of_date);
    let sql = r#"
        INSERT INTO asset_reference_data
            (id, asset_id, fact_type, valid_at, known_at, value, source)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id, asset_id, fact_type, as_of_date, knowledge_date, value, source
        "#;
    let row: AssetReferenceData = sqlx::query_as(sql)
        .bind(id)
        .bind(input.asset_id.as_uuid())
        .bind(&input.fact_type)
        .bind(valid_at)
        .bind(known_at)
        .bind(&input.value)
        .bind(&input.source)
        .fetch_one(pool)
        .await?;
    Ok(row)
}
