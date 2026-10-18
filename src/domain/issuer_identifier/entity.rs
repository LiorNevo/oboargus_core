//! `IssuerIdentifier` struct and input types.
//!
//! An external identifier (LEI, CIK, ...) by which an issuer is known
//! outside Obolargus. Identifiers are current-state keys, not dual-temporal
//! facts (predecessor FR-033): the record carries a `scheme` and a `value`,
//! no `as_of_date` / `knowledge_date`.
//!
//! Implements FR-023 of `specs/016-implement-domain-model/spec.md`
//! (predecessor FR-029 of `specs/015-database-uml-domains/spec.md`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::domain::id::{DomainId, IdentifierId, IssuerId};

/// A persisted external identifier naming an issuer.
#[derive(Debug, PartialEq, Eq, FromRow, Serialize, Deserialize)]
/// Implements FR-023 of spec 016-implement-domain-model.
pub struct IssuerIdentifier {
    /// The identifier record id.
    pub id: uuid::Uuid,
    /// The issuer this identifier names.
    pub issuer_id: uuid::Uuid,
    /// The identifier scheme (open vocabulary, e.g. `"LEI"`).
    /// Stored trimmed and uppercased (FR-025).
    pub scheme: String,
    /// The identifier value. Stored trimmed; matched exactly (FR-025).
    pub value: String,
    /// When the identifier record was created.
    pub created_at: DateTime<Utc>,
}

impl IssuerIdentifier {
    /// Borrow the record id as an `IdentifierId`.
    /// Implements FR-023 of spec 016-implement-domain-model.
    pub fn id(&self) -> IdentifierId {
        IdentifierId::from_uuid(self.id)
    }
    /// Borrow the named issuer's id as an `IssuerId`.
    /// Implements FR-023 of spec 016-implement-domain-model.
    pub fn issuer_id(&self) -> IssuerId {
        IssuerId::from_uuid(self.issuer_id)
    }
}

/// Input for `create_issuer_identifier`.
#[derive(Debug, Clone)]
/// Implements FR-023 of spec 016-implement-domain-model.
pub struct NewIssuerIdentifier {
    /// The issuer this identifier names.
    pub issuer_id: IssuerId,
    /// The identifier scheme (open vocabulary). Normalized on write (FR-025).
    pub scheme: String,
    /// The identifier value. Trimmed on write (FR-025).
    pub value: String,
}
