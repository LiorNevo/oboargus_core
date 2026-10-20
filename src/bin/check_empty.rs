//! Check that the named database is empty (zero rows across all
//! high-level tables). Prints a one-line summary and exits 0 if
//! clean, 1 if not.
//!
//! This is a **test-database** utility, like its siblings `db_cleanup`
//! and `init_test_db`: it checks `TEST_DATABASE_URL` first and falls
//! back to `DATABASE_URL`. The test-first order matters — the test
//! suite runs with both variables set, and a check that silently
//! inspected the *production* database would assert on data the suite
//! does not own (and must never truncate).

use obolargus_core::bin_helpers::{count_high_level_rows, mask_url};
use sqlx::PgPool;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("TEST_DATABASE_URL").or_else(|_| std::env::var("DATABASE_URL"))?;
    let pool = PgPool::connect(&url).await?;
    let total = count_high_level_rows(&pool).await?;
    let masked = mask_url(&url);
    if total == 0 {
        println!("OK: {} is empty (0 rows across all tables).", masked);
        Ok(())
    } else {
        println!("DIRTY: {} has {} rows across all tables.", masked, total);
        std::process::exit(1);
    }
}
