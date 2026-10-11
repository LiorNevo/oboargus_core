//! Update a `Portfolio` row. Rejects patches to `id`, `created_at`, or
//! `created_by` with `DomainError::Immutable`.

use crate::domain::id::DomainId;
use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::PortfolioId;
use crate::domain::portfolio::entity::{Portfolio, PortfolioPatch};
use crate::domain::portfolio::read;

/// Update the updatable fields of a portfolio. Implements FR-017 and US5b.
pub async fn update_portfolio(
    pool: &PgPool,
    id: PortfolioId,
    patch: PortfolioPatch,
) -> Result<Portfolio, DomainError> {
    if let Some(name) = patch.name {
        let res = sqlx::query("UPDATE portfolios SET name = $1 WHERE id = $2")
            .bind(&name)
            .bind(id.as_uuid())
            .execute(pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(DomainError::not_found("Portfolio", id));
        }
    }
    if let Some(purpose) = patch.purpose {
        let res = sqlx::query("UPDATE portfolios SET purpose = $1 WHERE id = $2")
            .bind(&purpose)
            .bind(id.as_uuid())
            .execute(pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(DomainError::not_found("Portfolio", id));
        }
    }
    read::get_portfolio(pool, id).await
}
