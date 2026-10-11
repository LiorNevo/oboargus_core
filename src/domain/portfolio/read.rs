//! Read `Portfolio` rows.

use sqlx::PgPool;
use crate::domain::id::DomainId;

use crate::domain::error::DomainError;
use crate::domain::id::PortfolioId;
use crate::domain::portfolio::entity::Portfolio;

/// Fetch a portfolio by id. Returns `DomainError::NotFound` if no row
/// matches.
/// Implements FR-008 of spec 016-implement-domain-model.
pub async fn get_portfolio(pool: &PgPool, id: PortfolioId) -> Result<Portfolio, DomainError> {
    let row: Option<Portfolio> = sqlx::query_as(
        r#"
        SELECT id, name, purpose, created_at, created_by, parent_id
        FROM portfolios WHERE id = $1
        "#,
    )
    .bind(id.as_uuid())
    .fetch_optional(pool)
    .await?;

    row.ok_or_else(|| DomainError::not_found("Portfolio", id))
}

/// List the direct children of a portfolio. Returns an empty `Vec` if the
/// parent has no children.
/// Implements FR-008 of spec 016-implement-domain-model.
pub async fn list_children(pool: &PgPool, parent_id: PortfolioId) -> Result<Vec<Portfolio>, DomainError> {
    let rows: Vec<Portfolio> = sqlx::query_as(
        r#"
        SELECT id, name, purpose, created_at, created_by, parent_id
        FROM portfolios WHERE parent_id = $1 ORDER BY created_at ASC
        "#,
    )
    .bind(parent_id.as_uuid())
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// List the ancestors of a portfolio in root-to-leaf order. Returns an empty
/// `Vec` if the portfolio has no parent.
/// Implements FR-008 of spec 016-implement-domain-model.
pub async fn list_ancestors(pool: &PgPool, id: PortfolioId) -> Result<Vec<Portfolio>, DomainError> {
    let rows: Vec<Portfolio> = sqlx::query_as(
        r#"
        WITH RECURSIVE ancestors AS (
            SELECT id, name, purpose, created_at, created_by, parent_id
            FROM portfolios WHERE id = $1
            UNION
            SELECT p.id, p.name, p.purpose, p.created_at, p.created_by, p.parent_id
            FROM portfolios p
            INNER JOIN ancestors a ON a.parent_id = p.id
        )
        SELECT id, name, purpose, created_at, created_by, parent_id
        FROM ancestors WHERE id <> $1
        ORDER BY created_at ASC
        "#,
    )
    .bind(id.as_uuid())
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
