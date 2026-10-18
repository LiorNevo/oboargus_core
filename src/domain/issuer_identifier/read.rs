//! Read `IssuerIdentifier` records and resolve identifiers to issuers.
//!
//! Implements FR-023 of `specs/016-implement-domain-model/spec.md`.

use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::{DomainId, IdentifierId, IssuerId};
use crate::domain::identifier::normalize_lookup;
use crate::domain::issuer::entity::Issuer;
use crate::domain::issuer_identifier::entity::IssuerIdentifier;

/// Fetch an identifier record by id.
/// Implements FR-023 of spec 016-implement-domain-model.
pub async fn get_issuer_identifier(
    pool: &PgPool,
    id: IdentifierId,
) -> Result<IssuerIdentifier, DomainError> {
    let row: Option<IssuerIdentifier> = sqlx::query_as(
        "SELECT id, issuer_id, scheme, value, created_at FROM issuer_identifiers WHERE id = $1",
    )
    .bind(id.as_uuid())
    .fetch_optional(pool)
    .await?;
    row.ok_or_else(|| DomainError::not_found("IssuerIdentifier", id))
}

/// List every identifier registered for an issuer (zero is the normal case).
/// Implements FR-023 of spec 016-implement-domain-model.
pub async fn list_issuer_identifiers(
    pool: &PgPool,
    issuer_id: IssuerId,
) -> Result<Vec<IssuerIdentifier>, DomainError> {
    let rows: Vec<IssuerIdentifier> = sqlx::query_as(
        "SELECT id, issuer_id, scheme, value, created_at \
         FROM issuer_identifiers WHERE issuer_id = $1 ORDER BY created_at ASC, id ASC",
    )
    .bind(issuer_id.as_uuid())
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Find the identifier record claiming `(scheme, value)`, if any.
///
/// The pair is normalized exactly as on write (FR-025). An empty scheme or
/// value can never be claimed (writes reject it), so an empty pair simply
/// matches nothing and returns `Ok(None)`.
///
/// Implements FR-023 of spec 016-implement-domain-model.
pub async fn find_issuer_identifier(
    pool: &PgPool,
    scheme: &str,
    value: &str,
) -> Result<Option<IssuerIdentifier>, DomainError> {
    let (scheme, value) = normalize_lookup(scheme, value);
    if scheme.is_empty() || value.is_empty() {
        return Ok(None);
    }
    let row: Option<IssuerIdentifier> = sqlx::query_as(
        "SELECT id, issuer_id, scheme, value, created_at \
         FROM issuer_identifiers WHERE scheme = $1 AND value = $2",
    )
    .bind(scheme)
    .bind(value)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Resolve an external identifier to the issuer that carries it.
///
/// Because `(scheme, value)` is unique within the issuer-identifier domain
/// (FR-024), the lookup resolves **at most one** issuer. `Ok(None)` means no
/// issuer claims the pair (which is not an error).
///
/// Implements FR-023 of spec 016-implement-domain-model.
pub async fn find_issuer_by_identifier(
    pool: &PgPool,
    scheme: &str,
    value: &str,
) -> Result<Option<Issuer>, DomainError> {
    let (scheme, value) = normalize_lookup(scheme, value);
    if scheme.is_empty() || value.is_empty() {
        return Ok(None);
    }
    let row: Option<Issuer> = sqlx::query_as(
        "SELECT s.id, s.name, s.issuer_type, s.legal_jurisdiction, s.created_at, s.parent_id \
         FROM issuers s \
         JOIN issuer_identifiers i ON i.issuer_id = s.id \
         WHERE i.scheme = $1 AND i.value = $2",
    )
    .bind(scheme)
    .bind(value)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}
