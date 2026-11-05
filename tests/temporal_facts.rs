//! Integration tests for the normalized bitemporal fact contract.
//!
//! Database-gated: returns early when the fixture cannot connect.

use chrono::{DateTime, Utc};
use obolargus_core::domain::id::{AssetId, DomainId};
use obolargus_core::temporal::facts::read::{
    fact_as_known, fact_at, fact_history, fact_series_page,
};
use obolargus_core::temporal::{
    FactInput, FactSubject, FactValue, KnownAt, RetractionInput, TemporalError, TemporalWindow,
    ValidAt, retract_fact, write_fact,
};
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::str::FromStr;
use uuid::Uuid;

fn instant(raw: &str) -> DateTime<Utc> {
    raw.parse().expect("rfc3339 instant")
}

fn valid(raw: &str) -> ValidAt {
    ValidAt::new(instant(raw)).expect("valid time")
}

fn known(raw: &str) -> KnownAt {
    KnownAt::new(instant(raw)).expect("known time")
}

async fn seed_asset(pool: &PgPool) -> FactSubject {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO assets (id, asset_type, display_name, currency) \
         VALUES ($1, 'stock', 'Bitemporal Asset', 'USD')",
    )
    .bind(id)
    .execute(pool)
    .await
    .expect("seed asset");
    FactSubject::Asset(AssetId::from_uuid(id))
}

fn decimal(raw: &str) -> Decimal {
    Decimal::from_str(raw).expect("decimal")
}

async fn append(
    pool: &PgPool,
    subject: FactSubject,
    valid_at: ValidAt,
    known_at: KnownAt,
    value: &str,
    source: &str,
) {
    write_fact(
        pool,
        FactInput {
            subject,
            fact_type: "nav".to_string(),
            valid_at,
            known_at,
            source: source.to_string(),
            value: FactValue::Decimal {
                value: decimal(value),
            },
        },
    )
    .await
    .expect("write fact");
}

async fn retract(
    pool: &PgPool,
    subject: FactSubject,
    valid_at: ValidAt,
    known_at: KnownAt,
    reason: &str,
    source: &str,
) {
    retract_fact(
        pool,
        RetractionInput {
            subject,
            fact_type: "nav".to_string(),
            valid_at,
            known_at,
            source: source.to_string(),
            reason: reason.to_string(),
        },
    )
    .await
    .expect("retract fact");
}

fn decimal_value(point: &obolargus_core::temporal::FactPoint) -> &Decimal {
    match point.value.as_ref().expect("value") {
        FactValue::Decimal { value } => value,
        other => panic!("expected decimal, got {other:?}"),
    }
}

const VALID_DAY: &str = "2026-01-02T00:00:00Z";
const FIRST_KNOWN: &str = "2026-01-02T12:00:00Z";
const RESTATED_KNOWN: &str = "2026-01-02T12:00:00.000001Z";
const RETRACT_KNOWN: &str = "2026-01-03T09:00:00Z";
const REINSTATE_KNOWN: &str = "2026-01-04T09:00:00Z";

#[tokio::test]
async fn same_day_revisions_are_distinct_and_ordered() {
    let Some((pool, guard)) = obolargus_core::common::tx_pool().await else {
        return;
    };
    let subject = seed_asset(&pool).await;

    append(
        &pool,
        subject,
        valid(VALID_DAY),
        known(FIRST_KNOWN),
        "100.00",
        "fixture",
    )
    .await;
    append(
        &pool,
        subject,
        valid(VALID_DAY),
        known(RESTATED_KNOWN),
        "101.00",
        "fixture",
    )
    .await;

    let current = fact_at(&pool, subject, "nav", valid(VALID_DAY))
        .await
        .expect("read current")
        .expect("value in force");
    assert_eq!(decimal_value(&current), &decimal("101"));

    let earlier = fact_as_known(&pool, subject, "nav", valid(VALID_DAY), known(FIRST_KNOWN))
        .await
        .expect("read as known")
        .expect("earlier belief is still visible");
    assert_eq!(decimal_value(&earlier), &decimal("100"));

    let history = fact_history(&pool, subject, "nav", valid(VALID_DAY))
        .await
        .expect("history");
    assert_eq!(history.len(), 2);
    assert_eq!(decimal_value(&history[0]), &decimal("101"));
    assert_eq!(decimal_value(&history[1]), &decimal("100"));

    guard.cleanup().await;
}

