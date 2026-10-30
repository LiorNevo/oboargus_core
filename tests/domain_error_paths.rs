//! Database-backed tests for the domain model's error paths.
//!
//! Implements the executable proof of the *rejection* behaviour of spec 016
//! (FR-004, FR-017, FR-019, FR-024, FR-027): the `NotFound`, `CannotDelete`
//! and `InvariantViolation` branches that only fire when the database
//! actually holds (or lacks) the conflicting rows.
//!
//! These branches are unreachable from the pure unit tests, which have no
//! database, so the crate's coverage run never reached them.
//!
//! The tests use the shared `obolargus_core::common::tx_pool()` fixture,
//! which truncates every high-level table before each test and on drop.
//! The suite is forced to run serially via `RUST_TEST_THREADS=1` in
//! `obolargus_core/.cargo/config.toml`.
//!
//! The same database is shared with `obolargus_server`'s and
//! `obolargus_cli`'s integration tests, so run one crate's suite at a
//! time (the root `Makefile`'s `test` / `coverage` targets do that).
//!
//! When no test database is configured the tests skip (as a passing
//! no-op), per the `obolargus_core/AGENTS.md` convention.

use chrono::NaiveDate;
use rust_decimal::Decimal;

use obolargus_core::domain::asset::create::create_asset;
use obolargus_core::domain::asset::delete::delete_asset;
use obolargus_core::domain::asset::entity::{AssetFilter, AssetPatch, AssetSearchQuery, NewAsset};
use obolargus_core::domain::asset::read::{list_assets, search_asset_options};
use obolargus_core::domain::asset::update::update_asset;
use obolargus_core::domain::asset_identifier::create::create_asset_identifier;
use obolargus_core::domain::asset_identifier::entity::NewAssetIdentifier;
use obolargus_core::domain::asset_identifier::read::{
    find_asset_by_identifier, find_asset_identifier,
};
use obolargus_core::domain::error::DomainError;
use obolargus_core::domain::event::create::create_event;
use obolargus_core::domain::event::entity::{EventType, NewEvent};
use obolargus_core::domain::id::{AssetId, DomainId, IssuerId, PortfolioId};
use obolargus_core::domain::issuer::create::create_issuer;
use obolargus_core::domain::issuer::delete::delete_issuer;
use obolargus_core::domain::issuer::entity::{IssuerPatch, NewIssuer};
use obolargus_core::domain::issuer::reparent::reparent_issuer;
use obolargus_core::domain::issuer::update::update_issuer;
use obolargus_core::domain::issuer_identifier::create::create_issuer_identifier;
use obolargus_core::domain::issuer_identifier::entity::NewIssuerIdentifier;
use obolargus_core::domain::issuer_identifier::read::{
    find_issuer_by_identifier, find_issuer_identifier,
};
use obolargus_core::domain::portfolio::create::create_portfolio;
use obolargus_core::domain::portfolio::delete::delete_portfolio;
use obolargus_core::domain::portfolio::entity::{NewPortfolio, PortfolioPatch};
use obolargus_core::domain::portfolio::update::update_portfolio;

use obolargus_core::common::*;

/// A uuid that is guaranteed not to exist in the database.
fn missing_uuid() -> uuid::Uuid {
    uuid::Uuid::new_v4()
}

// ---------------------------------------------------------------------------
// Asset: not-found + filter paths
// ---------------------------------------------------------------------------

/// Deleting an asset that does not exist reports `NotFound` (FR-019).
#[tokio::test]
async fn delete_asset_reports_not_found() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let error = delete_asset(&pool, AssetId::from_uuid(missing_uuid()))
        .await
        .expect_err("err");
    assert!(
        matches!(
            error,
            DomainError::NotFound {
                entity: "Asset",
                ..
            }
        ),
        "got: {error:?}"
    );

    guard.cleanup().await;
}

