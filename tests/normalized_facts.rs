//! Integration tests for the normalized fact model.
//!
//! Database-gated through the shared `tx_pool` fixture.

use chrono::{DateTime, NaiveDate, Utc};
use obolargus_core::common::tx_pool;
use obolargus_core::domain::id::{AssetId, DomainId, IssuerId, PortfolioId};
use obolargus_core::temporal::catalog::fact_catalog;
use obolargus_core::temporal::facts::read::{
    fact_as_known, fact_at, fact_coverage, fact_history, fact_point_by_id, fact_series,
    fact_series_page, observation_series,
};
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

#[tokio::test]
async fn fact_writes_and_retractions_reject_invalid_inputs() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset = seed_asset(&pool, "Invalid Write Asset").await;
    let issuer = seed_issuer(&pool).await;
    let portfolio = seed_portfolio(&pool).await;
    let day = valid("2026-01-02");
    let moment = known("2026-01-02T12:00:00Z");

    let unknown = write_fact(
        &pool,
        FactInput {
            subject: asset,
            fact_type: "does_not_exist".to_string(),
            valid_at: day,
            known_at: moment,
            source: "fixture".to_string(),
            value: FactValue::Text { value: "x".into() },
        },
    )
    .await
    .expect_err("unknown fact type");
    assert!(matches!(unknown, TemporalError::UnknownFactType { .. }));

    let empty_source = write_fact(
        &pool,
        FactInput {
            subject: asset,
            fact_type: "name".to_string(),
            valid_at: day,
            known_at: moment,
            source: "   ".to_string(),
            value: FactValue::Text { value: "x".into() },
        },
    )
    .await
    .expect_err("empty source");
    assert!(matches!(
        empty_source,
        TemporalError::InvalidFactValue { .. }
    ));

    let first_belief = write_fact(
        &pool,
        FactInput {
            subject: issuer,
            fact_type: "credit_rating".to_string(),
            valid_at: day,
            known_at: moment,
            source: "fixture".to_string(),
            value: FactValue::Text { value: "AA".into() },
        },
    )
    .await
    .expect("first belief");
    write_fact(
        &pool,
        FactInput {
            subject: issuer,
            fact_type: "credit_rating".to_string(),
            valid_at: day,
            known_at: moment,
            source: "fixture".to_string(),
            value: FactValue::Text { value: "AA".into() },
        },
    )
    .await
    .expect("idempotent re-write");
    let conflict = write_fact(
        &pool,
        FactInput {
            subject: issuer,
            fact_type: "credit_rating".to_string(),
            valid_at: day,
            known_at: moment,
            source: "fixture".to_string(),
            value: FactValue::Text { value: "BB".into() },
        },
    )
    .await
    .expect_err("same known_at is a conflict");
    assert!(matches!(conflict, TemporalError::ConflictingAppend { .. }));
    assert_eq!(
        fact_point_by_id(&pool, Uuid::new_v4())
            .await
            .expect("missing id"),
        None
    );

    let unknown_retraction = retract_fact(
        &pool,
        RetractionInput {
            subject: asset,
            fact_type: "does_not_exist".to_string(),
            valid_at: day,
            known_at: moment,
            source: "fixture".to_string(),
            reason: "gone".to_string(),
        },
    )
    .await
    .expect_err("unknown fact type");
    assert!(matches!(
        unknown_retraction,
        TemporalError::UnknownFactType { .. }
    ));

    let empty_reason = retract_fact(
        &pool,
        RetractionInput {
            subject: asset,
            fact_type: "name".to_string(),
            valid_at: day,
            known_at: moment,
            source: "fixture".to_string(),
            reason: "  ".to_string(),
        },
    )
    .await
    .expect_err("empty reason");
    assert!(matches!(
        empty_reason,
        TemporalError::InvalidFactValue { .. }
    ));

    let missing = retract_fact(
        &pool,
        RetractionInput {
            subject: portfolio,
            fact_type: "benchmark".to_string(),
            valid_at: day,
            known_at: moment,
            source: "fixture".to_string(),
            reason: "gone".to_string(),
        },
    )
    .await
    .expect_err("no value");
    assert!(matches!(missing, TemporalError::NoValue { .. }));

    write(
        &pool,
        issuer,
        "credit_rating",
        day,
        known("2026-01-03T12:00:00Z"),
        FactValue::Text { value: "AA".into() },
    )
    .await;
    let retraction = RetractionInput {
        subject: issuer,
        fact_type: "credit_rating".to_string(),
        valid_at: day,
        known_at: known("2026-01-04T12:00:00Z"),
        source: "fixture".to_string(),
        reason: "downgrade".to_string(),
    };
    retract_fact(&pool, retraction.clone())
        .await
        .expect("retract");
    retract_fact(&pool, retraction.clone())
        .await
        .expect("idempotent retraction");
    let retraction_conflict = retract_fact(
        &pool,
        RetractionInput {
            reason: "different reason".to_string(),
            known_at: known("2026-01-03T12:00:00Z"),
            ..retraction
        },
    )
    .await
    .expect_err("older retraction conflicts");
    assert!(matches!(
        retraction_conflict,
        TemporalError::ConflictingAppend { .. }
    ));

    // The earlier conflict was a read-only check: history still holds one belief
    // plus the retraction, and the pre-retraction belief is readable as known.
    let as_known = fact_as_known(
        &pool,
        issuer,
        "credit_rating",
        day,
        known("2026-01-03T12:00:00Z"),
    )
    .await
    .expect("as known")
    .expect("belief");
    assert_eq!(as_known.id, first_belief.id);
    assert_eq!(as_known.value, Some(FactValue::Text { value: "AA".into() }));
    assert_eq!(
        fact_history(&pool, issuer, "credit_rating", day)
            .await
            .expect("history")
            .len(),
        2
    );
    assert!(
        fact_at(&pool, issuer, "credit_rating", day)
            .await
            .expect("current")
            .is_none()
    );

    guard.cleanup().await;
}

