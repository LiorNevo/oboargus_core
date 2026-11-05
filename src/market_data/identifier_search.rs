//! Read-only external identifier search for the Data Load page.
//!
//! Implements the discovery half of `specs/026-identifier-search/spec.md`:
//! ask every configured, enabled source that can serve the requested domain
//! for free-text ticker/name matches, normalize the hits through
//! [`crate::market_data::identifier_mapping`], consolidate them, label known
//! assets through read-only batched queries, filter, and paginate.
//!
//! The module never writes. Asset creation remains the load's existing
//! `create_missing_assets` path.

#![forbid(unsafe_code)]

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::asset_identifier::read::{self as asset_identifier_read, KnownIdentifier};
use crate::domain::error::DomainError;
use crate::domain::identifier::IdentifierScheme;
use crate::market_data::data_domain::DataDomain;
use crate::market_data::error::MarketDataProviderError;
use crate::market_data::identifier_mapping::{
    canonical_asset_type, canonical_identifier, derive_display_name, exchange_code_mic,
    is_plausible_symbol, ticker_venue,
};
use crate::market_data::registry::{RegistrySnapshot, SourceLookupOutcome};
use crate::market_data::types::{TickerLookupHit, TypedIdentifier};

/// Minimum number of non-whitespace characters in a query.
///
/// Implements FR-002 of `specs/026-identifier-search/spec.md`.
pub const MIN_QUERY_NON_WHITESPACE: usize = 2;
/// Maximum number of characters in a query.
///
/// Implements FR-002 of `specs/026-identifier-search/spec.md`.
pub const MAX_QUERY_LENGTH: usize = 120;
/// Default page size.
///
/// Implements FR-033 of `specs/026-identifier-search/spec.md`.
pub const DEFAULT_LIMIT: u32 = 25;
/// Largest accepted page size.
///
/// Implements FR-033 of `specs/026-identifier-search/spec.md`.
pub const MAX_LIMIT: u32 = 100;

/// A validated identifier-search request.
///
/// Implements FR-002 / FR-012 / FR-013 of
/// `specs/026-identifier-search/spec.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentifierSearchRequest {
    /// Trimmed free-text query.
    pub query: String,
    /// Domain to search.
    pub domain: DataDomain,
    /// Optional single-source filter.
    pub source: Option<String>,
    /// Optional canonical asset-type filter.
    pub asset_type: Option<String>,
    /// Optional MIC/raw-exchange filter.
    pub exchange: Option<String>,
    /// Page size.
    pub limit: u32,
    /// Zero-based page offset.
    pub offset: u32,
}

impl IdentifierSearchRequest {
    /// Validates raw HTTP query values.
    ///
    /// `domain` defaults to `market_data`; `holdings` is rejected because it
    /// is not searchable in this release. `limit` must be `1..=100` and
    /// `offset` must be non-negative.
    ///
    /// Implements FR-002 / FR-033 of `specs/026-identifier-search/spec.md`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        query: &str,
        domain: Option<&str>,
        source: Option<String>,
        asset_type: Option<String>,
        exchange: Option<String>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<Self, IdentifierSearchError> {
        let query = query.trim().to_string();
        let non_whitespace = query.chars().filter(|c| !c.is_whitespace()).count();
        if non_whitespace < MIN_QUERY_NON_WHITESPACE {
            return Err(IdentifierSearchError::Validation(format!(
                "query must be at least {MIN_QUERY_NON_WHITESPACE} characters"
            )));
        }
        if query.chars().count() > MAX_QUERY_LENGTH {
            return Err(IdentifierSearchError::Validation(format!(
                "query must be at most {MAX_QUERY_LENGTH} characters"
            )));
        }

        let domain = match domain {
            None => DataDomain::MarketData,
            Some(raw) => DataDomain::from_str(raw).map_err(IdentifierSearchError::Validation)?,
        };
        if domain != DataDomain::MarketData {
            return Err(IdentifierSearchError::Validation(
                "holdings data is not searchable in this release".to_string(),
            ));
        }

        let limit = match limit.unwrap_or(i64::from(DEFAULT_LIMIT)) {
            value if (1..=i64::from(MAX_LIMIT)).contains(&value) => value as u32,
            _ => {
                return Err(IdentifierSearchError::Validation(format!(
                    "limit must be between 1 and {MAX_LIMIT}"
                )));
            }
        };
        let offset = match offset.unwrap_or(0) {
            value if value >= 0 => value as u32,
            _ => {
                return Err(IdentifierSearchError::Validation(
                    "offset must be zero or greater".to_string(),
                ));
            }
        };

        Ok(Self {
            query,
            domain,
            source: clean_filter(source),
            asset_type: clean_filter(asset_type),
            exchange: clean_filter(exchange),
            limit,
            offset,
        })
    }
}

fn clean_filter(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// A validation or storage failure from the search operation.
///
/// Implements FR-033 of `specs/026-identifier-search/spec.md` and the error
/// envelope of `specs/026-identifier-search/contracts/identifier-search-api.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentifierSearchError {
    /// The request violates the documented rules.
    Validation(String),
    /// Known-state could not be read.
    Storage(String),
}

impl fmt::Display for IdentifierSearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(message) | Self::Storage(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for IdentifierSearchError {}

impl From<DomainError> for IdentifierSearchError {
    fn from(error: DomainError) -> Self {
        Self::Storage(error.to_string())
    }
}

/// Overall status of one search.
///
/// Implements FR-026 / FR-033 of `specs/026-identifier-search/spec.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum SearchOutcome {
    /// Every consulted source answered.
    Complete,
    /// Some sources answered and some did not.
    Partial,
    /// Every consulted source failed.
    Failed,
    /// No enabled source can serve the domain.
    NoSources,
}

/// One source's typed status.
///
/// Implements FR-026 / FR-028 of `specs/026-identifier-search/spec.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum SourceStatus {
    /// The source returned one or more hits.
    Answered,
    /// The source answered authoritatively with zero hits.
    NoMatch,
    /// The source's budget was exhausted.
    RateLimited,
    /// A transient or infrastructure failure.
    Unavailable,
    /// The source refused the request.
    Rejected,
}

impl SourceStatus {
    /// Whether the source produced a usable answer (including zero hits).
    ///
    /// Implements FR-026 of `specs/026-identifier-search/spec.md`.
    pub fn answered(self) -> bool {
        matches!(self, Self::Answered | Self::NoMatch)
    }
}

/// One source's outcome for a search.
///
/// Implements FR-026 / FR-028 of `specs/026-identifier-search/spec.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SearchSourceOutcome {
    /// Configured source id.
    pub source_id: String,
    /// Operator-facing label.
    pub display_name: String,
    /// Typed status.
    pub status: SourceStatus,
    /// Hits returned (0 for non-answered sources).
    pub hit_count: usize,
    /// Stable machine code for a non-answered source.
    pub error_code: Option<String>,
    /// Human-readable explanation.
    pub message: Option<String>,
    /// Cooldown supplied by a rate-limited source.
    pub retry_after_seconds: Option<u64>,
}

/// A source that answered a candidate.
///
/// Implements FR-007 / FR-008 of `specs/026-identifier-search/spec.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CandidateSource {
    /// Stable source id.
    pub source_id: String,
    /// Operator-facing label.
    pub display_name: String,
    /// That source's relevance score.
    pub relevance: Option<f64>,
}

