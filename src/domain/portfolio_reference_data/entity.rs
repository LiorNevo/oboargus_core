use chrono::NaiveDate;
use crate::domain::id::DomainId;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::domain::id::{PortfolioId, ReferenceDataId};

#[derive(Debug, PartialEq, Eq, FromRow, Serialize, Deserialize)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct PortfolioReferenceData {
    pub id: uuid::Uuid,
    pub portfolio_id: uuid::Uuid,
    pub fact_type: String,
    pub as_of_date: NaiveDate,
    pub knowledge_date: NaiveDate,
    pub value: String,
    pub source: String,
}

impl PortfolioReferenceData {
    /// Implements FR-008 of spec 016-implement-domain-model.
    pub fn id(&self) -> ReferenceDataId { ReferenceDataId::from_uuid(self.id) }
    /// Implements FR-008 of spec 016-implement-domain-model.
    pub fn portfolio_id(&self) -> PortfolioId { PortfolioId::from_uuid(self.portfolio_id) }
}

#[derive(Debug, Clone)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct NewPortfolioReferenceData {
    pub portfolio_id: PortfolioId,
    pub fact_type: String,
    pub as_of_date: NaiveDate,
    pub knowledge_date: NaiveDate,
    pub value: String,
    pub source: String,
}
