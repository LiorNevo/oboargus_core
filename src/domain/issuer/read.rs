use crate::domain::id::DomainId;
use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::IssuerId;
use crate::domain::issuer::entity::Issuer;

/// Implements FR-008 of spec 016-implement-domain-model.
pub async fn get_issuer(pool: &PgPool, id: IssuerId) -> Result<Issuer, DomainError> {
    let row: Option<Issuer> = sqlx::query_as(
        "SELECT id, name, issuer_type, legal_jurisdiction, created_at, parent_id FROM issuers WHERE id = $1",
    )
    .bind(id.as_uuid())
    .fetch_optional(pool)
    .await?;
    row.ok_or_else(|| DomainError::not_found("Issuer", id))
}

/// Implements FR-008 of spec 016-implement-domain-model.
pub async fn list_issuers(pool: &PgPool) -> Result<Vec<Issuer>, DomainError> {
    let rows: Vec<Issuer> = sqlx::query_as(
        "SELECT id, name, issuer_type, legal_jurisdiction, created_at, parent_id FROM issuers ORDER BY created_at ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Return the immediate children of `parent_id` (issuers whose `parent_id`
/// points to `parent_id`).
///
/// Returns an empty `Vec` if the issuer has no children — not an error.
/// Implements FR-002 of spec 016-implement-domain-model.
pub async fn list_children_issuers(
    pool: &PgPool,
    parent_id: IssuerId,
) -> Result<Vec<Issuer>, DomainError> {
    let rows: Vec<Issuer> = sqlx::query_as(
        "SELECT id, name, issuer_type, legal_jurisdiction, created_at, parent_id FROM issuers WHERE parent_id = $1 ORDER BY created_at ASC",
    )
    .bind(parent_id.as_uuid())
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Return the full ancestor chain of `id`, ordered from immediate parent
/// to the root (bottom-up order).
///
/// Uses a recursive CTE to walk up the `parent_id` chain. Returns an empty
/// `Vec` for a root issuer (the starting node `id` itself is NOT included
/// in the result — only its ancestors). Implements FR-002 of
/// spec 016-implement-domain-model.
pub async fn list_ancestor_issuers(
    pool: &PgPool,
    id: IssuerId,
) -> Result<Vec<Issuer>, DomainError> {
    let rows: Vec<Issuer> = sqlx::query_as(
        r#"
        WITH RECURSIVE ancestors AS (
            -- Start from the immediate parent of id. We do NOT include `id`
            -- itself in the result — only its ancestors. For a root issuer
            -- (no parent), this base case selects no rows, and the result
            -- is an empty Vec as the spec requires.
            SELECT id, name, issuer_type, legal_jurisdiction, created_at, parent_id, 1 AS depth
            FROM issuers
            WHERE id = (SELECT parent_id FROM issuers WHERE id = $1)

            UNION ALL

            -- Walk further up the chain
            SELECT e.id, e.name, e.issuer_type, e.legal_jurisdiction, e.created_at, e.parent_id, a.depth + 1
            FROM issuers e
            JOIN ancestors a ON e.id = a.parent_id
            WHERE a.parent_id IS NOT NULL
        )
        SELECT id, name, issuer_type, legal_jurisdiction, created_at, parent_id
        FROM ancestors
        ORDER BY depth ASC
        "#,
    )
    .bind(id.as_uuid())
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
