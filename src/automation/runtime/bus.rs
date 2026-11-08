//! Runtime message bus and run metadata.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Metadata available to every node in a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunMetadata {
    /// Run id.
    pub id: Uuid,
    /// Flow id.
    pub flow_id: Uuid,
    /// Executed version id.
    pub flow_version_id: Uuid,
    /// Trigger kind (`manual`, `schedule`, `api`).
    pub trigger_kind: String,
    /// Inject node that started the run.
    pub trigger_node_id: Option<String>,
    /// Run start instant.
    pub started_at: DateTime<Utc>,
    /// Actor metadata.
    pub actor_id: Option<String>,
    /// Runtime inputs.
    pub inputs: Value,
}

/// A message travelling on a wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageEnvelope {
    /// Runtime message id.
    pub msg_id: String,
    /// JSON message.
    pub msg: Value,
}

impl MessageEnvelope {
    /// Wraps a message with a fresh `_msgid` if absent.
    pub fn new(mut msg: Value) -> Self {
        let msg_id = msg
            .get("_msgid")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        if let Value::Object(map) = &mut msg {
            map.insert("_msgid".to_string(), Value::String(msg_id.clone()));
        }
        Self { msg_id, msg }
    }

    /// Returns a deep clone with a new message id.
    pub fn clone_for_wire(&self) -> Self {
        let mut msg = self.msg.clone();
        let msg_id = Uuid::new_v4().to_string();
        if let Value::Object(map) = &mut msg {
            map.insert("_msgid".to_string(), Value::String(msg_id.clone()));
        }
        Self { msg_id, msg }
    }
}
