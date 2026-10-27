//! Tests for US3 — AssetReferenceData dual-temporal semantics.
//!
//! Implements FR-008. Gated by `DATABASE_URL`.

use crate::common::*;
use crate::domain::asset::create_asset;
use crate::domain::asset::entity::NewAsset;
use crate::domain::asset_reference_data::entity::NewAssetReferenceData;
use crate::domain::asset_reference_data::{
    create_asset_reference_data, get_asset_reference_data_history,
    get_current_asset_reference_data, update_asset_reference_value,
};
use chrono::Utc;
use sqlx::PgPool;

async fn new_asset(pool: &PgPool) -> crate::domain::id::AssetId {
    let asset = create_asset(
        pool,
        NewAsset {
            asset_type: "stock".into(),
            display_name: "For Ref".into(),
            currency: "USD".into(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
    )
    .await
    .expect("create asset");
    asset.id()
}

#[tokio::test]
async fn create_asset_reference_data_persists_record() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset_id = new_asset(&pool).await;
    let today = Utc::now().date_naive();
    let rec = create_asset_reference_data(
        &pool,
        NewAssetReferenceData {
            asset_id,
            fact_type: "market_price".into(),
            as_of_date: today,
            knowledge_date: today,
            value: "100.00".into(),
            source: "manual".into(),
        },
    )
    .await
    .expect("create");
    assert_eq!(rec.value, "100.00");
    guard.cleanup().await;
}

#[tokio::test]
async fn get_current_asset_reference_data_returns_latest_knowledge() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset_id = new_asset(&pool).await;
    let today = Utc::now().date_naive();
    let k1 = today - chrono::Duration::days(2);
    let k2 = today - chrono::Duration::days(1);
    create_asset_reference_data(
        &pool,
        NewAssetReferenceData {
            asset_id,
            fact_type: "price".into(),
            as_of_date: today,
            knowledge_date: k1,
            value: "10".into(),
            source: "a".into(),
        },
    )
    .await
    .expect("k1");
    create_asset_reference_data(
        &pool,
        NewAssetReferenceData {
            asset_id,
            fact_type: "price".into(),
            as_of_date: today,
            knowledge_date: k2,
            value: "20".into(),
            source: "a".into(),
        },
    )
    .await
    .expect("k2");
    let current = get_current_asset_reference_data(&pool, asset_id, "price", today)
        .await
        .expect("current")
        .expect("present");
    assert_eq!(current.value, "20");
    assert_eq!(current.knowledge_date, k2);
    guard.cleanup().await;
}

#[tokio::test]
async fn get_asset_reference_data_history_returns_all_records_ordered() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset_id = new_asset(&pool).await;
    let today = Utc::now().date_naive();
    let k1 = today - chrono::Duration::days(2);
    let k2 = today - chrono::Duration::days(1);
    create_asset_reference_data(
        &pool,
        NewAssetReferenceData {
            asset_id,
            fact_type: "x".into(),
            as_of_date: today,
            knowledge_date: k1,
            value: "a".into(),
            source: "s".into(),
        },
    )
    .await
    .expect("k1");
    create_asset_reference_data(
        &pool,
        NewAssetReferenceData {
            asset_id,
            fact_type: "x".into(),
            as_of_date: today,
            knowledge_date: k2,
            value: "b".into(),
            source: "s".into(),
        },
    )
    .await
    .expect("k2");
    let history = get_asset_reference_data_history(&pool, asset_id, "x", today)
        .await
        .expect("history");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].knowledge_date, k2);
    assert_eq!(history[1].knowledge_date, k1);
    guard.cleanup().await;
}

#[tokio::test]
async fn update_asset_reference_value_writes_new_record() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset_id = new_asset(&pool).await;
    let today = Utc::now().date_naive();
    let k1 = today - chrono::Duration::days(1);
    create_asset_reference_data(
        &pool,
        NewAssetReferenceData {
            asset_id,
            fact_type: "price".into(),
            as_of_date: today,
            knowledge_date: k1,
            value: "old".into(),
            source: "s".into(),
        },
    )
    .await
    .expect("initial");
    let updated =
        update_asset_reference_value(&pool, asset_id, "price", today, "new".into(), "s".into())
            .await
            .expect("update");
    assert_eq!(updated.value, "new");
    let history = get_asset_reference_data_history(&pool, asset_id, "price", today)
        .await
        .expect("history");
    assert_eq!(history.len(), 2);
    let current = get_current_asset_reference_data(&pool, asset_id, "price", today)
        .await
        .expect("current")
        .expect("present");
    assert_eq!(current.value, "new");
    guard.cleanup().await;
}

#[test]
fn entity_accessors_return_wrapped_id_types() {
    use crate::domain::asset_reference_data::entity::AssetReferenceData;
    use crate::domain::id::DomainId;
    let id = uuid::Uuid::new_v4();
    let asset_id = uuid::Uuid::new_v4();
    let today = Utc::now().date_naive();
    let row = AssetReferenceData {
        id,
        asset_id,
        fact_type: "price".into(),
        as_of_date: today,
        knowledge_date: today,
        value: "v".into(),
        source: "s".into(),
    };
    assert_eq!(row.id().as_uuid(), id);
    assert_eq!(row.asset_id().as_uuid(), asset_id);
}

#[tokio::test]
async fn two_same_day_updates_both_succeed_and_both_are_retrievable() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset_id = new_asset(&pool).await;
    let today = Utc::now().date_naive();
    create_asset_reference_data(
        &pool,
        NewAssetReferenceData {
            asset_id,
            fact_type: "price".into(),
            as_of_date: today,
            knowledge_date: today,
            value: "first".into(),
            source: "s".into(),
        },
    )
    .await
    .expect("initial");

    // The pre-021 model keyed knowledge on a DATE, so the second call would
    // have collided with the first. Both must now be stored.
    update_asset_reference_value(&pool, asset_id, "price", today, "second".into(), "s".into())
        .await
        .expect("first same-day correction");
    update_asset_reference_value(&pool, asset_id, "price", today, "third".into(), "s".into())
        .await
        .expect("second same-day correction");

    let history = get_asset_reference_data_history(&pool, asset_id, "price", today)
        .await
        .expect("history");
    assert_eq!(history.len(), 3, "same-day corrections must all be stored");
    assert_eq!(history[0].value, "third");
    assert_eq!(history[1].value, "second");
    assert_eq!(history[2].value, "first");

    let current = get_current_asset_reference_data(&pool, asset_id, "price", today)
        .await
        .expect("current")
        .expect("present");
    assert_eq!(current.value, "third");
    guard.cleanup().await;
}
