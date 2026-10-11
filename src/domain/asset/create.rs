//! Create an `Asset` row.

use sqlx::PgPool;
use crate::domain::id::DomainId;

use crate::domain::asset::entity::{Asset, NewAsset};
use crate::domain::asset::no_self_investment;
use crate::domain::error::DomainError;
use crate::domain::id::{new_uuid, AssetId};

/// Create a new asset. Enforces the no-self-investment invariant before
/// insertion.
/// Implements FR-002 of spec 016-implement-domain-model.
pub async fn create_asset(pool: &PgPool, input: NewAsset) -> Result<Asset, DomainError> {
    if let Some(backed_by) = input.backed_by_portfolio_id {
        let new_id = AssetId::from_uuid(new_uuid());
        no_self_investment::check_no_self_investment(pool, new_id, backed_by).await?;
    }

    let new_id = new_uuid();
    sqlx::query(
        r#"
        INSERT INTO assets (id, asset_type, display_name, currency, backed_by_portfolio_id, issuer_id)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(new_id)
    .bind(&input.asset_type)
    .bind(&input.display_name)
    .bind(&input.currency)
    .bind(input.backed_by_portfolio_id.map(|p| p.as_uuid()))
    .bind(input.issuer_id.map(|i| i.as_uuid()))
    .execute(pool)
    .await?;

    super::read::get_asset(pool, AssetId::from_uuid(new_id)).await
}
