use crate::domain::id::DomainId;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::domain::id::{AssetId, ReferenceDataId};

#[derive(Debug, PartialEq, Eq, FromRow, Serialize, Deserialize)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct AssetReferenceData {
    pub id: uuid::Uuid,
    pub asset_id: uuid::Uuid,
    pub fact_type: String,
    pub as_of_date: NaiveDate,
    pub knowledge_date: NaiveDate,
    pub value: String,
    pub source: String,
}

impl AssetReferenceData {
    /// Implements FR-008 of spec 016-implement-domain-model.
    pub fn id(&self) -> ReferenceDataId {
        ReferenceDataId::from_uuid(self.id)
    }
    /// Implements FR-008 of spec 016-implement-domain-model.
    pub fn asset_id(&self) -> AssetId {
        AssetId::from_uuid(self.asset_id)
    }
}

#[derive(Debug, Clone)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct NewAssetReferenceData {
    pub asset_id: AssetId,
    pub fact_type: String,
    pub as_of_date: NaiveDate,
    pub knowledge_date: NaiveDate,
    pub value: String,
    pub source: String,
}
