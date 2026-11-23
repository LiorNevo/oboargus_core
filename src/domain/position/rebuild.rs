//! Internal rebuild/verify path for materialized positions.
//!
//! Implements FR-029 of `specs/034-production-hardening/spec.md`.
//! Positions are a cache derived from the immutable event stream. These
//! functions recompute a pair from its events and repair drift. They are
//! deliberately internal to the crate: spec 034 does not expose a portfolio
//! write API.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{Executor, PgPool, Postgres};

use crate::domain::error::DomainError;
use crate::domain::event::entity::Event;
use crate::domain::id::{AssetId, PortfolioId};
use crate::domain::position::entity::Position;
use crate::domain::position::materialize;
use crate::domain::position::replay::replay_events;

/// Recompute a position from its event stream and repair the stored row.
///
/// Runs in one transaction under the same advisory lock as the write path, so
/// a rebuild cannot race an event insert. Idempotent: replaying an unchanged
/// stream produces the same quantity.
pub async fn rebuild_position(
    pool: &PgPool,
    portfolio_id: PortfolioId,
    asset_id: AssetId,
) -> Result<Position, DomainError> {
    let mut tx = pool.begin().await?;
    materialize::lock_position_pairs(&mut tx, &[(portfolio_id, asset_id)]).await?;
    let position = replay_stored_events(&mut *tx, portfolio_id, asset_id).await?;
    materialize::upsert_position(
        &mut *tx,
        portfolio_id,
        asset_id,
        position.quantity,
        position.as_of,
    )
    .await?;
    tx.commit().await?;
    Ok(position)
}

/// Compare the stored position with a replay of its events.
///
/// Returns `true` when the stored quantity matches the replay (a missing row
/// matches a zero replay). `as_of` is metadata and is not part of the
/// comparison.
pub async fn verify_position(
    pool: &PgPool,
    portfolio_id: PortfolioId,
    asset_id: AssetId,
) -> Result<bool, DomainError> {
    let replayed = replay_stored_events(pool, portfolio_id, asset_id).await?;
    let stored: Option<(Decimal, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT quantity, as_of FROM positions
        WHERE portfolio_id = $1 AND asset_id = $2
        "#,
    )
    .bind(portfolio_id.0)
    .bind(asset_id.0)
    .fetch_optional(pool)
    .await?;

    Ok(match stored {
        Some((quantity, _)) => quantity == replayed.quantity,
        None => replayed.quantity.is_zero(),
    })
}

/// Replay every event that affects one position.
///
/// The stored event stream only records the source side of a transfer, so
/// incoming transfers are re-expressed as a destination-side event with the
/// sign inverted before replay. This is the same arithmetic the incremental
/// write path applies.
async fn replay_stored_events<'e, E>(
    executor: E,
    portfolio_id: PortfolioId,
    asset_id: AssetId,
) -> Result<Position, DomainError>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows: Vec<Event> = sqlx::query_as(
        r#"
        SELECT id, event_type, effective_date, quantity_delta, recorded_at,
               portfolio_id, asset_id, to_portfolio_id
        FROM events
        WHERE asset_id = $1 AND (portfolio_id = $2 OR to_portfolio_id = $2)
        ORDER BY effective_date ASC, recorded_at ASC, id ASC
        "#,
    )
    .bind(asset_id.0)
    .bind(portfolio_id.0)
    .fetch_all(executor)
    .await?;

    let mut events = Vec::with_capacity(rows.len());
    for row in rows {
        if row.portfolio_id != portfolio_id.0 && row.to_portfolio_id == Some(portfolio_id.0) {
            events.push(Event {
                portfolio_id: portfolio_id.0,
                to_portfolio_id: None,
                quantity_delta: -row.quantity_delta,
                ..row
            });
        } else {
            events.push(row);
        }
    }

    let mut summary = replay_events(portfolio_id, asset_id, &events)?;
    // Incremental writes keep the latest knowledge time; replay must agree.
    summary.position.as_of = events
        .iter()
        .map(|event| event.recorded_at)
        .max()
        .unwrap_or_else(Utc::now);
    Ok(summary.position)
}
