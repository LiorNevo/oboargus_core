//! Update the `value`/`source` of a reference-data record. Per Assumption
//! 11, this writes a NEW record with a later `knowledge_date`; the original
//! record is preserved in the history.

use chrono::{NaiveDate, Utc};
use sqlx::PgPool;

use crate::domain::asset_reference_data::create::create_asset_reference_data;
use crate::domain::asset_reference_data::entity::NewAssetReferenceData;
use crate::domain::error::DomainError;
use crate::domain::id::AssetId;

/// Implements FR-018 of spec 016-implement-domain-model.
pub async fn update_asset_reference_value(
    pool: &PgPool,
    asset_id: AssetId,
    fact_type: &str,
    as_of_date: NaiveDate,
    new_value: String,
    new_source: String,
) -> Result<crate::domain::asset_reference_data::entity::AssetReferenceData, DomainError> {
    let new_knowledge_date = Utc::now().date_naive();
    create_asset_reference_data(
        pool,
        NewAssetReferenceData {
            asset_id,
            fact_type: fact_type.to_string(),
            as_of_date,
            knowledge_date: new_knowledge_date,
            value: new_value,
            source: new_source,
        },
    )
    .await
}
