//! Typed errors for [`crate::temporal`].
//!
//! `NoValue` and `OutsideRetention` are deliberately distinct: "we never
//! recorded it" and "we recorded it but retired it" are different answers to
//! a historical question.

use std::fmt;

/// The error type returned by every public function in `obolargus_core::temporal`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TemporalError {
    /// A window is empty, reversed, or a precision guard was violated.
    #[error("invalid time: {detail}")]
    InvalidTime {
        /// Which coordinate failed and why.
        detail: String,
    },

    /// The subject does not exist.
    #[error("unknown subject: {subject} {id}")]
    UnknownSubject {
        /// The subject kind.
        subject: String,
        /// The subject id.
        id: String,
    },

    /// No record exists for the requested identity and instant.
    #[error("no value for {subject} at {at}")]
    NoValue {
        /// The subject kind.
        subject: String,
        /// The requested instant.
        at: String,
    },

    /// Data exists but is outside the retained window.
    #[error("outside retention: {window}")]
    OutsideRetention {
        /// The requested window.
        window: String,
    },

    /// An append conflicts with an existing version under the identity key.
    #[error("conflicting append for {identity}")]
    ConflictingAppend {
        /// The identity that conflicted.
        identity: String,
    },

    /// The requested interval is not supported or does not divide the stored one.
    #[error("unsupported interval {requested}s (stored: {stored:?})")]
    UnsupportedInterval {
        /// The requested interval in seconds.
        requested: u32,
        /// The intervals that are actually stored.
        stored: Vec<u32>,
    },

    /// The feed is disconnected or the sink is under backpressure.
    #[error("ingest unavailable for {source_id}: {detail}")]
    IngestUnavailable {
        /// The source id. Named `source_id` because `thiserror` reserves
        /// `source` for the error-chain field.
        source_id: String,
        /// What went wrong.
        detail: String,
    },

    /// A retention or archive operation failed.
    #[error("retention: {detail}")]
    Retention {
        /// What went wrong.
        detail: String,
    },

    /// Infrastructure failure; never swallowed.
    #[error("infrastructure: {0}")]
    Infrastructure(#[from] sqlx::Error),
}

impl TemporalError {
    /// Constructs an [`TemporalError::InvalidTime`].
    pub fn invalid_time(detail: impl fmt::Display) -> Self {
        Self::InvalidTime {
            detail: detail.to_string(),
        }
    }

    /// Constructs a [`TemporalError::ConflictingAppend`].
    pub fn conflict(identity: impl fmt::Display) -> Self {
        Self::ConflictingAppend {
            identity: identity.to_string(),
        }
    }
}

impl From<crate::domain::DomainError> for TemporalError {
    fn from(error: crate::domain::DomainError) -> Self {
        match error {
            crate::domain::DomainError::Infrastructure(inner) => Self::Infrastructure(inner),
            other => Self::Retention {
                detail: other.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_value_and_outside_retention_are_distinct_messages() {
        let missing = TemporalError::NoValue {
            subject: "asset".to_string(),
            at: "2026-01-02T00:00:00Z".to_string(),
        };
        let retired = TemporalError::OutsideRetention {
            window: "[2020-01-01, 2020-02-01)".to_string(),
        };
        assert!(missing.to_string().contains("no value"));
        assert!(retired.to_string().contains("outside retention"));
        assert_ne!(missing.to_string(), retired.to_string());
    }

    #[test]
    fn every_variant_has_a_display_message() {
        let errors = [
            TemporalError::invalid_time("reversed"),
            TemporalError::UnknownSubject {
                subject: "asset".to_string(),
                id: "id".to_string(),
            },
            TemporalError::NoValue {
                subject: "asset".to_string(),
                at: "now".to_string(),
            },
            TemporalError::OutsideRetention {
                window: "w".to_string(),
            },
            TemporalError::conflict("identity"),
            TemporalError::UnsupportedInterval {
                requested: 1,
                stored: vec![60],
            },
            TemporalError::IngestUnavailable {
                source_id: "yahoo".to_string(),
                detail: "disconnected".to_string(),
            },
            TemporalError::Retention {
                detail: "detach failed".to_string(),
            },
        ];
        for error in errors {
            assert!(!error.to_string().is_empty());
        }
    }

    #[test]
    fn sqlx_errors_are_converted_not_swallowed() {
        let inner = sqlx::Error::RowNotFound;
        let converted = TemporalError::from(inner);
        assert!(matches!(converted, TemporalError::Infrastructure(_)));
    }
}
