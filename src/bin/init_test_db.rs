//! One-shot helper: create the `obolargus_test_db` database on the live
//! postgres. Idempotent (does nothing if the database already exists).
//!
//! Usage:
//!     cargo run -p obolargus-core --bin init_test_db

use obolargus_core::bin_helpers::{ensure_test_database, validate_test_db_name};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = std::env::var("POSTGRES_HOST").unwrap_or_else(|_| "postgres".to_string());
    let port = std::env::var("POSTGRES_PORT").unwrap_or_else(|_| "5432".to_string());
    let user = std::env::var("POSTGRES_USER").unwrap_or_else(|_| "postgres_user".to_string());
    let pass =
        std::env::var("POSTGRES_PASSWORD").unwrap_or_else(|_| "postgres_db_password".to_string());
    let test_db =
        std::env::var("POSTGRES_TEST_DB").unwrap_or_else(|_| "obolargus_test_db".to_string());

    // Reject unsafe names *before* opening a connection so we never
    // even talk to postgres with a potentially-injected identifier.
    validate_test_db_name(&test_db).map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

    let admin_url = format!("postgres://{user}:{pass}@{host}:{port}/postgres");
    let created = ensure_test_database(&admin_url, &test_db).await?;
    if created {
        println!("init_test_db: created {}.", test_db);
    } else {
        println!("init_test_db: {} already exists; nothing to do.", test_db);
    }
    Ok(())
}
