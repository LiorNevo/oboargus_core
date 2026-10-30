//! Integration tests for the bitemporal fact contract (US1).
//!
//! Covers `specs/021-bitemporal-market-data/quickstart.md` §2–§4:
//! same-day revision, as-known before the first record, retraction
//! visibility, and reinstatement. Database-gated: returns early when the
//! fixture cannot connect.

use chrono::{DateTime, Utc};
use obolargus_core::domain::id::{AssetId, DomainId};
use obolargus_core::temporal::facts::read::{
    fact_as_known, fact_at, fact_history, fact_series_page,
};
use obolargus_core::temporal::facts::write::{append_fact, revoke_fact};
use obolargus_core::temporal::{FactSubject, KnownAt, TemporalError, TemporalWindow, ValidAt};
use sqlx::PgPool;
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

    append_fact(
        &pool,
        subject,
        "market_price",
        valid(VALID_DAY),
        known(FIRST_KNOWN),
        "100.00",
        "fixture",
    )
    .await
    .expect("first belief");

    append_fact(
        &pool,
        subject,
        "market_price",
        valid(VALID_DAY),
        known(RESTATED_KNOWN),
        "101.00",
        "fixture",
    )
    .await
    .expect("same-day restatement");

    let current = fact_at(&pool, subject, "market_price", valid(VALID_DAY))
        .await
        .expect("read current")
        .expect("value in force");
    assert_eq!(current.value, "101.00");

    let earlier = fact_as_known(
        &pool,
        subject,
        "market_price",
        valid(VALID_DAY),
        known(FIRST_KNOWN),
    )
    .await
    .expect("read as known")
    .expect("earlier belief is still visible");
    assert_eq!(earlier.value, "100.00");

    let history = fact_history(&pool, subject, "market_price", valid(VALID_DAY))
        .await
        .expect("history");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].value, "101.00");
    assert_eq!(history[1].value, "100.00");

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
        "market_price",
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

    append_fact(
        &pool,
        subject,
        "market_price",
        valid(VALID_DAY),
        known(FIRST_KNOWN),
        "100.00",
        "fixture",
    )
    .await
    .expect("belief");

    revoke_fact(
        &pool,
        subject,
        "market_price",
        valid(VALID_DAY),
        known(RETRACT_KNOWN),
        "delisted",
        "fixture",
    )
    .await
    .expect("retraction");

    assert!(
        fact_at(&pool, subject, "market_price", valid(VALID_DAY))
            .await
            .expect("current")
            .is_none(),
        "a retracted winner must resolve to no value"
    );

    assert!(
        fact_as_known(
            &pool,
            subject,
            "market_price",
            valid(VALID_DAY),
            known(RETRACT_KNOWN),
        )
        .await
        .expect("as known at retraction")
        .is_none()
    );

    let before_retraction = fact_as_known(
        &pool,
        subject,
        "market_price",
        valid(VALID_DAY),
        known(FIRST_KNOWN),
    )
    .await
    .expect("as known before retraction")
    .expect("the older belief is untouched");
    assert_eq!(before_retraction.value, "100.00");

    guard.cleanup().await;
}

#[tokio::test]
async fn a_retracted_value_can_be_reinstated_by_a_later_belief() {
    let Some((pool, guard)) = obolargus_core::common::tx_pool().await else {
        return;
    };
    let subject = seed_asset(&pool).await;

    append_fact(
        &pool,
        subject,
        "market_price",
        valid(VALID_DAY),
        known(FIRST_KNOWN),
        "100.00",
        "fixture",
    )
    .await
    .expect("belief");
    revoke_fact(
        &pool,
        subject,
        "market_price",
        valid(VALID_DAY),
        known(RETRACT_KNOWN),
        "delisted",
        "fixture",
    )
    .await
    .expect("retraction");
    append_fact(
        &pool,
        subject,
        "market_price",
        valid(VALID_DAY),
        known(REINSTATE_KNOWN),
        "102.00",
        "fixture",
    )
    .await
    .expect("reinstatement");

    let current = fact_at(&pool, subject, "market_price", valid(VALID_DAY))
        .await
        .expect("current")
        .expect("reinstated value");
    assert_eq!(current.value, "102.00");

    let mid_retraction = fact_as_known(
        &pool,
        subject,
        "market_price",
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

    append_fact(
        &pool,
        subject,
        "market_price",
        valid(VALID_DAY),
        known(FIRST_KNOWN),
        "100.00",
        "fixture",
    )
    .await
    .expect("belief");

    let error = append_fact(
        &pool,
        subject,
        "market_price",
        valid(VALID_DAY),
        known(FIRST_KNOWN),
        "100.00",
        "fixture",
    )
    .await
    .expect_err("an identical instant must not create a second row");
    assert!(matches!(error, TemporalError::ConflictingAppend { .. }));

    guard.cleanup().await;
}

#[tokio::test]
async fn two_sources_cannot_overwrite_each_other() {
    let Some((pool, guard)) = obolargus_core::common::tx_pool().await else {
        return;
    };
    let subject = seed_asset(&pool).await;

    append_fact(
        &pool,
        subject,
        "market_price",
        valid(VALID_DAY),
        known(FIRST_KNOWN),
        "100.00",
        "source-a",
    )
    .await
    .expect("source a");
    append_fact(
        &pool,
        subject,
        "market_price",
        valid(VALID_DAY),
        known(RESTATED_KNOWN),
        "101.00",
        "source-b",
    )
    .await
    .expect("source b at the same instant, different source");

    let history = fact_history(&pool, subject, "market_price", valid(VALID_DAY))
        .await
        .expect("history");
    assert_eq!(history.len(), 2, "source is part of the identity key");

    guard.cleanup().await;
}

#[tokio::test]
async fn series_pages_forward_and_reports_more() {
    let Some((pool, _guard)) = obolargus_core::common::tx_pool().await else {
        return;
    };
    let subject = seed_asset(&pool).await;
    for day in ["2026-01-02", "2026-01-05", "2026-01-06"] {
        append_fact(
            &pool,
            subject,
            "market_price",
            valid(&format!("{day}T00:00:00Z")),
            known("2026-01-07T12:00:00Z"),
            r#"{"close":"1.00"}"#,
            "test",
        )
        .await
        .expect("append");
    }

    let window = TemporalWindow::new(valid("2026-01-01T00:00:00Z"), valid("2026-02-01T00:00:00Z"))
        .expect("window");
    let first = fact_series_page(&pool, subject, "market_price", window, None, None, 2)
        .await
        .expect("first page");
    assert_eq!(first.points.len(), 2);
    assert!(first.has_more);
    let cursor = first.points.last().expect("cursor").valid_at;
    let second = fact_series_page(
        &pool,
        subject,
        "market_price",
        window,
        None,
        Some(cursor),
        2,
    )
    .await
    .expect("second page");
    assert_eq!(second.points.len(), 1);
    assert!(!second.has_more);
}
