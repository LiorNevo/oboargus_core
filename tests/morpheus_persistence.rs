//! Persistence tests for the Morpheus chat schema.

use serde_json::json;
use uuid::Uuid;

use obolargus_core::common::tx_pool;
use obolargus_core::morpheus::error::{ErrorCode, MorpheusError};
use obolargus_core::morpheus::persistence;

#[tokio::test]
async fn conversation_turn_and_idempotent_send_round_trip() {
    let Some((pool, _guard)) = tx_pool().await else {
        return;
    };
    let owner = "morpheus-test-owner";
    let conversation = persistence::create_conversation(&pool, owner, Uuid::new_v4())
        .await
        .expect("create conversation");
    assert!(conversation.title.is_empty());

    let (user, agent, turn) = persistence::create_turn(
        &pool,
        owner,
        conversation.id,
        "What is a bitemporal fact?",
        "client-message-1",
    )
    .await
    .expect("create turn");
    assert_eq!(user.role, "user");
    assert_eq!(agent.role, "agent");
    assert_eq!(agent.status, "queued");
    assert_eq!(turn.status, "queued");
    assert_eq!(user.seq + 1, agent.seq);

    let (user_again, agent_again, turn_again) = persistence::create_turn(
        &pool,
        owner,
        conversation.id,
        "What is a bitemporal fact?",
        "client-message-1",
    )
    .await
    .expect("idempotent send");
    assert_eq!(user_again.id, user.id);
    assert_eq!(agent_again.id, agent.id);
    assert_eq!(turn_again.id, turn.id);

    let claimed = persistence::claim_next_turn(&pool, conversation.id)
        .await
        .expect("claim")
        .expect("queued turn");
    assert_eq!(claimed.id, turn.id);
    assert_eq!(claimed.status, "running");
    assert!(
        persistence::claim_next_turn(&pool, conversation.id)
            .await
            .expect("second claim")
            .is_none()
    );

    persistence::append_message_content(&pool, agent.id, "A fact with ")
        .await
        .expect("append");
    persistence::append_message_content(&pool, agent.id, "two time axes.")
        .await
        .expect("append");
    persistence::set_message_status(&pool, agent.id, "complete", None, None)
        .await
        .expect("message status");
    persistence::set_turn_status(&pool, turn.id, "completed", None, None, None)
        .await
        .expect("turn status");

    let (messages, has_more) = persistence::list_messages(&pool, conversation.id, 50)
        .await
        .expect("messages");
    assert!(!has_more);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].content, "A fact with two time axes.");
    assert_eq!(messages[1].status, "complete");

    let (items, total) = persistence::list_conversations(&pool, owner, 20, 0)
        .await
        .expect("list");
    assert_eq!(total, 1);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].title, "What is a bitemporal fact?");

    persistence::delete_conversation(&pool, owner, conversation.id)
        .await
        .expect("delete");
    let (items, total) = persistence::list_conversations(&pool, owner, 20, 0)
        .await
        .expect("list after delete");
    assert_eq!(total, 0);
    assert!(items.is_empty());
}

#[tokio::test]
async fn conversations_are_owner_scoped() {
    let Some((pool, _guard)) = tx_pool().await else {
        return;
    };
    let conversation = persistence::create_conversation(&pool, "owner-a", Uuid::new_v4())
        .await
        .expect("create");
    assert!(
        persistence::get_conversation(&pool, "owner-b", conversation.id)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn failed_turns_can_be_retried_without_duplicating_the_question() {
    let Some((pool, _guard)) = tx_pool().await else {
        return;
    };
    let owner = "morpheus-retry-owner";
    let conversation = persistence::create_conversation(&pool, owner, Uuid::new_v4())
        .await
        .expect("create");
    let (user, _agent, turn) =
        persistence::create_turn(&pool, owner, conversation.id, "Retry me", "retry-client-1")
            .await
            .expect("turn");
    let claimed = persistence::claim_next_turn(&pool, conversation.id)
        .await
        .expect("claim")
        .expect("queued");
    assert_eq!(claimed.id, turn.id);
    persistence::set_turn_status(
        &pool,
        turn.id,
        "failed",
        Some("provider_timeout"),
        Some("timeout"),
        None,
    )
    .await
    .expect("fail");

    let (retry, agent) = persistence::retry_turn(&pool, owner, turn.id)
        .await
        .expect("retry");
    assert_eq!(retry.attempt, 2);
    assert_eq!(retry.user_message_id, user.id);
    assert_eq!(retry.retry_of, Some(turn.id));
    assert_eq!(agent.role, "agent");
    assert_eq!(agent.status, "queued");

    let (messages, _) = persistence::list_messages(&pool, conversation.id, 50)
        .await
        .expect("messages");
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.role == "user")
            .count(),
        1
    );
}

