use crate::domain::id::DomainId;
use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::IssuerId;
use crate::domain::issuer::entity::Issuer;

/// Implements FR-008 of spec 016-implement-domain-model.
pub async fn get_issuer(pool: &PgPool, id: IssuerId) -> Result<Issuer, DomainError> {
    let row: Option<Issuer> = sqlx::query_as(
        "SELECT id, name, issuer_type, legal_jurisdiction, created_at FROM issuers WHERE id = $1",
    )
    .bind(id.as_uuid())
    .fetch_optional(pool)
    .await?;
    row.ok_or_else(|| DomainError::not_found("Issuer", id))
}

/// Implements FR-008 of spec 016-implement-domain-model.
pub async fn list_issuers(pool: &PgPool) -> Result<Vec<Issuer>, DomainError> {
    let rows: Vec<Issuer> = sqlx::query_as(
        "SELECT id, name, issuer_type, legal_jurisdiction, created_at FROM issuers ORDER BY created_at ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
