//! Typed Morpheus failures.
//!
//! Every error carries a stable code, an HTTP-oriented category, and a
//! user-safe sentence. Provider bodies, credentials, and raw SQL never cross
//! this boundary.

use thiserror::Error;

use crate::morpheus::model::ApprovalStatus;

/// Stable failure classes used by the API and the conversation transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// Input failed validation.
    Validation,
    /// The requested row does not exist for this actor.
    NotFound,
    /// The request conflicts with current state.
    Conflict,
    /// A configured limit was exceeded.
    Limit,
    /// The provider rejected the credentials.
    ProviderAuth,
    /// The provider rate-limited the request.
    ProviderRateLimited,
    /// The provider is unreachable.
    ProviderUnavailable,
    /// The provider did not answer in time.
    ProviderTimeout,
    /// The provider returned malformed data.
    ProviderBadResponse,
    /// The configured model lacks a required capability.
    ProviderCapability,
    /// Tool arguments failed schema validation.
    ToolInvalidArguments,
    /// A tool failed.
    ToolFailed,
    /// A tool timed out.
    ToolTimeout,
    /// A UI action target was stale.
    ActionRejected,
    /// The approval expired.
    ApprovalExpired,
    /// The user denied the action.
    ApprovalDenied,
    /// Screen capture was not granted.
    CaptureDenied,
    /// The shared surface was not the current tab.
    CaptureWrongSurface,
    /// The capture was malformed or too large.
    ObservationRejected,
    /// The turn was interrupted by a restart.
    Interrupted,
    /// A turn limit was reached.
    TurnLimit,
    /// The answer hit its stored limit.
    AnswerTruncated,
    /// The queue is full.
    QueueFull,
    /// Database or unexpected infrastructure failure.
    Infrastructure,
}

impl ErrorCode {
    /// Stable wire code.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "validation_error",
            Self::NotFound => "not_found",
            Self::Conflict => "generation_in_progress",
            Self::Limit => "limit_exceeded",
            Self::ProviderAuth => "provider_auth",
            Self::ProviderRateLimited => "provider_rate_limited",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::ProviderTimeout => "provider_timeout",
            Self::ProviderBadResponse => "provider_bad_response",
            Self::ProviderCapability => "provider_capability",
            Self::ToolInvalidArguments => "tool_invalid_arguments",
            Self::ToolFailed => "tool_failed",
            Self::ToolTimeout => "tool_timeout",
            Self::ActionRejected => "action_rejected",
            Self::ApprovalExpired => "approval_expired",
            Self::ApprovalDenied => "approval_denied",
            Self::CaptureDenied => "capture_denied",
            Self::CaptureWrongSurface => "capture_wrong_surface",
            Self::ObservationRejected => "observation_rejected",
            Self::Interrupted => "interrupted",
            Self::TurnLimit => "turn_limit",
            Self::AnswerTruncated => "answer_truncated",
            Self::QueueFull => "queue_full",
            Self::Infrastructure => "internal_error",
        }
    }

    /// User-facing sentence. Never includes provider internals.
    pub fn message(self) -> &'static str {
        match self {
            Self::Validation => "The request was not valid.",
            Self::NotFound => "That item could not be found.",
            Self::Conflict => "Morpheus is still working on the previous turn.",
            Self::Limit => "That request is too large.",
            Self::ProviderAuth => {
                "Morpheus is not configured correctly. Contact your administrator."
            }
            Self::ProviderRateLimited => "Morpheus is busy. Try again in a moment.",
            Self::ProviderUnavailable => "Morpheus is unavailable right now.",
            Self::ProviderTimeout => "Morpheus took too long to respond.",
            Self::ProviderBadResponse => "Morpheus returned an unreadable response.",
            Self::ProviderCapability => {
                "Morpheus cannot use this capability with the configured model."
            }
            Self::ToolInvalidArguments => "The action was not valid and was not run.",
            Self::ToolFailed => "The action failed.",
            Self::ToolTimeout => "The action took too long.",
            Self::ActionRejected => "The screen changed; Morpheus needs to look again.",
            Self::ApprovalExpired => "The action was not approved in time.",
            Self::ApprovalDenied => "You denied the action.",
            Self::CaptureDenied => "Screen access was not granted.",
            Self::CaptureWrongSurface => "Morpheus can only work in this tab.",
            Self::ObservationRejected => "The screen capture could not be accepted.",
            Self::Interrupted => "The turn was interrupted. You can retry.",
            Self::TurnLimit => "Morpheus stopped at the execution limit.",
            Self::AnswerTruncated => "The answer was cut off because it reached the length limit.",
            Self::QueueFull => "There are too many queued messages.",
            Self::Infrastructure => "Something went wrong on the server.",
        }
    }
}

