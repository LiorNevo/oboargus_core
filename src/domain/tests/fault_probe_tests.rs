//! Probe tests for the fault-injection pool. These tests verify that
//! the `hard_fail_after_pool` and `poison_after_pool` helpers actually
//! fail statements in the expected positions. They are excluded from
//! the coverage report (the file matches the `*_tests.rs` exclusion
//! pattern).
//!
//! The probes run against the real test database — they confirm the
//! fault-injection mechanism end-to-end without touching the domain
//! code.

use crate::domain::tests::fault::{hard_fail_after_pool, poison_after_pool};

fn test_url() -> Option<String> {
    std::env::var("TEST_DATABASE_URL")
        .ok()
        .or_else(|| std::env::var("DATABASE_URL").ok())
}

#[tokio::test]
async fn poison_after_pool_fails_table_statements_at_the_right_index() {
    let Some(url) = test_url() else {
        return;
    };

    // First 2 acquires succeed; from the 3rd onwards, table-touching
    // statements should fail because the search_path has been broken.
    let pool = poison_after_pool(&url, 2);

    // Statements 1 and 2 succeed (acquires 0 and 1 are not poisoned).
    let r1: Result<i64, _> = sqlx::query_scalar("SELECT COUNT(*) FROM issuers")
        .fetch_one(&pool)
        .await;
    let r2: Result<i64, _> = sqlx::query_scalar("SELECT COUNT(*) FROM assets")
        .fetch_one(&pool)
        .await;
    assert!(r1.is_ok(), "first table-touching statement should succeed");
    assert!(r2.is_ok(), "second table-touching statement should succeed");

    // Statement 3 is on a poisoned connection → "relation ... does not exist".
    let r3: Result<i64, _> = sqlx::query_scalar("SELECT COUNT(*) FROM portfolios")
        .fetch_one(&pool)
        .await;
    let err = r3.expect_err("third table-touching statement should fail");
    let msg = err.to_string();
    assert!(
        msg.contains("does not exist")
            || msg.contains("invalid input syntax")
            || msg.contains("schema"),
        "expected schema/relation error, got: {msg}"
    );
}

#[tokio::test]
async fn hard_fail_after_pool_returns_error_after_k_successful_acquires() {
    let Some(url) = test_url() else {
        return;
    };

    let pool = hard_fail_after_pool(&url, 1);

    // First acquire succeeds.
    let r1: Result<i64, _> = sqlx::query_scalar("SELECT 1::int8").fetch_one(&pool).await;
    assert!(r1.is_ok());

    // Second acquire fails.
    let r2: Result<i64, _> = sqlx::query_scalar("SELECT 1::int8").fetch_one(&pool).await;
    let err = r2.expect_err("second acquire should fail");
    let msg = err.to_string();
    assert!(
        msg.to_lowercase().contains("timed out") || msg.contains("fault-injection"),
        "expected pool timed out / fault-injection error, got: {msg}"
    );
}