#[tokio::test]
async fn as_known_before_the_first_record_has_no_value() {
    let Some((pool, guard)) = obolargus_core::common::tx_pool().await else {
        return;
    };
    let subject = seed_asset(&pool).await;

    let before_first = fact_as_known(
        &pool,
        subject,
        "nav",
        valid(VALID_DAY),
        known("2026-01-01T00:00:00Z"),
    )
    .await
    .expect("read as known");
    assert!(before_first.is_none());

    guard.cleanup().await;
}

#[tokio::test]
async fn a_retraction_hides_the_value_after_its_known_at_only() {
    let Some((pool, guard)) = obolargus_core::common::tx_pool().await else {
        return;
    };
    let subject = seed_asset(&pool).await;

    append(
        &pool,
        subject,
        valid(VALID_DAY),
        known(FIRST_KNOWN),
        "100.00",
        "fixture",
    )
    .await;
    retract(
        &pool,
        subject,
        valid(VALID_DAY),
        known(RETRACT_KNOWN),
        "delisted",
        "fixture",
    )
    .await;

    assert!(
        fact_at(&pool, subject, "nav", valid(VALID_DAY))
            .await
            .expect("current")
            .is_none(),
        "a retracted winner must resolve to no value"
    );

    let before_retraction =
        fact_as_known(&pool, subject, "nav", valid(VALID_DAY), known(FIRST_KNOWN))
            .await
            .expect("as known before retraction")
            .expect("the older belief is untouched");
    assert_eq!(decimal_value(&before_retraction), &decimal("100"));

    let retraction = fact_history(&pool, subject, "nav", valid(VALID_DAY))
        .await
        .expect("history")
        .into_iter()
        .find(|point| point.retracted)
        .expect("retraction row");
    assert_eq!(retraction.retraction_reason.as_deref(), Some("delisted"));
    assert!(retraction.value.is_none());

    guard.cleanup().await;
}

#[tokio::test]
async fn a_retracted_value_can_be_reinstated_by_a_later_belief() {
    let Some((pool, guard)) = obolargus_core::common::tx_pool().await else {
        return;
    };
    let subject = seed_asset(&pool).await;

    append(
        &pool,
        subject,
        valid(VALID_DAY),
        known(FIRST_KNOWN),
        "100.00",
        "fixture",
    )
    .await;
    retract(
        &pool,
        subject,
        valid(VALID_DAY),
        known(RETRACT_KNOWN),
        "delisted",
        "fixture",
    )
    .await;
    append(
        &pool,
        subject,
        valid(VALID_DAY),
        known(REINSTATE_KNOWN),
        "102.00",
        "fixture",
    )
    .await;

    let current = fact_at(&pool, subject, "nav", valid(VALID_DAY))
        .await
        .expect("current")
        .expect("reinstated value");
    assert_eq!(decimal_value(&current), &decimal("102"));

    let mid_retraction = fact_as_known(
        &pool,
        subject,
        "nav",
        valid(VALID_DAY),
        known(RETRACT_KNOWN),
    )
    .await
    .expect("mid retraction");
    assert!(mid_retraction.is_none());

    guard.cleanup().await;
}

