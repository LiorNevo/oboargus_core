//! `AssetIdentifier`: an external identifier (ISIN, ticker, CUSIP,
//! Bloomberg ID, ...) by which an asset is known outside Obolargus.
//!
//! Identifiers are additional, indexed keys: the asset's identity remains
//! its internal Obolargus id (predecessor FR-028). `(scheme, value)` is
//! unique within this domain (FR-024), the scheme vocabulary is open
//! (FR-026), and identifier records are replaceable current-state keys that
//! die with their asset (FR-027).
//!
//! Implements US8 and FR-022..FR-028 of
//! `specs/016-implement-domain-model/spec.md` (predecessor FR-028..FR-032 of
//! `specs/015-database-uml-domains/spec.md`).

pub mod create;
pub mod delete;
pub mod entity;
pub mod read;

#[cfg(test)]
mod tests;

pub use create::create_asset_identifier;
pub use delete::delete_asset_identifier;
pub use entity::{AssetIdentifier, NewAssetIdentifier};
pub use read::{
    find_asset_by_identifier, find_asset_identifier, get_asset_identifier, list_asset_identifiers,
};