#[tokio::test]
async fn fact_reads_cover_subjects_pagination_and_missing_payloads() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let asset = seed_asset(&pool, "Read Paths Asset").await;
    let issuer = seed_issuer(&pool).await;
    let portfolio = seed_portfolio(&pool).await;
    let window = TemporalWindow::new(valid("2026-01-01"), valid("2026-02-01")).expect("window");

    // Every subject kind resolves its own table before returning empty.
    for subject in [issuer, portfolio] {
        assert!(
            fact_series(&pool, subject, "name", window, None)
                .await
                .expect("empty series")
                .is_empty()
        );
        assert!(
            fact_coverage(&pool, subject)
                .await
                .expect("empty coverage")
                .is_empty()
        );
    }
    let missing_issuer = FactSubject::Issuer(IssuerId::from_uuid(Uuid::new_v4()));
    assert!(matches!(
        fact_at(&pool, missing_issuer, "credit_rating", valid("2026-01-02"))
            .await
            .expect_err("missing issuer"),
        TemporalError::UnknownSubject { .. }
    ));
    let missing_portfolio = FactSubject::Portfolio(PortfolioId::from_uuid(Uuid::new_v4()));
    assert!(matches!(
        fact_coverage(&pool, missing_portfolio)
            .await
            .expect_err("missing portfolio"),
        TemporalError::UnknownSubject { .. }
    ));

    assert!(matches!(
        fact_at(&pool, asset, "does_not_exist", valid("2026-01-02"))
            .await
            .expect_err("unknown fact type"),
        TemporalError::UnknownFactType { .. }
    ));
    assert!(matches!(
        fact_as_known(
            &pool,
            asset,
            "does_not_exist",
            valid("2026-01-02"),
            known("2026-01-02T12:00:00Z"),
        )
        .await
        .expect_err("unknown fact type"),
        TemporalError::UnknownFactType { .. }
    ));
    assert!(matches!(
        fact_history(&pool, asset, "does_not_exist", valid("2026-01-02"))
            .await
            .expect_err("unknown fact type"),
        TemporalError::UnknownFactType { .. }
    ));
    assert!(matches!(
        fact_series_page(&pool, asset, "does_not_exist", window, None, None, 10,)
            .await
            .expect_err("unknown fact type"),
        TemporalError::UnknownFactType { .. }
    ));

    // Market-price reads are asset-only, including the observation facade.
    for subject in [issuer, portfolio] {
        assert!(matches!(
            fact_at(&pool, subject, "market_price", valid("2026-01-02"))
                .await
                .expect_err("asset-only"),
            TemporalError::InvalidSubject { .. }
        ));
        assert!(matches!(
            fact_series(&pool, subject, "market_price", window, None)
                .await
                .expect_err("asset-only"),
            TemporalError::InvalidSubject { .. }
        ));
        assert!(matches!(
            observation_series(&pool, subject, "fixture", window, None)
                .await
                .expect_err("asset-only"),
            TemporalError::InvalidSubject { .. }
        ));
    }
    assert!(
        fact_at(&pool, asset, "market_price", valid("2026-01-02"))
            .await
            .expect("market price read")
            .is_none()
    );

    // Pagination: three days, a two-row page, then the remainder.
    let mut expected_dates = Vec::new();
    for (index, day) in ["2026-01-02", "2026-01-03", "2026-01-04"]
        .iter()
        .enumerate()
    {
        let at = valid(day);
        expected_dates.push(at);
        write(
            &pool,
            asset,
            "nav",
            at,
            known("2026-01-05T12:00:00Z"),
            FactValue::Decimal {
                value: Decimal::from(index + 1),
            },
        )
        .await;
    }
    let first_page = fact_series_page(&pool, asset, "nav", window, None, None, 2)
        .await
        .expect("first page");
    assert!(first_page.has_more);
    assert_eq!(first_page.points.len(), 2);
    let second_page = fact_series_page(
        &pool,
        asset,
        "nav",
        window,
        None,
        Some(first_page.points[1].valid_at),
        2,
    )
    .await
    .expect("second page");
    assert!(!second_page.has_more);
    assert_eq!(second_page.points.len(), 1);
    assert_eq!(second_page.points[0].valid_at, expected_dates[2]);

    let mut reference_id = None;
    for point in &first_page.points {
        assert_eq!(
            fact_point_by_id(&pool, point.id)
                .await
                .expect("point by id")
                .expect("present")
                .id,
            point.id
        );
        reference_id = Some(point.id);
    }
    assert!(reference_id.is_some());

    // An envelope without its payload is an error, not a silent empty value.
    let orphan_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO temporal_facts \
         (id, asset_id, fact_type, valid_at, known_at, source, content_hash) \
         VALUES ($1, $2, 'name', '2026-01-02T00:00:00Z', \
                 '2026-01-02T12:00:00Z', 'orphan', 'hash')",
    )
    .bind(orphan_id)
    .bind(asset.id())
    .execute(&pool)
    .await
    .expect("insert orphan");
    let missing = fact_point_by_id(&pool, orphan_id)
        .await
        .expect_err("missing payload");
    assert!(matches!(missing, TemporalError::MissingPayload { .. }));

    guard.cleanup().await;
}

