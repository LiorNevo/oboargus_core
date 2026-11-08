//! Persistence helpers for automation flows, versions, runs, messages, and logs.
//!
//! The HTTP layer and executor call these functions; keeping SQL here keeps
//! route handlers thin and testable. All statements use runtime binding so the
//! crate does not require a live database at build time.

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::automation::error::AutomationError;
use crate::automation::model::{
    AuditEventRecord, ContextRecord, FlowRecord, FlowVersionRecord, LogRecord, MessageRecord,
    NodeRunRecord, RunRecord, TriggerRecord,
};

fn db_error(error: sqlx::Error) -> AutomationError {
    AutomationError::Infrastructure(error.to_string())
}

/// Creates a flow and its initial draft in one transaction.
pub async fn create_flow(
    pool: &sqlx::PgPool,
    owner_id: &str,
    name: &str,
    description: &str,
    tags: &[String],
    flow_json: &Value,
) -> Result<FlowRecord, AutomationError> {
    let mut tx = pool.begin().await.map_err(db_error)?;
    let flow_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO automation_flows (id, owner_id, name, description, tags) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(flow_id)
    .bind(owner_id)
    .bind(name)
    .bind(description)
    .bind(tags)
    .execute(&mut *tx)
    .await
    .map_err(|error| match error {
        sqlx::Error::Database(ref db) if db.is_unique_violation() => {
            AutomationError::Conflict("flow.name_duplicate".to_string())
        }
        other => db_error(other),
    })?;

    let draft_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO automation_flow_versions \
         (id, flow_id, version_number, status, flow_json, created_by) \
         VALUES ($1, $2, NULL, 'draft', $3, $4)",
    )
    .bind(draft_id)
    .bind(flow_id)
    .bind(flow_json)
    .bind(owner_id)
    .execute(&mut *tx)
    .await
    .map_err(db_error)?;

    sqlx::query(
        "UPDATE automation_flows SET current_draft_id = $2, updated_at = NOW() WHERE id = $1",
    )
    .bind(flow_id)
    .bind(draft_id)
    .execute(&mut *tx)
    .await
    .map_err(db_error)?;

    let flow = fetch_flow(&mut tx, flow_id).await?;
    tx.commit().await.map_err(db_error)?;
    Ok(flow)
}

