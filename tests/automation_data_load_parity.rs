//! Parity between the automation `data_load` node and a direct dry-run load.
//!
//! A dry run performs the same validation, de-duplication, and report
//! construction without touching the database, so the test can compare the
//! node's payload with the report returned by `run_identifier_load` directly.

use chrono::NaiveDate;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use obolargus_core::automation::flow::document::FlowDocument;
use obolargus_core::automation::model::RunStatus;
use obolargus_core::automation::nodes::{NodeOutput, default_registry};
use obolargus_core::automation::runtime::bus::RunMetadata;
use obolargus_core::automation::runtime::executor::{
    ExecutionObserver, RunExecutionConfig, execute_flow,
};
use obolargus_core::market_data::load::{IdentifierLoadConfig, run_identifier_load};
use obolargus_core::market_data::registry::DataSourceRegistry;
use obolargus_core::market_data::types::TypedIdentifier;

#[derive(Default)]
struct PayloadCollector {
    payload: std::sync::Mutex<Option<Value>>,
}

#[async_trait::async_trait]
impl ExecutionObserver for PayloadCollector {
    async fn node_started(&self, _node_id: &str, _attempt: u32, _message: &Value) {}

    async fn node_finished(
        &self,
        _node_id: &str,
        _attempt: u32,
        _output: Option<&NodeOutput>,
        _error: Option<&obolargus_core::automation::error::NodeExecutionError>,
    ) {
    }

    async fn message(&self, node_id: &str, _port: &str, message: &Value) {
        if node_id == "n2"
            && let Ok(mut guard) = self.payload.lock()
        {
            *guard = Some(message.clone());
        }
    }
}

#[tokio::test]
async fn data_load_node_matches_a_direct_dry_run() {
    let identifiers = vec![
        TypedIdentifier::new("TICKER", "XNAS:AAPL"),
        TypedIdentifier::new("TICKER", "XNAS:AAPL"),
        TypedIdentifier::new("TICKER", "XNAS:MSFT"),
    ];
    let date = NaiveDate::from_ymd_opt(2026, 1, 2).expect("date");
    let flow: FlowDocument = serde_json::from_value(json!({
        "schema_version": 1,
        "name": "parity",
        "nodes": [
            { "id": "n1", "type": "inject", "type_version": 1, "config": { "repeat": "none", "once": true } },
            {
                "id": "n2",
                "type": "data_load",
                "type_version": 1,
                "config": {
                    "identifiers": [
                        { "scheme": "TICKER", "value": "XNAS:AAPL" },
                        { "scheme": "TICKER", "value": "XNAS:AAPL" },
                        { "scheme": "TICKER", "value": "XNAS:MSFT" }
                    ],
                    "as_of_date": "2026-01-02",
                    "dry_run": true,
                    "create_missing_assets": false,
                    "default_currency": "USD"
                }
            }
        ],
        "wires": [
            { "id": "w1", "source": "n1", "source_port": "out", "target": "n2", "target_port": "in" }
        ]
    }))
    .expect("flow");
    let registry = default_registry();
    let market_data = DataSourceRegistry::new(Vec::new()).expect("registry");
    let run = RunMetadata {
        id: Uuid::new_v4(),
        flow_id: Uuid::new_v4(),
        flow_version_id: Uuid::new_v4(),
        trigger_kind: "manual".to_string(),
        trigger_node_id: None,
        started_at: chrono::Utc::now(),
        actor_id: Some("tester".to_string()),
        inputs: json!({}),
    };
    let collector = PayloadCollector::default();
    let outcome = execute_flow(
        &flow,
        &registry,
        run,
        None,
        &market_data,
        CancellationToken::new(),
        RunExecutionConfig::default(),
        &collector,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Succeeded, "{:?}", outcome.error);
    let message = collector
        .payload
        .lock()
        .expect("payload")
        .clone()
        .expect("data_load message");
    let payload = message.get("payload").cloned().expect("payload");

    let direct = run_identifier_load(
        None,
        &market_data,
        IdentifierLoadConfig {
            identifiers,
            as_of_date: date,
            preferred_source: None,
            trigger: "automation".to_string(),
            dry_run: true,
            create_missing_assets: false,
            default_currency: "USD".to_string(),
        },
    )
    .await
    .expect("direct load");

    assert_eq!(payload["report"]["status"], json!(direct.status));
    assert_eq!(
        payload["report"]["identifiers_requested_raw"],
        json!(direct.identifiers_requested_raw)
    );
    assert_eq!(
        payload["report"]["identifiers_duplicated"],
        json!(direct.identifiers_duplicated)
    );
    assert_eq!(
        payload["report"]["identifiers_attempted"],
        json!(direct.identifiers_attempted)
    );
    assert_eq!(payload["counters"]["loaded"], json!(direct.loaded));
    assert_eq!(
        payload["counters"]["not_resolved"],
        json!(direct.not_resolved)
    );
    assert_eq!(
        payload["report"]["identifiers"],
        serde_json::to_value(&direct.identifiers).expect("identifiers")
    );
}

#[tokio::test]
async fn malformed_identifier_config_fails_the_node() {
    let flow: FlowDocument = serde_json::from_value(json!({
        "schema_version": 1,
        "name": "invalid data load",
        "nodes": [
            { "id": "n1", "type": "inject", "type_version": 1, "config": { "repeat": "none", "once": true } },
            {
                "id": "n2",
                "type": "data_load",
                "type_version": 1,
                "config": { "identifiers": "not-an-array", "as_of_date": "2026-01-02", "dry_run": true }
            }
        ],
        "wires": [
            { "id": "w1", "source": "n1", "source_port": "out", "target": "n2", "target_port": "in" }
        ]
    }))
    .expect("flow");
    let market_data = DataSourceRegistry::new(Vec::new()).expect("registry");
    let outcome = execute_flow(
        &flow,
        &default_registry(),
        RunMetadata {
            id: Uuid::new_v4(),
            flow_id: Uuid::new_v4(),
            flow_version_id: Uuid::new_v4(),
            trigger_kind: "manual".to_string(),
            trigger_node_id: None,
            started_at: chrono::Utc::now(),
            actor_id: None,
            inputs: json!({}),
        },
        None,
        &market_data,
        CancellationToken::new(),
        RunExecutionConfig::default(),
        &obolargus_core::automation::runtime::executor::NoopObserver,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.error.expect("error")["code"], "node.config_invalid");
}
