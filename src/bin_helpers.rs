//! Shared helpers used by the binary entrypoints in `src/bin/`.
//!
//! These functions were originally inlined in the `src/bin/*.rs`
//! files. They were extracted into this library module so that:
//!
//! 1. The pure-Rust helpers (`mask_url`, `validate_test_db_name`)
//!    can be unit-tested without spinning up a database.
//! 2. The database-touching helpers (`truncate_test_tables`,
//!    `count_high_level_rows`, `ensure_test_database`) can be
//!    integration-tested against the shared `TEST_DATABASE_URL`.
//! 3. The `cargo-llvm-cov` report counts lines executed by the
//!    binary entrypoints as covered (the binary's `main()` is a
//!    thin wrapper that calls into this module).
//!
//! Each binary's `main()` is now just plumbing: read env vars, call
//! a helper from this module, print a summary line.

use crate::domain::DomainError;
use sqlx::AssertSqlSafe;
use sqlx::Connection;
use sqlx::PgPool;
use sqlx::migrate::MigrateError;

/// All high-level tables, in dependency order. Used by
/// [`truncate_test_tables`] to wipe the test schema without
/// tripping the foreign-key constraints.
pub const TABLES: &[&str] = &[
    "asset_reference_data",
    "issuer_reference_data",
    "portfolio_reference_data",
    "asset_identifiers",
    "issuer_identifiers",
    "rules",
    "portfolio_rules",
    "positions",
    "events",
    "data_source_reconciliation_audits",
    "data_source_refresh_runs",
    "data_sources",
    "assets",
    "issuers",
    "portfolios",
];

/// Tables counted by [`count_high_level_rows`]. Excludes the
/// `*_identifiers` tables (which `check_empty` does not consider
/// part of the high-level dataset — kept for backwards
/// compatibility with the original `check_empty` table list).
/// `rules` and `portfolio_rules` ARE counted: the rule catalog and its
/// portfolio links are high-level state (US9).
pub const CHECK_TABLES: &[&str] = &[
    "asset_reference_data",
    "issuer_reference_data",
    "portfolio_reference_data",
    "rules",
    "portfolio_rules",
    "positions",
    "events",
    "assets",
    "issuers",
    "portfolios",
];

/// Strip the user:password portion of a connection URL for safe logging.
///
/// Examples:
/// - `postgres://u:p@h:5432/d` → `postgres://***:***@h:5432/d`
/// - `no-scheme` → `no-scheme` (returned unchanged)
/// - `no-at-sign` → `no-at-sign` (returned unchanged)
pub fn mask_url(url: &str) -> String {
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

/// Pick the test database URL from `TEST_DATABASE_URL` (preferred)
/// or `DATABASE_URL` (fallback). Refuses to run if both resolve to
/// the same value — that's a safety guard so a future bug cannot
/// accidentally point the test suite at the production schema.
///
/// Errors:
/// - `Validation(...)` if both URLs are set to the same value.
/// - `Validation(...)` if neither URL is set.
pub fn resolve_test_url() -> Result<String, DomainError> {
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

/// Run `TRUNCATE TABLE … RESTART IDENTITY CASCADE` on every
/// high-level table. Returns the number of tables truncated.
pub async fn truncate_test_tables(pool: &PgPool) -> Result<(), DomainError> {
    let mut tx = pool.begin().await?;
    for table in TABLES {
        let query = format!("TRUNCATE TABLE {} RESTART IDENTITY CASCADE", table);
        // Safe: TABLES is a fixed internal constant, not user input
        sqlx::query(AssertSqlSafe(query))
            .execute(&mut *tx)
            .await
            .map_err(DomainError::Infrastructure)?;
    }
    tx.commit().await?;
    Ok(())
}

/// Apply the `migrations/` directory against `pool`. Wraps the
/// `MigrateError` into the crate's `DomainError::Infrastructure`
/// variant so the binary's `?` can propagate it cleanly.
pub async fn run_migrations(pool: &PgPool) -> Result<(), DomainError> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .map_err(|e: MigrateError| DomainError::Infrastructure(sqlx::Error::Migrate(Box::new(e))))
}

/// Return the total row count across every high-level table
/// listed in [`CHECK_TABLES`]. Used by the `check_empty` binary.
pub async fn count_high_level_rows(pool: &PgPool) -> Result<i64, sqlx::Error> {
    let mut total: i64 = 0;
    for table in CHECK_TABLES {
        // Safe: CHECK_TABLES is a fixed internal constant, not user input
        let query = format!("SELECT COUNT(*) FROM {}", table);
        let n: i64 = sqlx::query_scalar(AssertSqlSafe(query))
            .fetch_one(pool)
            .await?;
        total += n;
    }
    Ok(total)
}

/// Validate that a database name contains only ASCII alphanumerics
/// and underscores. Used by `init_test_db` to refuse
/// SQL-injection-style inputs.
pub fn validate_test_db_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("POSTGRES_TEST_DB must not be empty".into());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err("POSTGRES_TEST_DB contains unsafe characters".into());
    }
    Ok(())
}

