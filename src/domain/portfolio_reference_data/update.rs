use chrono::{NaiveDate, Utc};
use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::PortfolioId;
use crate::domain::portfolio_reference_data::create::create_portfolio_reference_data;
use crate::domain::portfolio_reference_data::entity::{
    NewPortfolioReferenceData, PortfolioReferenceData,
};

/// Implements FR-018 of spec 016-implement-domain-model.
pub async fn update_portfolio_reference_value(
    pool: &PgPool,
    portfolio_id: PortfolioId,
    fact_type: &str,
    as_of_date: NaiveDate,
    new_value: String,
    new_source: String,
) -> Result<PortfolioReferenceData, DomainError> {
    let new_knowledge_date = Utc::now().date_naive();
    create_portfolio_reference_data(
        pool,
        NewPortfolioReferenceData {
            portfolio_id,
            fact_type: fact_type.to_string(),
            as_of_date,
            knowledge_date: new_knowledge_date,
            value: new_value,
            source: new_source,
        },
    )
    .await
}
