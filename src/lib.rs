//! obolargus-core: shared financial domain library.
//!
//! This skeleton crate is the shared dependency target of
//! [`obolargus-server`](https://github.com/LiorNevo/obolargus-server) and
//! [`obolargus-cli`](https://github.com/LiorNevo/obolargus-cli). It
//! intentionally carries no business logic yet; domain models and behavior are
//! specified in `specs/architecture/entity-framework.md`.

/// Precise decimal parsing for monetary values (no floating point).
pub mod decimal;

/// The crate version, taken from `Cargo.toml` at compile time.
///
/// Kept as a stable constant so generated documentation and health checks
/// report the exact runtime version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
