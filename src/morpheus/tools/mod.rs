//! Governed tool catalog.
//!
//! Tools are code-defined contracts. The model may request them, but only the
//! server validates and executes them. Every schema is closed
//! (`additionalProperties: false`), versioned, and risk-classified.

use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::morpheus::error::MorpheusError;
use crate::morpheus::model::ToolRisk;

/// One tool contract.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    /// Stable dotted id.
    pub id: &'static str,
    /// Schema/behavior version.
    pub version: i32,
    /// Human label used in approvals.
    pub display_name: &'static str,
    /// Model-facing description.
    pub description: &'static str,
    /// Risk classification.
    pub risk: ToolRisk,
    /// Closed JSON Schema for the arguments.
    pub parameters: Value,
}

impl ToolSpec {
    /// Stable hash of the id, version, and schema.
    pub fn spec_hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.id.as_bytes());
        hasher.update(self.version.to_string().as_bytes());
        hasher.update(self.parameters.to_string().as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Redacted, one-line argument summary for approval and audit.
    pub fn summarize(&self, arguments: &Value) -> String {
        summarize_arguments(self.id, arguments)
    }
}

/// The active catalog.
#[derive(Debug, Clone)]
pub struct ToolCatalog {
    specs: Vec<ToolSpec>,
}

impl ToolCatalog {
    /// Builds the catalog, optionally filtered by ids.
    pub fn new(enabled: &[String]) -> Result<Self, MorpheusError> {
        let mut specs = default_specs();
        if !enabled.is_empty() {
            specs.retain(|spec| enabled.iter().any(|id| id == spec.id));
            let missing: Vec<&str> = enabled
                .iter()
                .map(String::as_str)
                .filter(|id| !specs.iter().any(|spec| spec.id == *id))
                .collect();
            if !missing.is_empty() {
                return Err(MorpheusError::validation(format!(
                    "unknown tool ids: {}",
                    missing.join(", ")
                )));
            }
        }
        Ok(Self { specs })
    }

    /// All specs.
    pub fn specs(&self) -> &[ToolSpec] {
        &self.specs
    }

    /// Looks up a tool by id.
    pub fn get(&self, id: &str) -> Option<&ToolSpec> {
        self.specs.iter().find(|spec| spec.id == id)
    }

    /// Stable fingerprint over sorted specs.
    pub fn fingerprint(&self) -> String {
        let mut entries: Vec<(String, String)> = self
            .specs
            .iter()
            .map(|spec| (spec.id.to_string(), spec.spec_hash()))
            .collect();
        entries.sort();
        let mut hasher = Sha256::new();
        for (id, hash) in entries {
            hasher.update(id.as_bytes());
            hasher.update(hash.as_bytes());
        }
        format!("sha256:{:x}", hasher.finalize())
    }

    /// Validates arguments and returns the tool spec.
    pub fn validate(&self, id: &str, arguments: &Value) -> Result<&ToolSpec, MorpheusError> {
        let spec = self
            .get(id)
            .ok_or_else(|| MorpheusError::validation(format!("tool not permitted: {id}")))?;
        let validator = jsonschema::validator_for(&spec.parameters).map_err(|error| {
            MorpheusError::tool(
                crate::morpheus::error::ErrorCode::ToolInvalidArguments,
                format!("invalid tool schema for {id}: {error}"),
            )
        })?;
        if !validator.is_valid(arguments) {
            let detail = validator
                .iter_errors(arguments)
                .next()
                .map(|error| error.to_string())
                .unwrap_or_else(|| "arguments do not match the schema".to_string());
            return Err(MorpheusError::tool(
                crate::morpheus::error::ErrorCode::ToolInvalidArguments,
                format!("{id}: {detail}"),
            ));
        }
        Ok(spec)
    }
}

fn summarize_arguments(tool_id: &str, arguments: &Value) -> String {
    let pick = |key: &str| {
        arguments
            .get(key)
            .map(|value| match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            })
            .unwrap_or_default()
    };
    match tool_id {
        "assets.search" => format!("query: {}", pick("query")),
        "market_data.lookup" | "market_data.series" => {
            format!(
                "identifier: {}, range: {}",
                pick("identifier"),
                pick("range")
            )
        }
        "data_load.history" => {
            let mut parts = vec![format!("limit: {}", pick("limit"))];
            for key in ["domain", "source", "status", "entry_type", "from", "to"] {
                let value = pick(key);
                if !value.is_empty() {
                    parts.push(format!("{key}: {value}"));
                }
            }
            parts.join(", ")
        }
        "market_data.load" => format!("identifier: {}, date: {}", pick("identifier"), pick("date")),
        "temporal.fact.append" => format!(
            "fact: {}, valid time: {}",
            pick("fact_id"),
            pick("valid_time")
        ),
        _ => "action requested".to_string(),
    }
}