/// Deleting an asset that carries reference data is rejected (FR-019).
#[tokio::test]
async fn delete_asset_rejects_an_asset_with_reference_data() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let asset = create_asset(
        &pool,
        NewAsset {
            asset_type: "EQUITY".to_string(),
            display_name: "Referenced Asset".to_string(),
            currency: "USD".to_string(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
    )
    .await
    .expect("create asset");

    sqlx::query(
        "INSERT INTO asset_reference_data \
         (id, asset_id, fact_type, valid_at, known_at, value, source) \
         VALUES (gen_random_uuid(), $1, 'market_price', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, '{}', 'test')",
    )
    .bind(asset.id)
    .execute(&pool)
    .await
    .expect("insert reference data");

    let error = delete_asset(&pool, AssetId::from_uuid(asset.id))
        .await
        .expect_err("err");
    assert!(
        matches!(
            error,
            DomainError::CannotDelete {
                entity: "Asset",
                ..
            }
        ),
        "got: {error:?}"
    );

    guard.cleanup().await;
}

/// Patching a missing asset reports `NotFound` for every updatable field
/// (FR-017).
#[tokio::test]
async fn update_asset_reports_not_found_for_every_field() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let missing = AssetId::from_uuid(missing_uuid());

    let error = update_asset(
        &pool,
        missing,
        AssetPatch {
            asset_type: Some("BOND".to_string()),
            display_name: None,
            currency: None,
        },
    )
    .await
    .expect_err("err");
    assert!(
        matches!(
            error,
            DomainError::NotFound {
                entity: "Asset",
                ..
            }
        ),
        "got: {error:?}"
    );

    let error = update_asset(
        &pool,
        missing,
        AssetPatch {
            asset_type: None,
            display_name: Some("Nope".to_string()),
            currency: None,
        },
    )
    .await
    .expect_err("err");
    assert!(
        matches!(
            error,
            DomainError::NotFound {
                entity: "Asset",
                ..
            }
        ),
        "got: {error:?}"
    );

    let error = update_asset(
        &pool,
        missing,
        AssetPatch {
            asset_type: None,
            display_name: None,
            currency: Some("EUR".to_string()),
        },
    )
    .await
    .expect_err("err");
    assert!(
        matches!(
            error,
            DomainError::NotFound {
                entity: "Asset",
                ..
            }
        ),
        "got: {error:?}"
    );

    guard.cleanup().await;
}

/// The asset filter narrows by type, by currency and by issuer (FR-008).
#[tokio::test]
async fn list_assets_filters_by_type_currency_and_issuer() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let issuer = create_issuer(
        &pool,
        NewIssuer {
            name: "Filter Issuer".to_string(),
            issuer_type: "CORPORATION".to_string(),
            legal_jurisdiction: "US".to_string(),
            parent_id: None,
        },
    )
    .await
    .expect("create issuer");

    create_asset(
        &pool,
        NewAsset {
            asset_type: "EQUITY".to_string(),
            display_name: "Equity USD".to_string(),
            currency: "USD".to_string(),
            backed_by_portfolio_id: None,
            issuer_id: Some(IssuerId::from_uuid(issuer.id)),
        },
    )
    .await
    .expect("create equity");
    create_asset(
        &pool,
        NewAsset {
            asset_type: "BOND".to_string(),
            display_name: "Bond EUR".to_string(),
            currency: "EUR".to_string(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
    )
    .await
    .expect("create bond");

    let all = list_assets(&pool, AssetFilter::default())
        .await
        .expect("all");
    assert_eq!(all.len(), 2);

    let equities = list_assets(
        &pool,
        AssetFilter {
            asset_type: Some("EQUITY".to_string()),
            ..AssetFilter::default()
        },
    )
    .await
    .expect("equities");
    assert_eq!(equities.len(), 1);
    assert_eq!(equities[0].display_name, "Equity USD");

    let euros = list_assets(
        &pool,
        AssetFilter {
            currency: Some("EUR".to_string()),
            ..AssetFilter::default()
        },
    )
    .await
    .expect("euros");
    assert_eq!(euros.len(), 1);
    assert_eq!(euros[0].display_name, "Bond EUR");

    let by_issuer = list_assets(
        &pool,
        AssetFilter {
            issuer_id: Some(IssuerId::from_uuid(issuer.id)),
            ..AssetFilter::default()
        },
    )
    .await
    .expect("by issuer");
    assert_eq!(by_issuer.len(), 1);
    assert_eq!(by_issuer[0].display_name, "Equity USD");

    guard.cleanup().await;
}

// ---------------------------------------------------------------------------
// Asset identifiers: not-found, uniqueness, empty-pair lookups
// ---------------------------------------------------------------------------

/// Registering an identifier for a missing asset reports `NotFound`
/// (FR-024).
#[tokio::test]
async fn create_asset_identifier_rejects_a_missing_asset() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let error = create_asset_identifier(
        &pool,
        NewAssetIdentifier {
            asset_id: AssetId::from_uuid(missing_uuid()),
            scheme: "TICKER".to_string(),
            value: "AAPL".to_string(),
        },
    )
    .await
    .expect_err("err");
    assert!(
        matches!(
            error,
            DomainError::NotFound {
                entity: "Asset",
                ..
            }
        ),
        "got: {error:?}"
    );

    guard.cleanup().await;
}

