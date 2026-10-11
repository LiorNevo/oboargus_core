//! `Portfolio` struct and input/patch types.

use crate::domain::id::DomainId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::domain::id::PortfolioId;

/// A persisted portfolio. Implements `sqlx::FromRow` so it can be loaded
/// directly from a query.
#[derive(Debug, PartialEq, Eq, FromRow, Serialize, Deserialize)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct Portfolio {
    /// The portfolio id.
    pub id: UuidWrapper,
    /// The human-readable name.
    pub name: String,
    /// The human-readable purpose.
    pub purpose: String,
    /// When the portfolio was created.
    pub created_at: DateTime<Utc>,
    /// Who created the portfolio.
    pub created_by: String,
    /// The parent portfolio id, if any.
    pub parent_id: Option<UuidWrapper>,
}

/// Wrapper for the `sqlx::Type` derivation. We use a transparent newtype
/// around `uuid::Uuid` so the struct can implement `FromRow` directly.
pub type UuidWrapper = uuid::Uuid;

/// Input for `create_portfolio`.
#[derive(Debug, Clone)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct NewPortfolio {
    /// The portfolio name.
    pub name: String,
    /// The portfolio purpose.
    pub purpose: String,
    /// Who is creating the portfolio.
    pub created_by: String,
    /// The parent portfolio id, if any.
    pub parent_id: Option<PortfolioId>,
}

/// Patch for `update_portfolio`. Only the fields present in the patch are
/// updated; absent fields are unchanged. `id`, `created_at`, `created_by`,
/// and `parent_id` are NOT updatable through this struct (parent changes
/// require a separate `reparent_portfolio` operation, which is out of scope
/// for this iteration).
#[derive(Debug, Clone, Default)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct PortfolioPatch {
    /// New name, if changing.
    pub name: Option<String>,
    /// New purpose, if changing.
    pub purpose: Option<String>,
}

impl Portfolio {
    /// Borrow the id as a `PortfolioId`.
    /// Implements FR-008 of spec 016-implement-domain-model.
    pub fn id(&self) -> PortfolioId {
        PortfolioId::from_uuid(self.id)
    }

    /// Borrow the parent id, if any, as a `PortfolioId`.
    /// Implements FR-008 of spec 016-implement-domain-model.
    pub fn parent_id_opt(&self) -> Option<PortfolioId> {
        self.parent_id.map(PortfolioId::from_uuid)
    }
}
