//! Retry and error-policy helpers.

use crate::automation::error::NodeExecutionError;
use crate::automation::flow::document::FlowNode;

/// Returns true when another attempt is allowed.
pub fn should_retry(node: &FlowNode, attempt: u32, error: &NodeExecutionError) -> bool {
    error.retryable && attempt < node.retry.max_attempts
}

/// Backoff duration before the next attempt.
pub fn backoff_seconds(node: &FlowNode, attempt: u32) -> u64 {
    node.retry
        .backoff_seconds
        .saturating_mul(1u32 << attempt.saturating_sub(1).min(8)) as u64
}
