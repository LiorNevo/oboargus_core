use sqlx::PgPool;
use crate::domain::id::DomainId;

use crate::domain::error::DomainError;
use crate::domain::event::entity::{Event, NewEvent};
use crate::domain::id::new_uuid;
use crate::domain::position::materialize;

/// Implements FR-008 of spec 016-implement-domain-model.
pub async fn create_event(pool: &PgPool, input: NewEvent) -> Result<Event, DomainError> {
    if input.quantity_delta.is_zero() {
        return Err(DomainError::Validation(
            "quantity_delta must be non-zero".to_string(),
        ));
    }
    if input.event_type == crate::domain::event::entity::EventType::Transfer
        && input.to_portfolio_id.is_none()
    {
        return Err(DomainError::Validation(
            "transfer event requires to_portfolio_id".to_string(),
        ));
    }
    if input.event_type == crate::domain::event::entity::EventType::Transfer
        && input.to_portfolio_id == Some(input.portfolio_id)
    {
        return Err(DomainError::Validation(
            "transfer destination must differ from source".to_string(),
        ));
    }

    let id = new_uuid();
    sqlx::query(
        r#"
        INSERT INTO events (id, event_type, effective_date, quantity_delta, portfolio_id, asset_id, to_portfolio_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(id)
    .bind(input.event_type.as_str())
    .bind(input.effective_date)
    .bind(input.quantity_delta)
    .bind(input.portfolio_id.as_uuid())
    .bind(input.asset_id.as_uuid())
    .bind(input.to_portfolio_id.map(|p| p.as_uuid()))
    .execute(pool)
    .await?;

    // Materialize the source position (and the destination for a transfer).
    materialize::materialize_position(pool, input.portfolio_id, input.asset_id).await?;
    if let Some(to) = input.to_portfolio_id {
        materialize::materialize_position(pool, to, input.asset_id).await?;
    }

    super::read::get_event(pool, crate::domain::id::EventId::from_uuid(id)).await
}
