//! Integration tests for the normalized fact model.
//!
//! Database-gated through the shared `tx_pool` fixture.

use chrono::{DateTime, NaiveDate, Utc};
use obolargus_core::common::tx_pool;
use obolargus_core::domain::id::{AssetId, DomainId, IssuerId, PortfolioId};
use obolargus_core::temporal::catalog::fact_catalog;
use obolargus_core::temporal::facts::read::{fact_at, fact_coverage, fact_history, fact_series};
use obolargus_core::temporal::facts::write::write_fact;
use obolargus_core::temporal::observations::{
    NewMarketObservation, append_observation, ensure_market_observation_partitions,
    observation_range, retract_observation,
};
use obolargus_core::temporal::verify::verify_parity;
use obolargus_core::temporal::{
    Adjustment, FactInput, FactSubject, FactValue, KnownAt, MarketPriceValue, RetractionInput,
    TemporalError, TemporalWindow, ValidAt, retract_fact,
};
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::str::FromStr;
use uuid::Uuid;

fn date(raw: &str) -> NaiveDate {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d").expect("date")
}

fn valid(raw: &str) -> ValidAt {
    ValidAt::new(
        format!("{raw}T00:00:00Z")
            .parse::<DateTime<Utc>>()
            .expect("instant"),
    )
    .expect("valid")
}

fn known(raw: &str) -> KnownAt {
    KnownAt::new(raw.parse::<DateTime<Utc>>().expect("instant")).expect("known")
}

fn decimal(raw: &str) -> Decimal {
    Decimal::from_str(raw).expect("decimal")
}

async fn seed_asset(pool: &PgPool, name: &str) -> FactSubject {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO assets (id, asset_type, display_name, currency) \
         VALUES ($1, 'stock', $2, 'USD')",
    )
    .bind(id)
    .bind(name)
    .execute(pool)
    .await
    .expect("seed asset");
    FactSubject::Asset(AssetId::from_uuid(id))
}

async fn seed_issuer(pool: &PgPool) -> FactSubject {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO issuers (id, name, issuer_type, legal_jurisdiction) \
         VALUES ($1, 'Issuer', 'CORPORATION', 'US')",
    )
    .bind(id)
    .execute(pool)
    .await
    .expect("seed issuer");
    FactSubject::Issuer(IssuerId::from_uuid(id))
}

async fn seed_portfolio(pool: &PgPool) -> FactSubject {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO portfolios (id, name, purpose, created_by) \
         VALUES ($1, 'Portfolio', 'testing', 'fixture')",
    )
    .bind(id)
    .execute(pool)
    .await
    .expect("seed portfolio");
    FactSubject::Portfolio(PortfolioId::from_uuid(id))
}

async fn write(
    pool: &PgPool,
    subject: FactSubject,
    fact_type: &str,
    valid_at: ValidAt,
    known_at: KnownAt,
    value: FactValue,
) {
    write_fact(
        pool,
        FactInput {
            subject,
            fact_type: fact_type.to_string(),
            valid_at,
            known_at,
            source: "fixture".to_string(),
            value,
        },
    )
    .await
    .expect("write fact");
}

