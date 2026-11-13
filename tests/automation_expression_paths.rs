//! Error-path and parsing coverage for the lx_engine expression adapter.

use std::collections::BTreeMap;

use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use obolargus_core::automation::expression::{
    EvalContext, ExpressionError, eval_rule, eval_str, resolve_value,
};
use obolargus_core::automation::runtime::bus::RunMetadata;
use obolargus_core::automation::runtime::context::ContextStore;

fn run() -> RunMetadata {
    RunMetadata {
        id: Uuid::new_v4(),
        flow_id: Uuid::new_v4(),
        flow_version_id: Uuid::new_v4(),
        trigger_kind: "manual".to_string(),
        trigger_node_id: Some("n1".to_string()),
        started_at: Utc::now(),
        actor_id: None,
        inputs: json!({ "symbol": "AAPL", "rows": 2 }),
    }
}

fn with_context<F: FnOnce(&EvalContext<'_>)>(f: F) {
    let msg = json!({
        "payload": { "report": { "loaded": 7 }, "name": "alpha" },
        "items": [{ "value": 1 }, { "value": 2 }],
        "null_value": null
    });
    let run = run();
    let context = ContextStore::new(
        "flow",
        "global",
        BTreeMap::from([("last".to_string(), json!("previous"))]),
        BTreeMap::from([("seen".to_string(), json!(true))]),
    );
    let ctx = EvalContext {
        msg: &msg,
        inputs: &run.inputs,
        run: &run,
        context: &context,
    };
    f(&ctx);
}

#[test]
fn resolve_value_covers_every_object_form() {
    with_context(|ctx| {
        assert_eq!(resolve_value(&json!(42), ctx).expect("scalar"), json!(42));
        assert_eq!(
            resolve_value(&json!({ "no_kind": true }), ctx).expect("plain object"),
            json!({ "no_kind": true })
        );
        assert_eq!(
            resolve_value(&json!({ "kind": "literal", "value": [1, 2] }), ctx).expect("literal"),
            json!([1, 2])
        );
        assert_eq!(
            resolve_value(&json!({ "kind": "literal" }), ctx).expect("missing literal"),
            serde_json::Value::Null
        );
        assert_eq!(
            resolve_value(
                &json!({ "kind": "reference", "path": "inputs.symbol" }),
                ctx
            )
            .expect("reference"),
            json!("AAPL")
        );
        assert_eq!(
            resolve_value(
                &json!({ "kind": "expression", "expression": "inputs.rows" }),
                ctx
            )
            .expect("string expression"),
            json!(2)
        );
        assert_eq!(
            resolve_value(
                &json!({ "kind": "expression", "expression": { "get": { "selector": "symbol", "from": { "get": { "selector": "inputs" } } } } }),
                ctx
            )
            .expect("rule expression"),
            json!("AAPL")
        );
        assert_eq!(
            resolve_value(
                &json!({ "kind": "template", "template": "n=${inputs.rows}" }),
                ctx
            )
            .expect("template"),
            json!("n=2")
        );
        assert_eq!(
            resolve_value(&json!({ "kind": "mystery", "value": 1 }), ctx).expect("unknown kind"),
            json!({ "kind": "mystery", "value": 1 })
        );

        let reference = resolve_value(&json!({ "kind": "reference" }), ctx).expect_err("no path");
        assert_eq!(reference.code, "node.expression_invalid");
        let expression =
            resolve_value(&json!({ "kind": "expression" }), ctx).expect_err("no expression");
        assert_eq!(expression.code, "node.expression_invalid");
        let template = resolve_value(&json!({ "kind": "template" }), ctx).expect_err("no template");
        assert_eq!(template.code, "node.expression_invalid");
    });
}

#[test]
fn templates_support_escapes_and_value_stringification() {
    with_context(|ctx| {
        assert_eq!(
            eval_str(r"literal \${not_evaluated}", ctx).expect("escaped"),
            json!("literal ${not_evaluated}")
        );
        assert_eq!(
            eval_str(r"backslash \ end ${inputs.rows}", ctx).expect("plain backslash"),
            json!("backslash \\ end 2")
        );
        assert_eq!(
            eval_str(
                "null=[${msg.null_value}] object=[${msg.payload.report}]",
                ctx
            )
            .expect("stringify"),
            json!("null=[] object=[{\"loaded\":7}]")
        );
        let error = eval_str("unterminated ${msg.payload", ctx).expect_err("unterminated");
        assert_eq!(error.code, "expression.parse_error");
    });
}

#[test]
fn path_shorthand_parses_indexes_and_reports_errors() {
    with_context(|ctx| {
        assert_eq!(
            eval_str("msg.items[0][\"value\"]", ctx).expect("integer index"),
            json!(1)
        );
        assert_eq!(
            eval_str(r#"msg["payload"]["name"]"#, ctx).expect("string index"),
            json!("alpha")
        );
        assert_eq!(
            eval_str("flow.last", ctx).expect("flow context"),
            json!("previous")
        );
        assert_eq!(
            eval_str("global.seen", ctx).expect("global context"),
            json!(true)
        );

        for source in [
            "",
            "   ",
            "msg..payload",
            "msg.items[",
            "msg.items[abc]",
            "1msg",
        ] {
            let error = eval_str(source, ctx).expect_err(source);
            assert_eq!(error.code, "expression.parse_error", "{source}");
        }
    });
}

#[test]
fn rule_json_and_unwrap_result_are_supported() {
    with_context(|ctx| {
        let rule =
            json!({ "get": { "selector": "rows", "from": { "get": { "selector": "inputs" } } } });
        assert_eq!(eval_rule(&rule, ctx).expect("rule"), json!(2));

        let array = json!([{ "get": { "selector": "symbol", "from": { "get": { "selector": "inputs" } } } }]);
        assert_eq!(
            eval_str(&array.to_string(), ctx).expect("array rule"),
            json!(["AAPL"])
        );

        let malformed = eval_str("{not json", ctx).expect_err("malformed");
        assert_eq!(malformed.code, "expression.parse_error");
    });
}

#[test]
fn expression_error_display_and_conversion_keep_the_code() {
    let error = ExpressionError {
        code: "expression.parse_error",
        message: "bad".to_string(),
    };
    assert_eq!(error.to_string(), "expression.parse_error: bad");
    let node: obolargus_core::automation::error::NodeExecutionError = error.into();
    assert_eq!(node.code, "expression.parse_error");
    assert_eq!(node.message, "bad");
}
