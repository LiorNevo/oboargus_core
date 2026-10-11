use crate::domain::id::DomainId;
use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::IssuerId;

/// Implements FR-019 of spec 016-implement-domain-model.
pub async fn delete_issuer(pool: &PgPool, id: IssuerId) -> Result<(), DomainError> {
    let (asset_count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM assets WHERE issuer_id = $1")
        .bind(id.as_uuid())
        .fetch_one(pool)
        .await?;
    if asset_count > 0 {
        return Err(DomainError::cannot_delete(
            "Issuer",
            id,
            "issuer has assets; re-assign or retire them first",
        ));
    }

    let (ref_count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM issuer_reference_data WHERE issuer_id = $1")
            .bind(id.as_uuid())
            .fetch_one(pool)
            .await?;
    if ref_count > 0 {
        return Err(DomainError::cannot_delete(
            "Issuer",
            id,
            "issuer has reference data; reference data is immutable",
        ));
    }

    let res = sqlx::query("DELETE FROM issuers WHERE id = $1")
        .bind(id.as_uuid())
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(DomainError::not_found("Issuer", id));
    }
    Ok(())
}
