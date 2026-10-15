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
/// This walks UP from `candidate_child_id`'s current parent chain and
/// checks whether `candidate_parent_id` is already reachable as an
/// ancestor. If so, the link would close a cycle.
///
/// Used by both `create_issuer` and `reparent_issuer`:
/// - `create_issuer(new_id, Some(parent_id))`: `candidate_child_id = new_id`
///   (which doesn't exist yet, so the walk yields no rows → always `Ok`)
///   and `candidate_parent_id = parent_id`. The self-loop guard
///   (`is_self_parent`) is the only real check for creation.
/// - `reparent_issuer(id, Some(new_parent))`: we need to check whether
///   `id` is an ancestor of `new_parent` (because the reparent sets
///   `id.parent = new_parent`, and if `id` is already an ancestor of
///   `new_parent` the new edge `new_parent → id` closes the loop).
///   Call as `detect_cycle(candidate_child = new_parent,
///   candidate_parent = id)` so the walk starts at `new_parent` and
///   looks for `id` in its ancestor chain.
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
    let found: Option<(i32,)> = sqlx::query_as(
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
