//! Materialize a `Position` by replaying its event stream and upserting the
//! result into the `positions` table.

use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::event::read as event_read;
use crate::domain::id::{AssetId, PortfolioId};
use crate::domain::position::replay;

/// Recompute and upsert the position for `(portfolio_id, asset_id)`.
/// Implements FR-008 of spec 016-implement-domain-model.
pub async fn materialize_position(
    pool: &PgPool,
    portfolio_id: PortfolioId,
    asset_id: AssetId,
) -> Result<(), DomainError> {
    let events = event_read::list_events_for_position(pool, portfolio_id, asset_id).await?;
    let summary = replay::replay_events(portfolio_id, asset_id, &events)?;
    sqlx::query(
        r#"
        INSERT INTO positions (portfolio_id, asset_id, quantity, as_of)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (portfolio_id, asset_id) DO UPDATE
        SET quantity = EXCLUDED.quantity, as_of = EXCLUDED.as_of
        "#,
    )
    .bind(summary.position.portfolio_id)
    .bind(summary.position.asset_id)
    .bind(summary.position.quantity)
    .bind(summary.position.as_of)
    .execute(pool)
    .await?;
    Ok(())
}
