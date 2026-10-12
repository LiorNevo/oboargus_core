//! Cycle-detection helpers for the issuer DAG.
//!
//! Implements the write-time enforcement of Invariant 12
//! ("issuer graph is a DAG") for the `create_issuer` and `reparent_issuer`
//! operations. The database layer (the `CHECK (id <> parent_id)` constraint)
//! provides defence-in-depth; the application layer is the primary enforcer.

use sqlx::PgPool;

use crate::domain::id::DomainId;

use crate::domain::error::DomainError;
use crate::domain::id::IssuerId;

/// Return `true` if `parent_id == child_id` (trivial self-loop).
///
/// This is a cheap O(1) check before hitting the database.
/// Implements FR-002 (write-time DAG enforcement) of spec 016-implement-domain-model.
pub fn is_self_parent(child_id: IssuerId, parent_id: IssuerId) -> bool {
    child_id == parent_id
}

/// Check whether linking `candidate_child_id` as a child of
/// `candidate_parent_id` would create a cycle in the issuer graph.
///
/// Uses a recursive CTE to walk up from `candidate_child_id`'s ancestors
/// and checks whether `candidate_parent_id` is already reachable. If so,
/// the link would close a cycle and is rejected.
///
/// Returns `Ok(())` if the link is safe; `Err(DomainError)` if it would
/// create a cycle.
/// Implements FR-002 (write-time DAG enforcement) of spec 016-implement-domain-model.
pub async fn detect_cycle(
    pool: &PgPool,
    candidate_child_id: IssuerId,
    candidate_parent_id: IssuerId,
) -> Result<(), DomainError> {
    // Recursive CTE: start from candidate_child_id and walk up parent links.
    // If we ever reach candidate_parent_id, a cycle would result.
    let found: Option<(i64,)> = sqlx::query_as(
        r#"
        WITH RECURSIVE ancestors AS (
            -- Base case: start from candidate_child's current parent
            SELECT id, parent_id
            FROM issuers
            WHERE id = $1

            UNION ALL

            -- Recursive step: walk up the parent chain
            SELECT e.id, e.parent_id
            FROM issuers e
            JOIN ancestors a ON e.id = a.parent_id
            WHERE a.parent_id IS NOT NULL
        )
        SELECT 1 AS found
        FROM ancestors
        WHERE id = $2
        LIMIT 1
        "#,
    )
    .bind(candidate_child_id.as_uuid())
    .bind(candidate_parent_id.as_uuid())
    .fetch_optional(pool)
    .await?;

    if found.is_some() {
        return Err(DomainError::invariant(
            "issuer_dag_acyclicity",
            format!(
                "linking {} as child of {} would create a cycle",
                candidate_child_id, candidate_parent_id
            ),
        ));
    }
    Ok(())
}
