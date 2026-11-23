//! Concurrency tests for the atomic event/position write path (FR-028/029).
//!
//! Database-gated through the shared `tx_pool` fixture.

use chrono::Utc;
use obolargus_core::common::tx_pool;
use obolargus_core::domain::asset::create_asset;
use obolargus_core::domain::asset::entity::NewAsset;
use obolargus_core::domain::event::create::create_event;
use obolargus_core::domain::event::entity::{EventType, NewEvent};
use obolargus_core::domain::id::{AssetId, PortfolioId};
use obolargus_core::domain::portfolio::create_portfolio;
use obolargus_core::domain::portfolio::entity::NewPortfolio;
use obolargus_core::domain::position::get_position;
use rust_decimal::Decimal;
use sqlx::PgPool;

async fn portfolio(pool: &PgPool, name: &str) -> PortfolioId {
    create_portfolio(
        pool,
        NewPortfolio {
            name: name.to_string(),
            purpose: "concurrency-test".to_string(),
            created_by: "tester".to_string(),
            parent_id: None,
        },
    )
    .await
    .expect("create portfolio")
    .id()
}

async fn asset(pool: &PgPool) -> AssetId {
    create_asset(
        pool,
        NewAsset {
            asset_type: "stock".to_string(),
            display_name: format!("Concurrency {}", uuid::Uuid::new_v4()),
            currency: "USD".to_string(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
    )
    .await
    .expect("create asset")
    .id()
}

fn buy(portfolio_id: PortfolioId, asset_id: AssetId, quantity: i64) -> NewEvent {
    NewEvent {
        event_type: EventType::Buy,
        effective_date: Utc::now().date_naive(),
        quantity_delta: Decimal::from(quantity),
        portfolio_id,
        asset_id,
        to_portfolio_id: None,
    }
}

#[tokio::test]
async fn concurrent_buys_sum_the_position() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let portfolio_id = portfolio(&pool, "Concurrent buys").await;
    let asset_id = asset(&pool).await;

    // 100 rounds of two simultaneous buys. Without the per-pair advisory
    // lock the incremental upserts lose one of the deltas.
    for _ in 0..100 {
        let (first, second) = tokio::join!(
            create_event(&pool, buy(portfolio_id, asset_id, 10)),
            create_event(&pool, buy(portfolio_id, asset_id, 10)),
        );
        first.expect("first buy");
        second.expect("second buy");
    }

    let position = get_position(&pool, portfolio_id, asset_id)
        .await
        .expect("position");
    assert_eq!(position.quantity, Decimal::from(2_000));

    let events: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE portfolio_id = $1 AND asset_id = $2")
            .bind(portfolio_id.0)
            .bind(asset_id.0)
            .fetch_one(&pool)
            .await
            .expect("count events");
    assert_eq!(events, 200, "every concurrent event is stored");

    guard.cleanup().await;
}

#[tokio::test]
async fn non_transfer_destination_is_rejected() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let portfolio_id = portfolio(&pool, "Invalid destination").await;
    let destination = portfolio(&pool, "Invalid destination target").await;
    let asset_id = asset(&pool).await;

    let mut input = buy(portfolio_id, asset_id, 10);
    input.to_portfolio_id = Some(destination);
    let error = create_event(&pool, input)
        .await
        .expect_err("a buy cannot carry a transfer destination");
    assert!(
        error.to_string().contains("only transfer events"),
        "unexpected error: {error}"
    );

    guard.cleanup().await;
}

#[tokio::test]
async fn concurrent_transfer_and_buy_preserve_both_sides() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let source = portfolio(&pool, "Transfer source").await;
    let destination = portfolio(&pool, "Transfer destination").await;
    let asset_id = asset(&pool).await;

    create_event(&pool, buy(source, asset_id, 100))
        .await
        .expect("seed buy");

    let transfer = NewEvent {
        event_type: EventType::Transfer,
        effective_date: Utc::now().date_naive(),
        quantity_delta: Decimal::from(-30),
        portfolio_id: source,
        asset_id,
        to_portfolio_id: Some(destination),
    };
    let (transferred, bought) = tokio::join!(
        create_event(&pool, transfer),
        create_event(&pool, buy(source, asset_id, 10)),
    );
    transferred.expect("transfer");
    bought.expect("buy");

    let source_position = get_position(&pool, source, asset_id)
        .await
        .expect("source position");
    let destination_position = get_position(&pool, destination, asset_id)
        .await
        .expect("destination position");
    assert_eq!(source_position.quantity, Decimal::from(80));
    assert_eq!(destination_position.quantity, Decimal::from(30));

    guard.cleanup().await;
}
