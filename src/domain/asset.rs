//! Asset: a unit of legal property that can be held in a portfolio.
//!
//! Implements US2 and FR-001, FR-002, FR-005, FR-017, FR-019 of
//! `specs/016-implement-domain-model/spec.md`.

pub mod create;
pub mod delete;
pub mod entity;
pub mod no_self_investment;
pub mod read;
pub mod update;

#[cfg(test)]
mod tests;

pub use create::create_asset;
pub use delete::delete_asset;
pub use entity::{Asset, AssetFilter, AssetOption, AssetPatch, AssetSearchQuery, NewAsset};
pub use no_self_investment::check_no_self_investment;
pub use read::{get_asset, list_assets, search_asset_options};
pub use update::update_asset;
