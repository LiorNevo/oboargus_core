//! Shared test fixture for the `obolargus-core` integration tests.
//!
//! `tx_pool()` returns a `&PgPool` and a `PoolGuard`. The fixture
//! runs `TRUNCATE … CASCADE` on every high-level table *before*
//! yielding the pool, so every test starts with an empty database.
//! Call `PoolGuard::cleanup()` if a test also wants to tidy the
//! database before dropping the guard.
//!
//! **The fixture serializes database tests itself.** These integration
//! tests share a single PostgreSQL test database (named by
//! `TEST_DATABASE_URL`) and truncate its tables in-place. The
//! `DB_TEST_LOCK` mutex is held for the lifetime of every `PoolGuard`,
//! so even when the Rust test harness runs tests in parallel, only one
//! database test can own the database at a time. This avoids the
//! `TRUNCATE`/foreign-key deadlocks that occur when two tests clean and
//! repopulate the same tables concurrently.
//!
//! The same database is shared with the **other** crates'
//! integration tests (`obolargus_server`'s `market_data_e2e.rs` and
//! `obolargus_cli`'s `sync_db.rs`). Cargo runs different crates' test
//! binaries sequentially, which is the supported way to run them.
//! Running two crates' suites concurrently in separate shells still
//! requires separate test databases (the root `Makefile` uses its
//! serial project loop for this reason).
//!
//! Migrations and the fact-catalog sync run **once per process** per
//! database URL (they are idempotent and the tables they touch are not
//! truncated), so the per-test fixture cost is a connection plus one
//! multi-table `TRUNCATE`. A single `TRUNCATE` statement is an order of
//! magnitude faster than one statement per table on the shared test
//! database.
//!
//! The fixture connects to the **test** database (named by
//! `TEST_DATABASE_URL`), NEVER the production database. The
//! `db-cleanup` binary refuses to run against `DATABASE_URL`; this
//! fixture and the cleanup binary together ensure the test suite
//! can truncate freely without ever touching the production schema.

#![allow(dead_code)]

use sqlx::AssertSqlSafe;
use sqlx::PgPool;
use std::sync::LazyLock;
use tokio::sync::{Mutex, OwnedMutexGuard};

/// Serializes database-backed tests even when the test harness runs
/// tests in parallel. The guard is held by [`PoolGuard`].
static DB_TEST_LOCK: LazyLock<std::sync::Arc<Mutex<()>>> =
    LazyLock::new(|| std::sync::Arc::new(Mutex::new(())));

/// The URL whose database has already been migrated and catalog-synced
/// by this process. Guarded by [`DB_TEST_LOCK`], so the plain mutex is
/// only ever held for a short, non-awaiting read or write.
static INITIALIZED_URL: LazyLock<std::sync::Mutex<Option<String>>> =
    LazyLock::new(|| std::sync::Mutex::new(None));

/// Serializes tests that temporarily mutate process-wide environment
/// variables against tests that read them. `tx_pool` holds this lock
/// only while capturing the two URL values; the database work happens
/// after it is released.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// All high-level tables, in dependency order.
const TABLES: &[&str] = &[
    "fact_text_values",
    "fact_decimal_values",
    "fact_integer_values",
    "fact_boolean_values",
    "fact_date_values",
    "fact_timestamp_values",
    "temporal_facts",
    "market_observations",
    "fact_reconciliations",
    "data_source_fact_types",
    "asset_identifiers",
    "issuer_identifiers",
    "rules",
    "portfolio_rules",
    "positions",
    "events",
    // Market-data audit tables. `fact_reconciliations` references
    // `temporal_facts`, so it is truncated before the fact envelope (the
    // `CASCADE` would handle the order either way, but keeping the
    // dependency order explicit makes the list readable).
    "data_source_refresh_runs",
    "data_source_state_changes",
    "reference_data_load_runs",
    "data_sources",
    "assets",
    "issuers",
    "portfolios",
    // Automation tables (spec 030). The automation integration tests
    // create flows/runs with fixed names, so leaving these rows behind
    // makes a second test run fail on the unique flow-name index.
    "automation_messages",
    "automation_run_logs",
    "automation_context",
    "automation_node_runs",
    "automation_audit_events",
    "automation_runs",
    "automation_triggers",
    "automation_flow_versions",
    "automation_flows",
    // Exchange calendars (spec 035). Children first.
    "exchange_calendar_audit",
    "asset_calendar_assignments",
    "exchange_calendar_exceptions",
    "exchange_calendar_weekdays",
    "exchange_calendars",
    // Morpheus chat (spec 032). Children first.
    "morpheus_observations",
    "morpheus_approval_requests",
    "morpheus_ui_actions",
    "morpheus_tool_calls",
    "morpheus_steps",
    "morpheus_turns",
    "morpheus_messages",
    "morpheus_control_leases",
    "morpheus_standing_approvals",
    "morpheus_conversations",
];

