use crate::domain::id::DomainId;
use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::event::entity::Event;
use crate::domain::id::{AssetId, EventId, PortfolioId};

/// Implements FR-008 of spec 016-implement-domain-model.
pub async fn get_event(pool: &PgPool, id: EventId) -> Result<Event, DomainError> {
    let row: Option<Event> = sqlx::query_as(
        r#"
        SELECT id, event_type, effective_date, quantity_delta, recorded_at, portfolio_id, asset_id, to_portfolio_id
        FROM events WHERE id = $1
        "#,
    )
    .bind(id.as_uuid())
    .fetch_optional(pool)
    .await?;
    row.ok_or_else(|| DomainError::not_found("Event", id))
}

/// Implements FR-008 of spec 016-implement-domain-model.
pub async fn list_events_for_position(
    pool: &PgPool,
    portfolio_id: PortfolioId,
    asset_id: AssetId,
) -> Result<Vec<Event>, DomainError> {
    let rows: Vec<Event> = sqlx::query_as(
        r#"
        SELECT id, event_type, effective_date, quantity_delta, recorded_at, portfolio_id, asset_id, to_portfolio_id
        FROM events
        WHERE portfolio_id = $1 AND asset_id = $2
        ORDER BY effective_date ASC, recorded_at ASC, id ASC
        "#,
    )
    .bind(portfolio_id.as_uuid())
    .bind(asset_id.as_uuid())
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