#[tokio::test]
async fn a_non_increasing_known_at_is_a_conflicting_append() {
    let Some((pool, guard)) = obolargus_core::common::tx_pool().await else {
        return;
    };
    let subject = seed_asset(&pool).await;

    append(
        &pool,
        subject,
        valid(VALID_DAY),
        known(FIRST_KNOWN),
        "100.00",
        "fixture",
    )
    .await;

    let error = write_fact(
        &pool,
        FactInput {
            subject,
            fact_type: "nav".to_string(),
            valid_at: valid(VALID_DAY),
            known_at: known(FIRST_KNOWN),
            source: "fixture".to_string(),
            value: FactValue::Decimal {
                value: decimal("101.00"),
            },
        },
    )
    .await
    .expect_err("a different value at an identical instant must conflict");
    assert!(matches!(error, TemporalError::ConflictingAppend { .. }));

    guard.cleanup().await;
}

#[tokio::test]
async fn identical_values_are_idempotent_regardless_of_decimal_format() {
    let Some((pool, guard)) = obolargus_core::common::tx_pool().await else {
        return;
    };
    let subject = seed_asset(&pool).await;

    append(
        &pool,
        subject,
        valid(VALID_DAY),
        known(FIRST_KNOWN),
        "100.0",
        "fixture",
    )
    .await;
    append(
        &pool,
        subject,
        valid(VALID_DAY),
        known(RESTATED_KNOWN),
        "100.00",
        "fixture",
    )
    .await;

    let history = fact_history(&pool, subject, "nav", valid(VALID_DAY))
        .await
        .expect("history");
    assert_eq!(history.len(), 1, "identical typed values must not revise");

    guard.cleanup().await;
}

#[tokio::test]
async fn two_sources_cannot_overwrite_each_other() {
    let Some((pool, guard)) = obolargus_core::common::tx_pool().await else {
        return;
    };
    let subject = seed_asset(&pool).await;

    append(
        &pool,
        subject,
        valid(VALID_DAY),
        known(FIRST_KNOWN),
        "100.00",
        "source-a",
    )
    .await;
    append(
        &pool,
        subject,
        valid(VALID_DAY),
        known(RESTATED_KNOWN),
        "101.00",
        "source-b",
    )
    .await;

    let history = fact_history(&pool, subject, "nav", valid(VALID_DAY))
        .await
        .expect("history");
    assert_eq!(history.len(), 2, "source is part of the identity key");

    guard.cleanup().await;
}

#[tokio::test]
async fn an_unknown_fact_type_is_rejected() {
    let Some((pool, guard)) = obolargus_core::common::tx_pool().await else {
        return;
    };
    let subject = seed_asset(&pool).await;

    let error = write_fact(
        &pool,
        FactInput {
            subject,
            fact_type: "not_a_fact".to_string(),
            valid_at: valid(VALID_DAY),
            known_at: known(FIRST_KNOWN),
            source: "fixture".to_string(),
            value: FactValue::Text { value: "x".into() },
        },
    )
    .await
    .expect_err("unknown fact type");
    assert!(matches!(error, TemporalError::UnknownFactType { .. }));

    guard.cleanup().await;
}

#[tokio::test]
async fn series_pages_forward_and_reports_more() {
    let Some((pool, _guard)) = obolargus_core::common::tx_pool().await else {
        return;
    };
    let subject = seed_asset(&pool).await;
    for day in ["2026-01-02", "2026-01-05", "2026-01-06"] {
        append(
            &pool,
            subject,
            valid(&format!("{day}T00:00:00Z")),
            known("2026-01-07T12:00:00Z"),
            "1.00",
            "test",
        )
        .await;
    }

    let window = TemporalWindow::new(valid("2026-01-01T00:00:00Z"), valid("2026-02-01T00:00:00Z"))
        .expect("window");
    let first = fact_series_page(&pool, subject, "nav", window, None, None, 2)
        .await
        .expect("first page");
    assert_eq!(first.points.len(), 2);
    assert!(first.has_more);
    let cursor = first.points.last().expect("cursor").valid_at;
    let second = fact_series_page(&pool, subject, "nav", window, None, Some(cursor), 2)
        .await
        .expect("second page");
    assert_eq!(second.points.len(), 1);
    assert!(!second.has_more);
}
