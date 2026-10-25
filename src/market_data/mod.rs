//! Source-agnostic market data interface for obolargus.
//!
//! Implements US1..US5 of `specs/017-yahoo-data-source/spec.md`. The `trait_`
//! module exposes the `MarketDataProvider` trait; `registry` is the
//! consumer-facing façade that walks configured providers; `refresh`,
//! `reconcile`, and `audit` are the operational entry points. `load` is the
//! batch reference-data load of spec 018, and `load_range` drives it once per
//! calendar day over a requested range.
//!
//! This module sits on top of the predecessor's `domain::asset_reference_data`
//! (writes go through `create_asset_reference_data`). It keeps zero direct
//! third-party dependencies — concrete providers live in
//! `obolargus_server::market_data_provider` to honor `obolargus_core`'s
//! dependency-light rule.

#![forbid(unsafe_code)]

pub mod audit;
pub mod config;
pub mod data_sources;
pub mod error;
pub mod load;
pub mod load_range;
pub mod reconcile;
pub mod refresh;
pub mod registry;
pub mod trait_;
pub mod types;

pub use audit::{
    DataSourceRefreshCounters, ReconciliationAuditRow, RefreshRunRow, finish_refresh_run,
    record_reconciliation_audit, start_refresh_run,
};
pub use config::{DataSourceConfig, MarketDataConfig};
pub use data_sources::{DataSourceRow, list_data_sources, upsert_data_source};
pub use error::MarketDataProviderError;
pub use load::{
    AssetCreation, CreationDecline, CurrencySource, DEFAULT_ASSET_CURRENCY, IdentifierLoadConfig,
    IdentifierLoadReport, LoadIdentifierOutcome, LoadOutcome, RunStatus, run_identifier_load,
};
pub use load_range::{
    DateLoadSection, IdentifierRangeLoadConfig, IdentifierRangeLoadReport, MAX_CONCURRENCY,
    RangeLoadTotals, run_identifier_range_load,
};
pub use reconcile::{ReconcileConfig, ReconcileSummary, ReconciliationPolicy, run_reconcile};
pub use refresh::{RefreshConfig, RefreshSummary, run_refresh};
pub use registry::{DataSourceRegistry, SourceDescriptor};
pub use trait_::MarketDataProvider;
pub use types::{
    HistoricalRange, HistoricalRangeRequest, LatestBar, LatestBarRequest, PriceBar,
    TickerLookupHit, TickerLookupRequest, TickerLookupResult, TypedIdentifier,
};
