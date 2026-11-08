//! Automation flow engine domain errors.
//!
//! Implements the error taxonomy in
//! `specs/030-automation-workflow-engine/contracts/node-type.md` and
//! `data-model.md`. Errors are explicit and carry stable codes so the HTTP
//! layer and the editor can render targeted messages.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Severity of a flow validation finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValidationSeverity {
    /// Blocks publish/run.
    Error,
    /// Does not block publish/run.
    Warning,
}

/// One flow validation finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowValidationFinding {
    /// Stable code, e.g. `flow.dangling_wire`.
    pub code: String,
    /// Human-readable message.
    pub message: String,
    /// Severity.
    pub severity: ValidationSeverity,
    /// Optional node id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    /// Optional wire id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wire_id: Option<String>,
    /// Optional JSON path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl FlowValidationFinding {
    /// Builds an error finding.
    pub fn error(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            severity: ValidationSeverity::Error,
            node_id: None,
            wire_id: None,
            path: None,
        }
    }

    /// Builds a warning finding.
    pub fn warning(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            severity: ValidationSeverity::Warning,
            node_id: None,
            wire_id: None,
            path: None,
        }
    }

    /// Attaches a node id.
    pub fn with_node(mut self, node_id: impl Into<String>) -> Self {
        self.node_id = Some(node_id.into());
        self
    }

    /// Attaches a wire id.
    pub fn with_wire(mut self, wire_id: impl Into<String>) -> Self {
        self.wire_id = Some(wire_id.into());
        self
    }
}

/// The result of validating a flow document.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationReport {
    /// Errors that block publish/run.
    pub errors: Vec<FlowValidationFinding>,
    /// Non-blocking warnings.
    pub warnings: Vec<FlowValidationFinding>,
}

impl ValidationReport {
    /// Returns true when there are no errors.
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// Adds an error.
    pub fn push_error(&mut self, finding: FlowValidationFinding) {
        self.errors.push(finding);
    }

    /// Adds a warning.
    pub fn push_warning(&mut self, finding: FlowValidationFinding) {
        self.warnings.push(finding);
    }
}

/// Top-level automation domain error.
#[derive(Debug, Error)]
pub enum AutomationError {
    /// Caller input failed validation.
    #[error("validation error: {0}")]
    Validation(String),
    /// A flow, version, trigger, run, or node was not found.
    #[error("not found: {0}")]
    NotFound(String),
    /// An optimistic-concurrency or idempotency conflict.
    #[error("conflict: {0}")]
    Conflict(String),
    /// The request or document is unsupported.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The request exceeded a configured limit.
    #[error("payload too large: {0}")]
    PayloadTooLarge(String),
    /// Infrastructure failure (database, executor).
    #[error("infrastructure error: {0}")]
    Infrastructure(String),
    /// Unexpected internal error.
    #[error("internal error: {0}")]
    Internal(String),
}

/// A node execution failure with a stable code.
#[derive(Debug, Clone, Error, PartialEq, Eq, Serialize, Deserialize)]
#[error("{code}: {message}")]
pub struct NodeExecutionError {
    /// Stable code, e.g. `node.timeout`.
    pub code: String,
    /// Human-readable message.
    pub message: String,
    /// Whether a retry may succeed.
    pub retryable: bool,
    /// Optional structured details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Box<serde_json::Value>>,
}

impl NodeExecutionError {
    /// Builds a non-retryable error.
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            retryable: false,
            details: None,
        }
    }

    /// Builds a retryable error.
    pub fn retryable(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            retryable: true,
            details: None,
        }
    }

    /// Attaches structured details.
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(Box::new(details));
        self
    }
}