fn closed(properties: Value, required: Vec<&str>) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn default_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            id: "assets.search",
            version: 1,
            display_name: "Search instruments",
            description: "Search the instrument catalog by name or identifier.",
            risk: ToolRisk::Read,
            parameters: closed(
                json!({
                    "query": { "type": "string", "minLength": 1, "maxLength": 120 },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
                }),
                vec!["query"],
            ),
        },
        ToolSpec {
            id: "market_data.lookup",
            version: 1,
            display_name: "Look up market data",
            description: "Read the latest price and metadata for one instrument.",
            risk: ToolRisk::Read,
            parameters: closed(
                json!({
                    "identifier": { "type": "string", "minLength": 1, "maxLength": 64 },
                    "range": { "type": "string", "enum": ["1d", "5d", "1mo", "3mo", "1y"] }
                }),
                vec!["identifier"],
            ),
        },
        ToolSpec {
            id: "market_data.series",
            version: 1,
            display_name: "Read a price series",
            description: "Read a bounded date range of prices for an instrument.",
            risk: ToolRisk::Read,
            parameters: closed(
                json!({
                    "identifier": { "type": "string", "minLength": 1, "maxLength": 64 },
                    "from": { "type": "string", "format": "date" },
                    "to": { "type": "string", "format": "date" }
                }),
                vec!["identifier", "from", "to"],
            ),
        },
        ToolSpec {
            id: "data_load.history",
            version: 1,
            display_name: "Read load history",
            description: "List recent data-load runs and their outcomes.",
            risk: ToolRisk::Read,
            parameters: closed(
                json!({
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50 },
                    "domain": { "type": "string", "maxLength": 64 },
                    "source": { "type": "string", "maxLength": 120 },
                    "status": { "type": "string", "enum": ["complete", "partial", "failed", "running", "incomplete"] },
                    "entry_type": { "type": "string", "enum": ["load_run", "range_run", "source_change", "run"] },
                    "from": { "type": "string", "format": "date" },
                    "to": { "type": "string", "format": "date" }
                }),
                vec![],
            ),
        },
        ToolSpec {
            id: "market_data.load",
            version: 1,
            display_name: "Load market data",
            description: "Load prices for one instrument and date.",
            risk: ToolRisk::Write,
            parameters: closed(
                json!({
                    "identifier": { "type": "string", "minLength": 1, "maxLength": 64 },
                    "date": { "type": "string", "format": "date" }
                }),
                vec!["identifier", "date"],
            ),
        },
        ToolSpec {
            id: "temporal.fact.append",
            version: 1,
            display_name: "Append a fact value",
            description: "Append a corrected value to a temporal fact.",
            risk: ToolRisk::Write,
            parameters: closed(
                json!({
                    "fact_id": { "type": "string", "minLength": 1, "maxLength": 120 },
                    "valid_time": { "type": "string", "format": "date-time" },
                    "value": { "type": "string", "maxLength": 4000 }
                }),
                vec!["fact_id", "valid_time", "value"],
            ),
        },
    ]
}

/// Wire descriptor for `GET /tools`.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDescriptor {
    /// Tool id.
    pub id: &'static str,
    /// Version.
    pub version: i32,
    /// Human label.
    pub display_name: &'static str,
    /// Description.
    pub description: &'static str,
    /// Risk.
    pub risk: &'static str,
}

