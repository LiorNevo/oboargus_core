//! Tests for US8 — asset external identifiers: registration, lookup,
//! uniqueness, validation, open scheme vocabulary, and lifecycle.
//!
//! Implements FR-028 and the US8 acceptance scenarios. Gated by
//! `DATABASE_URL`.

use crate::common::*;
use crate::domain::asset::create_asset;
use crate::domain::asset::entity::NewAsset;
use crate::domain::asset_identifier::entity::NewAssetIdentifier;
use crate::domain::asset_identifier::{
    create_asset_identifier, delete_asset_identifier, find_asset_by_identifier,
    find_asset_identifier, get_asset_identifier, list_asset_identifiers,
};
use crate::domain::error::DomainError;
use crate::domain::id::AssetId;
use crate::domain::identifier::IdentifierScheme;

async fn new_asset(pool: &sqlx::PgPool, name: &str) -> AssetId {
    create_asset(
        pool,
        NewAsset {
            asset_type: "stock".into(),
            display_name: name.into(),
            currency: "USD".into(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
    )
    .await
    .expect("create asset")
    .id()
}

#[tokio::test]
async fn create_asset_identifier_persists_and_lists() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset_id = new_asset(&pool, "Apple Inc.").await;
    let created = create_asset_identifier(
        &pool,
        NewAssetIdentifier {
            asset_id,
            scheme: " isin ".into(),
            value: " US0378331005 ".into(),
        },
    )
    .await
    .expect("create identifier");

    // FR-025: the scheme is trimmed and uppercased; the value is trimmed.
    assert_eq!(created.scheme, "ISIN");
    assert_eq!(created.value, "US0378331005");
    assert_eq!(created.asset_id(), asset_id);

    let fetched = get_asset_identifier(&pool, created.id())
        .await
        .expect("read");
    assert_eq!(fetched, created);

    let listed = list_asset_identifiers(&pool, asset_id).await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id(), created.id());
    guard.cleanup().await;
}

#[tokio::test]
async fn find_asset_by_identifier_resolves_the_asset() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset_id = new_asset(&pool, "Apple Inc.").await;
    create_asset_identifier(
        &pool,
        NewAssetIdentifier {
            asset_id,
            scheme: IdentifierScheme::ISIN.into(),
            value: "US0378331005".into(),
        },
    )
    .await
    .expect("create identifier");

    let found = find_asset_by_identifier(&pool, "ISIN", "US0378331005")
        .await
        .expect("lookup");
    let found = found.expect("asset resolved");
    assert_eq!(found.id(), asset_id);

    // An unclaimed pair resolves to nothing — not an error, not an
    // arbitrary row.
    let none = find_asset_by_identifier(&pool, "ISIN", "US9999999999")
        .await
        .expect("lookup unclaimed");
    assert!(none.is_none());
    guard.cleanup().await;
}

#[tokio::test]
async fn duplicate_identifier_on_second_asset_is_rejected() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset_a = new_asset(&pool, "Apple Inc.").await;
    let asset_b = new_asset(&pool, "Apple Inc. copy").await;
    let first = create_asset_identifier(
        &pool,
        NewAssetIdentifier {
            asset_id: asset_a,
            scheme: IdentifierScheme::ISIN.into(),
            value: "US0378331005".into(),
        },
    )
    .await
    .expect("first registration");

    let err = create_asset_identifier(
        &pool,
        NewAssetIdentifier {
            asset_id: asset_b,
            scheme: IdentifierScheme::ISIN.into(),
            value: "US0378331005".into(),
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

    // Asset A still owns the pair.
    let owner = find_asset_identifier(&pool, "ISIN", "US0378331005")
        .await
        .expect("lookup")
        .expect("still claimed");
    assert_eq!(owner.id(), first.id());
    assert_eq!(owner.asset_id(), asset_a);
    guard.cleanup().await;
}

#[tokio::test]
async fn empty_scheme_or_value_is_rejected() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset_id = new_asset(&pool, "Apple Inc.").await;
    for (scheme, value) in [("   ", "US0378331005"), ("ISIN", "")] {
        let err = create_asset_identifier(
            &pool,
            NewAssetIdentifier {
                asset_id,
                scheme: scheme.into(),
                value: value.into(),
            },
        )
        .await
        .expect_err("empty key must be rejected");
        assert!(matches!(err, DomainError::Validation(_)));
    }
    guard.cleanup().await;
}

