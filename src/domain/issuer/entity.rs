//! `Issuer` struct and input/patch types.

use crate::domain::id::DomainId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::domain::id::IssuerId;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct Issuer {
    pub id: uuid::Uuid,
    pub name: String,
    pub issuer_type: String,
    pub legal_jurisdiction: String,
    pub created_at: DateTime<Utc>,
    /// `None` for root issuers; `Some` for child issuers.
    /// The issuer graph must be a DAG — cycles are prevented at write time.
    pub parent_id: Option<uuid::Uuid>,
}

impl Issuer {
    /// Implements FR-008 of spec 016-implement-domain-model.
    pub fn id(&self) -> IssuerId {
        IssuerId::from_uuid(self.id)
    }

    /// Returns `true` if this issuer is a root (has no parent).
    pub fn is_root(&self) -> bool {
        self.parent_id.is_none()
    }
}

/// `sqlx::FromRow` implementation that explicitly selects all columns
/// including the optional `parent_id`.
impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for Issuer {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.get("id"),
            name: row.get("name"),
            issuer_type: row.get("issuer_type"),
            legal_jurisdiction: row.get("legal_jurisdiction"),
            created_at: row.get("created_at"),
            parent_id: row.get("parent_id"),
        })
    }
}

#[derive(Debug, Clone)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct NewIssuer {
    pub name: String,
    pub issuer_type: String,
    pub legal_jurisdiction: String,
    /// `None` for a root issuer; `Some(parent_id)` to link as a child.
    /// Creating a cycle or self-loop is rejected at the application layer
    /// (`issuer::dag`).
    pub parent_id: Option<IssuerId>,
}

/// `IssuerPatch` does NOT include `parent_id` — reparenting is exclusively
/// handled by `reparent_issuer` (per data-model.md: "reparenting is a separate
/// `reparent_issuer` operation").
#[derive(Debug, Clone, Default)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct IssuerPatch {
    pub name: Option<String>,
    pub issuer_type: Option<String>,
    pub legal_jurisdiction: Option<String>,
}
