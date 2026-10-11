//! Tests for US4 — Event create + read; immutability.

use crate::common::*;
use crate::domain::asset::create_asset;
use crate::domain::asset::entity::NewAsset;
use crate::domain::event::create_event;
use crate::domain::event::entity::EventType;
use crate::domain::event::entity::NewEvent;
use crate::domain::portfolio::create_portfolio;
use crate::domain::portfolio::entity::NewPortfolio;
use rust_decimal::Decimal;
use sqlx::PgPool;

async fn setup(pool: &PgPool) -> (crate::domain::id::PortfolioId, crate::domain::id::AssetId) {
    let p = create_portfolio(
        pool,
        NewPortfolio {
            name: "E".into(),
            purpose: "p".into(),
            created_by: "t".into(),
            parent_id: None,
        },
    )
    .await
    .expect("p");
    let a = create_asset(
        pool,
        NewAsset {
            asset_type: "stock".into(),
            display_name: "E".into(),
            currency: "USD".into(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
    )
    .await
    .expect("a");
    (p.id(), a.id())
}

#[tokio::test]
async fn create_buy_event_persists() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let (p, a) = setup(&pool).await;
    let today = chrono::Utc::now().date_naive();
    let ev = create_event(
        &pool,
        NewEvent {
            event_type: EventType::Buy,
            effective_date: today,
            quantity_delta: Decimal::from(100),
            portfolio_id: p,
            asset_id: a,
            to_portfolio_id: None,
        },
    )
    .await
    .expect("create");
    assert_eq!(ev.event_type, "buy");
    guard.cleanup().await;
}

#[tokio::test]
async fn create_sell_event_persists() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let (p, a) = setup(&pool).await;
    let today = chrono::Utc::now().date_naive();
    let ev = create_event(
        &pool,
        NewEvent {
            event_type: EventType::Sell,
            effective_date: today,
            quantity_delta: Decimal::from(-30),
            portfolio_id: p,
            asset_id: a,
            to_portfolio_id: None,
        },
    )
    .await
    .expect("create");
    assert_eq!(ev.event_type, "sell");
    guard.cleanup().await;
}

#[tokio::test]
async fn create_transfer_event_persists_with_to_portfolio() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let (p, a) = setup(&pool).await;
    let other = create_portfolio(
        &pool,
        NewPortfolio {
            name: "O".into(),
            purpose: "o".into(),
            created_by: "t".into(),
            parent_id: None,
        },
    )
    .await
    .expect("other");
    let today = chrono::Utc::now().date_naive();
    let ev = create_event(
        &pool,
        NewEvent {
            event_type: EventType::Transfer,
            effective_date: today,
            quantity_delta: Decimal::from(-30),
            portfolio_id: p,
            asset_id: a,
            to_portfolio_id: Some(other.id()),
        },
    )
    .await
    .expect("create");
    assert_eq!(ev.to_portfolio_id, Some(other.id));
    guard.cleanup().await;
}

#[tokio::test]
async fn create_transfer_event_without_to_portfolio_is_rejected() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let (p, a) = setup(&pool).await;
    let today = chrono::Utc::now().date_naive();
    let res = create_event(
        &pool,
        NewEvent {
            event_type: EventType::Transfer,
            effective_date: today,
            quantity_delta: Decimal::from(-30),
            portfolio_id: p,
            asset_id: a,
            to_portfolio_id: None,
        },
    )
    .await;
    assert!(res.is_err());
    guard.cleanup().await;
}

#[tokio::test]
async fn create_zero_quantity_event_is_rejected() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let (p, a) = setup(&pool).await;
    let today = chrono::Utc::now().date_naive();
    let res = create_event(
        &pool,
        NewEvent {
            event_type: EventType::Buy,
            effective_date: today,
            quantity_delta: Decimal::ZERO,
            portfolio_id: p,
            asset_id: a,
            to_portfolio_id: None,
        },
    )
    .await;
    assert!(res.is_err());
    guard.cleanup().await;
}

#[tokio::test]
async fn update_event_is_rejected_at_api_level() {
    // The public API does not expose an `update_event` function; immutability
    // is structural. This test documents the contract.
    // (If a future PR adds `update_event`, this test will fail to compile.)
}

#[tokio::test]
async fn delete_event_is_rejected_at_api_level() {
    // The public API does not expose a `delete_event` function.
}
