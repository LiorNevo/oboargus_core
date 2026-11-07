//! Deterministic canonical serialization and hashing.

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::data_dictionary::error::DataDictionaryError;

/// Serializes a JSON value with sorted keys, two-space indentation, and one
/// trailing newline.
///
/// Keys are sorted recursively rather than relying on `serde_json::Map`
/// ordering: workspace feature unification can enable `preserve_order`, and
/// canonical bytes must not depend on which crate enabled it.
pub fn canonical_json(value: &Value) -> Result<String, DataDictionaryError> {
    let mut rendered = serde_json::to_string_pretty(&sorted(value)).map_err(|error| {
        DataDictionaryError::Serialization {
            message: error.to_string(),
        }
    })?;
    rendered.push('\n');
    Ok(rendered)
}

fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort();
            let mut sorted_object = serde_json::Map::new();
            for key in keys {
                sorted_object.insert(key.clone(), sorted(&object[key]));
            }
            Value::Object(sorted_object)
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

/// Returns the lowercase SHA-256 hex digest of the canonical bytes.
pub fn sha256(canonical: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
