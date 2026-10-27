//! obolargus-core: shared financial domain library.
//!
//! This skeleton crate is the shared dependency target of
//! [`obolargus-server`](https://github.com/LiorNevo/obolargus-server) and
//! [`obolargus-cli`](https://github.com/LiorNevo/obolargus-cli). It
//! intentionally carries no business logic yet; domain models and behavior are
//! specified in `specs/architecture/entity-framework.md`.

/// Precise decimal parsing for monetary values (no floating point).
pub mod decimal;

/// The high-level financial domain model (see `specs/015-database-uml-domains/spec.md`).
pub mod domain;

/// Source-agnostic market data interface (see `specs/017-yahoo-data-source/spec.md`).
pub mod market_data;

/// Bitemporal market data and the intraday time model.
///
/// See `specs/021-bitemporal-market-data/spec.md`. Additive to [`domain`]:
/// the per-entity `domain::*` functions keep their signatures and behavior.
pub mod temporal;

/// Helpers shared by the binary entrypoints in `src/bin/`.
///
/// Extracted from `db_cleanup.rs`, `init_test_db.rs`, and
/// `check_empty.rs` so that the helper logic is unit-tested and so
/// that `cargo-llvm-cov` credits the binaries' code paths against
/// the report when the binaries are invoked. See the module docs
/// for details.
pub mod bin_helpers;

// Test-only shared fixture. Each per-entity unit test file under
// `src/domain/<x>/tests.rs` includes this module locally via
// `#[path = "..."] mod common;`, which is the standard cargo pattern
// for sharing test code across files in a single crate. Each
// `#[path = "..."]` inclusion is treated by rustc as a separate
// module declaration, and `clippy::duplicate_mod` flags that as
// duplication. We allow the lint here because the duplication is
// deliberate: each test file is its own compilation unit, and the
// fixture's runtime cost is negligible.
//
// The module is `pub` so that external integration tests under
// `tests/` can reach it via `obolargus_core::common::*`. The body
// is gated by `#[cfg(test)]` so it is not compiled into the release
// build.
#[allow(clippy::duplicate_mod)]
pub mod common;

/// The crate version, taken from `Cargo.toml` at compile time.
///
/// Kept as a stable constant so generated documentation and health checks
/// report the exact runtime version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

// No re-export needed: each test target (lib test + each integration
// test binary under `tests/`) sees the `common` module via its own
// `#[path = "..."] mod common;` inclusion. The per-entity unit
// tests under `src/domain/<x>/tests.rs` and the integration test
// `tests/seed.rs` each declare `mod common;` locally, which is the
// standard cargo pattern. We accept the `clippy::duplicate_mod`
// warning at the `pub mod common;` declaration below because the
// duplication is by design.
