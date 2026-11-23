//! Bitemporal market data and the intraday time model.
//!
//! Implements `specs/021-bitemporal-market-data/spec.md` and the normalized
//! fact model of `specs/027-normalized-fact-model/spec.md`. Every fact carries
//! an independent valid time (`valid_at`) and transaction time (`known_at`),
//! and resolution happens at read time using one documented rule.

/// The machine-readable fact catalog (the data dictionary).
pub mod catalog;
/// Typed errors for the temporal surface.
pub mod error;
/// Typed append-only facts for assets, issuers, and portfolios.
pub mod facts;
/// The UTC instant newtypes and the precision guard.
pub mod instant;
/// Typed market observations (EOD bars, intraday bars, quotes).
pub mod observations;
/// Bitemporal resolution: the single winner-selection rule.
pub mod resolution;
/// Operator-facing integrity checks.
pub mod verify;

pub use error::TemporalError;
pub use facts::{
    Adjustment, FactCoverage, FactInput, FactPoint, FactSeriesPage, FactSubject, FactValue,
    MarketPriceValue, RetractionInput, retract_fact, write_fact, write_fact_outcome,
};
pub use instant::{KnownAt, TemporalWindow, ValidAt};
