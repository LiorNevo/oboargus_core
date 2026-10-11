use sqlx::PgPool;
use crate::domain::id::DomainId;

use crate::domain::error::DomainError;
use crate::domain::id::{new_uuid, IssuerId};
use crate::domain::issuer::entity::{Issuer, NewIssuer};

/// Implements FR-008 of spec 016-implement-domain-model.
pub async fn create_issuer(pool: &PgPool, input: NewIssuer) -> Result<Issuer, DomainError> {
    let id = new_uuid();
    sqlx::query(
        "INSERT INTO issuers (id, name, issuer_type, legal_jurisdiction) VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(&input.name)
    .bind(&input.issuer_type)
    .bind(&input.legal_jurisdiction)
    .execute(pool)
    .await?;
    super::read::get_issuer(pool, IssuerId::from_uuid(id)).await
}
