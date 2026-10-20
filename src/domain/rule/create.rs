//! Create a `Rule` record in the shared rule catalog.
//!
//! Implements FR-029, FR-033, FR-034, and FR-035 of
//! `specs/016-implement-domain-model/spec.md`.

use lx_engine::input::rule::LxRule;
use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::{DomainId, RuleId, new_uuid};
use crate::domain::rule::entity::{NewRule, Rule, normalize_category};

/// Create a rule in the shared rule catalog.
///
/// The rule is **not** attached to a portfolio here — a rule is owned by no
/// portfolio and may govern any number of them. Use
/// `attach_rule_to_portfolio` to link it to one.
///
/// The write validates the rule before it touches the database:
///
/// * the `name`, `category`, and `expression` must be non-empty after
///   trimming (FR-033);
/// * the `expression` must parse as a rule (FR-034) — the check is a
///   **parse**, never an evaluation, so persisting a rule never runs it;
/// * no other rule in the catalog may already carry the same `name`
///   (FR-035). A conflicting write returns
///   `DomainError::InvariantViolation { invariant: "rule_name_uniqueness", .. }`;
///   the same typed error is returned if a concurrent create wins the
///   unique-constraint race.
///
/// The `category` is normalized (trimmed, lowercased) on write (FR-032). An
/// unrecognized category is accepted — the vocabulary is open (FR-031).
///
/// Implements FR-030 of spec 016-implement-domain-model.
pub async fn create_rule(pool: &PgPool, input: NewRule) -> Result<Rule, DomainError> {
    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err(DomainError::Validation(
            "rule name must not be empty".to_string(),
        ));
    }

    let category = normalize_category(&input.category)?;

    let expression = input.expression.trim().to_string();
    if expression.is_empty() {
        return Err(DomainError::Validation(
            "rule expression must not be empty".to_string(),
        ));
    }
    validate_expression(&expression)?;

    let description = input.description.trim().to_string();
    let created_by = input.created_by.trim().to_string();

    // Pre-insert uniqueness check (FR-035). The database unique constraint
    // is the defense-in-depth behind this check.
    if let Some(claimant) = claimant_of(pool, &name).await? {
        return Err(name_uniqueness_violation(&name, claimant));
    }

    let new_id = new_uuid();
    let inserted = sqlx::query(
        "INSERT INTO rules (id, name, category, expression, description, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(new_id)
    .bind(&name)
    .bind(&category)
    .bind(&expression)
    .bind(&description)
    .bind(&created_by)
    .execute(pool)
    .await;

    match inserted {
        Ok(_) => {}
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            // Lost a race with a concurrent create of the same rule name.
            let claimant = claimant_of(pool, &name)
                .await?
                .unwrap_or_else(|| "another record".to_string());
            return Err(name_uniqueness_violation(&name, claimant));
        }
        Err(e) => return Err(e.into()),
    }

    super::read::get_rule(pool, RuleId::from_uuid(new_id)).await
}

/// The id of the rule that already claims `name` in the catalog, if any.
async fn claimant_of(pool: &PgPool, name: &str) -> Result<Option<String>, DomainError> {
    let row: Option<(uuid::Uuid,)> = sqlx::query_as("SELECT id FROM rules WHERE name = $1")
        .bind(name)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|(id,)| RuleId::from_uuid(id).to_string()))
}

/// The typed error for a violated rule-name uniqueness key (FR-035).
///
/// Used both for the pre-insert application check and for mapping a database
/// unique-constraint violation, so a race cannot leak a raw `Infrastructure`
/// error.
pub(super) fn name_uniqueness_violation(
    name: &str,
    claimed_by: impl std::fmt::Display,
) -> DomainError {
    DomainError::invariant(
        "rule_name_uniqueness",
        format!("rule name '{name}' is already used by rule {claimed_by}"),
    )
}

/// Check that the expression parses as a rule (FR-034).
///
/// The expression is parsed with the rule engine's own deserializer, so the
/// accepted syntax is exactly the syntax the engine will accept at
/// evaluation time. The expression is never evaluated here.
pub(super) fn validate_expression(expression: &str) -> Result<(), DomainError> {
    serde_json::from_str::<LxRule>(expression)
        .map(|_| ())
        .map_err(|e| DomainError::Validation(format!("rule expression is not a valid rule: {e}")))
}
