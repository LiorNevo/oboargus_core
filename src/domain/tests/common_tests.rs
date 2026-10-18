//! Unit tests for `obolargus_core::common`. Excluded from the
//! `cargo-llvm-cov` report by the `*_tests.rs` filename pattern.

// `let Some(...) = ... else { return None; }` is used in the
// `block_on` closure below instead of `?` so the closure's body
// stays `async { ... }`-shaped and easy to read; clippy prefers
// `?` here, but the two are equivalent for the `Option`-returning
// closure. Allow the lint locally to keep the build green.
#![allow(clippy::question_mark)]

use crate::common::{PoolGuard, cleanup, resolve_url, tx_pool_from};
use crate::domain::tests::fault::hard_fail_after_pool;
use sqlx::Connection;
use sqlx::PgPool;
use std::time::Duration;
use tokio::time::sleep;

fn test_url() -> Option<String> {
    std::env::var("TEST_DATABASE_URL")
        .ok()
        .or_else(|| std::env::var("DATABASE_URL").ok())
}

#[test]
fn resolve_url_prefers_test_url_over_prod_url() {
    assert_eq!(
        resolve_url(Some("test".into()), Some("prod".into())),
        Some("test".into())
    );
}

#[test]
fn resolve_url_falls_back_to_prod_url_when_test_is_none() {
    assert_eq!(resolve_url(None, Some("prod".into())), Some("prod".into()));
}

#[test]
fn resolve_url_returns_none_when_both_are_none() {
    assert_eq!(resolve_url(None, None), None);
}

#[tokio::test]
async fn tx_pool_from_returns_none_when_no_url_is_provided() {
    // The branch where neither env-var-derived URL is set.
    let result = tx_pool_from(None, None).await;
    assert!(result.is_none());
}

#[tokio::test]
async fn tx_pool_from_with_test_url_truncates_and_yields_a_usable_pool() {
    let Some(url) = test_url() else {
        return;
    };
    let Some((pool, guard)) = tx_pool_from(Some(url.clone()), None).await else {
        panic!("tx_pool_from should have returned Some");
    };
    // The fixture must have truncated every high-level table — there
    // should be no rows left in any of them.
    for table in [
        "assets",
        "issuers",
        "portfolios",
        "events",
        "positions",
        "asset_identifiers",
        "issuer_identifiers",
        "asset_reference_data",
        "issuer_reference_data",
        "portfolio_reference_data",
    ] {
        let n: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .expect("count");
        assert_eq!(n, 0, "table {table} should be empty after fixture");
    }
    guard.cleanup().await;
}

#[tokio::test]
async fn cleanup_reports_and_returns_when_pool_begin_fails() {
    let Some(url) = test_url() else {
        return;
    };
    // Hard-fail at the very first acquire, so `pool.begin()` errors
    // and `cleanup` prints the eprintln and returns. This covers the
    // begin-Err branch.
    let pool = hard_fail_after_pool(&url, 0);
    cleanup(&pool).await;
}

#[tokio::test]
async fn cleanup_continues_after_an_execute_failure_and_reports_a_commit_error() {
    let Some(url) = test_url() else {
        return;
    };
    // Connect to a scratch database that does NOT have the high-level
    // tables: every TRUNCATE statement fails, then we force COMMIT
    // to fail by setting up a deferred check constraint violation
    // (PostgreSQL returns COMMIT success on aborted transactions, so
    // we need a real error at commit time).
    let scratch_url = url_for_scratch_db(&url).await;
    setup_commit_trap(&url).await;
    let pool = PgPool::connect(&scratch_url)
        .await
        .expect("scratch connect");
    cleanup(&pool).await;
    drop(pool);
    drop_scratch_db(&url, &scratch_url).await;
}

