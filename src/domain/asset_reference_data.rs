//! AssetReferenceData: a time-stamped fact about an asset, with
//! dual-temporal semantics.
//!
//! Implements US3 of `specs/016-implement-domain-model/spec.md`.

pub mod create;
pub mod entity;
pub mod read;
pub mod update;

#[cfg(test)]
mod tests;

pub use create::create_asset_reference_data;
pub use entity::{AssetReferenceData, NewAssetReferenceData};
pub use read::{get_asset_reference_data_history, get_current_asset_reference_data};
pub use update::update_asset_reference_value;
