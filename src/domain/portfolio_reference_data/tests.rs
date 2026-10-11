//! Tests for US3 — PortfolioReferenceData dual-temporal semantics.
//! Gated by `DATABASE_URL`.

use crate::domain::portfolio::create_portfolio;
use crate::domain::portfolio::entity::NewPortfolio;
use crate::domain::portfolio_reference_data::entity::NewPortfolioReferenceData;
use crate::domain::portfolio_reference_data::{
    create_portfolio_reference_data, get_current_portfolio_reference_data,
    get_portfolio_reference_data_history, update_portfolio_reference_value,
};
use chrono::Utc;
use sqlx::PgPool;

async fn pool() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPool::connect(&url).await.expect("connect");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("migrations should succeed");
    Some(pool)
}

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
    let Some(pool) = pool().await else {
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
}

#[tokio::test]
async fn get_current_portfolio_reference_data_returns_latest_knowledge() {
    let Some(pool) = pool().await else {
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
}

#[tokio::test]
async fn get_portfolio_reference_data_history_returns_all_records_ordered() {
    let Some(pool) = pool().await else {
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
}

#[tokio::test]
async fn update_portfolio_reference_value_writes_new_record() {
    let Some(pool) = pool().await else {
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
}