#[tokio::test]
async fn every_persistence_operation_round_trips() {
    let Some((pool, _guard)) = tx_pool().await else {
        return;
    };
    let owner = format!("morpheus-owner-{}", Uuid::new_v4());
    let other_owner = format!("morpheus-other-{}", Uuid::new_v4());
    let client_id = Uuid::new_v4();

    let conversation = persistence::create_conversation(&pool, &owner, client_id)
        .await
        .expect("create");
    let by_client = persistence::get_conversation_by_client(&pool, &owner, client_id)
        .await
        .expect("by client");
    assert_eq!(by_client.id, conversation.id);
    assert!(
        persistence::get_conversation_by_client(&pool, &owner, Uuid::new_v4())
            .await
            .is_err()
    );
    assert_eq!(
        persistence::count_messages(&pool, conversation.id)
            .await
            .expect("count"),
        0
    );
    let (empty, empty_total) = persistence::list_conversations(&pool, &owner, 20, 0)
        .await
        .expect("list empty");
    assert!(empty.is_empty());
    assert_eq!(empty_total, 0);

    let (user, agent, turn) = persistence::create_turn(
        &pool,
        &owner,
        conversation.id,
        "Explain the temporal model",
        "client-message-round-trip",
    )
    .await
    .expect("turn");
    assert_eq!(
        persistence::count_messages(&pool, conversation.id)
            .await
            .expect("count"),
        2
    );
    assert!(
        persistence::create_turn(&pool, &owner, Uuid::new_v4(), "orphan", "orphan-client")
            .await
            .is_err()
    );

    let renamed = persistence::rename_conversation(&pool, &owner, conversation.id, "Renamed")
        .await
        .expect("rename");
    assert_eq!(renamed.title, "Renamed");
    assert!(
        persistence::rename_conversation(&pool, &other_owner, conversation.id, "Nope")
            .await
            .is_err()
    );

    let (latest, has_more) = persistence::list_messages(&pool, conversation.id, 1)
        .await
        .expect("latest");
    assert!(has_more);
    assert_eq!(latest.len(), 1);
    assert_eq!(latest[0].id, agent.id);
    let (none, none_more) = persistence::list_messages(&pool, conversation.id, 0)
        .await
        .expect("none");
    assert!(none_more);
    assert!(none.is_empty());

    let (older, older_more) =
        persistence::list_messages_before(&pool, conversation.id, agent.seq, 50)
            .await
            .expect("before");
    assert!(!older_more);
    assert_eq!(older.len(), 1);
    assert_eq!(older[0].id, user.id);
    let (older_none, older_has_more) =
        persistence::list_messages_before(&pool, conversation.id, agent.seq, 0)
            .await
            .expect("before zero");
    assert!(older_none.is_empty());
    assert!(older_has_more);

    let loaded = persistence::get_message(&pool, conversation.id, user.id)
        .await
        .expect("get message");
    assert_eq!(loaded.content, "Explain the temporal model");
    assert!(
        persistence::get_message(&pool, conversation.id, Uuid::new_v4())
            .await
            .is_err()
    );

    assert_eq!(
        persistence::list_turns(&pool, conversation.id)
            .await
            .expect("turns")
            .len(),
        1
    );
    assert_eq!(
        persistence::get_turn(&pool, turn.id)
            .await
            .expect("turn")
            .id,
        turn.id
    );
    assert!(persistence::get_turn(&pool, Uuid::new_v4()).await.is_err());
    assert_eq!(
        persistence::get_owned_turn(&pool, &owner, turn.id)
            .await
            .expect("owned turn")
            .id,
        turn.id
    );
    assert!(
        persistence::get_owned_turn(&pool, &other_owner, turn.id)
            .await
            .is_err()
    );
    assert_eq!(
        persistence::get_turn_user_message(&pool, turn.id)
            .await
            .expect("user message")
            .id,
        user.id
    );
    assert_eq!(
        persistence::get_turn_agent_message(&pool, turn.id)
            .await
            .expect("agent message")
            .id,
        agent.id
    );
    assert!(
        persistence::get_turn_user_message(&pool, Uuid::new_v4())
            .await
            .is_err()
    );
    assert!(
        persistence::get_turn_agent_message(&pool, Uuid::new_v4())
            .await
            .is_err()
    );

    persistence::increment_turn_provider_calls(&pool, turn.id)
        .await
        .expect("provider calls");
    assert_eq!(
        persistence::get_turn(&pool, turn.id)
            .await
            .expect("turn")
            .provider_calls,
        1
    );

    persistence::set_context_trimmed(&pool, user.id, true)
        .await
        .expect("trimmed");
    assert!(
        persistence::get_message(&pool, conversation.id, user.id)
            .await
            .expect("message")
            .context_trimmed
    );

    persistence::set_message_content(&pool, agent.id, "Rewritten")
        .await
        .expect("content");
    assert_eq!(
        persistence::append_message_content(&pool, agent.id, " answer")
            .await
            .expect("append"),
        "Rewritten answer"
    );
    assert!(
        persistence::append_message_content(&pool, Uuid::new_v4(), "x")
            .await
            .is_err()
    );
    persistence::set_message_status(&pool, agent.id, "streaming", None, None)
        .await
        .expect("streaming");
    assert!(
        persistence::get_message(&pool, conversation.id, agent.id)
            .await
            .expect("message")
            .completed_at
            .is_none()
    );
    persistence::set_message_status(&pool, agent.id, "complete", None, None)
        .await
        .expect("complete");

    let step_one = persistence::insert_step(
        &pool,
        turn.id,
        "reasoning",
        "started",
        "Looking at the question",
        None,
        json!({"hidden": false}),
    )
    .await
    .expect("step one");
    assert_eq!(step_one.seq, 1);
    let step_two = persistence::insert_step(
        &pool,
        turn.id,
        "answer",
        "started",
        "Writing the answer",
        Some(agent.id),
        json!({}),
    )
    .await
    .expect("step two");
    assert_eq!(step_two.seq, 2);
    persistence::finish_step(&pool, step_one.id, "completed", "Understood")
        .await
        .expect("finish step");
    let steps = persistence::list_steps(&pool, turn.id)
        .await
        .expect("steps");
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].status, "completed");
    assert_eq!(steps[1].ref_id, Some(agent.id));

    let tool_call = persistence::insert_tool_call(
        &pool,
        turn.id,
        "call-1",
        "assets.search",
        1,
        json!({"query": "AAPL"}),
        "query=AAPL",
        "read",
        "proposed",
    )
    .await
    .expect("tool call");
    assert_eq!(
        persistence::get_tool_call(&pool, turn.id, tool_call.id)
            .await
            .expect("get tool call")
            .id,
        tool_call.id
    );
    assert!(
        persistence::get_tool_call(&pool, Uuid::new_v4(), tool_call.id)
            .await
            .is_err()
    );
    assert_eq!(
        persistence::list_tool_calls(&pool, turn.id)
            .await
            .expect("tool calls")
            .len(),
        1
    );
    persistence::start_tool_call(&pool, tool_call.id)
        .await
        .expect("start tool");
    persistence::finish_tool_call(
        &pool,
        tool_call.id,
        "succeeded",
        Some("one result"),
        true,
        None,
        None,
    )
    .await
    .expect("finish tool");
    let finished_tool = persistence::get_tool_call(&pool, turn.id, tool_call.id)
        .await
        .expect("finished tool");
    assert_eq!(finished_tool.status, "succeeded");
    assert!(finished_tool.result_truncated);
    assert!(finished_tool.started_at.is_some());
    assert!(finished_tool.completed_at.is_some());
    persistence::set_tool_call_status(&pool, tool_call.id, "failed")
        .await
        .expect("set tool status");
    assert_eq!(
        persistence::get_tool_call(&pool, turn.id, tool_call.id)
            .await
            .expect("tool")
            .status,
        "failed"
    );

    let action_id = Uuid::new_v4();
    let approval = persistence::create_approval(&pool, turn.id, "tool", action_id, "hash", 60)
        .await
        .expect("approval");
    let same = persistence::create_approval(&pool, turn.id, "tool", action_id, "other", 60)
        .await
        .expect("idempotent approval");
    assert_eq!(same.id, approval.id);
    assert_eq!(
        persistence::get_approval(&pool, approval.id)
            .await
            .expect("get approval")
            .id,
        approval.id
    );
    assert!(
        persistence::get_approval(&pool, Uuid::new_v4())
            .await
            .is_err()
    );
    assert_eq!(
        persistence::get_approval_for_action(&pool, "tool", action_id)
            .await
            .expect("for action")
            .expect("present")
            .id,
        approval.id
    );
    assert!(
        persistence::get_approval_for_action(&pool, "ui_action", action_id)
            .await
            .expect("for action")
            .is_none()
    );
    assert_eq!(
        persistence::get_owned_approval(&pool, &owner, approval.id)
            .await
            .expect("owned approval")
            .id,
        approval.id
    );
    assert!(
        persistence::get_owned_approval(&pool, &other_owner, approval.id)
            .await
            .is_err()
    );
    persistence::set_tool_call_approval(&pool, tool_call.id, approval.id)
        .await
        .expect("link approval");
    assert_eq!(
        persistence::get_tool_call(&pool, turn.id, tool_call.id)
            .await
            .expect("tool")
            .approval_id,
        Some(approval.id)
    );
    assert_eq!(
        persistence::list_pending_approvals(&pool, turn.id)
            .await
            .expect("pending")
            .len(),
        1
    );
    persistence::decide_approval(&pool, approval.id, "approved", "individual")
        .await
        .expect("decide");
    let decided = persistence::get_approval(&pool, approval.id)
        .await
        .expect("decided");
    assert_eq!(decided.status, "approved");
    assert_eq!(decided.decision_mode.as_deref(), Some("individual"));
    assert!(decided.decided_at.is_some());
    assert!(
        persistence::list_pending_approvals(&pool, turn.id)
            .await
            .expect("pending")
            .is_empty()
    );

    let second_action = Uuid::new_v4();
    let second_approval =
        persistence::create_approval(&pool, turn.id, "ui_action", second_action, "hash2", 60)
            .await
            .expect("second approval");
    persistence::approve_pending_turn_approvals(&pool, turn.id, "turn")
        .await
        .expect("approve turn");
    assert_eq!(
        persistence::get_approval(&pool, second_approval.id)
            .await
            .expect("second approval")
            .status,
        "approved"
    );

    let pending_tool = persistence::insert_tool_call(
        &pool,
        turn.id,
        "call-2",
        "ui.click",
        1,
        json!({}),
        "",
        "write",
        "awaiting_approval",
    )
    .await
    .expect("pending tool");
    persistence::approve_pending_turn_tools(&pool, turn.id)
        .await
        .expect("approve tools");
    assert_eq!(
        persistence::get_tool_call(&pool, turn.id, pending_tool.id)
            .await
            .expect("pending tool")
            .status,
        "approved"
    );
    persistence::set_approve_all_turn(&pool, turn.id, true)
        .await
        .expect("approve all");
    assert!(
        persistence::get_turn(&pool, turn.id)
            .await
            .expect("turn")
            .approve_all_turn
    );
    assert!(
        persistence::list_ui_actions(&pool, turn.id)
            .await
            .expect("ui actions")
            .is_empty()
    );

    assert!(
        persistence::get_active_standing(&pool, &owner)
            .await
            .expect("standing")
            .is_none()
    );
    let standing = persistence::upsert_standing(
        &pool,
        &owner,
        json!({"assets.search": "abc"}),
        &["click".to_string()],
    )
    .await
    .expect("upsert standing");
    let updated = persistence::upsert_standing(
        &pool,
        &owner,
        json!({"assets.search": "def"}),
        &["click".to_string(), "submit".to_string()],
    )
    .await
    .expect("update standing");
    assert_eq!(updated.id, standing.id);
    assert_eq!(updated.tool_grants["assets.search"], "def");
    assert_eq!(updated.ui_action_kinds.len(), 2);
    assert!(
        persistence::get_active_standing(&pool, &owner)
            .await
            .expect("active standing")
            .is_some()
    );
    assert!(
        persistence::list_standing_history(&pool, &owner)
            .await
            .expect("history")
            .is_empty()
    );
    persistence::revoke_standing(&pool, &owner, standing.id)
        .await
        .expect("revoke");
    assert!(
        persistence::revoke_standing(&pool, &owner, standing.id)
            .await
            .is_err()
    );
    assert!(
        persistence::get_active_standing(&pool, &owner)
            .await
            .expect("active standing")
            .is_none()
    );
    assert_eq!(
        persistence::list_standing_history(&pool, &owner)
            .await
            .expect("history")
            .len(),
        1
    );

    assert!(
        !persistence::has_active_turn(&pool, conversation.id)
            .await
            .expect("active")
    );
    assert_eq!(
        persistence::queued_turn_count(&pool, conversation.id)
            .await
            .expect("queued"),
        1
    );
    assert!(persistence::has_queued_or_active(&pool).await.expect("any"));
    let claimed = persistence::claim_next_turn(&pool, conversation.id)
        .await
        .expect("claim")
        .expect("queued turn");
    assert_eq!(claimed.id, turn.id);
    assert!(
        persistence::has_active_turn(&pool, conversation.id)
            .await
            .expect("active")
    );
    assert_eq!(
        persistence::queued_turn_count(&pool, conversation.id)
            .await
            .expect("queued"),
        0
    );
    assert!(
        persistence::claim_next_turn(&pool, conversation.id)
            .await
            .expect("claim")
            .is_none()
    );

    persistence::set_message_status(&pool, agent.id, "streaming", None, None)
        .await
        .expect("stream again");
    persistence::mark_interrupted(&pool)
        .await
        .expect("interrupt");
    let interrupted = persistence::get_turn(&pool, turn.id)
        .await
        .expect("interrupted");
    assert_eq!(interrupted.status, "failed");
    assert_eq!(interrupted.error_code.as_deref(), Some("interrupted"));
    assert!(interrupted.ended_at.is_some());
    assert_eq!(
        persistence::get_message(&pool, conversation.id, agent.id)
            .await
            .expect("interrupted message")
            .status,
        "failed"
    );
    assert_eq!(
        persistence::mark_interrupted(&pool).await.expect("no-op"),
        0
    );

    persistence::set_turn_status(
        &pool,
        turn.id,
        "completed",
        Some("limit"),
        Some("done"),
        Some("turns"),
    )
    .await
    .expect("turn status");
    let completed = persistence::get_turn(&pool, turn.id)
        .await
        .expect("completed");
    assert_eq!(completed.status, "completed");
    assert_eq!(completed.limit_reached.as_deref(), Some("turns"));
    assert_eq!(completed.error_code.as_deref(), Some("limit"));
    persistence::touch_conversation(&pool, conversation.id)
        .await
        .expect("touch");

    assert_eq!(persistence::truncate_summary("short", 10), "short");
    assert_eq!(persistence::truncate_summary("abcdef", 3), "abc…");
    assert_eq!(persistence::truncate_summary("aébcd", 2), "aé…");
    let now = chrono::Utc::now();
    assert_eq!(persistence::as_utc(now), now);
    assert!(matches!(
        persistence::missing_row("thing"),
        MorpheusError::NotFound(_)
    ));
    assert_eq!(
        persistence::provider_unavailable("down").code(),
        ErrorCode::ProviderUnavailable
    );

    assert!(
        persistence::delete_conversation(&pool, &other_owner, conversation.id)
            .await
            .is_err()
    );
    persistence::delete_conversation(&pool, &owner, conversation.id)
        .await
        .expect("delete");
    assert!(
        persistence::delete_conversation(&pool, &owner, conversation.id)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn retry_rejects_a_turn_that_is_not_failed_and_a_non_latest_attempt() {
    let Some((pool, _guard)) = tx_pool().await else {
        return;
    };
    let owner = format!("morpheus-retry-errors-{}", Uuid::new_v4());
    let conversation = persistence::create_conversation(&pool, &owner, Uuid::new_v4())
        .await
        .expect("create");
    let (_user, _agent, turn) =
        persistence::create_turn(&pool, &owner, conversation.id, "Question", "retry-errors")
            .await
            .expect("turn");

    assert!(
        persistence::retry_turn(&pool, &owner, turn.id)
            .await
            .is_err()
    );
    assert!(
        persistence::retry_turn(&pool, "someone-else", turn.id)
            .await
            .is_err()
    );

    persistence::set_turn_status(
        &pool,
        turn.id,
        "failed",
        Some("provider"),
        Some("down"),
        None,
    )
    .await
    .expect("fail");
    let (first_retry, _) = persistence::retry_turn(&pool, &owner, turn.id)
        .await
        .expect("first retry");
    assert_eq!(first_retry.attempt, 2);
    assert!(
        persistence::retry_turn(&pool, &owner, first_retry.id)
            .await
            .is_err(),
        "a queued attempt cannot be retried"
    );
    persistence::set_turn_status(
        &pool,
        first_retry.id,
        "failed",
        Some("provider"),
        Some("down"),
        None,
    )
    .await
    .expect("fail retry");
    let (second_retry, _) = persistence::retry_turn(&pool, &owner, first_retry.id)
        .await
        .expect("second retry");
    assert_eq!(second_retry.attempt, 3);
    assert!(
        persistence::retry_turn(&pool, &owner, first_retry.id)
            .await
            .is_err(),
        "only the latest attempt may be retried"
    );
}
