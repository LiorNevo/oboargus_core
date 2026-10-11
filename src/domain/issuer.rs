//! Issuer: the entity that issues an asset.
//!
//! Implements US2 of `specs/016-implement-domain-model/spec.md`.

pub mod create;
pub mod delete;
pub mod entity;
pub mod read;
pub mod update;

#[cfg(test)]
mod tests;

pub use create::create_issuer;
pub use delete::delete_issuer;
pub use entity::{Issuer, IssuerPatch, NewIssuer};
pub use read::{get_issuer, list_issuers};
pub use update::update_issuer;
