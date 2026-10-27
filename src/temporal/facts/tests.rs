//! Unit tests for the fact subject mapping and the shared row decoder.

use uuid::Uuid;

use crate::domain::id::{AssetId, DomainId, IssuerId, PortfolioId};

use super::*;

#[test]
fn each_subject_maps_to_its_own_table_and_column() {
    let id = Uuid::from_u128(7);
    let asset = FactSubject::Asset(AssetId::from_uuid(id));
    let issuer = FactSubject::Issuer(IssuerId::from_uuid(id));
    let portfolio = FactSubject::Portfolio(PortfolioId::from_uuid(id));

    assert_eq!(asset.table(), "asset_reference_data");
    assert_eq!(asset.subject_column(), "asset_id");
    assert_eq!(issuer.table(), "issuer_reference_data");
    assert_eq!(issuer.subject_column(), "issuer_id");
    assert_eq!(portfolio.table(), "portfolio_reference_data");
    assert_eq!(portfolio.subject_column(), "portfolio_id");
}

#[test]
fn subjects_round_trip_through_their_wire_parts() {
    let id = Uuid::from_u128(11);
    for (kind, subject) in [
        ("asset", FactSubject::Asset(AssetId::from_uuid(id))),
        ("issuer", FactSubject::Issuer(IssuerId::from_uuid(id))),
        (
            "portfolio",
            FactSubject::Portfolio(PortfolioId::from_uuid(id)),
        ),
    ] {
        assert_eq!(
            FactSubject::from_parts(kind, id).expect("known subject"),
            subject
        );
        assert_eq!(subject.id(), id);
        assert_eq!(subject.subject_name(), kind);
    }
    assert!(FactSubject::from_parts("instrument", id).is_err());
}

#[test]
fn row_decoder_preserves_the_subject_and_both_time_axes() {
    let valid = chrono::DateTime::<chrono::Utc>::UNIX_EPOCH + chrono::Duration::seconds(10);
    let known = chrono::DateTime::<chrono::Utc>::UNIX_EPOCH + chrono::Duration::seconds(20);
    let row = FactRow {
        id: Uuid::from_u128(3),
        subject_id: Uuid::from_u128(4),
        fact_type: "market_price".to_string(),
        valid_at: valid,
        known_at: known,
        value: "{}".to_string(),
        source: "yahoo".to_string(),
        retracted: false,
    };
    let point = row_to_fact_point(
        row,
        FactSubject::Asset(AssetId::from_uuid(Uuid::from_u128(4))),
    )
    .expect("decodes");
    assert_eq!(point.valid_at.as_datetime(), valid);
    assert_eq!(point.known_at.as_datetime(), known);
    assert!(!point.retracted);
}

#[test]
fn a_row_from_another_subject_is_rejected_by_the_decoder() {
    let row = FactRow {
        id: Uuid::from_u128(3),
        subject_id: Uuid::from_u128(9),
        fact_type: "market_price".to_string(),
        valid_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
        known_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
        value: "{}".to_string(),
        source: "yahoo".to_string(),
        retracted: false,
    };
    let error = row_to_fact_point(
        row,
        FactSubject::Asset(AssetId::from_uuid(Uuid::from_u128(4))),
    )
    .expect_err("the row belongs to a different subject");
    assert!(matches!(
        error,
        crate::temporal::TemporalError::UnknownSubject { .. }
    ));
}

/// A `lazy` pool never opens a connection, so a test using it proves the
/// validation runs before any database work.
fn lazy_pool() -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://obolargus@127.0.0.1/obolargus")
        .expect("a lazy pool is built without connecting")
}

#[tokio::test]
async fn append_rejects_an_empty_fact_type_before_touching_the_database() {
    let error = super::write::append_fact(
        &lazy_pool(),
        FactSubject::Asset(AssetId::from_uuid(Uuid::from_u128(4))),
        "  ",
        crate::temporal::ValidAt::default(),
        crate::temporal::KnownAt::default(),
        "{}",
        "fixture",
    )
    .await
    .expect_err("an empty fact_type is invalid");
    assert!(matches!(
        error,
        crate::temporal::TemporalError::InvalidTime { .. }
    ));
}

#[tokio::test]
async fn revoke_rejects_an_empty_source_before_touching_the_database() {
    let error = super::write::revoke_fact(
        &lazy_pool(),
        FactSubject::Asset(AssetId::from_uuid(Uuid::from_u128(4))),
        "market_price",
        crate::temporal::ValidAt::default(),
        crate::temporal::KnownAt::default(),
        "delisted",
        "",
    )
    .await
    .expect_err("an empty source is invalid");
    assert!(matches!(
        error,
        crate::temporal::TemporalError::InvalidTime { .. }
    ));
}
