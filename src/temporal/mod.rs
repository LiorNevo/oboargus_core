//! Bitemporal market data and the intraday time model.
//!
//! Implements `specs/021-bitemporal-market-data/spec.md`. This module is the
//! append-only, instant-based counterpart to [`crate::domain`]: every fact
//! carries an independent valid time (`valid_at`) and transaction time
//! (`known_at`), and resolution happens at read time using a single documented
//! rule (see [`resolution`](crate::temporal::resolution)).
//!
//! The existing `crate::domain` surface keeps its signatures; the temporal
//! module is additive.

/// The shared append-only instant model (`ValidAt`, `KnownAt`,
/// `TemporalWindow`).
pub mod error;
/// Bitemporal facts for assets, issuers, and portfolios.
pub mod facts;
/// Feed orchestration and stream adaptation (delivered by US5).
pub mod ingest;
/// The UTC instant newtypes and the precision guard.
pub mod instant;
/// Intraday market observations (delivered by US3).
pub mod observations;
/// Position and valuation history (delivered by US4).
pub mod positions;
/// Bitemporal resolution: the single winner-selection rule.
pub mod resolution;
/// Retention, rollup, and archive-aware reads (delivered by US6).
pub mod retention;
/// Operator-facing integrity checks (parity and look-ahead).
pub mod verify;

pub use error::TemporalError;
pub use facts::{FactPoint, FactSubject};
pub use instant::{KnownAt, TemporalWindow, ValidAt};
