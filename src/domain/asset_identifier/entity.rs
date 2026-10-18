//! `AssetIdentifier` struct and input types.
//!
//! An external identifier (ISIN, ticker, CUSIP, Bloomberg ID, ...) by which
//! an asset is known outside Obolargus. Identifiers are current-state keys,
//! not dual-temporal facts (predecessor FR-033): the record carries a
//! `scheme` and a `value`, no `as_of_date` / `knowledge_date`.
//!
//! Implements FR-023 of `specs/016-implement-domain-model/spec.md`
//! (predecessor FR-028 of `specs/015-database-uml-domains/spec.md`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::domain::id::{AssetId, DomainId, IdentifierId};

/// A persisted external identifier naming an asset.
#[derive(Debug, PartialEq, Eq, FromRow, Serialize, Deserialize)]
/// Implements FR-023 of spec 016-implement-domain-model.
pub struct AssetIdentifier {
    /// The identifier record id.
    pub id: uuid::Uuid,
    /// The asset this identifier names.
    pub asset_id: uuid::Uuid,
    /// The identifier scheme (open vocabulary, e.g. `"ISIN"`).
    /// Stored trimmed and uppercased (FR-025).
    pub scheme: String,
    /// The identifier value (e.g. `"US0378331005"`).
    /// Stored trimmed; matched exactly (FR-025).
    pub value: String,
    /// When the identifier record was created.
    pub created_at: DateTime<Utc>,
}

impl AssetIdentifier {
    /// Borrow the record id as an `IdentifierId`.
    /// Implements FR-023 of spec 016-implement-domain-model.
    pub fn id(&self) -> IdentifierId {
        IdentifierId::from_uuid(self.id)
    }
    /// Borrow the named asset's id as an `AssetId`.
    /// Implements FR-023 of spec 016-implement-domain-model.
    pub fn asset_id(&self) -> AssetId {
        AssetId::from_uuid(self.asset_id)
    }
}

/// Input for `create_asset_identifier`.
#[derive(Debug, Clone)]
/// Implements FR-023 of spec 016-implement-domain-model.
pub struct NewAssetIdentifier {
    /// The asset this identifier names.
    pub asset_id: AssetId,
    /// The identifier scheme (open vocabulary). Normalized on write (FR-025).
    pub scheme: String,
    /// The identifier value. Trimmed on write (FR-025).
    pub value: String,
}
