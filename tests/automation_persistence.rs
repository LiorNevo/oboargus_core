//! Database-backed coverage for the automation persistence helpers.
//!
//! The HTTP routes and the executor both call these helpers, but the core
//! crate had no direct integration test for them. These tests are DB-gated in
//! the same way as the other core integration suites: when no test database is
//! configured they return early.

use chrono::{Duration, Utc};
use serde_json::json;
use uuid::Uuid;

use obolargus_core::automation::error::AutomationError;
use obolargus_core::automation::runtime::persistence;
use obolargus_core::common::tx_pool;

fn flow_json(name: &str) -> serde_json::Value {
    json!({
        "schema_version": 1,
        "name": name,
        "nodes": [
            { "id": "n1", "type": "inject", "type_version": 1, "config": {} },
            { "id": "n2", "type": "debug", "type_version": 1, "config": {} }
        ],
        "wires": [
            { "id": "w1", "source": "n1", "source_port": "out", "target": "n2", "target_port": "in" }
        ]
    })
}

fn unique_name(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

#[tokio::test]
async fn flow_draft_version_and_metadata_lifecycle() {
    let Some((pool, _guard)) = tx_pool().await else {
        return;
    };

    let owner = "persistence-owner";
    let name = unique_name("flow-lifecycle");
    let flow = persistence::create_flow(
        &pool,
        owner,
        &name,
        "initial description",
        &["alpha".to_string()],
        &flow_json("initial"),
    )
    .await
    .expect("create flow");
    assert_eq!(flow.owner_id, owner);
    assert_eq!(flow.name, name);
    assert_eq!(flow.status, "active");
    assert_eq!(flow.draft_revision, 1);
    assert!(flow.current_draft_id.is_some());

    let duplicate = persistence::create_flow(
        &pool,
        owner,
        &name.to_uppercase(),
        "duplicate",
        &[],
        &flow_json("duplicate"),
    )
    .await;
    assert!(matches!(duplicate, Err(AutomationError::Conflict(_))));

    let (loaded, draft) = persistence::get_flow(&pool, flow.id)
        .await
        .expect("load flow");
    assert_eq!(loaded.id, flow.id);
    let draft = draft.expect("initial draft");
    assert_eq!(draft.status, "draft");
    assert_eq!(draft.version_number, None);
    assert_eq!(draft.flow_json["name"], "initial");

    assert_eq!(
        persistence::list_flows(&pool, None, None, 10, 0)
            .await
            .expect("list all")
            .len(),
        1
    );
    assert_eq!(
        persistence::list_flows(&pool, Some("active"), Some("lifecycle"), 10, 0)
            .await
            .expect("filtered list")
            .len(),
        1
    );
    assert!(
        persistence::list_flows(&pool, Some("archived"), None, 10, 0)
            .await
            .expect("status filter")
            .is_empty()
    );
    assert_eq!(
        persistence::count_flows(&pool, None, None)
            .await
            .expect("count flows"),
        1
    );
    assert_eq!(
        persistence::count_flows(&pool, Some("active"), Some("lifecycle"))
            .await
            .expect("count filtered"),
        1
    );

    let updated = persistence::update_flow_metadata(
        &pool,
        flow.id,
        Some("renamed"),
        Some("updated description"),
        Some(&["beta".to_string()]),
    )
    .await
    .expect("update metadata");
    assert_eq!(updated.name, "renamed");
    assert_eq!(updated.description, "updated description");
    assert_eq!(updated.tags, vec!["beta".to_string()]);

    let other = persistence::create_flow(
        &pool,
        owner,
        &unique_name("other"),
        "",
        &[],
        &flow_json("other"),
    )
    .await
    .expect("other flow");
    let conflict =
        persistence::update_flow_metadata(&pool, other.id, Some("RENAMED"), None, None).await;
    assert!(matches!(conflict, Err(AutomationError::Conflict(_))));

    let archived = persistence::set_flow_status(&pool, flow.id, "archived")
        .await
        .expect("archive flow");
    assert_eq!(archived.status, "archived");
    let reactivated = persistence::set_flow_status(&pool, flow.id, "active")
        .await
        .expect("reactivate flow");
    assert_eq!(reactivated.status, "active");

    let first_save =
        persistence::upsert_draft(&pool, flow.id, Some(1), &flow_json("draft one"), "author")
            .await
            .expect("save draft");
    assert_eq!(first_save.revision, 2);
    assert_eq!(first_save.version.flow_json["name"], "draft one");

    let mismatch =
        persistence::upsert_draft(&pool, flow.id, Some(1), &flow_json("stale"), "author").await;
    assert!(matches!(mismatch, Err(AutomationError::Conflict(_))));

    let second_save =
        persistence::upsert_draft(&pool, flow.id, Some(2), &flow_json("draft two"), "author")
            .await
            .expect("replace draft");
    assert_eq!(second_save.revision, 3);
    assert_eq!(second_save.version.id, first_save.version.id);

    let publish_mismatch = persistence::publish_flow(&pool, flow.id, 2, "stale", "author").await;
    assert!(matches!(
        publish_mismatch,
        Err(AutomationError::Conflict(_))
    ));

    let published = persistence::publish_flow(&pool, flow.id, 3, "first release", "author")
        .await
        .expect("publish");
    assert_eq!(published.version_number, Some(1));
    assert_eq!(published.status, "published");
    assert_eq!(published.change_note.as_deref(), Some("first release"));
    assert_eq!(published.published_by.as_deref(), Some("author"));
    assert!(published.published_at.is_some());

    let by_id = persistence::get_version_by_id(&pool, published.id)
        .await
        .expect("version by id")
        .expect("published exists");
    assert_eq!(by_id.id, published.id);
    let by_number = persistence::get_version(&pool, flow.id, 1)
        .await
        .expect("version by number");
    assert_eq!(by_number.id, published.id);
    assert!(matches!(
        persistence::get_version(&pool, flow.id, 99).await,
        Err(AutomationError::NotFound(_))
    ));
    assert_eq!(
        persistence::list_versions(&pool, flow.id, 10, 0)
            .await
            .expect("list versions")
            .len(),
        1
    );
    assert_eq!(
        persistence::count_versions(&pool, flow.id)
            .await
            .expect("count versions"),
        1
    );

    let archived_version = persistence::archive_version(&pool, flow.id, 1)
        .await
        .expect("archive version");
    assert_eq!(archived_version.status, "archived");
    assert!(matches!(
        persistence::archive_version(&pool, flow.id, 1).await,
        Err(AutomationError::Conflict(_))
    ));

    // Publishing a flow whose draft pointer was cleared reports draft.none.
    let no_draft = persistence::create_flow(
        &pool,
        owner,
        &unique_name("no-draft"),
        "",
        &[],
        &flow_json("no draft"),
    )
    .await
    .expect("no-draft flow");
    sqlx::query("UPDATE automation_flows SET current_draft_id = NULL WHERE id = $1")
        .bind(no_draft.id)
        .execute(&pool)
        .await
        .expect("clear draft pointer");
    assert!(matches!(
        persistence::publish_flow(&pool, no_draft.id, 1, "none", "author").await,
        Err(AutomationError::Conflict(_))
    ));

    persistence::soft_delete_flow(&pool, other.id)
        .await
        .expect("soft delete");
    assert!(matches!(
        persistence::get_flow(&pool, other.id).await,
        Err(AutomationError::NotFound(_))
    ));
    assert!(matches!(
        persistence::soft_delete_flow(&pool, other.id).await,
        Err(AutomationError::NotFound(_))
    ));
    assert!(matches!(
        persistence::soft_delete_flow(&pool, Uuid::new_v4()).await,
        Err(AutomationError::NotFound(_))
    ));
}

#[tokio::test]
async fn trigger_lifecycle_and_scheduling_queries() {
    let Some((pool, _guard)) = tx_pool().await else {
        return;
    };

    let flow = persistence::create_flow(
        &pool,
        "trigger-owner",
        &unique_name("trigger-flow"),
        "",
        &[],
        &flow_json("trigger flow"),
    )
    .await
    .expect("create flow");
    let draft =
        persistence::upsert_draft(&pool, flow.id, Some(1), &flow_json("trigger flow"), "owner")
            .await
            .expect("save draft");
    let published = persistence::publish_flow(&pool, flow.id, draft.revision, "publish", "owner")
        .await
        .expect("publish");

    let now = Utc::now();
    let next = now - Duration::minutes(1);
    let inject = persistence::insert_inject_trigger(
        &pool,
        flow.id,
        published.id,
        "n1",
        "interval",
        Some(60),
        None,
        "UTC",
        next,
        "owner",
    )
    .await
    .expect("insert inject trigger");
    assert_eq!(inject.kind, "inject");
    assert_eq!(inject.flow_version_id, Some(published.id));
    assert_eq!(inject.node_id.as_deref(), Some("n1"));
    assert_eq!(inject.interval_seconds, Some(60));

    assert_eq!(
        persistence::get_trigger(&pool, inject.id)
            .await
            .expect("get trigger")
            .id,
        inject.id
    );
    assert!(matches!(
        persistence::get_trigger(&pool, Uuid::new_v4()).await,
        Err(AutomationError::NotFound(_))
    ));
    assert_eq!(
        persistence::list_triggers(&pool, flow.id)
            .await
            .expect("list triggers")
            .len(),
        1
    );
    let due = persistence::due_triggers(&pool, now, 10)
        .await
        .expect("due triggers");
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].id, inject.id);
    assert_eq!(
        persistence::next_scheduled_run(&pool, flow.id)
            .await
            .expect("next scheduled")
            .map(|instant| instant.timestamp_micros()),
        Some(next.timestamp_micros())
    );

    let last = now;
    let next_at = now + Duration::minutes(5);
    persistence::advance_trigger(&pool, inject.id, last, next_at)
        .await
        .expect("advance trigger");
    let advanced = persistence::get_trigger(&pool, inject.id)
        .await
        .expect("advanced trigger");
    assert_eq!(
        advanced
            .last_occurrence_at
            .map(|instant| instant.timestamp_micros()),
        Some(last.timestamp_micros())
    );
    assert_eq!(
        advanced
            .next_occurrence_at
            .map(|instant| instant.timestamp_micros()),
        Some(next_at.timestamp_micros())
    );

    let api = persistence::upsert_api_trigger(&pool, flow.id, true, "skip", "run_once", "owner")
        .await
        .expect("insert api trigger");
    assert_eq!(api.kind, "api");
    assert!(api.enabled);
    let api_id = api.id;
    let api = persistence::upsert_api_trigger(&pool, flow.id, false, "queue", "skip", "owner")
        .await
        .expect("update api trigger");
    assert_eq!(api.id, api_id);
    assert!(!api.enabled);
    assert_eq!(api.overlap_policy, "queue");
    assert_eq!(api.missed_policy, "skip");

    let updated = persistence::update_trigger(
        &pool,
        api.id,
        Some(true),
        Some("cancel_previous"),
        Some("run_once"),
    )
    .await
    .expect("update trigger");
    assert!(updated.enabled);
    assert_eq!(updated.overlap_policy, "cancel_previous");
    assert_eq!(updated.missed_policy, "run_once");

    persistence::delete_inject_triggers(&pool, flow.id)
        .await
        .expect("delete inject triggers");
    assert!(
        persistence::due_triggers(&pool, now + Duration::days(1), 10)
            .await
            .expect("no due triggers")
            .is_empty()
    );

    persistence::delete_trigger(&pool, api.id)
        .await
        .expect("delete trigger");
    assert!(matches!(
        persistence::delete_trigger(&pool, api.id).await,
        Err(AutomationError::NotFound(_))
    ));
    assert!(
        persistence::list_triggers(&pool, flow.id)
            .await
            .expect("empty triggers")
            .is_empty()
    );
}

