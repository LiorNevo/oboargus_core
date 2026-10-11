//! High-level financial domain model.
//!
//! Implements the 8 entities, 11 relationships, and 11 invariants defined
//! in `specs/015-database-uml-domains/spec.md`. See the spec for the model
//! and `specs/016-implement-domain-model/spec.md` for the implementation
//! contract.
//!
//! Module organization: one submodule per high-level entity, plus shared
//! scaffolding (`error`, `id`, `model_description`, `seed`).

pub mod asset;
pub mod asset_reference_data;
pub mod entity_accessors;
pub mod error;
pub mod event;
pub mod id;
pub mod issuer;
pub mod issuer_reference_data;
pub mod model_description;
pub mod portfolio;
pub mod portfolio_reference_data;
pub mod position;
pub mod seed;

#[cfg(test)]
mod tests;

pub use error::DomainError;
pub use model_description::{model_description, EntityDescription, FieldDescription, ModelDescription, RelationshipDescription};
pub use seed::{seed_canonical_dataset, SeedSummary};