async fn fetch_flow(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    flow_id: Uuid,
) -> Result<FlowRecord, AutomationError> {
    sqlx::query_as::<_, FlowRecord>(
        "SELECT id, owner_id, name, description, tags, status, current_draft_id, \
                current_published_version_id, draft_revision, created_at, updated_at, deleted_at \
         FROM automation_flows WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(flow_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(db_error)?
    .ok_or_else(|| AutomationError::NotFound(format!("flow {flow_id}")))
}

/// Loads a flow and its current draft, if any.
pub async fn get_flow(
    pool: &sqlx::PgPool,
    flow_id: Uuid,
) -> Result<(FlowRecord, Option<FlowVersionRecord>), AutomationError> {
    let flow = sqlx::query_as::<_, FlowRecord>(
        "SELECT id, owner_id, name, description, tags, status, current_draft_id, \
                current_published_version_id, draft_revision, created_at, updated_at, deleted_at \
         FROM automation_flows WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(flow_id)
    .fetch_optional(pool)
    .await
    .map_err(db_error)?
    .ok_or_else(|| AutomationError::NotFound(format!("flow {flow_id}")))?;

    let draft = match flow.current_draft_id {
        Some(draft_id) => get_version_by_id(pool, draft_id).await?,
        None => None,
    };
    Ok((flow, draft))
}

/// Lists live flows, newest first.
pub async fn list_flows(
    pool: &sqlx::PgPool,
    status: Option<&str>,
    search: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<FlowRecord>, AutomationError> {
    sqlx::query_as::<_, FlowRecord>(
        "SELECT id, owner_id, name, description, tags, status, current_draft_id, \
                current_published_version_id, draft_revision, created_at, updated_at, deleted_at \
         FROM automation_flows \
         WHERE deleted_at IS NULL \
           AND ($1::text IS NULL OR status = $1) \
           AND ($2::text IS NULL OR name ILIKE '%' || $2 || '%' OR description ILIKE '%' || $2 || '%') \
         ORDER BY updated_at DESC LIMIT $3 OFFSET $4",
    )
    .bind(status)
    .bind(search)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(db_error)
}

/// Updates mutable flow metadata.
pub async fn update_flow_metadata(
    pool: &sqlx::PgPool,
    flow_id: Uuid,
    name: Option<&str>,
    description: Option<&str>,
    tags: Option<&[String]>,
) -> Result<FlowRecord, AutomationError> {
    sqlx::query(
        "UPDATE automation_flows SET \
             name = COALESCE($2, name), \
             description = COALESCE($3, description), \
             tags = COALESCE($4, tags), \
             updated_at = NOW() \
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(flow_id)
    .bind(name)
    .bind(description)
    .bind(tags)
    .execute(pool)
    .await
    .map_err(|error| match error {
        sqlx::Error::Database(ref db) if db.is_unique_violation() => {
            AutomationError::Conflict("flow.name_duplicate".to_string())
        }
        other => db_error(other),
    })?;
    get_flow(pool, flow_id).await.map(|(flow, _)| flow)
}

/// Sets a flow's lifecycle status (`active` or `archived`).
pub async fn set_flow_status(
    pool: &sqlx::PgPool,
    flow_id: Uuid,
    status: &str,
) -> Result<FlowRecord, AutomationError> {
    sqlx::query(
        "UPDATE automation_flows SET status = $2, updated_at = NOW() \
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(flow_id)
    .bind(status)
    .execute(pool)
    .await
    .map_err(db_error)?;
    get_flow(pool, flow_id).await.map(|(flow, _)| flow)
}

/// Soft-deletes a flow.
pub async fn soft_delete_flow(pool: &sqlx::PgPool, flow_id: Uuid) -> Result<(), AutomationError> {
    let result = sqlx::query(
        "UPDATE automation_flows SET deleted_at = NOW(), updated_at = NOW() \
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(flow_id)
    .execute(pool)
    .await
    .map_err(db_error)?;
    if result.rows_affected() == 0 {
        return Err(AutomationError::NotFound(format!("flow {flow_id}")));
    }
    Ok(())
}

/// Loads one version by id.
pub async fn get_version_by_id(
    pool: &sqlx::PgPool,
    version_id: Uuid,
) -> Result<Option<FlowVersionRecord>, AutomationError> {
    sqlx::query_as::<_, FlowVersionRecord>(
        "SELECT id, flow_id, version_number, status, flow_json, change_note, created_by, \
                created_at, published_by, published_at \
         FROM automation_flow_versions WHERE id = $1",
    )
    .bind(version_id)
    .fetch_optional(pool)
    .await
    .map_err(db_error)
}

/// Loads a version by flow and version number.
pub async fn get_version(
    pool: &sqlx::PgPool,
    flow_id: Uuid,
    version_number: i32,
) -> Result<FlowVersionRecord, AutomationError> {
    sqlx::query_as::<_, FlowVersionRecord>(
        "SELECT id, flow_id, version_number, status, flow_json, change_note, created_by, \
                created_at, published_by, published_at \
         FROM automation_flow_versions \
         WHERE flow_id = $1 AND version_number = $2",
    )
    .bind(flow_id)
    .bind(version_number)
    .fetch_optional(pool)
    .await
    .map_err(db_error)?
    .ok_or_else(|| AutomationError::NotFound(format!("version {version_number}")))
}

/// Lists published and archived versions, newest first.
pub async fn list_versions(
    pool: &sqlx::PgPool,
    flow_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<FlowVersionRecord>, AutomationError> {
    sqlx::query_as::<_, FlowVersionRecord>(
        "SELECT id, flow_id, version_number, status, flow_json, change_note, created_by, \
                created_at, published_by, published_at \
         FROM automation_flow_versions \
         WHERE flow_id = $1 AND version_number IS NOT NULL \
         ORDER BY version_number DESC LIMIT $2 OFFSET $3",
    )
    .bind(flow_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(db_error)
}

/// Result of a draft save.
#[derive(Debug, Clone)]
pub struct SavedDraft {
    /// Stored draft row.
    pub version: FlowVersionRecord,
    /// New revision token.
    pub revision: i64,
}

/// Creates or replaces a flow draft with optimistic concurrency.
///
/// `expected_revision` is `None` for the first save on a flow without a draft.
pub async fn upsert_draft(
    pool: &sqlx::PgPool,
    flow_id: Uuid,
    expected_revision: Option<i64>,
    flow_json: &Value,
    actor: &str,
) -> Result<SavedDraft, AutomationError> {
    let mut tx = pool.begin().await.map_err(db_error)?;
    let flow = fetch_flow(&mut tx, flow_id).await?;
    if let Some(expected) = expected_revision
        && expected != flow.draft_revision
    {
        return Err(AutomationError::Conflict(format!(
            "draft.revision_mismatch current_revision={}",
            flow.draft_revision
        )));
    }

    let revision = flow.draft_revision + 1;
    let version = match flow.current_draft_id {
        Some(draft_id) => {
            sqlx::query(
                "UPDATE automation_flow_versions SET flow_json = $2 \
                 WHERE id = $1 AND status = 'draft'",
            )
            .bind(draft_id)
            .bind(flow_json)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
            sqlx::query_as::<_, FlowVersionRecord>(
                "SELECT id, flow_id, version_number, status, flow_json, change_note, created_by, \
                        created_at, published_by, published_at \
                 FROM automation_flow_versions WHERE id = $1",
            )
            .bind(draft_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(db_error)?
        }
        None => {
            let draft_id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO automation_flow_versions \
                 (id, flow_id, version_number, status, flow_json, created_by) \
                 VALUES ($1, $2, NULL, 'draft', $3, $4)",
            )
            .bind(draft_id)
            .bind(flow_id)
            .bind(flow_json)
            .bind(actor)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
            sqlx::query("UPDATE automation_flows SET current_draft_id = $2 WHERE id = $1")
                .bind(flow_id)
                .bind(draft_id)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            sqlx::query_as::<_, FlowVersionRecord>(
                "SELECT id, flow_id, version_number, status, flow_json, change_note, created_by, \
                        created_at, published_by, published_at \
                 FROM automation_flow_versions WHERE id = $1",
            )
            .bind(draft_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(db_error)?
        }
    };

    sqlx::query(
        "UPDATE automation_flows SET draft_revision = $2, updated_at = NOW() WHERE id = $1",
    )
    .bind(flow_id)
    .bind(revision)
    .execute(&mut *tx)
    .await
    .map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    Ok(SavedDraft { version, revision })
}

/// Publishes the current draft as the next immutable version.
pub async fn publish_flow(
    pool: &sqlx::PgPool,
    flow_id: Uuid,
    expected_revision: i64,
    change_note: &str,
    actor: &str,
) -> Result<FlowVersionRecord, AutomationError> {
    let mut tx = pool.begin().await.map_err(db_error)?;
    let flow = fetch_flow(&mut tx, flow_id).await?;
    if flow.draft_revision != expected_revision {
        return Err(AutomationError::Conflict(format!(
            "draft.revision_mismatch current_revision={}",
            flow.draft_revision
        )));
    }
    let Some(draft_id) = flow.current_draft_id else {
        return Err(AutomationError::Conflict("draft.none".to_string()));
    };
    let draft = sqlx::query_as::<_, FlowVersionRecord>(
        "SELECT id, flow_id, version_number, status, flow_json, change_note, created_by, \
                created_at, published_by, published_at \
         FROM automation_flow_versions WHERE id = $1 AND status = 'draft'",
    )
    .bind(draft_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_error)?
    .ok_or_else(|| AutomationError::Conflict("draft.none".to_string()))?;

    let next_number: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(version_number), 0)::int + 1 FROM automation_flow_versions \
         WHERE flow_id = $1",
    )
    .bind(flow_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(db_error)?;

    let version_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO automation_flow_versions \
         (id, flow_id, version_number, status, flow_json, change_note, created_by, \
          published_by, published_at) \
         VALUES ($1, $2, $3, 'published', $4, $5, $6, $6, NOW())",
    )
    .bind(version_id)
    .bind(flow_id)
    .bind(next_number)
    .bind(&draft.flow_json)
    .bind(change_note)
    .bind(actor)
    .execute(&mut *tx)
    .await
    .map_err(db_error)?;

    sqlx::query("DELETE FROM automation_flow_versions WHERE id = $1")
        .bind(draft_id)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

    sqlx::query(
        "UPDATE automation_flows SET current_draft_id = NULL, \
             current_published_version_id = $2, draft_revision = draft_revision + 1, \
             updated_at = NOW() \
         WHERE id = $1",
    )
    .bind(flow_id)
    .bind(version_id)
    .execute(&mut *tx)
    .await
    .map_err(db_error)?;

    insert_audit_in_tx(
        &mut tx,
        Some(actor),
        "version.publish",
        "version",
        version_id,
        Some(flow_id),
        None,
        serde_json::json!({ "version_number": next_number, "change_note": change_note }),
    )
    .await?;

    let version = sqlx::query_as::<_, FlowVersionRecord>(
        "SELECT id, flow_id, version_number, status, flow_json, change_note, created_by, \
                created_at, published_by, published_at \
         FROM automation_flow_versions WHERE id = $1",
    )
    .bind(version_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    Ok(version)
}

/// Archives a published version so no new runs may use it.
pub async fn archive_version(
    pool: &sqlx::PgPool,
    flow_id: Uuid,
    version_number: i32,
) -> Result<FlowVersionRecord, AutomationError> {
    let result = sqlx::query(
        "UPDATE automation_flow_versions SET status = 'archived' \
         WHERE flow_id = $1 AND version_number = $2 AND status = 'published'",
    )
    .bind(flow_id)
    .bind(version_number)
    .execute(pool)
    .await
    .map_err(db_error)?;
    if result.rows_affected() == 0 {
        return Err(AutomationError::Conflict(
            "version.not_published".to_string(),
        ));
    }
    get_version(pool, flow_id, version_number).await
}

/// Inserts a queued run and returns the stored row.
#[allow(clippy::too_many_arguments)]
pub async fn insert_run(
    pool: &sqlx::PgPool,
    flow_id: Uuid,
    flow_version_id: Uuid,
    trigger_id: Option<Uuid>,
    trigger_kind: &str,
    trigger_node_id: Option<&str>,
    actor_id: Option<&str>,
    inputs: &Value,
    idempotency_key: Option<&str>,
    parent_run_id: Option<Uuid>,
) -> Result<RunRecord, AutomationError> {
    let run_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO automation_runs \
         (id, flow_id, flow_version_id, trigger_id, trigger_kind, trigger_node_id, actor_id, \
          status, inputs, idempotency_key, parent_run_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'queued', $8, $9, $10)",
    )
    .bind(run_id)
    .bind(flow_id)
    .bind(flow_version_id)
    .bind(trigger_id)
    .bind(trigger_kind)
    .bind(trigger_node_id)
    .bind(actor_id)
    .bind(inputs)
    .bind(idempotency_key)
    .bind(parent_run_id)
    .execute(pool)
    .await
    .map_err(|error| match error {
        sqlx::Error::Database(ref db) if db.is_unique_violation() => {
            AutomationError::Conflict("idempotency.duplicate".to_string())
        }
        other => db_error(other),
    })?;
    get_run(pool, run_id).await
}

/// Loads a run by id.
pub async fn get_run(pool: &sqlx::PgPool, run_id: Uuid) -> Result<RunRecord, AutomationError> {
    sqlx::query_as::<_, RunRecord>(
        "SELECT id, flow_id, flow_version_id, trigger_id, trigger_kind, trigger_node_id, \
                actor_id, status, inputs, outputs, summary, error, idempotency_key, \
                parent_run_id, cancel_requested_at, queued_at, started_at, ended_at, \
                heartbeat_at, message_count, created_at \
         FROM automation_runs WHERE id = $1",
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await
    .map_err(db_error)?
    .ok_or_else(|| AutomationError::NotFound(format!("run {run_id}")))
}

/// Lists runs with optional filters.
#[allow(clippy::too_many_arguments)]
pub async fn list_runs(
    pool: &sqlx::PgPool,
    flow_id: Option<Uuid>,
    status: Option<&str>,
    trigger_kind: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<RunRecord>, AutomationError> {
    sqlx::query_as::<_, RunRecord>(
        "SELECT id, flow_id, flow_version_id, trigger_id, trigger_kind, trigger_node_id, \
                actor_id, status, inputs, outputs, summary, error, idempotency_key, \
                parent_run_id, cancel_requested_at, queued_at, started_at, ended_at, \
                heartbeat_at, message_count, created_at \
         FROM automation_runs \
         WHERE ($1::uuid IS NULL OR flow_id = $1) \
           AND ($2::text IS NULL OR status = $2) \
           AND ($3::text IS NULL OR trigger_kind = $3) \
         ORDER BY created_at DESC LIMIT $4 OFFSET $5",
    )
    .bind(flow_id)
    .bind(status)
    .bind(trigger_kind)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(db_error)
}

/// Marks a run as running.
pub async fn mark_run_running(pool: &sqlx::PgPool, run_id: Uuid) -> Result<(), AutomationError> {
    sqlx::query(
        "UPDATE automation_runs SET status = 'running', started_at = COALESCE(started_at, NOW()), \
             heartbeat_at = NOW() \
         WHERE id = $1 AND status = 'queued'",
    )
    .bind(run_id)
    .execute(pool)
    .await
    .map_err(db_error)?;
    Ok(())
}

/// Records an executor heartbeat.
pub async fn heartbeat(pool: &sqlx::PgPool, run_id: Uuid) -> Result<(), AutomationError> {
    sqlx::query("UPDATE automation_runs SET heartbeat_at = NOW() WHERE id = $1")
        .bind(run_id)
        .execute(pool)
        .await
        .map_err(db_error)?;
    Ok(())
}

/// Sets a run's retry parent.
pub async fn set_run_parent(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    parent_run_id: Option<Uuid>,
) -> Result<(), AutomationError> {
    sqlx::query("UPDATE automation_runs SET parent_run_id = $2 WHERE id = $1")
        .bind(run_id)
        .bind(parent_run_id)
        .execute(pool)
        .await
        .map_err(db_error)?;
    Ok(())
}

/// Finishes a run with a terminal status.
pub async fn finish_run(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    status: &str,
    outputs: &Value,
    summary: &Value,
    error: Option<&Value>,
    message_count: i64,
) -> Result<(), AutomationError> {
    sqlx::query(
        "UPDATE automation_runs SET status = $2, outputs = $3, summary = $4, error = $5, \
             message_count = $6, ended_at = NOW(), heartbeat_at = NOW() \
         WHERE id = $1",
    )
    .bind(run_id)
    .bind(status)
    .bind(outputs)
    .bind(summary)
    .bind(error)
    .bind(message_count)
    .execute(pool)
    .await
    .map_err(db_error)?;
    Ok(())
}

/// Marks every non-terminal run as interrupted at process startup.
///
/// The executor is single-process in v1, so a queued or running row that
/// survives a restart has no worker behind it and must not stay claimed.
pub async fn mark_interrupted_runs(pool: &sqlx::PgPool) -> Result<u64, AutomationError> {
    let result = sqlx::query(
        "UPDATE automation_runs SET status = 'interrupted', ended_at = NOW(), \
             heartbeat_at = NOW(), \
             error = COALESCE(error, '{\"code\":\"run.interrupted\",\"message\":\"executor restarted\"}'::jsonb) \
         WHERE status IN ('queued','running')",
    )
    .execute(pool)
    .await
    .map_err(db_error)?;
    Ok(result.rows_affected())
}

/// Requests cancellation of a non-terminal run.
pub async fn request_cancel(
    pool: &sqlx::PgPool,
    run_id: Uuid,
) -> Result<RunRecord, AutomationError> {
    let run = get_run(pool, run_id).await?;
    if matches!(
        run.status.as_str(),
        "succeeded" | "partial" | "failed" | "cancelled" | "interrupted"
    ) {
        return Err(AutomationError::Conflict(
            "run.already_terminal".to_string(),
        ));
    }
    sqlx::query(
        "UPDATE automation_runs SET cancel_requested_at = COALESCE(cancel_requested_at, NOW()) \
         WHERE id = $1",
    )
    .bind(run_id)
    .execute(pool)
    .await
    .map_err(db_error)?;
    get_run(pool, run_id).await
}

/// Inserts pending node rows for every node in the flow.
pub async fn insert_pending_node_runs(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    nodes: &[(String, String, u32)],
) -> Result<(), AutomationError> {
    for (node_id, node_type_id, node_type_version) in nodes {
        sqlx::query(
            "INSERT INTO automation_node_runs \
             (run_id, node_id, node_type_id, node_type_version, attempt, status) \
             VALUES ($1, $2, $3, $4, 1, 'pending') \
             ON CONFLICT (run_id, node_id, attempt) DO NOTHING",
        )
        .bind(run_id)
        .bind(node_id)
        .bind(node_type_id)
        .bind(*node_type_version as i32)
        .execute(pool)
        .await
        .map_err(db_error)?;
    }
    Ok(())
}

/// Updates one node run row.
#[allow(clippy::too_many_arguments)]
pub async fn update_node_run(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    node_id: &str,
    attempt: i32,
    status: &str,
    started_at: Option<DateTime<Utc>>,
    ended_at: Option<DateTime<Utc>>,
    duration_ms: Option<i64>,
    input_message: Option<&Value>,
    output_messages: Option<&Value>,
    output_truncated: bool,
    messages_in: i64,
    messages_out: i64,
    error: Option<&Value>,
) -> Result<(), AutomationError> {
    sqlx::query(
        "UPDATE automation_node_runs SET status = $4, started_at = COALESCE($5, started_at), \
             ended_at = COALESCE($6, ended_at), duration_ms = $7, input_message = $8, \
             output_messages = $9, output_truncated = $10, \
             messages_in = messages_in + $11, messages_out = messages_out + $12, \
             error = $13 \
         WHERE run_id = $1 AND node_id = $2 AND attempt = $3",
    )
    .bind(run_id)
    .bind(node_id)
    .bind(attempt)
    .bind(status)
    .bind(started_at)
    .bind(ended_at)
    .bind(duration_ms)
    .bind(input_message)
    .bind(output_messages)
    .bind(output_truncated)
    .bind(messages_in)
    .bind(messages_out)
    .bind(error)
    .execute(pool)
    .await
    .map_err(db_error)?;
    Ok(())
}

/// Inserts a node run row for an attempt if it does not exist yet.
///
/// Retries produce rows for attempts beyond the first; the executor observer
/// calls this before updating an attempt so the update always targets a row.
pub async fn ensure_node_run(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    node_id: &str,
    node_type_id: &str,
    node_type_version: u32,
    attempt: i32,
) -> Result<(), AutomationError> {
    sqlx::query(
        "INSERT INTO automation_node_runs \
         (run_id, node_id, node_type_id, node_type_version, attempt, status) \
         VALUES ($1, $2, $3, $4, $5, 'pending') \
         ON CONFLICT (run_id, node_id, attempt) DO NOTHING",
    )
    .bind(run_id)
    .bind(node_id)
    .bind(node_type_id)
    .bind(node_type_version as i32)
    .bind(attempt)
    .execute(pool)
    .await
    .map_err(db_error)?;
    Ok(())
}

/// Loads node run rows for a run.
pub async fn list_node_runs(
    pool: &sqlx::PgPool,
    run_id: Uuid,
) -> Result<Vec<NodeRunRecord>, AutomationError> {
    sqlx::query_as::<_, NodeRunRecord>(
        "SELECT id, run_id, node_id, node_type_id, node_type_version, attempt, status, \
                started_at, ended_at, duration_ms, input_message, output_messages, \
                output_truncated, messages_in, messages_out, error, created_at \
         FROM automation_node_runs WHERE run_id = $1 ORDER BY created_at, node_id",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .map_err(db_error)
}

/// Inserts a message record.
#[allow(clippy::too_many_arguments)]
pub async fn insert_message(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    node_id: Option<&str>,
    direction: &str,
    output_port: Option<&str>,
    msg_id: &str,
    payload: Option<&Value>,
    size_bytes: i32,
    truncated: bool,
    error: Option<&Value>,
) -> Result<(), AutomationError> {
    sqlx::query(
        "INSERT INTO automation_messages \
         (run_id, node_id, direction, output_port, msg_id, payload, size_bytes, truncated, error) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(run_id)
    .bind(node_id)
    .bind(direction)
    .bind(output_port)
    .bind(msg_id)
    .bind(payload)
    .bind(size_bytes)
    .bind(truncated)
    .bind(error)
    .execute(pool)
    .await
    .map_err(db_error)?;
    Ok(())
}

/// Lists message records for a run.
pub async fn list_messages(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    node_id: Option<&str>,
    direction: Option<&str>,
    after_id: Option<i64>,
    limit: i64,
) -> Result<Vec<MessageRecord>, AutomationError> {
    sqlx::query_as::<_, MessageRecord>(
        "SELECT id, run_id, node_id, direction, output_port, msg_id, ts, payload, \
                size_bytes, truncated, error \
         FROM automation_messages \
         WHERE run_id = $1 \
           AND ($2::text IS NULL OR node_id = $2) \
           AND ($3::text IS NULL OR direction = $3) \
           AND ($4::bigint IS NULL OR id > $4) \
         ORDER BY id LIMIT $5",
    )
    .bind(run_id)
    .bind(node_id)
    .bind(direction)
    .bind(after_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(db_error)
}

/// Inserts a run log.
pub async fn insert_log(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    node_id: Option<&str>,
    level: &str,
    message: &str,
    fields: Option<&Value>,
) -> Result<(), AutomationError> {
    sqlx::query(
        "INSERT INTO automation_run_logs (run_id, node_id, level, message, fields) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(run_id)
    .bind(node_id)
    .bind(level)
    .bind(message)
    .bind(fields)
    .execute(pool)
    .await
    .map_err(db_error)?;
    Ok(())
}

/// Lists run logs.
pub async fn list_logs(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    node_id: Option<&str>,
    after_id: Option<i64>,
    limit: i64,
) -> Result<Vec<LogRecord>, AutomationError> {
    sqlx::query_as::<_, LogRecord>(
        "SELECT id, run_id, node_id, ts, level, message, fields \
         FROM automation_run_logs \
         WHERE run_id = $1 \
           AND ($2::text IS NULL OR node_id = $2) \
           AND ($3::bigint IS NULL OR id > $3) \
         ORDER BY id LIMIT $4",
    )
    .bind(run_id)
    .bind(node_id)
    .bind(after_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(db_error)
}

/// Loads flow and global context for a flow.
pub async fn get_context(
    pool: &sqlx::PgPool,
    flow_scope_id: &str,
    global_scope_id: &str,
) -> Result<(Vec<ContextRecord>, Vec<ContextRecord>), AutomationError> {
    let flow = sqlx::query_as::<_, ContextRecord>(
        "SELECT scope, scope_id, key, value, updated_at, updated_by_run_id \
         FROM automation_context WHERE scope = 'flow' AND scope_id = $1 ORDER BY key",
    )
    .bind(flow_scope_id)
    .fetch_all(pool)
    .await
    .map_err(db_error)?;
    let global = sqlx::query_as::<_, ContextRecord>(
        "SELECT scope, scope_id, key, value, updated_at, updated_by_run_id \
         FROM automation_context WHERE scope = 'global' AND scope_id = $1 ORDER BY key",
    )
    .bind(global_scope_id)
    .fetch_all(pool)
    .await
    .map_err(db_error)?;
    Ok((flow, global))
}

/// Loads flow context for a flow and every subflow instance scope.
pub async fn get_scoped_flow_context(
    pool: &sqlx::PgPool,
    flow_id: &str,
) -> Result<Vec<ContextRecord>, AutomationError> {
    sqlx::query_as::<_, ContextRecord>(
        "SELECT scope, scope_id, key, value, updated_at, updated_by_run_id \
         FROM automation_context \
         WHERE scope = 'flow' AND (scope_id = $1 OR scope_id LIKE $1 || ':%') \
         ORDER BY scope_id, key",
    )
    .bind(flow_id)
    .fetch_all(pool)
    .await
    .map_err(db_error)
}

/// Writes one context key atomically.
pub async fn set_context(
    pool: &sqlx::PgPool,
    scope: &str,
    scope_id: &str,
    key: &str,
    value: &Value,
    run_id: Option<Uuid>,
) -> Result<(), AutomationError> {
    sqlx::query(
        "INSERT INTO automation_context (scope, scope_id, key, value, updated_at, updated_by_run_id) \
         VALUES ($1, $2, $3, $4, NOW(), $5) \
         ON CONFLICT (scope, scope_id, key) DO UPDATE \
         SET value = EXCLUDED.value, updated_at = NOW(), \
             updated_by_run_id = EXCLUDED.updated_by_run_id",
    )
    .bind(scope)
    .bind(scope_id)
    .bind(key)
    .bind(value)
    .bind(run_id)
    .execute(pool)
    .await
    .map_err(db_error)?;
    Ok(())
}

/// Deletes durable flow context.
pub async fn delete_context(
    pool: &sqlx::PgPool,
    scope: &str,
    scope_id: &str,
) -> Result<(), AutomationError> {
    let statement = if scope == "flow" {
        "DELETE FROM automation_context \
         WHERE scope = $1 AND (scope_id = $2 OR scope_id LIKE $2 || ':%')"
    } else {
        "DELETE FROM automation_context WHERE scope = $1 AND scope_id = $2"
    };
    sqlx::query(statement)
        .bind(scope)
        .bind(scope_id)
        .execute(pool)
        .await
        .map_err(db_error)?;
    Ok(())
}

/// Inserts an audit event.
#[allow(clippy::too_many_arguments)]
pub async fn insert_audit(
    pool: &sqlx::PgPool,
    actor_id: Option<&str>,
    action: &str,
    target_type: &str,
    target_id: Uuid,
    flow_id: Option<Uuid>,
    run_id: Option<Uuid>,
    metadata: Value,
) -> Result<(), AutomationError> {
    sqlx::query(
        "INSERT INTO automation_audit_events \
         (actor_id, action, target_type, target_id, flow_id, run_id, outcome, metadata) \
         VALUES ($1, $2, $3, $4, $5, $6, 'success', $7)",
    )
    .bind(actor_id)
    .bind(action)
    .bind(target_type)
    .bind(target_id)
    .bind(flow_id)
    .bind(run_id)
    .bind(metadata)
    .execute(pool)
    .await
    .map_err(db_error)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_audit_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_id: Option<&str>,
    action: &str,
    target_type: &str,
    target_id: Uuid,
    flow_id: Option<Uuid>,
    run_id: Option<Uuid>,
    metadata: Value,
) -> Result<(), AutomationError> {
    sqlx::query(
        "INSERT INTO automation_audit_events \
         (actor_id, action, target_type, target_id, flow_id, run_id, outcome, metadata) \
         VALUES ($1, $2, $3, $4, $5, $6, 'success', $7)",
    )
    .bind(actor_id)
    .bind(action)
    .bind(target_type)
    .bind(target_id)
    .bind(flow_id)
    .bind(run_id)
    .bind(metadata)
    .execute(&mut **tx)
    .await
    .map_err(db_error)?;
    Ok(())
}

/// Lists audit events for a flow.
#[allow(clippy::too_many_arguments)]
pub async fn list_audit(
    pool: &sqlx::PgPool,
    flow_id: Uuid,
    action: Option<&str>,
    actor_id: Option<&str>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    limit: i64,
    offset: i64,
) -> Result<Vec<AuditEventRecord>, AutomationError> {
    sqlx::query_as::<_, AuditEventRecord>(
        "SELECT id, actor_id, action, target_type, target_id, flow_id, run_id, outcome, \
                metadata, created_at \
         FROM automation_audit_events \
         WHERE flow_id = $1 \
           AND ($2::text IS NULL OR action = $2) \
           AND ($3::text IS NULL OR actor_id = $3) \
           AND ($4::timestamptz IS NULL OR created_at >= $4) \
           AND ($5::timestamptz IS NULL OR created_at <= $5) \
         ORDER BY created_at DESC LIMIT $6 OFFSET $7",
    )
    .bind(flow_id)
    .bind(action)
    .bind(actor_id)
    .bind(from)
    .bind(to)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(db_error)
}

/// Counts audit events matching the same filters as [`list_audit`].
pub async fn count_audit(
    pool: &sqlx::PgPool,
    flow_id: Uuid,
    action: Option<&str>,
    actor_id: Option<&str>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
) -> Result<i64, AutomationError> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM automation_audit_events \
         WHERE flow_id = $1 \
           AND ($2::text IS NULL OR action = $2) \
           AND ($3::text IS NULL OR actor_id = $3) \
           AND ($4::timestamptz IS NULL OR created_at >= $4) \
           AND ($5::timestamptz IS NULL OR created_at <= $5)",
    )
    .bind(flow_id)
    .bind(action)
    .bind(actor_id)
    .bind(from)
    .bind(to)
    .fetch_one(pool)
    .await
    .map_err(db_error)
}

/// Lists triggers for a flow.
pub async fn list_triggers(
    pool: &sqlx::PgPool,
    flow_id: Uuid,
) -> Result<Vec<TriggerRecord>, AutomationError> {
    sqlx::query_as::<_, TriggerRecord>(
        "SELECT id, flow_id, flow_version_id, node_id, kind, enabled, repeat_kind, \
                interval_seconds, cron_expression, timezone, overlap_policy, missed_policy, \
                next_occurrence_at, last_occurrence_at, created_by, created_at, updated_at \
         FROM automation_triggers WHERE flow_id = $1 ORDER BY kind, created_at",
    )
    .bind(flow_id)
    .fetch_all(pool)
    .await
    .map_err(db_error)
}

/// Loads one trigger.
pub async fn get_trigger(
    pool: &sqlx::PgPool,
    trigger_id: Uuid,
) -> Result<TriggerRecord, AutomationError> {
    sqlx::query_as::<_, TriggerRecord>(
        "SELECT id, flow_id, flow_version_id, node_id, kind, enabled, repeat_kind, \
                interval_seconds, cron_expression, timezone, overlap_policy, missed_policy, \
                next_occurrence_at, last_occurrence_at, created_by, created_at, updated_at \
         FROM automation_triggers WHERE id = $1",
    )
    .bind(trigger_id)
    .fetch_optional(pool)
    .await
    .map_err(db_error)?
    .ok_or_else(|| AutomationError::NotFound(format!("trigger {trigger_id}")))
}

/// Upserts an API trigger for a flow.
pub async fn upsert_api_trigger(
    pool: &sqlx::PgPool,
    flow_id: Uuid,
    enabled: bool,
    overlap_policy: &str,
    missed_policy: &str,
    actor: &str,
) -> Result<TriggerRecord, AutomationError> {
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM automation_triggers WHERE flow_id = $1 AND kind = 'api' LIMIT 1",
    )
    .bind(flow_id)
    .fetch_optional(pool)
    .await
    .map_err(db_error)?;

    let trigger_id = match existing {
        Some(id) => {
            sqlx::query(
                "UPDATE automation_triggers SET enabled = $2, overlap_policy = $3, \
                     missed_policy = $4, updated_at = NOW() WHERE id = $1",
            )
            .bind(id)
            .bind(enabled)
            .bind(overlap_policy)
            .bind(missed_policy)
            .execute(pool)
            .await
            .map_err(db_error)?;
            id
        }
        None => {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO automation_triggers \
                 (id, flow_id, kind, enabled, overlap_policy, missed_policy, created_by) \
                 VALUES ($1, $2, 'api', $3, $4, $5, $6)",
            )
            .bind(id)
            .bind(flow_id)
            .bind(enabled)
            .bind(overlap_policy)
            .bind(missed_policy)
            .bind(actor)
            .execute(pool)
            .await
            .map_err(db_error)?;
            id
        }
    };
    get_trigger(pool, trigger_id).await
}

/// Updates a trigger's mutable fields.
pub async fn update_trigger(
    pool: &sqlx::PgPool,
    trigger_id: Uuid,
    enabled: Option<bool>,
    overlap_policy: Option<&str>,
    missed_policy: Option<&str>,
) -> Result<TriggerRecord, AutomationError> {
    sqlx::query(
        "UPDATE automation_triggers SET enabled = COALESCE($2, enabled), \
             overlap_policy = COALESCE($3, overlap_policy), \
             missed_policy = COALESCE($4, missed_policy), updated_at = NOW() \
         WHERE id = $1",
    )
    .bind(trigger_id)
    .bind(enabled)
    .bind(overlap_policy)
    .bind(missed_policy)
    .execute(pool)
    .await
    .map_err(db_error)?;
    get_trigger(pool, trigger_id).await
}

/// Deletes every inject trigger for a flow (before re-reconciling).
pub async fn delete_inject_triggers(
    pool: &sqlx::PgPool,
    flow_id: Uuid,
) -> Result<(), AutomationError> {
    sqlx::query("DELETE FROM automation_triggers WHERE flow_id = $1 AND kind = 'inject'")
        .bind(flow_id)
        .execute(pool)
        .await
        .map_err(db_error)?;
    Ok(())
}

/// Inserts a reconciled inject trigger.
#[allow(clippy::too_many_arguments)]
pub async fn insert_inject_trigger(
    pool: &sqlx::PgPool,
    flow_id: Uuid,
    flow_version_id: Uuid,
    node_id: &str,
    repeat_kind: &str,
    interval_seconds: Option<i32>,
    cron_expression: Option<&str>,
    timezone: &str,
    next_occurrence_at: DateTime<Utc>,
    actor: &str,
) -> Result<TriggerRecord, AutomationError> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO automation_triggers \
         (id, flow_id, flow_version_id, node_id, kind, enabled, repeat_kind, \
          interval_seconds, cron_expression, timezone, next_occurrence_at, created_by) \
         VALUES ($1, $2, $3, $4, 'inject', TRUE, $5, $6, $7, $8, $9, $10) \
         ON CONFLICT (flow_version_id, node_id) WHERE kind = 'inject' DO UPDATE \
         SET repeat_kind = EXCLUDED.repeat_kind, interval_seconds = EXCLUDED.interval_seconds, \
             cron_expression = EXCLUDED.cron_expression, timezone = EXCLUDED.timezone, \
             next_occurrence_at = EXCLUDED.next_occurrence_at, updated_at = NOW()",
    )
    .bind(id)
    .bind(flow_id)
    .bind(flow_version_id)
    .bind(node_id)
    .bind(repeat_kind)
    .bind(interval_seconds)
    .bind(cron_expression)
    .bind(timezone)
    .bind(next_occurrence_at)
    .bind(actor)
    .execute(pool)
    .await
    .map_err(db_error)?;
    get_trigger(pool, id).await
}

/// Lists inject triggers that are due at or before `now`.
pub async fn due_triggers(
    pool: &sqlx::PgPool,
    now: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<TriggerRecord>, AutomationError> {
    sqlx::query_as::<_, TriggerRecord>(
        "SELECT id, flow_id, flow_version_id, node_id, kind, enabled, repeat_kind, \
                interval_seconds, cron_expression, timezone, overlap_policy, missed_policy, \
                next_occurrence_at, last_occurrence_at, created_by, created_at, updated_at \
         FROM automation_triggers \
         WHERE kind = 'inject' AND enabled AND next_occurrence_at IS NOT NULL \
           AND next_occurrence_at <= $1 \
         ORDER BY next_occurrence_at LIMIT $2",
    )
    .bind(now)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(db_error)
}

/// Advances a trigger's next occurrence and records the last occurrence.
pub async fn advance_trigger(
    pool: &sqlx::PgPool,
    trigger_id: Uuid,
    last_occurrence_at: DateTime<Utc>,
    next_occurrence_at: DateTime<Utc>,
) -> Result<(), AutomationError> {
    sqlx::query(
        "UPDATE automation_triggers SET last_occurrence_at = $2, \
             next_occurrence_at = $3, updated_at = NOW() WHERE id = $1",
    )
    .bind(trigger_id)
    .bind(last_occurrence_at)
    .bind(next_occurrence_at)
    .execute(pool)
    .await
    .map_err(db_error)?;
    Ok(())
}

/// Deletes a trigger.
pub async fn delete_trigger(pool: &sqlx::PgPool, trigger_id: Uuid) -> Result<(), AutomationError> {
    let result = sqlx::query("DELETE FROM automation_triggers WHERE id = $1")
        .bind(trigger_id)
        .execute(pool)
        .await
        .map_err(db_error)?;
    if result.rows_affected() == 0 {
        return Err(AutomationError::NotFound(format!("trigger {trigger_id}")));
    }
    Ok(())
}

/// Returns the most recent run for a flow.
pub async fn latest_run(
    pool: &sqlx::PgPool,
    flow_id: Uuid,
) -> Result<Option<RunRecord>, AutomationError> {
    sqlx::query_as::<_, RunRecord>(
        "SELECT id, flow_id, flow_version_id, trigger_id, trigger_kind, trigger_node_id, \
                actor_id, status, inputs, outputs, summary, error, idempotency_key, \
                parent_run_id, cancel_requested_at, queued_at, started_at, ended_at, \
                heartbeat_at, message_count, created_at \
         FROM automation_runs WHERE flow_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(flow_id)
    .fetch_optional(pool)
    .await
    .map_err(db_error)
}

/// Returns the earliest next scheduled occurrence for a flow.
pub async fn next_scheduled_run(
    pool: &sqlx::PgPool,
    flow_id: Uuid,
) -> Result<Option<DateTime<Utc>>, AutomationError> {
    sqlx::query_scalar(
        "SELECT MIN(next_occurrence_at) FROM automation_triggers \
         WHERE flow_id = $1 AND kind = 'inject' AND enabled",
    )
    .bind(flow_id)
    .fetch_one(pool)
    .await
    .map_err(db_error)
}

/// Counts queued or running runs, optionally scoped to one flow.
pub async fn count_active_runs(
    pool: &sqlx::PgPool,
    flow_id: Option<Uuid>,
) -> Result<i64, AutomationError> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM automation_runs \
         WHERE status IN ('queued','running') \
           AND ($1::uuid IS NULL OR flow_id = $1)",
    )
    .bind(flow_id)
    .fetch_one(pool)
    .await
    .map_err(db_error)
}

/// Counts runs started for one trigger since an instant.
pub async fn count_trigger_runs_since(
    pool: &sqlx::PgPool,
    trigger_id: Uuid,
    since: DateTime<Utc>,
) -> Result<i64, AutomationError> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM automation_runs WHERE trigger_id = $1 AND queued_at >= $2",
    )
    .bind(trigger_id)
    .bind(since)
    .fetch_one(pool)
    .await
    .map_err(db_error)
}

/// Counts runs for paginated responses.
pub async fn count_runs(
    pool: &sqlx::PgPool,
    flow_id: Option<Uuid>,
    status: Option<&str>,
    trigger_kind: Option<&str>,
) -> Result<i64, AutomationError> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM automation_runs \
         WHERE ($1::uuid IS NULL OR flow_id = $1) \
           AND ($2::text IS NULL OR status = $2) \
           AND ($3::text IS NULL OR trigger_kind = $3)",
    )
    .bind(flow_id)
    .bind(status)
    .bind(trigger_kind)
    .fetch_one(pool)
    .await
    .map_err(db_error)
}

/// Counts live flows for paginated responses.
pub async fn count_flows(
    pool: &sqlx::PgPool,
    status: Option<&str>,
    search: Option<&str>,
) -> Result<i64, AutomationError> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM automation_flows \
         WHERE deleted_at IS NULL \
           AND ($1::text IS NULL OR status = $1) \
           AND ($2::text IS NULL OR name ILIKE '%' || $2 || '%' OR description ILIKE '%' || $2 || '%')",
    )
    .bind(status)
    .bind(search)
    .fetch_one(pool)
    .await
    .map_err(db_error)
}

/// Counts versions for a flow.
pub async fn count_versions(pool: &sqlx::PgPool, flow_id: Uuid) -> Result<i64, AutomationError> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM automation_flow_versions \
         WHERE flow_id = $1 AND version_number IS NOT NULL",
    )
    .bind(flow_id)
    .fetch_one(pool)
    .await
    .map_err(db_error)
}

