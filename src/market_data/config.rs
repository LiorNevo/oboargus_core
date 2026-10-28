//! In-process configuration structs for market data sources.
//!
//! Implements FR-002 / FR-040 of `specs/017-yahoo-data-source/spec.md`.
//!
//! These structures are the in-memory representation. They are loaded from
//! environment variables (`MARKET_DATA_SOURCES` is a JSON array) in
//! `obolargus_server::config` and validated at startup. They are independent
//! of any third-party crate so live in `obolargus_core`.

use serde::{Deserialize, Serialize};

use crate::market_data::error::MarketDataProviderError;

/// Configuration for the market data subsystem as a whole.
///
/// Implements FR-002 of spec 017. Loaded via `envconfig` (see
/// `contracts/config-contract-additions.md` for the env-var schema).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarketDataConfig {
    /// One entry per configured source.
    pub sources: Vec<DataSourceConfig>,
    /// Cadence for the scheduled EOD refresh, in seconds. Default 86400
    /// (24 h).
    pub eod_interval_seconds: u64,
    /// Reserved for the parallel-batch follow-on. Sequential in v1.
    pub refresh_batch_size: u64,
    /// When `true` (default), reconciliation is opt-in; the scheduled
    /// task does NOT trigger it. Set `false` to enable reconciliation
    /// in dev.
    pub reconcile_deferred: bool,
    /// When `true`, the server spawns the scheduled task on `serve()`.
    /// Default `false` so tests / CI can boot without scheduled work.
    pub run_scheduled_tasks: bool,
}

impl Default for MarketDataConfig {
    fn default() -> Self {
        Self {
            sources: vec![DataSourceConfig::default_yahoo()],
            eod_interval_seconds: 86_400,
            refresh_batch_size: 50,
            reconcile_deferred: true,
            run_scheduled_tasks: false,
        }
    }
}

impl MarketDataConfig {
    /// Validates the configuration. Returns `Err(MarketDataProviderError::Validation)`
    /// with a clear message on the first violation.
    ///
    /// Implements FR-002 / FR-003 / FR-040 of spec 017.
    pub fn validate(&self) -> Result<(), MarketDataProviderError> {
        if self.sources.is_empty() {
            return Err(MarketDataProviderError::Validation(
                "MARKET_DATA_SOURCES must have at least one entry".into(),
            ));
        }
        let mut seen_ids = std::collections::BTreeSet::new();
        for (idx, source) in self.sources.iter().enumerate() {
            if source.priority <= 0 {
                return Err(MarketDataProviderError::Validation(format!(
                    "MARKET_DATA_SOURCES[{idx}].priority must be > 0 (got {})",
                    source.priority
                )));
            }
            if source.source_id.is_empty() {
                return Err(MarketDataProviderError::Validation(format!(
                    "MARKET_DATA_SOURCES[{idx}].source_id must be non-empty"
                )));
            }
            if !seen_ids.insert(source.source_id.clone()) {
                return Err(MarketDataProviderError::Validation(format!(
                    "duplicate source_id '{}' (also at indices {:?})",
                    source.source_id,
                    self.sources
                        .iter()
                        .enumerate()
                        .filter_map(|(i, s)| {
                            if s.source_id == source.source_id && i != idx {
                                Some(i)
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                )));
            }
            match source.provider_type.as_str() {
                "yahoo_finance" => {}
                other => {
                    return Err(MarketDataProviderError::Validation(format!(
                        "MARKET_DATA_SOURCES[{idx}].provider_type '{other}' is unknown"
                    )));
                }
            }
            if !(1..=1000).contains(&self.refresh_batch_size) {
                return Err(MarketDataProviderError::Validation(format!(
                    "MARKET_DATA_REFRESH_BATCH_SIZE must be in 1..=1000 (got {})",
                    self.refresh_batch_size
                )));
            }
            if self.eod_interval_seconds == 0 {
                return Err(MarketDataProviderError::Validation(
                    "MARKET_DATA_EOD_INTERVAL_SECONDS must be > 0".into(),
                ));
            }
        }
        Ok(())
    }
}

/// Per-source configuration.
///
/// Implements FR-002 / FR-040 of spec 017. The discriminator `provider_type`
/// gates which concrete builder is used (see `obolargus_server`'s
/// `market_data_provider::build_provider_from_config`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataSourceConfig {
    /// Stable id (e.g. `"yahoo_finance"`). MUST be unique across the
    /// `MarketDataConfig::sources` list (FR-003).
    pub source_id: String,
    /// Human-readable label.
    #[serde(default)]
    pub display_name: String,
    /// Provider discriminator; `"yahoo_finance"` in v1.
    pub provider_type: String,
    /// `false` ⇒ registry skips this source for every call (FR-004).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Lower = tried first; ties broken by `source_id` ASC.
    #[serde(default = "default_priority")]
    pub priority: i32,
    /// Open vocabulary of identifier schemes this source can answer.
    /// Use `"TICKER:*"` for venue-qualified wildcards, `"TICKER"` for
    /// unqualified.
    #[serde(default)]
    pub supported_schemes: Vec<String>,
    /// Data domains this source can serve (FR-027 of spec 022).
    #[serde(default = "default_data_domains")]
    pub supported_data_domains: Vec<String>,
    /// Optional per-source rate-limit (requests per minute). `None` ⇒
    /// upstream default (10 req/s with the `governor` feature).
    #[serde(default)]
    pub rate_limit_per_minute: Option<u32>,
    /// Name of an env-var that holds the upstream's API key. The
    /// provider reads the env var; the value is never persisted.
    #[serde(default)]
    pub api_key_env_var: Option<String>,
    /// Per-source `User-Agent` override (rarely needed).
    #[serde(default)]
    pub user_agent: Option<String>,
    /// Per-source upstream timeout, in seconds.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    /// Retry configuration for transient errors.
    #[serde(default)]
    pub retry: RetryConfig,
}

/// Serde default for `enabled` (true).
fn default_true() -> bool {
    true
}

/// Serde default for `priority` (10).
fn default_priority() -> i32 {
    10
}

/// Serde default for `supported_data_domains` (market data only).
fn default_data_domains() -> Vec<String> {
    vec!["market_data".to_string()]
}

/// Per-source retry configuration.
///
/// Implements FR-041 of spec 017. Defaults to base 100 ms, multiplier 2.0,
/// max 10 s, max 3 retries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Number of retries before the typed `Transient` is surfaced.
    pub max_retries: u8,
    /// Backoff base in milliseconds.
    pub backoff_base_ms: u64,
    /// Backoff maximum in milliseconds.
    pub backoff_max_ms: u64,
    /// Multiplier (typically 2.0).
    pub backoff_multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            backoff_base_ms: 100,
            backoff_max_ms: 10_000,
            backoff_multiplier: 2.0,
        }
    }
}

