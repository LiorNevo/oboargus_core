//! Database-backed tests for the bundled calendar seed and coverage read.
//!
//! Skipped as a passing no-op when no test database is configured.

use chrono::{DateTime, NaiveDate, Utc};

use obolargus_core::common::tx_pool;
use obolargus_core::domain::asset::create::create_asset_with_identifier;
use obolargus_core::domain::asset::entity::NewAsset;
use obolargus_core::market_data::calendar::{asset_coverage, seed_bundled_calendars};

fn date(value: &str) -> NaiveDate {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("date")
}

fn now(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("timestamp")
        .with_timezone(&Utc)
}

#[tokio::test]
async fn seed_is_idempotent_and_creates_no_demo_prices() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let first = seed_bundled_calendars(&pool).await.expect("seed");
    assert_eq!(first, 6);
    let second = seed_bundled_calendars(&pool).await.expect("seed again");
    assert_eq!(second, 0);
    let calendars: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM exchange_calendars")
        .fetch_one(&pool)
        .await
        .expect("calendar count");
    assert_eq!(calendars, 6);
    let observations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM market_observations")
        .fetch_one(&pool)
        .await
        .expect("observation count");
    assert_eq!(observations, 0);
    guard.cleanup().await;
}

#[tokio::test]
async fn coverage_uses_the_seeded_calendar_and_reports_missing_sessions() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    seed_bundled_calendars(&pool).await.expect("seed");
    let asset = create_asset_with_identifier(
        &pool,
        NewAsset {
            asset_type: "stock".into(),
            display_name: "Test US Equity".into(),
            currency: "USD".into(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
        "TICKER:XNAS",
        "TEST",
    )
    .await
    .expect("asset");

    let coverage = asset_coverage(
        &pool,
        asset.id(),
        date("2026-11-16"),
        date("2026-11-20"),
        now("2026-11-24T17:00:00Z"),
    )
    .await
    .expect("coverage");

    assert_eq!(coverage.segments.len(), 1);
    assert_eq!(coverage.segments[0].mic.as_deref(), Some("XNAS"));
    assert!(coverage.segments[0].exact);
    assert_eq!(coverage.expected_session_count, 5);
    assert_eq!(coverage.missing_sessions.len(), 5);
    assert_eq!(coverage.latest_completed_session, Some(date("2026-11-23")));
    guard.cleanup().await;
}

use obolargus_core::domain::id::AssetId;
use obolargus_core::temporal::{
    Adjustment, FactInput, FactSubject, FactValue, KnownAt, MarketPriceValue, RetractionInput,
    ValidAt, retract_fact, write_fact_outcome,
};
use rust_decimal::Decimal;

async fn store_bar(pool: &sqlx::PgPool, asset: AssetId, value: &str) {
    write_fact_outcome(
        pool,
        FactInput {
            subject: FactSubject::Asset(asset),
            fact_type: "market_price".to_string(),
            valid_at: ValidAt::midnight_utc(date(value)),
            known_at: KnownAt::now(),
            source: "test".to_string(),
            value: FactValue::MarketPrice(MarketPriceValue {
                open: Decimal::new(100, 0),
                high: Decimal::new(101, 0),
                low: Decimal::new(99, 0),
                close: Decimal::new(100, 0),
                adjclose: Some(Decimal::new(100, 0)),
                volume: Some(Decimal::new(1_000_000, 0)),
                currency: "USD".to_string(),
                adjustment: Adjustment::Unadjusted,
            }),
        },
    )
    .await
    .expect("stored bar");
}

#[tokio::test]
async fn stored_and_retracted_dates_match_the_series_resolver() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    seed_bundled_calendars(&pool).await.expect("seed");
    let asset = create_asset_with_identifier(
        &pool,
        NewAsset {
            asset_type: "stock".into(),
            display_name: "Coverage Equity".into(),
            currency: "USD".into(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
        "TICKER:XNAS",
        "COVERAGE",
    )
    .await
    .expect("asset");
    for value in ["2026-11-16", "2026-11-17", "2026-11-18"] {
        store_bar(&pool, asset.id(), value).await;
    }
    retract_fact(
        &pool,
        RetractionInput {
            subject: FactSubject::Asset(asset.id()),
            fact_type: "market_price".to_string(),
            valid_at: ValidAt::midnight_utc(date("2026-11-18")),
            known_at: KnownAt::now(),
            source: "test".to_string(),
            reason: "correction".to_string(),
        },
    )
    .await
    .expect("retract");

    let coverage = asset_coverage(
        &pool,
        asset.id(),
        date("2026-11-16"),
        date("2026-11-20"),
        now("2026-11-24T17:00:00Z"),
    )
    .await
    .expect("coverage");
    assert_eq!(
        coverage.missing_sessions,
        vec![date("2026-11-18"), date("2026-11-19"), date("2026-11-20")]
    );
    assert_eq!(coverage.expected_session_count, 5);
    assert_eq!(coverage.latest_completed_session, Some(date("2026-11-23")));

    let long = asset_coverage(
        &pool,
        asset.id(),
        date("2022-01-01"),
        date("2026-12-31"),
        now("2027-01-04T17:00:00Z"),
    )
    .await
    .expect("long coverage");
    assert!(!long.missing_sessions.contains(&date("2025-11-27")));
    assert!(!long.missing_sessions.contains(&date("2026-07-03")));
    assert_eq!(
        long.missing_sessions.len(),
        long.expected_session_count - 2,
        "only 2026-11-16 and 2026-11-17 are stored; the retracted 18 is missing"
    );
    assert!(long.missing_sessions.contains(&date("2026-11-18")));
    guard.cleanup().await;
}
