//! Issuer: the entity that issues an asset.
//!
//! Implements US2 and US2b of `specs/016-implement-domain-model/spec.md`.
//! The `parent_id` field and DAG support are added in US2b.

pub mod create;
pub mod dag;
pub mod delete;
pub mod entity;
pub mod read;
pub mod reparent;
pub mod update;

#[cfg(test)]
mod tests;

pub use create::create_issuer;
pub use dag::{detect_cycle, is_self_parent};
pub use delete::delete_issuer;
pub use entity::{Issuer, IssuerPatch, NewIssuer};
pub use read::{get_issuer, list_ancestor_issuers, list_children_issuers, list_issuers};
pub use reparent::reparent_issuer;
pub use update::update_issuer;
