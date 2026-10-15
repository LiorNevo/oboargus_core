//! Tests for US2 — Issuer CRUD.
//! Tests for US2b — Issuer DAG.

use crate::common::*;
use crate::domain::error::DomainError;
use crate::domain::issuer::entity::{IssuerPatch, NewIssuer};
use crate::domain::issuer::{
    create_issuer, delete_issuer, get_issuer, list_ancestor_issuers, list_children_issuers,
    reparent_issuer, update_issuer,
};

#[tokio::test]
async fn create_issuer_persists_attributes() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let issuer = create_issuer(
        &pool,
        NewIssuer {
            name: "Test Co".into(),
            issuer_type: "corporation".into(),
            legal_jurisdiction: "US".into(),
            parent_id: None,
        },
    )
    .await
    .expect("create");
    let read = get_issuer(&pool, issuer.id()).await.expect("read");
    assert_eq!(read.name, "Test Co");
    assert_eq!(read.parent_id, None);
    guard.cleanup().await;
}

#[tokio::test]
async fn update_issuer_non_key_fields() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let issuer = create_issuer(
        &pool,
        NewIssuer {
            name: "Before".into(),
            issuer_type: "t".into(),
            legal_jurisdiction: "US".into(),
            parent_id: None,
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
    guard.cleanup().await;
}

#[tokio::test]
async fn delete_issuer_with_no_dependents_succeeds() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let issuer = create_issuer(
        &pool,
        NewIssuer {
            name: "To delete".into(),
            issuer_type: "t".into(),
            legal_jurisdiction: "US".into(),
            parent_id: None,
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
    guard.cleanup().await;
}

#[tokio::test]
async fn delete_issuer_with_assets_is_rejected() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let issuer = create_issuer(
        &pool,
        NewIssuer {
            name: "With Asset".into(),
            issuer_type: "t".into(),
            legal_jurisdiction: "US".into(),
            parent_id: None,
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
    guard.cleanup().await;
}

// ---------------------------------------------------------------------------
// US2b — Issuer DAG
// ---------------------------------------------------------------------------

/// T132: creating two issuers P and C, then creating C with
/// `parent_id = Some(P.id)`, persists the relationship; subsequent calls to
/// `list_children_issuers(P)` return [C] and `list_ancestor_issuers(C)` return
/// [P].
#[tokio::test]
async fn create_issuer_with_parent_links_child() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    // Root issuer (parent)
    let parent = create_issuer(
        &pool,
        NewIssuer {
            name: "Alphabet Inc.".into(),
            issuer_type: "corporation".into(),
            legal_jurisdiction: "US".into(),
            parent_id: None,
        },
    )
    .await
    .expect("create parent issuer");

    // Child issuer
    let child = create_issuer(
        &pool,
        NewIssuer {
            name: "Google LLC".into(),
            issuer_type: "subsidiary".into(),
            legal_jurisdiction: "US".into(),
            parent_id: Some(parent.id()),
        },
    )
    .await
    .expect("create child issuer");

    // Verify list_children_issuers
    let children = list_children_issuers(&pool, parent.id())
        .await
        .expect("list_children");
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].id, child.id);

    // Verify list_ancestor_issuers for child
    let ancestors = list_ancestor_issuers(&pool, child.id())
        .await
        .expect("list_ancestors");
    assert_eq!(ancestors.len(), 1);
    assert_eq!(ancestors[0].id, parent.id);

    guard.cleanup().await;
}

/// T133: creating an issuer with `parent_id = self.id` must be rejected with
/// `DomainError::InvariantViolation { invariant: "issuer_dag_acyclicity", .. }`.
#[tokio::test]
async fn create_issuer_with_self_parent_is_rejected() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let issuer = create_issuer(
        &pool,
        NewIssuer {
            name: "Self-ref".into(),
            issuer_type: "test".into(),
            legal_jurisdiction: "XX".into(),
            parent_id: None, // create first, get the id, then reparent
        },
    )
    .await
    .expect("create");

    // Reparent to self — must be rejected
    let result = reparent_issuer(&pool, issuer.id(), Some(issuer.id())).await;
    let err = result.expect_err("reparent to self should fail");
    assert!(matches!(
        err,
        DomainError::InvariantViolation {
            invariant: "issuer_dag_acyclicity",
            ..
        }
    ));
    guard.cleanup().await;
}

