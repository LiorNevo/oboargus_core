//! Portfolio: a node in a DAG of portfolios.
//!
//! Implements US1 and FR-001, FR-002, FR-003, FR-004, FR-017, FR-019 of
//! `specs/016-implement-domain-model/spec.md`.

pub mod create;
pub mod dag;
pub mod delete;
pub mod entity;
pub mod read;
pub mod update;

#[cfg(test)]
mod tests;

pub use create::create_portfolio;
pub use delete::delete_portfolio;
pub use entity::{NewPortfolio, Portfolio, PortfolioPatch};
pub use read::{get_portfolio, list_ancestors, list_children};
pub use update::update_portfolio;
