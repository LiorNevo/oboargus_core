//! Enforce the no-self-investment invariant.

use sqlx::PgPool;
use crate::domain::id::DomainId;

use crate::domain::error::DomainError;
use crate::domain::id::{AssetId, PortfolioId};

/// Check that an asset is not (transitively) contained in the portfolio that
/// backs it. Implements the no-self-investment invariant.
/// Implements FR-002 of spec 016-implement-domain-model.
pub async fn check_no_self_investment(
    pool: &PgPool,
    asset_id: AssetId,
    backed_by_portfolio_id: PortfolioId,
) -> Result<(), DomainError> {
    // The simple case: the asset is directly in the backed_by list of any
    // asset whose backed_by_portfolio_id is the candidate. For the
    // transitive case, walk the chain.
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        WITH RECURSIVE chain AS (
            SELECT id, backed_by_portfolio_id
            FROM assets WHERE id = $1
            UNION
            SELECT a.id, a.backed_by_portfolio_id
            FROM assets a
            INNER JOIN chain c ON a.backed_by_portfolio_id = c.id
        )
        SELECT 1 FROM chain WHERE backed_by_portfolio_id = $2 LIMIT 1
        "#,
    )
    .bind(asset_id.as_uuid())
    .bind(backed_by_portfolio_id.as_uuid())
    .fetch_optional(pool)
    .await?;

    if row.is_some() {
        return Err(DomainError::invariant(
            "no_self_investment",
            format!(
                "asset {} would be transitively contained in the portfolio {} that backs it",
                asset_id, backed_by_portfolio_id
            ),
        ));
    }
    Ok(())
}
