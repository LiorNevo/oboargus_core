use sqlx::PgPool;
use crate::domain::id::DomainId;

use crate::domain::error::DomainError;
use crate::domain::id::IssuerId;
use crate::domain::issuer::entity::{Issuer, IssuerPatch};
use crate::domain::issuer::read;

/// Implements FR-017 of spec 016-implement-domain-model.
pub async fn update_issuer(
    pool: &PgPool,
    id: IssuerId,
    patch: IssuerPatch,
) -> Result<Issuer, DomainError> {
    if let Some(n) = patch.name {
        let res = sqlx::query("UPDATE issuers SET name = $1 WHERE id = $2")
            .bind(&n)
            .bind(id.as_uuid())
            .execute(pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(DomainError::not_found("Issuer", id));
        }
    }
    if let Some(t) = patch.issuer_type {
        let res = sqlx::query("UPDATE issuers SET issuer_type = $1 WHERE id = $2")
            .bind(&t)
            .bind(id.as_uuid())
            .execute(pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(DomainError::not_found("Issuer", id));
        }
    }
    if let Some(j) = patch.legal_jurisdiction {
        let res = sqlx::query("UPDATE issuers SET legal_jurisdiction = $1 WHERE id = $2")
            .bind(&j)
            .bind(id.as_uuid())
            .execute(pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(DomainError::not_found("Issuer", id));
        }
    }
    read::get_issuer(pool, id).await
}
