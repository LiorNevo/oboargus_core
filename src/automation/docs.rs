//! Code-driven documentation artifacts for the automation catalog.
//!
//! The node-type catalog is generated from the live
//! [`NodeRegistry`](crate::automation::nodes::NodeRegistry), so the
//! checked-in fixture can never drift from the executable descriptors.

use serde_json::Value;

use crate::automation::nodes::{NodeTypeDescriptor, default_registry};

/// Returns the versioned node-type catalog document.
///
/// The shape is stable and used by the generation test that keeps
/// `obolargus_core/schema/automation-node-types.json` current.
pub fn node_types_document() -> Value {
    let registry = default_registry();
    let mut items: Vec<NodeTypeDescriptor> = registry.latest_descriptors();
    items.sort_by(|left, right| left.id.cmp(&right.id));
    serde_json::json!({
        "schema_version": 1,
        "generated_by": "obolargus_core::automation::docs::node_types_document",
        "items": items,
    })
}

/// Serializes the catalog document as pretty-printed JSON with a trailing
/// newline, matching the checked-in fixture.
///
/// Object keys are sorted recursively so the output does not depend on the
/// `serde_json` map implementation or enabled features.
pub fn node_types_json() -> String {
    let canonical = canonicalize(node_types_document());
    let mut text = serde_json::to_string_pretty(&canonical).unwrap_or_else(|_| "{}".to_string());
    text.push('\n');
    text
}

fn canonicalize(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted: Vec<(String, Value)> = map
                .into_iter()
                .map(|(key, value)| (key, canonicalize(value)))
                .collect();
            sorted.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonicalize).collect()),
        other => other,
    }
}
