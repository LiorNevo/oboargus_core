use crate::domain::id::DomainId;
use chrono::NaiveDate;
use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::IssuerId;
use crate::domain::issuer_reference_data::entity::IssuerReferenceData;

/// The value in force for `(issuer, fact_type, as_of_date)` under the latest
/// knowledge, or `None` when the latest belief is a retraction.
///
/// Implements FR-006 of spec 016-implement-domain-model and FR-005/FR-006 of
/// spec 021-bitemporal-market-data.
pub async fn get_current_issuer_reference_data(
    pool: &PgPool,
    issuer_id: IssuerId,
    fact_type: &str,
    as_of_date: NaiveDate,
) -> Result<Option<IssuerReferenceData>, DomainError> {
    let sql = r#"
        SELECT id, issuer_id, fact_type, as_of_date, knowledge_date, value, source
        FROM issuer_reference_data
        WHERE issuer_id = $1 AND fact_type = $2 AND as_of_date = $3
          AND NOT retracted
        ORDER BY known_at DESC, id DESC LIMIT 1
        "#;
    let row: Option<IssuerReferenceData> = sqlx::query_as(sql)
        .bind(issuer_id.as_uuid())
        .bind(fact_type)
        .bind(as_of_date)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// Every recorded belief for `(issuer, fact_type, as_of_date)`, newest
/// knowledge first.
///
/// Implements FR-006 of spec 016-implement-domain-model and FR-007 of
/// spec 021-bitemporal-market-data.
pub async fn get_issuer_reference_data_history(
    pool: &PgPool,
    issuer_id: IssuerId,
    fact_type: &str,
    as_of_date: NaiveDate,
) -> Result<Vec<IssuerReferenceData>, DomainError> {
    let sql = r#"
        SELECT id, issuer_id, fact_type, as_of_date, knowledge_date, value, source
        FROM issuer_reference_data
        WHERE issuer_id = $1 AND fact_type = $2 AND as_of_date = $3
        ORDER BY known_at DESC, id DESC
        "#;
    let rows: Vec<IssuerReferenceData> = sqlx::query_as(sql)
        .bind(issuer_id.as_uuid())
        .bind(fact_type)
        .bind(as_of_date)
        .fetch_all(pool)
        .await?;
    Ok(rows)
}
