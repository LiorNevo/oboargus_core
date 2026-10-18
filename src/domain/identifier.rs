//! Shared vocabulary and key handling for the external-identifier entities
//! (`AssetIdentifier`, `IssuerIdentifier`).
//!
//! The two identifier entities are mirror images (one for assets, one for
//! issuers); everything they share — the open scheme vocabulary (FR-026),
//! key validation and normalization (FR-025), and the typed uniqueness error
//! (FR-024) — lives here.
//!
//! Implements FR-024, FR-025, and FR-026 of
//! `specs/016-implement-domain-model/spec.md`.

pub mod scheme;

#[cfg(test)]
mod tests;

pub use scheme::{IdentifierScheme, normalize_lookup, normalize_pair, uniqueness_violation};