impl From<&ToolSpec> for ToolDescriptor {
    fn from(spec: &ToolSpec) -> Self {
        Self {
            id: spec.id,
            version: spec.version,
            display_name: spec.display_name,
            description: spec.description,
            risk: spec.risk.as_str(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_tools_and_extra_fields() {
        let catalog = ToolCatalog::new(&[]).expect("catalog");
        assert!(catalog.validate("nope", &json!({ "query": "x" })).is_err());
        assert!(
            catalog
                .validate("assets.search", &json!({ "query": "x", "extra": 1 }))
                .is_err()
        );
        assert!(
            catalog
                .validate("assets.search", &json!({ "query": "x" }))
                .is_ok()
        );
    }

    #[test]
    fn fingerprint_is_stable_and_changes_with_filtering() {
        let all = ToolCatalog::new(&[]).expect("catalog");
        let filtered = ToolCatalog::new(&["assets.search".to_string()]).expect("catalog");
        assert!(all.fingerprint().starts_with("sha256:"));
        assert_ne!(all.fingerprint(), filtered.fingerprint());
    }

    #[test]
    fn unknown_filtered_ids_are_rejected() {
        let error = ToolCatalog::new(&["assets.search".to_string(), "nope".to_string()])
            .expect_err("unknown id");
        assert!(error.to_string().contains("nope"));
    }

    #[test]
    fn lookup_and_spec_hashes_are_stable() {
        let catalog = ToolCatalog::new(&[]).expect("catalog");
        let spec = catalog.get("assets.search").expect("tool");
        assert_eq!(spec.spec_hash(), spec.spec_hash());
        assert!(catalog.get("nope").is_none());
        assert_eq!(catalog.specs().len(), 6);
    }

    #[test]
    fn summaries_redact_by_tool_shape() {
        let catalog = ToolCatalog::new(&[]).expect("catalog");
        let search = catalog.get("assets.search").expect("search");
        assert_eq!(search.summarize(&json!({"query": "AAPL"})), "query: AAPL");
        assert_eq!(search.summarize(&json!({})), "query: ");

        let lookup = catalog.get("market_data.lookup").expect("lookup");
        assert_eq!(
            lookup.summarize(&json!({"identifier": "TICKER:AAPL", "range": "5d"})),
            "identifier: TICKER:AAPL, range: 5d"
        );

        let series = catalog.get("market_data.series").expect("series");
        assert!(
            series
                .summarize(&json!({"identifier": "AAPL"}))
                .starts_with("identifier: AAPL, range: ")
        );

        let history = catalog.get("data_load.history").expect("history");
        assert_eq!(
            history.summarize(&json!({
                "limit": 10,
                "domain": "market_data",
                "source": "yahoo",
                "status": "complete",
                "entry_type": "load_run",
                "from": "2026-01-01",
                "to": "2026-01-02"
            })),
            "limit: 10, domain: market_data, source: yahoo, status: complete, \
             entry_type: load_run, from: 2026-01-01, to: 2026-01-02"
        );
        assert_eq!(history.summarize(&json!({})), "limit: ");

        let load = catalog.get("market_data.load").expect("load");
        assert_eq!(
            load.summarize(&json!({"identifier": "AAPL", "date": "2026-01-02"})),
            "identifier: AAPL, date: 2026-01-02"
        );

        let append = catalog.get("temporal.fact.append").expect("append");
        assert_eq!(
            append.summarize(&json!({"fact_id": "nav", "valid_time": "2026-01-02T00:00:00Z"})),
            "fact: nav, valid time: 2026-01-02T00:00:00Z"
        );

        let custom = ToolSpec {
            id: "custom.tool",
            version: 1,
            display_name: "Custom",
            description: "Custom tool.",
            risk: ToolRisk::Read,
            parameters: json!({}),
        };
        assert_eq!(custom.summarize(&json!({})), "action requested");
    }

    #[test]
    fn descriptors_expose_wire_risk_names() {
        let catalog = ToolCatalog::new(&[]).expect("catalog");
        let descriptor = ToolDescriptor::from(catalog.get("market_data.load").expect("tool"));
        assert_eq!(descriptor.id, "market_data.load");
        assert_eq!(descriptor.risk, "write");
        assert_eq!(descriptor.version, 1);
        assert!(json!(descriptor).get("display_name").is_some());
    }

    #[test]
    fn required_and_range_constraints_are_enforced() {
        let catalog = ToolCatalog::new(&[]).expect("catalog");
        assert!(catalog.validate("assets.search", &json!({})).is_err());
        assert!(
            catalog
                .validate("assets.search", &json!({"query": "x", "limit": 0}))
                .is_err()
        );
        assert!(
            catalog
                .validate(
                    "market_data.lookup",
                    &json!({"identifier": "AAPL", "range": "2d"})
                )
                .is_err()
        );
        assert!(
            catalog
                .validate("temporal.fact.append", &json!({"fact_id": "nav"}))
                .is_err()
        );
    }
}
