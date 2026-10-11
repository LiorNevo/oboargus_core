//! Event: an immutable record of a state change to a position.

use crate::domain::id::DomainId;
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::domain::id::{AssetId, EventId, PortfolioId};

/// Discriminator for the kind of event.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
/// Implements FR-008 of spec 016-implement-domain-model.
pub enum EventType {
    /// A buy at the target (portfolio, asset) for `+quantity_delta`.
    Buy,
    /// A sell at the target (portfolio, asset) for `-quantity_delta`.
    Sell,
    /// A transfer of `quantity_delta` units from the target portfolio to
    /// `to_portfolio_id` (per FR-026 of the predecessor).
    Transfer,
    /// A split that multiplies the current quantity by the ratio carried in
    /// the per-type attribute payload.
    Split,
    /// A merger (per-type attribute payload carries the exchange ratio).
    Merger,
    /// A spin-off (per-type attribute payload carries the distribution).
    SpinOff,
    /// A dividend (per-type attribute payload carries the per-share amount).
    Dividend,
    /// A stock-based compensation grant (per-type attribute payload carries
    /// the grant details).
    StockBasedCompensation,
}

impl EventType {
    /// The wire representation as stored in the `event_type` column.
    /// Implements FR-008 of spec 016-implement-domain-model.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
            Self::Transfer => "transfer",
            Self::Split => "split",
            Self::Merger => "merger",
            Self::SpinOff => "spin-off",
            Self::Dividend => "dividend",
            Self::StockBasedCompensation => "stock-based-compensation",
        }
    }
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for EventType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "buy" => Ok(Self::Buy),
            "sell" => Ok(Self::Sell),
            "transfer" => Ok(Self::Transfer),
            "split" => Ok(Self::Split),
            "merger" => Ok(Self::Merger),
            "spin-off" => Ok(Self::SpinOff),
            "dividend" => Ok(Self::Dividend),
            "stock-based-compensation" => Ok(Self::StockBasedCompensation),
            other => Err(format!("unknown event type: {other}")),
        }
    }
}

/// A persisted event.
#[derive(Debug, PartialEq, Eq, FromRow, Serialize, Deserialize)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct Event {
    pub id: uuid::Uuid,
    pub event_type: String,
    pub effective_date: NaiveDate,
    pub quantity_delta: Decimal,
    pub recorded_at: DateTime<Utc>,
    pub portfolio_id: uuid::Uuid,
    pub asset_id: uuid::Uuid,
    pub to_portfolio_id: Option<uuid::Uuid>,
}

impl AsRef<Event> for Event {
    fn as_ref(&self) -> &Event {
        self
    }
}

impl Event {
    /// Borrow the event id.
    /// Implements FR-008 of spec 016-implement-domain-model.
    pub fn id(&self) -> EventId {
        EventId::from_uuid(self.id)
    }
    /// Borrow the event type as a typed enum.
    /// Implements FR-008 of spec 016-implement-domain-model.
    pub fn event_type(&self) -> Result<EventType, String> {
        self.event_type.parse()
    }
    /// Borrow the source portfolio id.
    /// Implements FR-008 of spec 016-implement-domain-model.
    pub fn portfolio_id(&self) -> PortfolioId {
        PortfolioId::from_uuid(self.portfolio_id)
    }
    /// Borrow the destination portfolio id (for transfer events).
    /// Implements FR-008 of spec 016-implement-domain-model.
    pub fn to_portfolio_id_opt(&self) -> Option<PortfolioId> {
        self.to_portfolio_id.map(PortfolioId::from_uuid)
    }
    /// Borrow the asset id.
    /// Implements FR-008 of spec 016-implement-domain-model.
    pub fn asset_id(&self) -> AssetId {
        AssetId::from_uuid(self.asset_id)
    }
}

/// Input for `create_event`.
#[derive(Debug, Clone)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct NewEvent {
    pub event_type: EventType,
    pub effective_date: NaiveDate,
    pub quantity_delta: Decimal,
    pub portfolio_id: PortfolioId,
    pub asset_id: AssetId,
    pub to_portfolio_id: Option<PortfolioId>,
}
