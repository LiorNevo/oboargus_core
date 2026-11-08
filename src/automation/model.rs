//! Durable automation model types.
//!
//! Mirrors `specs/030-automation-workflow-engine/data-model.md`. These types
//! are shared by the core persistence layer, the runtime, and the HTTP routes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Lifecycle of a flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowStatus {
    /// Editable/run-able.
    Active,
    /// Read-only; no new runs.
    Archived,
}

impl FlowStatus {
    /// Database text value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
        }
    }
}

/// Lifecycle of a flow version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionStatus {
    /// Mutable working copy.
    Draft,
    /// Immutable, run-able snapshot.
    Published,
    /// No new runs; history remains.
    Archived,
}

impl VersionStatus {
    /// Database text value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Published => "published",
            Self::Archived => "archived",
        }
    }
}

/// Status of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Waiting for an executor.
    Queued,
    /// Currently executing.
    Running,
    /// Completed successfully.
    Succeeded,
    /// Completed with handled/partial failures.
    Partial,
    /// Failed with an uncaught error or limit.
    Failed,
    /// Cancelled by an operator.
    Cancelled,
    /// Executor disappeared before completion.
    Interrupted,
}

impl RunStatus {
    /// Database text value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Partial => "partial",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }

    /// True for terminal statuses.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Partial | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }

    /// Parses a database text value.
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "succeeded" => Self::Succeeded,
            "partial" => Self::Partial,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "interrupted" => Self::Interrupted,
            _ => return None,
        })
    }
}

/// Status of one node run attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRunStatus {
    /// Waiting for a message.
    Pending,
    /// Ready to execute.
    Ready,
    /// Executing.
    Running,
    /// Succeeded.
    Succeeded,
    /// Failed.
    Failed,
    /// Never received a message.
    Skipped,
    /// Cancelled.
    Cancelled,
}

impl NodeRunStatus {
    /// Database text value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::Cancelled => "cancelled",
        }
    }
}

/// What started a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerKind {
    /// A person pressed Run.
    Manual,
    /// An inject schedule fired.
    Schedule,
    /// An API trigger was invoked.
    Api,
}

impl TriggerKind {
    /// Database text value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Schedule => "schedule",
            Self::Api => "api",
        }
    }
}

/// A flow row.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct FlowRecord {
    /// Flow id.
    pub id: Uuid,
    /// Configured actor metadata.
    pub owner_id: String,
    /// Display name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Tags.
    pub tags: Vec<String>,
    /// Lifecycle status.
    pub status: String,
    /// Current draft id.
    pub current_draft_id: Option<Uuid>,
    /// Current published version id.
    pub current_published_version_id: Option<Uuid>,
    /// Optimistic concurrency token.
    pub draft_revision: i64,
    /// Creation instant.
    pub created_at: DateTime<Utc>,
    /// Last update instant.
    pub updated_at: DateTime<Utc>,
    /// Soft-delete instant.
    pub deleted_at: Option<DateTime<Utc>>,
}

/// A flow version row.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct FlowVersionRecord {
    /// Version id.
    pub id: Uuid,
    /// Parent flow id.
    pub flow_id: Uuid,
    /// Published version number; null for drafts.
    pub version_number: Option<i32>,
    /// Lifecycle status.
    pub status: String,
    /// Canonical flow document.
    pub flow_json: Value,
    /// Change note.
    pub change_note: Option<String>,
    /// Author metadata.
    pub created_by: String,
    /// Creation instant.
    pub created_at: DateTime<Utc>,
    /// Publisher metadata.
    pub published_by: Option<String>,
    /// Publication instant.
    pub published_at: Option<DateTime<Utc>>,
}

/// A trigger row.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TriggerRecord {
    /// Trigger id.
    pub id: Uuid,
    /// Parent flow id.
    pub flow_id: Uuid,
    /// Version that declared the inject schedule.
    pub flow_version_id: Option<Uuid>,
    /// Inject node id.
    pub node_id: Option<String>,
    /// `inject` or `api`.
    pub kind: String,
    /// Enabled state.
    pub enabled: bool,
    /// `interval` or `cron`.
    pub repeat_kind: Option<String>,
    /// Interval seconds.
    pub interval_seconds: Option<i32>,
    /// Cron expression.
    pub cron_expression: Option<String>,
    /// IANA timezone.
    pub timezone: String,
    /// Overlap policy.
    pub overlap_policy: String,
    /// Missed-occurrence policy.
    pub missed_policy: String,
    /// Next occurrence.
    pub next_occurrence_at: Option<DateTime<Utc>>,
    /// Last occurrence.
    pub last_occurrence_at: Option<DateTime<Utc>>,
    /// Author metadata.
    pub created_by: String,
    /// Creation instant.
    pub created_at: DateTime<Utc>,
    /// Last update instant.
    pub updated_at: DateTime<Utc>,
}