/// A `(scheme, value)` pair can only be claimed once (FR-024).
#[tokio::test]
async fn create_asset_identifier_rejects_a_duplicate_pair() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let asset = create_asset(
        &pool,
        NewAsset {
            asset_type: "EQUITY".to_string(),
            display_name: "First".to_string(),
            currency: "USD".to_string(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
    )
    .await
    .expect("create asset");
    let other = create_asset(
        &pool,
        NewAsset {
            asset_type: "EQUITY".to_string(),
            display_name: "Second".to_string(),
            currency: "USD".to_string(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
    )
    .await
    .expect("create asset");

    create_asset_identifier(
        &pool,
        NewAssetIdentifier {
            asset_id: AssetId::from_uuid(asset.id),
            scheme: "TICKER".to_string(),
            value: "AAPL".to_string(),
        },
    )
    .await
    .expect("first identifier");

    let error = create_asset_identifier(
        &pool,
        NewAssetIdentifier {
            asset_id: AssetId::from_uuid(other.id),
            scheme: "TICKER".to_string(),
            value: "AAPL".to_string(),
        },
    )
    .await
    .expect_err("err");
    assert!(
        matches!(error, DomainError::InvariantViolation { .. }),
        "got: {error:?}"
    );
    assert!(
        error.to_string().contains("identifier_uniqueness"),
        "got: {error}"
    );

    guard.cleanup().await;
}

/// An empty scheme or value can never be claimed, so the lookup resolves
/// to `Ok(None)` instead of erroring (FR-023).
#[tokio::test]
async fn asset_identifier_lookup_with_an_empty_pair_resolves_to_none() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    assert!(
        find_asset_identifier(&pool, "", "AAPL")
            .await
            .expect("lookup")
            .is_none()
    );
    assert!(
        find_asset_identifier(&pool, "TICKER", "")
            .await
            .expect("lookup")
            .is_none()
    );
    assert!(
        find_asset_by_identifier(&pool, "", "")
            .await
            .expect("lookup")
            .is_none()
    );

    guard.cleanup().await;
}

// ---------------------------------------------------------------------------
// Issuer: not-found, cannot-delete, is_root
// ---------------------------------------------------------------------------

/// Deleting an issuer that does not exist reports `NotFound` (FR-019).
#[tokio::test]
async fn delete_issuer_reports_not_found() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let error = delete_issuer(&pool, IssuerId::from_uuid(missing_uuid()))
        .await
        .expect_err("err");
    assert!(
        matches!(
            error,
            DomainError::NotFound {
                entity: "Issuer",
                ..
            }
        ),
        "got: {error:?}"
    );

    guard.cleanup().await;
}

/// Deleting an issuer that carries reference data is rejected — reference
/// data is immutable history (FR-019).
#[tokio::test]
async fn delete_issuer_rejects_an_issuer_with_reference_data() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let issuer = create_issuer(
        &pool,
        NewIssuer {
            name: "Referenced Issuer".to_string(),
            issuer_type: "CORPORATION".to_string(),
            legal_jurisdiction: "US".to_string(),
            parent_id: None,
        },
    )
    .await
    .expect("create issuer");

    sqlx::query(
        "INSERT INTO issuer_reference_data \
         (id, issuer_id, fact_type, valid_at, known_at, value, source) \
         VALUES (gen_random_uuid(), $1, 'lei', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, '{}', 'test')",
    )
    .bind(issuer.id)
    .execute(&pool)
    .await
    .expect("insert reference data");

    let error = delete_issuer(&pool, IssuerId::from_uuid(issuer.id))
        .await
        .expect_err("err");
    assert!(
        matches!(
            error,
            DomainError::CannotDelete {
                entity: "Issuer",
                ..
            }
        ),
        "got: {error:?}"
    );
    assert!(error.to_string().contains("reference data"), "got: {error}");

    guard.cleanup().await;
}