/// One identifier the operator could add.
///
/// Implements FR-007 / FR-011 / FR-014 of
/// `specs/026-identifier-search/spec.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CandidateIdentifier {
    /// Normalized scheme (e.g. `TICKER:XNAS`).
    pub scheme: String,
    /// Normalized value.
    pub value: String,
    /// Operator-facing label.
    pub label: String,
    /// Exactly one primary per candidate.
    pub primary: bool,
    /// Whether the pair may enter the batch.
    pub addable: bool,
    /// Whether the load can create a missing asset from it.
    pub creatable: bool,
    /// Why the pair is not addable/creatable.
    pub reason: Option<String>,
}

/// One value a source reported for a conflicting field.
///
/// Implements FR-009 of `specs/026-identifier-search/spec.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ConflictingValue {
    /// The reported value.
    pub value: String,
    /// Sources that reported it.
    pub sources: Vec<String>,
}

/// A disagreement between sources about one field.
///
/// Implements FR-009 of `specs/026-identifier-search/spec.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FieldConflict {
    /// `display_name`, `asset_type`, `exchange`, or `currency`.
    pub field: String,
    /// Every distinct value and its sources.
    pub values: Vec<ConflictingValue>,
}

/// How the system already knows a candidate.
///
/// Implements FR-019 of `specs/026-identifier-search/spec.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum KnownMatchKind {
    /// The exact normalized `(scheme, value)` pair is stored.
    Exact,
    /// A ticker with the same value is stored under any ticker scheme.
    EquivalentTicker,
}

/// A reference to the existing asset that claims a candidate identifier.
///
/// Implements FR-019 of `specs/026-identifier-search/spec.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct KnownAssetRef {
    /// Existing asset id.
    pub asset_id: Uuid,
    /// Existing canonical asset type.
    pub asset_type: String,
    /// Existing display name.
    pub display_name: String,
    /// Existing currency.
    pub currency: String,
    /// The candidate identifier that matched.
    pub matched_scheme: String,
    /// The candidate value that matched.
    pub matched_value: String,
    /// How it matched.
    pub match_kind: KnownMatchKind,
}

/// One consolidated, deduplicated search result.
///
/// Implements FR-007..FR-012 of `specs/026-identifier-search/spec.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SearchCandidate {
    /// Trimmed upstream symbol.
    pub symbol: String,
    /// Operator-facing name.
    pub display_name: String,
    /// Canonical asset type, when recognized.
    pub asset_type: Option<String>,
    /// Raw upstream exchange code.
    pub exchange: Option<String>,
    /// Mapped MIC, when known.
    pub venue_mic: Option<String>,
    /// Upstream currency, when provided.
    pub currency: Option<String>,
    /// Highest relevance across merged hits.
    pub relevance: Option<f64>,
    /// Every answering source.
    pub sources: Vec<CandidateSource>,
    /// Primary identifier first, then alternates.
    pub identifiers: Vec<CandidateIdentifier>,
    /// Source disagreements.
    pub conflicts: Vec<FieldConflict>,
    /// Existing asset, when known.
    pub known: Option<KnownAssetRef>,
    /// Whether any identifier may enter the batch.
    pub addable: bool,
    /// Why no identifier may enter the batch.
    pub not_addable_reason: Option<String>,
    /// Whether the load can create a missing asset from the primary.
    pub creatable: bool,
    /// Why creation is blocked.
    pub creation_blocked_reason: Option<String>,
}

/// The complete response of one search.
///
/// Implements FR-033 of `specs/026-identifier-search/spec.md` and the response
/// contract of `specs/026-identifier-search/contracts/identifier-search-api.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct IdentifierSearchResult {
    /// Trimmed executed query.
    pub query: String,
    /// Executed domain (`market_data`).
    pub domain: String,
    /// Overall outcome.
    pub outcome: SearchOutcome,
    /// Sources actually contacted.
    pub consulted_sources: Vec<String>,
    /// One outcome per consulted source.
    pub sources: Vec<SearchSourceOutcome>,
    /// Consolidated, filtered count before pagination.
    pub total: usize,
    /// Echoed offset.
    pub offset: u32,
    /// Effective limit.
    pub limit: u32,
    /// The requested page.
    pub results: Vec<SearchCandidate>,
    /// Search-level explanation for `failed`/`no_sources`.
    pub message: Option<String>,
}

/// A typed provider failure mapped onto the source-status vocabulary.
///
/// Implements FR-028 / FR-031 of `specs/026-identifier-search/spec.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappedProviderError {
    /// Source status.
    pub status: SourceStatus,
    /// Stable machine code.
    pub error_code: String,
    /// Human-readable explanation.
    pub message: String,
    /// Cooldown, when the source supplied one.
    pub retry_after_seconds: Option<u64>,
}

/// Maps a provider error onto the search source-status vocabulary.
///
/// Unknown future variants map to `unavailable` with a generic message, so a
/// newer provider cannot crash the page.
///
/// Implements FR-028 / FR-031 of `specs/026-identifier-search/spec.md`.
pub fn map_provider_error_to_source_status(error: &MarketDataProviderError) -> MappedProviderError {
    // `MarketDataProviderError` is `#[non_exhaustive]`; the wildcard keeps a
    // newer provider variant from breaking search. Inside this crate it is
    // currently unreachable, so silence the compiler's local knowledge.
    #[allow(unreachable_patterns)]
    match error {
        MarketDataProviderError::NotFound(message) => MappedProviderError {
            status: SourceStatus::NoMatch,
            error_code: "no_match".to_string(),
            message: message.clone(),
            retry_after_seconds: None,
        },
        MarketDataProviderError::RateLimited { msg, retry_after } => MappedProviderError {
            status: SourceStatus::RateLimited,
            error_code: "rate_limited".to_string(),
            message: msg.clone(),
            retry_after_seconds: retry_after.map(duration_seconds),
        },
        MarketDataProviderError::InvalidInput(message)
        | MarketDataProviderError::Unsupported(message)
        | MarketDataProviderError::Unauthorized(message)
        | MarketDataProviderError::Validation(message) => MappedProviderError {
            status: SourceStatus::Rejected,
            error_code: "rejected".to_string(),
            message: message.clone(),
            retry_after_seconds: None,
        },
        MarketDataProviderError::Transient(message)
        | MarketDataProviderError::Infrastructure(message) => MappedProviderError {
            status: SourceStatus::Unavailable,
            error_code: "unavailable".to_string(),
            message: message.clone(),
            retry_after_seconds: None,
        },
        _ => MappedProviderError {
            status: SourceStatus::Unavailable,
            error_code: "unavailable".to_string(),
            message: "The source could not be searched.".to_string(),
            retry_after_seconds: None,
        },
    }
}

fn duration_seconds(duration: Duration) -> u64 {
    duration.as_secs()
}

/// Searches configured sources and labels known assets.
///
/// Performs only `SELECT`s. `pool` is required because known state cannot be
/// determined honestly without it.
///
/// Implements FR-001..FR-033 of `specs/026-identifier-search/spec.md`.
pub async fn search_identifiers(
    snapshot: &RegistrySnapshot,
    pool: &PgPool,
    request: &IdentifierSearchRequest,
) -> Result<IdentifierSearchResult, IdentifierSearchError> {
    let mut result = search_without_known_state(snapshot, request).await;
    apply_known_state(pool, &mut result.results).await?;
    Ok(result)
}

