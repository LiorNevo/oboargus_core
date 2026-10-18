//! Tests for US1 — Portfolio CRUD and the DAG-acylclicity invariant.
//!
//! Each test acquires a `&PgPool` and a `PoolGuard` from the shared
//! `tests/common` fixture. The guard runs `TRUNCATE … CASCADE` on
//! every high-level table when the test future drops, so the host
//! database is left unchanged.
//!
//! Gated by `DATABASE_URL`: when unset, tests exit early as no-ops.

use crate::domain::error::DomainError;
use crate::domain::id::DomainId;
use crate::domain::model_description::model_description;
use crate::domain::portfolio::entity::{NewPortfolio, PortfolioPatch};
use crate::domain::portfolio::{
    create_portfolio, delete_portfolio, get_portfolio, list_ancestors, list_children,
    update_portfolio,
};

use crate::common::*;

#[tokio::test]
async fn create_portfolio_persists_attributes() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let p = create_portfolio(
        &pool,
        NewPortfolio {
            name: "Test".into(),
            purpose: "Testing".into(),
            created_by: "tester".into(),
            parent_id: None,
        },
    )
    .await
    .expect("create");
    let read = get_portfolio(&pool, p.id()).await.expect("read");
    assert_eq!(read.name, "Test");
    assert_eq!(read.purpose, "Testing");
    assert_eq!(read.created_by, "tester");
    guard.cleanup().await;
}

#[tokio::test]
async fn create_portfolio_with_parent_links_children() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let parent = create_portfolio(
        &pool,
        NewPortfolio {
            name: "P".into(),
            purpose: "p".into(),
            created_by: "t".into(),
            parent_id: None,
        },
    )
    .await
    .expect("create parent");
    let child = create_portfolio(
        &pool,
        NewPortfolio {
            name: "C".into(),
            purpose: "c".into(),
            created_by: "t".into(),
            parent_id: Some(parent.id()),
        },
    )
    .await
    .expect("create child");
    let children = list_children(&pool, parent.id())
        .await
        .expect("list children");
    assert!(children.iter().any(|c| c.id == child.id));
    guard.cleanup().await;
}

#[tokio::test]
async fn create_portfolio_with_self_parent_is_rejected() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let new_id = uuid::Uuid::new_v4();
    let res = sqlx::query(
        "INSERT INTO portfolios (id, name, purpose, created_by, parent_id) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(new_id)
    .bind("S")
    .bind("s")
    .bind("t")
    .bind(new_id)
    .execute(&pool)
    .await;
    assert!(res.is_err(), "self-parent insert must fail");
    guard.cleanup().await;
}

#[tokio::test]
async fn update_portfolio_non_key_fields() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let p = create_portfolio(
        &pool,
        NewPortfolio {
            name: "Before".into(),
            purpose: "p".into(),
            created_by: "t".into(),
            parent_id: None,
        },
    )
    .await
    .expect("create");
    let original_created_at = p.created_at;
    let updated = update_portfolio(
        &pool,
        p.id(),
        PortfolioPatch {
            name: Some("After".into()),
            purpose: Some("p2".into()),
        },
    )
    .await
    .expect("update");
    assert_eq!(updated.name, "After");
    assert_eq!(updated.purpose, "p2");
    assert_eq!(updated.created_at, original_created_at);
    guard.cleanup().await;
}

#[tokio::test]
async fn update_portfolio_id_is_immutable() {
    // The PortfolioPatch struct does not expose `id`, so immutability is
    // structural; this test documents the contract.
    let patch = PortfolioPatch::default();
    assert!(patch.name.is_none());
    assert!(patch.purpose.is_none());
}

#[tokio::test]
async fn delete_leaf_portfolio_succeeds() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let p = create_portfolio(
        &pool,
        NewPortfolio {
            name: "Leaf".into(),
            purpose: "l".into(),
            created_by: "t".into(),
            parent_id: None,
        },
    )
    .await
    .expect("create");
    delete_portfolio(&pool, p.id()).await.expect("delete");
    let res = get_portfolio(&pool, p.id()).await;
    assert!(matches!(
        res,
        Err(DomainError::NotFound {
            entity: "Portfolio",
            ..
        })
    ));
    guard.cleanup().await;
}

#[tokio::test]
async fn delete_portfolio_with_children_is_rejected() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let parent = create_portfolio(
        &pool,
        NewPortfolio {
            name: "P".into(),
            purpose: "p".into(),
            created_by: "t".into(),
            parent_id: None,
        },
    )
    .await
    .expect("create parent");
    let _child = create_portfolio(
        &pool,
        NewPortfolio {
            name: "C".into(),
            purpose: "c".into(),
            created_by: "t".into(),
            parent_id: Some(parent.id()),
        },
    )
    .await
    .expect("create child");
    let res = delete_portfolio(&pool, parent.id()).await;
    assert!(matches!(
        res,
        Err(DomainError::CannotDelete {
            entity: "Portfolio",
            ..
        })
    ));
    guard.cleanup().await;
}

#[tokio::test]
async fn model_description_includes_portfolio() {
    // Pure-data test; the fixture gating is harmless.
    let Some((_pool, _guard)) = tx_pool().await else {
        return;
    };
    let desc = model_description();
    let p = desc
        .entities
        .iter()
        .find(|e| e.name == "Portfolio")
        .expect("Portfolio");
    assert_eq!(p.fields.len(), 6);
    assert!(desc.relationships.iter().any(|r| r.label == "composes"));
}

#[tokio::test]
async fn list_ancestors_returns_root_to_leaf_chain() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    // Build a 3-deep chain: root → mid → leaf.
    let root = create_portfolio(
        &pool,
        NewPortfolio {
            name: "root".into(),
            purpose: "r".into(),
            created_by: "t".into(),
            parent_id: None,
        },
    )
    .await
    .expect("root");
    let mid = create_portfolio(
        &pool,
        NewPortfolio {
            name: "mid".into(),
            purpose: "m".into(),
            created_by: "t".into(),
            parent_id: Some(root.id()),
        },
    )
    .await
    .expect("mid");
    let leaf = create_portfolio(
        &pool,
        NewPortfolio {
            name: "leaf".into(),
            purpose: "l".into(),
            created_by: "t".into(),
            parent_id: Some(mid.id()),
        },
    )
    .await
    .expect("leaf");

    // From `leaf`: ancestors must be [root, mid] in root-to-leaf order.
    let chain = list_ancestors(&pool, leaf.id()).await.expect("ancestors");
    assert_eq!(
        chain.iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![root.id().as_uuid(), mid.id().as_uuid()],
        "ancestors must be returned root-to-leaf, excluding the leaf itself"
    );

    // From `root`: no ancestors.
    let none = list_ancestors(&pool, root.id())
        .await
        .expect("no ancestors");
    assert!(none.is_empty(), "root has no ancestors");

    guard.cleanup().await;
}
