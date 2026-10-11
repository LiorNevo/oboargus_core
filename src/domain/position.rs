//! Position: the current state of a holding in a portfolio. Derived from
//! events; not directly constructible.
//!
//! Implements US4 of `specs/016-implement-domain-model/spec.md`.

pub mod entity;
pub mod materialize;
pub mod read;
pub mod replay;

#[cfg(test)]
mod tests;

pub use entity::Position;
pub use read::{get_position, list_positions_for_asset, list_positions_for_portfolio};
pub use replay::{ReplaySummary, replay_events};
