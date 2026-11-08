//! Low-code automation flow engine.
//!
//! Domain model, validation, lx_engine-backed expressions, node registry,
//! message runtime, context, subflows, templates, and persistence.

pub mod docs;
pub mod error;
pub mod expression;
pub mod flow;
pub mod model;
pub mod nodes;
pub mod runtime;
pub mod templates;
pub mod triggers;
