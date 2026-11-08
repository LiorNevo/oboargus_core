//! Data-load range node: calls the existing range load operation.

use async_trait::async_trait;
use chrono::NaiveDate;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::automation::error::NodeExecutionError;
use crate::automation::nodes::common::{descriptor, input_port, output_port};
use crate::automation::nodes::{NodeCategory, NodeContext, NodeOutput, WorkflowNode};
use crate::market_data::data_domain::DataDomain;
use crate::market_data::load_range::{
    IdentifierRangeLoadConfig, run_identifier_range_load_with_group_cancellable,
};
use crate::market_data::types::TypedIdentifier;

/// Range data-load configuration.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default)]
pub struct DataLoadRangeConfig {
    /// Identifier array or expression object.
    pub identifiers: Value,
    /// First date (inclusive).
    pub from: Value,
    /// Last date (inclusive).
    pub to: Value,
    /// Preferred source id.
    pub preferred_source: Option<String>,
    /// Scheduling window.
    pub concurrency: usize,
    /// Create missing assets.
    pub create_missing_assets: bool,
    /// Dry run.
    pub dry_run: bool,
    /// Default currency for created assets.
    pub default_currency: String,
}

impl Default for DataLoadRangeConfig {
    fn default() -> Self {
        Self {
            identifiers: Value::Array(Vec::new()),
            from: Value::String(String::new()),
            to: Value::String(String::new()),
            preferred_source: None,
            concurrency: 4,
            create_missing_assets: false,
            dry_run: false,
            default_currency: "USD".to_string(),
        }
    }
}

/// The range data-load node.
pub struct DataLoadRangeNode;

#[async_trait]
impl WorkflowNode for DataLoadRangeNode {
    fn descriptor(&self) -> crate::automation::nodes::NodeTypeDescriptor {
        descriptor::<DataLoadRangeConfig>(
            "data_load_range",
            1,
            NodeCategory::Data,
            "Load reference data range",
            "Loads reference data for a batch of identifiers over a date range.",
            "date_range",
            Some(input_port()),
            vec![output_port()],
            vec![
                "read:market_data".to_string(),
                "write:reference_data".to_string(),
            ],
            "Calls the existing range load operation with one registry snapshot and per-date cancellation.",
        )
    }

    async fn execute(&self, ctx: NodeContext<'_>) -> Result<NodeOutput, NodeExecutionError> {
        let config: DataLoadRangeConfig = serde_json::from_value(ctx.config.clone())
            .map_err(|error| NodeExecutionError::new("node.config_invalid", error.to_string()))?;
        let identifiers = parse_identifiers(&ctx.resolve(&config.identifiers)?)?;
        let from = parse_date(&ctx.resolve(&config.from)?, "from")?;
        let to = parse_date(&ctx.resolve(&config.to)?, "to")?;
        let report = run_identifier_range_load_with_group_cancellable(
            ctx.pool,
            ctx.market_data,
            IdentifierRangeLoadConfig {
                identifiers,
                from,
                to,
                preferred_source: config.preferred_source,
                trigger: "automation".to_string(),
                dry_run: config.dry_run,
                concurrency: config.concurrency.clamp(1, 32),
                create_missing_assets: config.create_missing_assets,
                default_currency: config.default_currency,
            },
            Uuid::new_v4(),
            DataDomain::MarketData,
            None,
        )
        .await
        .map_err(map_load_error)?;
        let payload = json!({
            "report": report,
            "status": report.status,
            "totals": report.totals,
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

fn parse_date(value: &Value, field: &str) -> Result<NaiveDate, NodeExecutionError> {
    let text = value.as_str().ok_or_else(|| {
        NodeExecutionError::new(
            "node.config_invalid",
            format!("{field} must resolve to a string"),
        )
    })?;
    NaiveDate::parse_from_str(text, "%Y-%m-%d").map_err(|error| {
        NodeExecutionError::new(
            "node.config_invalid",
            format!("invalid {field} '{text}': {error}"),
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
