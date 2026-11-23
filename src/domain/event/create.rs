use sqlx::{PgPool, Postgres, Transaction};

use crate::domain::error::DomainError;
use crate::domain::event::entity::{Event, EventType, NewEvent};
use crate::domain::id::{AssetId, PortfolioId, new_uuid};
use crate::domain::position::materialize;

fn validate(input: &NewEvent) -> Result<(), DomainError> {
    if input.quantity_delta.is_zero() {
        return Err(DomainError::Validation(
            "quantity_delta must be non-zero".to_string(),
        ));
    }
    if input.event_type == EventType::Transfer && input.to_portfolio_id.is_none() {
        return Err(DomainError::Validation(
            "transfer event requires to_portfolio_id".to_string(),
        ));
    }
    if input.event_type == EventType::Transfer && input.to_portfolio_id == Some(input.portfolio_id)
    {
        return Err(DomainError::Validation(
            "transfer destination must differ from source".to_string(),
        ));
    }
    if input.event_type != EventType::Transfer && input.to_portfolio_id.is_some() {
        return Err(DomainError::Validation(
            "only transfer events may have to_portfolio_id".to_string(),
        ));
    }
    if matches!(
        input.event_type,
        EventType::Merger
            | EventType::SpinOff
            | EventType::Dividend
            | EventType::StockBasedCompensation
    ) {
        // Replay cannot value these yet; reject before any row is written.
        return Err(DomainError::UnsupportedEventType {
            event_type: input.event_type.to_string(),
        });
    }
    Ok(())
}

/// Persist an event and update the affected position(s) in one transaction.
///
/// Implements FR-028 of `specs/034-production-hardening/spec.md`.
/// The advisory locks serialize concurrent writers for the same
/// `(portfolio_id, asset_id)` pair. A transfer locks both the source and the
/// destination pair in deterministic order. Any failure rolls back both the
/// event and the position update.
pub async fn create_event(pool: &PgPool, input: NewEvent) -> Result<Event, DomainError> {
    validate(&input)?;

    let mut tx = pool.begin().await?;
    let event = match persist(&mut tx, &input).await {
        Ok(event) => event,
        Err(error) => {
            let _ = tx.rollback().await;
            return Err(error);
        }
    };
    tx.commit().await?;
    Ok(event)
}

async fn persist(
    tx: &mut Transaction<'_, Postgres>,
    input: &NewEvent,
) -> Result<Event, DomainError> {
    let mut pairs: Vec<(PortfolioId, AssetId)> = vec![(input.portfolio_id, input.asset_id)];
    if let Some(destination) = input.to_portfolio_id {
        pairs.push((destination, input.asset_id));
    }
    materialize::lock_position_pairs(tx, &pairs).await?;

    let event: Event = sqlx::query_as(
        r#"
        INSERT INTO events (id, event_type, effective_date, quantity_delta, portfolio_id, asset_id, to_portfolio_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id, event_type, effective_date, quantity_delta, recorded_at, portfolio_id, asset_id, to_portfolio_id
        "#,
    )
    .bind(new_uuid())
    .bind(input.event_type.as_str())
    .bind(input.effective_date)
    .bind(input.quantity_delta)
    .bind(input.portfolio_id.0)
    .bind(input.asset_id.0)
    .bind(input.to_portfolio_id.map(|portfolio| portfolio.0))
    .fetch_one(&mut **tx)
    .await?;

    materialize::apply_position_delta(
        &mut **tx,
        input.portfolio_id,
        input.asset_id,
        input.quantity_delta,
        event.recorded_at,
    )
    .await?;

    if let Some(destination) = input.to_portfolio_id {
        // The source delta is the signed quantity leaving the source, so the
        // destination receives its negation.
        materialize::apply_position_delta(
            &mut **tx,
            destination,
            input.asset_id,
            -input.quantity_delta,
            event.recorded_at,
        )
        .await?;
    }

    Ok(event)
}
