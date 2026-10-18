//! Fault-injection helpers for the `obolargus_core` test suite.
//!
//! These helpers build `PgPool`s that fail on a chosen statement index, so
//! the test suite can exercise every `?` propagation point in the domain
//! code without having to invent bespoke failure conditions for each one.
//!
//! Two failure modes are provided:
//!
//! 1. **`hard_fail_after_pool`** — every acquire *after* the k-th one fails
//!    with `sqlx::Error::PoolTimedOut` (after `acquire_timeout`). Used to
//!    cover the residual of `?` on `pool.begin()`, `pool.connect()`,
//!    `pool.acquire()`, etc. (instant once `acquire_timeout` elapses).
//!
//! 2. **`poison_after_pool`** — connections created after the k-th acquire
//!    have their `search_path` set to a nonexistent schema, so the very
//!    next statement that references an unqualified table fails with
//!    `relation does not exist`. Used to cover the residual of `?` on
//!    `sqlx::query(...).execute(pool).await?`, `.fetch_one(pool).await?`,
//!    `.fetch_optional(pool).await?`, etc. (the error is instant and
//!    deterministic).
//!
//! Both modes discard the connection after each use
//! (`after_release` → `Ok(false)`) so every statement triggers a fresh
//! `after_connect` invocation, giving the hook precise control over
//! which statement index is the failing one.
//!
//! These helpers live in a `tests`-named directory so they are excluded
//! from the `cargo-llvm-cov` report by the crate's default ignore regex
//! (see `cargo-llvm-cov` `report.rs` line 943). The test code that
//! invokes them therefore does not need to be covered itself.
//!
//! ## Why we need this
//!
//! `?` desugars to a `match` whose `Err` arm is a separate code region.
//! For coverage to be 100%, every one of those regions must be executed
//! at least once, which means every `?` on a `sqlx::Result` must see
//! `Err` at least once. Most of the time we cannot (and do not want to)
//! induce a real error on the shared test database — so we use these
//! fault-injection pools to make the database *behave* as if it returned
//! an error.

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A pool that *fails* (returns `sqlx::Error::PoolTimedOut`) starting at
/// the `(k+1)`-th acquire. The first `k` acquires succeed; from acquire
/// `k+1` onwards, every acquire returns `Error::PoolTimedOut` after
/// `acquire_timeout` (250ms by default).
pub(crate) fn hard_fail_after_pool(url: &str, successful_acquires: usize) -> PgPool {
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_for_hook = counter.clone();
    PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(250))
        // Force a fresh connection on every acquire so the
        // `after_connect` hook runs each time and can apply the failure
        // mode to the right statement.
        .after_release(|_, _| Box::pin(async { Ok(false) }))
        .after_connect(move |_conn, _meta| {
            let n = counter_for_hook.fetch_add(1, Ordering::SeqCst);
            if n >= successful_acquires {
                // Returning `Err` makes the pool discard the connection
                // and retry; the retry budget is bounded by
                // `acquire_timeout`, so the caller's `?` residual
                // executes within ~`acquire_timeout`.
                Box::pin(async {
                    Err(sqlx::Error::Protocol(
                        "fault-injection: hard_fail_after_pool".to_string(),
                    ))
                })
            } else {
                Box::pin(async { Ok(()) })
            }
        })
        .connect_lazy(url)
        .expect("lazy pool")
}

/// A pool whose connections become unusable for table-touching statements
/// at acquire index `k`. The first `k` acquires succeed; from acquire
/// `k+1` onwards, the connection's `search_path` is set to a schema that
/// does not exist, so any subsequent `sqlx::query("... FROM <table> ...")`
/// fails with `relation "<table>" does not exist` instantly.
pub(crate) fn poison_after_pool(url: &str, successful_acquires: usize) -> PgPool {
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_for_hook = counter.clone();
    PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(250))
        .after_release(|_, _| Box::pin(async { Ok(false) }))
        .after_connect(move |conn, _meta| {
            let n = counter_for_hook.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if n >= successful_acquires {
                    // Break the search path so unqualified table
                    // references in the next statement fail. The
                    // statement itself is a SET (which resolves
                    // against the `pg_catalog` system catalog
                    // implicitly), so this hook succeeds and the
                    // *handed-out* connection is poisoned.
                    sqlx::query("SET search_path TO obolargus_no_such_schema_xyz")
                        .execute(&mut *conn)
                        .await?;
                }
                Ok(())
            })
        })
        .connect_lazy(url)
        .expect("lazy pool")
}

/// Build a closed pool (all acquires return `Error::PoolClosed` instantly,
/// no `acquire_timeout` wait). Useful as a deterministic, zero-wait
/// "dead pool" for tests that need `?` on a connection-acquiring call to
/// fail.
#[allow(unused)]
pub(crate) async fn closed_pool(url: &str) -> PgPool {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_millis(100))
        .connect(url)
        .await
        .expect("pool");
    pool.close().await;
    pool
}

/// Run a closure with a count of how many acquires the happy path made.
/// The closure must complete without panicking.
///
/// The pool is configured exactly like the poison/hard-fail pools
/// (discard-after-use, identical defaults), but without any
/// after_connect failure. The returned counter increments per
/// `after_connect` invocation (i.e. per fresh connection created, which
/// under discard-after-use is the same as per acquire).
#[allow(unused)]
pub(crate) async fn count_acquires<F, T>(url: &str, op: F) -> (T, usize)
where
    F: for<'a> FnOnce(
        &'a PgPool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>,
{
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_for_hook = counter.clone();
    let pool = PgPoolOptions::new()
        .after_release(|_, _| Box::pin(async { Ok(false) }))
        .after_connect(move |_conn, _meta| {
            counter_for_hook.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        })
        .connect_lazy(url)
        .expect("lazy pool");
    let result = op(&pool).await;
    let n = counter.load(Ordering::SeqCst);
    (result, n)
}
