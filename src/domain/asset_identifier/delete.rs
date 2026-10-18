//! Delete an `AssetIdentifier` record.
//!
//! Identifiers are current-state keys, not protected immutable history
//! (predecessor FR-033/FR-034): unlike events and reference-data records,
//! they MAY be deleted and replaced. Deleting an identifier is how a stale
//! ticker or a retired ISIN is unregistered. Deleting the asset itself
//! removes its identifiers automatically (the schema cascades; FR-022).
//!
//! Implements FR-027 of `specs/016-implement-domain-model/spec.md`.

use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::{DomainId, IdentifierId};

/// Delete an identifier record by id.
///
/// Returns `DomainError::NotFound` if no such record exists.
///
/// Implements FR-027 of spec 016-implement-domain-model.
pub async fn delete_asset_identifier(pool: &PgPool, id: IdentifierId) -> Result<(), DomainError> {
    let res = sqlx::query("DELETE FROM asset_identifiers WHERE id = $1")
        .bind(id.as_uuid())
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(DomainError::not_found("AssetIdentifier", id));
    }
    Ok(())
}
