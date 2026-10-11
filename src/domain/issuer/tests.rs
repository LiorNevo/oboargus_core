//! Tests for US2 — Issuer CRUD.

use crate::domain::error::DomainError;
use crate::domain::issuer::entity::{IssuerPatch, NewIssuer};
use crate::domain::issuer::{create_issuer, delete_issuer, get_issuer, update_issuer};
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

#[tokio::test]
async fn create_issuer_persists_attributes() {
    let Some(pool) = pool().await else {
        return;
    };
    let issuer = create_issuer(
        &pool,
        NewIssuer {
            name: "Test Co".into(),
            issuer_type: "corporation".into(),
            legal_jurisdiction: "US".into(),
        },
    )
    .await
    .expect("create");
    let read = get_issuer(&pool, issuer.id()).await.expect("read");
    assert_eq!(read.name, "Test Co");
}

#[tokio::test]
async fn update_issuer_non_key_fields() {
    let Some(pool) = pool().await else {
        return;
    };
    let issuer = create_issuer(
        &pool,
        NewIssuer {
            name: "Before".into(),
            issuer_type: "t".into(),
            legal_jurisdiction: "US".into(),
        },
    )
    .await
    .expect("create");
    let updated = update_issuer(
        &pool,
        issuer.id(),
        IssuerPatch {
            name: Some("After".into()),
            ..Default::default()
        },
    )
    .await
    .expect("update");
    assert_eq!(updated.name, "After");
}

#[tokio::test]
async fn delete_issuer_with_no_dependents_succeeds() {
    let Some(pool) = pool().await else {
        return;
    };
    let issuer = create_issuer(
        &pool,
        NewIssuer {
            name: "To delete".into(),
            issuer_type: "t".into(),
            legal_jurisdiction: "US".into(),
        },
    )
    .await
    .expect("create");
    delete_issuer(&pool, issuer.id()).await.expect("delete");
    let res = get_issuer(&pool, issuer.id()).await;
    assert!(matches!(
        res,
        Err(DomainError::NotFound {
            entity: "Issuer",
            ..
        })
    ));
}

#[tokio::test]
async fn delete_issuer_with_assets_is_rejected() {
    let Some(pool) = pool().await else {
        return;
    };
    let issuer = create_issuer(
        &pool,
        NewIssuer {
            name: "With Asset".into(),
            issuer_type: "t".into(),
            legal_jurisdiction: "US".into(),
        },
    )
    .await
    .expect("create");
    sqlx::query("INSERT INTO assets (id, asset_type, display_name, currency, issuer_id) VALUES ($1, $2, $3, $4, $5)")
        .bind(uuid::Uuid::new_v4())
        .bind("stock")
        .bind("X")
        .bind("USD")
        .bind(issuer.id)
        .execute(&pool).await.expect("insert asset");
    let res = delete_issuer(&pool, issuer.id()).await;
    assert!(matches!(
        res,
        Err(DomainError::CannotDelete {
            entity: "Issuer",
            ..
        })
    ));
}
