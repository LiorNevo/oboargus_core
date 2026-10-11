use crate::domain::id::DomainId;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::domain::id::{IssuerId, ReferenceDataId};

#[derive(Debug, PartialEq, Eq, FromRow, Serialize, Deserialize)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct IssuerReferenceData {
    pub id: uuid::Uuid,
    pub issuer_id: uuid::Uuid,
    pub fact_type: String,
    pub as_of_date: NaiveDate,
    pub knowledge_date: NaiveDate,
    pub value: String,
    pub source: String,
}

impl IssuerReferenceData {
    /// Implements FR-008 of spec 016-implement-domain-model.
    pub fn id(&self) -> ReferenceDataId {
        ReferenceDataId::from_uuid(self.id)
    }
    /// Implements FR-008 of spec 016-implement-domain-model.
    pub fn issuer_id(&self) -> IssuerId {
        IssuerId::from_uuid(self.issuer_id)
    }
}

#[derive(Debug, Clone)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct NewIssuerReferenceData {
    pub issuer_id: IssuerId,
    pub fact_type: String,
    pub as_of_date: NaiveDate,
    pub knowledge_date: NaiveDate,
    pub value: String,
    pub source: String,
}