#[tokio::test]
async fn every_scalar_storage_kind_round_trips() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset = seed_asset(&pool, "Scalar Asset").await;
    let portfolio = seed_portfolio(&pool).await;
    let day = valid("2026-01-02");
    let moment = known("2026-01-02T12:00:00Z");

    write(
        &pool,
        asset,
        "name",
        day,
        moment,
        FactValue::Text {
            value: "Apple Inc.".into(),
        },
    )
    .await;
    write(
        &pool,
        asset,
        "nav",
        day,
        moment,
        FactValue::Decimal {
            value: decimal("12.34"),
        },
    )
    .await;
    write(
        &pool,
        asset,
        "is_active",
        day,
        moment,
        FactValue::Boolean { value: true },
    )
    .await;
    write(
        &pool,
        asset,
        "rank",
        day,
        moment,
        FactValue::Integer { value: 7 },
    )
    .await;
    write(
        &pool,
        asset,
        "last_verified_at",
        day,
        moment,
        FactValue::Timestamp {
            value: moment.as_datetime(),
        },
    )
    .await;
    write(
        &pool,
        portfolio,
        "inception_date",
        day,
        moment,
        FactValue::Date {
            value: date("2020-01-01"),
        },
    )
    .await;

    assert!(matches!(
        fact_at(&pool, asset, "name", day)
            .await
            .expect("read")
            .expect("value")
            .value,
        Some(FactValue::Text { .. })
    ));
    assert!(matches!(
        fact_at(&pool, asset, "nav", day)
            .await
            .expect("read")
            .expect("value")
            .value,
        Some(FactValue::Decimal { .. })
    ));
    assert!(matches!(
        fact_at(&pool, asset, "is_active", day)
            .await
            .expect("read")
            .expect("value")
            .value,
        Some(FactValue::Boolean { value: true })
    ));
    assert!(matches!(
        fact_at(&pool, asset, "rank", day)
            .await
            .expect("read")
            .expect("value")
            .value,
        Some(FactValue::Integer { value: 7 })
    ));
    assert!(matches!(
        fact_at(&pool, asset, "last_verified_at", day)
            .await
            .expect("read")
            .expect("value")
            .value,
        Some(FactValue::Timestamp { .. })
    ));
    assert!(matches!(
        fact_at(&pool, portfolio, "inception_date", day)
            .await
            .expect("read")
            .expect("value")
            .value,
        Some(FactValue::Date { .. })
    ));

    let coverage = fact_coverage(&pool, asset).await.expect("coverage");
    assert!(coverage.iter().any(|entry| entry.fact_type == "nav"));
    assert!(coverage.iter().any(|entry| entry.fact_type == "name"));
    assert!(coverage.iter().any(|entry| entry.fact_type == "rank"));

    guard.cleanup().await;
}

#[tokio::test]
async fn market_price_observations_round_trip_and_revise() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset = seed_asset(&pool, "Price Asset").await;
    let day = valid("2026-01-02");
    let first = known("2026-01-02T12:00:00Z");
    let second = known("2026-01-03T12:00:00Z");

    let price = |close: &str| {
        FactValue::MarketPrice(MarketPriceValue {
            open: decimal(close),
            high: decimal(close),
            low: decimal(close),
            close: decimal(close),
            adjclose: Some(decimal(close)),
            volume: Some(decimal("1000")),
            currency: "USD".into(),
            adjustment: Adjustment::Unadjusted,
        })
    };

    write(&pool, asset, "market_price", day, first, price("100")).await;
    let read = fact_at(&pool, asset, "market_price", day)
        .await
        .expect("read")
        .expect("value");
    let FactValue::MarketPrice(value) = read.value.expect("value") else {
        panic!("expected market price");
    };
    assert_eq!(value.close, decimal("100"));

    // Same value at a later known_at is an idempotent no-op.
    write(&pool, asset, "market_price", day, second, price("100")).await;
    let history = fact_history(&pool, asset, "market_price", day)
        .await
        .expect("history");
    assert_eq!(history.len(), 1);

    // A different value at a later known_at is a correction.
    let third = known("2026-01-04T12:00:00Z");
    write(&pool, asset, "market_price", day, third, price("101")).await;
    let history = fact_history(&pool, asset, "market_price", day)
        .await
        .expect("history");
    assert_eq!(history.len(), 2);
    let current = fact_at(&pool, asset, "market_price", day)
        .await
        .expect("read")
        .expect("value");
    let FactValue::MarketPrice(value) = current.value.expect("value") else {
        panic!("expected market price");
    };
    assert_eq!(value.close, decimal("101"));

    let window = TemporalWindow::new(valid("2026-01-01"), valid("2026-01-31")).expect("window");
    let series = fact_series(&pool, asset, "market_price", window, None)
        .await
        .expect("series");
    assert_eq!(series.len(), 1);
    assert_eq!(
        obolargus_core::temporal::facts::read::observation_series(
            &pool, asset, "fixture", window, None,
        )
        .await
        .expect("observation series")
        .len(),
        1
    );

    // Retraction.
    retract_fact(
        &pool,
        RetractionInput {
            subject: asset,
            fact_type: "market_price".to_string(),
            valid_at: day,
            known_at: known("2026-01-05T12:00:00Z"),
            source: "fixture".to_string(),
            reason: "delisted".to_string(),
        },
    )
    .await
    .expect("retract");
    assert!(
        fact_at(&pool, asset, "market_price", day)
            .await
            .expect("read")
            .is_none()
    );
    let history = fact_history(&pool, asset, "market_price", day)
        .await
        .expect("history");
    assert!(history.iter().any(|point| point.retracted));

    guard.cleanup().await;
}

