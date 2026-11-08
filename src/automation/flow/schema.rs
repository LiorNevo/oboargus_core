//! Generated flow JSON Schema.

use serde_json::Value;

use crate::automation::flow::document::FlowDocument;
use crate::automation::nodes::schema_value;

/// Returns the JSON Schema for a flow document.
pub fn flow_schema() -> Value {
    schema_value::<FlowDocument>()
}