#[tokio::test]
async fn pool_guard_drop_runs_cleanup_concurrently_inside_a_runtime() {
    let Some(url) = test_url() else {
        return;
    };
    // Run `tx_pool` on a runtime whose current_thread scheduler we
    // then hand a non-tokio task to: dropping the guard inside the
    // tokio runtime exercises the `Handle::try_current() == Ok` arm,
    // and the spawned cleanup task must complete before we assert
    // the table is empty.
    let Some((pool, guard)) = tx_pool_from(Some(url.clone()), None).await else {
        return;
    };
    // Insert a row we can observe later.
    sqlx::query(
        "INSERT INTO issuers (id, name, issuer_type, legal_jurisdiction) \
         VALUES (gen_random_uuid(), 'drop-test', 'corporation', 'US')",
    )
    .execute(&pool)
    .await
    .expect("insert issuer");
    // Drop the guard — the Drop impl spawns `cleanup` onto the
    // current tokio runtime. On a `current_thread` runtime we must
    // yield to let the spawned task run; the sleep below provides
    // enough wall time for the spawned cleanup to truncate and
    // release its connection.
    drop(guard);
    sleep(Duration::from_millis(500)).await;
    // The dropped guard's spawned cleanup must have truncated the
    // table — no rows left.
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM issuers")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(n, 0, "Drop-spawned cleanup must have truncated the table");
    cleanup(&pool).await;
}

#[test]
fn pool_guard_drop_outside_a_runtime_does_not_panic() {
    // We need a runtime to construct the guard, but the drop itself
    // must happen *after* the runtime is shut down so the
    // `Handle::try_current()` returns `Err` and the else-arm fires.
    //
    // We use a plain `#[test]` (not `#[tokio::test]`) so there is no
    // outer tokio runtime for `Handle::try_current()` to find when
    // we drop the guard.
    let Some(url) = test_url() else {
        return;
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let guard: Option<PoolGuard> = runtime.block_on(async {
        let Some((_pool, guard)) = tx_pool_from(Some(url), None).await else {
            return None;
        };
        Some(guard)
    });
    drop(runtime);
    // Drop the guard outside any runtime: the `Drop` impl's
    // `try_current()` returns `Err` and the else-arm is exercised.
    drop(guard);
}

// --- helpers ---

/// Create (or reuse) a scratch database that has NO high-level
/// tables. The first call creates the DB; subsequent calls reuse
/// it. Used by `cleanup_*` tests that need a pool to a schema-less
/// database.
async fn url_for_scratch_db(base_url: &str) -> String {
    // base_url is of the form `postgres://user:pass@host:port/dbname`.
    // We derive an `admin` URL that points at the maintenance
    // `postgres` database and a `scratch` URL that points at
    // `obolargus_common_cov_scratch`.
    let (head, _db) = base_url.rsplit_once('/').expect("base url has a /");
    let scratch_db = "obolargus_common_cov_scratch";
    let admin_url = format!("{head}/postgres");

    // Ensure the scratch database exists.
    let mut admin = sqlx::PgConnection::connect(&admin_url)
        .await
        .expect("admin connect");
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)")
            .bind(scratch_db)
            .fetch_one(&mut admin)
            .await
            .expect("exists check");
    if !exists {
        let sql = format!("CREATE DATABASE {scratch_db}");
        sqlx::query(&sql)
            .execute(&mut admin)
            .await
            .expect("create scratch db");
    }
    format!("{head}/{scratch_db}")
}

async fn drop_scratch_db(base_url: &str, _scratch_url: &str) {
    let (head, _db) = base_url.rsplit_once('/').expect("base url has a /");
    let scratch_db = "obolargus_common_cov_scratch";
    let admin_url = format!("{head}/postgres");
    // Best-effort: terminate any remaining backends and drop the
    // scratch DB. Ignore errors so this helper is safe to call from
    // a test that already cleaned up.
    let mut admin = sqlx::PgConnection::connect(&admin_url)
        .await
        .expect("admin connect");
    let _ = sqlx::query(&format!(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
         WHERE datname = '{scratch_db}' AND pid <> pg_backend_pid()"
    ))
    .execute(&mut admin)
    .await;
    let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS {scratch_db}"))
        .execute(&mut admin)
        .await;
}

