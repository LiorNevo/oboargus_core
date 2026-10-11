//! Create a `Portfolio` row, enforcing the DAG-acylclicity invariant.

use crate::domain::id::DomainId;
use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::{PortfolioId, new_uuid};
use crate::domain::portfolio::{dag, entity::NewPortfolio, entity::Portfolio};

/// Create a new portfolio. Enforces the portfolio-graph-is-a-DAG invariant
/// (no self-loop, no cycle) before insertion. Implements FR-002 and US1.
pub async fn create_portfolio(
    pool: &PgPool,
    input: NewPortfolio,
) -> Result<Portfolio, DomainError> {
    if let Some(parent_id) = input.parent_id {
        let child_id = PortfolioId::from_uuid(new_uuid());
        if dag::is_self_parent(child_id, parent_id) {
            return Err(DomainError::invariant(
                "portfolio_dag_acyclicity",
                format!("portfolio cannot be its own parent: {}", child_id),
            ));
        }
        dag::detect_cycle(pool, child_id, parent_id).await?;
    }

    let new_id = new_uuid();
    sqlx::query(
        r#"
        INSERT INTO portfolios (id, name, purpose, created_by, parent_id)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(new_id)
    .bind(&input.name)
    .bind(&input.purpose)
    .bind(&input.created_by)
    .bind(input.parent_id.map(|p| p.as_uuid()))
    .execute(pool)
    .await?;

    super::read::get_portfolio(pool, PortfolioId::from_uuid(new_id)).await
}
