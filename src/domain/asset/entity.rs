//! `Asset` struct and input/patch types.

use chrono::{DateTime, Utc};
use crate::domain::id::DomainId;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::domain::id::{AssetId, IssuerId, PortfolioId};

/// A persisted asset.
#[derive(Debug, PartialEq, Eq, FromRow, Serialize, Deserialize)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct Asset {
    /// The asset id.
    pub id: uuid::Uuid,
    /// The asset type (e.g., `"stock"`, `"bond"`, `"etf"`).
    pub asset_type: String,
    /// The human-readable display name.
    pub display_name: String,
    /// The ISO 4217 currency code.
    pub currency: String,
    /// When the asset was created.
    pub created_at: DateTime<Utc>,
    /// The portfolio that backs this asset, if any (e.g., for an ETF).
    pub backed_by_portfolio_id: Option<uuid::Uuid>,
    /// The issuer of this asset, if any.
    pub issuer_id: Option<uuid::Uuid>,
}

impl Asset {
    /// Borrow the id as an `AssetId`.
    /// Implements FR-008 of spec 016-implement-domain-model.
    pub fn id(&self) -> AssetId {
        AssetId::from_uuid(self.id)
    }
    /// Borrow the back-reference, if any.
    /// Implements FR-008 of spec 016-implement-domain-model.
    pub fn backed_by(&self) -> Option<PortfolioId> {
        self.backed_by_portfolio_id.map(PortfolioId::from_uuid)
    }
    /// Borrow the issuer, if any.
    /// Implements FR-008 of spec 016-implement-domain-model.
    pub fn issuer(&self) -> Option<IssuerId> {
        self.issuer_id.map(IssuerId::from_uuid)
    }
}

/// Input for `create_asset`.
#[derive(Debug, Clone)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct NewAsset {
    /// The asset type.
    pub asset_type: String,
    /// The display name.
    pub display_name: String,
    /// The currency code.
    pub currency: String,
    /// The portfolio that backs this asset, if any.
    pub backed_by_portfolio_id: Option<PortfolioId>,
    /// The issuer of this asset, if any.
    pub issuer_id: Option<IssuerId>,
}

/// Patch for `update_asset`. Only the fields present in the patch are
/// updated. `id`, `created_at`, `backed_by_portfolio_id`, and `issuer_id`
/// are NOT updatable through this struct.
#[derive(Debug, Clone, Default)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct AssetPatch {
    /// New asset type, if changing.
    pub asset_type: Option<String>,
    /// New display name, if changing.
    pub display_name: Option<String>,
    /// New currency, if changing.
    pub currency: Option<String>,
}

/// Filter for `list_assets`.
#[derive(Debug, Clone, Default)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct AssetFilter {
    /// Filter by asset type, if set.
    pub asset_type: Option<String>,
    /// Filter by currency, if set.
    pub currency: Option<String>,
    /// Filter by issuer, if set.
    pub issuer_id: Option<IssuerId>,
}
