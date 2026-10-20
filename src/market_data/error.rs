//! Typed error for a single provider call.
//!
//! Implements FR-005 and FR-041 of `specs/017-yahoo-data-source/spec.md`.
//! Six variants fold the upstream `YahooError` taxonomy (22 variants) into a
//! stable consumer-facing taxonomy. The mapping is implemented in
//! `obolargus_server::market_data_provider::yahoo::From<yahoo_finance_api::YahooError>`
//! (kept there because `yahoo_finance_api` is not a dependency of
//! `obolargus_core`).

use std::time::Duration;

/// Typed error for a single provider call. `#[non_exhaustive]` so future
/// variants are additive.
///
/// Implements FR-005 of spec 017.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MarketDataProviderError {
    /// The provider has no answer for this query. Registry MUST treat
    /// this as a final answer (do not fall over).
    #[error("not found: {0}")]
    NotFound(String),

    /// The request was malformed. Registry MUST treat this as a final answer.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// The provider does not support this operation. Registry MUST treat
    /// this as a final answer.
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// A retryable transport / decode / server error. Registry MUST fall
    /// over to the next enabled source. A single provider call MUST retry
    /// with exponential backoff (FR-041) before returning this.
    #[error("transient error: {0}")]
    Transient(String),

    /// The provider's rate budget is exhausted. Registry MUST fall over
    /// and MUST NOT retry until `retry_after` has elapsed.
    #[error("rate limited: {msg}")]
    RateLimited {
        /// Human-readable reason (typically the upstream message).
        msg: String,
        /// Cool-down window the consumer should honor before retrying the
        /// same source. `None` when the upstream did not communicate a value.
        retry_after: Option<Duration>,
    },

    /// The provider rejected credentials. Registry MUST treat this as a
    /// final answer (retrying with the same credentials is pointless).
    #[error("unauthorized: {0}")]
    Unauthorized(String),

    /// A non-retryable infrastructure error. Registry MUST treat this
    /// as a final answer.
    #[error("infrastructure error: {0}")]
    Infrastructure(String),

    /// Configuration validation failed at startup. Registry MUST treat
    /// this as a final answer; the operator MUST fix the config and
    /// restart.
    #[error("validation error: {0}")]
    Validation(String),
}

impl MarketDataProviderError {
    /// Whether the registry should fall over to the next enabled source
    /// after seeing this error. `Transient` and `RateLimited` fall over;
    /// the others are final.
    ///
    /// Implements FR-005 of spec 017.
    pub fn should_fallover(&self) -> bool {
        matches!(self, Self::Transient(_) | Self::RateLimited { .. })
    }
}
