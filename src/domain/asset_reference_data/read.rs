use crate::domain::id::DomainId;
use chrono::NaiveDate;
use sqlx::PgPool;

use crate::domain::asset_reference_data::entity::AssetReferenceData;
use crate::domain::error::DomainError;
use crate::domain::id::AssetId;

/// The value in force for `(asset, fact_type, as_of_date)` under the latest
/// knowledge, or `None` when the latest belief is a retraction.
///
/// Implements FR-006 of spec 016-implement-domain-model and FR-005/FR-006 of
/// spec 021-bitemporal-market-data.
pub async fn get_current_asset_reference_data(
    pool: &PgPool,
    asset_id: AssetId,
    fact_type: &str,
    as_of_date: NaiveDate,
) -> Result<Option<AssetReferenceData>, DomainError> {
    let sql = r#"
        SELECT id, asset_id, fact_type, as_of_date, knowledge_date, value, source
        FROM asset_reference_data
        WHERE asset_id = $1 AND fact_type = $2 AND as_of_date = $3
          AND NOT retracted
        ORDER BY known_at DESC, id DESC LIMIT 1
        "#;
    let row: Option<AssetReferenceData> = sqlx::query_as(sql)
        .bind(asset_id.as_uuid())
        .bind(fact_type)
        .bind(as_of_date)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// Every recorded belief for `(asset, fact_type, as_of_date)`, newest
/// knowledge first.
///
/// Implements FR-006 of spec 016-implement-domain-model and FR-007 of
/// spec 021-bitemporal-market-data.
pub async fn get_asset_reference_data_history(
    pool: &PgPool,
    asset_id: AssetId,
    fact_type: &str,
    as_of_date: NaiveDate,
) -> Result<Vec<AssetReferenceData>, DomainError> {
    let sql = r#"
        SELECT id, asset_id, fact_type, as_of_date, knowledge_date, value, source
        FROM asset_reference_data
        WHERE asset_id = $1 AND fact_type = $2 AND as_of_date = $3
        ORDER BY known_at DESC, id DESC
        "#;
    let rows: Vec<AssetReferenceData> = sqlx::query_as(sql)
        .bind(asset_id.as_uuid())
        .bind(fact_type)
        .bind(as_of_date)
        .fetch_all(pool)
        .await?;
    Ok(rows)
}
