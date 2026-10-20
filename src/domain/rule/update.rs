//! Update a `Rule` record.
//!
//! Implements FR-037 of `specs/016-implement-domain-model/spec.md`.

use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::{DomainId, RuleId};
use crate::domain::rule::create::name_uniqueness_violation;
use crate::domain::rule::entity::{Rule, RulePatch, normalize_category};
use crate::domain::rule::read::get_rule;

/// Update the mutable fields of a rule.
///
/// The `name`, `category`, `expression`, and `description` are updatable.
/// The audit fields `created_at` / `created_by` are not part of
/// [`RulePatch`] and therefore cannot be changed through this API (FR-037).
///
/// Because the rule is **shared**, an update is visible to every portfolio
/// the rule is attached to — the implementation never copies the rule per
/// portfolio to isolate an edit.
///
/// Every supplied field is validated exactly as it is on create: a
/// non-empty name and expression (FR-033), a parseable expression (FR-034), a
/// non-empty category (FR-033), and a name that no other rule in the
/// catalog already uses (FR-035).
///
/// Implements FR-037 of spec 016-implement-domain-model.
pub async fn update_rule(pool: &PgPool, id: RuleId, patch: RulePatch) -> Result<Rule, DomainError> {
    let existing = get_rule(pool, id).await?;

    let name = match &patch.name {
        Some(raw) => {
            let trimmed = raw.trim().to_string();
            if trimmed.is_empty() {
                return Err(DomainError::Validation(
                    "rule name must not be empty".to_string(),
                ));
            }
            trimmed
        }
        None => existing.name.clone(),
    };

    let category = match &patch.category {
        Some(raw) => normalize_category(raw)?,
        None => existing.category.clone(),
    };

    let expression = match &patch.expression {
        Some(raw) => {
            let trimmed = raw.trim().to_string();
            if trimmed.is_empty() {
                return Err(DomainError::Validation(
                    "rule expression must not be empty".to_string(),
                ));
            }
            super::create::validate_expression(&trimmed)?;
            trimmed
        }
        None => existing.expression.clone(),
    };

    let description = match &patch.description {
        Some(raw) => raw.trim().to_string(),
        None => existing.description.clone(),
    };

    // A rename must not collide with another rule in the catalog (FR-035).
    // The rule being renamed is excluded from the check.
    if name != existing.name {
        let claimant: Option<(uuid::Uuid,)> =
            sqlx::query_as("SELECT id FROM rules WHERE name = $1 AND id <> $2")
                .bind(&name)
                .bind(existing.id)
                .fetch_optional(pool)
                .await?;
        if let Some((claimant_id,)) = claimant {
            return Err(name_uniqueness_violation(
                &name,
                RuleId::from_uuid(claimant_id),
            ));
        }
    }

    let updated = sqlx::query(
        "UPDATE rules SET name = $1, category = $2, expression = $3, description = $4 \
         WHERE id = $5",
    )
    .bind(&name)
    .bind(&category)
    .bind(&expression)
    .bind(&description)
    .bind(id.as_uuid())
    .execute(pool)
    .await;

    match updated {
        Ok(_) => {}
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            // Lost a race with a concurrent rename onto the same name.
            let claimant: Option<(uuid::Uuid,)> =
                sqlx::query_as("SELECT id FROM rules WHERE name = $1")
                    .bind(&name)
                    .fetch_optional(pool)
                    .await?;
            let claimed_by = claimant
                .map(|(claimant_id,)| RuleId::from_uuid(claimant_id))
                .unwrap_or_else(|| RuleId::from_uuid(existing.id));
            return Err(name_uniqueness_violation(&name, claimed_by));
        }
        Err(e) => return Err(e.into()),
    }

    get_rule(pool, id).await
}
