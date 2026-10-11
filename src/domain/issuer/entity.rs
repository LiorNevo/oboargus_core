//! `Issuer` struct and input/patch types.

use crate::domain::id::DomainId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::domain::id::IssuerId;

#[derive(Debug, PartialEq, Eq, FromRow, Serialize, Deserialize)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct Issuer {
    pub id: uuid::Uuid,
    pub name: String,
    pub issuer_type: String,
    pub legal_jurisdiction: String,
    pub created_at: DateTime<Utc>,
}

impl Issuer {
    /// Implements FR-008 of spec 016-implement-domain-model.
    pub fn id(&self) -> IssuerId {
        IssuerId::from_uuid(self.id)
    }
}

#[derive(Debug, Clone)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct NewIssuer {
    pub name: String,
    pub issuer_type: String,
    pub legal_jurisdiction: String,
}

#[derive(Debug, Clone, Default)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct IssuerPatch {
    pub name: Option<String>,
    pub issuer_type: Option<String>,
    pub legal_jurisdiction: Option<String>,
}
