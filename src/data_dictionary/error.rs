//! Errors produced while building or validating the system schema.

use std::error::Error;
use std::fmt;

/// A failure while building the system data-dictionary schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataDictionaryError {
    /// The authoritative fact catalog is empty.
    EmptyCatalog,
    /// Two definitions would use the same name.
    DuplicateDefinition {
        /// The duplicated definition name.
        name: String,
    },
    /// A field is missing metadata required to build its schema.
    MissingMetadata {
        /// The entity or fact type that owns the field.
        entity: String,
        /// The field name.
        field: String,
    },
    /// The document could not be serialized.
    Serialization {
        /// The serialization diagnostic.
        message: String,
    },
}

impl fmt::Display for DataDictionaryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyCatalog => write!(f, "the authoritative fact catalog is empty"),
            Self::DuplicateDefinition { name } => {
                write!(f, "duplicate data-dictionary definition: {name}")
            }
            Self::MissingMetadata { entity, field } => {
                write!(f, "missing metadata for {entity}.{field}")
            }
            Self::Serialization { message } => {
                write!(f, "failed to serialize the system schema: {message}")
            }
        }
    }
}

impl Error for DataDictionaryError {}

/// One violation found while validating an instance against the schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// JSON Pointer to the offending instance location.
    pub pointer: String,
    /// The violated JSON Schema keyword or extension rule.
    pub rule: String,
}

impl ValidationError {
    /// Creates a validation error for a JSON Pointer and rule name.
    pub fn new(pointer: impl Into<String>, rule: impl Into<String>) -> Self {
        Self {
            pointer: pointer.into(),
            rule: rule.into(),
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} violated at {}", self.rule, self.pointer)
    }
}

impl Error for ValidationError {}
