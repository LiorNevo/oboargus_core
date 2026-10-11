//! Delete a `Portfolio` row. Rejects deletion of a portfolio that has
//! children, events, or positions. Implements FR-019 and US5b.

use sqlx::PgPool;
use crate::domain::id::DomainId;

use crate::domain::error::DomainError;
use crate::domain::id::PortfolioId;

/// Delete a leaf portfolio (no children, no events, no positions). Returns
/// `DomainError::CannotDelete` with a structured reason if the portfolio
/// cannot be deleted.
/// Implements FR-019 of spec 016-implement-domain-model.
pub async fn delete_portfolio(pool: &PgPool, id: PortfolioId) -> Result<(), DomainError> {
    let (child_count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM portfolios WHERE parent_id = $1")
            .bind(id.as_uuid())
            .fetch_one(pool)
            .await?;
    if child_count > 0 {
        return Err(DomainError::cannot_delete(
            "Portfolio",
            id,
            format!("portfolio has {} children, retire them first", child_count),
        ));
    }

    let (event_count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM events WHERE portfolio_id = $1 OR to_portfolio_id = $1")
            .bind(id.as_uuid())
            .fetch_one(pool)
            .await?;
    if event_count > 0 {
        return Err(DomainError::cannot_delete(
            "Portfolio",
            id,
            "portfolio has events; events are immutable history",
        ));
    }

    let (position_count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM positions WHERE portfolio_id = $1")
            .bind(id.as_uuid())
            .fetch_one(pool)
            .await?;
    if position_count > 0 {
        return Err(DomainError::cannot_delete(
            "Portfolio",
            id,
            "portfolio has positions",
        ));
    }

    let res = sqlx::query("DELETE FROM portfolios WHERE id = $1")
        .bind(id.as_uuid())
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(DomainError::not_found("Portfolio", id));
    }
    Ok(())
}