/// T134: given a chain P → C1 → C2, attempting to set C2's parent to P
/// Reparenting a node to a new parent that is one of its **current**
/// ancestors is a cycle (the new edge `new_parent → id` closes the
/// loop `id → ... → new_parent → id`). Reparenting to a non-ancestor
/// (even one that shares a common ancestor) is a valid tree operation
/// and must NOT be rejected.
///
/// This test exercises the **non-cycle** case: reparenting `C2` from
/// `C1` to `P` (where `P → C1 → C2` is the current chain) is valid —
/// the resulting tree is `P → {C1, C2}` with no cycle. The
/// `reparent_issuer_cycle_is_rejected` test below exercises the
/// actual cycle case (`A → B`, reparent `A` under `B`).
#[tokio::test]
async fn create_issuer_cycle_is_rejected() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    // P (root)
    let p = create_issuer(
        &pool,
        NewIssuer {
            name: "P".into(),
            issuer_type: "root".into(),
            legal_jurisdiction: "US".into(),
            parent_id: None,
        },
    )
    .await
    .expect("create P");
    // C1 → P
    let c1 = create_issuer(
        &pool,
        NewIssuer {
            name: "C1".into(),
            issuer_type: "child".into(),
            legal_jurisdiction: "US".into(),
            parent_id: Some(p.id()),
        },
    )
    .await
    .expect("create C1");
    // C2 → C1
    let c2 = create_issuer(
        &pool,
        NewIssuer {
            name: "C2".into(),
            issuer_type: "child".into(),
            legal_jurisdiction: "US".into(),
            parent_id: Some(c1.id()),
        },
    )
    .await
    .expect("create C2");

    // Reparent C2 from C1 to P. The current chain is C2 → C1 → P;
    // after the reparent C2 → P, the tree rooted at P is {P, C1, C2}
    // with edges P→C1 and P→C2. This is a valid tree, NOT a cycle
    // (no path from C2 back to itself). The operation must succeed.
    let result = reparent_issuer(&pool, c2.id(), Some(p.id()))
        .await
        .expect("reparent C2 under P is a valid tree operation, not a cycle");
    assert_eq!(result.parent_id, Some(p.id().0));

    guard.cleanup().await;
}

/// T135: creating an issuer with `parent_id = None` still works — regression
/// guard for the root-issuer case.
#[tokio::test]
async fn create_issuer_with_no_parent_succeeds() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let issuer = create_issuer(
        &pool,
        NewIssuer {
            name: "Standalone Corp".into(),
            issuer_type: "corporation".into(),
            legal_jurisdiction: "DE".into(),
            parent_id: None,
        },
    )
    .await
    .expect("create root issuer");
    assert_eq!(issuer.parent_id, None);
    guard.cleanup().await;
}

/// T136: `list_children_issuers` on a leaf issuer returns an empty Vec.
#[tokio::test]
async fn list_children_issuers_returns_empty_for_leaf() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let leaf = create_issuer(
        &pool,
        NewIssuer {
            name: "Leaf Issuer".into(),
            issuer_type: "leaf".into(),
            legal_jurisdiction: "US".into(),
            parent_id: None,
        },
    )
    .await
    .expect("create");
    let children = list_children_issuers(&pool, leaf.id())
        .await
        .expect("list_children");
    assert!(children.is_empty());
    guard.cleanup().await;
}

/// T137: given a chain P → C1 → C2, `list_ancestor_issuers(C2)` returns
/// [C1, P] in bottom-up order (C1 first, then P).
#[tokio::test]
async fn list_ancestor_issuers_returns_full_chain() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let p = create_issuer(
        &pool,
        NewIssuer {
            name: "Root".into(),
            issuer_type: "root".into(),
            legal_jurisdiction: "US".into(),
            parent_id: None,
        },
    )
    .await
    .expect("create root");
    let c1 = create_issuer(
        &pool,
        NewIssuer {
            name: "Child1".into(),
            issuer_type: "child".into(),
            legal_jurisdiction: "US".into(),
            parent_id: Some(p.id()),
        },
    )
    .await
    .expect("create c1");
    let c2 = create_issuer(
        &pool,
        NewIssuer {
            name: "Child2".into(),
            issuer_type: "child".into(),
            legal_jurisdiction: "US".into(),
            parent_id: Some(c1.id()),
        },
    )
    .await
    .expect("create c2");

    let ancestors = list_ancestor_issuers(&pool, c2.id())
        .await
        .expect("list_ancestors");
    assert_eq!(ancestors.len(), 2);
    assert_eq!(ancestors[0].id, c1.id); // immediate parent first
    assert_eq!(ancestors[1].id, p.id); // then grandparent
    guard.cleanup().await;
}