#[tokio::test]
async fn unknown_scheme_is_accepted_and_looked_up() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset_id = new_asset(&pool, "SAP SE").await;
    // FR-026: an open vocabulary — "TICKER:XETRA" is not a well-known
    // constant and must behave exactly like one.
    create_asset_identifier(
        &pool,
        NewAssetIdentifier {
            asset_id,
            scheme: "ticker:xetra".into(),
            value: "SAP".into(),
        },
    )
    .await
    .expect("create identifier with unknown scheme");

    let found = find_asset_by_identifier(&pool, "TICKER:XETRA", "SAP")
        .await
        .expect("lookup")
        .expect("asset resolved");
    assert_eq!(found.id(), asset_id);

    // A formatting variant of a well-known scheme resolves the same way.
    create_asset_identifier(
        &pool,
        NewAssetIdentifier {
            asset_id,
            scheme: "isin".into(),
            value: "DE0007164600".into(),
        },
    )
    .await
    .expect("create identifier with lowercase scheme");
    let found = find_asset_by_identifier(&pool, " ISIN ", " DE0007164600 ")
        .await
        .expect("lookup")
        .expect("asset resolved");
    assert_eq!(found.id(), asset_id);
    guard.cleanup().await;
}

#[tokio::test]
async fn deleting_subject_deletes_its_identifiers() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset_id = new_asset(&pool, "Apple Inc.").await;
    let identifier = create_asset_identifier(
        &pool,
        NewAssetIdentifier {
            asset_id,
            scheme: IdentifierScheme::TICKER.into(),
            value: "AAPL".into(),
        },
    )
    .await
    .expect("create identifier");

    // Identifiers are current-state keys, NOT protected history: unlike
    // reference data, they must not block the subject's deletion.
    crate::domain::asset::delete_asset(&pool, asset_id)
        .await
        .expect("delete asset");

    let gone = find_asset_identifier(&pool, "TICKER", "AAPL")
        .await
        .expect("lookup");
    assert!(gone.is_none(), "identifier must be deleted with its asset");
    let record = get_asset_identifier(&pool, identifier.id()).await;
    assert!(matches!(record, Err(DomainError::NotFound { .. })));
    guard.cleanup().await;
}

#[tokio::test]
async fn delete_asset_identifier_succeeds() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset_id = new_asset(&pool, "Apple Inc.").await;
    let identifier = create_asset_identifier(
        &pool,
        NewAssetIdentifier {
            asset_id,
            scheme: IdentifierScheme::TICKER.into(),
            value: "AAPL".into(),
        },
    )
    .await
    .expect("create identifier");

    delete_asset_identifier(&pool, identifier.id())
        .await
        .expect("delete identifier");

    let listed = list_asset_identifiers(&pool, asset_id).await.expect("list");
    assert!(listed.is_empty());

    let err = delete_asset_identifier(&pool, identifier.id())
        .await
        .expect_err("second delete must be not-found");
    assert!(matches!(err, DomainError::NotFound { .. }));
    guard.cleanup().await;
}

#[test]
fn model_description_includes_identifier_entities() {
    let desc = crate::domain::model_description::model_description();
    for name in ["AssetIdentifier", "IssuerIdentifier"] {
        let entity = desc
            .entities
            .iter()
            .find(|e| e.name == name)
            .unwrap_or_else(|| panic!("{} missing from model_description", name));
        assert_eq!(entity.fields.len(), 5, "{} should have 5 fields", name);
        let scheme = entity
            .fields
            .iter()
            .find(|f| f.name == "scheme")
            .expect("scheme field");
        assert_eq!(scheme.aggregates, &["count"]);
        let value = entity
            .fields
            .iter()
            .find(|f| f.name == "value")
            .expect("value field");
        assert_eq!(value.aggregates, &["none"]);
    }
    for (from, to) in [("Asset", "AssetIdentifier"), ("Issuer", "IssuerIdentifier")] {
        assert!(
            desc.relationships
                .iter()
                .any(|r| { r.from == from && r.to == to && r.label == "identified_by" }),
            "{} identified_by {} missing from model_description",
            from,
            to
        );
    }
    assert!(
        desc.invariants
            .iter()
            .any(|i| i.contains("identifier names at most one subject")),
        "identifier-uniqueness invariant missing from model_description"
    );
}
