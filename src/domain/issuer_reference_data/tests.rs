//! Tests for US3 — IssuerReferenceData dual-temporal semantics.
//! Gated by `DATABASE_URL`.

use crate::domain::issuer::create_issuer;
use crate::domain::issuer::entity::NewIssuer;
use crate::domain::issuer_reference_data::entity::NewIssuerReferenceData;
use crate::domain::issuer_reference_data::{
    create_issuer_reference_data, get_current_issuer_reference_data,
    get_issuer_reference_data_history, update_issuer_reference_value,
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

async fn new_issuer(pool: &PgPool) -> crate::domain::id::IssuerId {
    let issuer = create_issuer(
        pool,
        NewIssuer {
            name: "For Ref".into(),
            issuer_type: "test".into(),
            legal_jurisdiction: "US".into(),
        },
    )
    .await
    .expect("create");
    issuer.id()
}

#[tokio::test]
async fn create_issuer_reference_data_persists_record() {
    let Some(pool) = pool().await else {
        return;
    };
    let issuer_id = new_issuer(&pool).await;
    let today = Utc::now().date_naive();
    let rec = create_issuer_reference_data(
        &pool,
        NewIssuerReferenceData {
            issuer_id,
            fact_type: "name".into(),
            as_of_date: today,
            knowledge_date: today,
            value: "X".into(),
            source: "s".into(),
        },
    )
    .await
    .expect("create");
    assert_eq!(rec.value, "X");
}

#[tokio::test]
async fn get_current_issuer_reference_data_returns_latest_knowledge() {
    let Some(pool) = pool().await else {
        return;
    };
    let issuer_id = new_issuer(&pool).await;
    let today = Utc::now().date_naive();
    let k1 = today - chrono::Duration::days(2);
    let k2 = today - chrono::Duration::days(1);
    create_issuer_reference_data(
        &pool,
        NewIssuerReferenceData {
            issuer_id,
            fact_type: "x".into(),
            as_of_date: today,
            knowledge_date: k1,
            value: "old".into(),
            source: "s".into(),
        },
    )
    .await
    .expect("k1");
    create_issuer_reference_data(
        &pool,
        NewIssuerReferenceData {
            issuer_id,
            fact_type: "x".into(),
            as_of_date: today,
            knowledge_date: k2,
            value: "new".into(),
            source: "s".into(),
        },
    )
    .await
    .expect("k2");
    let current = get_current_issuer_reference_data(&pool, issuer_id, "x", today)
        .await
        .expect("current")
        .expect("present");
    assert_eq!(current.value, "new");
}

#[tokio::test]
async fn get_issuer_reference_data_history_returns_all_records_ordered() {
    let Some(pool) = pool().await else {
        return;
    };
    let issuer_id = new_issuer(&pool).await;
    let today = Utc::now().date_naive();
    let k1 = today - chrono::Duration::days(2);
    let k2 = today - chrono::Duration::days(1);
    create_issuer_reference_data(
        &pool,
        NewIssuerReferenceData {
            issuer_id,
            fact_type: "x".into(),
            as_of_date: today,
            knowledge_date: k1,
            value: "a".into(),
            source: "s".into(),
        },
    )
    .await
    .expect("k1");
    create_issuer_reference_data(
        &pool,
        NewIssuerReferenceData {
            issuer_id,
            fact_type: "x".into(),
            as_of_date: today,
            knowledge_date: k2,
            value: "b".into(),
            source: "s".into(),
        },
    )
    .await
    .expect("k2");
    let history = get_issuer_reference_data_history(&pool, issuer_id, "x", today)
        .await
        .expect("history");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].knowledge_date, k2);
    assert_eq!(history[1].knowledge_date, k1);
}

#[tokio::test]
async fn update_issuer_reference_value_writes_new_record() {
    let Some(pool) = pool().await else {
        return;
    };
    let issuer_id = new_issuer(&pool).await;
    let today = Utc::now().date_naive();
    let k1 = today - chrono::Duration::days(1);
    create_issuer_reference_data(
        &pool,
        NewIssuerReferenceData {
            issuer_id,
            fact_type: "x".into(),
            as_of_date: today,
            knowledge_date: k1,
            value: "old".into(),
            source: "s".into(),
        },
    )
    .await
    .expect("initial");
    let updated =
        update_issuer_reference_value(&pool, issuer_id, "x", today, "new".into(), "s".into())
            .await
            .expect("update");
    assert_eq!(updated.value, "new");
    let current = get_current_issuer_reference_data(&pool, issuer_id, "x", today)
        .await
        .expect("current")
        .expect("present");
    assert_eq!(current.value, "new");
}