/// A run row.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct RunRecord {
    /// Run id.
    pub id: Uuid,
    /// Flow id.
    pub flow_id: Uuid,
    /// Executed version id.
    pub flow_version_id: Uuid,
    /// Trigger id, if any.
    pub trigger_id: Option<Uuid>,
    /// Trigger kind.
    pub trigger_kind: String,
    /// Inject node that started the run.
    pub trigger_node_id: Option<String>,
    /// Actor metadata.
    pub actor_id: Option<String>,
    /// Run status.
    pub status: String,
    /// Runtime inputs.
    pub inputs: Value,
    /// Final outputs.
    pub outputs: Value,
    /// Summary counters.
    pub summary: Value,
    /// Terminal error.
    pub error: Option<Value>,
    /// Idempotency key.
    pub idempotency_key: Option<String>,
    /// Retry parent.
    pub parent_run_id: Option<Uuid>,
    /// Cancellation request instant.
    pub cancel_requested_at: Option<DateTime<Utc>>,
    /// Queue instant.
    pub queued_at: DateTime<Utc>,
    /// Start instant.
    pub started_at: Option<DateTime<Utc>>,
    /// End instant.
    pub ended_at: Option<DateTime<Utc>>,
    /// Executor heartbeat.
    pub heartbeat_at: Option<DateTime<Utc>>,
    /// Total messages processed.
    pub message_count: i64,
    /// Creation instant.
    pub created_at: DateTime<Utc>,
}

/// A node run row.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NodeRunRecord {
    /// Row id.
    pub id: Uuid,
    /// Parent run id.
    pub run_id: Uuid,
    /// Flow node id.
    pub node_id: String,
    /// Node type id.
    pub node_type_id: String,
    /// Node type version.
    pub node_type_version: i32,
    /// Attempt number.
    pub attempt: i32,
    /// Status.
    pub status: String,
    /// Start instant.
    pub started_at: Option<DateTime<Utc>>,
    /// End instant.
    pub ended_at: Option<DateTime<Utc>>,
    /// Duration.
    pub duration_ms: Option<i64>,
    /// Input message.
    pub input_message: Option<Value>,
    /// Output messages.
    pub output_messages: Option<Value>,
    /// Truncation flag.
    pub output_truncated: bool,
    /// Input message count.
    pub messages_in: i64,
    /// Output message count.
    pub messages_out: i64,
    /// Error.
    pub error: Option<Value>,
    /// Creation instant.
    pub created_at: DateTime<Utc>,
}

/// A recorded message row.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct MessageRecord {
    /// Row id.
    pub id: i64,
    /// Run id.
    pub run_id: Uuid,
    /// Node id.
    pub node_id: Option<String>,
    /// `in` or `out`.
    pub direction: String,
    /// Output port.
    pub output_port: Option<String>,
    /// Runtime message id.
    pub msg_id: String,
    /// Timestamp.
    pub ts: DateTime<Utc>,
    /// Redacted payload.
    pub payload: Option<Value>,
    /// Size in bytes.
    pub size_bytes: i32,
    /// Truncated flag.
    pub truncated: bool,
    /// Error payload.
    pub error: Option<Value>,
}

/// A run log row.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct LogRecord {
    /// Row id.
    pub id: i64,
    /// Run id.
    pub run_id: Uuid,
    /// Node id.
    pub node_id: Option<String>,
    /// Timestamp.
    pub ts: DateTime<Utc>,
    /// Level.
    pub level: String,
    /// Message.
    pub message: String,
    /// Structured fields.
    pub fields: Option<Value>,
}

/// A context entry row.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ContextRecord {
    /// `flow` or `global`.
    pub scope: String,
    /// Scope id.
    pub scope_id: String,
    /// Key.
    pub key: String,
    /// Value.
    pub value: Value,
    /// Last update instant.
    pub updated_at: DateTime<Utc>,
    /// Last writer.
    pub updated_by_run_id: Option<Uuid>,
}

/// An audit event row.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AuditEventRecord {
    /// Event id.
    pub id: Uuid,
    /// Actor metadata.
    pub actor_id: Option<String>,
    /// Action.
    pub action: String,
    /// Target type.
    pub target_type: String,
    /// Target id.
    pub target_id: Uuid,
    /// Flow id.
    pub flow_id: Option<Uuid>,
    /// Run id.
    pub run_id: Option<Uuid>,
    /// Outcome.
    pub outcome: String,
    /// Metadata.
    pub metadata: Value,
    /// Creation instant.
    pub created_at: DateTime<Utc>,
}

/// A bounded run summary stored in `automation_runs.summary`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunSummary {
    /// Total nodes.
    pub nodes_total: i64,
    /// Succeeded nodes.
    pub nodes_succeeded: i64,
    /// Failed nodes.
    pub nodes_failed: i64,
    /// Skipped nodes.
    pub nodes_skipped: i64,
    /// Total messages.
    pub messages_total: i64,
    /// Duration in milliseconds.
    pub duration_ms: i64,
    /// Optional data-load summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_load: Option<DataLoadSummary>,
}

/// Data-load summary embedded in a run summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataLoadSummary {
    /// `single` or `range`.
    pub report_kind: String,
    /// Report status.
    pub status: String,
    /// Loaded count.
    pub loaded: i64,
    /// No-op count.
    pub no_op: i64,
    /// Not-resolved count.
    pub not_resolved: i64,
    /// Failed count.
    pub failed: i64,
    /// Node that produced the report.
    pub report_node_id: String,
}
