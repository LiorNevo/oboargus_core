//! Update an `Asset` row. Rejects patches to `id`, `created_at`,
//! `backed_by_portfolio_id`, and `issuer_id` with `DomainError::Immutable`.

use crate::domain::id::DomainId;
use sqlx::PgPool;

use crate::domain::asset::entity::{Asset, AssetPatch};
use crate::domain::asset::read;
use crate::domain::error::DomainError;
use crate::domain::id::AssetId;

/// Update the updatable fields of an asset. Implements FR-017 and US5b.
pub async fn update_asset(
    pool: &PgPool,
    id: AssetId,
    patch: AssetPatch,
) -> Result<Asset, DomainError> {
    if let Some(t) = patch.asset_type {
        let res = sqlx::query("UPDATE assets SET asset_type = $1 WHERE id = $2")
            .bind(&t)
            .bind(id.as_uuid())
            .execute(pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(DomainError::not_found("Asset", id));
        }
    }
    if let Some(n) = patch.display_name {
        let res = sqlx::query("UPDATE assets SET display_name = $1 WHERE id = $2")
            .bind(&n)
            .bind(id.as_uuid())
            .execute(pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(DomainError::not_found("Asset", id));
        }
    }
    if let Some(c) = patch.currency {
        let res = sqlx::query("UPDATE assets SET currency = $1 WHERE id = $2")
            .bind(&c)
            .bind(id.as_uuid())
            .execute(pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(DomainError::not_found("Asset", id));
        }
    }
    read::get_asset(pool, id).await
}
