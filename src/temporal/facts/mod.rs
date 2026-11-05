//! Typed, append-only facts for assets, issuers, and portfolios.
//!
//! The table layout is described in
//! `specs/027-normalized-fact-model/data-model.md`. The catalog is in
//! [`crate::temporal::catalog`]; this module owns the value types and the
//! public read/write surface.

pub mod read;
pub mod value;
pub mod write;

#[cfg(test)]
mod tests;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::id::{AssetId, DomainId, IssuerId, PortfolioId};
use crate::temporal::catalog::SubjectKind;
use crate::temporal::error::TemporalError;
use crate::temporal::instant::{KnownAt, ValidAt};

pub use value::{Adjustment, FactValue, MarketPriceValue};
pub use write::{FactInput, RetractionInput, retract_fact, write_fact, write_fact_outcome};

/// The subject of a temporal fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "id")]
pub enum FactSubject {
    /// A fact about an [`AssetId`].
    Asset(AssetId),
    /// A fact about an [`IssuerId`].
    Issuer(IssuerId),
    /// A fact about a [`PortfolioId`].
    Portfolio(PortfolioId),
}

impl FactSubject {
    /// The catalog subject kind.
    pub fn kind(self) -> SubjectKind {
        match self {
            Self::Asset(_) => SubjectKind::Asset,
            Self::Issuer(_) => SubjectKind::Issuer,
            Self::Portfolio(_) => SubjectKind::Portfolio,
        }
    }

    /// The human-readable subject name used in errors and responses.
    pub fn subject_name(self) -> &'static str {
        self.kind().as_str()
    }

    /// The subject id as a raw UUID.
    pub fn id(self) -> Uuid {
        match self {
            Self::Asset(id) => id.as_uuid(),
            Self::Issuer(id) => id.as_uuid(),
            Self::Portfolio(id) => id.as_uuid(),
        }
    }

    /// The exclusive subject columns `(asset_id, issuer_id, portfolio_id)`.
    pub fn columns(self) -> (Option<Uuid>, Option<Uuid>, Option<Uuid>) {
        match self {
            Self::Asset(id) => (Some(id.as_uuid()), None, None),
            Self::Issuer(id) => (None, Some(id.as_uuid()), None),
            Self::Portfolio(id) => (None, None, Some(id.as_uuid())),
        }
    }

    /// Rebuild a subject from its wire name and id.
    pub fn from_parts(kind: &str, id: Uuid) -> Result<Self, TemporalError> {
        match kind {
            "asset" => Ok(Self::Asset(AssetId::from_uuid(id))),
            "issuer" => Ok(Self::Issuer(IssuerId::from_uuid(id))),
            "portfolio" => Ok(Self::Portfolio(PortfolioId::from_uuid(id))),
            other => Err(TemporalError::InvalidSubject {
                subject: other.to_string(),
            }),
        }
    }
}

/// One resolved fact.
///
/// `retracted` rows are returned by history reads only; current, as-of, and
/// as-known readers map a retracted winner to "no value".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactPoint {
    /// The row id (envelope id or observation id).
    pub id: Uuid,
    /// The fact's subject.
    pub subject: FactSubject,
    /// The fact kind (e.g. `market_price`).
    pub fact_type: String,
    /// The catalog schema version used when the fact was written.
    pub schema_version: i32,
    /// When the fact is true in the world.
    pub valid_at: ValidAt,
    /// When the system recorded this belief.
    pub known_at: KnownAt,
    /// The domain value; `None` for a retraction.
    pub value: Option<FactValue>,
    /// The reporting source.
    pub source: String,
    /// Whether this belief retracts the value.
    pub retracted: bool,
    /// Why the value was retracted.
    pub retraction_reason: Option<String>,
    /// Canonical hash of the typed value.
    pub content_hash: String,
}

impl FactPoint {
    /// The resolved value, or `None` for a retraction.
    pub fn resolved_value(&self) -> Option<&FactValue> {
        if self.retracted {
            None
        } else {
            self.value.as_ref()
        }
    }

    /// The value at a point in time, for server projections.
    pub fn valid_at_datetime(&self) -> DateTime<Utc> {
        self.valid_at.as_datetime()
    }
}

/// One page of a resolved series.
#[derive(Debug, Clone)]
pub struct FactSeriesPage {
    /// The resolved points in ascending valid time.
    pub points: Vec<FactPoint>,
    /// True when at least one more point exists after this page.
    pub has_more: bool,
}

/// Coverage for one fact type about one subject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FactCoverage {
    /// The fact type.
    pub fact_type: String,
    /// First valid instant.
    pub first_valid_at: Option<DateTime<Utc>>,
    /// Last valid instant.
    pub last_valid_at: Option<DateTime<Utc>>,
    /// First known instant.
    pub first_known_at: Option<DateTime<Utc>>,
    /// Last known instant.
    pub last_known_at: Option<DateTime<Utc>>,
    /// Number of beliefs.
    pub revisions: i64,
}
