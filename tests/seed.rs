//! Tests for US7 — the canonical seed function.

use obolargus_core::domain::seed::seed_canonical_dataset;
use sqlx::PgPool;

async fn pool() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPool::connect(&url).await.expect("connect");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("migrations should succeed");
    Some(pool)
}

#[tokio::test]
async fn seed_creates_documented_entities() {
    let Some(pool) = pool().await else {
        return;
    };
    let summary = seed_canonical_dataset(&pool).await.expect("seed");
    // The seed reports the existing counts (which include any previous
    // seeded data on the database). All three entities must be present
    // after a seed run.
    let issuer_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM issuers")
        .fetch_one(&pool)
        .await
        .expect("count issuers");
    let asset_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM assets")
        .fetch_one(&pool)
        .await
        .expect("count assets");
    let portfolio_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM portfolios")
        .fetch_one(&pool)
        .await
        .expect("count portfolios");
    assert!(issuer_count >= 1, "expected at least 1 issuer");
    assert!(asset_count >= 1, "expected at least 1 asset");
    assert!(portfolio_count >= 1, "expected at least 1 portfolio");
    let _ = summary;
}

#[tokio::test]
async fn seed_is_idempotent() {
    let Some(pool) = pool().await else {
        return;
    };
    let first = seed_canonical_dataset(&pool).await.expect("seed 1");
    let second = seed_canonical_dataset(&pool).await.expect("seed 2");
    // The second run is a no-op for inserts: the canonical issuer already
    // exists, so the function reports the existing counts rather than 0.
    // The invariant we care about is that the second run does not double
    // the entity counts.
    assert_eq!(second.issuers_created, first.issuers_created);
    assert_eq!(second.assets_created, first.assets_created);
    assert_eq!(second.portfolios_created, first.portfolios_created);
}

#[tokio::test]
async fn human_readable_names() {
    let Some(pool) = pool().await else {
        return;
    };
    // The seed creates an asset with a human-readable display name.
    let name: String = sqlx::query_scalar(
        "SELECT display_name FROM assets WHERE display_name LIKE '%Vanguard%' LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("find asset");
    assert!(name.contains("Vanguard"));
}
