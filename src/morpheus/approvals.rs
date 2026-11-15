//! Approval policy and scope fingerprints.
//!
//! Consent is never inferred. A standing approval covers only the tool ids and
//! schema hashes present when the user granted it, plus the UI action kinds they
//! saw. Anything else prompts again.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::morpheus::model::StandingApprovalRecord;

/// Decision accepted by `POST /approvals/{id}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// Approve this action only.
    Approve,
    /// Deny this action.
    Deny,
    /// Approve the rest of this turn.
    ApproveAllTurn,
    /// Approve the current capabilities for future sessions.
    ApproveAllRemember,
}

impl ApprovalDecision {
    /// Parses a wire decision.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "approve" => Some(Self::Approve),
            "deny" => Some(Self::Deny),
            "approve_all_turn" => Some(Self::ApproveAllTurn),
            "approve_all_remember" => Some(Self::ApproveAllRemember),
            _ => None,
        }
    }

    /// True when the decision executes the action.
    pub fn approves(self) -> bool {
        !matches!(self, Self::Deny)
    }
}

/// Hashes a JSON value for approval-context binding.
pub fn hash_json(value: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.to_string().as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Hashes a text value.
pub fn hash_text(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Binds an approval to a tool id and its argument hash.
pub fn approval_context_hash(tool_id: &str, arguments_hash: &str) -> String {
    hash_text(&format!("{tool_id}:{arguments_hash}"))
}

/// True when the approval has expired.
pub fn is_expired(expires_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    expires_at <= now
}

/// True when a standing approval covers a tool at the given spec hash.
pub fn standing_covers_tool(
    approval: &StandingApprovalRecord,
    tool_id: &str,
    spec_hash: &str,
) -> bool {
    approval
        .tool_grants
        .get(tool_id)
        .and_then(Value::as_str)
        .is_some_and(|granted| granted == spec_hash)
}

/// True when a standing approval covers a UI action kind.
pub fn standing_covers_ui_action(approval: &StandingApprovalRecord, kind: &str) -> bool {
    approval.ui_action_kinds.iter().any(|item| item == kind)
}

/// True when the standing approval is active.
pub fn standing_is_active(approval: &StandingApprovalRecord) -> bool {
    approval.revoked_at.is_none()
}

/// Builds the tool-grant map for a grant from the active catalog.
pub fn tool_grants(specs: impl IntoIterator<Item = (String, String)>) -> Value {
    let mut map = serde_json::Map::new();
    for (id, hash) in specs {
        map.insert(id, Value::String(hash));
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standing(grants: Value, kinds: Vec<&str>) -> StandingApprovalRecord {
        StandingApprovalRecord {
            id: uuid::Uuid::new_v4(),
            actor_id: "dev-user".to_string(),
            tool_grants: grants,
            ui_action_kinds: kinds.into_iter().map(str::to_string).collect(),
            granted_at: Utc::now(),
            revoked_at: None,
        }
    }

    #[test]
    fn a_standing_grant_matches_only_its_hashes() {
        let approval = standing(serde_json::json!({ "assets.search": "abc" }), vec!["click"]);
        assert!(standing_covers_tool(&approval, "assets.search", "abc"));
        assert!(!standing_covers_tool(&approval, "assets.search", "def"));
        assert!(!standing_covers_tool(&approval, "new.tool", "abc"));
        assert!(standing_covers_ui_action(&approval, "click"));
        assert!(!standing_covers_ui_action(&approval, "submit"));
    }

    #[test]
    fn decisions_parse_and_approve() {
        assert!(ApprovalDecision::parse("approve").is_some_and(ApprovalDecision::approves));
        assert!(!ApprovalDecision::parse("deny").is_some_and(ApprovalDecision::approves));
        assert_eq!(ApprovalDecision::parse("nope"), None);
    }
}
