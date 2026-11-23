//! Retention pruning for automation runs and audit events.
//!
//! Implements FR-023 / FR-024 of `specs/034-production-hardening/spec.md`.
//! Terminal runs are deleted with their node runs, messages, and logs once
//! their terminal time is older than the run window. Audit events outlive
//! runs and are pruned independently on their own window. `queued` and
//! `running` rows are never eligible, and a run with a retained child run is
//! deferred to a later pass instead of breaking the self-reference.

use sqlx::{AssertSqlSafe, PgPool};

use crate::automation::error::AutomationError;

/// Counts from one automation retention pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetentionReport {
    /// Terminal runs deleted.
    pub runs_deleted: u64,
    /// Node-run rows deleted with their runs.
    pub node_runs_deleted: u64,
    /// Message rows deleted with their runs.
    pub messages_deleted: u64,
    /// Log rows deleted with their runs.
    pub logs_deleted: u64,
    /// Audit events deleted by their own window.
    pub audit_events_deleted: u64,
}

/// Selects terminal runs past the run window whose children are all gone.
///
/// `$1` is the run retention window in days. The `parent_run_id`
/// self-reference has no cascade, so a parent with a retained child is left
/// for a later pass rather than failing the foreign key.
const EXPIRED_RUNS: &str = "\
    SELECT target.id FROM automation_runs target \
    WHERE target.status IN \
        ('succeeded', 'partial', 'failed', 'cancelled', 'interrupted') \
      AND target.ended_at IS NOT NULL \
      AND target.ended_at < NOW() - ($1::bigint * INTERVAL '1 day') \
      AND NOT EXISTS \
        (SELECT 1 FROM automation_runs child WHERE child.parent_run_id = target.id)";

fn db_error(error: sqlx::Error) -> AutomationError {
    AutomationError::Infrastructure(error.to_string())
}

/// Prunes expired automation rows and returns the deleted counts.
///
/// `run_retention_days` applies to terminal runs (and their cascaded node
/// runs, messages, and logs); `audit_retention_days` applies to audit
/// events. Both windows must be positive. The pass is idempotent: rerunning
/// it deletes nothing already removed and returns zero counts.
pub async fn prune(
    pool: &PgPool,
    run_retention_days: u32,
    audit_retention_days: u32,
) -> Result<RetentionReport, AutomationError> {
    if run_retention_days == 0 || audit_retention_days == 0 {
        return Err(AutomationError::Validation(
            "retention windows must be positive".to_string(),
        ));
    }
    let run_days = i64::from(run_retention_days);
    let audit_days = i64::from(audit_retention_days);

    let mut tx = pool.begin().await.map_err(db_error)?;

    // Children first so the counts are explicit; the run delete cascades
    // anyway, but `rows_affected` on the cascade is not observable.
    let node_runs_deleted = sqlx::query(AssertSqlSafe(format!(
        "DELETE FROM automation_node_runs WHERE run_id IN ({EXPIRED_RUNS})"
    )))
    .bind(run_days)
    .execute(&mut *tx)
    .await
    .map_err(db_error)?
    .rows_affected();
    let messages_deleted = sqlx::query(AssertSqlSafe(format!(
        "DELETE FROM automation_messages WHERE run_id IN ({EXPIRED_RUNS})"
    )))
    .bind(run_days)
    .execute(&mut *tx)
    .await
    .map_err(db_error)?
    .rows_affected();
    let logs_deleted = sqlx::query(AssertSqlSafe(format!(
        "DELETE FROM automation_run_logs WHERE run_id IN ({EXPIRED_RUNS})"
    )))
    .bind(run_days)
    .execute(&mut *tx)
    .await
    .map_err(db_error)?
    .rows_affected();
    let runs_deleted = sqlx::query(AssertSqlSafe(format!(
        "DELETE FROM automation_runs WHERE id IN ({EXPIRED_RUNS})"
    )))
    .bind(run_days)
    .execute(&mut *tx)
    .await
    .map_err(db_error)?
    .rows_affected();

    let audit_events_deleted = sqlx::query(
        "DELETE FROM automation_audit_events \
         WHERE created_at < NOW() - ($1::bigint * INTERVAL '1 day')",
    )
    .bind(audit_days)
    .execute(&mut *tx)
    .await
    .map_err(db_error)?
    .rows_affected();

    tx.commit().await.map_err(db_error)?;

    Ok(RetentionReport {
        runs_deleted,
        node_runs_deleted,
        messages_deleted,
        logs_deleted,
        audit_events_deleted,
    })
}