#[tokio::test]
async fn reads_reject_an_unknown_subject_instead_of_returning_empty() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let missing = FactSubject::Asset(AssetId::from_uuid(Uuid::new_v4()));
    let window = TemporalWindow::new(valid("2026-01-01"), valid("2026-02-01")).expect("window");
    let error = fact_series(&pool, missing, "market_price", window, None)
        .await
        .expect_err("unknown subject");
    assert!(matches!(error, TemporalError::UnknownSubject { .. }));

    guard.cleanup().await;
}

#[tokio::test]
async fn invalid_shapes_and_subjects_are_rejected() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset = seed_asset(&pool, "Invalid Asset").await;
    let issuer = seed_issuer(&pool).await;
    let day = valid("2026-01-02");
    let moment = known("2026-01-02T12:00:00Z");

    let shape_error = write_fact(
        &pool,
        FactInput {
            subject: asset,
            fact_type: "nav".to_string(),
            valid_at: day,
            known_at: moment,
            source: "fixture".to_string(),
            value: FactValue::Text { value: "x".into() },
        },
    )
    .await
    .expect_err("wrong shape");
    assert!(matches!(
        shape_error,
        TemporalError::InvalidFactValue { .. }
    ));

    let subject_error = write_fact(
        &pool,
        FactInput {
            subject: issuer,
            fact_type: "market_price".to_string(),
            valid_at: day,
            known_at: moment,
            source: "fixture".to_string(),
            value: FactValue::MarketPrice(MarketPriceValue {
                open: Decimal::ONE,
                high: Decimal::ONE,
                low: Decimal::ONE,
                close: Decimal::ONE,
                adjclose: None,
                volume: None,
                currency: "USD".into(),
                adjustment: Adjustment::Unadjusted,
            }),
        },
    )
    .await
    .expect_err("wrong subject");
    assert!(matches!(
        subject_error,
        TemporalError::InvalidSubject { .. }
    ));

    let empty_text = write_fact(
        &pool,
        FactInput {
            subject: asset,
            fact_type: "name".to_string(),
            valid_at: day,
            known_at: moment,
            source: "fixture".to_string(),
            value: FactValue::Text { value: "  ".into() },
        },
    )
    .await
    .expect_err("empty text");
    assert!(matches!(empty_text, TemporalError::InvalidFactValue { .. }));

    guard.cleanup().await;
}

#[tokio::test]
#[allow(clippy::type_complexity)]
async fn catalog_projection_matches_the_rust_catalog() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let rows: Vec<(String, String, Option<String>, Option<i32>, i32)> = sqlx::query_as(
        "SELECT fact_type, storage_kind, observation_kind, interval_seconds, schema_version \
         FROM fact_types ORDER BY fact_type",
    )
    .fetch_all(&pool)
    .await
    .expect("catalog rows");

    let mut expected: Vec<_> = fact_catalog()
        .iter()
        .map(|def| {
            (
                def.fact_type.to_string(),
                def.storage_kind.as_str().to_string(),
                def.observation_kind.map(str::to_string),
                def.interval_seconds,
                def.schema_version,
            )
        })
        .collect();
    expected.sort();

    assert_eq!(rows, expected);

    let field_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fact_type_fields")
        .fetch_one(&pool)
        .await
        .expect("field count");
    let expected_fields: i64 = fact_catalog()
        .iter()
        .map(|def| i64::try_from(def.fields.len()).unwrap_or(0))
        .sum();
    assert_eq!(field_count, expected_fields);

    guard.cleanup().await;
}

