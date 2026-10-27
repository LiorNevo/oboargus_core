use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::DomainId;
use crate::domain::id::new_uuid;
use crate::domain::portfolio_reference_data::entity::{
    NewPortfolioReferenceData, PortfolioReferenceData,
};
use crate::domain::time_support::utc_midnight;

/// Appends one belief about a portfolio, with day-granular time coordinates.
///
/// The `as_of_date` becomes `valid_at` at midnight UTC and `knowledge_date`
/// becomes `known_at` at midnight UTC. Corrections that need sub-day
/// transaction time use [`create_portfolio_reference_data_with_known_at`].
///
/// Implements FR-018 of spec 016-implement-domain-model and FR-001/FR-009 of
/// spec 021-bitemporal-market-data.
pub async fn create_portfolio_reference_data(
    pool: &PgPool,
    input: NewPortfolioReferenceData,
) -> Result<PortfolioReferenceData, DomainError> {
    let known_at = utc_midnight(input.knowledge_date);
    create_portfolio_reference_data_with_known_at(pool, input, known_at).await
}

/// Appends one belief about a portfolio with an explicit `known_at`.
///
/// Used by same-day corrections and by market-data ingestion, where the
/// transaction time is "when we recorded it", not a calendar date.
pub async fn create_portfolio_reference_data_with_known_at(
    pool: &PgPool,
    input: NewPortfolioReferenceData,
    known_at: DateTime<Utc>,
) -> Result<PortfolioReferenceData, DomainError> {
    if input.fact_type.trim().is_empty() {
        return Err(DomainError::Validation(
            "fact_type must not be empty".to_string(),
        ));
    }
    if input.source.trim().is_empty() {
        return Err(DomainError::Validation(
            "source must not be empty".to_string(),
        ));
    }

    let id = new_uuid();
    let valid_at = utc_midnight(input.as_of_date);
    let sql = r#"
        INSERT INTO portfolio_reference_data
            (id, portfolio_id, fact_type, valid_at, known_at, value, source)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id, portfolio_id, fact_type, as_of_date, knowledge_date, value, source
        "#;
    let row: PortfolioReferenceData = sqlx::query_as(sql)
        .bind(id)
        .bind(input.portfolio_id.as_uuid())
        .bind(&input.fact_type)
        .bind(valid_at)
        .bind(known_at)
        .bind(&input.value)
        .bind(&input.source)
        .fetch_one(pool)
        .await?;
    Ok(row)
}
