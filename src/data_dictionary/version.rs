//! Schema and model version metadata.

/// The semantic version of the exported data-dictionary schema.
pub const SYSTEM_SCHEMA_VERSION: &str = "1.0.0";

/// The version of the `obolargus-core` crate that produced the schema.
pub fn model_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