#[tokio::test]
async fn observation_partition_helper_creates_a_month() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    ensure_market_observation_partitions(&pool, date("2030-01-01"), date("2030-01-31"))
        .await
        .expect("partitions");
    let exists: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('market_observations_2030_01')::text")
            .fetch_one(&pool)
            .await
            .expect("regclass");
    assert_eq!(exists.as_deref(), Some("market_observations_2030_01"));

    guard.cleanup().await;
}

#[tokio::test]
async fn verify_parity_detects_an_envelope_without_a_payload() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset = seed_asset(&pool, "Orphan Asset").await;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO temporal_facts \
         (id, asset_id, fact_type, valid_at, known_at, source, content_hash) \
         VALUES ($1, $2, 'name', '2026-01-02T00:00:00Z', '2026-01-02T12:00:00Z', 't', 'h')",
    )
    .bind(id)
    .bind(asset.id())
    .execute(&pool)
    .await
    .expect("insert orphan");

    let report = verify_parity(&pool, None, None).await.expect("parity");
    assert!(!report.is_ok());
    assert!(
        report
            .failures
            .iter()
            .any(|line| line.contains("no typed payload"))
    );

    guard.cleanup().await;
}

#[tokio::test]
async fn raw_observation_append_and_range_are_source_scoped() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let FactSubject::Asset(asset_id) = seed_asset(&pool, "Observation Asset").await else {
        unreachable!()
    };
    let input = NewMarketObservation {
        asset_id,
        kind: "eod".into(),
        interval_seconds: 0,
        valid_at: valid("2026-01-02"),
        known_at: known("2026-01-02T12:00:00Z"),
        source: "source-a".into(),
        open: Some(Decimal::ONE),
        high: Some(Decimal::ONE),
        low: Some(Decimal::ONE),
        close: Some(Decimal::ONE),
        adjclose: None,
        price: None,
        volume: Some(Decimal::ONE),
        currency: "USD".into(),
        adjustment: Adjustment::Unadjusted,
        sequence: None,
    };
    let row = append_observation(&pool, input.clone())
        .await
        .expect("append");
    assert!(!row.retracted);
    let again = append_observation(&pool, input)
        .await
        .expect("idempotent append");
    assert_eq!(again.id, row.id);

    let window = TemporalWindow::new(valid("2026-01-01"), valid("2026-01-31")).expect("window");
    let series = observation_range(&pool, asset_id, "eod", 0, "source-a", window, None)
        .await
        .expect("range");
    assert_eq!(series.len(), 1);

    let other = observation_range(&pool, asset_id, "eod", 0, "source-b", window, None)
        .await
        .expect("range");
    assert!(other.is_empty());

    retract_observation(
        &pool,
        NewMarketObservation {
            known_at: known("2026-01-03T12:00:00Z"),
            ..input_for_retraction(asset_id, valid("2026-01-02"))
        },
        "gone",
    )
    .await
    .expect("retract");
    let series = observation_range(&pool, asset_id, "eod", 0, "source-a", window, None)
        .await
        .expect("range");
    assert!(series.is_empty());

    guard.cleanup().await;
}

fn input_for_retraction(asset_id: AssetId, valid_at: ValidAt) -> NewMarketObservation {
    NewMarketObservation {
        asset_id,
        kind: "eod".into(),
        interval_seconds: 0,
        valid_at,
        known_at: known("2026-01-03T12:00:00Z"),
        source: "source-a".into(),
        open: None,
        high: None,
        low: None,
        close: None,
        adjclose: None,
        price: None,
        volume: None,
        currency: "USD".into(),
        adjustment: Adjustment::None,
        sequence: None,
    }
}
