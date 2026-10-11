//! Delete an `Asset` row. Rejects deletion if the asset has reference data.

use crate::domain::id::DomainId;
use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::AssetId;

/// Delete an asset. Rejects if the asset has any `AssetReferenceData`
/// records.
/// Implements FR-019 of spec 016-implement-domain-model.
pub async fn delete_asset(pool: &PgPool, id: AssetId) -> Result<(), DomainError> {
    let (ref_count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM asset_reference_data WHERE asset_id = $1")
            .bind(id.as_uuid())
            .fetch_one(pool)
            .await?;
    if ref_count > 0 {
        return Err(DomainError::cannot_delete(
            "Asset",
            id,
            "asset has reference data; reference data is immutable",
        ));
    }

    let res = sqlx::query("DELETE FROM assets WHERE id = $1")
        .bind(id.as_uuid())
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(DomainError::not_found("Asset", id));
    }
    Ok(())
}