/// Pick the test database URL from the two candidates, preferring
/// `test_url` (the value of `TEST_DATABASE_URL`) and falling back to
/// `prod_url` (the value of `DATABASE_URL`).
///
/// This is a pure helper: the env-var reads live in [`tx_pool`], so
/// tests can exercise every branch (preference, fallback, neither set)
/// without manipulating process-wide environment state.
pub fn resolve_url(test_url: Option<String>, prod_url: Option<String>) -> Option<String> {
    test_url.or(prod_url)
}

/// Open the migrations, truncate every high-level table, return a
/// `&PgPool` for the test to use.
pub async fn tx_pool() -> Option<(PgPool, PoolGuard)> {
    #[cfg(test)]
    let _env_guard = env_lock();
    let test_url = std::env::var("TEST_DATABASE_URL").ok();
    let prod_url = std::env::var("DATABASE_URL").ok();
    #[cfg(test)]
    drop(_env_guard);
    tx_pool_from(test_url, prod_url).await
}

/// Parameterized variant of [`tx_pool`]. The two `Option<String>`
/// arguments are the values that [`tx_pool`] would read from
/// `TEST_DATABASE_URL` and `DATABASE_URL` respectively. Tests use
/// this to exercise the `None` and fallback paths without touching
/// the process environment.
pub async fn tx_pool_from(
    test_url: Option<String>,
    prod_url: Option<String>,
) -> Option<(PgPool, PoolGuard)> {
    let url = resolve_url(test_url, prod_url)?;
    let lock = (*DB_TEST_LOCK).clone().lock_owned().await;
    let pool = PgPool::connect(&url).await.expect("connect");
    let already_initialized = INITIALIZED_URL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_deref()
        == Some(url.as_str());
    if !already_initialized {
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("migrations");
        crate::temporal::catalog::sync_fact_catalog(&pool)
            .await
            .expect("fact catalog");
        *INITIALIZED_URL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(url);
    }
    cleanup(&pool).await;
    let pool_clone = pool.clone();
    Some((
        pool,
        PoolGuard {
            pool: pool_clone,
            _lock: lock,
        },
    ))
}

/// Run `TRUNCATE … CASCADE` on every high-level table.
///
/// The tables are truncated by a single statement: on the shared test
/// database one statement per table costs seconds because each
/// `TRUNCATE` re-acquires `ACCESS EXCLUSIVE` locks. If the batch fails
/// (for example, when a caller passes a schema-less scratch database),
/// fall back to per-table statements so the individual failures are
/// reported instead of hidden behind the first one.
pub async fn cleanup(pool: &PgPool) {
    let mut tx = match pool.begin().await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("db-cleanup: begin: {}", e);
            return;
        }
    };
    let batch = format!(
        "TRUNCATE TABLE {} RESTART IDENTITY CASCADE",
        TABLES.join(", ")
    );
    if let Err(error) = sqlx::query(AssertSqlSafe(batch)).execute(&mut *tx).await {
        eprintln!("db-cleanup: batch truncate failed: {error}");
        let _ = tx.rollback().await;
        for table in TABLES {
            // Safe: TABLES is a fixed internal constant, not user input
            let q = format!("TRUNCATE TABLE {} RESTART IDENTITY CASCADE", table);
            if let Err(e) = sqlx::query(AssertSqlSafe(q)).execute(pool).await {
                eprintln!("db-cleanup: {}: {}", table, e);
            }
        }
        return;
    }
    if let Err(e) = tx.commit().await {
        eprintln!("db-cleanup: commit: {}", e);
    }
}

/// Holds the database-test mutex for the lifetime of the test and
/// exposes an explicit `cleanup()` helper. The pre-test truncate in
/// `tx_pool` is the correctness guarantee; callers may also run
/// `cleanup()` before dropping the guard to keep the database tidy.
pub struct PoolGuard {
    pool: PgPool,
    _lock: OwnedMutexGuard<()>,
}