/// Performs the source search, consolidation, filtering, ordering, and
/// pagination without consulting known state.
///
/// Kept separate so unit tests can exercise the whole pipeline without a
/// database.
///
/// Implements FR-026 of `specs/026-identifier-search/spec.md`.
pub async fn search_without_known_state(
    snapshot: &RegistrySnapshot,
    request: &IdentifierSearchRequest,
) -> IdentifierSearchResult {
    let outcomes = snapshot
        .search_ticker_lookup(&request.query, &request.domain)
        .await;
    let (sources, hits) = collect_outcomes(outcomes);
    let outcome = derive_search_outcome(&sources);
    let consulted_sources = sources
        .iter()
        .map(|source| source.source_id.clone())
        .collect::<Vec<_>>();

    if outcome == SearchOutcome::NoSources {
        return IdentifierSearchResult {
            query: request.query.clone(),
            domain: request.domain.as_str().to_string(),
            outcome,
            consulted_sources,
            sources,
            total: 0,
            offset: request.offset,
            limit: request.limit,
            results: Vec::new(),
            message: Some(
                "No enabled source can search market data. Enable a source in the source catalog."
                    .to_string(),
            ),
        };
    }

    let mut candidates = build_candidates(hits);
    for candidate in &mut candidates {
        finalize_candidate(candidate);
    }
    candidates.retain(|candidate| matches_filters(candidate, request));
    sort_candidates(&mut candidates);
    let total = candidates.len();
    let results = candidates
        .into_iter()
        .skip(request.offset as usize)
        .take(request.limit as usize)
        .collect();

    let message = if outcome == SearchOutcome::Failed {
        Some("Every consulted source failed. Try again or check the source catalog.".to_string())
    } else {
        None
    };

    IdentifierSearchResult {
        query: request.query.clone(),
        domain: request.domain.as_str().to_string(),
        outcome,
        consulted_sources,
        sources,
        total,
        offset: request.offset,
        limit: request.limit,
        results,
        message,
    }
}

fn collect_outcomes(
    outcomes: Vec<SourceLookupOutcome>,
) -> (Vec<SearchSourceOutcome>, Vec<SourceHit>) {
    let mut sources = Vec::with_capacity(outcomes.len());
    let mut hits = Vec::new();
    for outcome in outcomes {
        match outcome.result {
            Ok(result) => {
                let hit_count = result.hits.len();
                sources.push(SearchSourceOutcome {
                    source_id: outcome.source_id.clone(),
                    display_name: outcome.display_name.clone(),
                    status: if hit_count == 0 {
                        SourceStatus::NoMatch
                    } else {
                        SourceStatus::Answered
                    },
                    hit_count,
                    error_code: None,
                    message: None,
                    retry_after_seconds: None,
                });
                for hit in result.hits {
                    hits.push(SourceHit {
                        source_id: outcome.source_id.clone(),
                        display_name: outcome.display_name.clone(),
                        priority: outcome.priority,
                        hit,
                    });
                }
            }
            Err(error) => {
                let mapped = map_provider_error_to_source_status(&error);
                sources.push(SearchSourceOutcome {
                    source_id: outcome.source_id,
                    display_name: outcome.display_name,
                    status: mapped.status,
                    hit_count: 0,
                    error_code: Some(mapped.error_code),
                    message: Some(mapped.message),
                    retry_after_seconds: mapped.retry_after_seconds,
                });
            }
        }
    }
    (sources, hits)
}

fn derive_search_outcome(sources: &[SearchSourceOutcome]) -> SearchOutcome {
    if sources.is_empty() {
        return SearchOutcome::NoSources;
    }
    let answered = sources
        .iter()
        .filter(|source| source.status.answered())
        .count();
    if answered == sources.len() {
        SearchOutcome::Complete
    } else if answered == 0 {
        SearchOutcome::Failed
    } else {
        SearchOutcome::Partial
    }
}

struct SourceHit {
    source_id: String,
    display_name: String,
    priority: i32,
    hit: TickerLookupHit,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum CandidateKey {
    VenueQualified {
        mic: String,
        symbol: String,
    },
    Bare {
        symbol: String,
        raw_exchange: String,
    },
}

#[derive(Debug, Clone)]
struct HitRank {
    relevance: Option<f64>,
    priority: i32,
    source_id: String,
}

impl HitRank {
    fn of(source_hit: &SourceHit) -> Self {
        Self {
            relevance: source_hit.hit.relevance,
            priority: source_hit.priority,
            source_id: source_hit.source_id.clone(),
        }
    }

    fn is_better_than(&self, other: &Self) -> bool {
        let mine = self.relevance.unwrap_or(f64::NEG_INFINITY);
        let theirs = other.relevance.unwrap_or(f64::NEG_INFINITY);
        match mine.partial_cmp(&theirs).unwrap_or(Ordering::Equal) {
            Ordering::Greater => true,
            Ordering::Less => false,
            Ordering::Equal => {
                self.priority
                    .cmp(&other.priority)
                    .then_with(|| self.source_id.cmp(&other.source_id))
                    == Ordering::Less
            }
        }
    }
}

struct HeadlineValue {
    value: String,
    rank: HitRank,
}

struct CandidateAccumulator {
    symbol: String,
    display_name: Option<HeadlineValue>,
    asset_type: Option<HeadlineValue>,
    exchange: Option<HeadlineValue>,
    venue_mic: Option<String>,
    currency: Option<HeadlineValue>,
    relevance: Option<f64>,
    sources: BTreeMap<String, CandidateSource>,
    identifiers: Vec<CandidateIdentifier>,
    seen_identifiers: BTreeSet<(String, String)>,
    display_names: BTreeMap<String, BTreeSet<String>>,
    asset_types: BTreeMap<String, BTreeSet<String>>,
    exchanges: BTreeMap<String, BTreeSet<String>>,
    currencies: BTreeMap<String, BTreeSet<String>>,
}

impl CandidateAccumulator {
    fn new(source_hit: &SourceHit) -> Self {
        let symbol = source_hit.hit.symbol.trim().to_string();
        let venue_mic = exchange_code_mic(source_hit.hit.exchange.as_deref()).map(str::to_string);
        Self {
            symbol,
            display_name: None,
            asset_type: None,
            exchange: None,
            venue_mic,
            currency: None,
            relevance: None,
            sources: BTreeMap::new(),
            identifiers: Vec::new(),
            seen_identifiers: BTreeSet::new(),
            display_names: BTreeMap::new(),
            asset_types: BTreeMap::new(),
            exchanges: BTreeMap::new(),
            currencies: BTreeMap::new(),
        }
    }