/// Install a deferred foreign-key constraint on the scratch
/// database such that the COMMIT in `cleanup` will fail with
/// "violates foreign key constraint". We set up a `trap_parent`
/// table and a `trap_child` table that references it; the BEFORE
/// TRUNCATE trigger on the last table inserts a `trap_child` row
/// referencing a non-existent parent, and the FK is `DEFERRABLE
/// INITIALLY DEFERRED`, so the violation is reported at COMMIT
/// time.
async fn setup_commit_trap(base_url: &str) {
    let (head, _db) = base_url.rsplit_once('/').expect("base url has a /");
    let scratch_db = "obolargus_common_cov_scratch";
    let scratch_url = format!("{head}/{scratch_db}");
    let mut conn = sqlx::PgConnection::connect(&scratch_url)
        .await
        .expect("scratch connect");
    // The FK is `DEFERRABLE INITIALLY DEFERRED`; an INSERT that
    // violates the FK (parent doesn't exist) is accepted by the
    // engine and only rejected when the transaction COMMITs.
    sqlx::query("CREATE TABLE IF NOT EXISTS trap_parent (id int PRIMARY KEY)")
        .execute(&mut conn)
        .await
        .expect("create trap_parent");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS trap_child ( \
             parent_id int NOT NULL REFERENCES trap_parent(id) \
             DEFERRABLE INITIALLY DEFERRED \
         )",
    )
    .execute(&mut conn)
    .await
    .expect("create trap_child");
    // We need ALL the high-level tables to exist on the scratch
    // database so every TRUNCATE in `cleanup` succeeds (otherwise
    // the transaction is aborted and the deferred FK constraint
    // is never checked at COMMIT time).
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS asset_reference_data ( \
             id UUID PRIMARY KEY \
         )",
    )
    .execute(&mut conn)
    .await
    .expect("create asset_reference_data");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS issuer_reference_data ( \
             id UUID PRIMARY KEY \
         )",
    )
    .execute(&mut conn)
    .await
    .expect("create issuer_reference_data");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS portfolio_reference_data ( \
             id UUID PRIMARY KEY \
         )",
    )
    .execute(&mut conn)
    .await
    .expect("create portfolio_reference_data");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS asset_identifiers ( \
             id UUID PRIMARY KEY \
         )",
    )
    .execute(&mut conn)
    .await
    .expect("create asset_identifiers");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS issuer_identifiers ( \
             id UUID PRIMARY KEY \
         )",
    )
    .execute(&mut conn)
    .await
    .expect("create issuer_identifiers");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS positions ( \
             portfolio_id UUID, asset_id UUID, quantity INT, as_of TIMESTAMPTZ \
         )",
    )
    .execute(&mut conn)
    .await
    .expect("create positions");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS events ( \
             id UUID PRIMARY KEY \
         )",
    )
    .execute(&mut conn)
    .await
    .expect("create events");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS assets ( \
             id UUID PRIMARY KEY \
         )",
    )
    .execute(&mut conn)
    .await
    .expect("create assets");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS issuers ( \
             id UUID PRIMARY KEY \
         )",
    )
    .execute(&mut conn)
    .await
    .expect("create issuers");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS portfolios ( \
             id UUID PRIMARY KEY \
         )",
    )
    .execute(&mut conn)
    .await
    .expect("create portfolios");
    // A BEFORE TRUNCATE trigger on the FIRST table inserts a
    // `trap_child` row that references no existing parent. Because
    // the FK is deferred, the row insert completes successfully
    // and the TRUNCATE statement completes (the trigger returns
    // NULL which skips the truncation, but that doesn't abort the
    // transaction). All subsequent TRUNCATEs also succeed because
    // the tables exist. COMMIT then checks the deferred FK and
    // fails the transaction.
    sqlx::query(
        "CREATE OR REPLACE FUNCTION trap_fn() RETURNS trigger AS $$ \
             BEGIN \
                 INSERT INTO trap_child (parent_id) VALUES (999); \
                 RETURN NULL; \
             END; \
         $$ LANGUAGE plpgsql",
    )
    .execute(&mut conn)
    .await
    .expect("create function");
    sqlx::query("DROP TRIGGER IF EXISTS trap ON asset_reference_data")
        .execute(&mut conn)
        .await
        .expect("drop trigger");
    sqlx::query(
        "CREATE TRIGGER trap BEFORE TRUNCATE ON asset_reference_data \
         FOR EACH STATEMENT EXECUTE FUNCTION trap_fn()",
    )
    .execute(&mut conn)
    .await
    .expect("create trigger");
}
