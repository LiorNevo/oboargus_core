//! Per-node worker primitives.
//!
//! The executor in v1 processes deliveries from a queue; the worker type is the
//! seam for a future per-node channel implementation.

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::automation::runtime::bus::MessageEnvelope;

/// A handle used to deliver messages to a node worker.
pub struct WorkerHandle {
    /// Delivery channel.
    pub sender: mpsc::Sender<MessageEnvelope>,
    /// Cancellation token.
    pub cancel: CancellationToken,
}
