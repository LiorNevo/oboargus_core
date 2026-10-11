//! Event: an immutable record of a state change to a position.
//!
//! Implements US4 of `specs/016-implement-domain-model/spec.md`.

pub mod create;
pub mod entity;
pub mod read;

#[cfg(test)]
mod tests;

pub use create::create_event;
pub use entity::{Event, EventType, NewEvent};
pub use read::{get_event, list_events_for_position};