    fn merge(&mut self, source_hit: &SourceHit) {
        let hit = &source_hit.hit;
        let rank = HitRank::of(source_hit);
        if self
            .relevance
            .is_none_or(|current| hit.relevance.is_some_and(|next| next > current))
        {
            self.relevance = hit.relevance.or(self.relevance);
        }

        let source = self
            .sources
            .entry(source_hit.source_id.clone())
            .or_insert_with(|| CandidateSource {
                source_id: source_hit.source_id.clone(),
                display_name: source_hit.display_name.clone(),
                relevance: hit.relevance,
            });
        if hit
            .relevance
            .is_some_and(|next| source.relevance.is_none_or(|current| next > current))
        {
            source.relevance = hit.relevance;
        }

        let display_name = derive_display_name(hit);
        record_field(
            &mut self.display_name,
            &mut self.display_names,
            display_name,
            &rank,
            &source_hit.source_id,
        );
        if let Some(asset_type) = canonical_asset_type(hit.quote_type.as_deref()) {
            record_field(
                &mut self.asset_type,
                &mut self.asset_types,
                asset_type.to_string(),
                &rank,
                &source_hit.source_id,
            );
        }
        if let Some(exchange) = hit
            .exchange
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let exchange = exchange.to_ascii_uppercase();
            // Compare venues by MIC when one is known, so `NMS` and `NAS`
            // from two sources are not reported as a conflict.
            let conflict_value = self.venue_mic.clone().unwrap_or_else(|| exchange.clone());
            self.exchanges
                .entry(conflict_value)
                .or_default()
                .insert(source_hit.source_id.clone());
            let should_replace = self
                .exchange
                .as_ref()
                .is_none_or(|current| rank.is_better_than(&current.rank));
            if should_replace {
                self.exchange = Some(HeadlineValue {
                    value: exchange,
                    rank: rank.clone(),
                });
            }
        }
        if let Some(currency) = hit
            .currency
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            record_field(
                &mut self.currency,
                &mut self.currencies,
                currency.to_ascii_uppercase(),
                &rank,
                &source_hit.source_id,
            );
        }

        for identifier in build_identifiers(hit, &self.symbol, self.venue_mic.as_deref()) {
            let pair = (identifier.scheme.clone(), identifier.value.clone());
            if self.seen_identifiers.insert(pair) {
                self.identifiers.push(identifier);
            }
        }
    }

    fn finish(self) -> SearchCandidate {
        let conflicts = vec![
            conflict("display_name", &self.display_names),
            conflict("asset_type", &self.asset_types),
            conflict("exchange", &self.exchanges),
            conflict("currency", &self.currencies),
        ]
        .into_iter()
        .flatten()
        .collect();
        let display_name = self
            .display_name
            .map(|value| value.value)
            .unwrap_or_else(|| self.symbol.clone());
        let mut sources = self.sources.into_values().collect::<Vec<_>>();
        sources.sort_by(|a, b| a.source_id.cmp(&b.source_id));
        SearchCandidate {
            symbol: self.symbol,
            display_name,
            asset_type: self.asset_type.map(|value| value.value),
            exchange: self.exchange.map(|value| value.value),
            venue_mic: self.venue_mic,
            currency: self.currency.map(|value| value.value),
            relevance: self.relevance,
            sources,
            identifiers: self.identifiers,
            conflicts,
            known: None,
            addable: false,
            not_addable_reason: None,
            creatable: false,
            creation_blocked_reason: None,
        }
    }
}

fn record_field(
    headline: &mut Option<HeadlineValue>,
    values: &mut BTreeMap<String, BTreeSet<String>>,
    value: String,
    rank: &HitRank,
    source_id: &str,
) {
    values
        .entry(value.clone())
        .or_default()
        .insert(source_id.to_string());
    let should_replace = headline
        .as_ref()
        .is_none_or(|current| rank.is_better_than(&current.rank));
    if should_replace {
        *headline = Some(HeadlineValue {
            value,
            rank: rank.clone(),
        });
    }
}

fn conflict(field: &str, values: &BTreeMap<String, BTreeSet<String>>) -> Option<FieldConflict> {
    if values.len() < 2 {
        return None;
    }
    Some(FieldConflict {
        field: field.to_string(),
        values: values
            .iter()
            .map(|(value, sources)| ConflictingValue {
                value: value.clone(),
                sources: sources.iter().cloned().collect(),
            })
            .collect(),
    })
}

fn build_candidates(hits: Vec<SourceHit>) -> Vec<SearchCandidate> {
    let mut accumulators: BTreeMap<CandidateKey, CandidateAccumulator> = BTreeMap::new();
    for source_hit in hits {
        let key = candidate_key(&source_hit.hit);
        let accumulator = accumulators
            .entry(key)
            .or_insert_with(|| CandidateAccumulator::new(&source_hit));
        accumulator.merge(&source_hit);
    }
    accumulators
        .into_values()
        .map(CandidateAccumulator::finish)
        .collect()
}

fn candidate_key(hit: &TickerLookupHit) -> CandidateKey {
    let symbol = hit.symbol.trim().to_ascii_uppercase();
    match exchange_code_mic(hit.exchange.as_deref()) {
        Some(mic) => CandidateKey::VenueQualified {
            mic: mic.to_string(),
            symbol,
        },
        None => CandidateKey::Bare {
            symbol,
            raw_exchange: hit
                .exchange
                .as_deref()
                .map(|value| value.trim().to_ascii_uppercase())
                .unwrap_or_default(),
        },
    }
}

fn build_identifiers(
    hit: &TickerLookupHit,
    symbol: &str,
    venue_mic: Option<&str>,
) -> Vec<CandidateIdentifier> {
    let mut identifiers = Vec::new();
    let primary_scheme = venue_mic
        .map(|mic| format!("TICKER:{mic}"))
        .unwrap_or_else(|| IdentifierScheme::TICKER.to_string());
    let primary_value = symbol.trim().to_ascii_uppercase();
    let plausible = is_plausible_symbol(symbol);
    identifiers.push(CandidateIdentifier {
        scheme: primary_scheme.clone(),
        value: primary_value.clone(),
        label: venue_label(venue_mic),
        primary: true,
        addable: plausible,
        creatable: plausible,
        reason: (!plausible)
            .then(|| "The symbol cannot be expressed as a load identifier.".to_string()),
    });

    let mut seen = BTreeSet::from([(primary_scheme, primary_value)]);
    for (scheme, value) in &hit.schemes {
        let canonical = canonical_identifier(&TypedIdentifier::new(scheme.clone(), value.clone()));
        if canonical.scheme.is_empty() || canonical.value.is_empty() {
            continue;
        }
        let pair = (canonical.scheme.clone(), canonical.value.clone());
        if !seen.insert(pair) {
            continue;
        }
        let ticker = ticker_venue(&canonical.scheme).is_some();
        let recognized = ticker || is_recognized_scheme(&canonical.scheme);
        let addable = recognized;
        identifiers.push(CandidateIdentifier {
            scheme: canonical.scheme.clone(),
            value: canonical.value,
            label: scheme_label(&canonical.scheme),
            primary: false,
            addable,
            creatable: addable && ticker,
            reason: if !recognized {
                Some("The load does not recognize this identifier scheme.".to_string())
            } else if !ticker {
                Some(
                    "The load can resolve this scheme but cannot create an asset from it."
                        .to_string(),
                )
            } else {
                None
            },
        });
    }
    identifiers
}

fn is_recognized_scheme(scheme: &str) -> bool {
    matches!(
        scheme,
        IdentifierScheme::ISIN
            | IdentifierScheme::CUSIP
            | IdentifierScheme::BBGID
            | IdentifierScheme::FIGI
            | IdentifierScheme::SEDOL
            | IdentifierScheme::LEI
            | IdentifierScheme::CIK
            | IdentifierScheme::RIC
            | IdentifierScheme::WKN
            | IdentifierScheme::VALOR
    )
}

fn venue_label(venue_mic: Option<&str>) -> String {
    match venue_mic {
        Some(mic) => format!("Ticker ({mic})"),
        None => "Ticker".to_string(),
    }
}