#[tokio::test]
async fn run_messages_logs_context_and_audit_lifecycle() {
    let Some((pool, _guard)) = tx_pool().await else {
        return;
    };

    let flow = persistence::create_flow(
        &pool,
        "run-owner",
        &unique_name("run-flow"),
        "run lifecycle",
        &[],
        &flow_json("run flow"),
    )
    .await
    .expect("create flow");
    let draft = persistence::upsert_draft(&pool, flow.id, Some(1), &flow_json("run flow"), "owner")
        .await
        .expect("save draft");
    let version = persistence::publish_flow(&pool, flow.id, draft.revision, "publish", "owner")
        .await
        .expect("publish");

    let inputs = json!({ "ticker": "AAPL" });
    let run = persistence::insert_run(
        &pool,
        flow.id,
        version.id,
        None,
        "manual",
        None,
        Some("owner"),
        &inputs,
        Some("manual-key"),
        None,
    )
    .await
    .expect("insert run");
    assert_eq!(run.status, "queued");
    assert_eq!(run.inputs, inputs);

    let duplicate = persistence::insert_run(
        &pool,
        flow.id,
        version.id,
        None,
        "manual",
        None,
        Some("owner"),
        &inputs,
        Some("manual-key"),
        None,
    )
    .await;
    assert!(matches!(duplicate, Err(AutomationError::Conflict(_))));

    assert_eq!(
        persistence::get_run(&pool, run.id)
            .await
            .expect("get run")
            .id,
        run.id
    );
    assert!(matches!(
        persistence::get_run(&pool, Uuid::new_v4()).await,
        Err(AutomationError::NotFound(_))
    ));
    assert_eq!(
        persistence::list_runs(&pool, Some(flow.id), Some("queued"), Some("manual"), 10, 0)
            .await
            .expect("filtered runs")
            .len(),
        1
    );
    assert_eq!(
        persistence::count_runs(&pool, Some(flow.id), Some("queued"), Some("manual"))
            .await
            .expect("count runs"),
        1
    );
    assert_eq!(
        persistence::count_active_runs(&pool, Some(flow.id))
            .await
            .expect("active runs"),
        1
    );
    assert_eq!(
        persistence::latest_run(&pool, flow.id)
            .await
            .expect("latest run")
            .expect("a run")
            .id,
        run.id
    );
    assert_eq!(
        persistence::find_run_by_idempotency(&pool, flow.id, None, "manual-key")
            .await
            .expect("find by key")
            .expect("run by key")
            .id,
        run.id
    );
    assert!(
        persistence::find_run_by_idempotency(&pool, flow.id, None, "missing")
            .await
            .expect("missing key")
            .is_none()
    );
    assert!(
        persistence::run_exists(&pool, run.id)
            .await
            .expect("run exists")
    );
    assert!(
        !persistence::run_exists(&pool, Uuid::new_v4())
            .await
            .expect("run absent")
    );

    persistence::mark_run_running(&pool, run.id)
        .await
        .expect("mark running");
    assert_eq!(
        persistence::get_run(&pool, run.id)
            .await
            .expect("running run")
            .status,
        "running"
    );
    persistence::heartbeat(&pool, run.id)
        .await
        .expect("heartbeat");

    let parent = persistence::insert_run(
        &pool,
        flow.id,
        version.id,
        None,
        "manual",
        None,
        Some("owner"),
        &json!({}),
        Some("parent-key"),
        None,
    )
    .await
    .expect("parent run");
    persistence::set_run_parent(&pool, run.id, Some(parent.id))
        .await
        .expect("set parent");
    assert_eq!(
        persistence::get_run(&pool, run.id)
            .await
            .expect("child run")
            .parent_run_id,
        Some(parent.id)
    );

    let cancel_requested = persistence::request_cancel(&pool, run.id)
        .await
        .expect("request cancel");
    assert!(cancel_requested.cancel_requested_at.is_some());
    let second_request = persistence::request_cancel(&pool, run.id)
        .await
        .expect("cancel is idempotent");
    assert_eq!(
        second_request.cancel_requested_at,
        cancel_requested.cancel_requested_at
    );

    persistence::finish_run(
        &pool,
        run.id,
        "cancelled",
        &json!({ "done": false }),
        &json!({ "nodes_total": 2 }),
        Some(&json!({ "code": "node.cancelled" })),
        4,
    )
    .await
    .expect("finish run");
    let finished = persistence::get_run(&pool, run.id)
        .await
        .expect("finished run");
    assert_eq!(finished.status, "cancelled");
    assert_eq!(finished.message_count, 4);
    assert!(finished.ended_at.is_some());
    assert!(matches!(
        persistence::request_cancel(&pool, run.id).await,
        Err(AutomationError::Conflict(_))
    ));

    let queued = persistence::insert_run(
        &pool,
        flow.id,
        version.id,
        None,
        "manual",
        None,
        None,
        &json!({}),
        Some("interrupt-queued"),
        None,
    )
    .await
    .expect("queued run");
    let running = persistence::insert_run(
        &pool,
        flow.id,
        version.id,
        None,
        "manual",
        None,
        None,
        &json!({}),
        Some("interrupt-running"),
        None,
    )
    .await
    .expect("running run");
    persistence::mark_run_running(&pool, running.id)
        .await
        .expect("mark interrupt running");
    let interrupted = persistence::mark_interrupted_runs(&pool)
        .await
        .expect("mark interrupted");
    assert!(interrupted >= 2);
    assert_eq!(
        persistence::get_run(&pool, queued.id)
            .await
            .expect("interrupted queued")
            .status,
        "interrupted"
    );
    assert_eq!(
        persistence::get_run(&pool, running.id)
            .await
            .expect("interrupted running")
            .status,
        "interrupted"
    );

    // A trigger-scoped run exercises the trigger idempotency lookup.
    let api = persistence::upsert_api_trigger(&pool, flow.id, true, "skip", "run_once", "owner")
        .await
        .expect("api trigger");
    let trigger_run = persistence::insert_run(
        &pool,
        flow.id,
        version.id,
        Some(api.id),
        "api",
        Some("n1"),
        None,
        &json!({}),
        Some("api-key"),
        None,
    )
    .await
    .expect("trigger run");
    assert_eq!(
        persistence::find_run_by_idempotency(&pool, flow.id, Some(api.id), "api-key")
            .await
            .expect("trigger idempotency")
            .expect("trigger run")
            .id,
        trigger_run.id
    );
    assert_eq!(
        persistence::count_trigger_runs_since(&pool, api.id, Utc::now() - Duration::minutes(5))
            .await
            .expect("trigger run count"),
        1
    );

    persistence::insert_pending_node_runs(
        &pool,
        trigger_run.id,
        &[
            ("n1".to_string(), "inject".to_string(), 1),
            ("n2".to_string(), "debug".to_string(), 1),
        ],
    )
    .await
    .expect("pending node runs");
    persistence::ensure_node_run(&pool, trigger_run.id, "n2", "debug", 1, 2)
        .await
        .expect("ensure retry node");
    persistence::update_node_run(
        &pool,
        trigger_run.id,
        "n1",
        1,
        "succeeded",
        Some(Utc::now()),
        Some(Utc::now()),
        Some(12),
        Some(&json!({ "in": true })),
        Some(&json!([{ "out": true }])),
        false,
        1,
        1,
        None,
    )
    .await
    .expect("update node run");
    persistence::update_node_run(
        &pool,
        trigger_run.id,
        "n2",
        2,
        "failed",
        None,
        None,
        None,
        None,
        None,
        true,
        0,
        0,
        Some(&json!({ "code": "node.failed" })),
    )
    .await
    .expect("update retry node");
    let nodes = persistence::list_node_runs(&pool, trigger_run.id)
        .await
        .expect("list node runs");
    assert_eq!(nodes.len(), 3);
    let n1 = nodes.iter().find(|node| node.node_id == "n1").expect("n1");
    assert_eq!(n1.status, "succeeded");
    assert_eq!(n1.messages_in, 1);
    assert_eq!(n1.messages_out, 1);

    persistence::insert_message(
        &pool,
        trigger_run.id,
        Some("n1"),
        "out",
        Some("out"),
        "msg-1",
        Some(&json!({ "value": 1 })),
        8,
        false,
        None,
    )
    .await
    .expect("insert message");
    persistence::insert_message(
        &pool,
        trigger_run.id,
        None,
        "in",
        None,
        "msg-2",
        None,
        0,
        true,
        Some(&json!({ "code": "message.truncated" })),
    )
    .await
    .expect("insert second message");
    let messages = persistence::list_messages(&pool, trigger_run.id, None, None, None, 10)
        .await
        .expect("list messages");
    assert_eq!(messages.len(), 2);
    let after_first =
        persistence::list_messages(&pool, trigger_run.id, None, None, Some(messages[0].id), 10)
            .await
            .expect("messages after id");
    assert_eq!(after_first.len(), 1);
    assert_eq!(
        persistence::list_messages(&pool, trigger_run.id, Some("n1"), Some("out"), None, 10)
            .await
            .expect("filtered messages")
            .len(),
        1
    );

    persistence::insert_log(
        &pool,
        trigger_run.id,
        Some("n1"),
        "info",
        "started",
        Some(&json!({ "attempt": 1 })),
    )
    .await
    .expect("insert log");
    persistence::insert_log(&pool, trigger_run.id, None, "warn", "global log", None)
        .await
        .expect("insert second log");
    let logs = persistence::list_logs(&pool, trigger_run.id, None, None, 10)
        .await
        .expect("list logs");
    assert_eq!(logs.len(), 2);
    assert_eq!(
        persistence::list_logs(&pool, trigger_run.id, Some("n1"), None, 10)
            .await
            .expect("filtered logs")
            .len(),
        1
    );
    assert_eq!(
        persistence::list_logs(&pool, trigger_run.id, None, Some(logs[0].id), 10)
            .await
            .expect("logs after id")
            .len(),
        1
    );

    let scope = flow.id.to_string();
    persistence::set_context(
        &pool,
        "flow",
        &scope,
        "counter",
        &json!(1),
        Some(trigger_run.id),
    )
    .await
    .expect("set flow context");
    persistence::set_context(
        &pool,
        "flow",
        &format!("{scope}:sub-1"),
        "nested",
        &json!(true),
        None,
    )
    .await
    .expect("set subflow context");
    persistence::set_context(&pool, "global", "shared", "key", &json!("value"), None)
        .await
        .expect("set global context");
    persistence::set_context(&pool, "flow", &scope, "counter", &json!(2), None)
        .await
        .expect("update context");
    let (flow_context, global_context) = persistence::get_context(&pool, &scope, "shared")
        .await
        .expect("get context");
    assert_eq!(flow_context.len(), 1);
    assert_eq!(flow_context[0].value, json!(2));
    assert_eq!(global_context.len(), 1);
    assert_eq!(
        persistence::get_scoped_flow_context(&pool, &scope)
            .await
            .expect("scoped context")
            .len(),
        2
    );
    persistence::delete_context(&pool, "flow", &scope)
        .await
        .expect("delete flow context");
    assert!(
        persistence::get_scoped_flow_context(&pool, &scope)
            .await
            .expect("deleted flow context")
            .is_empty()
    );
    persistence::delete_context(&pool, "global", "shared")
        .await
        .expect("delete global context");
    assert!(
        persistence::get_context(&pool, &scope, "shared")
            .await
            .expect("deleted global context")
            .1
            .is_empty()
    );

    let target = Uuid::new_v4();
    persistence::insert_audit(
        &pool,
        Some("owner"),
        "flow.update",
        "flow",
        target,
        Some(flow.id),
        Some(trigger_run.id),
        json!({ "field": "name" }),
    )
    .await
    .expect("insert audit");
    persistence::insert_audit(
        &pool,
        Some("other"),
        "flow.archive",
        "flow",
        target,
        Some(flow.id),
        None,
        json!({}),
    )
    .await
    .expect("insert second audit");
    let audit = persistence::list_audit(&pool, flow.id, None, None, None, None, 10, 0)
        .await
        .expect("list audit");
    assert!(audit.len() >= 3);
    assert_eq!(
        persistence::list_audit(&pool, flow.id, Some("flow.update"), None, None, None, 10, 0)
            .await
            .expect("action filter")
            .len(),
        1
    );
    assert_eq!(
        persistence::list_audit(
            &pool,
            flow.id,
            None,
            Some("owner"),
            Some(Utc::now() - Duration::minutes(5)),
            Some(Utc::now() + Duration::minutes(5)),
            10,
            0
        )
        .await
        .expect("actor and time filter")
        .len(),
        2
    );
    assert_eq!(
        persistence::list_audit(&pool, flow.id, None, None, None, None, 1, 1)
            .await
            .expect("paginated audit")
            .len(),
        1
    );
    assert_eq!(
        persistence::count_audit(&pool, flow.id, None, None, None, None)
            .await
            .expect("count audit"),
        audit.len() as i64
    );
    assert_eq!(
        persistence::count_audit(&pool, flow.id, Some("flow.update"), None, None, None)
            .await
            .expect("count filtered audit"),
        1
    );
}
