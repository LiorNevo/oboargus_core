//! Tests for US8 — issuer external identifiers and the independence of the
//! issuer and asset identifier domains.
//!
//! Implements FR-028 and the US8 acceptance scenarios (including scenario 6,
//! cross-domain independence). Gated by `DATABASE_URL`.

use crate::common::*;
use crate::domain::error::DomainError;
use crate::domain::id::IssuerId;
use crate::domain::identifier::IdentifierScheme;
use crate::domain::issuer::create_issuer;
use crate::domain::issuer::entity::NewIssuer;
use crate::domain::issuer_identifier::entity::NewIssuerIdentifier;
use crate::domain::issuer_identifier::{
    create_issuer_identifier, delete_issuer_identifier, find_issuer_by_identifier,
    find_issuer_identifier, get_issuer_identifier, list_issuer_identifiers,
};

async fn new_issuer(pool: &sqlx::PgPool, name: &str) -> IssuerId {
    create_issuer(
        pool,
        NewIssuer {
            name: name.into(),
            issuer_type: "corporation".into(),
            legal_jurisdiction: "US".into(),
            parent_id: None,
        },
    )
    .await
    .expect("create issuer")
    .id()
}

#[tokio::test]
async fn create_issuer_identifier_persists_and_resolves() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let issuer_id = new_issuer(&pool, "Alphabet Inc.").await;
    let created = create_issuer_identifier(
        &pool,
        NewIssuerIdentifier {
            issuer_id,
            scheme: IdentifierScheme::LEI.into(),
            value: "5493006MHB84DD0ZWV18".into(),
        },
    )
    .await
    .expect("create identifier");
    assert_eq!(created.issuer_id(), issuer_id);

    let found = find_issuer_by_identifier(&pool, "lei", "5493006MHB84DD0ZWV18")
        .await
        .expect("lookup")
        .expect("issuer resolved");
    assert_eq!(found.id(), issuer_id);

    let listed = list_issuer_identifiers(&pool, issuer_id)
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);
    let _ = get_issuer_identifier(&pool, created.id())
        .await
        .expect("read");
    guard.cleanup().await;
}

#[tokio::test]
async fn duplicate_identifier_within_issuer_domain_is_rejected() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let issuer_a = new_issuer(&pool, "Alphabet Inc.").await;
    let issuer_b = new_issuer(&pool, "Google LLC").await;
    create_issuer_identifier(
        &pool,
        NewIssuerIdentifier {
            issuer_id: issuer_a,
            scheme: IdentifierScheme::LEI.into(),
            value: "5493006MHB84DD0ZWV18".into(),
        },
    )
    .await
    .expect("first registration");

    let err = create_issuer_identifier(
        &pool,
        NewIssuerIdentifier {
            issuer_id: issuer_b,
            scheme: IdentifierScheme::LEI.into(),
            value: "5493006MHB84DD0ZWV18".into(),
        },
    )
    .await
    .expect_err("second claim must be rejected");
    assert!(matches!(
        err,
        DomainError::InvariantViolation {
            invariant: "identifier_uniqueness",
            ..
        }
    ));
    guard.cleanup().await;
}

#[tokio::test]
async fn issuer_identifier_domain_is_independent() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let issuer_id = new_issuer(&pool, "Alphabet Inc.").await;
    let asset_id = crate::domain::asset::create_asset(
        &pool,
        crate::domain::asset::entity::NewAsset {
            asset_type: "stock".into(),
            display_name: "Alphabet Inc. Class A".into(),
            currency: "USD".into(),
            backed_by_portfolio_id: None,
            issuer_id: Some(issuer_id),
        },
    )
    .await
    .expect("create asset")
    .id();

    // The same `value` under different schemes in different domains: no
    // conflict — uniqueness is per subject domain (predecessor Assumption 23).
    create_issuer_identifier(
        &pool,
        NewIssuerIdentifier {
            issuer_id,
            scheme: IdentifierScheme::CIK.into(),
            value: "1652044".into(),
        },
    )
    .await
    .expect("issuer identifier");
    create_asset_identifier_for(&pool, asset_id, IdentifierScheme::TICKER, "1652044").await;

    // Each lookup resolves its own subject.
    let issuer = find_issuer_by_identifier(&pool, "CIK", "1652044")
        .await
        .expect("lookup issuer")
        .expect("issuer resolved");
    assert_eq!(issuer.id(), issuer_id);
    let asset =
        crate::domain::asset_identifier::find_asset_by_identifier(&pool, "TICKER", "1652044")
            .await
            .expect("lookup asset")
            .expect("asset resolved");
    assert_eq!(asset.id(), asset_id);
    guard.cleanup().await;
}

#[tokio::test]
async fn deleting_issuer_deletes_its_identifiers() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let issuer_id = new_issuer(&pool, "Alphabet Inc.").await;
    create_issuer_identifier(
        &pool,
        NewIssuerIdentifier {
            issuer_id,
            scheme: IdentifierScheme::LEI.into(),
            value: "5493006MHB84DD0ZWV18".into(),
        },
    )
    .await
    .expect("create identifier");

    crate::domain::issuer::delete_issuer(&pool, issuer_id)
        .await
        .expect("delete issuer");
    let gone = find_issuer_identifier(&pool, "LEI", "5493006MHB84DD0ZWV18")
        .await
        .expect("lookup");
    assert!(gone.is_none(), "identifier must be deleted with its issuer");
    guard.cleanup().await;
}

#[tokio::test]
async fn delete_issuer_identifier_succeeds() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let issuer_id = new_issuer(&pool, "Alphabet Inc.").await;
    let identifier = create_issuer_identifier(
        &pool,
        NewIssuerIdentifier {
            issuer_id,
            scheme: IdentifierScheme::LEI.into(),
            value: "5493006MHB84DD0ZWV18".into(),
        },
    )
    .await
    .expect("create identifier");

    delete_issuer_identifier(&pool, identifier.id())
        .await
        .expect("delete identifier");
    let err = delete_issuer_identifier(&pool, identifier.id())
        .await
        .expect_err("second delete must be not-found");
    assert!(matches!(err, DomainError::NotFound { .. }));
    guard.cleanup().await;
}

/// Small helper: register an asset identifier from raw parts (used by the
/// cross-domain independence test).
async fn create_asset_identifier_for(
    pool: &sqlx::PgPool,
    asset_id: crate::domain::id::AssetId,
    scheme: &str,
    value: &str,
) {
    crate::domain::asset_identifier::create_asset_identifier(
        pool,
        crate::domain::asset_identifier::entity::NewAssetIdentifier {
            asset_id,
            scheme: scheme.into(),
            value: value.into(),
        },
    )
    .await
    .expect("create asset identifier");
}