fn scheme_label(scheme: &str) -> String {
    match ticker_venue(scheme) {
        Some(Some(mic)) => format!("Ticker ({mic})"),
        Some(None) => "Ticker".to_string(),
        None => scheme.to_string(),
    }
}

fn finalize_candidate(candidate: &mut SearchCandidate) {
    let primary = candidate
        .identifiers
        .iter()
        .find(|identifier| identifier.primary);
    let any_addable = candidate
        .identifiers
        .iter()
        .any(|identifier| identifier.addable);
    let primary_ticker =
        primary.is_some_and(|identifier| ticker_venue(&identifier.scheme).is_some());
    let type_known = candidate.asset_type.is_some();
    let known = candidate.known.is_some();

    if !type_known && !known {
        candidate.addable = false;
        candidate.not_addable_reason =
            Some("The load cannot create an asset whose type is not recognized.".to_string());
    } else {
        candidate.addable = any_addable;
        candidate.not_addable_reason = if any_addable {
            None
        } else {
            primary
                .and_then(|identifier| identifier.reason.clone())
                .or_else(|| {
                    Some("The identifier cannot be expressed in the load grammar.".to_string())
                })
        };
    }

    candidate.creatable = candidate.addable && primary_ticker && type_known;
    candidate.creation_blocked_reason = if candidate.creatable {
        None
    } else if !type_known {
        Some("The load cannot create an asset whose type is not recognized.".to_string())
    } else if !primary_ticker {
        Some("The load can only create assets from ticker identifiers.".to_string())
    } else if !candidate.addable {
        Some("The identifier cannot enter the batch.".to_string())
    } else {
        None
    };
}

fn matches_filters(candidate: &SearchCandidate, request: &IdentifierSearchRequest) -> bool {
    if let Some(source) = request.source.as_deref()
        && !candidate
            .sources
            .iter()
            .any(|answered| answered.source_id.eq_ignore_ascii_case(source))
    {
        return false;
    }
    if let Some(asset_type) = request.asset_type.as_deref()
        && !candidate
            .asset_type
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case(asset_type))
    {
        return false;
    }
    if let Some(exchange) = request.exchange.as_deref() {
        let mic_matches = candidate
            .venue_mic
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case(exchange));
        let raw_matches = candidate
            .exchange
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case(exchange));
        if !mic_matches && !raw_matches {
            return false;
        }
    }
    true
}

fn sort_candidates(candidates: &mut [SearchCandidate]) {
    candidates.sort_by(|a, b| {
        let a_relevance = a.relevance.unwrap_or(f64::NEG_INFINITY);
        let b_relevance = b.relevance.unwrap_or(f64::NEG_INFINITY);
        b_relevance
            .partial_cmp(&a_relevance)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.symbol.cmp(&b.symbol))
            .then_with(|| venue_key(a).cmp(venue_key(b)))
            .then_with(|| primary_scheme(a).cmp(primary_scheme(b)))
            .then_with(|| primary_value(a).cmp(primary_value(b)))
    });
}

fn venue_key(candidate: &SearchCandidate) -> &str {
    candidate
        .venue_mic
        .as_deref()
        .or(candidate.exchange.as_deref())
        .unwrap_or("")
}

fn primary_scheme(candidate: &SearchCandidate) -> &str {
    candidate
        .identifiers
        .iter()
        .find(|identifier| identifier.primary)
        .map(|identifier| identifier.scheme.as_str())
        .unwrap_or("")
}

fn primary_value(candidate: &SearchCandidate) -> &str {
    candidate
        .identifiers
        .iter()
        .find(|identifier| identifier.primary)
        .map(|identifier| identifier.value.as_str())
        .unwrap_or("")
}

/// Labels every candidate against the system's existing assets.
///
/// Issues at most two batched read-only queries and never writes.
///
/// Implements FR-019 / FR-023 of `specs/026-identifier-search/spec.md`.
pub async fn apply_known_state(
    pool: &PgPool,
    candidates: &mut [SearchCandidate],
) -> Result<(), IdentifierSearchError> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut ticker_values: Vec<String> = Vec::new();
    for candidate in candidates.iter() {
        for identifier in &candidate.identifiers {
            pairs.push((identifier.scheme.clone(), identifier.value.clone()));
            if ticker_venue(&identifier.scheme).is_some() {
                ticker_values.push(identifier.value.clone());
            }
        }
    }
    pairs.sort();
    pairs.dedup();
    ticker_values.sort();
    ticker_values.dedup();

    let exact = asset_identifier_read::find_identifiers_by_pairs(pool, &pairs).await?;
    let equivalent =
        asset_identifier_read::find_assets_by_ticker_values(pool, &ticker_values).await?;

    let exact_map: BTreeMap<(String, String), KnownIdentifier> = exact
        .into_iter()
        .map(|known| ((known.scheme.clone(), known.value.clone()), known))
        .collect();
    let mut equivalent_map: BTreeMap<String, Vec<KnownIdentifier>> = BTreeMap::new();
    for known in equivalent {
        equivalent_map
            .entry(known.value.to_uppercase())
            .or_default()
            .push(known);
    }

    for candidate in candidates.iter_mut() {
        let exact_match = candidate.identifiers.iter().find_map(|identifier| {
            exact_map
                .get(&(identifier.scheme.clone(), identifier.value.clone()))
                .map(|known| (identifier, known))
        });
        if let Some((identifier, known)) = exact_match {
            candidate.known = Some(known_ref(
                known,
                &identifier.scheme,
                &identifier.value,
                KnownMatchKind::Exact,
            ));
            finalize_candidate(candidate);
            continue;
        }

        let mut equivalent_match = None;
        for identifier in candidate
            .identifiers
            .iter()
            .filter(|identifier| ticker_venue(&identifier.scheme).is_some())
        {
            let Some(rows) = equivalent_map.get(&identifier.value.to_uppercase()) else {
                continue;
            };
            let assets = rows
                .iter()
                .map(|known| known.asset_id)
                .collect::<BTreeSet<_>>();
            if assets.len() == 1
                && let Some(known) = rows.first()
            {
                equivalent_match = Some((identifier, known));
                break;
            }
        }
        if let Some((identifier, known)) = equivalent_match {
            candidate.known = Some(known_ref(
                known,
                &identifier.scheme,
                &identifier.value,
                KnownMatchKind::EquivalentTicker,
            ));
        }
        finalize_candidate(candidate);
    }
    Ok(())
}

