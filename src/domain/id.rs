//! Typed id newtypes for the high-level domain model.
//!
//! Implements the id scaffolding described in
//! `specs/016-implement-domain-model/data-model.md` § "Entities." Each
//! newtype wraps a `Uuid` and is the canonical way to refer to a record of
//! the corresponding entity.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A typed id for a `Portfolio` record. Implements `Display`, `FromStr`,
/// `Serialize`, `Deserialize`, and `sqlx::Type` so it can round-trip through
/// the database and over the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(transparent)]
#[serde(transparent)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct PortfolioId(pub Uuid);

/// A typed id for an `Asset` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(transparent)]
#[serde(transparent)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct AssetId(pub Uuid);

/// A typed id for an `Event` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(transparent)]
#[serde(transparent)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct EventId(pub Uuid);

/// A typed id for an `Issuer` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(transparent)]
#[serde(transparent)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct IssuerId(pub Uuid);

/// A typed id for any of the three reference-data records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(transparent)]
#[serde(transparent)]
/// Implements FR-008 of spec 016-implement-domain-model.
pub struct ReferenceDataId(pub Uuid);

/// A typed id for either of the two external-identifier records
/// (`AssetIdentifier`, `IssuerIdentifier`). Mirrors `ReferenceDataId`: one
/// newtype covers both domains, exactly as one covers the three
/// reference-data entities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(transparent)]
#[serde(transparent)]
/// Implements FR-023 of spec 016-implement-domain-model.
pub struct IdentifierId(pub Uuid);

/// A typed id for a `Rule` record — a rule expression that governs a
/// portfolio. Implements FR-030 of spec 016-implement-domain-model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(transparent)]
#[serde(transparent)]
pub struct RuleId(pub Uuid);

/// A typed id for a `data_source_refresh_runs` audit row.
/// Implements FR-007 of spec 017-yahoo-data-source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(transparent)]
#[serde(transparent)]
pub struct DataSourceRefreshRunId(pub Uuid);
/// A typed id for a `data_source_reconciliation_audits` audit row.
/// Implements FR-022 of spec 017-yahoo-data-source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(transparent)]
#[serde(transparent)]
pub struct ReconciliationAuditId(pub Uuid);

/// A typed id for a `data_sources` configuration row.
/// Implements FR-002 of spec 017-yahoo-data-source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(transparent)]
#[serde(transparent)]
pub struct DataSourceId(pub Uuid);

/// A trait implemented by all entity-id newtypes. Provides a uniform
/// constructor and a uniform accessor.
pub trait DomainId: Copy + Eq {
    /// Construct a new id from a `Uuid`.
    fn from_uuid(uuid: Uuid) -> Self;
    /// Return the underlying `Uuid`.
    fn as_uuid(&self) -> Uuid;
}

macro_rules! impl_domain_id {
    ($ty:ty) => {
        impl DomainId for $ty {
            fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }
            fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl FromStr for $ty {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(Uuid::from_str(s)?))
            }
        }
    };
}

impl_domain_id!(PortfolioId);
impl_domain_id!(AssetId);
impl_domain_id!(EventId);
impl_domain_id!(IssuerId);
impl_domain_id!(ReferenceDataId);
impl_domain_id!(IdentifierId);
impl_domain_id!(RuleId);
impl_domain_id!(DataSourceRefreshRunId);
impl_domain_id!(ReconciliationAuditId);
impl_domain_id!(DataSourceId);

/// Generate a fresh `Uuid` v4.
/// Implements FR-008 of spec 016-implement-domain-model.
pub fn new_uuid() -> Uuid {
    Uuid::new_v4()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_uuid() {
        let id = PortfolioId::from_uuid(new_uuid());
        let s = id.to_string();
        let parsed: PortfolioId = s.parse().expect("uuid parse");
        assert_eq!(id, parsed);
    }

    #[test]
    fn serde_json_round_trip() {
        let id = AssetId::from_uuid(new_uuid());
        let s = serde_json::to_string(&id).expect("serialize");
        let parsed: AssetId = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(id, parsed);
    }
}
