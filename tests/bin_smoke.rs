//! Smoke tests for the binary entrypoints in `src/bin/`.
//!
//! Each binary is instrumented by `cargo-llvm-cov`, but its `main()`
//! body only gets credited when the binary is actually executed.
//! This file spawns each binary once so that the coverage report
//! accounts for the `main()` bodies (and the library helpers they
//! call into).
//!
//! The tests are gated on the same env vars as the binaries themselves
//! — they short-circuit to a passing no-op when the test database is
//! not configured.
//!
//! Note: this integration test file lives under `tests/` and is
//! therefore excluded from `cargo-llvm-cov`'s own report (the default
//! ignore regex skips `tests/*`). What matters for coverage is that
//! running this test executes the binaries, which writes coverage
//! data for `src/bin/*.rs` and `src/bin_helpers.rs`.

use std::process::Command;

/// Run a binary built by cargo with the inherited environment.
/// Returns the exit status. The `CARGO_BIN_EXE_<name>` env var is
/// set by cargo for every integration test that uses a binary built
/// by the same package.
fn run_binary(name: &str, args: &[&str]) -> std::io::Result<std::process::ExitStatus> {
    let bin = std::env::var(format!("CARGO_BIN_EXE_{name}"))
        .unwrap_or_else(|_| panic!("CARGO_BIN_EXE_{name} not set; cargo did not build it"));
    Command::new(bin).args(args).status()
}

/// Skip if no test database is configured.
fn db_env_present() -> bool {
    std::env::var("TEST_DATABASE_URL").is_ok()
        || std::env::var("DATABASE_URL").is_ok()
        || std::env::var("POSTGRES_HOST").is_ok()
}

#[test]
fn check_empty_binary_succeeds_on_an_empty_database() {
    // The check must run against the TEST database. Asserting on the
    // production database from a test would be wrong on two counts: the
    // suite does not own that data, and it must never truncate it.
    if std::env::var("TEST_DATABASE_URL").is_err() {
        return;
    }
    // `db_cleanup` applies the migrations and truncates the test
    // database, so the check that follows starts from a known-empty
    // database with a current schema — and does not depend on the
    // order in which the test binaries happen to run.
    let cleanup = run_binary("db_cleanup", &[]).expect("spawn db_cleanup");
    assert!(
        cleanup.success(),
        "db_cleanup should exit 0 so the test database is empty (got {cleanup:?})"
    );
    let status = run_binary("check_empty", &[]).expect("spawn check_empty");
    assert!(
        status.success(),
        "check_empty on an empty database should exit 0 (got {status:?})"
    );
}

#[test]
fn db_cleanup_binary_truncates_high_level_tables() {
    if !db_env_present() {
        return;
    }
    let status = run_binary("db_cleanup", &[]).expect("spawn db_cleanup");
    assert!(
        status.success(),
        "db_cleanup should exit 0 on success (got {status:?})"
    );
}

#[test]
fn init_test_db_binary_reports_existing_database() {
    // `init_test_db` connects to the admin `postgres` database to
    // check whether the test database exists. It needs at least one
    // of the `POSTGRES_*` env vars; the host defaults to `postgres`
    // inside the docker network.
    if std::env::var("TEST_DATABASE_URL").is_err() && std::env::var("POSTGRES_HOST").is_err() {
        return;
    }
    let status = run_binary("init_test_db", &[]).expect("spawn init_test_db");
    assert!(
        status.success(),
        "init_test_db against an existing database should exit 0 (got {status:?})"
    );
}
