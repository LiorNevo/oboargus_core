use crate::domain::id::DomainId;
use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::new_uuid;
use crate::domain::issuer_reference_data::entity::{IssuerReferenceData, NewIssuerReferenceData};

/// Implements FR-018 of spec 016-implement-domain-model.
pub async fn create_issuer_reference_data(
    pool: &PgPool,
    input: NewIssuerReferenceData,
) -> Result<IssuerReferenceData, DomainError> {
    let id = new_uuid();
    sqlx::query(
        r#"
        INSERT INTO issuer_reference_data (id, issuer_id, fact_type, as_of_date, knowledge_date, value, source)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(id)
    .bind(input.issuer_id.as_uuid())
    .bind(&input.fact_type)
    .bind(input.as_of_date)
    .bind(input.knowledge_date)
    .bind(&input.value)
    .bind(&input.source)
    .execute(pool)
    .await?;
    super::read::get_current_issuer_reference_data(
        pool,
        input.issuer_id,
        &input.fact_type,
        input.as_of_date,
    )
    .await?
    .ok_or_else(|| DomainError::not_found("IssuerReferenceData", id))
}
