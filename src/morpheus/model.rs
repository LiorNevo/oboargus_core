//! Morpheus conversation, turn, action, and approval records.
//!
//! Mirrors `specs/032-morpheus-agent-chat/data-model.md`. Statuses are stored as
//! text so the projection can be queried without custom SQL types; the enums
//! provide the only accepted values.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

/// Author of a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    /// The user's turn.
    User,
    /// Morpheus's answer.
    Agent,
}

impl MessageRole {
    /// Database text value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
        }
    }

    /// Parses a stored role.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "agent" => Some(Self::Agent),
            _ => None,
        }
    }
}

/// Delivery/production state of a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    /// Waiting for its turn to start.
    Queued,
    /// Content is being produced.
    Streaming,
    /// Finished successfully.
    Complete,
    /// Stopped by the user or a limit.
    Stopped,
    /// Failed.
    Failed,
}

impl MessageStatus {
    /// Database text value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Streaming => "streaming",
            Self::Complete => "complete",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }
}

/// Lifecycle of a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    /// Enqueued behind another turn.
    Queued,
    /// Actively calling the provider or a tool.
    Running,
    /// Waiting for an approval decision.
    AwaitingApproval,
    /// Waiting for a screen observation.
    AwaitingObservation,
    /// Waiting for a UI action result.
    AwaitingActionResult,
    /// Finished successfully.
    Completed,
    /// Stopped by the user or a limit.
    Stopped,
    /// Failed.
    Failed,
}

impl TurnStatus {
    /// Database text value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::AwaitingApproval => "awaiting_approval",
            Self::AwaitingObservation => "awaiting_observation",
            Self::AwaitingActionResult => "awaiting_action_result",
            Self::Completed => "completed",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }

    /// True when no further work will occur.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Stopped | Self::Failed)
    }
}

/// Kind of activity step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    /// Reasoning summary (never hidden chain-of-thought).
    Reasoning,
    /// Tool invocation.
    Tool,
    /// Interface action.
    UiAction,
    /// Screen observation.
    Observation,
    /// Final answer.
    Answer,
    /// User-facing notice.
    Notice,
}

impl StepKind {
    /// Database text value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reasoning => "reasoning",
            Self::Tool => "tool",
            Self::UiAction => "ui_action",
            Self::Observation => "observation",
            Self::Answer => "answer",
            Self::Notice => "notice",
        }
    }
}

/// Status of a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    /// In progress.
    Started,
    /// Succeeded.
    Completed,
    /// Failed.
    Failed,
    /// Not executed.
    Skipped,
}

impl StepStatus {
    /// Database text value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

/// Risk classification of a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRisk {
    /// Reads only.
    Read,
    /// Changes workspace state.
    Write,
    /// Removes data.
    Destructive,
}

impl ToolRisk {
    /// Database text value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Destructive => "destructive",
        }
    }
}

/// Status of a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    /// Proposed by the model.
    Proposed,
    /// Waiting for the user's decision.
    AwaitingApproval,
    /// Approved but not started.
    Approved,
    /// Denied by the user.
    Denied,
    /// Executing.
    Running,
    /// Succeeded.
    Succeeded,
    /// Failed.
    Failed,
    /// Cancelled with the turn.
    Cancelled,
    /// Approval expired.
    Expired,
}

impl ToolCallStatus {
    /// Database text value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        }
    }
}

/// Kind of interface action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiActionKind {
    /// Same-origin route change.
    Navigate,
    /// Click an element.
    Click,
    /// Type into an input.
    Type,
    /// Select an option.
    Select,
    /// Press a key.
    Press,
    /// Scroll.
    Scroll,
    /// Submit a form.
    Submit,
}

impl UiActionKind {
    /// Database text value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Navigate => "navigate",
            Self::Click => "click",
            Self::Type => "type",
            Self::Select => "select",
            Self::Press => "press",
            Self::Scroll => "scroll",
            Self::Submit => "submit",
        }
    }
}

/// Status of an interface action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiActionStatus {
    /// Proposed by the model.
    Proposed,
    /// Waiting for the user's decision.
    AwaitingApproval,
    /// Approved but not executed.
    Approved,
    /// Denied by the user.
    Denied,
    /// Executing in the browser.
    Running,
    /// Succeeded.
    Succeeded,
    /// Failed.
    Failed,
    /// Rejected because the screen changed.
    Rejected,
    /// Cancelled.
    Cancelled,
    /// Approval expired.
    Expired,
}

impl UiActionStatus {
    /// Database text value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        }
    }
}

/// The kind of action an approval covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalActionKind {
    /// A tool call.
    Tool,
    /// An interface action.
    UiAction,
}

impl ApprovalActionKind {
    /// Database text value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::UiAction => "ui_action",
        }
    }
}

