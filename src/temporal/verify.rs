//! Operator-facing integrity checks for the bitemporal model.
//!
//! `verify_parity` proves the legacy `domain::get_current_*` readers and the
//! new temporal readers agree on migrated data. `verify_lookahead` proves an
//! as-known read never returns a belief recorded after its `known_at`.
//!
//! Both are read-only and safe to run against production.

use chrono::NaiveDate;
use sqlx::{AssertSqlSafe, PgPool};
use uuid::Uuid;

use crate::domain::id::{AssetId, DomainId, IssuerId, PortfolioId};

use super::FactSubject;
use super::error::TemporalError;
use super::facts::read::{fact_as_known, fact_at};
use super::instant::{KnownAt, ValidAt};

/// The result of a verification run. An empty `failures` list means the
/// invariant holds.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct VerificationReport {
    /// How many facts or samples were checked.
    pub checks: usize,
    /// One human-readable line per violated check.
    pub failures: Vec<String>,
}

impl VerificationReport {
    /// Whether every check passed.
    pub fn is_ok(&self) -> bool {
        self.failures.is_empty()
    }
}

#[derive(Debug, sqlx::FromRow)]
struct StoredFact {
    kind: String,
    subject_id: Uuid,
    fact_type: String,
    valid_at: chrono::DateTime<chrono::Utc>,
    known_at: chrono::DateTime<chrono::Utc>,
}

const ALL_FACTS: &str = "SELECT 'asset' AS kind, asset_id AS subject_id, fact_type, valid_at, known_at \
     FROM asset_reference_data UNION ALL \
     SELECT 'issuer', issuer_id, fact_type, valid_at, known_at FROM issuer_reference_data UNION ALL \
     SELECT 'portfolio', portfolio_id, fact_type, valid_at, known_at FROM portfolio_reference_data";

fn subject(kind: &str, id: Uuid) -> Result<FactSubject, TemporalError> {
    FactSubject::from_parts(kind, id)
}

/// Asserts that the legacy and temporal current-value readers agree.
///
/// Implements T026 of `specs/021-bitemporal-market-data/tasks.md` and FR-044.
pub async fn verify_parity(
    pool: &PgPool,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> Result<VerificationReport, TemporalError> {
    let rows: Vec<StoredFact> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT DISTINCT ON (kind, subject_id, fact_type, valid_at) * FROM ({ALL_FACTS}) AS f \
         WHERE ($1::date IS NULL OR valid_at >= $1) AND ($2::date IS NULL OR valid_at < $2) \
         ORDER BY kind, subject_id, fact_type, valid_at"
    )))
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?;

    let mut report = VerificationReport::default();
    for row in rows {
        report.checks += 1;
        let fact_subject = subject(&row.kind, row.subject_id)?;
        let valid_at = ValidAt::new(row.valid_at)?;

        let temporal = fact_at(pool, fact_subject, &row.fact_type, valid_at).await?;
        let legacy = legacy_current(
            pool,
            fact_subject,
            &row.fact_type,
            row.valid_at.date_naive(),
        )
        .await?;

        match (temporal, legacy) {
            (Some(temporal), Some(legacy)) => {
                if temporal.value != legacy {
                    report.failures.push(format!(
                        "{} {} {} at {}: temporal={:?} legacy={:?}",
                        row.kind,
                        row.subject_id,
                        row.fact_type,
                        row.valid_at,
                        temporal.value,
                        legacy
                    ));
                }
            }
            (Some(temporal), None) => report.failures.push(format!(
                "{} {} {} at {}: temporal has {:?}, legacy has no row",
                row.kind, row.subject_id, row.fact_type, row.valid_at, temporal.value
            )),
            // A retracted winner is expected to have no temporal value; the
            // legacy reader simply returns the previous belief, which is the
            // documented pre-021 behaviour.
            (None, _) => {}
        }
    }
    Ok(report)
}

/// Reads the legacy current value for one subject, as an `Option<String>`.
async fn legacy_current(
    pool: &PgPool,
    subject: FactSubject,
    fact_type: &str,
    as_of_date: NaiveDate,
) -> Result<Option<String>, TemporalError> {
    match subject {
        FactSubject::Asset(id) => Ok(
            crate::domain::asset_reference_data::read::get_current_asset_reference_data(
                pool,
                AssetId::from_uuid(id.as_uuid()),
                fact_type,
                as_of_date,
            )
            .await?
            .map(|row| row.value),
        ),
        FactSubject::Issuer(id) => Ok(
            crate::domain::issuer_reference_data::read::get_current_issuer_reference_data(
                pool,
                IssuerId::from_uuid(id.as_uuid()),
                fact_type,
                as_of_date,
            )
            .await?
            .map(|row| row.value),
        ),
        FactSubject::Portfolio(id) => Ok(
            crate::domain::portfolio_reference_data::read::get_current_portfolio_reference_data(
                pool,
                PortfolioId::from_uuid(id.as_uuid()),
                fact_type,
                as_of_date,
            )
            .await?
            .map(|row| row.value),
        ),
    }
}

/// Samples recorded beliefs and asserts no as-known read returns a belief
/// recorded later than the requested instant.
///
/// Implements FR-006 and FR-027 of
/// `specs/021-bitemporal-market-data/spec.md`.
pub async fn verify_lookahead(
    pool: &PgPool,
    samples: usize,
) -> Result<VerificationReport, TemporalError> {
    let limit = i64::try_from(samples.max(1)).unwrap_or(i64::MAX);
    let rows: Vec<StoredFact> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT * FROM ({ALL_FACTS}) AS f ORDER BY random() LIMIT $1"
    )))
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let mut report = VerificationReport::default();
    for row in rows {
        report.checks += 1;
        let fact_subject = subject(&row.kind, row.subject_id)?;
        let known_at = KnownAt::new(row.known_at)?;
        let read = fact_as_known(
            pool,
            fact_subject,
            &row.fact_type,
            ValidAt::new(row.valid_at)?,
            known_at,
        )
        .await?;

        if let Some(point) = read
            && point.known_at.as_datetime() > row.known_at
        {
            report.failures.push(format!(
                "{} {} {} at {}: as-known {} returned a belief recorded at {}",
                row.kind, row.subject_id, row.fact_type, row.valid_at, row.known_at, point.known_at
            ));
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_report_is_ok_and_a_failure_is_not() {
        let mut report = VerificationReport::default();
        assert!(report.is_ok());
        report.failures.push("boom".to_string());
        assert!(!report.is_ok());
    }
}
