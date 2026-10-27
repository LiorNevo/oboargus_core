//! Bitemporal facts for assets, issuers, and portfolios.
//!
//! Every physical table (`asset_reference_data`, `issuer_reference_data`,
//! `portfolio_reference_data`) has the same shape and differs only in the
//! subject column. [`FactSubject`] owns that mapping so the write and read
//! paths share one decoder.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod read;
pub mod write;

use crate::domain::id::{AssetId, DomainId, IssuerId, PortfolioId};

use super::error::TemporalError;
use super::instant::{KnownAt, ValidAt};

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
    /// The physical table that stores this subject's facts.
    pub fn table(self) -> &'static str {
        match self {
            Self::Asset(_) => "asset_reference_data",
            Self::Issuer(_) => "issuer_reference_data",
            Self::Portfolio(_) => "portfolio_reference_data",
        }
    }

    /// The subject-id column in [`Self::table`].
    pub fn subject_column(self) -> &'static str {
        match self {
            Self::Asset(_) => "asset_id",
            Self::Issuer(_) => "issuer_id",
            Self::Portfolio(_) => "portfolio_id",
        }
    }

    /// The human-readable subject name used in errors and responses.
    pub fn subject_name(self) -> &'static str {
        match self {
            Self::Asset(_) => "asset",
            Self::Issuer(_) => "issuer",
            Self::Portfolio(_) => "portfolio",
        }
    }

    /// The subject id as a raw UUID.
    pub fn id(self) -> Uuid {
        match self {
            Self::Asset(id) => id.as_uuid(),
            Self::Issuer(id) => id.as_uuid(),
            Self::Portfolio(id) => id.as_uuid(),
        }
    }

    /// Rebuild a subject from its wire name and id.
    pub fn from_parts(kind: &str, id: Uuid) -> Result<Self, TemporalError> {
        match kind {
            "asset" => Ok(Self::Asset(AssetId::from_uuid(id))),
            "issuer" => Ok(Self::Issuer(IssuerId::from_uuid(id))),
            "portfolio" => Ok(Self::Portfolio(PortfolioId::from_uuid(id))),
            other => Err(TemporalError::InvalidTime {
                detail: format!("unknown fact subject {other:?}"),
            }),
        }
    }
}

/// One resolved fact.
///
/// `retracted` rows are returned by history reads only; the current, as-of,
/// and as-known readers map a retracted winner to "no value".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactPoint {
    /// The row id.
    pub id: Uuid,
    /// The fact's subject.
    pub subject: FactSubject,
    /// The fact kind (e.g. `market_price`).
    pub fact_type: String,
    /// When the fact is true in the world.
    pub valid_at: ValidAt,
    /// When the system recorded this belief.
    pub known_at: KnownAt,
    /// The domain payload (JSON in the typical case).
    pub value: String,
    /// The reporting source.
    pub source: String,
    /// Whether this belief retracts the value.
    pub retracted: bool,
}

/// The raw row shape shared by all three fact tables.
#[derive(Debug, sqlx::FromRow)]
pub(crate) struct FactRow {
    pub id: Uuid,
    pub subject_id: Uuid,
    pub fact_type: String,
    pub valid_at: DateTime<Utc>,
    pub known_at: DateTime<Utc>,
    pub value: String,
    pub source: String,
    pub retracted: bool,
}

/// The shared projection: every table's subject column is aliased to
/// `subject_id`, so one decoder serves all three.
pub(crate) fn column_list(subject: FactSubject) -> String {
    format!(
        "id, {} AS subject_id, fact_type, valid_at, known_at, value, source, retracted",
        subject.subject_column()
    )
}

/// The identity-preserving projection used in `INSERT (...)` column lists,
/// where an alias is not legal.
pub(crate) fn insert_columns(subject: FactSubject) -> String {
    format!(
        "id, {}, fact_type, valid_at, known_at, value, source, retracted",
        subject.subject_column()
    )
}

/// Decodes a row from any of the three fact tables into a [`FactPoint`].
pub(crate) fn row_to_fact_point(
    row: FactRow,
    subject: FactSubject,
) -> Result<FactPoint, TemporalError> {
    if row.subject_id != subject.id() {
        return Err(TemporalError::UnknownSubject {
            subject: subject.subject_name().to_string(),
            id: row.subject_id.to_string(),
        });
    }
    Ok(FactPoint {
        id: row.id,
        subject,
        fact_type: row.fact_type,
        valid_at: ValidAt::new(row.valid_at)?,
        known_at: KnownAt::new(row.known_at)?,
        value: row.value,
        source: row.source,
        retracted: row.retracted,
    })
}

#[cfg(test)]
mod tests;
