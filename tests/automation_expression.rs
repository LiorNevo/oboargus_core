//! Expression adapter tests for the automation engine.

use std::collections::BTreeMap;

use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use obolargus_core::automation::expression::{EvalContext, eval_str, resolve_value};
use obolargus_core::automation::runtime::bus::RunMetadata;
use obolargus_core::automation::runtime::context::ContextStore;

fn run_metadata() -> RunMetadata {
    RunMetadata {
        id: Uuid::new_v4(),
        flow_id: Uuid::new_v4(),
        flow_version_id: Uuid::new_v4(),
        trigger_kind: "manual".to_string(),
        trigger_node_id: None,
        started_at: Utc::now(),
        actor_id: Some("tester".to_string()),
        inputs: json!({ "symbol": "AAPL", "rows": 3 }),
    }
}

#[test]
fn path_shorthand_reads_message_fields() {
    let msg = json!({ "payload": { "report": { "loaded": 7 } } });
    let run = run_metadata();
    let context = ContextStore::new("flow", "global", BTreeMap::new(), BTreeMap::new());
    let ctx = EvalContext {
        msg: &msg,
        inputs: &run.inputs,
        run: &run,
        context: &context,
    };
    assert_eq!(
        eval_str("msg.payload.report.loaded", &ctx).expect("value"),
        json!(7)
    );
}

#[test]
fn path_shorthand_reads_inputs() {
    let msg = json!({});
    let run = run_metadata();
    let context = ContextStore::new("flow", "global", BTreeMap::new(), BTreeMap::new());
    let ctx = EvalContext {
        msg: &msg,
        inputs: &run.inputs,
        run: &run,
        context: &context,
    };
    assert_eq!(
        eval_str("inputs.symbol", &ctx).expect("value"),
        json!("AAPL")
    );
}

#[test]
fn templates_interpolate_values() {
    let msg = json!({ "payload": { "loaded": 5 } });
    let run = run_metadata();
    let context = ContextStore::new("flow", "global", BTreeMap::new(), BTreeMap::new());
    let ctx = EvalContext {
        msg: &msg,
        inputs: &run.inputs,
        run: &run,
        context: &context,
    };
    assert_eq!(
        eval_str("Loaded ${msg.payload.loaded} rows", &ctx).expect("value"),
        json!("Loaded 5 rows")
    );
}

#[test]
fn rule_json_is_evaluated_by_lx_engine() {
    let msg = json!({ "payload": { "a": 2, "b": 3 } });
    let run = run_metadata();
    let context = ContextStore::new("flow", "global", BTreeMap::new(), BTreeMap::new());
    let ctx = EvalContext {
        msg: &msg,
        inputs: &run.inputs,
        run: &run,
        context: &context,
    };
    let value = eval_str(
        r#"{"add": [{"get": {"selector": "a", "from": {"get": {"selector": "payload", "from": {"get": {"selector": "msg"}}}}}}, 1]}"#,
        &ctx,
    );
    assert!(value.is_ok(), "rule should evaluate: {value:?}");
}

#[test]
fn reference_objects_resolve_to_inputs() {
    let msg = json!({});
    let run = run_metadata();
    let context = ContextStore::new("flow", "global", BTreeMap::new(), BTreeMap::new());
    let ctx = EvalContext {
        msg: &msg,
        inputs: &run.inputs,
        run: &run,
        context: &context,
    };
    let value = resolve_value(
        &json!({ "kind": "reference", "path": "inputs.symbol" }),
        &ctx,
    )
    .expect("reference");
    assert_eq!(value, json!("AAPL"));
}

#[test]
fn literals_pass_through_resolution() {
    let msg = json!({});
    let run = run_metadata();
    let context = ContextStore::new("flow", "global", BTreeMap::new(), BTreeMap::new());
    let ctx = EvalContext {
        msg: &msg,
        inputs: &run.inputs,
        run: &run,
        context: &context,
    };
    assert_eq!(
        resolve_value(&json!({ "nested": [1, 2, 3] }), &ctx).expect("literal"),
        json!({ "nested": [1, 2, 3] })
    );
}

#[test]
fn oversized_expression_is_rejected() {
    let msg = json!({});
    let run = run_metadata();
    let context = ContextStore::new("flow", "global", BTreeMap::new(), BTreeMap::new());
    let ctx = EvalContext {
        msg: &msg,
        inputs: &run.inputs,
        run: &run,
        context: &context,
    };
    let source = "x".repeat(5_000);
    let error = eval_str(&source, &ctx).expect_err("limit");
    assert_eq!(error.code, "expression.limit_exceeded");
}

#[test]
fn empty_expression_is_rejected() {
    let msg = json!({});
    let run = run_metadata();
    let context = ContextStore::new("flow", "global", BTreeMap::new(), BTreeMap::new());
    let ctx = EvalContext {
        msg: &msg,
        inputs: &run.inputs,
        run: &run,
        context: &context,
    };
    let error = eval_str("   ", &ctx).expect_err("empty");
    assert_eq!(error.code, "expression.parse_error");
}
