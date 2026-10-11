//! DAG cycle detection for the portfolio self-referential edge.

use sqlx::PgPool;
use crate::domain::id::DomainId;

use crate::domain::error::DomainError;
use crate::domain::id::PortfolioId;

/// Returns true if the given child and parent ids are the same (a self-loop).
/// Implements FR-002 of spec 016-implement-domain-model.
pub fn is_self_parent(child_id: PortfolioId, parent_id: PortfolioId) -> bool {
    child_id == parent_id
}

/// Detect whether linking `candidate_parent_id` as a parent of
/// `candidate_child_id` would create a cycle in the portfolio graph.
///
/// A cycle exists iff `candidate_parent_id` is (transitively) a descendant
/// of `candidate_child_id`. The function uses a recursive CTE.
/// Implements FR-002 of spec 016-implement-domain-model.
pub async fn detect_cycle(
    pool: &PgPool,
    candidate_child_id: PortfolioId,
    candidate_parent_id: PortfolioId,
) -> Result<(), DomainError> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        WITH RECURSIVE descendants AS (
            SELECT id, parent_id FROM portfolios WHERE id = $1
            UNION
            SELECT p.id, p.parent_id
            FROM portfolios p
            INNER JOIN descendants d ON p.parent_id = d.id
        )
        SELECT 1 FROM descendants WHERE id = $2 LIMIT 1
        "#,
    )
    .bind(candidate_child_id.as_uuid())
    .bind(candidate_parent_id.as_uuid())
    .fetch_optional(pool)
    .await?;

    if row.is_some() {
        return Err(DomainError::invariant(
            "portfolio_dag_acyclicity",
            format!(
                "linking {} as parent of {} would create a cycle",
                candidate_parent_id, candidate_child_id
            ),
        ));
    }
    Ok(())
}
