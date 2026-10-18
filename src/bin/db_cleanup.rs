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

use obolargus_core::domain::DomainError;
use sqlx::PgPool;
use sqlx::migrate::MigrateError;

const TABLES: &[&str] = &[
    "asset_reference_data",
    "issuer_reference_data",
    "portfolio_reference_data",
    "asset_identifiers",
    "issuer_identifiers",
    "positions",
    "events",
    "assets",
    "issuers",
    "portfolios",
];

/// Safety check: refuse to run if `TEST_DATABASE_URL` and
/// `DATABASE_URL` both resolve to the same value. This catches
/// misconfiguration where someone has pointed the test suite at
/// the production database by accident.
fn resolve_test_url() -> Result<String, DomainError> {
    let test_url = std::env::var("TEST_DATABASE_URL").ok();
    let prod_url = std::env::var("DATABASE_URL").ok();
    match (test_url, prod_url) {
        (Some(t), Some(p)) if t == p => Err(DomainError::Validation(format!(
            "TEST_DATABASE_URL and DATABASE_URL resolve to the same value ({t}); refusing to truncate. Set TEST_DATABASE_URL to a separate test database.",
        ))),
        (Some(t), _) => Ok(t),
        (None, Some(p)) => {
            eprintln!(
                "warning: TEST_DATABASE_URL is not set; falling back to DATABASE_URL ({}).",
                mask_url(&p)
            );
            Ok(p)
        }
        (None, None) => Err(DomainError::Validation(
            "TEST_DATABASE_URL (or DATABASE_URL) must be set to run db_cleanup".to_string(),
        )),
    }
}

/// Strip the user:password portion of a connection URL for safe logging.
fn mask_url(url: &str) -> String {
    if let Some(at) = url.rfind('@')
        && let Some(slash) = url[..at].rfind("://")
    {
        let mut s = String::from(&url[..slash + 3]);
        s.push_str("***:***@");
        s.push_str(&url[at + 1..]);
        return s;
    }
    url.to_string()
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), DomainError> {
    let url = resolve_test_url()?;
    let pool = PgPool::connect(&url).await?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .map_err(|e: MigrateError| {
            DomainError::Infrastructure(sqlx::Error::Migrate(Box::new(e)))
        })?;
    let mut tx = pool.begin().await?;
    for table in TABLES {
        sqlx::query(&format!(
            "TRUNCATE TABLE {} RESTART IDENTITY CASCADE",
            table
        ))
        .execute(&mut *tx)
        .await
        .map_err(DomainError::Infrastructure)?;
    }
    tx.commit().await?;
    println!(
        "db-cleanup: truncated {} tables in the test database ({}); schema is unchanged.",
        TABLES.len(),
        mask_url(&url),
    );
    Ok(())
}
