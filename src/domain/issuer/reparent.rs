//! Reparent an issuer — move it from one parent to another or to root.
//!
//! Implements the `reparent_issuer` operation for US2b. Reparenting to
//! `None` makes the issuer a root.

use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::DomainId;
use crate::domain::id::IssuerId;
use crate::domain::issuer::dag;
use crate::domain::issuer::read;

/// Reparent `id` to `new_parent_id`.
///
/// - `new_parent_id = None`: makes `id` a root issuer (clears `parent_id`).
/// - `new_parent_id = Some(pid)`: links `id` as a child of `pid`.
///
/// Rejects the operation with `InvariantViolation` if:
///   - `new_parent_id == id` (self-loop), or
///   - `new_parent_id` is already a descendant of `id` (would close a cycle).
///
/// Implements FR-002 of spec 016-implement-domain-model.
pub async fn reparent_issuer(
    pool: &PgPool,
    id: IssuerId,
    new_parent_id: Option<IssuerId>,
) -> Result<crate::domain::issuer::entity::Issuer, DomainError> {
    if let Some(parent) = new_parent_id {
        // Self-loop guard
        if dag::is_self_parent(id, parent) {
            return Err(DomainError::invariant(
                "issuer_dag_acyclicity",
                format!("issuer {} cannot be its own parent", id),
            ));
        }
        // Cycle guard: check whether new_parent is already a descendant of id
        dag::detect_cycle(pool, id, parent).await?;
    }

    let res = sqlx::query("UPDATE issuers SET parent_id = $1 WHERE id = $2")
        .bind(new_parent_id.map(|p| p.as_uuid()))
        .bind(id.as_uuid())
        .execute(pool)
        .await?;

    if res.rows_affected() == 0 {
        return Err(DomainError::not_found("Issuer", id));
    }

    read::get_issuer(pool, id).await
}