/// Finds a run by idempotency key scoped to a flow or trigger.
pub async fn find_run_by_idempotency(
    pool: &sqlx::PgPool,
    flow_id: Uuid,
    trigger_id: Option<Uuid>,
    key: &str,
) -> Result<Option<RunRecord>, AutomationError> {
    let row = sqlx::query_as::<_, RunRecord>(
        "SELECT id, flow_id, flow_version_id, trigger_id, trigger_kind, trigger_node_id, \
                actor_id, status, inputs, outputs, summary, error, idempotency_key, \
                parent_run_id, cancel_requested_at, queued_at, started_at, ended_at, \
                heartbeat_at, message_count, created_at \
         FROM automation_runs \
         WHERE flow_id = $1 AND idempotency_key = $3 \
           AND (($2::uuid IS NULL AND trigger_id IS NULL) OR trigger_id = $2) \
         LIMIT 1",
    )
    .bind(flow_id)
    .bind(trigger_id)
    .bind(key)
    .fetch_optional(pool)
    .await
    .map_err(db_error)?;
    Ok(row)
}

/// Verifies that a row exists; used by tests and defensive checks.
pub async fn run_exists(pool: &sqlx::PgPool, run_id: Uuid) -> Result<bool, AutomationError> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM automation_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(db_error)?;
    Ok(count > 0)
}