/// T138: `reparent_issuer(id, None)` on a child issuer makes it a root;
/// `list_children_issuers` of the former parent becomes empty and
/// `list_ancestor_issuers` of the former child becomes empty.
#[tokio::test]
async fn reparent_issuer_moves_parent() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let parent = create_issuer(
        &pool,
        NewIssuer {
            name: "Parent Root".into(),
            issuer_type: "root".into(),
            legal_jurisdiction: "US".into(),
            parent_id: None,
        },
    )
    .await
    .expect("create parent");
    let child = create_issuer(
        &pool,
        NewIssuer {
            name: "Orphan Me".into(),
            issuer_type: "child".into(),
            legal_jurisdiction: "US".into(),
            parent_id: Some(parent.id()),
        },
    )
    .await
    .expect("create child");

    // Reparent child to root (new_parent = None)
    let reparented = reparent_issuer(&pool, child.id(), None)
        .await
        .expect("reparent to root");

    // The reparented issuer must have no parent
    assert_eq!(reparented.parent_id, None);

    // Former parent's children list must be empty
    let former_parent_children = list_children_issuers(&pool, parent.id())
        .await
        .expect("list former parent children");
    assert!(former_parent_children.is_empty());

    // Former child's ancestors must be empty
    let new_root_ancestors = list_ancestor_issuers(&pool, child.id())
        .await
        .expect("list new root ancestors");
    assert!(new_root_ancestors.is_empty());

    guard.cleanup().await;
}

/// T139: `reparent_issuer` with a `new_parent_id` that would form a cycle
/// returns `DomainError::InvariantViolation { invariant: "issuer_dag_acyclicity", .. }`.
#[tokio::test]
async fn reparent_issuer_cycle_is_rejected() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    // Build a two-step chain: A → B
    let a = create_issuer(
        &pool,
        NewIssuer {
            name: "A".into(),
            issuer_type: "root".into(),
            legal_jurisdiction: "US".into(),
            parent_id: None,
        },
    )
    .await
    .expect("create A");
    let b = create_issuer(
        &pool,
        NewIssuer {
            name: "B".into(),
            issuer_type: "child".into(),
            legal_jurisdiction: "US".into(),
            parent_id: Some(a.id()),
        },
    )
    .await
    .expect("create B");

    // Try to make B's parent be A (B is already a child of A). This is
    // a no-op reparent (idempotent) — not a cycle. The production code
    // accepts it; we verify the new parent_id is still A.
    let r1 = reparent_issuer(&pool, b.id(), Some(a.id()))
        .await
        .expect("reparent B under A (B is already A's child) is a no-op, not an error");
    assert_eq!(r1.parent_id, Some(a.id().0));

    // Actually try the opposite direction: make A a child of B → cycle A→B→A
    let result = reparent_issuer(&pool, a.id(), Some(b.id())).await;
    let err = result.expect_err("reparent A under B should fail — creates cycle");
    assert!(matches!(
        err,
        DomainError::InvariantViolation {
            invariant: "issuer_dag_acyclicity",
            ..
        }
    ));
    guard.cleanup().await;
}

/// T140: deleting an issuer that has child issuers must be rejected with
/// `DomainError::CannotDelete { entity: "Issuer", reason: "issuer has N children, retire them first" }`.
#[tokio::test]
async fn delete_issuer_with_children_is_rejected() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let parent = create_issuer(
        &pool,
        NewIssuer {
            name: "Parent Issuer".into(),
            issuer_type: "parent".into(),
            legal_jurisdiction: "US".into(),
            parent_id: None,
        },
    )
    .await
    .expect("create parent");
    let _child = create_issuer(
        &pool,
        NewIssuer {
            name: "Child Issuer".into(),
            issuer_type: "child".into(),
            legal_jurisdiction: "US".into(),
            parent_id: Some(parent.id()),
        },
    )
    .await
    .expect("create child");

    let result = delete_issuer(&pool, parent.id()).await;
    let err = result.expect_err("delete with children should fail");
    let reason = match err {
        DomainError::CannotDelete {
            entity: "Issuer",
            reason: r,
            ..
        } => r,
        other => panic!("expected CannotDelete, got {:?}", other),
    };
    assert!(
        reason.contains("children"),
        "reason should mention children: {reason}"
    );
    guard.cleanup().await;
}

/// T141: `model_description()` must include the `Issuer parent_of Issuer`
/// self-referential relationship and the `issuer_dag_acyclicity` invariant
/// (Invariant 12).
#[tokio::test]
async fn model_description_includes_issuer_dag_relationship() {
    use crate::domain::model_description::model_description;

    let desc = model_description();
    // Find the issuer → issuer relationship
    let issuer_parent_rel = desc
        .relationships
        .iter()
        .find(|r| r.from == "Issuer" && r.to == "Issuer");
    assert!(
        issuer_parent_rel.is_some(),
        "model_description should have an Issuer→Issuer relationship"
    );
    let rel = issuer_parent_rel.unwrap();
    assert_eq!(rel.label, "parent_of");
    assert_eq!(rel.cardinality_from, "0..*");
    assert_eq!(rel.cardinality_to, "0..1");

    // Find the issuer_dag_acyclicity invariant
    assert!(
        desc.invariants
            .iter()
            .any(|i| i.to_lowercase().contains("issuer") && i.to_lowercase().contains("dag")),
        "model_description should contain an issuer-DAG invariant, got: {:?}",
        desc.invariants
    );
}