fn known_ref(
    known: &KnownIdentifier,
    matched_scheme: &str,
    matched_value: &str,
    match_kind: KnownMatchKind,
) -> KnownAssetRef {
    KnownAssetRef {
        asset_id: known.asset_id,
        asset_type: known.asset_type.clone(),
        display_name: known.display_name.clone(),
        currency: known.currency.clone(),
        matched_scheme: matched_scheme.to_string(),
        matched_value: matched_value.to_string(),
        match_kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::market_data::registry::DataSourceRegistry;
    use crate::market_data::registry::tests::MockMarketDataProvider;
    use crate::market_data::types::TickerLookupResult;

    fn hit(symbol: &str, exchange: &str, name: &str, relevance: f64) -> TickerLookupHit {
        TickerLookupHit {
            symbol: symbol.to_string(),
            short_name: Some(name.to_string()),
            long_name: None,
            quote_type: Some("EQUITY".to_string()),
            exchange: Some(exchange.to_string()),
            currency: None,
            schemes: Vec::new(),
            relevance: Some(relevance),
        }
    }

    fn provider(
        id: &str,
        priority: i32,
        hits: Vec<TickerLookupHit>,
    ) -> Arc<dyn crate::market_data::trait_::MarketDataProvider + Send + Sync> {
        let mut mock = MockMarketDataProvider::new(id, &["TICKER"], priority);
        mock.lookup.insert(
            "apple".to_string(),
            Ok(TickerLookupResult {
                hits,
                source_id: id.to_string(),
            }),
        );
        Arc::new(mock)
    }

    fn snapshot(
        providers: Vec<Arc<dyn crate::market_data::trait_::MarketDataProvider + Send + Sync>>,
    ) -> RegistrySnapshot {
        DataSourceRegistry::new(providers)
            .expect("registry")
            .snapshot()
    }

    fn request() -> IdentifierSearchRequest {
        IdentifierSearchRequest::new("apple", None, None, None, None, None, None).expect("request")
    }

    #[test]
    fn validation_accepts_the_documented_bounds() {
        assert!(IdentifierSearchRequest::new("ab", None, None, None, None, None, None).is_ok());
        assert!(
            IdentifierSearchRequest::new(&"a".repeat(120), None, None, None, None, None, None)
                .is_ok()
        );
        assert!(
            IdentifierSearchRequest::new(&"a".repeat(121), None, None, None, None, None, None)
                .is_err()
        );
        assert!(IdentifierSearchRequest::new(" a ", None, None, None, None, None, None).is_err());
        assert!(
            IdentifierSearchRequest::new("ab", Some("holdings"), None, None, None, None, None)
                .is_err()
        );
        assert!(IdentifierSearchRequest::new("ab", None, None, None, None, Some(0), None).is_err());
        assert!(
            IdentifierSearchRequest::new("ab", None, None, None, None, Some(101), None).is_err()
        );
        assert!(
            IdentifierSearchRequest::new("ab", None, None, None, None, None, Some(-1)).is_err()
        );
        let defaults = IdentifierSearchRequest::new("ab", None, None, None, None, None, None)
            .expect("defaults");
        assert_eq!(defaults.domain, DataDomain::MarketData);
        assert_eq!(defaults.limit, DEFAULT_LIMIT);
        assert_eq!(defaults.offset, 0);
    }

    #[tokio::test]
    async fn hits_are_normalized_into_venue_qualified_identifiers() {
        let snapshot = snapshot(vec![provider(
            "one",
            1,
            vec![hit("AAPL", "NMS", "Apple Inc.", 0.9)],
        )]);
        let result = search_without_known_state(&snapshot, &request()).await;
        assert_eq!(result.outcome, SearchOutcome::Complete);
        assert_eq!(result.total, 1);
        let candidate = &result.results[0];
        assert_eq!(candidate.symbol, "AAPL");
        assert_eq!(candidate.asset_type.as_deref(), Some("EQUITY"));
        assert_eq!(candidate.venue_mic.as_deref(), Some("XNAS"));
        assert_eq!(candidate.identifiers[0].scheme, "TICKER:XNAS");
        assert!(candidate.identifiers[0].primary);
        assert!(candidate.addable);
        assert!(candidate.creatable);
    }

    #[tokio::test]
    async fn unmapped_exchanges_fall_back_to_a_bare_ticker() {
        let snapshot = snapshot(vec![provider(
            "one",
            1,
            vec![hit("ABC", "GER", "ABC AG", 0.5)],
        )]);
        let result = search_without_known_state(&snapshot, &request()).await;
        let candidate = &result.results[0];
        assert_eq!(candidate.venue_mic, None);
        assert_eq!(candidate.exchange.as_deref(), Some("GER"));
        assert_eq!(candidate.identifiers[0].scheme, "TICKER");
    }

    #[tokio::test]
    async fn same_symbol_on_two_venues_stays_separate() {
        let snapshot = snapshot(vec![provider(
            "one",
            1,
            vec![hit("ABC", "NMS", "ABC", 0.9), hit("ABC", "NYQ", "ABC", 0.8)],
        )]);
        let result = search_without_known_state(&snapshot, &request()).await;
        assert_eq!(result.total, 2);
        let venues = result
            .results
            .iter()
            .map(|candidate| candidate.venue_mic.as_deref().unwrap_or(""))
            .collect::<Vec<_>>();
        assert_eq!(venues, vec!["XNAS", "XNYS"]);
    }

    #[tokio::test]
    async fn venue_less_answers_do_not_merge_with_venue_qualified_ones() {
        let mut unqualified = hit("ABC", "GER", "ABC AG", 0.9);
        unqualified.exchange = None;
        let snapshot = snapshot(vec![provider(
            "one",
            1,
            vec![hit("ABC", "NMS", "ABC Inc.", 0.8), unqualified],
        )]);
        let result = search_without_known_state(&snapshot, &request()).await;

        assert_eq!(result.total, 2);
        let schemes = result
            .results
            .iter()
            .map(|candidate| candidate.identifiers[0].scheme.as_str())
            .collect::<Vec<_>>();
        assert!(schemes.contains(&"TICKER:XNAS"));
        assert!(schemes.contains(&"TICKER"));
    }

    #[tokio::test]
    async fn alternate_identifiers_are_deduplicated_and_classified() {
        let mut source_hit = hit("AAPL", "NMS", "Apple Inc.", 0.9);
        source_hit.schemes = vec![
            ("ISIN".to_string(), "US0378331005".to_string()),
            ("TICKER:XNAS".to_string(), "AAPL".to_string()),
            ("NOT_A_SCHEME".to_string(), "X-1".to_string()),
        ];
        let snapshot = snapshot(vec![provider("one", 1, vec![source_hit])]);
        let result = search_without_known_state(&snapshot, &request()).await;

        let candidate = &result.results[0];
        assert_eq!(candidate.identifiers.len(), 3);
        assert_eq!(candidate.identifiers[0].scheme, "TICKER:XNAS");
        assert!(candidate.identifiers[0].primary);
        let isin = candidate
            .identifiers
            .iter()
            .find(|identifier| identifier.scheme == "ISIN")
            .expect("ISIN alternate");
        assert!(isin.addable);
        assert!(!isin.creatable);
        assert!(isin.reason.is_some());
        let unknown = candidate
            .identifiers
            .iter()
            .find(|identifier| identifier.scheme == "NOT_A_SCHEME")
            .expect("unknown alternate");
        assert!(!unknown.addable);
        assert!(!unknown.creatable);
        assert!(unknown.reason.is_some());
    }

    #[tokio::test]
    async fn the_same_identifier_from_two_sources_consolidates() {
        let snapshot = snapshot(vec![
            provider("one", 1, vec![hit("AAPL", "NMS", "Apple Inc.", 0.9)]),
            provider("two", 2, vec![hit("AAPL", "NAS", "Apple Inc.", 0.7)]),
        ]);
        let result = search_without_known_state(&snapshot, &request()).await;
        assert_eq!(result.total, 1);
        let candidate = &result.results[0];
        assert_eq!(candidate.sources.len(), 2);
        assert_eq!(candidate.relevance, Some(0.9));
        assert!(candidate.conflicts.is_empty());
    }

    #[tokio::test]
    async fn currency_is_captured_and_conflicts_are_disclosed() {
        let mut first = hit("AAPL", "NMS", "Apple Inc.", 0.9);
        first.currency = Some("usd".to_string());
        let mut second = hit("AAPL", "NMS", "Apple Inc.", 0.8);
        second.currency = Some("eur".to_string());
        let snapshot = snapshot(vec![
            provider("one", 1, vec![first]),
            provider("two", 2, vec![second]),
        ]);
        let result = search_without_known_state(&snapshot, &request()).await;

        let candidate = &result.results[0];
        assert_eq!(candidate.currency.as_deref(), Some("USD"));
        let conflict = candidate
            .conflicts
            .iter()
            .find(|conflict| conflict.field == "currency")
            .expect("currency conflict");
        assert_eq!(conflict.values.len(), 2);
    }

    #[tokio::test]
    async fn conflicting_metadata_is_disclosed_with_sources() {
        let snapshot = snapshot(vec![
            provider("one", 1, vec![hit("AAPL", "NMS", "Apple Inc.", 0.9)]),
            provider(
                "two",
                2,
                vec![hit("AAPL", "NMS", "Apple Incorporated", 0.7)],
            ),
        ]);
        let result = search_without_known_state(&snapshot, &request()).await;
        let candidate = &result.results[0];
        let conflict = candidate
            .conflicts
            .iter()
            .find(|conflict| conflict.field == "display_name")
            .expect("display name conflict");
        assert_eq!(conflict.values.len(), 2);
        assert!(
            conflict
                .values
                .iter()
                .any(|value| value.sources == vec!["one"])
        );
        assert!(
            conflict
                .values
                .iter()
                .any(|value| value.sources == vec!["two"])
        );
    }

    #[tokio::test]
    async fn filters_apply_before_pagination_and_total_is_truthful() {
        let snapshot = snapshot(vec![provider(
            "one",
            1,
            vec![
                hit("AAA", "NMS", "AAA", 0.9),
                hit("BBB", "NYQ", "BBB", 0.8),
                hit("CCC", "NMS", "CCC", 0.7),
            ],
        )]);
        let mut request = request();
        request.exchange = Some("xnas".to_string());
        request.limit = 1;
        let result = search_without_known_state(&snapshot, &request).await;
        assert_eq!(result.total, 2);
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.results[0].symbol, "AAA");
    }

    #[tokio::test]
    async fn results_order_by_relevance_then_symbol() {
        let snapshot = snapshot(vec![provider(
            "one",
            1,
            vec![
                hit("ZZZ", "NMS", "ZZZ", 0.1),
                hit("BBB", "NMS", "BBB", 0.9),
                hit("AAA", "NMS", "AAA", 0.9),
            ],
        )]);
        let result = search_without_known_state(&snapshot, &request()).await;
        let symbols = result
            .results
            .iter()
            .map(|candidate| candidate.symbol.as_str())
            .collect::<Vec<_>>();
        assert_eq!(symbols, vec!["AAA", "BBB", "ZZZ"]);
    }

    #[tokio::test]
    async fn no_eligible_source_is_not_a_no_match() {
        let snapshot = snapshot(Vec::new());
        let result = search_without_known_state(&snapshot, &request()).await;
        assert_eq!(result.outcome, SearchOutcome::NoSources);
        assert!(result.total == 0);
        assert!(result.message.is_some());
    }

    #[tokio::test]
    async fn a_source_with_zero_hits_is_a_complete_no_match() {
        let snapshot = snapshot(vec![provider("one", 1, Vec::new())]);
        let result = search_without_known_state(&snapshot, &request()).await;
        assert_eq!(result.outcome, SearchOutcome::Complete);
        assert_eq!(result.total, 0);
    }

    #[test]
    fn provider_errors_map_to_the_five_source_statuses() {
        assert_eq!(
            map_provider_error_to_source_status(&MarketDataProviderError::NotFound("nope".into()))
                .status,
            SourceStatus::NoMatch
        );
        assert_eq!(
            map_provider_error_to_source_status(&MarketDataProviderError::RateLimited {
                msg: "budget".into(),
                retry_after: Some(Duration::from_secs(60)),
            })
            .status,
            SourceStatus::RateLimited
        );
        assert_eq!(
            map_provider_error_to_source_status(&MarketDataProviderError::Unsupported("no".into()))
                .status,
            SourceStatus::Rejected
        );
        assert_eq!(
            map_provider_error_to_source_status(&MarketDataProviderError::Transient("boom".into()))
                .status,
            SourceStatus::Unavailable
        );
        assert_eq!(
            map_provider_error_to_source_status(&MarketDataProviderError::Infrastructure(
                "boom".into()
            ))
            .status,
            SourceStatus::Unavailable
        );
    }

    fn failing_provider(
        id: &str,
        priority: i32,
        error: MarketDataProviderError,
    ) -> Arc<dyn crate::market_data::trait_::MarketDataProvider + Send + Sync> {
        let mut mock = MockMarketDataProvider::new(id, &["TICKER"], priority);
        mock.lookup.insert("apple".to_string(), Err(error));
        Arc::new(mock)
    }

    #[tokio::test]
    async fn a_failed_source_is_disclosed_without_losing_the_search() {
        let snapshot = snapshot(vec![failing_provider(
            "one",
            1,
            MarketDataProviderError::Transient("upstream down".into()),
        )]);
        let result = search_without_known_state(&snapshot, &request()).await;

        assert_eq!(result.outcome, SearchOutcome::Failed);
        assert_eq!(result.total, 0);
        assert_eq!(result.sources[0].status, SourceStatus::Unavailable);
        assert_eq!(result.sources[0].error_code.as_deref(), Some("unavailable"));
        assert_eq!(result.sources[0].message.as_deref(), Some("upstream down"));
        assert!(result.message.is_some());
    }

    #[tokio::test]
    async fn a_partial_source_failure_keeps_the_answers() {
        let snapshot = snapshot(vec![
            provider("one", 1, vec![hit("AAPL", "NMS", "Apple Inc.", 0.9)]),
            failing_provider(
                "two",
                2,
                MarketDataProviderError::Unauthorized("no key".into()),
            ),
        ]);
        let result = search_without_known_state(&snapshot, &request()).await;

        assert_eq!(result.outcome, SearchOutcome::Partial);
        assert_eq!(result.total, 1);
        assert_eq!(result.sources[0].status, SourceStatus::Answered);
        assert_eq!(result.sources[1].status, SourceStatus::Rejected);
        assert_eq!(result.sources[1].error_code.as_deref(), Some("rejected"));
    }

    #[test]
    fn provider_error_taxonomy_covers_the_rejected_variants() {
        for error in [
            MarketDataProviderError::InvalidInput("bad".into()),
            MarketDataProviderError::Unauthorized("denied".into()),
            MarketDataProviderError::Validation("invalid".into()),
        ] {
            let mapped = map_provider_error_to_source_status(&error);
            assert_eq!(mapped.status, SourceStatus::Rejected);
            assert_eq!(mapped.error_code, "rejected");
        }
        let no_cooldown =
            map_provider_error_to_source_status(&MarketDataProviderError::RateLimited {
                msg: "budget".into(),
                retry_after: None,
            });
        assert_eq!(no_cooldown.status, SourceStatus::RateLimited);
        assert_eq!(no_cooldown.retry_after_seconds, None);
    }

    #[test]
    fn search_errors_display_and_convert_domain_failures() {
        assert_eq!(
            IdentifierSearchError::Validation("short query".into()).to_string(),
            "short query"
        );
        assert_eq!(
            IdentifierSearchError::Storage("database down".into()).to_string(),
            "database down"
        );
        let converted: IdentifierSearchError =
            crate::domain::error::DomainError::Validation("bad row".into()).into();
        assert_eq!(
            converted,
            IdentifierSearchError::Storage("validation: bad row".to_string())
        );
    }

    #[tokio::test]
    async fn known_state_storage_failures_are_typed() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(50))
            .connect_lazy("postgres://user:pass@127.0.0.1:1/obolargus")
            .expect("lazy pool");
        let snapshot = snapshot(vec![provider(
            "one",
            1,
            vec![hit("AAPL", "NMS", "Apple Inc.", 0.9)],
        )]);
        let mut result = search_without_known_state(&snapshot, &request()).await;
        let error = apply_known_state(&pool, &mut result.results)
            .await
            .expect_err("unreachable database must fail");

        assert!(matches!(error, IdentifierSearchError::Storage(_)));
    }

    #[tokio::test]
    async fn equal_relevance_breaks_ties_by_priority_and_source() {
        let snapshot = snapshot(vec![
            provider("one", 1, vec![hit("AAPL", "NMS", "Apple Inc.", 0.9)]),
            provider("two", 2, vec![hit("AAPL", "NMS", "Apple Inc.", 0.9)]),
        ]);
        let result = search_without_known_state(&snapshot, &request()).await;

        let candidate = &result.results[0];
        assert_eq!(candidate.display_name, "Apple Inc.");
        assert_eq!(candidate.sources.len(), 2);
    }

    #[tokio::test]
    async fn equal_relevance_breaks_symbol_ties_by_venue() {
        let snapshot = snapshot(vec![provider(
            "one",
            1,
            vec![hit("ABC", "NYQ", "ABC", 0.9), hit("ABC", "NMS", "ABC", 0.9)],
        )]);
        let result = search_without_known_state(&snapshot, &request()).await;

        let venues = result
            .results
            .iter()
            .map(|candidate| candidate.venue_mic.as_deref().unwrap_or(""))
            .collect::<Vec<_>>();
        assert_eq!(venues, vec!["XNAS", "XNYS"]);
    }

    #[tokio::test]
    async fn the_same_source_keeps_its_best_relevance() {
        let snapshot = snapshot(vec![provider(
            "one",
            1,
            vec![
                hit("AAPL", "NMS", "Apple Inc.", 0.2),
                hit("AAPL", "NMS", "Apple Inc.", 0.9),
            ],
        )]);
        let result = search_without_known_state(&snapshot, &request()).await;

        let candidate = &result.results[0];
        assert_eq!(candidate.sources[0].relevance, Some(0.9));
    }

    #[tokio::test]
    async fn alternate_tickers_are_labelled_and_blank_values_are_skipped() {
        let mut source_hit = hit("AAPL", "NMS", "Apple Inc.", 0.9);
        source_hit.schemes = vec![
            ("TICKER:XNYS".to_string(), "AAPL".to_string()),
            ("ticker".to_string(), "aapl".to_string()),
            (String::new(), "ignored".to_string()),
            ("ISIN".to_string(), String::new()),
        ];
        let snapshot = snapshot(vec![provider("one", 1, vec![source_hit])]);
        let result = search_without_known_state(&snapshot, &request()).await;

        let identifiers = &result.results[0].identifiers;
        assert_eq!(identifiers.len(), 3);
        assert_eq!(identifiers[1].scheme, "TICKER:XNYS");
        assert_eq!(identifiers[1].label, "Ticker (XNYS)");
        assert_eq!(identifiers[2].scheme, "TICKER");
        assert_eq!(identifiers[2].label, "Ticker");
        assert!(identifiers[2].creatable);
    }

    #[tokio::test]
    async fn an_unknown_asset_type_cannot_be_added_or_created() {
        let mut source_hit = hit("ABC", "NMS", "ABC", 0.9);
        source_hit.quote_type = None;
        let snapshot = snapshot(vec![provider("one", 1, vec![source_hit])]);
        let result = search_without_known_state(&snapshot, &request()).await;

        let candidate = &result.results[0];
        assert!(!candidate.addable);
        assert!(!candidate.creatable);
        assert_eq!(
            candidate.not_addable_reason.as_deref(),
            Some("The load cannot create an asset whose type is not recognized.")
        );
        assert_eq!(
            candidate.creation_blocked_reason.as_deref(),
            Some("The load cannot create an asset whose type is not recognized.")
        );
    }

    #[tokio::test]
    async fn an_unrepresentable_symbol_stays_visible_but_not_addable() {
        let snapshot = snapshot(vec![provider(
            "one",
            1,
            vec![hit("BAD SYMBOL", "NMS", "Bad", 0.9)],
        )]);
        let result = search_without_known_state(&snapshot, &request()).await;

        let candidate = &result.results[0];
        assert!(!candidate.addable);
        assert!(!candidate.creatable);
        assert_eq!(
            candidate.not_addable_reason.as_deref(),
            Some("The symbol cannot be expressed as a load identifier.")
        );
        assert_eq!(
            candidate.creation_blocked_reason.as_deref(),
            Some("The identifier cannot enter the batch.")
        );
    }

    #[tokio::test]
    async fn source_and_asset_type_filters_are_exact_and_case_insensitive() {
        let snapshot = snapshot(vec![
            provider("one", 1, vec![hit("AAPL", "NMS", "Apple Inc.", 0.9)]),
            provider("two", 2, vec![hit("MSFT", "NMS", "Microsoft", 0.8)]),
        ]);
        let mut request = request();
        request.source = Some("TWO".to_string());
        request.asset_type = Some("equity".to_string());
        let result = search_without_known_state(&snapshot, &request).await;
        assert_eq!(result.total, 1);
        assert_eq!(result.results[0].symbol, "MSFT");

        request.asset_type = Some("ETF".to_string());
        let filtered = search_without_known_state(&snapshot, &request).await;
        assert_eq!(filtered.total, 0);

        request.asset_type = None;
        request.source = Some("missing".to_string());
        let missing = search_without_known_state(&snapshot, &request).await;
        assert_eq!(missing.total, 0);
    }

    #[test]
    fn outcome_derivation_distinguishes_partial_and_failed() {
        let answered = SearchSourceOutcome {
            source_id: "a".into(),
            display_name: "A".into(),
            status: SourceStatus::Answered,
            hit_count: 1,
            error_code: None,
            message: None,
            retry_after_seconds: None,
        };
        let no_match = SearchSourceOutcome {
            status: SourceStatus::NoMatch,
            hit_count: 0,
            ..answered.clone()
        };
        let failed = SearchSourceOutcome {
            status: SourceStatus::Unavailable,
            hit_count: 0,
            ..answered.clone()
        };
        assert_eq!(
            derive_search_outcome(&[answered.clone(), no_match.clone()]),
            SearchOutcome::Complete
        );
        assert_eq!(
            derive_search_outcome(&[answered.clone(), failed.clone()]),
            SearchOutcome::Partial
        );
        assert_eq!(
            derive_search_outcome(std::slice::from_ref(&failed)),
            SearchOutcome::Failed
        );
        assert_eq!(derive_search_outcome(&[]), SearchOutcome::NoSources);
    }
}
