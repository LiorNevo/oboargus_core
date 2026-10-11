//! Check that the named database is empty (zero rows across all
//! high-level tables). Prints a one-line summary and exits 0 if
//! clean, 1 if not.

use sqlx::PgPool;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("DATABASE_URL").or_else(|_| std::env::var("TEST_DATABASE_URL"))?;
    let pool = PgPool::connect(&url).await?;
    let tables = [
        "asset_reference_data",
        "issuer_reference_data",
        "portfolio_reference_data",
        "positions",
        "events",
        "assets",
        "issuers",
        "portfolios",
    ];
    let mut total: i64 = 0;
    for table in tables {
        let n: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {}", table))
            .fetch_one(&pool)
            .await?;
        total += n;
    }
    let masked = mask_url(&url);
    if total == 0 {
        println!("OK: {} is empty (0 rows across all tables).", masked);
        Ok(())
    } else {
        println!("DIRTY: {} has {} rows across all tables.", masked, total);
        std::process::exit(1);
    }
}

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
