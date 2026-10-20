//! Read `Rule` records.
//!
//! Implements FR-030 of `specs/016-implement-domain-model/spec.md`.

use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::{DomainId, PortfolioId, RuleId};
use crate::domain::rule::entity::{Rule, normalize_category_for_lookup};

/// Fetch a rule record by id.
///
/// Implements FR-030 of spec 016-implement-domain-model.
pub async fn get_rule(pool: &PgPool, id: RuleId) -> Result<Rule, DomainError> {
    let row: Option<Rule> = sqlx::query_as(
        "SELECT id, name, category, expression, description, created_at, created_by \
         FROM rules WHERE id = $1",
    )
    .bind(id.as_uuid())
    .fetch_optional(pool)
    .await?;
    row.ok_or_else(|| DomainError::not_found("Rule", id))
}

/// List the whole rule catalog.
///
/// The catalog includes rules that are attached to no portfolio (drafts) and
/// rules that are attached to many. Rules are ordered by name so the result
/// is stable.
///
/// Implements FR-030 of spec 016-implement-domain-model.
pub async fn list_rules(pool: &PgPool) -> Result<Vec<Rule>, DomainError> {
    let rows: Vec<Rule> = sqlx::query_as(
        "SELECT id, name, category, expression, description, created_at, created_by \
         FROM rules ORDER BY name ASC, id ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// List every rule attached to a portfolio.
///
/// The list contains exactly the rules attached to `portfolio_id`. It never
/// traverses the portfolio DAG: a child portfolio does **not** inherit its
/// ancestors' rules (predecessor FR-039 / Invariant 15). A rule attached to
/// several portfolios appears in each of their lists — there is still only one
/// rule record.
///
/// Implements FR-030 of spec 016-implement-domain-model.
pub async fn list_rules_for_portfolio(
    pool: &PgPool,
    portfolio_id: PortfolioId,
) -> Result<Vec<Rule>, DomainError> {
    let rows: Vec<Rule> = sqlx::query_as(
        "SELECT r.id, r.name, r.category, r.expression, r.description, r.created_at, r.created_by \
         FROM rules r \
         JOIN portfolio_rules pr ON pr.rule_id = r.id \
         WHERE pr.portfolio_id = $1 ORDER BY r.name ASC, r.id ASC",
    )
    .bind(portfolio_id.as_uuid())
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// List the rules attached to a portfolio in one effect category.
///
/// The category is normalized exactly as on write (FR-032). An empty
/// category can never be claimed (writes reject it), so an empty category
/// simply matches nothing and returns an empty list. An unrecognized
/// category is a legitimate filter — the vocabulary is open (FR-031).
///
/// Implements FR-030 of spec 016-implement-domain-model.
pub async fn list_rules_by_category(
    pool: &PgPool,
    portfolio_id: PortfolioId,
    category: &str,
) -> Result<Vec<Rule>, DomainError> {
    let category = normalize_category_for_lookup(category);
    if category.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<Rule> = sqlx::query_as(
        "SELECT r.id, r.name, r.category, r.expression, r.description, r.created_at, r.created_by \
         FROM rules r \
         JOIN portfolio_rules pr ON pr.rule_id = r.id \
         WHERE pr.portfolio_id = $1 AND r.category = $2 ORDER BY r.name ASC, r.id ASC",
    )
    .bind(portfolio_id.as_uuid())
    .bind(&category)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
