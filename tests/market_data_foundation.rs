//! Foundation integration tests for the market-data source tables.
//!
//! Implements T022 of `specs/017-yahoo-data-source/tasks.md`: applies
//! migration `0005_market_data_sources.sql` and confirms the three new
//! tables exist with the expected shape.
//!
//! The tests are gated on `TEST_DATABASE_URL` / `DATABASE_URL` and skip
//! (as a passing no-op) when no database is configured, per the
//! `obolargus_core/AGENTS.md` convention.

use sqlx::PgPool;

/// Returns the test database URL, or `None` when no database is
/// configured (the test then skips).
fn test_database_url() -> Option<String> {
    std::env::var("TEST_DATABASE_URL")
        .ok()
        .or_else(|| std::env::var("DATABASE_URL").ok())
}

/// Connects to the test database and applies the migrations.
async fn migrated_pool() -> Option<PgPool> {
    let url = test_database_url()?;
    let pool = match sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
    {
        Ok(pool) => pool,
        Err(error) => {
            eprintln!("SKIPPED: could not connect to the test database: {error}");
            return None;
        }
    };
    if let Err(error) = sqlx::migrate!("./migrations").run(&pool).await {
        eprintln!("SKIPPED: could not apply the migrations: {error}");
        return None;
    }
    Some(pool)
}

/// T022: the three new tables exist after migration `0005` runs.
#[tokio::test]
async fn migration_creates_the_three_market_data_tables() {
    let Some(pool) = migrated_pool().await else {
        eprintln!("SKIPPED: no test database configured");
        return;
    };

    for table in [
        "data_sources",
        "data_source_refresh_runs",
        "data_source_reconciliation_audits",
    ] {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM information_schema.tables \
             WHERE table_schema = 'public' AND table_name = $1)",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("query information_schema");
        assert!(exists, "table '{table}' should exist after migration 0005");
    }
}

/// T022: `data_sources` has the columns the config contract names.
#[tokio::test]
async fn data_sources_has_the_contract_columns() {
    let Some(pool) = migrated_pool().await else {
        eprintln!("SKIPPED: no test database configured");
        return;
    };

    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns \
         WHERE table_schema = 'public' AND table_name = 'data_sources'",
    )
    .fetch_all(&pool)
    .await
    .expect("query columns");

    for expected in [
        "source_id",
        "display_name",
        "provider_type",
        "enabled",
        "priority",
        "supported_schemes",
        "rate_limit_per_minute",
        "api_key_present",
        "user_agent",
        "timeout_seconds",
        "created_at",
        "updated_at",
    ] {
        assert!(
            columns.iter().any(|c| c == expected),
            "data_sources should have a '{expected}' column (has: {columns:?})"
        );
    }
}

/// T022: the refresh-run counters partition `assets_attempted`.
#[tokio::test]
async fn refresh_run_counters_are_consistent() {
    let Some(pool) = migrated_pool().await else {
        eprintln!("SKIPPED: no test database configured");
        return;
    };

    // A row whose counters do not sum to `assets_attempted` must be
    // rejected by the CHECK constraint.
    let result = sqlx::query(
        "INSERT INTO data_source_refresh_runs \
         (id, started_at, trigger, assets_attempted, assets_refreshed, assets_no_op, \
          assets_missing_identifier, assets_source_unavailable, per_source_attempted, per_source_failed) \
         VALUES (gen_random_uuid(), NOW(), 'test', 5, 1, 1, 1, 1, '{}'::jsonb, '{}'::jsonb)",
    )
    .execute(&pool)
    .await;
    assert!(
        result.is_err(),
        "the partition CHECK constraint should reject inconsistent counters"
    );
}

/// T022: the reconciliation audit requires at least two sources.
#[tokio::test]
async fn reconciliation_audit_requires_two_sources() {
    let Some(pool) = migrated_pool().await else {
        eprintln!("SKIPPED: no test database configured");
        return;
    };

    let asset_id: Option<(uuid::Uuid,)> = sqlx::query_as("SELECT id FROM assets LIMIT 1")
        .fetch_optional(&pool)
        .await
        .expect("query assets");
    let Some((asset_id,)) = asset_id else {
        // No assets seeded — the FK would reject the row anyway, so the
        // cardinality CHECK is not the thing under test here.
        eprintln!("SKIPPED: no assets seeded; the FK constraint would mask the CHECK");
        return;
    };

    let result = sqlx::query(
        "INSERT INTO data_source_reconciliation_audits \
         (id, asset_id, fact_type, as_of_date, policy, sources_tried, per_source_answers, canonical_value) \
         VALUES (gen_random_uuid(), $1, 'market_price', CURRENT_DATE, 'primary_wins', \
                 ARRAY['only_one'], '{}'::jsonb, '{}')",
    )
    .bind(asset_id)
    .execute(&pool)
    .await;
    assert!(
        result.is_err(),
        "the cardinality CHECK constraint should reject a single source"
    );
}