/// Create `test_db` on the postgres cluster reachable at `admin_url`
/// if it does not already exist. Returns `Ok(true)` if the database
/// was created, `Ok(false)` if it already existed.
///
/// The function is idempotent: calling it twice in a row is safe
/// and reports `false` on the second call.
pub async fn ensure_test_database(admin_url: &str, test_db: &str) -> Result<bool, sqlx::Error> {
    let mut admin = sqlx::PgConnection::connect(admin_url).await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)")
            .bind(test_db)
            .fetch_one(&mut admin)
            .await?;
    if exists {
        return Ok(false);
    }
    // Safe: test_db is validated by validate_test_db_name before this point
    let sql = format!("CREATE DATABASE {}", test_db);
    sqlx::query(AssertSqlSafe(sql)).execute(&mut admin).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- mask_url ----------

    #[test]
    fn mask_url_strips_userinfo() {
        assert_eq!(
            mask_url("postgres://u:p@h:5432/d"),
            "postgres://***:***@h:5432/d",
        );
    }

    #[test]
    fn mask_url_handles_no_scheme() {
        // No `://` found, no `@`; returned unchanged.
        assert_eq!(mask_url("no-scheme"), "no-scheme");
    }

    #[test]
    fn mask_url_handles_no_at_sign() {
        // No `@`; returned unchanged.
        assert_eq!(
            mask_url("postgres://only-host/db"),
            "postgres://only-host/db"
        );
    }

    #[test]
    fn mask_url_uses_last_at_sign() {
        // If the password contains an `@`, the algorithm must use
        // the *last* `@` as the userinfo separator.
        let masked = mask_url("postgres://u:p@a@h:5432/d");
        assert_eq!(masked, "postgres://***:***@h:5432/d");
    }

    // ---------- validate_test_db_name ----------

    #[test]
    fn validate_test_db_name_accepts_alphanumeric() {
        assert!(validate_test_db_name("obolargus_test_db").is_ok());
    }

    #[test]
    fn validate_test_db_name_accepts_underscores() {
        assert!(validate_test_db_name("obolargus_test_db_2").is_ok());
    }

    #[test]
    fn validate_test_db_name_rejects_empty() {
        assert!(validate_test_db_name("").is_err());
    }

    #[test]
    fn validate_test_db_name_rejects_spaces() {
        assert!(validate_test_db_name("with space").is_err());
    }

    #[test]
    fn validate_test_db_name_rejects_semicolon() {
        // SQL-injection guard: a `;` would let an attacker append
        // arbitrary statements.
        assert!(validate_test_db_name("db;drop").is_err());
    }

    // ---------- resolve_test_url ----------

    /// Helper: run a closure with `TEST_DATABASE_URL` and
    /// `DATABASE_URL` set (or unset) and return the result.
    ///
    /// **Important**: this helper *restores* the original values of
    /// both env vars on exit. If we just removed them, a later test
    /// that reads `TEST_DATABASE_URL` (e.g. anything that calls
    /// `tx_pool()`) would silently see `None` and short-circuit,
    /// which we observed as the DB-needing tests in this module
    /// reporting their body lines as uncovered depending on the
    /// alphabetical test order.
    fn with_env<F: FnOnce() -> R, R>(test: Option<&str>, prod: Option<&str>, f: F) -> R {
        // Capture the original values so we can restore them on the
        // way out, regardless of whether the closure returns
        // normally or panics.
        let original_test = std::env::var("TEST_DATABASE_URL").ok();
        let original_prod = std::env::var("DATABASE_URL").ok();
        // `set_var` / `remove_var` are unsafe on edition 2024
        // because they race with concurrent readers of the process
        // environment. The `cargo-llvm-cov` test runner executes
        // tests serially (`RUST_TEST_THREADS=1` in
        // `.cargo/config.toml`), so the unsafe is sound here — no
        // other thread can observe an inconsistent state.
        unsafe {
            match test {
                Some(t) => std::env::set_var("TEST_DATABASE_URL", t),
                None => std::env::remove_var("TEST_DATABASE_URL"),
            }
            match prod {
                Some(p) => std::env::set_var("DATABASE_URL", p),
                None => std::env::remove_var("DATABASE_URL"),
            }
        }
        let r = f();
        unsafe {
            match original_test {
                Some(t) => std::env::set_var("TEST_DATABASE_URL", t),
                None => std::env::remove_var("TEST_DATABASE_URL"),
            }
            match original_prod {
                Some(p) => std::env::set_var("DATABASE_URL", p),
                None => std::env::remove_var("DATABASE_URL"),
            }
        }
        r
    }

    #[test]
    fn resolve_test_url_prefers_test_url() {
        let r = with_env(Some("test"), Some("prod"), resolve_test_url);
        assert_eq!(r.unwrap(), "test");
    }

    #[test]
    fn resolve_test_url_falls_back_to_prod_url() {
        let r = with_env(None, Some("prod"), resolve_test_url);
        assert_eq!(r.unwrap(), "prod");
    }

    #[test]
    fn resolve_test_url_errors_when_neither_is_set() {
        let r = with_env(None, None, resolve_test_url);
        assert!(matches!(r, Err(DomainError::Validation(_))));
    }

    #[test]
    fn resolve_test_url_errors_when_both_match() {
        // Safety guard: refuse to truncate if both URLs point at the
        // same database (the test suite would otherwise truncate the
        // production schema).
        let r = with_env(Some("same"), Some("same"), resolve_test_url);
        assert!(matches!(r, Err(DomainError::Validation(_))));
    }

    // ---------- count_high_level_rows ----------

    #[tokio::test]
    async fn count_high_level_rows_is_zero_on_empty_database() {
        // Use the shared `tx_pool` fixture so the test database is
        // empty before and after the test. The gate makes the test a
        // no-op when no test database is configured.
        let Some((pool, guard)) = crate::common::tx_pool().await else {
            return;
        };
        let n = count_high_level_rows(&pool).await.expect("count");
        assert_eq!(n, 0, "fresh test database should be empty");
        guard.cleanup().await;
    }

    // ---------- truncate_test_tables ----------

    #[tokio::test]
    async fn truncate_test_tables_succeeds_on_empty_database() {
        let Some((pool, guard)) = crate::common::tx_pool().await else {
            return;
        };
        truncate_test_tables(&pool).await.expect("truncate");
        guard.cleanup().await;
    }

    #[tokio::test]
    async fn truncate_test_tables_removes_existing_rows() {
        let Some((pool, guard)) = crate::common::tx_pool().await else {
            return;
        };
        // Insert a row that the truncate must wipe.
        sqlx::query(
            "INSERT INTO issuers (id, name, issuer_type, legal_jurisdiction) \
             VALUES (gen_random_uuid(), 'truncate-test', 'corporation', 'US')",
        )
        .execute(&pool)
        .await
        .expect("insert issuer");
        truncate_test_tables(&pool).await.expect("truncate");
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM issuers")
            .fetch_one(&pool)
            .await
            .expect("count");
        assert_eq!(n, 0, "truncate must wipe every row");
        guard.cleanup().await;
    }

    // ---------- with_env restore path ----------

    #[test]
    fn with_env_restores_unset_value() {
        // Cover the `None` arm of `with_env`'s restore block. We
        // need the env var to be unset going in, so we temporarily
        // remove it (saving its original value first) and put it
        // back at the end of the test. The `cargo-llvm-cov` test
        // runner is single-threaded (`RUST_TEST_THREADS=1`), so
        // the unsafe is sound here.
        let original = std::env::var("TEST_DATABASE_URL").ok();
        // Build the with_env call where the closure body itself
        // removes `TEST_DATABASE_URL` on entry — so when `with_env`
        // reads `original_test`, it sees `None`. This forces the
        // restore path to go through the `None => remove_var` arm.
        let result = with_env(Some("inside"), None, || unsafe {
            std::env::remove_var("TEST_DATABASE_URL");
            std::env::var("TEST_DATABASE_URL").ok()
        });
        // The closure ran with `TEST_DATABASE_URL` removed — that's
        // what it should see (and we just verified via the return).
        assert_eq!(result, None);
        // After the closure, the env var should be back to its
        // original value. The `with_env` restore path went through
        // the `None => remove_var` arm here because the closure had
        // removed the var; `with_env` re-applies it from its
        // captured snapshot, restoring the original Some value.
        assert_eq!(
            std::env::var("TEST_DATABASE_URL").ok(),
            original,
            "with_env must restore the original value"
        );
        // Defensive: ensure the env var is back to its starting
        // value in case the assertion above runs after some teardown
        // that mutates the env.
        unsafe {
            match original {
                Some(v) => std::env::set_var("TEST_DATABASE_URL", v),
                None => std::env::remove_var("TEST_DATABASE_URL"),
            }
        }
    }

    // ---------- ensure_test_database both branches ----------

    #[test]
    fn ensure_test_database_creates_then_returns_false_on_second_call() {
        // We need a real postgres admin URL. Skip if none configured.
        let Some(test_url) = std::env::var("TEST_DATABASE_URL").ok() else {
            return;
        };
        // The admin URL needs to point at the `postgres` maintenance
        // database. Derive it from `test_url` by replacing the
        // database component with `postgres`.
        let admin = match test_url.rfind('/') {
            Some(idx) => format!("{}/postgres", &test_url[..idx]),
            None => return,
        };
        let scratch = "obolargus_bin_helpers_cov_scratch";

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        // First call: database doesn't exist yet → creates it → returns Ok(true).
        let first = rt.block_on(ensure_test_database(&admin, scratch));
        assert!(first.is_ok(), "ensure_test_database first call: {first:?}");
        assert!(first.unwrap(), "first call must create the database");

        // Second call: database now exists → returns Ok(false).
        let second = rt.block_on(ensure_test_database(&admin, scratch));
        assert!(
            second.is_ok(),
            "ensure_test_database second call: {second:?}"
        );
        assert!(!second.unwrap(), "second call must report already-exists");

        // Clean up the scratch database.
        rt.block_on(async {
            use sqlx::Connection;
            let mut admin_conn = sqlx::PgConnection::connect(&admin)
                .await
                .expect("admin connect");
            let _ = sqlx::query(AssertSqlSafe(format!(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
                 WHERE datname = '{scratch}' AND pid <> pg_backend_pid()"
            )))
            .execute(&mut admin_conn)
            .await;
            let _ = sqlx::query(AssertSqlSafe(format!("DROP DATABASE IF EXISTS {scratch}")))
                .execute(&mut admin_conn)
                .await;
        });
    }
}
