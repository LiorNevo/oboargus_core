//! Bitemporal fact reads: current, as-of, as-known, history, series, profile,
//! and coverage.
//!
//! Every read resolves with the shared rule in [`super::super::resolution`]:
//! greatest `known_at`, ties broken by greatest `id`. A retracted winner means
//! "no value at this instant".

use chrono::{DateTime, Utc};
use sqlx::{AssertSqlSafe, PgPool};

use super::super::error::TemporalError;
use super::super::instant::{KnownAt, TemporalWindow, ValidAt};
use super::{FactPoint, FactRow, FactSubject, column_list, row_to_fact_point};

/// The value in force at `valid_at` under the latest knowledge.
///
/// Implements FR-005, FR-006, FR-007, FR-027, and FR-043 of
/// `specs/021-bitemporal-market-data/spec.md`.
pub async fn fact_at(
    pool: &PgPool,
    subject: FactSubject,
    fact_type: &str,
    valid_at: ValidAt,
) -> Result<Option<FactPoint>, TemporalError> {
    resolve_one(pool, subject, fact_type, valid_at, None).await
}

/// The value as believed at `known_at` about `valid_at`.
///
/// A row recorded after `known_at` never participates, even if it is the
/// newest belief today.
///
/// Implements FR-005, FR-006, and FR-027 of
/// `specs/021-bitemporal-market-data/spec.md`.
pub async fn fact_as_known(
    pool: &PgPool,
    subject: FactSubject,
    fact_type: &str,
    valid_at: ValidAt,
    known_at: KnownAt,
) -> Result<Option<FactPoint>, TemporalError> {
    resolve_one(pool, subject, fact_type, valid_at, Some(known_at)).await
}

/// Every revision of one fact, newest knowledge first.
///
/// Retractions are returned here; this is the only fact read that surfaces
/// them.
///
/// Implements FR-007 of `specs/021-bitemporal-market-data/spec.md`.
pub async fn fact_history(
    pool: &PgPool,
    subject: FactSubject,
    fact_type: &str,
    valid_at: ValidAt,
) -> Result<Vec<FactPoint>, TemporalError> {
    let rows: Vec<FactRow> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT {} FROM {} WHERE {} = $1 AND fact_type = $2 AND valid_at = $3 \
         ORDER BY known_at DESC, id DESC",
        column_list(subject),
        subject.table(),
        subject.subject_column()
    )))
    .bind(subject.id())
    .bind(fact_type)
    .bind(valid_at.as_datetime())
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| row_to_fact_point(row, subject))
        .collect()
}

/// The resolved series over a window, optionally as known at `known_at`.
///
/// One point per valid instant: the greatest `known_at` (and greatest `id`)
/// wins; a retracted winner removes the point from the series rather than
/// synthesising a value.
///
/// Implements FR-008 and FR-020 of `specs/021-bitemporal-market-data/spec.md`.
pub async fn fact_series(
    pool: &PgPool,
    subject: FactSubject,
    fact_type: &str,
    window: TemporalWindow,
    known_at: Option<KnownAt>,
) -> Result<Vec<FactPoint>, TemporalError> {
    let rows: Vec<FactRow> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT DISTINCT ON (valid_at) {} FROM {} \
         WHERE {} = $1 AND fact_type = $2 AND valid_at >= $3 AND valid_at < $4 \
           AND ($5::timestamptz IS NULL OR known_at <= $5) \
         ORDER BY valid_at, known_at DESC, id DESC",
        column_list(subject),
        subject.table(),
        subject.subject_column()
    )))
    .bind(subject.id())
    .bind(fact_type)
    .bind(window.from.as_datetime())
    .bind(window.to.as_datetime())
    .bind(known_at.map(|instant| instant.as_datetime()))
    .fetch_all(pool)
    .await?;

    let mut points = Vec::with_capacity(rows.len());
    for row in rows {
        let point = row_to_fact_point(row, subject)?;
        if !point.retracted {
            points.push(point);
        }
    }
    Ok(points)
}

async fn resolve_one(
    pool: &PgPool,
    subject: FactSubject,
    fact_type: &str,
    valid_at: ValidAt,
    known_at: Option<KnownAt>,
) -> Result<Option<FactPoint>, TemporalError> {
    let row: Option<FactRow> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT {} FROM {} \
         WHERE {} = $1 AND fact_type = $2 AND valid_at <= $3 \
           AND ($4::timestamptz IS NULL OR known_at <= $4) \
         ORDER BY valid_at DESC, known_at DESC, id DESC LIMIT 1",
        column_list(subject),
        subject.table(),
        subject.subject_column()
    )))
    .bind(subject.id())
    .bind(fact_type)
    .bind(valid_at.as_datetime())
    .bind(known_at.map(|instant| instant.as_datetime()))
    .fetch_optional(pool)
    .await?;

    match row {
        Some(row) => {
            let point = row_to_fact_point(row, subject)?;
            if point.retracted {
                Ok(None)
            } else {
                Ok(Some(point))
            }
        }
        None => Ok(None),
    }
}

/// The instant "now" as a valid-time coordinate, used by CLI defaults.
pub fn now_valid_at() -> Result<ValidAt, TemporalError> {
    let now = Utc::now();
    let micros =
        now - chrono::Duration::nanoseconds(i64::from(now.timestamp_subsec_nanos() % 1_000));
    ValidAt::new(micros)
}

/// The instant "now" as a transaction-time coordinate.
pub fn now_known_at() -> KnownAt {
    KnownAt::now()
}

/// Parses an RFC 3339 instant and wraps it as a valid-time coordinate.
pub fn parse_valid_at(raw: &str) -> Result<ValidAt, TemporalError> {
    let parsed: DateTime<Utc> = raw
        .parse()
        .map_err(|error| TemporalError::invalid_time(format!("{raw:?}: {error}")))?;
    ValidAt::new(parsed)
}

/// Parses an RFC 3339 instant and wraps it as a transaction-time coordinate.
pub fn parse_known_at(raw: &str) -> Result<KnownAt, TemporalError> {
    let parsed: DateTime<Utc> = raw
        .parse()
        .map_err(|error| TemporalError::invalid_time(format!("{raw:?}: {error}")))?;
    KnownAt::new(parsed)
}
