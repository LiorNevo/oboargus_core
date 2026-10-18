//! Read `AssetIdentifier` records and resolve identifiers to assets.
//!
//! Implements FR-023 of `specs/016-implement-domain-model/spec.md`.

use sqlx::PgPool;

use crate::domain::asset::entity::Asset;
use crate::domain::asset_identifier::entity::AssetIdentifier;
use crate::domain::error::DomainError;
use crate::domain::id::{AssetId, DomainId, IdentifierId};
use crate::domain::identifier::normalize_lookup;

/// Fetch an identifier record by id.
/// Implements FR-023 of spec 016-implement-domain-model.
pub async fn get_asset_identifier(
    pool: &PgPool,
    id: IdentifierId,
) -> Result<AssetIdentifier, DomainError> {
    let row: Option<AssetIdentifier> = sqlx::query_as(
        "SELECT id, asset_id, scheme, value, created_at FROM asset_identifiers WHERE id = $1",
    )
    .bind(id.as_uuid())
    .fetch_optional(pool)
    .await?;
    row.ok_or_else(|| DomainError::not_found("AssetIdentifier", id))
}

/// List every identifier registered for an asset (zero is the normal case).
/// Implements FR-023 of spec 016-implement-domain-model.
pub async fn list_asset_identifiers(
    pool: &PgPool,
    asset_id: AssetId,
) -> Result<Vec<AssetIdentifier>, DomainError> {
    let rows: Vec<AssetIdentifier> = sqlx::query_as(
        "SELECT id, asset_id, scheme, value, created_at \
         FROM asset_identifiers WHERE asset_id = $1 ORDER BY created_at ASC, id ASC",
    )
    .bind(asset_id.as_uuid())
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
pub async fn find_asset_identifier(
    pool: &PgPool,
    scheme: &str,
    value: &str,
) -> Result<Option<AssetIdentifier>, DomainError> {
    let (scheme, value) = normalize_lookup(scheme, value);
    if scheme.is_empty() || value.is_empty() {
        return Ok(None);
    }
    let row: Option<AssetIdentifier> = sqlx::query_as(
        "SELECT id, asset_id, scheme, value, created_at \
         FROM asset_identifiers WHERE scheme = $1 AND value = $2",
    )
    .bind(scheme)
    .bind(value)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Resolve an external identifier to the asset that carries it.
///
/// Because `(scheme, value)` is unique within the asset-identifier domain
/// (FR-024), the lookup resolves **at most one** asset. `Ok(None)` means no
/// asset claims the pair (which is not an error).
///
/// Implements FR-023 of spec 016-implement-domain-model.
pub async fn find_asset_by_identifier(
    pool: &PgPool,
    scheme: &str,
    value: &str,
) -> Result<Option<Asset>, DomainError> {
    let (scheme, value) = normalize_lookup(scheme, value);
    if scheme.is_empty() || value.is_empty() {
        return Ok(None);
    }
    let row: Option<Asset> = sqlx::query_as(
        "SELECT a.id, a.asset_type, a.display_name, a.currency, a.created_at, \
                a.backed_by_portfolio_id, a.issuer_id \
         FROM assets a \
         JOIN asset_identifiers i ON i.asset_id = a.id \
         WHERE i.scheme = $1 AND i.value = $2",
    )
    .bind(scheme)
    .bind(value)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}