impl PoolGuard {
    /// Run the database cleanup synchronously. Call this at the end of
    /// each test to guarantee the test database is empty after the
    /// test suite completes.
    pub async fn cleanup(&self) {
        cleanup(&self.pool).await;
    }
}

// =============================================================================
// Bitemporal fixtures (spec 021)
// =============================================================================

/// Deterministic ids for the bitemporal seed, so assertions can name rows
/// without re-reading them.
pub mod temporal_ids {
    use uuid::Uuid;

    /// The seeded asset.
    pub const ASSET: Uuid = Uuid::from_u128(0x0210_0000_0000_0000_0000_0000_0000_0001);
    /// The seeded issuer.
    pub const ISSUER: Uuid = Uuid::from_u128(0x0210_0000_0000_0000_0000_0000_0000_0002);
    /// The seeded portfolio.
    pub const PORTFOLIO: Uuid = Uuid::from_u128(0x0210_0000_0000_0000_0000_0000_0000_0003);
    /// Revision 1 (oldest belief).
    pub const FACT_V1: Uuid = Uuid::from_u128(0x0210_0000_0000_0000_0000_0000_0000_0011);
    /// Revision 2, same valid day.
    pub const FACT_V2: Uuid = Uuid::from_u128(0x0210_0000_0000_0000_0000_0000_0000_0012);
    /// Revision 3, the retraction.
    pub const FACT_RETRACT: Uuid = Uuid::from_u128(0x0210_0000_0000_0000_0000_0000_0000_0013);
}

/// Seeds one asset, one issuer, and one portfolio, plus three beliefs about
/// one asset fact across two days, the last of which retracts it.
///
/// The fixture is DB-gated: callers reach it through [`tx_pool`], which
/// returns `None` (and the test returns early) when neither
/// `TEST_DATABASE_URL` nor `DATABASE_URL` is set.
///
/// Time coordinates: `valid_at` is `2026-01-02T00:00:00Z`; the three
/// `known_at` values are `2026-01-02T12:00:00Z`, `2026-01-03T09:00:00Z`, and
/// `2026-01-04T09:00:00Z`.
pub async fn seed_temporal_fixture(pool: &PgPool) {
    sqlx::query(
        "INSERT INTO issuers (id, name, issuer_type, legal_jurisdiction) \
         VALUES ($1, 'Temporal Issuer', 'CORPORATION', 'US')",
    )
    .bind(temporal_ids::ISSUER)
    .execute(pool)
    .await
    .expect("seed issuer");

    sqlx::query(
        "INSERT INTO portfolios (id, name, purpose, created_by) \
         VALUES ($1, 'Temporal Portfolio', 'testing', 'fixture')",
    )
    .bind(temporal_ids::PORTFOLIO)
    .execute(pool)
    .await
    .expect("seed portfolio");

    sqlx::query(
        "INSERT INTO assets (id, asset_type, display_name, currency, issuer_id) \
         VALUES ($1, 'stock', 'Temporal Asset', 'USD', $2)",
    )
    .bind(temporal_ids::ASSET)
    .bind(temporal_ids::ISSUER)
    .execute(pool)
    .await
    .expect("seed asset");

    for (id, known_at, value, retracted) in [
        (
            temporal_ids::FACT_V1,
            "2026-01-02T12:00:00Z",
            "100.00",
            false,
        ),
        (
            temporal_ids::FACT_V2,
            "2026-01-03T09:00:00Z",
            "101.00",
            false,
        ),
        (
            temporal_ids::FACT_RETRACT,
            "2026-01-04T09:00:00Z",
            "delisted",
            true,
        ),
    ] {
        sqlx::query(
            "INSERT INTO temporal_facts \
                (id, asset_id, fact_type, valid_at, known_at, source, retracted, \
                 retraction_reason, content_hash) \
             VALUES ($1, $2, 'nav', '2026-01-02T00:00:00Z', $3::timestamptz, 'fixture', $4, $5, $6)",
        )
        .bind(id)
        .bind(temporal_ids::ASSET)
        .bind(known_at)
        .bind(retracted)
        .bind(if retracted { Some(value) } else { None })
        .bind(format!("fixture-{id}"))
        .execute(pool)
        .await
        .expect("seed fact");

        if !retracted {
            sqlx::query(
                "INSERT INTO fact_decimal_values (fact_id, value) VALUES ($1, $2::numeric)",
            )
            .bind(id)
            .bind(value)
            .execute(pool)
            .await
            .expect("seed fact value");
        }
    }
}
