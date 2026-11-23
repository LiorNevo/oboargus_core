//! Owner-scoped persistence for conversations, messages, and turns.
//!
//! SQL lives here so route handlers and the engine stay thin. Every read and
//! write that concerns a conversation filters by `owner_id` unless the caller
//! already resolved an owned turn.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::morpheus::error::{ErrorCode, MorpheusError};
use crate::morpheus::model::{
    ApprovalRecord, ConversationRecord, ConversationSummaryRecord, MessageRecord,
    StandingApprovalRecord, StepRecord, ToolCallRecord, TurnRecord, UiActionRecord,
};
use crate::morpheus::title::derive_title;

fn db(error: sqlx::Error) -> MorpheusError {
    MorpheusError::db(error)
}

fn not_found(what: &str) -> MorpheusError {
    MorpheusError::NotFound(what.to_string())
}

/// Creates a conversation idempotently.
pub async fn create_conversation(
    pool: &PgPool,
    owner_id: &str,
    client_conversation_id: Uuid,
) -> Result<ConversationRecord, MorpheusError> {
    sqlx::query(
        "INSERT INTO morpheus_conversations (owner_id, client_conversation_id) \
         VALUES ($1, $2) ON CONFLICT (owner_id, client_conversation_id) DO NOTHING",
    )
    .bind(owner_id)
    .bind(client_conversation_id)
    .execute(pool)
    .await
    .map_err(db)?;

    sqlx::query_as::<_, ConversationRecord>(
        "SELECT id, owner_id, client_conversation_id, title, next_message_seq, \
                created_at, updated_at \
         FROM morpheus_conversations WHERE owner_id = $1 AND client_conversation_id = $2",
    )
    .bind(owner_id)
    .bind(client_conversation_id)
    .fetch_one(pool)
    .await
    .map_err(db)
}

/// Loads a conversation by its client-generated id.
pub async fn get_conversation_by_client(
    pool: &PgPool,
    owner_id: &str,
    client_conversation_id: Uuid,
) -> Result<ConversationRecord, MorpheusError> {
    sqlx::query_as::<_, ConversationRecord>(
        "SELECT id, owner_id, client_conversation_id, title, next_message_seq, \
                created_at, updated_at \
         FROM morpheus_conversations WHERE owner_id = $1 AND client_conversation_id = $2",
    )
    .bind(owner_id)
    .bind(client_conversation_id)
    .fetch_optional(pool)
    .await
    .map_err(db)?
    .ok_or_else(|| not_found("conversation"))
}

/// Counts a conversation's messages.
pub async fn count_messages(pool: &PgPool, conversation_id: Uuid) -> Result<i64, MorpheusError> {
    sqlx::query_scalar("SELECT count(*) FROM morpheus_messages WHERE conversation_id = $1")
        .bind(conversation_id)
        .fetch_one(pool)
        .await
        .map_err(db)
}

