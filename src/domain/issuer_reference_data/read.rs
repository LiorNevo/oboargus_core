use chrono::NaiveDate;
use crate::domain::id::DomainId;
use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::IssuerId;
use crate::domain::issuer_reference_data::entity::IssuerReferenceData;

/// Implements FR-006 of spec 016-implement-domain-model.
pub async fn get_current_issuer_reference_data(
    pool: &PgPool,
    issuer_id: IssuerId,
    fact_type: &str,
    as_of_date: NaiveDate,
) -> Result<Option<IssuerReferenceData>, DomainError> {
    let row: Option<IssuerReferenceData> = sqlx::query_as(
        r#"
        SELECT id, issuer_id, fact_type, as_of_date, knowledge_date, value, source
        FROM issuer_reference_data
        WHERE issuer_id = $1 AND fact_type = $2 AND as_of_date = $3
        ORDER BY knowledge_date DESC LIMIT 1
        "#,
    )
    .bind(issuer_id.as_uuid())
    .bind(fact_type)
    .bind(as_of_date)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Implements FR-006 of spec 016-implement-domain-model.
pub async fn get_issuer_reference_data_history(
    pool: &PgPool,
    issuer_id: IssuerId,
    fact_type: &str,
    as_of_date: NaiveDate,
) -> Result<Vec<IssuerReferenceData>, DomainError> {
    let rows: Vec<IssuerReferenceData> = sqlx::query_as(
        r#"
        SELECT id, issuer_id, fact_type, as_of_date, knowledge_date, value, source
        FROM issuer_reference_data
        WHERE issuer_id = $1 AND fact_type = $2 AND as_of_date = $3
        ORDER BY knowledge_date DESC
        "#,
    )
    .bind(issuer_id.as_uuid())
    .bind(fact_type)
    .bind(as_of_date)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
