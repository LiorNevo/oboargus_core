//! Read `Asset` rows.

use sqlx::{PgPool, QueryBuilder};
use crate::domain::id::DomainId;

use crate::domain::asset::entity::{Asset, AssetFilter};
use crate::domain::error::DomainError;
use crate::domain::id::AssetId;

/// Fetch an asset by id.
/// Implements FR-008 of spec 016-implement-domain-model.
pub async fn get_asset(pool: &PgPool, id: AssetId) -> Result<Asset, DomainError> {
    let row: Option<Asset> = sqlx::query_as(
        r#"
        SELECT id, asset_type, display_name, currency, created_at, backed_by_portfolio_id, issuer_id
        FROM assets WHERE id = $1
        "#,
    )
    .bind(id.as_uuid())
    .fetch_optional(pool)
    .await?;
    row.ok_or_else(|| DomainError::not_found("Asset", id))
}

/// List assets, optionally filtered.
/// Implements FR-008 of spec 016-implement-domain-model.
pub async fn list_assets(pool: &PgPool, filter: AssetFilter) -> Result<Vec<Asset>, DomainError> {
    let mut qb: QueryBuilder<sqlx::Postgres> = QueryBuilder::new(
        "SELECT id, asset_type, display_name, currency, created_at, backed_by_portfolio_id, issuer_id FROM assets WHERE TRUE",
    );
    if let Some(t) = filter.asset_type {
        qb.push(" AND asset_type = ").push_bind(t);
    }
    if let Some(c) = filter.currency {
        qb.push(" AND currency = ").push_bind(c);
    }
    if let Some(i) = filter.issuer_id {
        qb.push(" AND issuer_id = ").push_bind(i.as_uuid());
    }
    qb.push(" ORDER BY created_at ASC");
    let rows: Vec<Asset> = qb.build_query_as().fetch_all(pool).await?;
    Ok(rows)
}
