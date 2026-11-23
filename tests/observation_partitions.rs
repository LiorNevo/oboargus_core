//! Monthly market-observation partition tests (US4).
//!
//! Database-gated through the shared `tx_pool` fixture. Partition tables are
//! created once in the test database and intentionally not dropped; every
//! assertion is idempotent across reruns.

use chrono::NaiveDate;
use obolargus_core::common::{seed_temporal_fixture, temporal_ids, tx_pool};
use obolargus_core::domain::id::{AssetId, DomainId};
use obolargus_core::temporal::observations::{
    NewMarketObservation, append_observation, ensure_market_observation_partitions,
    ensure_month_partition, ensure_month_partitions,
};
use obolargus_core::temporal::{Adjustment, KnownAt, ValidAt};
use rust_decimal::Decimal;
use sqlx::PgPool;

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).expect("date")
}

async fn partition_exists(pool: &PgPool, name: &str) -> bool {
    let found: Option<String> = sqlx::query_scalar("SELECT to_regclass($1)::text")
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("partition lookup");
    found.is_some()
}

fn observation(valid_at: NaiveDate, known_at: NaiveDate) -> NewMarketObservation {
    NewMarketObservation {
        asset_id: AssetId::from_uuid(temporal_ids::ASSET),
        kind: "eod".to_string(),
        interval_seconds: 0,
        valid_at: ValidAt::midnight_utc(valid_at),
        known_at: KnownAt::midnight_utc(known_at),
        source: "partition-test".to_string(),
        open: Some(Decimal::from(100)),
        high: Some(Decimal::from(101)),
        low: Some(Decimal::from(99)),
        close: Some(Decimal::from(100)),
        adjclose: None,
        price: None,
        volume: Some(Decimal::from(1000)),
        currency: "USD".to_string(),
        adjustment: Adjustment::Unadjusted,
        sequence: None,
    }
}

#[tokio::test]
async fn month_partition_is_created_once_and_receives_rows() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    seed_temporal_fixture(&pool).await;

    ensure_month_partition(&pool, 2035, 7)
        .await
        .expect("create partition");
    assert!(partition_exists(&pool, "market_observations_2035_07").await);

    // Rerunning is a no-op, not a duplicate.
    ensure_month_partition(&pool, 2035, 7)
        .await
        .expect("rerun partition creation");
    let partitions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_inherits \
         WHERE inhrelid = to_regclass('market_observations_2035_07')",
    )
    .fetch_one(&pool)
    .await
    .expect("partition count");
    assert_eq!(partitions, 1);

    let row = append_observation(&pool, observation(date(2035, 7, 15), date(2035, 7, 16)))
        .await
        .expect("append observation");
    let table: String = sqlx::query_scalar(
        "SELECT tableoid::regclass::text FROM market_observations WHERE id = $1",
    )
    .bind(row.id)
    .fetch_one(&pool)
    .await
    .expect("table oid");
    assert_eq!(table, "market_observations_2035_07");

    guard.cleanup().await;
}

#[tokio::test]
async fn range_helper_covers_every_month_and_year_rollover() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    ensure_month_partitions(&pool, date(2036, 11, 15), date(2037, 2, 2))
        .await
        .expect("create range partitions");
    for name in [
        "market_observations_2036_11",
        "market_observations_2036_12",
        "market_observations_2037_01",
        "market_observations_2037_02",
    ] {
        assert!(partition_exists(&pool, name).await, "missing {name}");
    }
    // The backwards-compatible wrapper behaves the same and is idempotent.
    ensure_market_observation_partitions(&pool, date(2036, 11, 15), date(2037, 2, 2))
        .await
        .expect("rerun range partitions");

    let reversed = ensure_month_partitions(&pool, date(2037, 2, 1), date(2037, 1, 1)).await;
    assert!(reversed.is_err(), "a reversed range must be rejected");

    guard.cleanup().await;
}

#[tokio::test]
async fn concurrent_creation_serializes_on_the_advisory_lock() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let attempts = (0..8).map(|_| {
        let pool = pool.clone();
        async move { ensure_month_partition(&pool, 2038, 3).await }
    });
    let results = futures::future::join_all(attempts).await;
    assert!(
        results.iter().all(Result::is_ok),
        "every concurrent creator must succeed: {results:?}"
    );
    assert!(partition_exists(&pool, "market_observations_2038_03").await);

    let invalid = ensure_month_partition(&pool, 2038, 13).await;
    assert!(invalid.is_err(), "month 13 must be rejected");

    guard.cleanup().await;
}
