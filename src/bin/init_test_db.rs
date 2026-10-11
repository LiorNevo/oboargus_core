//! One-shot helper: create the `obolargus_test_db` database on the live
//! postgres. Idempotent (does nothing if the database already exists).
//!
//! Usage:
//!     cargo run -p obolargus-core --bin init_test_db

use sqlx::Connection;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = std::env::var("POSTGRES_HOST").unwrap_or_else(|_| "postgres".to_string());
    let port = std::env::var("POSTGRES_PORT").unwrap_or_else(|_| "5432".to_string());
    let user = std::env::var("POSTGRES_USER").unwrap_or_else(|_| "postgres_user".to_string());
    let pass =
        std::env::var("POSTGRES_PASSWORD").unwrap_or_else(|_| "postgres_db_password".to_string());
    let test_db =
        std::env::var("POSTGRES_TEST_DB").unwrap_or_else(|_| "obolargus_test_db".to_string());

    let admin_url = format!("postgres://{user}:{pass}@{host}:{port}/postgres");
    let mut conn = sqlx::PgConnection::connect(&admin_url).await?;

    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)")
            .bind(&test_db)
            .fetch_one(&mut conn)
            .await?;
    if exists {
        println!("init_test_db: {} already exists; nothing to do.", test_db);
    } else {
        if !test_db
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err("POSTGRES_TEST_DB contains unsafe characters".into());
        }
        let sql = format!("CREATE DATABASE {}", test_db);
        sqlx::query(&sql).execute(&mut conn).await?;
        println!("init_test_db: created {}.", test_db);
    }
    Ok(())
}
