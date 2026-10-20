//! `Rule`: a shared rule expression that governs a portfolio.
//!
//! A rule is categorized by the **effect** it has on the portfolio's data —
//! `all_assets`, `portfolio_data`, `positions`, `events`, or any other value
//! (the vocabulary is open, per predecessor FR-037).
//!
//! Rules are **shared**: a rule lives in a rule catalog, is owned by no
//! portfolio, and governs every portfolio it is attached to. Attaching it to
//! a second portfolio neither copies nor forks it — there is still one rule.
//!
//! Rules are current-state configuration, not dual-temporal facts
//! (predecessor FR-041): they carry no `as_of_date` / `knowledge_date`.
//!
//! Implements US9 and FR-029..FR-040 of
//! `specs/016-implement-domain-model/spec.md` (predecessor FR-035..FR-044 of
//! `specs/015-database-uml-domains/spec.md`).

pub mod attach;
pub mod create;
pub mod delete;
pub mod entity;
pub mod read;
pub mod update;

#[cfg(test)]
mod tests;

pub use attach::{attach_rule_to_portfolio, detach_rule_from_portfolio};
pub use create::create_rule;
pub use delete::delete_rule;
pub use entity::{NewRule, Rule, RuleCategory, RulePatch};
pub use read::{get_rule, list_rules, list_rules_by_category, list_rules_for_portfolio};
pub use update::update_rule;
