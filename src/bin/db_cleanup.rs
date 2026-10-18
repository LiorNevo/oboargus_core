//! One-shot database cleanup utility.
//!
//! Truncates every high-level table in the **test** database. Gated
//! by `TEST_DATABASE_URL` (with `DATABASE_URL` as a fallback for
//! backward compatibility). Refuses to run if pointed at the
//! production database (it will exit with an error if
//! `TEST_DATABASE_URL` and `DATABASE_URL` resolve to the same target —
//! this is a safety guard so a future bug cannot accidentally truncate
//! the production schema).
//!
//! Usage:
//!     TEST_DATABASE_URL=postgres://… cargo run -p obolargus-core --bin db_cleanup

use obolargus_core::bin_helpers::{
    TABLES, mask_url, resolve_test_url, run_migrations, truncate_test_tables,
};
use obolargus_core::domain::DomainError;
use sqlx::PgPool;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), DomainError> {
    let url = resolve_test_url()?;
    let pool = PgPool::connect(&url).await?;
    run_migrations(&pool).await?;
    truncate_test_tables(&pool).await?;
    println!(
        "db-cleanup: truncated {} tables in the test database ({}); schema is unchanged.",
        TABLES.len(),
        mask_url(&url),
    );
    Ok(())
}
