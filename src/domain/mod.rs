//! High-level financial domain model.
//!
//! Implements the high-level entities, relationships, and invariants defined
//! in `specs/015-database-uml-domains/spec.md`, plus the normalized fact
//! model of `specs/027-normalized-fact-model/spec.md`. Reference-data facts
//! are typed and append-only in `crate::temporal`; this module owns the
//! identity, position, event, identifier, and rule entities.
//!
//! Module organization: one submodule per high-level entity, plus shared
//! scaffolding (`error`, `id`, `model_description`, `seed`).

pub mod asset;
pub mod asset_identifier;
pub mod entity_accessors;
pub mod error;
pub mod event;
pub mod id;
pub mod identifier;
pub mod issuer;
pub mod issuer_identifier;
pub mod model_description;
pub mod portfolio;
pub mod position;
pub mod rule;
pub mod seed;
pub(crate) mod time_support;

#[cfg(test)]
mod tests;

pub use error::DomainError;
pub use identifier::IdentifierScheme;
pub use model_description::{
    EntityDescription, FieldDescription, ModelDescription, RelationshipDescription,
    model_description,
};
pub use rule::RuleCategory;
pub use seed::{SeedSummary, seed_canonical_dataset};
