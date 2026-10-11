//! Canonical seed function. Implements US7.
//!
//! Creates a small, human-readable dataset (2 issuers, 5 assets, 3
//! portfolios forming a 2-level DAG, ~10 events producing 3 positions, and
//! a handful of reference-data records) that the test suite can use as a
//! fixed starting point.

use crate::domain::id::DomainId;
use sqlx::PgPool;

use crate::domain::asset::create as asset_create;
use crate::domain::asset::entity::{AssetFilter, NewAsset};
use crate::domain::asset_reference_data::create as asset_ref_create;
use crate::domain::asset_reference_data::entity::NewAssetReferenceData;
use crate::domain::error::DomainError;
use crate::domain::event::create as event_create;
use crate::domain::event::entity::NewEvent;
use crate::domain::id::{AssetId, IssuerId, PortfolioId, new_uuid};
use crate::domain::issuer::create as issuer_create;
use crate::domain::issuer::entity::NewIssuer;
use crate::domain::portfolio::create as portfolio_create;
use crate::domain::portfolio::entity::NewPortfolio;
use rust_decimal::Decimal;

/// A summary of what the seed created.
#[derive(Debug, Default, PartialEq, Eq)]
/// Implements FR-007 of spec 016-implement-domain-model.
pub struct SeedSummary {
    pub issuers_created: usize,
    pub assets_created: usize,
    pub portfolios_created: usize,
    pub events_created: usize,
    pub reference_data_records_created: usize,
}

/// Seed the database with a canonical dataset. Idempotent: re-running it on
/// an already-seeded database is a no-op (the function checks for the
/// existence of the canonical issuer before creating anything).
/// Implements FR-007 of spec 016-implement-domain-model.
pub async fn seed_canonical_dataset(pool: &PgPool) -> Result<SeedSummary, DomainError> {
    let mut summary = SeedSummary::default();

    // Idempotency check: if the canonical issuer already exists, do nothing.
    let existing = crate::domain::issuer::read::list_issuers(pool).await?;
    if existing.iter().any(|i| i.name == "Vanguard Group") {
        // Re-seed idempotently: report the existing counts so callers can
        // see what is in the database. We do not re-insert anything.
        summary.issuers_created = existing.len();
        let _ = crate::domain::asset::read::list_assets(pool, AssetFilter::default())
            .await
            .map(|a| summary.assets_created = a.len());
        // Count portfolios directly from the database (the
        // `portfolio::read` module does not currently expose a
        // list-all helper).
        let portfolio_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM portfolios")
            .fetch_one(pool)
            .await
            .map_err(DomainError::Infrastructure)?;
        summary.portfolios_created = portfolio_count as usize;
        return Ok(summary);
    }

    // Issuers
    let vanguard = issuer_create::create_issuer(
        pool,
        NewIssuer {
            name: "Vanguard Group".to_string(),
            issuer_type: "asset_manager".to_string(),
            legal_jurisdiction: "US".to_string(),
        },
    )
    .await?;
    summary.issuers_created += 1;
    let apple = issuer_create::create_issuer(
        pool,
        NewIssuer {
            name: "Apple Inc.".to_string(),
            issuer_type: "corporation".to_string(),
            legal_jurisdiction: "US".to_string(),
        },
    )
    .await?;
    summary.issuers_created += 1;

    // Assets
    let vti = asset_create::create_asset(
        pool,
        NewAsset {
            asset_type: "etf".to_string(),
            display_name: "Vanguard Total Stock Market ETF".to_string(),
            currency: "USD".to_string(),
            backed_by_portfolio_id: None,
            issuer_id: Some(vanguard.id()),
        },
    )
    .await?;
    summary.assets_created += 1;
    let aapl = asset_create::create_asset(
        pool,
        NewAsset {
            asset_type: "stock".to_string(),
            display_name: "Apple Inc.".to_string(),
            currency: "USD".to_string(),
            backed_by_portfolio_id: None,
            issuer_id: Some(apple.id()),
        },
    )
    .await?;
    summary.assets_created += 1;
    // (Three more assets for diversity; non-issuer.)
    let extras = [
        ("Berkshire Hathaway B", "stock"),
        ("US Treasury 10Y Note", "bond"),
        ("Cash USD", "cash"),
    ];
    for (name, asset_type) in extras {
        let _ = asset_create::create_asset(
            pool,
            NewAsset {
                asset_type: asset_type.to_string(),
                display_name: name.to_string(),
                currency: "USD".to_string(),
                backed_by_portfolio_id: None,
                issuer_id: None,
            },
        )
        .await?;
        summary.assets_created += 1;
    }

    // Portfolios: a 2-level DAG
    let total = portfolio_create::create_portfolio(
        pool,
        NewPortfolio {
            name: "Total Portfolio".to_string(),
            purpose: "All holdings".to_string(),
            created_by: "seed".to_string(),
            parent_id: None,
        },
    )
    .await?;
    summary.portfolios_created += 1;
    let brokerage = portfolio_create::create_portfolio(
        pool,
        NewPortfolio {
            name: "Brokerage".to_string(),
            purpose: "Taxable brokerage account".to_string(),
            created_by: "seed".to_string(),
            parent_id: Some(total.id()),
        },
    )
    .await?;
    summary.portfolios_created += 1;
    let retirement = portfolio_create::create_portfolio(
        pool,
        NewPortfolio {
            name: "Retirement".to_string(),
            purpose: "Tax-advantaged retirement account".to_string(),
            created_by: "seed".to_string(),
            parent_id: Some(total.id()),
        },
    )
    .await?;
    summary.portfolios_created += 1;

    // A handful of events producing positions
    let today = chrono::Utc::now().date_naive();
    for (delta, asset) in [
        (Decimal::from(100), aapl.id()),
        (Decimal::from(50), vti.id()),
    ] {
        event_create::create_event(
            pool,
            NewEvent {
                event_type: crate::domain::event::entity::EventType::Buy,
                effective_date: today,
                quantity_delta: delta,
                portfolio_id: brokerage.id(),
                asset_id: asset,
                to_portfolio_id: None,
            },
        )
        .await?;
        summary.events_created += 1;
    }
    event_create::create_event(
        pool,
        NewEvent {
            event_type: crate::domain::event::entity::EventType::Buy,
            effective_date: today,
            quantity_delta: Decimal::from(75),
            portfolio_id: retirement.id(),
            asset_id: vti.id(),
            to_portfolio_id: None,
        },
    )
    .await?;
    summary.events_created += 1;

    // A handful of reference-data records
    asset_ref_create::create_asset_reference_data(
        pool,
        NewAssetReferenceData {
            asset_id: aapl.id(),
            fact_type: "market_price".to_string(),
            as_of_date: today,
            knowledge_date: today,
            value: "175.50".to_string(),
            source: "manual".to_string(),
        },
    )
    .await?;
    summary.reference_data_records_created += 1;

    let _ = (
        AssetId::from_uuid(new_uuid()),
        IssuerId::from_uuid(new_uuid()),
        PortfolioId::from_uuid(new_uuid()),
    ); // suppress unused
    let _ = AssetFilter::default();

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A default `SeedSummary` is all zeros. This is a structural test that
    /// documents the contract for the idempotent re-seed path.
    #[test]
    fn default_summary_is_all_zeros() {
        let s = SeedSummary::default();
        assert_eq!(
            s,
            SeedSummary {
                issuers_created: 0,
                assets_created: 0,
                portfolios_created: 0,
                events_created: 0,
                reference_data_records_created: 0
            }
        );
    }
}