/// An issuer with no parent is a root (FR-008).
#[tokio::test]
async fn issuer_is_root_reflects_the_parent_link() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let root = create_issuer(
        &pool,
        NewIssuer {
            name: "Root Issuer".to_string(),
            issuer_type: "CORPORATION".to_string(),
            legal_jurisdiction: "US".to_string(),
            parent_id: None,
        },
    )
    .await
    .expect("create issuer");
    assert!(root.is_root(), "an issuer with no parent is a root");

    let child = create_issuer(
        &pool,
        NewIssuer {
            name: "Child Issuer".to_string(),
            issuer_type: "CORPORATION".to_string(),
            legal_jurisdiction: "US".to_string(),
            parent_id: Some(IssuerId::from_uuid(root.id)),
        },
    )
    .await
    .expect("create child issuer");
    assert!(!child.is_root(), "an issuer with a parent is not a root");

    guard.cleanup().await;
}

/// Patching a missing issuer reports `NotFound` for every updatable field
/// (FR-017).
#[tokio::test]
async fn update_issuer_reports_not_found_for_every_field() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let missing = IssuerId::from_uuid(missing_uuid());

    let error = update_issuer(
        &pool,
        missing,
        IssuerPatch {
            name: Some("Nope".to_string()),
            issuer_type: None,
            legal_jurisdiction: None,
        },
    )
    .await
    .expect_err("err");
    assert!(
        matches!(
            error,
            DomainError::NotFound {
                entity: "Issuer",
                ..
            }
        ),
        "got: {error:?}"
    );

    let error = update_issuer(
        &pool,
        missing,
        IssuerPatch {
            name: None,
            issuer_type: Some("BANK".to_string()),
            legal_jurisdiction: None,
        },
    )
    .await
    .expect_err("err");
    assert!(
        matches!(
            error,
            DomainError::NotFound {
                entity: "Issuer",
                ..
            }
        ),
        "got: {error:?}"
    );

    let error = update_issuer(
        &pool,
        missing,
        IssuerPatch {
            name: None,
            issuer_type: None,
            legal_jurisdiction: Some("DE".to_string()),
        },
    )
    .await
    .expect_err("err");
    assert!(
        matches!(
            error,
            DomainError::NotFound {
                entity: "Issuer",
                ..
            }
        ),
        "got: {error:?}"
    );

    guard.cleanup().await;
}

/// Reparenting an issuer that does not exist reports `NotFound` (FR-002).
#[tokio::test]
async fn reparent_issuer_reports_not_found() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let error = reparent_issuer(&pool, IssuerId::from_uuid(missing_uuid()), None)
        .await
        .expect_err("err");
    assert!(
        matches!(
            error,
            DomainError::NotFound {
                entity: "Issuer",
                ..
            }
        ),
        "got: {error:?}"
    );

    guard.cleanup().await;
}

/// Registering an issuer identifier for a missing issuer reports
/// `NotFound` (FR-027).
#[tokio::test]
async fn create_issuer_identifier_rejects_a_missing_issuer() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let error = create_issuer_identifier(
        &pool,
        NewIssuerIdentifier {
            issuer_id: IssuerId::from_uuid(missing_uuid()),
            scheme: "LEI".to_string(),
            value: "5493001KJTIIGC8Y1R12".to_string(),
        },
    )
    .await
    .expect_err("err");
    assert!(
        matches!(
            error,
            DomainError::NotFound {
                entity: "Issuer",
                ..
            }
        ),
        "got: {error:?}"
    );

    guard.cleanup().await;
}

/// An issuer identifier pair can only be claimed once (FR-027).
#[tokio::test]
async fn create_issuer_identifier_rejects_a_duplicate_pair() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let first = create_issuer(
        &pool,
        NewIssuer {
            name: "First Issuer".to_string(),
            issuer_type: "CORPORATION".to_string(),
            legal_jurisdiction: "US".to_string(),
            parent_id: None,
        },
    )
    .await
    .expect("create issuer");
    let second = create_issuer(
        &pool,
        NewIssuer {
            name: "Second Issuer".to_string(),
            issuer_type: "CORPORATION".to_string(),
            legal_jurisdiction: "US".to_string(),
            parent_id: None,
        },
    )
    .await
    .expect("create issuer");

    create_issuer_identifier(
        &pool,
        NewIssuerIdentifier {
            issuer_id: IssuerId::from_uuid(first.id),
            scheme: "LEI".to_string(),
            value: "5493001KJTIIGC8Y1R12".to_string(),
        },
    )
    .await
    .expect("first identifier");

    let error = create_issuer_identifier(
        &pool,
        NewIssuerIdentifier {
            issuer_id: IssuerId::from_uuid(second.id),
            scheme: "LEI".to_string(),
            value: "5493001KJTIIGC8Y1R12".to_string(),
        },
    )
    .await
    .expect_err("err");
    assert!(
        matches!(error, DomainError::InvariantViolation { .. }),
        "got: {error:?}"
    );

    guard.cleanup().await;
}