#[tokio::test]
async fn observation_edges_reject_conflicts_and_cover_lookups() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let FactSubject::Asset(asset_id) = seed_asset(&pool, "Observation Edges").await else {
        unreachable!()
    };
    let day = valid("2026-01-02");
    let first_known = known("2026-01-02T12:00:00Z");
    let mut input = input_for_retraction(asset_id, day);
    input.source = "edge-source".into();
    input.known_at = first_known;
    input.open = Some(Decimal::ONE);
    input.high = Some(Decimal::ONE);
    input.low = Some(Decimal::ONE);
    input.close = Some(Decimal::ONE);
    input.volume = Some(Decimal::ONE);
    input.adjustment = Adjustment::Unadjusted;

    let first = observation_edges_append(&pool, &input).await;
    let (again, inserted) =
        obolargus_core::temporal::observations::append_observation_outcome(&pool, input.clone())
            .await
            .expect("idempotent append");
    assert_eq!(again.id, first.id);
    assert!(!inserted);

    let mut conflicting = input.clone();
    conflicting.close = Some(Decimal::from(2));
    conflicting.high = Some(Decimal::from(2));
    conflicting.open = Some(Decimal::from(2));
    conflicting.low = Some(Decimal::from(2));
    let conflict =
        obolargus_core::temporal::observations::append_observation_outcome(&pool, conflicting)
            .await
            .expect_err("stale known_at conflicts");
    assert!(matches!(conflict, TemporalError::ConflictingAppend { .. }));

    let empty_reason = retract_observation(&pool, input.clone(), "  ")
        .await
        .expect_err("empty reason");
    assert!(matches!(
        empty_reason,
        TemporalError::InvalidFactValue { .. }
    ));

    let at = obolargus_core::temporal::observations::observation_at(
        &pool,
        asset_id,
        "eod",
        0,
        "edge-source",
        day,
    )
    .await
    .expect("at")
    .expect("present");
    assert_eq!(at.id, first.id);

    let retraction = obolargus_core::temporal::observations::retract_observation(
        &pool,
        NewMarketObservation {
            known_at: known("2026-01-03T12:00:00Z"),
            ..input.clone()
        },
        "gone",
    )
    .await
    .expect("retract");
    let repeated = obolargus_core::temporal::observations::retract_observation(
        &pool,
        NewMarketObservation {
            known_at: known("2026-01-03T12:00:00Z"),
            ..input.clone()
        },
        "gone",
    )
    .await
    .expect("idempotent retraction");
    assert_eq!(repeated.id, retraction.id);

    let retraction_conflict = obolargus_core::temporal::observations::retract_observation(
        &pool,
        NewMarketObservation {
            known_at: first_known,
            ..input.clone()
        },
        "different",
    )
    .await
    .expect_err("older retraction conflicts");
    assert!(matches!(
        retraction_conflict,
        TemporalError::ConflictingAppend { .. }
    ));

    assert!(
        obolargus_core::temporal::observations::observation_at(
            &pool,
            asset_id,
            "eod",
            0,
            "edge-source",
            day,
        )
        .await
        .expect("retracted at")
        .is_none()
    );
    assert!(matches!(
        ensure_market_observation_partitions(&pool, date("2026-02-01"), date("2026-01-01"))
            .await
            .expect_err("reversed"),
        TemporalError::InvalidTime { .. }
    ));
    ensure_market_observation_partitions(&pool, date("2031-11-15"), date("2032-02-02"))
        .await
        .expect("multi-month partitions");

    guard.cleanup().await;
}

async fn observation_edges_append(
    pool: &PgPool,
    input: &NewMarketObservation,
) -> obolargus_core::temporal::observations::Observation {
    append_observation(pool, input.clone())
        .await
        .expect("append observation")
}
