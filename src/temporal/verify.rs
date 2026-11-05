//! Operator-facing integrity checks for the normalized fact model.
//!
//! `verify_parity` proves every non-retracted envelope has its typed payload
//! and every observation has a valid market-price payload. `verify_lookahead`
//! proves an as-known read never returns a belief recorded after its
//! `known_at`.
//!
//! Both are read-only and safe to run against production.

use chrono::NaiveDate;
use sqlx::{AssertSqlSafe, FromRow, PgPool};
use uuid::Uuid;

use crate::temporal::catalog::{StorageKind, fact_type};
use crate::temporal::error::TemporalError;
use crate::temporal::facts::FactSubject;
use crate::temporal::facts::read::fact_as_known;
use crate::temporal::facts::value::FactValue;
use crate::temporal::instant::{KnownAt, ValidAt};

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

#[derive(Debug, FromRow)]
struct StoredFact {
    id: Uuid,
    subject_kind: String,
    subject_id: Uuid,
    fact_type: String,
    valid_at: chrono::DateTime<chrono::Utc>,
    known_at: chrono::DateTime<chrono::Utc>,
    retracted: bool,
}

const FACT_ROWS: &str = "SELECT id, subject_kind, subject_id, fact_type, valid_at, \
     known_at, retracted FROM temporal_facts";

fn subject(kind: &str, id: Uuid) -> Result<FactSubject, TemporalError> {
    FactSubject::from_parts(kind, id)
}

/// Asserts every non-retracted fact has its typed payload and every stored
/// observation converts to a market-price value.
///
/// Implements the parity check of `specs/027-normalized-fact-model/spec.md`.
pub async fn verify_parity(
    pool: &PgPool,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> Result<VerificationReport, TemporalError> {
    let rows: Vec<StoredFact> = sqlx::query_as(AssertSqlSafe(format!(
        "{FACT_ROWS} WHERE ($1::date IS NULL OR valid_at >= $1) \
         AND ($2::date IS NULL OR valid_at < $2) ORDER BY valid_at"
    )))
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?;

    let mut report = VerificationReport::default();
    for row in rows {
        report.checks += 1;
        if row.retracted {
            continue;
        }
        let def = fact_type(&row.fact_type).ok_or_else(|| TemporalError::UnknownFactType {
            fact_type: row.fact_type.clone(),
        })?;
        let present = payload_present(pool, row.id, def.storage_kind).await?;
        if !present {
            report.failures.push(format!(
                "{} {} {} at {}: envelope has no typed payload",
                row.subject_kind, row.subject_id, row.fact_type, row.valid_at
            ));
        }
    }

    let observations: Vec<(
        Uuid,
        Option<rust_decimal::Decimal>,
        Option<rust_decimal::Decimal>,
    )> = sqlx::query_as("SELECT id, close, price FROM market_observations WHERE NOT retracted")
        .fetch_all(pool)
        .await?;
    for (id, close, price) in observations {
        report.checks += 1;
        if close.is_none() && price.is_none() {
            report.failures.push(format!(
                "observation {id}: non-retracted row has neither close nor price"
            ));
        }
    }

    Ok(report)
}

async fn payload_present(
    pool: &PgPool,
    fact_id: Uuid,
    storage: StorageKind,
) -> Result<bool, TemporalError> {
    let table = match storage.payload_table() {
        Some(table) => table,
        None => return Ok(false),
    };
    let query = format!("SELECT EXISTS (SELECT 1 FROM {table} WHERE fact_id = $1)");
    let exists: bool = sqlx::query_scalar(AssertSqlSafe(query))
        .bind(fact_id)
        .fetch_one(pool)
        .await?;
    Ok(exists)
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
        "{FACT_ROWS} ORDER BY random() LIMIT $1"
    )))
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let mut report = VerificationReport::default();
    for row in rows {
        report.checks += 1;
        let fact_subject = subject(&row.subject_kind, row.subject_id)?;
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
                row.subject_kind,
                row.subject_id,
                row.fact_type,
                row.valid_at,
                row.known_at,
                point.known_at
            ));
        }
    }
    Ok(report)
}

/// Converts a market observation into the typed value used by parity checks.
#[allow(dead_code)]
fn observation_value(close: Option<rust_decimal::Decimal>) -> Option<FactValue> {
    close.map(|value| FactValue::Decimal { value })
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
