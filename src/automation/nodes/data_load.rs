//! Data-load node: calls the existing single-date reference-data load.

use async_trait::async_trait;
use chrono::NaiveDate;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::automation::error::NodeExecutionError;
use crate::automation::nodes::common::{descriptor, input_port, output_port};
use crate::automation::nodes::{NodeCategory, NodeContext, NodeOutput, WorkflowNode};
use crate::market_data::load::{IdentifierLoadConfig, run_identifier_load};
use crate::market_data::types::TypedIdentifier;

/// Data-load configuration.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default)]
pub struct DataLoadConfig {
    /// Identifier array or expression object.
    pub identifiers: Value,
    /// ISO date or expression object.
    pub as_of_date: Value,
    /// Preferred source id.
    pub preferred_source: Option<String>,
    /// Create missing assets.
    pub create_missing_assets: bool,
    /// Dry run.
    pub dry_run: bool,
    /// Default currency for created assets.
    pub default_currency: String,
}

impl Default for DataLoadConfig {
    fn default() -> Self {
        Self {
            identifiers: Value::Array(Vec::new()),
            as_of_date: Value::String(String::new()),
            preferred_source: None,
            create_missing_assets: false,
            dry_run: false,
            default_currency: "USD".to_string(),
        }
    }
}

/// The data-load node.
pub struct DataLoadNode;

#[async_trait]
impl WorkflowNode for DataLoadNode {
    fn descriptor(&self) -> crate::automation::nodes::NodeTypeDescriptor {
        descriptor::<DataLoadConfig>(
            "data_load",
            1,
            NodeCategory::Data,
            "Load reference data",
            "Loads reference data for a batch of identifiers on one date.",
            "database",
            Some(input_port()),
            vec![output_port()],
            vec![
                "read:market_data".to_string(),
                "write:reference_data".to_string(),
            ],
            "Calls the existing single-date load operation. Per-identifier failures are outcomes, not node failures.",
        )
    }

    async fn execute(&self, ctx: NodeContext<'_>) -> Result<NodeOutput, NodeExecutionError> {
        let config: DataLoadConfig = serde_json::from_value(ctx.config.clone())
            .map_err(|error| NodeExecutionError::new("node.config_invalid", error.to_string()))?;
        let identifiers = parse_identifiers(&ctx.resolve(&config.identifiers)?)?;
        let date = parse_date(&ctx.resolve(&config.as_of_date)?)?;
        let report = run_identifier_load(
            ctx.pool,
            ctx.market_data,
            IdentifierLoadConfig {
                identifiers,
                as_of_date: date,
                preferred_source: config.preferred_source,
                trigger: "automation".to_string(),
                dry_run: config.dry_run,
                create_missing_assets: config.create_missing_assets,
                default_currency: config.default_currency,
            },
        )
        .await
        .map_err(map_load_error)?;
        let payload = json!({
            "report": report,
            "status": report.status,
            "counters": {
                "loaded": report.loaded,
                "no_op": report.no_op,
                "not_resolved": report.not_resolved,
                "no_data_for_date": report.no_data_for_date,
                "unsupported_scheme": report.unsupported_scheme,
                "rejected": report.rejected,
                "source_unavailable": report.source_unavailable,
                "rate_limited": report.rate_limited,
                "storage_failure": report.storage_failure,
            }
        });
        let mut output = NodeOutput::empty();
        output.push(
            "out",
            crate::automation::nodes::common::with_payload(ctx.msg.clone(), payload),
        );
        Ok(output)
    }
}

fn parse_identifiers(value: &Value) -> Result<Vec<TypedIdentifier>, NodeExecutionError> {
    let array = value.as_array().ok_or_else(|| {
        NodeExecutionError::new(
            "node.config_invalid",
            "identifiers must resolve to an array",
        )
    })?;
    array
        .iter()
        .map(|item| {
            let scheme = item.get("scheme").and_then(Value::as_str).ok_or_else(|| {
                NodeExecutionError::new(
                    "node.config_invalid",
                    "each identifier needs a string 'scheme'",
                )
            })?;
            let value = item.get("value").and_then(Value::as_str).ok_or_else(|| {
                NodeExecutionError::new(
                    "node.config_invalid",
                    "each identifier needs a string 'value'",
                )
            })?;
            Ok(TypedIdentifier::new(scheme, value))
        })
        .collect()
}

fn parse_date(value: &Value) -> Result<NaiveDate, NodeExecutionError> {
    let text = value.as_str().ok_or_else(|| {
        NodeExecutionError::new("node.config_invalid", "as_of_date must resolve to a string")
    })?;
    NaiveDate::parse_from_str(text, "%Y-%m-%d").map_err(|error| {
        NodeExecutionError::new(
            "node.config_invalid",
            format!("invalid as_of_date '{text}': {error}"),
        )
    })
}

fn map_load_error(error: crate::market_data::error::MarketDataProviderError) -> NodeExecutionError {
    use crate::market_data::error::MarketDataProviderError;
    match error {
        MarketDataProviderError::Validation(message)
        | MarketDataProviderError::InvalidInput(message) => {
            NodeExecutionError::new("node.config_invalid", message)
        }
        MarketDataProviderError::RateLimited { msg, .. } => {
            NodeExecutionError::retryable("node.provider_unavailable", msg)
        }
        MarketDataProviderError::Transient(message) => {
            NodeExecutionError::retryable("node.provider_unavailable", message)
        }
        MarketDataProviderError::Infrastructure(message) => {
            NodeExecutionError::retryable("node.storage_failure", message)
        }
        MarketDataProviderError::NotFound(message)
        | MarketDataProviderError::Unauthorized(message)
        | MarketDataProviderError::Unsupported(message) => {
            NodeExecutionError::new("node.provider_unavailable", message)
        }
    }
}
