//! Retention pruning for Morpheus conversations.
//!
//! Implements FR-023 of `specs/034-production-hardening/spec.md`. A
//! conversation is eligible when its `updated_at` activity clock is older
//! than the configured window. Conversations with an active turn or an
//! in-flight message are never eligible. Deleting a conversation cascades
//! its messages, turns, approvals, observations, steps, tool calls, UI
//! actions, and control leases.

use sqlx::PgPool;
use uuid::Uuid;

use crate::morpheus::error::MorpheusError;

/// Counts from one Morpheus retention pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetentionReport {
    /// Conversations deleted.
    pub conversations_deleted: u64,
    /// Messages deleted with their conversations.
    pub messages_deleted: u64,
    /// Turn rows deleted with their conversations.
    pub turns_deleted: u64,
}

/// Prunes conversations whose last activity is older than `retention_days`.
///
/// The conversations are locked before their children are counted so the
/// reported counts match the rows removed by this pass. Rerunning the pass
/// is idempotent and returns zero counts.
pub async fn prune(pool: &PgPool, retention_days: u32) -> Result<RetentionReport, MorpheusError> {
    if retention_days == 0 {
        return Err(MorpheusError::validation(
            "retention window must be positive",
        ));
    }
    let days = i64::from(retention_days);

    let mut tx = pool.begin().await?;

    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT c.id FROM morpheus_conversations c \
         WHERE c.updated_at < NOW() - ($1::bigint * INTERVAL '1 day') \
           AND NOT EXISTS ( \
             SELECT 1 FROM morpheus_turns t \
             WHERE t.conversation_id = c.id \
               AND t.status IN ('queued', 'running', 'awaiting_approval', \
                                'awaiting_observation', 'awaiting_action_result') \
           ) \
           AND NOT EXISTS ( \
             SELECT 1 FROM morpheus_messages m \
             WHERE m.conversation_id = c.id AND m.status IN ('queued', 'streaming') \
           ) \
         FOR UPDATE OF c",
    )
    .bind(days)
    .fetch_all(&mut *tx)
    .await?;

    if ids.is_empty() {
        tx.commit().await?;
        return Ok(RetentionReport::default());
    }

    let messages_deleted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM morpheus_messages WHERE conversation_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_one(&mut *tx)
    .await?;
    let turns_deleted: i64 =
        sqlx::query_scalar("SELECT count(*) FROM morpheus_turns WHERE conversation_id = ANY($1)")
            .bind(&ids)
            .fetch_one(&mut *tx)
            .await?;

    let conversations_deleted =
        sqlx::query("DELETE FROM morpheus_conversations WHERE id = ANY($1)")
            .bind(&ids)
            .execute(&mut *tx)
            .await?
            .rows_affected();

    tx.commit().await?;

    Ok(RetentionReport {
        conversations_deleted,
        messages_deleted: u64::try_from(messages_deleted).unwrap_or(0),
        turns_deleted: u64::try_from(turns_deleted).unwrap_or(0),
    })
}
