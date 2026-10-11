//! PortfolioReferenceData: a time-stamped fact about a portfolio, with
//! dual-temporal semantics.
//!
//! Implements US3c of the predecessor and US3 of this feature.

pub mod create;
pub mod entity;
pub mod read;
pub mod update;

#[cfg(test)]
mod tests;

pub use create::create_portfolio_reference_data;
pub use entity::{NewPortfolioReferenceData, PortfolioReferenceData};
pub use read::{get_current_portfolio_reference_data, get_portfolio_reference_data_history};
pub use update::update_portfolio_reference_value;
