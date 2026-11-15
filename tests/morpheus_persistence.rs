//! Persistence tests for the Morpheus chat schema.

use uuid::Uuid;

use obolargus_core::common::tx_pool;
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
