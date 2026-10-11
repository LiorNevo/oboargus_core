//! Position: the current state of a holding in a portfolio. Derived from
//! events; not directly constructible.

use chrono::{DateTime, Utc};
use crate::domain::id::DomainId;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::domain::id::{AssetId, PortfolioId};

/// A materialized position. Constructed only by replaying events
/// (`Position::from_events`); there is no public field-level constructor.
#[derive(Debug, PartialEq, Eq, FromRow, Serialize, Deserialize)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct Position {
    pub portfolio_id: uuid::Uuid,
    pub asset_id: uuid::Uuid,
    pub quantity: Decimal,
    pub as_of: DateTime<Utc>,
}

impl Position {
    /// Borrow the portfolio id.
    /// Implements FR-008 of spec 016-implement-domain-model.
    pub fn portfolio_id(&self) -> PortfolioId {
        PortfolioId::from_uuid(self.portfolio_id)
    }
    /// Borrow the asset id.
    /// Implements FR-008 of spec 016-implement-domain-model.
    pub fn asset_id(&self) -> AssetId {
        AssetId::from_uuid(self.asset_id)
    }
}
