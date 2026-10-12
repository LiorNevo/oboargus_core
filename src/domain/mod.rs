//! High-level financial domain model.
//!
//! Implements the 8 entities, 12 relationships, and 12 invariants defined
//! in `specs/015-database-uml-domains/spec.md` (the 12th relationship and
//! 12th invariant are the issuer DAG added in US2b). See the spec for the
//! model and `specs/016-implement-domain-model/spec.md` for the implementation
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
pub use model_description::{
    EntityDescription, FieldDescription, ModelDescription, RelationshipDescription,
    model_description,
};
pub use seed::{SeedSummary, seed_canonical_dataset};
