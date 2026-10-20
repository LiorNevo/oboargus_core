//! Attach a shared rule to a portfolio, and detach it again.
//!
//! Implements FR-036 and FR-038 of `specs/016-implement-domain-model/spec.md`.

use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::{DomainId, PortfolioId, RuleId};

/// Attach a shared rule to a portfolio.
///
/// A rule is owned by no portfolio and may govern any number of them:
/// attaching it to a second portfolio neither copies nor forks it — there is
/// still one rule record, and an edit to it is visible to every portfolio
/// that carries it.
///
/// The `(portfolio, rule)` pair is unique (FR-036): a portfolio carries a
/// given rule at most once, so attaching a rule a portfolio already has
/// returns `DomainError::InvariantViolation { invariant:
/// "rule_attachment_uniqueness", .. }`. The same typed error is returned if a
/// concurrent attach wins the primary-key race.
///
/// Attaching a rule to a portfolio that does not exist, or attaching a rule
/// that does not exist, returns `DomainError::NotFound`.
///
/// Implements FR-036 of spec 016-implement-domain-model.
pub async fn attach_rule_to_portfolio(
    pool: &PgPool,
    portfolio_id: PortfolioId,
    rule_id: RuleId,
) -> Result<(), DomainError> {
    ensure_portfolio_exists(pool, portfolio_id).await?;
    ensure_rule_exists(pool, rule_id).await?;

    // Pre-insert uniqueness check (FR-036). The join table's primary key is
    // the defense-in-depth behind this check.
    if is_attached(pool, portfolio_id, rule_id).await? {
        return Err(attachment_uniqueness_violation(portfolio_id, rule_id));
    }

    let inserted =
        sqlx::query("INSERT INTO portfolio_rules (portfolio_id, rule_id) VALUES ($1, $2)")
            .bind(portfolio_id.as_uuid())
            .bind(rule_id.as_uuid())
            .execute(pool)
            .await;

    match inserted {
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            // Lost a race with a concurrent attach of the same rule.
            Err(attachment_uniqueness_violation(portfolio_id, rule_id))
        }
        Err(e) => Err(e.into()),
    }
}

/// Remove the link between a portfolio and a rule.
///
/// The rule itself stays in the shared catalog — it may still be attached to
/// other portfolios (FR-038).
///
/// Implements FR-038 of spec 016-implement-domain-model.
pub async fn detach_rule_from_portfolio(
    pool: &PgPool,
    portfolio_id: PortfolioId,
    rule_id: RuleId,
) -> Result<(), DomainError> {
    let res = sqlx::query("DELETE FROM portfolio_rules WHERE portfolio_id = $1 AND rule_id = $2")
        .bind(portfolio_id.as_uuid())
        .bind(rule_id.as_uuid())
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(DomainError::not_found(
            "PortfolioRule",
            format!("{portfolio_id}/{rule_id}"),
        ));
    }
    Ok(())
}

/// Whether the portfolio already carries the rule.
async fn is_attached(
    pool: &PgPool,
    portfolio_id: PortfolioId,
    rule_id: RuleId,
) -> Result<bool, DomainError> {
    let row: Option<(uuid::Uuid,)> = sqlx::query_as(
        "SELECT rule_id FROM portfolio_rules WHERE portfolio_id = $1 AND rule_id = $2",
    )
    .bind(portfolio_id.as_uuid())
    .bind(rule_id.as_uuid())
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Reject an attach that names a portfolio that does not exist.
async fn ensure_portfolio_exists(
    pool: &PgPool,
    portfolio_id: PortfolioId,
) -> Result<(), DomainError> {
    let row: Option<(uuid::Uuid,)> = sqlx::query_as("SELECT id FROM portfolios WHERE id = $1")
        .bind(portfolio_id.as_uuid())
        .fetch_optional(pool)
        .await?;
    if row.is_none() {
        return Err(DomainError::not_found("Portfolio", portfolio_id));
    }
    Ok(())
}

/// Reject an attach that names a rule that does not exist.
async fn ensure_rule_exists(pool: &PgPool, rule_id: RuleId) -> Result<(), DomainError> {
    let row: Option<(uuid::Uuid,)> = sqlx::query_as("SELECT id FROM rules WHERE id = $1")
        .bind(rule_id.as_uuid())
        .fetch_optional(pool)
        .await?;
    if row.is_none() {
        return Err(DomainError::not_found("Rule", rule_id));
    }
    Ok(())
}

/// The typed error for a violated `(portfolio, rule)` uniqueness key
/// (FR-036).
pub(super) fn attachment_uniqueness_violation(
    portfolio_id: PortfolioId,
    rule_id: RuleId,
) -> DomainError {
    DomainError::invariant(
        "rule_attachment_uniqueness",
        format!("portfolio {portfolio_id} already carries rule {rule_id}"),
    )
}
