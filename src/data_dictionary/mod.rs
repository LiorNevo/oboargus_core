//! The system-wide data dictionary exported as one JSON Schema document.
//!
//! The document is generated from [`crate::domain::model_description()`] and
//! [`crate::temporal::catalog`]. It is not a second source of truth: every
//! entity, field, relationship, invariant, fact type, and fact field is
//! projected from those authoritative descriptions.

pub mod canonical;
pub mod compat;
pub mod document;
pub mod entity;
pub mod error;
pub mod fact_type;
pub mod fact_value;
pub mod metadata;
pub mod parity;
pub mod types;
pub mod validate;
pub mod version;

#[cfg(test)]
mod tests;

pub use document::SystemSchema;
pub use error::{DataDictionaryError, ValidationError};
pub use validate::validate_instance;

/// Builds the complete system schema document.
pub fn export_system_schema() -> Result<SystemSchema, DataDictionaryError> {
    SystemSchema::build()
}