/// Loads a conversation owned by the actor.
pub async fn get_conversation(
    pool: &PgPool,
    owner_id: &str,
    conversation_id: Uuid,
) -> Result<ConversationRecord, MorpheusError> {
    sqlx::query_as::<_, ConversationRecord>(
        "SELECT id, owner_id, client_conversation_id, title, next_message_seq, \
                created_at, updated_at \
         FROM morpheus_conversations WHERE id = $1 AND owner_id = $2",
    )
    .bind(conversation_id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await
    .map_err(db)?
    .ok_or_else(|| not_found("conversation"))
}

/// Returns the owner recorded on a conversation.
pub async fn conversation_owner(
    pool: &PgPool,
    conversation_id: Uuid,
) -> Result<String, MorpheusError> {
    sqlx::query_scalar("SELECT owner_id FROM morpheus_conversations WHERE id = $1")
        .bind(conversation_id)
        .fetch_optional(pool)
        .await
        .map_err(db)?
        .ok_or_else(|| not_found("conversation"))
}

/// Lists non-empty conversations, newest activity first.
pub async fn list_conversations(
    pool: &PgPool,
    owner_id: &str,
    limit: i64,
    offset: i64,
) -> Result<(Vec<ConversationSummaryRecord>, i64), MorpheusError> {
    let items = sqlx::query_as::<_, ConversationSummaryRecord>(
        "SELECT c.id, c.title, \
                COALESCE((SELECT left(btrim(m.content), 120) FROM morpheus_messages m \
                          WHERE m.conversation_id = c.id ORDER BY m.seq DESC LIMIT 1), '') \
                    AS preview, \
                (SELECT count(*) FROM morpheus_messages m WHERE m.conversation_id = c.id) \
                    AS message_count, \
                c.created_at, c.updated_at \
         FROM morpheus_conversations c \
         WHERE c.owner_id = $1 \
           AND EXISTS (SELECT 1 FROM morpheus_messages m WHERE m.conversation_id = c.id) \
         ORDER BY c.updated_at DESC, c.id DESC LIMIT $2 OFFSET $3",
    )
    .bind(owner_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(db)?;

    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM morpheus_conversations c \
         WHERE c.owner_id = $1 \
           AND EXISTS (SELECT 1 FROM morpheus_messages m WHERE m.conversation_id = c.id)",
    )
    .bind(owner_id)
    .fetch_one(pool)
    .await
    .map_err(db)?;
    Ok((items, total))
}

/// Renames a conversation without changing activity ordering.
pub async fn rename_conversation(
    pool: &PgPool,
    owner_id: &str,
    conversation_id: Uuid,
    title: &str,
) -> Result<ConversationRecord, MorpheusError> {
    let updated = sqlx::query_as::<_, ConversationRecord>(
        "UPDATE morpheus_conversations SET title = $3 \
         WHERE id = $1 AND owner_id = $2 \
         RETURNING id, owner_id, client_conversation_id, title, next_message_seq, \
                   created_at, updated_at",
    )
    .bind(conversation_id)
    .bind(owner_id)
    .bind(title)
    .fetch_optional(pool)
    .await
    .map_err(db)?;
    updated.ok_or_else(|| not_found("conversation"))
}

/// Hard-deletes a conversation and every child row.
pub async fn delete_conversation(
    pool: &PgPool,
    owner_id: &str,
    conversation_id: Uuid,
) -> Result<(), MorpheusError> {
    let result = sqlx::query("DELETE FROM morpheus_conversations WHERE id = $1 AND owner_id = $2")
        .bind(conversation_id)
        .bind(owner_id)
        .execute(pool)
        .await
        .map_err(db)?;
    if result.rows_affected() == 0 {
        return Err(not_found("conversation"));
    }
    Ok(())
}

/// Loads the newest messages in transcript order.
pub async fn list_messages(
    pool: &PgPool,
    conversation_id: Uuid,
    limit: i64,
) -> Result<(Vec<MessageRecord>, bool), MorpheusError> {
    let mut items = sqlx::query_as::<_, MessageRecord>(
        "SELECT id, conversation_id, seq, role, status, content, client_message_id, \
                error_code, error_message, context_trimmed, created_at, updated_at, completed_at \
         FROM morpheus_messages WHERE conversation_id = $1 \
         ORDER BY seq DESC LIMIT $2",
    )
    .bind(conversation_id)
    .bind(limit + 1)
    .fetch_all(pool)
    .await
    .map_err(db)?;
    let has_more = items.len() as i64 > limit;
    if has_more {
        items.truncate(limit as usize);
    }
    items.reverse();
    Ok((items, has_more))
}

/// Loads messages older than `before_seq`.
pub async fn list_messages_before(
    pool: &PgPool,
    conversation_id: Uuid,
    before_seq: i64,
    limit: i64,
) -> Result<(Vec<MessageRecord>, bool), MorpheusError> {
    let mut items = sqlx::query_as::<_, MessageRecord>(
        "SELECT id, conversation_id, seq, role, status, content, client_message_id, \
                error_code, error_message, context_trimmed, created_at, updated_at, completed_at \
         FROM morpheus_messages WHERE conversation_id = $1 AND seq < $2 \
         ORDER BY seq DESC LIMIT $3",
    )
    .bind(conversation_id)
    .bind(before_seq)
    .bind(limit + 1)
    .fetch_all(pool)
    .await
    .map_err(db)?;
    let has_more = items.len() as i64 > limit;
    if has_more {
        items.truncate(limit as usize);
    }
    items.reverse();
    Ok((items, has_more))
}

/// Loads one message in a conversation.
pub async fn get_message(
    pool: &PgPool,
    conversation_id: Uuid,
    message_id: Uuid,
) -> Result<MessageRecord, MorpheusError> {
    sqlx::query_as::<_, MessageRecord>(
        "SELECT id, conversation_id, seq, role, status, content, client_message_id, \
                error_code, error_message, context_trimmed, created_at, updated_at, completed_at \
         FROM morpheus_messages WHERE id = $1 AND conversation_id = $2",
    )
    .bind(message_id)
    .bind(conversation_id)
    .fetch_optional(pool)
    .await
    .map_err(db)?
    .ok_or_else(|| not_found("message"))
}

/// Lists all turns in a conversation.
pub async fn list_turns(
    pool: &PgPool,
    conversation_id: Uuid,
) -> Result<Vec<TurnRecord>, MorpheusError> {
    sqlx::query_as::<_, TurnRecord>(
        "SELECT id, conversation_id, user_message_id, agent_message_id, attempt, retry_of, \
                status, surface_session_id, step_count, provider_calls, observation_count, \
                approve_all_turn, limit_reached, error_code, error_message, created_at, \
                started_at, ended_at, updated_at \
         FROM morpheus_turns WHERE conversation_id = $1 ORDER BY created_at, id",
    )
    .bind(conversation_id)
    .fetch_all(pool)
    .await
    .map_err(db)
}

/// Loads a turn by id.
pub async fn get_turn(pool: &PgPool, turn_id: Uuid) -> Result<TurnRecord, MorpheusError> {
    sqlx::query_as::<_, TurnRecord>(
        "SELECT id, conversation_id, user_message_id, agent_message_id, attempt, retry_of, \
                status, surface_session_id, step_count, provider_calls, observation_count, \
                approve_all_turn, limit_reached, error_code, error_message, created_at, \
                started_at, ended_at, updated_at \
         FROM morpheus_turns WHERE id = $1",
    )
    .bind(turn_id)
    .fetch_optional(pool)
    .await
    .map_err(db)?
    .ok_or_else(|| not_found("turn"))
}

/// Loads a turn only when the actor owns its conversation.
pub async fn get_owned_turn(
    pool: &PgPool,
    owner_id: &str,
    turn_id: Uuid,
) -> Result<TurnRecord, MorpheusError> {
    sqlx::query_as::<_, TurnRecord>(
        "SELECT t.id, t.conversation_id, t.user_message_id, t.agent_message_id, t.attempt, \
                t.retry_of, t.status, t.surface_session_id, t.step_count, t.provider_calls, \
                t.observation_count, t.approve_all_turn, t.limit_reached, t.error_code, \
                t.error_message, t.created_at, t.started_at, t.ended_at, t.updated_at \
         FROM morpheus_turns t JOIN morpheus_conversations c ON c.id = t.conversation_id \
         WHERE t.id = $1 AND c.owner_id = $2",
    )
    .bind(turn_id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await
    .map_err(db)?
    .ok_or_else(|| not_found("turn"))
}

/// Creates the user message, the agent placeholder, and a queued turn.
///
/// Idempotent on `(conversation_id, client_message_id)`. Returns the existing
/// pair when the client retries a send.
pub async fn create_turn(
    pool: &PgPool,
    owner_id: &str,
    conversation_id: Uuid,
    content: &str,
    client_message_id: &str,
) -> Result<(MessageRecord, MessageRecord, TurnRecord), MorpheusError> {
    let mut tx = pool.begin().await.map_err(db)?;

    let conversation = sqlx::query_as::<_, ConversationRecord>(
        "SELECT id, owner_id, client_conversation_id, title, next_message_seq, \
                created_at, updated_at \
         FROM morpheus_conversations WHERE id = $1 AND owner_id = $2 FOR UPDATE",
    )
    .bind(conversation_id)
    .bind(owner_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db)?
    .ok_or_else(|| not_found("conversation"))?;

    if let Some(user) = sqlx::query_as::<_, MessageRecord>(
        "SELECT id, conversation_id, seq, role, status, content, client_message_id, \
                error_code, error_message, context_trimmed, created_at, updated_at, completed_at \
         FROM morpheus_messages \
         WHERE conversation_id = $1 AND client_message_id = $2 AND role = 'user'",
    )
    .bind(conversation_id)
    .bind(client_message_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db)?
    {
        let turn = sqlx::query_as::<_, TurnRecord>(
            "SELECT id, conversation_id, user_message_id, agent_message_id, attempt, retry_of, \
                    status, surface_session_id, step_count, provider_calls, observation_count, \
                    approve_all_turn, limit_reached, error_code, error_message, created_at, \
                    started_at, ended_at, updated_at \
             FROM morpheus_turns WHERE user_message_id = $1 ORDER BY attempt DESC LIMIT 1",
        )
        .bind(user.id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        let agent = sqlx::query_as::<_, MessageRecord>(
            "SELECT id, conversation_id, seq, role, status, content, client_message_id, \
                    error_code, error_message, context_trimmed, created_at, updated_at, completed_at \
             FROM morpheus_messages WHERE id = $1",
        )
        .bind(turn.agent_message_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)?;
        return Ok((user, agent, turn));
    }

    let user_seq = conversation.next_message_seq;
    let agent_seq = user_seq + 1;
    let title = if conversation.title.trim().is_empty() {
        derive_title(content)
    } else {
        conversation.title.clone()
    };

    let user = sqlx::query_as::<_, MessageRecord>(
        "INSERT INTO morpheus_messages \
            (conversation_id, seq, role, status, content, client_message_id, completed_at) \
         VALUES ($1, $2, 'user', 'complete', $3, $4, NOW()) \
         RETURNING id, conversation_id, seq, role, status, content, client_message_id, \
                   error_code, error_message, context_trimmed, created_at, updated_at, completed_at",
    )
    .bind(conversation_id)
    .bind(user_seq)
    .bind(content)
    .bind(client_message_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(db)?;

    let agent = sqlx::query_as::<_, MessageRecord>(
        "INSERT INTO morpheus_messages \
            (conversation_id, seq, role, status, content) \
         VALUES ($1, $2, 'agent', 'queued', '') \
         RETURNING id, conversation_id, seq, role, status, content, client_message_id, \
                   error_code, error_message, context_trimmed, created_at, updated_at, completed_at",
    )
    .bind(conversation_id)
    .bind(agent_seq)
    .fetch_one(&mut *tx)
    .await
    .map_err(db)?;

    let turn = sqlx::query_as::<_, TurnRecord>(
        "INSERT INTO morpheus_turns \
            (conversation_id, user_message_id, agent_message_id, attempt, status) \
         VALUES ($1, $2, $3, 1, 'queued') \
         RETURNING id, conversation_id, user_message_id, agent_message_id, attempt, retry_of, \
                   status, surface_session_id, step_count, provider_calls, observation_count, \
                   approve_all_turn, limit_reached, error_code, error_message, created_at, \
                   started_at, ended_at, updated_at",
    )
    .bind(conversation_id)
    .bind(user.id)
    .bind(agent.id)
    .fetch_one(&mut *tx)
    .await
    .map_err(db)?;

    sqlx::query(
        "UPDATE morpheus_conversations \
         SET next_message_seq = next_message_seq + 2, title = $3, updated_at = NOW() \
         WHERE id = $1 AND owner_id = $2",
    )
    .bind(conversation_id)
    .bind(owner_id)
    .bind(title)
    .execute(&mut *tx)
    .await
    .map_err(db)?;

    tx.commit().await.map_err(db)?;
    Ok((user, agent, turn))
}

/// Claims the oldest queued turn for a conversation when none is active.
pub async fn claim_next_turn(
    pool: &PgPool,
    conversation_id: Uuid,
) -> Result<Option<TurnRecord>, MorpheusError> {
    let mut tx = pool.begin().await.map_err(db)?;
    sqlx::query("SELECT id FROM morpheus_conversations WHERE id = $1 FOR UPDATE")
        .bind(conversation_id)
        .execute(&mut *tx)
        .await
        .map_err(db)?;

    let active: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM morpheus_turns WHERE conversation_id = $1 AND status IN \
         ('running','awaiting_approval','awaiting_observation','awaiting_action_result') LIMIT 1",
    )
    .bind(conversation_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db)?;
    if active.is_some() {
        tx.commit().await.map_err(db)?;
        return Ok(None);
    }

    let turn = sqlx::query_as::<_, TurnRecord>(
        "SELECT id, conversation_id, user_message_id, agent_message_id, attempt, retry_of, \
                status, surface_session_id, step_count, provider_calls, observation_count, \
                approve_all_turn, limit_reached, error_code, error_message, created_at, \
                started_at, ended_at, updated_at \
         FROM morpheus_turns WHERE conversation_id = $1 AND status = 'queued' \
         ORDER BY created_at, id LIMIT 1 FOR UPDATE SKIP LOCKED",
    )
    .bind(conversation_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db)?;

    let Some(turn) = turn else {
        tx.commit().await.map_err(db)?;
        return Ok(None);
    };

    let claimed = sqlx::query_as::<_, TurnRecord>(
        "UPDATE morpheus_turns \
         SET status = 'running', started_at = COALESCE(started_at, NOW()), updated_at = NOW() \
         WHERE id = $1 AND status = 'queued' \
         RETURNING id, conversation_id, user_message_id, agent_message_id, attempt, retry_of, \
                   status, surface_session_id, step_count, provider_calls, observation_count, \
                   approve_all_turn, limit_reached, error_code, error_message, created_at, \
                   started_at, ended_at, updated_at",
    )
    .bind(turn.id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(claimed)
}

/// Updates a turn's status and terminal fields.
pub async fn set_turn_status(
    pool: &PgPool,
    turn_id: Uuid,
    status: &str,
    error_code: Option<&str>,
    error_message: Option<&str>,
    limit_reached: Option<&str>,
) -> Result<(), MorpheusError> {
    let terminal = matches!(status, "completed" | "stopped" | "failed");
    sqlx::query(
        "UPDATE morpheus_turns SET status = $2, error_code = $3, error_message = $4, \
                limit_reached = $5, updated_at = NOW(), \
                ended_at = CASE WHEN $6 THEN COALESCE(ended_at, NOW()) ELSE ended_at END \
         WHERE id = $1",
    )
    .bind(turn_id)
    .bind(status)
    .bind(error_code)
    .bind(error_message)
    .bind(limit_reached)
    .bind(terminal)
    .execute(pool)
    .await
    .map_err(db)?;
    Ok(())
}

/// Updates the turn counters after a provider call or observation.
pub async fn increment_turn_provider_calls(
    pool: &PgPool,
    turn_id: Uuid,
) -> Result<(), MorpheusError> {
    sqlx::query(
        "UPDATE morpheus_turns SET provider_calls = provider_calls + 1, updated_at = NOW() \
         WHERE id = $1",
    )
    .bind(turn_id)
    .execute(pool)
    .await
    .map_err(db)?;
    Ok(())
}

/// Sets a message's status and optional error.
pub async fn set_message_status(
    pool: &PgPool,
    message_id: Uuid,
    status: &str,
    error_code: Option<&str>,
    error_message: Option<&str>,
) -> Result<(), MorpheusError> {
    let terminal = matches!(status, "complete" | "stopped" | "failed");
    sqlx::query(
        "UPDATE morpheus_messages SET status = $2, error_code = $3, error_message = $4, \
                updated_at = NOW(), \
                completed_at = CASE WHEN $5 THEN COALESCE(completed_at, NOW()) ELSE completed_at END \
         WHERE id = $1",
    )
    .bind(message_id)
    .bind(status)
    .bind(error_code)
    .bind(error_message)
    .bind(terminal)
    .execute(pool)
    .await
    .map_err(db)?;
    Ok(())
}

/// Marks whether earlier context was omitted for a message.
pub async fn set_context_trimmed(
    pool: &PgPool,
    message_id: Uuid,
    trimmed: bool,
) -> Result<(), MorpheusError> {
    sqlx::query("UPDATE morpheus_messages SET context_trimmed = $2 WHERE id = $1")
        .bind(message_id)
        .bind(trimmed)
        .execute(pool)
        .await
        .map_err(db)?;
    Ok(())
}

/// Appends streamed content and returns the full stored content.
pub async fn append_message_content(
    pool: &PgPool,
    message_id: Uuid,
    delta: &str,
) -> Result<String, MorpheusError> {
    sqlx::query_scalar(
        "UPDATE morpheus_messages SET content = content || $2, updated_at = NOW() \
         WHERE id = $1 RETURNING content",
    )
    .bind(message_id)
    .bind(delta)
    .fetch_optional(pool)
    .await
    .map_err(db)?
    .ok_or_else(|| not_found("message"))
}

/// Replaces stored content (used by a snapshot reconciliation).
pub async fn set_message_content(
    pool: &PgPool,
    message_id: Uuid,
    content: &str,
) -> Result<(), MorpheusError> {
    sqlx::query("UPDATE morpheus_messages SET content = $2, updated_at = NOW() WHERE id = $1")
        .bind(message_id)
        .bind(content)
        .execute(pool)
        .await
        .map_err(db)?;
    Ok(())
}

/// Inserts a step with the next per-turn sequence.
pub async fn insert_step(
    pool: &PgPool,
    turn_id: Uuid,
    kind: &str,
    status: &str,
    summary: &str,
    ref_id: Option<Uuid>,
    payload: Value,
) -> Result<StepRecord, MorpheusError> {
    sqlx::query_as::<_, StepRecord>(
        "INSERT INTO morpheus_steps (turn_id, seq, kind, status, summary, ref_id, payload) \
         SELECT $1, COALESCE(MAX(seq), 0) + 1, $2, $3, $4, $5, $6 \
         FROM morpheus_steps WHERE turn_id = $1 \
         RETURNING id, turn_id, seq, kind, status, summary, ref_id, payload, created_at, completed_at",
    )
    .bind(turn_id)
    .bind(kind)
    .bind(status)
    .bind(summary)
    .bind(ref_id)
    .bind(payload)
    .fetch_one(pool)
    .await
    .map_err(db)
}

/// Completes a step.
pub async fn finish_step(
    pool: &PgPool,
    step_id: Uuid,
    status: &str,
    summary: &str,
) -> Result<(), MorpheusError> {
    sqlx::query(
        "UPDATE morpheus_steps SET status = $2, summary = $3, completed_at = NOW() WHERE id = $1",
    )
    .bind(step_id)
    .bind(status)
    .bind(summary)
    .execute(pool)
    .await
    .map_err(db)?;
    Ok(())
}

/// Lists a turn's steps.
pub async fn list_steps(pool: &PgPool, turn_id: Uuid) -> Result<Vec<StepRecord>, MorpheusError> {
    sqlx::query_as::<_, StepRecord>(
        "SELECT id, turn_id, seq, kind, status, summary, ref_id, payload, created_at, completed_at \
         FROM morpheus_steps WHERE turn_id = $1 ORDER BY seq",
    )
    .bind(turn_id)
    .fetch_all(pool)
    .await
    .map_err(db)
}

/// Inserts a tool call row.
///
/// The argument list mirrors the auditable row; callers pass a validated spec
/// and its redacted summary together.
#[allow(clippy::too_many_arguments)]
pub async fn insert_tool_call(
    pool: &PgPool,
    turn_id: Uuid,
    provider_call_id: &str,
    tool_id: &str,
    tool_version: i32,
    arguments: Value,
    arguments_summary: &str,
    risk: &str,
    status: &str,
) -> Result<ToolCallRecord, MorpheusError> {
    sqlx::query_as::<_, ToolCallRecord>(
        "INSERT INTO morpheus_tool_calls \
            (turn_id, provider_call_id, tool_id, tool_version, arguments, arguments_summary, \
             risk, status) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
         RETURNING id, turn_id, provider_call_id, tool_id, tool_version, arguments, \
                   arguments_summary, risk, status, result_summary, result_truncated, error_code, \
                   error_message, approval_id, started_at, completed_at, created_at",
    )
    .bind(turn_id)
    .bind(provider_call_id)
    .bind(tool_id)
    .bind(tool_version)
    .bind(arguments)
    .bind(arguments_summary)
    .bind(risk)
    .bind(status)
    .fetch_one(pool)
    .await
    .map_err(db)
}

/// Lists a turn's tool calls.
pub async fn list_tool_calls(
    pool: &PgPool,
    turn_id: Uuid,
) -> Result<Vec<ToolCallRecord>, MorpheusError> {
    sqlx::query_as::<_, ToolCallRecord>(
        "SELECT id, turn_id, provider_call_id, tool_id, tool_version, arguments, \
                arguments_summary, risk, status, result_summary, result_truncated, error_code, \
                error_message, approval_id, started_at, completed_at, created_at \
         FROM morpheus_tool_calls WHERE turn_id = $1 ORDER BY created_at, id",
    )
    .bind(turn_id)
    .fetch_all(pool)
    .await
    .map_err(db)
}

/// Lists a turn's UI actions.
pub async fn list_ui_actions(
    pool: &PgPool,
    turn_id: Uuid,
) -> Result<Vec<UiActionRecord>, MorpheusError> {
    sqlx::query_as::<_, UiActionRecord>(
        "SELECT id, turn_id, observation_id, kind, target_ref, target_label, value_summary, \
                state_hash, status, result_summary, error_code, approval_id, started_at, \
                completed_at, created_at \
         FROM morpheus_ui_actions WHERE turn_id = $1 ORDER BY created_at, id",
    )
    .bind(turn_id)
    .fetch_all(pool)
    .await
    .map_err(db)
}

/// True when the conversation has an active turn.
pub async fn has_active_turn(pool: &PgPool, conversation_id: Uuid) -> Result<bool, MorpheusError> {
    let value: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM morpheus_turns WHERE conversation_id = $1 AND status IN \
         ('running','awaiting_approval','awaiting_observation','awaiting_action_result') LIMIT 1",
    )
    .bind(conversation_id)
    .fetch_optional(pool)
    .await
    .map_err(db)?;
    Ok(value.is_some())
}

/// Counts queued turns in a conversation.
pub async fn queued_turn_count(pool: &PgPool, conversation_id: Uuid) -> Result<i64, MorpheusError> {
    sqlx::query_scalar(
        "SELECT count(*) FROM morpheus_turns WHERE conversation_id = $1 AND status = 'queued'",
    )
    .bind(conversation_id)
    .fetch_one(pool)
    .await
    .map_err(db)
}

/// Marks live turns interrupted during startup recovery.
pub async fn mark_interrupted(pool: &PgPool) -> Result<u64, MorpheusError> {
    let result = sqlx::query(
        "UPDATE morpheus_turns SET status = 'failed', error_code = 'interrupted', \
                error_message = 'The turn was interrupted. You can retry.', \
                ended_at = COALESCE(ended_at, NOW()), updated_at = NOW() \
         WHERE status IN ('running', 'awaiting_approval', 'awaiting_observation', \
                          'awaiting_action_result')",
    )
    .execute(pool)
    .await
    .map_err(db)?;

    sqlx::query(
        "UPDATE morpheus_messages SET status = 'failed', error_code = 'interrupted', \
                error_message = 'The turn was interrupted. You can retry.', \
                completed_at = COALESCE(completed_at, NOW()), updated_at = NOW() \
         WHERE status IN ('queued', 'streaming') AND role = 'agent'",
    )
    .execute(pool)
    .await
    .map_err(db)?;
    Ok(result.rows_affected())
}

/// Returns the turn's event stream message, used as the projection target.
pub async fn get_turn_agent_message(
    pool: &PgPool,
    turn_id: Uuid,
) -> Result<MessageRecord, MorpheusError> {
    sqlx::query_as::<_, MessageRecord>(
        "SELECT m.id, m.conversation_id, m.seq, m.role, m.status, m.content, \
                m.client_message_id, m.error_code, m.error_message, m.context_trimmed, \
                m.created_at, m.updated_at, m.completed_at \
         FROM morpheus_messages m JOIN morpheus_turns t ON t.agent_message_id = m.id \
         WHERE t.id = $1",
    )
    .bind(turn_id)
    .fetch_optional(pool)
    .await
    .map_err(db)?
    .ok_or_else(|| not_found("message"))
}

/// Returns the turn's user message.
pub async fn get_turn_user_message(
    pool: &PgPool,
    turn_id: Uuid,
) -> Result<MessageRecord, MorpheusError> {
    sqlx::query_as::<_, MessageRecord>(
        "SELECT m.id, m.conversation_id, m.seq, m.role, m.status, m.content, \
                m.client_message_id, m.error_code, m.error_message, m.context_trimmed, \
                m.created_at, m.updated_at, m.completed_at \
         FROM morpheus_messages m JOIN morpheus_turns t ON t.user_message_id = m.id \
         WHERE t.id = $1",
    )
    .bind(turn_id)
    .fetch_optional(pool)
    .await
    .map_err(db)?
    .ok_or_else(|| not_found("message"))
}

/// Updates the conversation's last-activity instant.
pub async fn touch_conversation(pool: &PgPool, conversation_id: Uuid) -> Result<(), MorpheusError> {
    sqlx::query("UPDATE morpheus_conversations SET updated_at = NOW() WHERE id = $1")
        .bind(conversation_id)
        .execute(pool)
        .await
        .map_err(db)?;
    Ok(())
}

/// Returns a short summary for a tool result.
pub fn truncate_summary(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    let mut out: String = chars[..max].iter().collect();
    out.push('…');
    out
}

/// Parses a stored timestamp for tests and projections.
pub fn as_utc(value: DateTime<Utc>) -> DateTime<Utc> {
    value
}

/// Error code for a missing row in a projection.
pub fn missing_row(what: &str) -> MorpheusError {
    MorpheusError::NotFound(what.to_string())
}

/// Convenience wrapper for provider failures in persistence tests.
pub fn provider_unavailable(detail: impl Into<String>) -> MorpheusError {
    MorpheusError::provider(ErrorCode::ProviderUnavailable, detail)
}

/// Loads one tool call.
pub async fn get_tool_call(
    pool: &PgPool,
    turn_id: Uuid,
    tool_call_id: Uuid,
) -> Result<ToolCallRecord, MorpheusError> {
    sqlx::query_as::<_, ToolCallRecord>(
        "SELECT id, turn_id, provider_call_id, tool_id, tool_version, arguments, \
                arguments_summary, risk, status, result_summary, result_truncated, error_code, \
                error_message, approval_id, started_at, completed_at, created_at \
         FROM morpheus_tool_calls WHERE id = $1 AND turn_id = $2",
    )
    .bind(tool_call_id)
    .bind(turn_id)
    .fetch_optional(pool)
    .await
    .map_err(db)?
    .ok_or_else(|| not_found("tool call"))
}

/// Updates a tool call's status.
pub async fn set_tool_call_status(
    pool: &PgPool,
    tool_call_id: Uuid,
    status: &str,
) -> Result<(), MorpheusError> {
    sqlx::query("UPDATE morpheus_tool_calls SET status = $2 WHERE id = $1")
        .bind(tool_call_id)
        .bind(status)
        .execute(pool)
        .await
        .map_err(db)?;
    Ok(())
}

/// Stores a tool call outcome.
#[allow(clippy::too_many_arguments)]
pub async fn finish_tool_call(
    pool: &PgPool,
    tool_call_id: Uuid,
    status: &str,
    result_summary: Option<&str>,
    truncated: bool,
    error_code: Option<&str>,
    error_message: Option<&str>,
) -> Result<(), MorpheusError> {
    sqlx::query(
        "UPDATE morpheus_tool_calls SET status = $2, result_summary = $3, \
                result_truncated = $4, error_code = $5, error_message = $6, \
                completed_at = NOW() WHERE id = $1",
    )
    .bind(tool_call_id)
    .bind(status)
    .bind(result_summary)
    .bind(truncated)
    .bind(error_code)
    .bind(error_message)
    .execute(pool)
    .await
    .map_err(db)?;
    Ok(())
}

/// Marks a tool call as running.
pub async fn start_tool_call(pool: &PgPool, tool_call_id: Uuid) -> Result<(), MorpheusError> {
    sqlx::query(
        "UPDATE morpheus_tool_calls SET status = 'running', started_at = NOW() WHERE id = $1",
    )
    .bind(tool_call_id)
    .execute(pool)
    .await
    .map_err(db)?;
    Ok(())
}

/// Creates or returns a pending approval for an action.
pub async fn create_approval(
    pool: &PgPool,
    turn_id: Uuid,
    action_kind: &str,
    action_id: Uuid,
    context_hash: &str,
    expires_in_seconds: u64,
) -> Result<ApprovalRecord, MorpheusError> {
    if let Some(existing) = get_approval_for_action(pool, action_kind, action_id).await? {
        return Ok(existing);
    }
    sqlx::query_as::<_, ApprovalRecord>(
        "INSERT INTO morpheus_approval_requests \
            (turn_id, action_kind, action_id, context_hash, expires_at) \
         VALUES ($1, $2, $3, $4, NOW() + make_interval(secs => $5)) \
         RETURNING id, turn_id, action_kind, action_id, status, decision_mode, context_hash, \
                   expires_at, created_at, decided_at",
    )
    .bind(turn_id)
    .bind(action_kind)
    .bind(action_id)
    .bind(context_hash)
    .bind(expires_in_seconds as f64)
    .fetch_one(pool)
    .await
    .map_err(db)
}

/// Loads one approval by id.
pub async fn get_approval(
    pool: &PgPool,
    approval_id: Uuid,
) -> Result<ApprovalRecord, MorpheusError> {
    sqlx::query_as::<_, ApprovalRecord>(
        "SELECT id, turn_id, action_kind, action_id, status, decision_mode, context_hash, \
                expires_at, created_at, decided_at \
         FROM morpheus_approval_requests WHERE id = $1",
    )
    .bind(approval_id)
    .fetch_optional(pool)
    .await
    .map_err(db)?
    .ok_or_else(|| not_found("approval"))
}

/// Loads an approval for a specific action.
pub async fn get_approval_for_action(
    pool: &PgPool,
    action_kind: &str,
    action_id: Uuid,
) -> Result<Option<ApprovalRecord>, MorpheusError> {
    sqlx::query_as::<_, ApprovalRecord>(
        "SELECT id, turn_id, action_kind, action_id, status, decision_mode, context_hash, \
                expires_at, created_at, decided_at \
         FROM morpheus_approval_requests WHERE action_kind = $1 AND action_id = $2",
    )
    .bind(action_kind)
    .bind(action_id)
    .fetch_optional(pool)
    .await
    .map_err(db)
}

/// Loads an approval owned by the actor.
pub async fn get_owned_approval(
    pool: &PgPool,
    owner_id: &str,
    approval_id: Uuid,
) -> Result<ApprovalRecord, MorpheusError> {
    sqlx::query_as::<_, ApprovalRecord>(
        "SELECT a.id, a.turn_id, a.action_kind, a.action_id, a.status, a.decision_mode, \
                a.context_hash, a.expires_at, a.created_at, a.decided_at \
         FROM morpheus_approval_requests a \
         JOIN morpheus_turns t ON t.id = a.turn_id \
         JOIN morpheus_conversations c ON c.id = t.conversation_id \
         WHERE a.id = $1 AND c.owner_id = $2",
    )
    .bind(approval_id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await
    .map_err(db)?
    .ok_or_else(|| not_found("approval"))
}

/// Applies a decision to an approval.
pub async fn decide_approval(
    pool: &PgPool,
    approval_id: Uuid,
    status: &str,
    mode: &str,
) -> Result<(), MorpheusError> {
    sqlx::query(
        "UPDATE morpheus_approval_requests SET status = $2, decision_mode = $3, \
                decided_at = NOW() WHERE id = $1 AND status = 'pending'",
    )
    .bind(approval_id)
    .bind(status)
    .bind(mode)
    .execute(pool)
    .await
    .map_err(db)?;
    Ok(())
}

/// Lists pending approvals for a turn.
pub async fn list_pending_approvals(
    pool: &PgPool,
    turn_id: Uuid,
) -> Result<Vec<ApprovalRecord>, MorpheusError> {
    sqlx::query_as::<_, ApprovalRecord>(
        "SELECT id, turn_id, action_kind, action_id, status, decision_mode, context_hash, \
                expires_at, created_at, decided_at \
         FROM morpheus_approval_requests WHERE turn_id = $1 AND status = 'pending' \
         ORDER BY created_at, id",
    )
    .bind(turn_id)
    .fetch_all(pool)
    .await
    .map_err(db)
}

/// Links an approval to a tool call.
pub async fn set_tool_call_approval(
    pool: &PgPool,
    tool_call_id: Uuid,
    approval_id: Uuid,
) -> Result<(), MorpheusError> {
    sqlx::query("UPDATE morpheus_tool_calls SET approval_id = $2 WHERE id = $1")
        .bind(tool_call_id)
        .bind(approval_id)
        .execute(pool)
        .await
        .map_err(db)?;
    Ok(())
}

/// Sets the turn-scoped "approve all" grant.
pub async fn set_approve_all_turn(
    pool: &PgPool,
    turn_id: Uuid,
    value: bool,
) -> Result<(), MorpheusError> {
    sqlx::query("UPDATE morpheus_turns SET approve_all_turn = $2 WHERE id = $1")
        .bind(turn_id)
        .bind(value)
        .execute(pool)
        .await
        .map_err(db)?;
    Ok(())
}

/// Approves every pending approval in a turn with the given mode.
pub async fn approve_pending_turn_approvals(
    pool: &PgPool,
    turn_id: Uuid,
    mode: &str,
) -> Result<(), MorpheusError> {
    sqlx::query(
        "UPDATE morpheus_approval_requests SET status = 'approved', decision_mode = $2, \
                decided_at = NOW() WHERE turn_id = $1 AND status = 'pending'",
    )
    .bind(turn_id)
    .bind(mode)
    .execute(pool)
    .await
    .map_err(db)?;
    Ok(())
}

/// Approves every pending tool call in a turn.
pub async fn approve_pending_turn_tools(pool: &PgPool, turn_id: Uuid) -> Result<(), MorpheusError> {
    sqlx::query(
        "UPDATE morpheus_tool_calls SET status = 'approved' \
         WHERE turn_id = $1 AND status = 'awaiting_approval'",
    )
    .bind(turn_id)
    .execute(pool)
    .await
    .map_err(db)?;
    Ok(())
}

/// Loads the active standing approval for an actor.
pub async fn get_active_standing(
    pool: &PgPool,
    actor_id: &str,
) -> Result<Option<StandingApprovalRecord>, MorpheusError> {
    sqlx::query_as::<_, StandingApprovalRecord>(
        "SELECT id, actor_id, tool_grants, ui_action_kinds, granted_at, revoked_at \
         FROM morpheus_standing_approvals WHERE actor_id = $1 AND revoked_at IS NULL",
    )
    .bind(actor_id)
    .fetch_optional(pool)
    .await
    .map_err(db)
}

/// Creates or replaces the active standing approval.
pub async fn upsert_standing(
    pool: &PgPool,
    actor_id: &str,
    tool_grants: Value,
    ui_action_kinds: &[String],
) -> Result<StandingApprovalRecord, MorpheusError> {
    sqlx::query(
        "UPDATE morpheus_standing_approvals SET tool_grants = $2, ui_action_kinds = $3, \
                granted_at = NOW() WHERE actor_id = $1 AND revoked_at IS NULL",
    )
    .bind(actor_id)
    .bind(&tool_grants)
    .bind(ui_action_kinds)
    .execute(pool)
    .await
    .map_err(db)?;
    let exists: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM morpheus_standing_approvals WHERE actor_id = $1 AND revoked_at IS NULL",
    )
    .bind(actor_id)
    .fetch_optional(pool)
    .await
    .map_err(db)?;
    if exists.is_none() {
        return sqlx::query_as::<_, StandingApprovalRecord>(
            "INSERT INTO morpheus_standing_approvals (actor_id, tool_grants, ui_action_kinds) \
             VALUES ($1, $2, $3) \
             RETURNING id, actor_id, tool_grants, ui_action_kinds, granted_at, revoked_at",
        )
        .bind(actor_id)
        .bind(tool_grants)
        .bind(ui_action_kinds)
        .fetch_one(pool)
        .await
        .map_err(db);
    }
    sqlx::query_as::<_, StandingApprovalRecord>(
        "SELECT id, actor_id, tool_grants, ui_action_kinds, granted_at, revoked_at \
         FROM morpheus_standing_approvals WHERE actor_id = $1 AND revoked_at IS NULL",
    )
    .bind(actor_id)
    .fetch_one(pool)
    .await
    .map_err(db)
}

/// Revokes a standing approval.
pub async fn revoke_standing(
    pool: &PgPool,
    actor_id: &str,
    approval_id: Uuid,
) -> Result<(), MorpheusError> {
    let result = sqlx::query(
        "UPDATE morpheus_standing_approvals SET revoked_at = NOW() \
         WHERE id = $1 AND actor_id = $2 AND revoked_at IS NULL",
    )
    .bind(approval_id)
    .bind(actor_id)
    .execute(pool)
    .await
    .map_err(db)?;
    if result.rows_affected() == 0 {
        return Err(not_found("standing approval"));
    }
    Ok(())
}

/// Lists revoked standing approvals for history.
pub async fn list_standing_history(
    pool: &PgPool,
    actor_id: &str,
) -> Result<Vec<StandingApprovalRecord>, MorpheusError> {
    sqlx::query_as::<_, StandingApprovalRecord>(
        "SELECT id, actor_id, tool_grants, ui_action_kinds, granted_at, revoked_at \
         FROM morpheus_standing_approvals WHERE actor_id = $1 AND revoked_at IS NOT NULL \
         ORDER BY granted_at DESC",
    )
    .bind(actor_id)
    .fetch_all(pool)
    .await
    .map_err(db)
}

/// Counts non-terminal turns awaiting a worker.
pub async fn has_queued_or_active(pool: &PgPool) -> Result<bool, MorpheusError> {
    let value: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM morpheus_turns WHERE status IN \
         ('queued','running','awaiting_approval','awaiting_observation','awaiting_action_result') \
         LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(db)?;
    Ok(value.is_some())
}

/// Creates a retry attempt for the same user message.
///
/// Only the latest attempt may be retried, and only when it failed or was
/// stopped. The question is reused; a new agent message and turn are created.
pub async fn retry_turn(
    pool: &PgPool,
    owner_id: &str,
    turn_id: Uuid,
) -> Result<(TurnRecord, MessageRecord), MorpheusError> {
    let mut tx = pool.begin().await.map_err(db)?;
    let turn = sqlx::query_as::<_, TurnRecord>(
        "SELECT t.id, t.conversation_id, t.user_message_id, t.agent_message_id, t.attempt, \
                t.retry_of, t.status, t.surface_session_id, t.step_count, t.provider_calls, \
                t.observation_count, t.approve_all_turn, t.limit_reached, t.error_code, \
                t.error_message, t.created_at, t.started_at, t.ended_at, t.updated_at \
         FROM morpheus_turns t JOIN morpheus_conversations c ON c.id = t.conversation_id \
         WHERE t.id = $1 AND c.owner_id = $2 FOR UPDATE OF t",
    )
    .bind(turn_id)
    .bind(owner_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db)?
    .ok_or_else(|| not_found("turn"))?;

    if !matches!(turn.status.as_str(), "failed" | "stopped") {
        return Err(MorpheusError::Conflict(
            "Only a failed or stopped turn can be retried.".to_string(),
        ));
    }
    let latest: i32 =
        sqlx::query_scalar("SELECT max(attempt) FROM morpheus_turns WHERE user_message_id = $1")
            .bind(turn.user_message_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?;
    if latest != turn.attempt {
        return Err(MorpheusError::Conflict(
            "Only the latest attempt can be retried.".to_string(),
        ));
    }

    let seq: i64 = sqlx::query_scalar(
        "UPDATE morpheus_conversations SET next_message_seq = next_message_seq + 1, \
                updated_at = NOW() WHERE id = $1 RETURNING next_message_seq - 1",
    )
    .bind(turn.conversation_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(db)?;

    let agent = sqlx::query_as::<_, MessageRecord>(
        "INSERT INTO morpheus_messages (conversation_id, seq, role, status, content) \
         VALUES ($1, $2, 'agent', 'queued', '') \
         RETURNING id, conversation_id, seq, role, status, content, client_message_id, \
                   error_code, error_message, context_trimmed, created_at, updated_at, completed_at",
    )
    .bind(turn.conversation_id)
    .bind(seq)
    .fetch_one(&mut *tx)
    .await
    .map_err(db)?;

    let retry = sqlx::query_as::<_, TurnRecord>(
        "INSERT INTO morpheus_turns \
            (conversation_id, user_message_id, agent_message_id, attempt, retry_of, status) \
         VALUES ($1, $2, $3, $4, $5, 'queued') \
         RETURNING id, conversation_id, user_message_id, agent_message_id, attempt, retry_of, \
                   status, surface_session_id, step_count, provider_calls, observation_count, \
                   approve_all_turn, limit_reached, error_code, error_message, created_at, \
                   started_at, ended_at, updated_at",
    )
    .bind(turn.conversation_id)
    .bind(turn.user_message_id)
    .bind(agent.id)
    .bind(turn.attempt + 1)
    .bind(turn.id)
    .fetch_one(&mut *tx)
    .await
    .map_err(db)?;

    tx.commit().await.map_err(db)?;
    Ok((retry, agent))
}
