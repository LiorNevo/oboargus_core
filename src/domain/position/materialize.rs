//! Incremental position materialization for the event write path.
//!
//! Events remain the source of truth; a `positions` row is a cache that is
//! updated in the same transaction as the event insert. Callers must hold the
//! transaction-scoped advisory lock for every `(portfolio_id, asset_id)` pair
//! they touch before writing.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{Executor, PgConnection, Postgres};

use crate::domain::error::DomainError;
use crate::domain::id::{AssetId, PortfolioId};

/// Acquire transaction-scoped advisory locks for position pairs.
///
/// Pairs are locked in a deterministic `(portfolio_id, asset_id)` order so
/// concurrent transfers between the same portfolios cannot deadlock. The lock
/// key is a hash: a collision only widens the lock, it never splits one.
pub(crate) async fn lock_position_pairs(
    conn: &mut PgConnection,
    pairs: &[(PortfolioId, AssetId)],
) -> Result<(), DomainError> {
    let mut keys: Vec<(uuid::Uuid, uuid::Uuid)> = pairs
        .iter()
        .map(|(portfolio, asset)| (portfolio.0, asset.0))
        .collect();
    keys.sort_unstable();
    keys.dedup();
    for (portfolio_id, asset_id) in keys {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("position:{portfolio_id}:{asset_id}"))
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// Apply one event's quantity delta to the materialized position.
///
/// The executor is the caller's transaction (or any other `Postgres`
/// executor); the caller owns the surrounding transaction. The position keeps
/// the latest knowledge time, so a backdated effective date never moves
/// `as_of` backwards.
pub(crate) async fn apply_position_delta<'e, E>(
    executor: E,
    portfolio_id: PortfolioId,
    asset_id: AssetId,
    delta: Decimal,
    recorded_at: DateTime<Utc>,
) -> Result<(), DomainError>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query(
        r#"
        INSERT INTO positions (portfolio_id, asset_id, quantity, as_of)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (portfolio_id, asset_id) DO UPDATE
        SET quantity = positions.quantity + EXCLUDED.quantity,
            as_of = GREATEST(positions.as_of, EXCLUDED.as_of)
        "#,
    )
    .bind(portfolio_id.0)
    .bind(asset_id.0)
    .bind(delta)
    .bind(recorded_at)
    .execute(executor)
    .await?;
    Ok(())
}

/// Replace a materialized position with a recomputed value.
///
/// Used by the internal rebuild path, which owns the transaction and the
/// advisory lock.
pub(crate) async fn upsert_position<'e, E>(
    executor: E,
    portfolio_id: PortfolioId,
    asset_id: AssetId,
    quantity: Decimal,
    as_of: DateTime<Utc>,
) -> Result<(), DomainError>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query(
        r#"
        INSERT INTO positions (portfolio_id, asset_id, quantity, as_of)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (portfolio_id, asset_id) DO UPDATE
        SET quantity = EXCLUDED.quantity, as_of = EXCLUDED.as_of
        "#,
    )
    .bind(portfolio_id.0)
    .bind(asset_id.0)
    .bind(quantity)
    .bind(as_of)
    .execute(executor)
    .await?;
    Ok(())
}
