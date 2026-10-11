//! Read materialized `Position` rows.

use crate::domain::id::DomainId;
use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::{AssetId, PortfolioId};
use crate::domain::position::entity::Position;

/// Implements FR-008 of spec 016-implement-domain-model.
pub async fn get_position(
    pool: &PgPool,
    portfolio_id: PortfolioId,
    asset_id: AssetId,
) -> Result<Position, DomainError> {
    let row: Option<Position> = sqlx::query_as(
        "SELECT portfolio_id, asset_id, quantity, as_of FROM positions WHERE portfolio_id = $1 AND asset_id = $2",
    )
    .bind(portfolio_id.as_uuid())
    .bind(asset_id.as_uuid())
    .fetch_optional(pool)
    .await?;
    row.ok_or_else(|| DomainError::not_found("Position", format!("{}/{}", portfolio_id, asset_id)))
}

/// Implements FR-008 of spec 016-implement-domain-model.
pub async fn list_positions_for_portfolio(
    pool: &PgPool,
    portfolio_id: PortfolioId,
) -> Result<Vec<Position>, DomainError> {
    let rows: Vec<Position> = sqlx::query_as(
        "SELECT portfolio_id, asset_id, quantity, as_of FROM positions WHERE portfolio_id = $1 ORDER BY as_of DESC",
    )
    .bind(portfolio_id.as_uuid())
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Implements FR-008 of spec 016-implement-domain-model.
pub async fn list_positions_for_asset(
    pool: &PgPool,
    asset_id: AssetId,
) -> Result<Vec<Position>, DomainError> {
    let rows: Vec<Position> = sqlx::query_as(
        "SELECT portfolio_id, asset_id, quantity, as_of FROM positions WHERE asset_id = $1 ORDER BY as_of DESC",
    )
    .bind(asset_id.as_uuid())
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
