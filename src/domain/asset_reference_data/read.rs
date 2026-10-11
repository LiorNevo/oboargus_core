use chrono::NaiveDate;
use crate::domain::id::DomainId;
use sqlx::PgPool;

use crate::domain::asset_reference_data::entity::AssetReferenceData;
use crate::domain::error::DomainError;
use crate::domain::id::AssetId;

/// Implements FR-006 of spec 016-implement-domain-model.
pub async fn get_current_asset_reference_data(
    pool: &PgPool,
    asset_id: AssetId,
    fact_type: &str,
    as_of_date: NaiveDate,
) -> Result<Option<AssetReferenceData>, DomainError> {
    let row: Option<AssetReferenceData> = sqlx::query_as(
        r#"
        SELECT id, asset_id, fact_type, as_of_date, knowledge_date, value, source
        FROM asset_reference_data
        WHERE asset_id = $1 AND fact_type = $2 AND as_of_date = $3
        ORDER BY knowledge_date DESC LIMIT 1
        "#,
    )
    .bind(asset_id.as_uuid())
    .bind(fact_type)
    .bind(as_of_date)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Implements FR-006 of spec 016-implement-domain-model.
pub async fn get_asset_reference_data_history(
    pool: &PgPool,
    asset_id: AssetId,
    fact_type: &str,
    as_of_date: NaiveDate,
) -> Result<Vec<AssetReferenceData>, DomainError> {
    let rows: Vec<AssetReferenceData> = sqlx::query_as(
        r#"
        SELECT id, asset_id, fact_type, as_of_date, knowledge_date, value, source
        FROM asset_reference_data
        WHERE asset_id = $1 AND fact_type = $2 AND as_of_date = $3
        ORDER BY knowledge_date DESC
        "#,
    )
    .bind(asset_id.as_uuid())
    .bind(fact_type)
    .bind(as_of_date)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
