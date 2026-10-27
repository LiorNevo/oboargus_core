//! Append-only fact writes: revisions and retractions.
//!
//! No function here updates or deletes a row. A correction is a new row with
//! a later `known_at`; a retraction is a new row with `retracted = TRUE`.

use chrono::{DateTime, Utc};
use sqlx::{AssertSqlSafe, PgPool};

use crate::domain::id::new_uuid;

use super::super::error::TemporalError;
use super::super::instant::{KnownAt, ValidAt};
use super::{FactPoint, FactRow, FactSubject, column_list, insert_columns, row_to_fact_point};

/// Appends a revision of a fact.
///
/// `known_at` must be strictly later than the current winner for the same
/// `(subject, fact_type, valid_at, source)`; otherwise the call returns
/// [`TemporalError::ConflictingAppend`]. The comparison and the insert share
/// one transaction so two writers cannot both "win".
///
/// Implements FR-010, FR-011, and FR-041 of
/// `specs/021-bitemporal-market-data/spec.md`.
pub async fn append_fact(
    pool: &PgPool,
    subject: FactSubject,
    fact_type: &str,
    valid_at: ValidAt,
    known_at: KnownAt,
    value: &str,
    source: &str,
) -> Result<FactPoint, TemporalError> {
    insert_belief(
        pool, subject, fact_type, valid_at, known_at, value, source, false,
    )
    .await
}

/// Appends a retraction. Resolution after `known_at` reports "no value";
/// earlier as-known reads still see the retracted belief.
///
/// Implements FR-012 and FR-013 of `specs/021-bitemporal-market-data/spec.md`.
pub async fn revoke_fact(
    pool: &PgPool,
    subject: FactSubject,
    fact_type: &str,
    valid_at: ValidAt,
    known_at: KnownAt,
    reason: &str,
    source: &str,
) -> Result<FactPoint, TemporalError> {
    insert_belief(
        pool, subject, fact_type, valid_at, known_at, reason, source, true,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn insert_belief(
    pool: &PgPool,
    subject: FactSubject,
    fact_type: &str,
    valid_at: ValidAt,
    known_at: KnownAt,
    value: &str,
    source: &str,
    retracted: bool,
) -> Result<FactPoint, TemporalError> {
    if fact_type.trim().is_empty() {
        return Err(TemporalError::invalid_time("fact_type must not be empty"));
    }
    if source.trim().is_empty() {
        return Err(TemporalError::invalid_time("source must not be empty"));
    }

    let identity = format!(
        "{}={} fact_type={fact_type} valid_at={valid_at} source={source}",
        subject.subject_name(),
        subject.id()
    );

    let mut tx = pool.begin().await?;

    let current: Option<(DateTime<Utc>,)> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT known_at FROM {} WHERE {} = $1 AND fact_type = $2 AND valid_at = $3 \
         AND source = $4 ORDER BY known_at DESC, id DESC LIMIT 1",
        subject.table(),
        subject.subject_column()
    )))
    .bind(subject.id())
    .bind(fact_type)
    .bind(valid_at.as_datetime())
    .bind(source)
    .fetch_optional(&mut *tx)
    .await?;

    if let Some((winner,)) = current
        && known_at.as_datetime() <= winner
    {
        return Err(TemporalError::conflict(identity));
    }

    let id = new_uuid();
    let row: FactRow = sqlx::query_as(AssertSqlSafe(format!(
        "INSERT INTO {} ({}) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
         RETURNING {}",
        subject.table(),
        insert_columns(subject),
        column_list(subject)
    )))
    .bind(id)
    .bind(subject.id())
    .bind(fact_type)
    .bind(valid_at.as_datetime())
    .bind(known_at.as_datetime())
    .bind(value)
    .bind(source)
    .bind(retracted)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;

    row_to_fact_point(row, subject)
}
