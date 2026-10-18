//! Create an `AssetIdentifier` record.
//!
//! Implements FR-024, FR-025, and FR-027 of
//! `specs/016-implement-domain-model/spec.md`.

use sqlx::PgPool;

use crate::domain::asset_identifier::entity::{AssetIdentifier, NewAssetIdentifier};
use crate::domain::error::DomainError;
use crate::domain::id::{DomainId, IdentifierId, new_uuid};
use crate::domain::identifier::{normalize_pair, uniqueness_violation};

/// Create an external identifier naming an asset.
///
/// The `(scheme, value)` pair is validated and normalized (FR-025: the
/// scheme is trimmed and uppercased, the value is trimmed), the named asset
/// must exist, and the pair must be unclaimed in the asset-identifier domain
/// (FR-024). A pair that is already claimed returns
/// `DomainError::InvariantViolation { invariant: "identifier_uniqueness", .. }`;
/// the same typed error is returned if a concurrent registration wins the
/// unique-constraint race.
///
/// Implements FR-024 of spec 016-implement-domain-model.
pub async fn create_asset_identifier(
    pool: &PgPool,
    input: NewAssetIdentifier,
) -> Result<AssetIdentifier, DomainError> {
    let (scheme, value) = normalize_pair(&input.scheme, &input.value)?;

    // The named asset must exist. A `NotFound` is a clearer typed error than
    // a raw foreign-key violation.
    let subject: Option<(uuid::Uuid,)> = sqlx::query_as("SELECT id FROM assets WHERE id = $1")
        .bind(input.asset_id.as_uuid())
        .fetch_optional(pool)
        .await?;
    if subject.is_none() {
        return Err(DomainError::not_found("Asset", input.asset_id));
    }

    // Pre-insert uniqueness check (FR-024). The database unique constraint
    // is the defense-in-depth behind this check.
    if let Some(claimant) = claimant_of(pool, &scheme, &value).await? {
        return Err(uniqueness_violation("Asset", &scheme, &value, claimant));
    }

    let new_id = new_uuid();
    let inserted = sqlx::query(
        "INSERT INTO asset_identifiers (id, asset_id, scheme, value) VALUES ($1, $2, $3, $4)",
    )
    .bind(new_id)
    .bind(input.asset_id.as_uuid())
    .bind(&scheme)
    .bind(&value)
    .execute(pool)
    .await;

    match inserted {
        Ok(_) => {}
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            // Lost a race with a concurrent registration of the same pair.
            let claimant = claimant_of(pool, &scheme, &value)
                .await?
                .unwrap_or_else(|| "another record".to_string());
            return Err(uniqueness_violation("Asset", &scheme, &value, claimant));
        }
        Err(e) => return Err(e.into()),
    }

    super::read::get_asset_identifier(pool, IdentifierId::from_uuid(new_id)).await
}

/// The id of the record that already claims `(scheme, value)`, if any.
async fn claimant_of(
    pool: &PgPool,
    scheme: &str,
    value: &str,
) -> Result<Option<String>, DomainError> {
    let row: Option<(uuid::Uuid,)> =
        sqlx::query_as("SELECT id FROM asset_identifiers WHERE scheme = $1 AND value = $2")
            .bind(scheme)
            .bind(value)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(id,)| IdentifierId::from_uuid(id).to_string()))
}
