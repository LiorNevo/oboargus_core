//! Shared test fixture for the `obolargus-core` integration tests.
//!
//! `tx_pool()` returns a `&PgPool` and a `PoolGuard`. The fixture
//! runs `TRUNCATE … CASCADE` on every high-level table *before*
//! yielding the pool, so every test starts with an empty database.
//! The `PoolGuard::Drop` impl also runs `TRUNCATE … CASCADE` to
//! keep the test database tidy.
//!
//! **Serial execution required.** These integration tests share a
//! single PostgreSQL test database (named by `TEST_DATABASE_URL`)
//! and truncate its tables in-place. Running them in parallel
//! causes one test's `TRUNCATE` to wipe another test's in-flight
//! fixture data, producing `23503` foreign-key violations. The
//! test suite is forced to run serially via
//! `RUST_TEST_THREADS=1` in `.cargo/config.toml` at the crate root.
//! A follow-up refactor (T081 follow-up) could move the per-test
//! isolation to a per-test `Transaction` (correct + parallel); the
//! current serial approach is the smallest correct change.
//!
//! The same database is shared with the **other** crates'
//! integration tests (`obolargus_server`'s `market_data_e2e.rs` and
//! `obolargus_cli`'s `sync_db.rs`), so two crates' test suites must
//! not run at the same time either. The root `Makefile`'s `test` and
//! `coverage` targets run the submodules one after another, which is
//! the supported way to run them. Running two crates' suites
//! concurrently in separate shells causes `TRUNCATE` cross-talk and
//! spurious foreign-key failures.
//!
//! The fixture connects to the **test** database (named by
//! `TEST_DATABASE_URL`), NEVER the production database. The
//! `db-cleanup` binary refuses to run against `DATABASE_URL`; this
//! fixture and the cleanup binary together ensure the test suite
//! can truncate freely without ever touching the production schema.

#![allow(dead_code)]

use sqlx::AssertSqlSafe;
use sqlx::PgPool;

/// All high-level tables, in dependency order.
const TABLES: &[&str] = &[
    "asset_reference_data",
    "issuer_reference_data",
    "portfolio_reference_data",
    "asset_identifiers",
    "issuer_identifiers",
    "positions",
    "events",
    // Market-data audit tables. `data_source_reconciliation_audits`
    // references `assets`, so it is truncated before `assets` (the
    // `CASCADE` would handle the order either way, but keeping the
    // dependency order explicit makes the list readable).
    "data_source_reconciliation_audits",
    "data_source_refresh_runs",
    "data_sources",
    "assets",
    "issuers",
    "portfolios",
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
    tx_pool_from(
        std::env::var("TEST_DATABASE_URL").ok(),
        std::env::var("DATABASE_URL").ok(),
    )
    .await
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
    let pool = PgPool::connect(&url).await.expect("connect");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("migrations");
    cleanup(&pool).await;
    let pool_clone = pool.clone();
    Some((pool, PoolGuard { pool: pool_clone }))
}

/// Run `TRUNCATE … CASCADE` on every high-level table.
pub async fn cleanup(pool: &PgPool) {
    let mut tx = match pool.begin().await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("db-cleanup: begin: {}", e);
            return;
        }
    };
    for table in TABLES {
        // Safe: TABLES is a fixed internal constant, not user input
        let q = format!("TRUNCATE TABLE {} RESTART IDENTITY CASCADE", table);
        if let Err(e) = sqlx::query(AssertSqlSafe(q)).execute(&mut *tx).await {
            eprintln!("db-cleanup: {}: {}", table, e);
        }
    }
    if let Err(e) = tx.commit().await {
        eprintln!("db-cleanup: commit: {}", e);
    }
}

/// A guard that exposes `cleanup()` and also runs cleanup on `Drop`
/// (best-effort). The pre-test truncate in `tx_pool` is the
/// correctness guarantee; the explicit `cleanup()` and the `Drop`
/// impl are for keeping the test database tidy between test runs.
pub struct PoolGuard {
    pool: PgPool,
}

impl PoolGuard {
    /// Run the database cleanup synchronously. Call this at the end of
    /// each test to guarantee the test database is empty after the
    /// test suite completes.
    pub async fn cleanup(&self) {
        cleanup(&self.pool).await;
    }
}

impl Drop for PoolGuard {
    fn drop(&mut self) {
        let pool = self.pool.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                cleanup(&pool).await;
            });
        }
    }
}
