//! Delete a `Rule` record.
//!
//! Implements FR-038 of `specs/016-implement-domain-model/spec.md`.

use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::{DomainId, RuleId};

/// Delete a rule from the shared catalog.
///
/// Rules are current-state configuration, not protected immutable history
/// (predecessor FR-041): unlike events and reference-data records, they MAY
/// be deleted and replaced.
///
/// Because the rule is **shared**, deleting it removes it **and its
/// attachments from every portfolio** that used it — the catalog owns the
/// definition, and the presence of attachments does not block the deletion.
/// Deleting a *portfolio*, by contrast, removes only that portfolio's
/// attachments; the rules stay in the catalog for other portfolios.
///
/// Returns `DomainError::NotFound` if no such record exists.
///
/// Implements FR-038 of spec 016-implement-domain-model.
pub async fn delete_rule(pool: &PgPool, id: RuleId) -> Result<(), DomainError> {
    let res = sqlx::query("DELETE FROM rules WHERE id = $1")
        .bind(id.as_uuid())
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(DomainError::not_found("Rule", id));
    }
    Ok(())
}
