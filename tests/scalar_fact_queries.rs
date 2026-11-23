//! Query-count regression test for scalar fact series (FR-031).
//!
//! A test-only `tracing` subscriber counts the `sqlx::query` debug events that
//! sqlx emits per executed statement. A scalar series must issue the subject
//! check plus one envelope query plus one payload query, regardless of how
//! many points it returns.

use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, Utc};
use obolargus_core::common::tx_pool;
use obolargus_core::domain::id::{AssetId, DomainId};
use obolargus_core::temporal::facts::read::fact_series;
use obolargus_core::temporal::{FactSubject, TemporalWindow, ValidAt};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};
use uuid::Uuid;

static QUERY_COUNT: AtomicUsize = AtomicUsize::new(0);
static INSTALL: Once = Once::new();

struct QueryCounter;

impl Subscriber for QueryCounter {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.target() == "sqlx::query"
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        if event.metadata().target() == "sqlx::query" {
            QUERY_COUNT.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

fn install_query_counter() {
    INSTALL.call_once(|| {
        tracing::subscriber::set_global_default(QueryCounter).expect("install query counter");
    });
}

fn valid(raw: &str) -> ValidAt {
    let instant: DateTime<Utc> = raw.parse().expect("instant");
    ValidAt::new(instant).expect("valid time")
}

async fn seed_series(pool: &sqlx::PgPool, asset_id: Uuid, fact_type: &str, points: i64) {
    sqlx::query(
        r#"
        WITH inserted AS (
            INSERT INTO temporal_facts
                (id, asset_id, fact_type, valid_at, known_at, source, content_hash)
            SELECT gen_random_uuid(), $1, $3,
                   TIMESTAMPTZ '2026-01-01 00:00:00+00' + (g * INTERVAL '1 day'),
                   TIMESTAMPTZ '2026-01-02 00:00:00+00',
                   'query-count-fixture',
                   'query-count-' || g
            FROM generate_series(1, $2::bigint) AS g
            RETURNING id
        )
        INSERT INTO fact_decimal_values (fact_id, value)
        SELECT id, 1.5 FROM inserted
        "#,
    )
    .bind(asset_id)
    .bind(points)
    .bind(fact_type)
    .execute(pool)
    .await
    .expect("seed scalar facts");
}

#[tokio::test]
async fn scalar_series_query_count_is_independent_of_points() {
    install_query_counter();
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    let asset_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO assets (id, asset_type, display_name, currency) \
         VALUES ($1, 'stock', 'Query Count Asset', 'USD')",
    )
    .bind(asset_id)
    .execute(&pool)
    .await
    .expect("seed asset");
    seed_series(&pool, asset_id, "nav", 5_000).await;

    let subject = FactSubject::Asset(AssetId::from_uuid(asset_id));
    let window = TemporalWindow {
        from: valid("2025-12-31T00:00:00Z"),
        to: valid("2040-01-01T00:00:00Z"),
    };

    // Warm the pool and any lazy statement metadata.
    let warmup = fact_series(&pool, subject, "nav", window, None)
        .await
        .expect("warmup series");
    assert_eq!(warmup.len(), 5_000);

    QUERY_COUNT.store(0, Ordering::SeqCst);
    let series = fact_series(&pool, subject, "nav", window, None)
        .await
        .expect("series");
    let large_count = QUERY_COUNT.load(Ordering::SeqCst);
    println!("5,000-point series issued {large_count} queries");
    assert_eq!(series.len(), 5_000);
    assert!(
        (1..=3).contains(&large_count),
        "a 5,000-point series issued {large_count} queries (expected <= 3; 0 means the \
         sqlx query counter is not observing statements)"
    );

    let small_window = TemporalWindow {
        from: valid("2025-12-31T00:00:00Z"),
        to: valid("2026-01-07T00:00:00Z"),
    };
    QUERY_COUNT.store(0, Ordering::SeqCst);
    let small = fact_series(&pool, subject, "nav", small_window, None)
        .await
        .expect("small series");
    let small_count = QUERY_COUNT.load(Ordering::SeqCst);
    assert_eq!(small.len(), 5);
    assert_eq!(
        small_count, large_count,
        "query count must not depend on the number of points"
    );

    guard.cleanup().await;
}
