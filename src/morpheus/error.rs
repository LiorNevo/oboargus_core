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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_has_a_stable_wire_value_and_message() {
        let codes = [
            (ErrorCode::Validation, "validation_error"),
            (ErrorCode::NotFound, "not_found"),
            (ErrorCode::Conflict, "generation_in_progress"),
            (ErrorCode::Limit, "limit_exceeded"),
            (ErrorCode::ProviderAuth, "provider_auth"),
            (ErrorCode::ProviderRateLimited, "provider_rate_limited"),
            (ErrorCode::ProviderUnavailable, "provider_unavailable"),
            (ErrorCode::ProviderTimeout, "provider_timeout"),
            (ErrorCode::ProviderBadResponse, "provider_bad_response"),
            (ErrorCode::ProviderCapability, "provider_capability"),
            (ErrorCode::ToolInvalidArguments, "tool_invalid_arguments"),
            (ErrorCode::ToolFailed, "tool_failed"),
            (ErrorCode::ToolTimeout, "tool_timeout"),
            (ErrorCode::ActionRejected, "action_rejected"),
            (ErrorCode::ApprovalExpired, "approval_expired"),
            (ErrorCode::ApprovalDenied, "approval_denied"),
            (ErrorCode::CaptureDenied, "capture_denied"),
            (ErrorCode::CaptureWrongSurface, "capture_wrong_surface"),
            (ErrorCode::ObservationRejected, "observation_rejected"),
            (ErrorCode::Interrupted, "interrupted"),
            (ErrorCode::TurnLimit, "turn_limit"),
            (ErrorCode::AnswerTruncated, "answer_truncated"),
            (ErrorCode::QueueFull, "queue_full"),
            (ErrorCode::Infrastructure, "internal_error"),
        ];
        for (code, wire) in codes {
            assert_eq!(code.as_str(), wire);
            assert!(!code.message().is_empty(), "{wire} needs a message");
        }
    }

    #[test]
    fn validation_errors_keep_their_message() {
        let error = MorpheusError::validation("Name is required.");
        assert_eq!(error.code(), ErrorCode::Validation);
        assert_eq!(error.user_message(), "Name is required.");
        assert_eq!(error.to_string(), "Name is required.");
    }

    #[test]
    fn missing_rows_are_not_found_with_a_generic_user_message() {
        let error = MorpheusError::NotFound("conversation".to_string());
        assert_eq!(error.code(), ErrorCode::NotFound);
        assert_eq!(error.user_message(), ErrorCode::NotFound.message());
        assert_eq!(error.to_string(), "not found: conversation");
    }

    #[test]
    fn conflicts_and_forbidden_states_expose_their_sentence() {
        let conflict = MorpheusError::Conflict("Only a failed turn can be retried.".to_string());
        assert_eq!(conflict.code(), ErrorCode::Conflict);
        assert_eq!(
            conflict.user_message(),
            "Only a failed turn can be retried."
        );

        let forbidden = MorpheusError::Forbidden("This action is not allowed.".to_string());
        assert_eq!(forbidden.code(), ErrorCode::Validation);
        assert_eq!(forbidden.user_message(), "This action is not allowed.");
        assert_eq!(forbidden.to_string(), "This action is not allowed.");
    }

    #[test]
    fn provider_and_tool_errors_keep_their_classification() {
        let provider = MorpheusError::provider(ErrorCode::ProviderTimeout, "slow upstream");
        assert_eq!(provider.code(), ErrorCode::ProviderTimeout);
        assert_eq!(
            provider.user_message(),
            ErrorCode::ProviderTimeout.message()
        );
        assert!(provider.to_string().contains("slow upstream"));
        assert!(!provider.user_message().contains("slow upstream"));

        let tool = MorpheusError::tool(ErrorCode::ToolInvalidArguments, "bad json");
        assert_eq!(tool.code(), ErrorCode::ToolInvalidArguments);
        assert_eq!(
            tool.user_message(),
            ErrorCode::ToolInvalidArguments.message()
        );
        assert!(tool.to_string().contains("bad json"));
    }

    #[test]
    fn approval_and_infrastructure_errors_are_classified() {
        let approval = MorpheusError::Approval("expired".to_string());
        assert_eq!(approval.code(), ErrorCode::ApprovalExpired);
        assert_eq!(
            approval.user_message(),
            ErrorCode::ApprovalExpired.message()
        );
        assert_eq!(approval.to_string(), "approval error: expired");

        let infrastructure = MorpheusError::Infrastructure("disk".to_string());
        assert_eq!(infrastructure.code(), ErrorCode::Infrastructure);
        assert_eq!(
            infrastructure.user_message(),
            ErrorCode::Infrastructure.message()
        );
        assert_eq!(infrastructure.to_string(), "infrastructure error: disk");
    }

    #[test]
    fn approval_status_maps_denied_and_expired() {
        assert_eq!(
            MorpheusError::tool(ErrorCode::ApprovalDenied, "no").approval_status(),
            ApprovalStatus::Denied
        );
        assert_eq!(
            MorpheusError::Approval("gone".to_string()).approval_status(),
            ApprovalStatus::Expired
        );
        assert_eq!(
            MorpheusError::validation("bad").approval_status(),
            ApprovalStatus::Expired
        );
    }

    #[test]
    fn sql_and_json_errors_become_infrastructure_failures() {
        let from_sql = MorpheusError::db(sqlx::Error::RowNotFound);
        assert_eq!(from_sql.code(), ErrorCode::Infrastructure);
        assert!(from_sql.to_string().contains("infrastructure error"));

        let via_from: MorpheusError = sqlx::Error::PoolClosed.into();
        assert_eq!(via_from.code(), ErrorCode::Infrastructure);

        let json_error = serde_json::from_str::<serde_json::Value>("{").expect_err("invalid");
        let via_json: MorpheusError = json_error.into();
        assert_eq!(via_json.code(), ErrorCode::Infrastructure);
        assert!(
            via_json
                .to_string()
                .starts_with("infrastructure error: json:")
        );
    }
}