/// A Morpheus failure.
#[derive(Debug, Error)]
pub enum MorpheusError {
    /// Validation failure with optional detail.
    #[error("{message}")]
    Validation {
        /// User-safe message.
        message: String,
    },
    /// Missing or non-owned row.
    #[error("not found: {0}")]
    NotFound(String),
    /// State conflict.
    #[error("{0}")]
    Conflict(String),
    /// Operation is not allowed in the current state.
    #[error("{0}")]
    Forbidden(String),
    /// Provider failure.
    #[error("provider error ({code:?}): {detail}")]
    Provider {
        /// Classified provider failure.
        code: ErrorCode,
        /// Server-side detail, never returned to the browser.
        detail: String,
    },
    /// Tool failure.
    #[error("tool error ({code:?}): {detail}")]
    Tool {
        /// Classified tool failure.
        code: ErrorCode,
        /// Server-side detail.
        detail: String,
    },
    /// Approval failure.
    #[error("approval error: {0}")]
    Approval(String),
    /// Infrastructure failure.
    #[error("infrastructure error: {0}")]
    Infrastructure(String),
}

impl MorpheusError {
    /// Stable wire code.
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Validation { .. } => ErrorCode::Validation,
            Self::NotFound(_) => ErrorCode::NotFound,
            Self::Conflict(_) => ErrorCode::Conflict,
            Self::Forbidden(_) => ErrorCode::Validation,
            Self::Provider { code, .. } | Self::Tool { code, .. } => *code,
            Self::Approval(_) => ErrorCode::ApprovalExpired,
            Self::Infrastructure(_) => ErrorCode::Infrastructure,
        }
    }

    /// User-safe message.
    pub fn user_message(&self) -> String {
        match self {
            Self::Validation { message } => message.clone(),
            Self::Conflict(message) | Self::Forbidden(message) => message.clone(),
            other => other.code().message().to_string(),
        }
    }

    /// Builds a validation failure.
    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation {
            message: message.into(),
        }
    }

    /// Builds a provider failure.
    pub fn provider(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self::Provider {
            code,
            detail: detail.into(),
        }
    }

    /// Builds a tool failure.
    pub fn tool(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self::Tool {
            code,
            detail: detail.into(),
        }
    }

    /// Maps a SQLx failure without leaking SQL to the user.
    pub fn db(error: sqlx::Error) -> Self {
        Self::Infrastructure(error.to_string())
    }

    /// Terminal message status for a failure.
    pub fn approval_status(&self) -> ApprovalStatus {
        match self.code() {
            ErrorCode::ApprovalDenied => ApprovalStatus::Denied,
            ErrorCode::ApprovalExpired => ApprovalStatus::Expired,
            _ => ApprovalStatus::Expired,
        }
    }
}

impl From<sqlx::Error> for MorpheusError {
    fn from(error: sqlx::Error) -> Self {
        Self::db(error)
    }
}

impl From<serde_json::Error> for MorpheusError {
    fn from(error: serde_json::Error) -> Self {
        Self::Infrastructure(format!("json: {error}"))
    }
}
