use chrono::{NaiveDate, Utc};
use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::IssuerId;
use crate::domain::issuer_reference_data::create::create_issuer_reference_data;
use crate::domain::issuer_reference_data::entity::{IssuerReferenceData, NewIssuerReferenceData};

/// Implements FR-018 of spec 016-implement-domain-model.
pub async fn update_issuer_reference_value(
    pool: &PgPool,
    issuer_id: IssuerId,
    fact_type: &str,
    as_of_date: NaiveDate,
    new_value: String,
    new_source: String,
) -> Result<IssuerReferenceData, DomainError> {
    let new_knowledge_date = Utc::now().date_naive();
    create_issuer_reference_data(
        pool,
        NewIssuerReferenceData {
            issuer_id,
            fact_type: fact_type.to_string(),
            as_of_date,
            knowledge_date: new_knowledge_date,
            value: new_value,
            source: new_source,
        },
    )
    .await
}
