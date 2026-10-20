//! `Rule` struct and input types.
//!
//! A rule expression held in a **shared rule catalog** — a mandate, a
//! concentration cap, a benchmark check. A rule is owned by no portfolio: it
//! governs every portfolio it is attached to. Each rule is categorized by the
//! **effect** it has on the portfolio's data (`all_assets`, `portfolio_data`,
//! `positions`, `events`, or any other value — the vocabulary is open, per
//! predecessor FR-037).
//!
//! Rules are current-state configuration, not dual-temporal facts
//! (predecessor FR-041): they carry no `as_of_date` / `knowledge_date`.
//!
//! Implements FR-030 and FR-031 of `specs/016-implement-domain-model/spec.md`
//! (predecessor FR-035..FR-041 of `specs/015-database-uml-domains/spec.md`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::domain::error::DomainError;
use crate::domain::id::{DomainId, RuleId};

/// The effect a rule has on its portfolio's data.
///
/// This is a **non-exhaustive** set of constants (FR-031): any category
/// string is storable and listable. An unrecognized category behaves exactly
/// like a recognized one, so a new kind of effect ships without a model
/// change. The category belongs to the shared rule, so it is the same for
/// every portfolio the rule is attached to.
#[derive(Debug, Clone, PartialEq, Eq)]
/// Implements FR-031 of spec 016-implement-domain-model.
pub struct RuleCategory;

impl RuleCategory {
    /// The rule acts upon every asset in the portfolio.
    pub const ALL_ASSETS: &'static str = "all_assets";
    /// The rule acts upon the portfolio's own data (reference data, attributes).
    pub const PORTFOLIO_DATA: &'static str = "portfolio_data";
    /// The rule acts upon the portfolio's positions.
    pub const POSITIONS: &'static str = "positions";
    /// The rule acts upon the portfolio's event stream.
    pub const EVENTS: &'static str = "events";

    /// Normalize a category for storage and lookup: trim, then lowercase
    /// (FR-032). `" All_Assets "` and `"all_assets"` both normalize to
    /// `"all_assets"`.
    /// Implements FR-032 of spec 016-implement-domain-model.
    pub fn normalize(category: &str) -> String {
        category.trim().to_lowercase()
    }
}

/// Validate and normalize a raw category for storage (FR-032, FR-033).
///
/// Returns the normalized category, or `DomainError::Validation` when the
/// category is empty after trimming. An unrecognized (but non-empty)
/// category is accepted — the vocabulary is open (FR-031).
///
/// Implements FR-033 of spec 016-implement-domain-model.
pub fn normalize_category(category: &str) -> Result<String, DomainError> {
    let normalized = RuleCategory::normalize(category);
    if normalized.is_empty() {
        return Err(DomainError::Validation(
            "rule category must not be empty".to_string(),
        ));
    }
    Ok(normalized)
}

/// Normalize a raw category for **lookup** (FR-032): the same normalization
/// as [`normalize_category`], but an empty category is not an error — an
/// empty category can never be claimed (writes reject it), so a lookup of an
/// empty category simply matches nothing and the caller treats the empty
/// result as "no rules in that category".
///
/// Implements FR-032 of spec 016-implement-domain-model.
pub fn normalize_category_for_lookup(category: &str) -> String {
    RuleCategory::normalize(category)
}

/// A rule in the shared rule catalog.
///
/// The rule is owned by no portfolio: it governs every portfolio it is
/// attached to, and editing or deleting it affects all of them.
#[derive(Debug, PartialEq, Eq, FromRow, Serialize, Deserialize)]
/// Implements FR-030 of spec 016-implement-domain-model.
pub struct Rule {
    /// The rule record id.
    pub id: uuid::Uuid,
    /// The rule's name. Unique across the whole catalog (FR-035).
    pub name: String,
    /// The effect category (open vocabulary, e.g. `"all_assets"`).
    /// Stored trimmed and lowercased (FR-032).
    pub category: String,
    /// The rule body, stored inline (predecessor FR-040).
    pub expression: String,
    /// A human-readable description of what the rule does.
    pub description: String,
    /// When the rule was created.
    pub created_at: DateTime<Utc>,
    /// Who authored the rule.
    pub created_by: String,
}

impl Rule {
    /// Borrow the record id as a `RuleId`.
    /// Implements FR-030 of spec 016-implement-domain-model.
    pub fn id(&self) -> RuleId {
        RuleId::from_uuid(self.id)
    }
}

/// Input for `create_rule`.
///
/// The rule is written into the shared catalog; it is **not** attached to a
/// portfolio here. Use `attach_rule_to_portfolio` for that.
#[derive(Debug, Clone)]
/// Implements FR-030 of spec 016-implement-domain-model.
pub struct NewRule {
    /// The rule's name. Trimmed on write; unique across the catalog.
    pub name: String,
    /// The effect category (open vocabulary). Normalized on write (FR-032).
    pub category: String,
    /// The rule body. Trimmed on write; must parse as a rule (FR-034).
    pub expression: String,
    /// A human-readable description of what the rule does.
    pub description: String,
    /// Who authored the rule.
    pub created_by: String,
}

/// The updatable fields of a rule.
///
/// The audit fields `created_at` and `created_by` are NOT updatable
/// (FR-037). Because the rule is shared, an update is visible to every
/// portfolio the rule is attached to.
#[derive(Debug, Clone, Default)]
/// Implements FR-037 of spec 016-implement-domain-model.
pub struct RulePatch {
    /// A new name for the rule. Trimmed on write.
    pub name: Option<String>,
    /// A new effect category. Normalized on write (FR-032).
    pub category: Option<String>,
    /// A new rule body. Trimmed on write; must parse as a rule (FR-034).
    pub expression: Option<String>,
    /// A new description.
    pub description: Option<String>,
}
