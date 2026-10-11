//! Tests for US4 — Position replay and read integration.

use crate::common::*;
use crate::domain::asset::create_asset;
use crate::domain::asset::entity::NewAsset;
use crate::domain::event::create_event;
use crate::domain::event::entity::{EventType, NewEvent};
use crate::domain::model_description::model_description;
use crate::domain::portfolio::create_portfolio;
use crate::domain::portfolio::entity::NewPortfolio;
use crate::domain::position::get_position;
use rust_decimal::Decimal;

#[tokio::test]
async fn get_position_returns_materialized_position_after_event() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let p = create_portfolio(
        &pool,
        NewPortfolio {
            name: "P".into(),
            purpose: "p".into(),
            created_by: "t".into(),
            parent_id: None,
        },
    )
    .await
    .expect("p");
    let a = create_asset(
        &pool,
        NewAsset {
            asset_type: "stock".into(),
            display_name: "A".into(),
            currency: "USD".into(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
    )
    .await
    .expect("a");
    let today = chrono::Utc::now().date_naive();
    create_event(
        &pool,
        NewEvent {
            event_type: EventType::Buy,
            effective_date: today,
            quantity_delta: Decimal::from(100),
            portfolio_id: p.id(),
            asset_id: a.id(),
            to_portfolio_id: None,
        },
    )
    .await
    .expect("buy");
    let pos = get_position(&pool, p.id(), a.id())
        .await
        .expect("get position");
    assert_eq!(pos.quantity, Decimal::from(100));
    guard.cleanup().await;
}

#[tokio::test]
async fn model_description_includes_event_and_position() {
    let desc = model_description();
    assert!(desc.entities.iter().any(|e| e.name == "Event"));
    assert!(desc.entities.iter().any(|e| e.name == "Position"));
    assert!(desc.relationships.iter().any(|r| r.label == "derived_from"));
    assert!(
        desc.relationships
            .iter()
            .any(|r| r.label == "is_quantity_of")
    );
}
