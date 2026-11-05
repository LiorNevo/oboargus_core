//! Integration tests for the operator-facing temporal verification helpers.
//!
//! Covers `verify_parity` across all three fact subjects and
//! `verify_lookahead` against real stored beliefs. Database-gated through the
//! shared `tx_pool` fixture.

use chrono::{DateTime, Utc};
use obolargus_core::common::{seed_temporal_fixture, tx_pool};
use obolargus_core::domain::id::{AssetId, DomainId, IssuerId, PortfolioId};
use obolargus_core::temporal::verify::{verify_lookahead, verify_parity};
use obolargus_core::temporal::{FactInput, FactValue, write_fact};
use obolargus_core::temporal::{FactSubject, KnownAt, ValidAt};
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

async fn seed_subject(pool: &PgPool, insert: &'static str, id: Uuid) -> FactSubject {
    sqlx::query(insert)
        .bind(id)
        .execute(pool)
        .await
        .expect("seed subject");
    match insert {
        _ if insert.contains("issuers") => FactSubject::Issuer(IssuerId::from_uuid(id)),
        _ if insert.contains("portfolios") => FactSubject::Portfolio(PortfolioId::from_uuid(id)),
        _ => FactSubject::Asset(AssetId::from_uuid(id)),
    }
}

#[tokio::test]
async fn parity_verifies_asset_issuer_and_portfolio_facts() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let asset_id = Uuid::new_v4();
    let issuer_id = Uuid::new_v4();
    let portfolio_id = Uuid::new_v4();

    let asset = seed_subject(
        &pool,
        "INSERT INTO assets (id, asset_type, display_name, currency) \
         VALUES ($1, 'stock', 'Parity Asset', 'USD')",
        asset_id,
    )
    .await;
    let issuer = seed_subject(
        &pool,
        "INSERT INTO issuers (id, name, issuer_type, legal_jurisdiction) \
         VALUES ($1, 'Parity Issuer', 'CORPORATION', 'US')",
        issuer_id,
    )
    .await;
    let portfolio = seed_subject(
        &pool,
        "INSERT INTO portfolios (id, name, purpose, created_by) \
         VALUES ($1, 'Parity Portfolio', 'testing', 'fixture')",
        portfolio_id,
    )
    .await;

    for (subject, fact_type, value) in [
        (
            asset,
            "nav",
            FactValue::Decimal {
                value: Decimal::from_str("100.00").expect("decimal"),
            },
        ),
        (
            issuer,
            "credit_rating",
            FactValue::Text { value: "AA".into() },
        ),
        (
            portfolio,
            "benchmark",
            FactValue::Text {
                value: "SPX".into(),
            },
        ),
    ] {
        write_fact(
            &pool,
            FactInput {
                subject,
                fact_type: fact_type.to_string(),
                valid_at: valid("2026-01-02T00:00:00Z"),
                known_at: known("2026-01-02T12:00:00Z"),
                source: "fixture".to_string(),
                value,
            },
        )
        .await
        .expect("append fact");
    }

    let report = verify_parity(&pool, None, None)
        .await
        .expect("parity report");
    assert_eq!(report.checks, 3);
    assert!(report.is_ok(), "{report:?}");

    guard.cleanup().await;
}

#[tokio::test]
async fn parity_date_filters_and_lookahead_samples_are_bounded() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset = seed_subject(
        &pool,
        "INSERT INTO assets (id, asset_type, display_name, currency) \
         VALUES ($1, 'stock', 'Filtered Asset', 'USD')",
        Uuid::new_v4(),
    )
    .await;
    write_fact(
        &pool,
        FactInput {
            subject: asset,
            fact_type: "nav".to_string(),
            valid_at: valid("2026-01-02T00:00:00Z"),
            known_at: known("2026-01-02T12:00:00Z"),
            source: "fixture".to_string(),
            value: FactValue::Decimal {
                value: Decimal::from_str("100.00").expect("decimal"),
            },
        },
    )
    .await
    .expect("append fact");

    let after_everything = verify_parity(
        &pool,
        Some(chrono::NaiveDate::from_ymd_opt(2027, 1, 1).expect("date")),
        None,
    )
    .await
    .expect("future-filtered parity");
    assert_eq!(after_everything.checks, 0);
    assert!(after_everything.is_ok());

    let before_everything = verify_parity(
        &pool,
        None,
        Some(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).expect("date")),
    )
    .await
    .expect("past-filtered parity");
    assert_eq!(before_everything.checks, 0);

    let lookahead = verify_lookahead(&pool, 10).await.expect("lookahead");
    assert!(lookahead.checks > 0);
    assert!(lookahead.is_ok(), "{lookahead:?}");

    let minimum_sample = verify_lookahead(&pool, 0).await.expect("minimum sample");
    assert_eq!(minimum_sample.checks, 1);
    assert!(minimum_sample.is_ok());

    guard.cleanup().await;
}

#[tokio::test]
async fn seeded_temporal_fixture_is_consistent() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    seed_temporal_fixture(&pool).await;

    let parity = verify_parity(&pool, None, None)
        .await
        .expect("seeded parity");
    assert!(parity.checks > 0);
    assert!(parity.is_ok(), "{parity:?}");

    let lookahead = verify_lookahead(&pool, 10).await.expect("seeded lookahead");
    assert!(lookahead.checks > 0);
    assert!(lookahead.is_ok(), "{lookahead:?}");

    guard.cleanup().await;
}
