use crate::domain::id::DomainId;
use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::new_uuid;
use crate::domain::portfolio_reference_data::entity::{
    NewPortfolioReferenceData, PortfolioReferenceData,
};

/// Implements FR-018 of spec 016-implement-domain-model.
pub async fn create_portfolio_reference_data(
    pool: &PgPool,
    input: NewPortfolioReferenceData,
) -> Result<PortfolioReferenceData, DomainError> {
    let id = new_uuid();
    sqlx::query(
        r#"
        INSERT INTO portfolio_reference_data (id, portfolio_id, fact_type, as_of_date, knowledge_date, value, source)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(id)
    .bind(input.portfolio_id.as_uuid())
    .bind(&input.fact_type)
    .bind(input.as_of_date)
    .bind(input.knowledge_date)
    .bind(&input.value)
    .bind(&input.source)
    .execute(pool)
    .await?;
    super::read::get_current_portfolio_reference_data(
        pool,
        input.portfolio_id,
        &input.fact_type,
        input.as_of_date,
    )
    .await?
    .ok_or_else(|| DomainError::not_found("PortfolioReferenceData", id))
}