impl DataSourceConfig {
    /// The default Yahoo Finance source shape — used by the
    /// `MarketDataConfig::default()` constructor and surfaced in the
    /// envconfig default for `MARKET_DATA_SOURCES` per the
    /// `contracts/config-contract-additions.md` "variable table" row.
    ///
    /// Implements FR-002 of spec 017.
    pub fn default_yahoo() -> Self {
        Self {
            source_id: "yahoo_finance".to_string(),
            display_name: "Yahoo Finance".to_string(),
            provider_type: "yahoo_finance".to_string(),
            enabled: true,
            priority: 10,
            supported_schemes: vec![
                "TICKER".into(),
                "TICKER:*".into(),
                "ISIN".into(),
                "CUSIP".into(),
            ],
            supported_data_domains: vec!["market_data".into()],
            rate_limit_per_minute: Some(60),
            api_key_env_var: None,
            user_agent: Some("obolargus/0.1.0 (+https://obolargus.example)".into()),
            timeout_seconds: Some(30),
            retry: RetryConfig::default(),
        }
    }

    /// Validates this source alone (used by the registry's `new()` for
    /// per-source checks).
    ///
    /// Implements FR-002 / FR-003 of spec 017.
    pub fn validate(&self) -> Result<(), MarketDataProviderError> {
        if self.source_id.is_empty() {
            return Err(MarketDataProviderError::Validation(
                "source_id must be non-empty".into(),
            ));
        }
        if self.priority <= 0 {
            return Err(MarketDataProviderError::Validation(format!(
                "priority must be > 0 (got {})",
                self.priority
            )));
        }
        if let Some(t) = self.timeout_seconds
            && !(1..=600).contains(&t)
        {
            return Err(MarketDataProviderError::Validation(format!(
                "timeout_seconds must be in 1..=600 (got {t})"
            )));
        }
        if self.supported_data_domains.is_empty() {
            return Err(MarketDataProviderError::Validation(
                "supported_data_domains must not be empty".into(),
            ));
        }
        for domain in &self.supported_data_domains {
            if domain
                .parse::<crate::market_data::data_domain::DataDomain>()
                .is_err()
            {
                return Err(MarketDataProviderError::Validation(format!(
                    "unknown supported_data_domain '{domain}'"
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_market_data_config_passes_validation() {
        let config = MarketDataConfig::default();
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn empty_sources_rejected() {
        let config = MarketDataConfig {
            sources: vec![],
            ..MarketDataConfig::default()
        };
        let result = config.validate();
        assert!(matches!(
            result,
            Err(MarketDataProviderError::Validation(ref msg)) if msg.contains("at least one entry")
        ));
    }

    #[test]
    fn priority_zero_rejected() {
        let mut config = MarketDataConfig::default();
        config.sources[0].priority = 0;
        let result = config.validate();
        assert!(matches!(
            result,
            Err(MarketDataProviderError::Validation(ref msg)) if msg.contains("priority")
        ));
    }

    #[test]
    fn duplicate_source_id_rejected() {
        let mut config = MarketDataConfig::default();
        let mut second = config.sources[0].clone();
        second.priority = config.sources[0].priority + 1;
        config.sources.push(second);
        let result = config.validate();
        assert!(matches!(
            result,
            Err(MarketDataProviderError::Validation(ref msg)) if msg.contains("duplicate")
        ));
    }

    #[test]
    fn unknown_provider_type_rejected() {
        let mut config = MarketDataConfig::default();
        config.sources[0].provider_type = "alpha_vantage".to_string();
        let result = config.validate();
        assert!(matches!(
            result,
            Err(MarketDataProviderError::Validation(ref msg)) if msg.contains("unknown")
        ));
    }

    #[test]
    fn refresh_batch_size_zero_rejected() {
        let config = MarketDataConfig {
            refresh_batch_size: 0,
            ..MarketDataConfig::default()
        };
        let result = config.validate();
        assert!(matches!(
            result,
            Err(MarketDataProviderError::Validation(ref msg)) if msg.contains("REFRESH_BATCH_SIZE")
        ));
    }

    #[test]
    fn refresh_batch_size_too_large_rejected() {
        let config = MarketDataConfig {
            refresh_batch_size: 1001,
            ..MarketDataConfig::default()
        };
        let result = config.validate();
        assert!(matches!(
            result,
            Err(MarketDataProviderError::Validation(ref msg)) if msg.contains("REFRESH_BATCH_SIZE")
        ));
    }

    #[test]
    fn eod_interval_zero_rejected() {
        let config = MarketDataConfig {
            eod_interval_seconds: 0,
            ..MarketDataConfig::default()
        };
        let result = config.validate();
        assert!(matches!(
            result,
            Err(MarketDataProviderError::Validation(ref msg)) if msg.contains("EOD_INTERVAL")
        ));
    }

    #[test]
    fn default_yahoo_passes_per_source_validation() {
        assert_eq!(DataSourceConfig::default_yahoo().validate(), Ok(()));
    }

    // ---------- per-source validation inside MarketDataConfig ----------

    /// A source with an empty `source_id` is rejected by the aggregate
    /// validation, naming the offending index (FR-003).
    #[test]
    fn empty_source_id_rejected_by_market_data_config() {
        let mut config = MarketDataConfig::default();
        config.sources[0].source_id = String::new();
        let result = config.validate();
        assert!(
            matches!(
                result,
                Err(MarketDataProviderError::Validation(ref msg))
                    if msg.contains("source_id must be non-empty")
            ),
            "got: {result:?}"
        );
    }

    // ---------- DataSourceConfig::validate ----------

    /// `DataSourceConfig::validate` rejects an empty `source_id` (FR-003).
    #[test]
    fn per_source_empty_source_id_rejected() {
        let mut config = DataSourceConfig::default_yahoo();
        config.source_id = String::new();
        let result = config.validate();
        assert!(
            matches!(
                result,
                Err(MarketDataProviderError::Validation(ref msg))
                    if msg.contains("source_id must be non-empty")
            ),
            "got: {result:?}"
        );
    }

    /// `DataSourceConfig::validate` rejects a non-positive priority (FR-002).
    #[test]
    fn per_source_non_positive_priority_rejected() {
        let mut config = DataSourceConfig::default_yahoo();
        config.priority = 0;
        let result = config.validate();
        assert!(
            matches!(
                result,
                Err(MarketDataProviderError::Validation(ref msg))
                    if msg.contains("priority must be > 0")
            ),
            "got: {result:?}"
        );
    }

    /// `DataSourceConfig::validate` rejects a timeout outside `1..=600`.
    #[test]
    fn per_source_timeout_out_of_range_rejected() {
        let mut config = DataSourceConfig::default_yahoo();
        config.timeout_seconds = Some(0);
        let result = config.validate();
        assert!(
            matches!(
                result,
                Err(MarketDataProviderError::Validation(ref msg))
                    if msg.contains("timeout_seconds must be in 1..=600")
            ),
            "got: {result:?}"
        );
    }

    /// A timeout of exactly 600 is accepted (the bound is inclusive).
    #[test]
    fn per_source_timeout_at_the_upper_bound_is_accepted() {
        let mut config = DataSourceConfig::default_yahoo();
        config.timeout_seconds = Some(600);
        assert_eq!(config.validate(), Ok(()));
    }

    // ---------- serde defaults ----------

    /// A source deserialized without `enabled` / `priority` gets the
    /// documented defaults (`true` / `10`).
    #[test]
    fn serde_defaults_fill_enabled_and_priority() {
        let config: DataSourceConfig =
            serde_json::from_str(r#"{"source_id":"s","provider_type":"yahoo_finance"}"#)
                .expect("deserialize");
        assert!(config.enabled);
        assert_eq!(config.priority, 10);
        assert!(config.supported_schemes.is_empty());
        assert_eq!(config.retry, RetryConfig::default());
    }

    /// An explicit `enabled: false` / `priority` in the JSON wins over the
    /// serde defaults.
    #[test]
    fn serde_defaults_do_not_override_explicit_values() {
        let config: DataSourceConfig = serde_json::from_str(
            r#"{"source_id":"s","provider_type":"yahoo_finance","enabled":false,"priority":3}"#,
        )
        .expect("deserialize");
        assert!(!config.enabled);
        assert_eq!(config.priority, 3);
    }
}
