//! `IssuerIdentifier`: an external identifier (LEI, CIK, company
//! registration number, ...) by which an issuer is known outside Obolargus.
//!
//! The mirror image of `AssetIdentifier`: identifiers are additional,
//! indexed keys (the issuer's identity remains its Obolargus id),
//! `(scheme, value)` is unique within this domain (FR-024), the scheme
//! vocabulary is open (FR-026), and identifier records are replaceable
//! current-state keys that die with their issuer (FR-027).
//!
//! Implements US8 and FR-022..FR-028 of
//! `specs/016-implement-domain-model/spec.md` (predecessor FR-029..FR-032 of
//! `specs/015-database-uml-domains/spec.md`).

pub mod create;
pub mod delete;
pub mod entity;
pub mod read;

#[cfg(test)]
mod tests;

pub use create::create_issuer_identifier;
pub use delete::delete_issuer_identifier;
pub use entity::{IssuerIdentifier, NewIssuerIdentifier};
pub use read::{
    find_issuer_by_identifier, find_issuer_identifier, get_issuer_identifier,
    list_issuer_identifiers,
};