/// Decision state of an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    /// Waiting for the user.
    Pending,
    /// Approved.
    Approved,
    /// Denied.
    Denied,
    /// Expired without a decision.
    Expired,
}

impl ApprovalStatus {
    /// Database text value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Expired => "expired",
        }
    }
}

/// How an action was approved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionMode {
    /// Approved for this action only.
    Individual,
    /// Approved for the rest of the turn.
    Turn,
    /// Covered by a standing approval.
    Standing,
}

impl DecisionMode {
    /// Database text value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Individual => "individual",
            Self::Turn => "turn",
            Self::Standing => "standing",
        }
    }
}

/// Status of a screen observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationStatus {
    /// Capture requested.
    Requested,
    /// Image uploaded.
    Received,
    /// Sent to the model.
    Consumed,
    /// Request timed out.
    Expired,
    /// Turn stopped or deleted.
    Cancelled,
}

impl ObservationStatus {
    /// Database text value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Received => "received",
            Self::Consumed => "consumed",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
        }
    }
}

/// A conversation row.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct ConversationRecord {
    /// Conversation id.
    pub id: Uuid,
    /// Acting user identifier.
    pub owner_id: String,
    /// Client-generated id used for idempotent creation.
    pub client_conversation_id: Uuid,
    /// Derived or user-set title.
    pub title: String,
    /// Next transcript sequence.
    pub next_message_seq: i64,
    /// Creation instant.
    pub created_at: DateTime<Utc>,
    /// Last message activity.
    pub updated_at: DateTime<Utc>,
}

/// A conversation list projection.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct ConversationSummaryRecord {
    /// Conversation id.
    pub id: Uuid,
    /// Title.
    pub title: String,
    /// Last-message preview.
    pub preview: String,
    /// Number of messages.
    pub message_count: i64,
    /// Creation instant.
    pub created_at: DateTime<Utc>,
    /// Last activity.
    pub updated_at: DateTime<Utc>,
}

/// A message row.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct MessageRecord {
    /// Message id.
    pub id: Uuid,
    /// Conversation id.
    pub conversation_id: Uuid,
    /// Transcript sequence.
    pub seq: i64,
    /// `user` or `agent`.
    pub role: String,
    /// Delivery/production status.
    pub status: String,
    /// Displayed content.
    pub content: String,
    /// User-message idempotency key.
    pub client_message_id: Option<String>,
    /// Stable error code.
    pub error_code: Option<String>,
    /// User-safe error sentence.
    pub error_message: Option<String>,
    /// Whether earlier context was omitted.
    pub context_trimmed: bool,
    /// Creation instant.
    pub created_at: DateTime<Utc>,
    /// Last update.
    pub updated_at: DateTime<Utc>,
    /// Terminal instant.
    pub completed_at: Option<DateTime<Utc>>,
}

/// A turn row.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct TurnRecord {
    /// Turn id.
    pub id: Uuid,
    /// Conversation id.
    pub conversation_id: Uuid,
    /// The question.
    pub user_message_id: Uuid,
    /// This attempt's answer.
    pub agent_message_id: Uuid,
    /// Attempt number.
    pub attempt: i32,
    /// Previous attempt.
    pub retry_of: Option<Uuid>,
    /// Turn status.
    pub status: String,
    /// Tab bound to screen control.
    pub surface_session_id: Option<String>,
    /// ReACT steps used.
    pub step_count: i32,
    /// Provider calls used.
    pub provider_calls: i32,
    /// Observations used.
    pub observation_count: i32,
    /// Whether the rest of this turn is approved.
    pub approve_all_turn: bool,
    /// Limit that ended the turn early.
    pub limit_reached: Option<String>,
    /// Terminal error code.
    pub error_code: Option<String>,
    /// User-safe terminal error.
    pub error_message: Option<String>,
    /// Enqueue instant.
    pub created_at: DateTime<Utc>,
    /// Worker claim instant.
    pub started_at: Option<DateTime<Utc>>,
    /// Terminal instant.
    pub ended_at: Option<DateTime<Utc>>,
    /// Last update.
    pub updated_at: DateTime<Utc>,
}

/// A step row.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct StepRecord {
    /// Step id.
    pub id: Uuid,
    /// Turn id.
    pub turn_id: Uuid,
    /// Step sequence.
    pub seq: i32,
    /// Step kind.
    pub kind: String,
    /// Step status.
    pub status: String,
    /// User-facing summary.
    pub summary: String,
    /// Related tool/action/observation id.
    pub ref_id: Option<Uuid>,
    /// Redacted metadata.
    pub payload: Value,
    /// Creation instant.
    pub created_at: DateTime<Utc>,
    /// Completion instant.
    pub completed_at: Option<DateTime<Utc>>,
}

