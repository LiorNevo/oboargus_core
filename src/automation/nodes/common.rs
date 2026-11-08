//! Shared helpers for built-in node descriptors.

use serde_json::{Value, json};

use crate::automation::nodes::{NodeCategory, NodeTypeDescriptor, PortDescriptor, schema_value};

/// Builds a node descriptor from its type and configuration schema.
#[allow(clippy::too_many_arguments)]
pub fn descriptor<T: schemars::JsonSchema>(
    id: &str,
    version: u32,
    category: NodeCategory,
    display_name: &str,
    description: &str,
    icon: &str,
    input: Option<PortDescriptor>,
    outputs: Vec<PortDescriptor>,
    capabilities: Vec<String>,
    docs: &str,
) -> NodeTypeDescriptor {
    NodeTypeDescriptor {
        id: id.to_string(),
        version,
        category,
        display_name: display_name.to_string(),
        description: description.to_string(),
        icon: icon.to_string(),
        input,
        outputs,
        config_schema: schema_value::<T>(),
        output_schema: json!({ "type": "object" }),
        capabilities,
        deprecated: false,
        docs: docs.to_string(),
    }
}

/// The common `in` port.
pub fn input_port() -> PortDescriptor {
    PortDescriptor {
        name: "in".to_string(),
        label: "Input".to_string(),
    }
}

/// The common `out` port.
pub fn output_port() -> PortDescriptor {
    PortDescriptor {
        name: "out".to_string(),
        label: "Output".to_string(),
    }
}

/// A named output port.
pub fn named_output(name: &str, label: &str) -> PortDescriptor {
    PortDescriptor {
        name: name.to_string(),
        label: label.to_string(),
    }
}

/// Clones a message and replaces its `payload`.
pub fn with_payload(mut msg: Value, payload: Value) -> Value {
    if let Value::Object(map) = &mut msg {
        map.insert("payload".to_string(), payload);
    } else {
        msg = json!({ "payload": payload });
    }
    msg
}
