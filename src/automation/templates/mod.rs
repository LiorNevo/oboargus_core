//! First-party flow templates.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A template descriptor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateDescriptor {
    /// Stable template id.
    pub id: String,
    /// Template version.
    pub version: u32,
    /// Display name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Category.
    pub category: String,
    /// Node count.
    pub node_count: usize,
    /// Canonical flow document.
    pub flow_json: Value,
}

/// Returns the first-party templates.
pub fn templates() -> Vec<TemplateDescriptor> {
    vec![data_load::template()]
}

/// Looks up a template by id.
pub fn get_template(id: &str) -> Option<TemplateDescriptor> {
    templates().into_iter().find(|template| template.id == id)
}

/// The data-loading template.
pub mod data_load {
    use super::TemplateDescriptor;
    use serde_json::json;

    /// Builds the single-date data-loading template.
    pub fn template() -> TemplateDescriptor {
        TemplateDescriptor {
            id: "data-load-single-date".to_string(),
            version: 1,
            name: "Load reference data for one date".to_string(),
            description: "Load a batch of identifiers for a chosen date.".to_string(),
            category: "data".to_string(),
            node_count: 3,
            flow_json: json!({
                "schema_version": 1,
                "name": "Load reference data for one date",
                "description": "Load a batch of identifiers for a chosen date.",
                "tags": ["market-data"],
                "inputs": [
                    {
                        "name": "identifiers",
                        "type": "array",
                        "required": true,
                        "secret": false,
                        "description": "Typed identifiers to load"
                    },
                    {
                        "name": "as_of_date",
                        "type": "string",
                        "required": true,
                        "secret": false,
                        "description": "ISO date (YYYY-MM-DD)"
                    }
                ],
                "nodes": [
                    {
                        "id": "n1",
                        "type": "inject",
                        "type_version": 1,
                        "label": "Start",
                        "position": { "x": 80, "y": 120 },
                        "config": { "payload_type": "none", "repeat": "none", "once": true, "enabled": true }
                    },
                    {
                        "id": "n2",
                        "type": "data_load",
                        "type_version": 1,
                        "label": "Load reference data",
                        "position": { "x": 360, "y": 120 },
                        "config": {
                            "identifiers": { "kind": "reference", "path": "inputs.identifiers" },
                            "as_of_date": { "kind": "reference", "path": "inputs.as_of_date" },
                            "preferred_source": null,
                            "create_missing_assets": false,
                            "dry_run": false,
                            "default_currency": "USD"
                        }
                    },
                    {
                        "id": "n3",
                        "type": "debug",
                        "type_version": 1,
                        "label": "Show report",
                        "position": { "x": 640, "y": 120 },
                        "config": { "target": "payload", "to_sidebar": true, "to_log": false }
                    }
                ],
                "wires": [
                    { "id": "w1", "source": "n1", "source_port": "out", "target": "n2", "target_port": "in" },
                    { "id": "w2", "source": "n2", "source_port": "out", "target": "n3", "target_port": "in" }
                ],
                "subflows": []
            }),
        }
    }
}