/// A tool call row.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct ToolCallRecord {
    /// Tool call id.
    pub id: Uuid,
    /// Turn id.
    pub turn_id: Uuid,
    /// Provider tool-call id.
    pub provider_call_id: String,
    /// Catalog id.
    pub tool_id: String,
    /// Catalog version.
    pub tool_version: i32,
    /// Full validated arguments.
    pub arguments: Value,
    /// Redacted summary.
    pub arguments_summary: String,
    /// Risk class.
    pub risk: String,
    /// Status.
    pub status: String,
    /// Bounded result summary.
    pub result_summary: Option<String>,
    /// Whether the result was truncated.
    pub result_truncated: bool,
    /// Failure code.
    pub error_code: Option<String>,
    /// User-safe failure.
    pub error_message: Option<String>,
    /// Approval id.
    pub approval_id: Option<Uuid>,
    /// Start instant.
    pub started_at: Option<DateTime<Utc>>,
    /// Completion instant.
    pub completed_at: Option<DateTime<Utc>>,
    /// Creation instant.
    pub created_at: DateTime<Utc>,
}

/// A UI action row.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct UiActionRecord {
    /// Action id.
    pub id: Uuid,
    /// Turn id.
    pub turn_id: Uuid,
    /// Observation the plan used.
    pub observation_id: Option<Uuid>,
    /// Action kind.
    pub kind: String,
    /// Element reference.
    pub target_ref: Option<String>,
    /// Accessible target label.
    pub target_label: String,
    /// Redacted value summary.
    pub value_summary: Option<String>,
    /// DOM state hash.
    pub state_hash: Option<String>,
    /// Status.
    pub status: String,
    /// Outcome summary.
    pub result_summary: Option<String>,
    /// Failure code.
    pub error_code: Option<String>,
    /// Approval id.
    pub approval_id: Option<Uuid>,
    /// Start instant.
    pub started_at: Option<DateTime<Utc>>,
    /// Completion instant.
    pub completed_at: Option<DateTime<Utc>>,
    /// Creation instant.
    pub created_at: DateTime<Utc>,
}

/// A screen observation row.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct ObservationRecord {
    /// Observation id.
    pub id: Uuid,
    /// Turn id.
    pub turn_id: Uuid,
    /// Lifecycle status.
    pub status: String,
    /// Capturing tab.
    pub surface_session_id: Option<String>,
    /// Image width.
    pub width: Option<i32>,
    /// Image height.
    pub height: Option<i32>,
    /// Image bytes (not serialized by API DTOs).
    pub image: Option<Vec<u8>>,
    /// Image MIME type.
    pub image_mime: Option<String>,
    /// Image size.
    pub image_bytes: i32,
    /// DOM state hash.
    pub dom_hash: Option<String>,
    /// Mask rectangles.
    pub mask_manifest: Option<Value>,
    /// Element map.
    pub elements: Option<Value>,
    /// Element count.
    pub element_count: i32,
    /// Mask count.
    pub mask_count: i32,
    /// Request instant.
    pub requested_at: DateTime<Utc>,
    /// Capture instant.
    pub captured_at: Option<DateTime<Utc>>,
    /// Expiry instant.
    pub expires_at: DateTime<Utc>,
    /// Image deletion instant.
    pub deleted_at: Option<DateTime<Utc>>,
}

/// An approval request row.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct ApprovalRecord {
    /// Approval id.
    pub id: Uuid,
    /// Turn id.
    pub turn_id: Uuid,
    /// Action kind.
    pub action_kind: String,
    /// Tool-call or UI-action id.
    pub action_id: Uuid,
    /// Decision status.
    pub status: String,
    /// Approval mode.
    pub decision_mode: Option<String>,
    /// Context hash.
    pub context_hash: String,
    /// Expiry instant.
    pub expires_at: DateTime<Utc>,
    /// Creation instant.
    pub created_at: DateTime<Utc>,
    /// Decision instant.
    pub decided_at: Option<DateTime<Utc>>,
}

/// A standing approval row.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct StandingApprovalRecord {
    /// Approval id.
    pub id: Uuid,
    /// Owner.
    pub actor_id: String,
    /// Tool id → spec hash map.
    pub tool_grants: Value,
    /// UI action kinds.
    pub ui_action_kinds: Vec<String>,
    /// Grant instant.
    pub granted_at: DateTime<Utc>,
    /// Revocation instant.
    pub revoked_at: Option<DateTime<Utc>>,
}

/// A control lease row.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct LeaseRecord {
    /// Lease id.
    pub id: Uuid,
    /// Owner.
    pub actor_id: String,
    /// Controlling tab.
    pub surface_session_id: String,
    /// Conversation id.
    pub conversation_id: Uuid,
    /// Turn id.
    pub turn_id: Option<Uuid>,
    /// Last heartbeat.
    pub heartbeat_at: DateTime<Utc>,
    /// Expiry instant.
    pub expires_at: DateTime<Utc>,
}
