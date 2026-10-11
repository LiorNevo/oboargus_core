use sqlx::PgPool;
use crate::domain::id::DomainId;

use crate::domain::asset_reference_data::entity::{AssetReferenceData, NewAssetReferenceData};
use crate::domain::error::DomainError;
use crate::domain::id::new_uuid;

/// Implements FR-018 of spec 016-implement-domain-model.
pub async fn create_asset_reference_data(
    pool: &PgPool,
    input: NewAssetReferenceData,
) -> Result<AssetReferenceData, DomainError> {
    let id = new_uuid();
    sqlx::query(
        r#"
        INSERT INTO asset_reference_data (id, asset_id, fact_type, as_of_date, knowledge_date, value, source)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(id)
    .bind(input.asset_id.as_uuid())
    .bind(&input.fact_type)
    .bind(input.as_of_date)
    .bind(input.knowledge_date)
    .bind(&input.value)
    .bind(&input.source)
    .execute(pool)
    .await?;
    super::read::get_current_asset_reference_data(
        pool,
        input.asset_id,
        &input.fact_type,
        input.as_of_date,
    )
    .await?
    .ok_or_else(|| DomainError::not_found("AssetReferenceData", id))
}