/// An empty issuer-identifier pair resolves to `Ok(None)` (FR-027).
#[tokio::test]
async fn issuer_identifier_lookup_with_an_empty_pair_resolves_to_none() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    assert!(
        find_issuer_identifier(&pool, "", "5493001KJTIIGC8Y1R12")
            .await
            .expect("lookup")
            .is_none()
    );
    assert!(
        find_issuer_by_identifier(&pool, "LEI", "")
            .await
            .expect("lookup")
            .is_none()
    );

    guard.cleanup().await;
}

// ---------------------------------------------------------------------------
// Portfolio: not-found + cannot-delete
// ---------------------------------------------------------------------------

/// Deleting a portfolio that does not exist reports `NotFound` (FR-019).
#[tokio::test]
async fn delete_portfolio_reports_not_found() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let error = delete_portfolio(&pool, PortfolioId::from_uuid(missing_uuid()))
        .await
        .expect_err("err");
    assert!(
        matches!(
            error,
            DomainError::NotFound {
                entity: "Portfolio",
                ..
            }
        ),
        "got: {error:?}"
    );

    guard.cleanup().await;
}

/// Deleting a portfolio that carries events is rejected — events are
/// immutable history (FR-019).
#[tokio::test]
async fn delete_portfolio_rejects_a_portfolio_with_events() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let portfolio = create_portfolio(
        &pool,
        NewPortfolio {
            name: "Eventful Portfolio".to_string(),
            purpose: "TEST".to_string(),
            created_by: "tester".to_string(),
            parent_id: None,
        },
    )
    .await
    .expect("create portfolio");
    let asset = create_asset(
        &pool,
        NewAsset {
            asset_type: "EQUITY".to_string(),
            display_name: "Event Asset".to_string(),
            currency: "USD".to_string(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
    )
    .await
    .expect("create asset");

    sqlx::query(
        "INSERT INTO events \
         (id, event_type, effective_date, quantity_delta, portfolio_id, asset_id) \
         VALUES (gen_random_uuid(), 'buy', CURRENT_DATE, 10, $1, $2)",
    )
    .bind(portfolio.id)
    .bind(asset.id)
    .execute(&pool)
    .await
    .expect("insert event");

    let error = delete_portfolio(&pool, PortfolioId::from_uuid(portfolio.id))
        .await
        .expect_err("err");
    assert!(
        matches!(
            error,
            DomainError::CannotDelete {
                entity: "Portfolio",
                ..
            }
        ),
        "got: {error:?}"
    );
    assert!(error.to_string().contains("events"), "got: {error}");

    guard.cleanup().await;
}

/// Deleting a portfolio that carries positions is rejected (FR-019).
#[tokio::test]
async fn delete_portfolio_rejects_a_portfolio_with_positions() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let portfolio = create_portfolio(
        &pool,
        NewPortfolio {
            name: "Positioned Portfolio".to_string(),
            purpose: "TEST".to_string(),
            created_by: "tester".to_string(),
            parent_id: None,
        },
    )
    .await
    .expect("create portfolio");
    let asset = create_asset(
        &pool,
        NewAsset {
            asset_type: "EQUITY".to_string(),
            display_name: "Position Asset".to_string(),
            currency: "USD".to_string(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
    )
    .await
    .expect("create asset");

    sqlx::query(
        "INSERT INTO positions (portfolio_id, asset_id, quantity, as_of) \
         VALUES ($1, $2, 100, NOW())",
    )
    .bind(portfolio.id)
    .bind(asset.id)
    .execute(&pool)
    .await
    .expect("insert position");

    let error = delete_portfolio(&pool, PortfolioId::from_uuid(portfolio.id))
        .await
        .expect_err("err");
    assert!(
        matches!(
            error,
            DomainError::CannotDelete {
                entity: "Portfolio",
                ..
            }
        ),
        "got: {error:?}"
    );
    assert!(error.to_string().contains("positions"), "got: {error}");

    guard.cleanup().await;
}

/// Patching a missing portfolio reports `NotFound` for every updatable
/// field (FR-017).
#[tokio::test]
async fn update_portfolio_reports_not_found_for_every_field() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let missing = PortfolioId::from_uuid(missing_uuid());

    let error = update_portfolio(
        &pool,
        missing,
        PortfolioPatch {
            name: Some("Nope".to_string()),
            purpose: None,
        },
    )
    .await
    .expect_err("err");
    assert!(
        matches!(
            error,
            DomainError::NotFound {
                entity: "Portfolio",
                ..
            }
        ),
        "got: {error:?}"
    );

    let error = update_portfolio(
        &pool,
        missing,
        PortfolioPatch {
            name: None,
            purpose: Some("OTHER".to_string()),
        },
    )
    .await
    .expect_err("err");
    assert!(
        matches!(
            error,
            DomainError::NotFound {
                entity: "Portfolio",
                ..
            }
        ),
        "got: {error:?}"
    );

    guard.cleanup().await;
}

// ---------------------------------------------------------------------------
// Events: validation paths
// ---------------------------------------------------------------------------

/// A transfer whose destination is the source portfolio is rejected
/// (FR-008).
#[tokio::test]
async fn create_event_rejects_a_transfer_to_the_same_portfolio() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let portfolio = create_portfolio(
        &pool,
        NewPortfolio {
            name: "Transfer Portfolio".to_string(),
            purpose: "TEST".to_string(),
            created_by: "tester".to_string(),
            parent_id: None,
        },
    )
    .await
    .expect("create portfolio");
    let asset = create_asset(
        &pool,
        NewAsset {
            asset_type: "EQUITY".to_string(),
            display_name: "Transfer Asset".to_string(),
            currency: "USD".to_string(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
    )
    .await
    .expect("create asset");

    let portfolio_id = PortfolioId::from_uuid(portfolio.id);
    let error = create_event(
        &pool,
        NewEvent {
            event_type: EventType::Transfer,
            effective_date: NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"),
            quantity_delta: Decimal::new(10, 0),
            portfolio_id,
            asset_id: AssetId::from_uuid(asset.id),
            to_portfolio_id: Some(portfolio_id),
        },
    )
    .await
    .expect_err("err");
    assert!(
        matches!(error, DomainError::Validation(ref msg) if msg.contains("must differ from source")),
        "got: {error:?}"
    );

    guard.cleanup().await;
}

/// Every event type has the wire label the `event_type` column stores
/// (FR-008).
#[test]
fn event_type_labels_cover_every_variant() {
    assert_eq!(EventType::Buy.as_str(), "buy");
    assert_eq!(EventType::Sell.as_str(), "sell");
    assert_eq!(EventType::Transfer.as_str(), "transfer");
    assert_eq!(EventType::Split.as_str(), "split");
    assert_eq!(EventType::Merger.as_str(), "merger");
    assert_eq!(EventType::SpinOff.as_str(), "spin-off");
    assert_eq!(EventType::Dividend.as_str(), "dividend");
    assert_eq!(
        EventType::StockBasedCompensation.as_str(),
        "stock-based-compensation"
    );
}

/// Asset search matches identifiers and escapes user wildcards.
#[tokio::test]
async fn asset_search_matches_identifiers_and_escapes_wildcards() {
    let Some((pool, _guard)) = tx_pool().await else {
        return;
    };

    let asset = create_asset(
        &pool,
        NewAsset {
            asset_type: "EQUITY".to_string(),
            display_name: "Searchable Widget".to_string(),
            currency: "USD".to_string(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
    )
    .await
    .expect("create asset");
    create_asset_identifier(
        &pool,
        NewAssetIdentifier {
            asset_id: asset.id(),
            scheme: "TICKER".to_string(),
            value: "SRCH".to_string(),
        },
    )
    .await
    .expect("create identifier");

    let by_ticker = search_asset_options(
        &pool,
        AssetSearchQuery {
            q: Some("srch".to_string()),
            limit: 10,
        },
    )
    .await
    .expect("search by ticker");
    assert!(by_ticker.iter().any(|option| option.id == asset.id));
    assert_eq!(
        by_ticker
            .iter()
            .find(|option| option.id == asset.id)
            .and_then(|option| option.primary_identifier.clone()),
        Some("SRCH".to_string())
    );

    let by_name = search_asset_options(
        &pool,
        AssetSearchQuery {
            q: Some("searchable".to_string()),
            limit: 10,
        },
    )
    .await
    .expect("search by name");
    assert!(by_name.iter().any(|option| option.id == asset.id));

    let wildcard = search_asset_options(
        &pool,
        AssetSearchQuery {
            q: Some("%".to_string()),
            limit: 10,
        },
    )
    .await
    .expect("escaped wildcard");
    assert!(wildcard.iter().all(|option| option.id != asset.id));
}
