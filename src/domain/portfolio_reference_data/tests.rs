//! Tests for US3 — PortfolioReferenceData dual-temporal semantics.
//! Gated by `DATABASE_URL`.

use crate::common::*;
use crate::domain::portfolio::create_portfolio;
use crate::domain::portfolio::entity::NewPortfolio;
use crate::domain::portfolio_reference_data::entity::NewPortfolioReferenceData;
use crate::domain::portfolio_reference_data::{
    create_portfolio_reference_data, get_current_portfolio_reference_data,
    get_portfolio_reference_data_history, update_portfolio_reference_value,
};
use chrono::Utc;
use sqlx::PgPool;

async fn new_portfolio(pool: &PgPool) -> crate::domain::id::PortfolioId {
    let p = create_portfolio(
        pool,
        NewPortfolio {
            name: "For Ref".into(),
            purpose: "p".into(),
            created_by: "t".into(),
            parent_id: None,
        },
    )
    .await
    .expect("create");
    p.id()
}

#[tokio::test]
async fn create_portfolio_reference_data_persists_record() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let portfolio_id = new_portfolio(&pool).await;
    let today = Utc::now().date_naive();
    let rec = create_portfolio_reference_data(
        &pool,
        NewPortfolioReferenceData {
            portfolio_id,
            fact_type: "benchmark".into(),
            as_of_date: today,
            knowledge_date: today,
            value: "SPY".into(),
            source: "s".into(),
        },
    )
    .await
    .expect("create");
    assert_eq!(rec.value, "SPY");
    guard.cleanup().await;
}

#[tokio::test]
async fn get_current_portfolio_reference_data_returns_latest_knowledge() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let portfolio_id = new_portfolio(&pool).await;
    let today = Utc::now().date_naive();
    let k1 = today - chrono::Duration::days(2);
    let k2 = today - chrono::Duration::days(1);
    create_portfolio_reference_data(
        &pool,
        NewPortfolioReferenceData {
            portfolio_id,
            fact_type: "x".into(),
            as_of_date: today,
            knowledge_date: k1,
            value: "a".into(),
            source: "s".into(),
        },
    )
    .await
    .expect("k1");
    create_portfolio_reference_data(
        &pool,
        NewPortfolioReferenceData {
            portfolio_id,
            fact_type: "x".into(),
            as_of_date: today,
            knowledge_date: k2,
            value: "b".into(),
            source: "s".into(),
        },
    )
    .await
    .expect("k2");
    let current = get_current_portfolio_reference_data(&pool, portfolio_id, "x", today)
        .await
        .expect("current")
        .expect("present");
    assert_eq!(current.value, "b");
    guard.cleanup().await;
}

#[tokio::test]
async fn get_portfolio_reference_data_history_returns_all_records_ordered() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let portfolio_id = new_portfolio(&pool).await;
    let today = Utc::now().date_naive();
    let k1 = today - chrono::Duration::days(2);
    let k2 = today - chrono::Duration::days(1);
    create_portfolio_reference_data(
        &pool,
        NewPortfolioReferenceData {
            portfolio_id,
            fact_type: "x".into(),
            as_of_date: today,
            knowledge_date: k1,
            value: "a".into(),
            source: "s".into(),
        },
    )
    .await
    .expect("k1");
    create_portfolio_reference_data(
        &pool,
        NewPortfolioReferenceData {
            portfolio_id,
            fact_type: "x".into(),
            as_of_date: today,
            knowledge_date: k2,
            value: "b".into(),
            source: "s".into(),
        },
    )
    .await
    .expect("k2");
    let history = get_portfolio_reference_data_history(&pool, portfolio_id, "x", today)
        .await
        .expect("history");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].knowledge_date, k2);
    guard.cleanup().await;
}

#[tokio::test]
async fn update_portfolio_reference_value_writes_new_record() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let portfolio_id = new_portfolio(&pool).await;
    let today = Utc::now().date_naive();
    let k1 = today - chrono::Duration::days(1);
    create_portfolio_reference_data(
        &pool,
        NewPortfolioReferenceData {
            portfolio_id,
            fact_type: "x".into(),
            as_of_date: today,
            knowledge_date: k1,
            value: "old".into(),
            source: "s".into(),
        },
    )
    .await
    .expect("initial");
    let _ =
        update_portfolio_reference_value(&pool, portfolio_id, "x", today, "new".into(), "s".into())
            .await
            .expect("update");
    let current = get_current_portfolio_reference_data(&pool, portfolio_id, "x", today)
        .await
        .expect("current")
        .expect("present");
    assert_eq!(current.value, "new");
    guard.cleanup().await;
}

#[test]
fn entity_accessors_return_wrapped_id_types() {
    use crate::domain::id::DomainId;
    use crate::domain::portfolio_reference_data::entity::PortfolioReferenceData;
    let id = uuid::Uuid::new_v4();
    let portfolio_id = uuid::Uuid::new_v4();
    let today = Utc::now().date_naive();
    let row = PortfolioReferenceData {
        id,
        portfolio_id,
        fact_type: "nav".into(),
        as_of_date: today,
        knowledge_date: today,
        value: "v".into(),
        source: "s".into(),
    };
    assert_eq!(row.id().as_uuid(), id);
    assert_eq!(row.portfolio_id().as_uuid(), portfolio_id);
}

#[tokio::test]
async fn two_same_day_updates_both_succeed_and_both_are_retrievable() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let portfolio_id = new_portfolio(&pool).await;
    let today = Utc::now().date_naive();
    create_portfolio_reference_data(
        &pool,
        NewPortfolioReferenceData {
            portfolio_id,
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
    update_portfolio_reference_value(
        &pool,
        portfolio_id,
        "price",
        today,
        "second".into(),
        "s".into(),
    )
    .await
    .expect("first same-day correction");
    update_portfolio_reference_value(
        &pool,
        portfolio_id,
        "price",
        today,
        "third".into(),
        "s".into(),
    )
    .await
    .expect("second same-day correction");

    let history = get_portfolio_reference_data_history(&pool, portfolio_id, "price", today)
        .await
        .expect("history");
    assert_eq!(history.len(), 3, "same-day corrections must all be stored");
    assert_eq!(history[0].value, "third");
    assert_eq!(history[1].value, "second");
    assert_eq!(history[2].value, "first");

    let current = get_current_portfolio_reference_data(&pool, portfolio_id, "price", today)
        .await
        .expect("current")
        .expect("present");
    assert_eq!(current.value, "third");
    guard.cleanup().await;
}
