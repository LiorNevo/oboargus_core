//! IssuerReferenceData: a time-stamped fact about an issuer, with
//! dual-temporal semantics.
//!
//! Implements US3 of `specs/016-implement-domain-model/spec.md`.

pub mod create;
pub mod entity;
pub mod read;
pub mod update;

#[cfg(test)]
mod tests;

pub use create::create_issuer_reference_data;
pub use entity::{IssuerReferenceData, NewIssuerReferenceData};
pub use read::{get_current_issuer_reference_data, get_issuer_reference_data_history};
pub use update::update_issuer_reference_value;
