//! Bitemporal fact reads for the normalized model.
//!
//! Scalar facts resolve from `temporal_facts` plus the typed payload table.
//! Market prices resolve from `market_observations` directly. Every read uses
//! the shared latest-`known_at` rule; retracted winners become "no value".

use chrono::{DateTime, Utc};
use sqlx::{AssertSqlSafe, FromRow, PgPool};
use uuid::Uuid;

use crate::domain::id::DomainId;
use crate::temporal::catalog::{StorageKind, fact_type as catalog_fact_type};
use crate::temporal::error::TemporalError;
use crate::temporal::facts::value::FactValue;
use crate::temporal::facts::{FactCoverage, FactPoint, FactSeriesPage, FactSubject};
use crate::temporal::instant::{KnownAt, TemporalWindow, ValidAt};
use crate::temporal::observations::{Observation, observation_range};

#[derive(Debug, FromRow)]
pub(crate) struct FactRow {
    pub id: Uuid,
    pub subject_kind: String,
    pub subject_id: Uuid,
    pub fact_type: String,
    pub valid_at: DateTime<Utc>,
    pub known_at: DateTime<Utc>,
    pub source: String,
    pub retracted: bool,
    pub retraction_reason: Option<String>,
    pub content_hash: String,
}

const FACT_COLUMNS: &str = "id, subject_kind, subject_id, fact_type, valid_at, known_at, \
     source, retracted, retraction_reason, content_hash";

/// Verifies the subject row exists before a read returns an empty result.
async fn ensure_subject_exists(pool: &PgPool, subject: FactSubject) -> Result<(), TemporalError> {
    let table = match subject.kind() {
        crate::temporal::catalog::SubjectKind::Asset => "assets",
        crate::temporal::catalog::SubjectKind::Issuer => "issuers",
        crate::temporal::catalog::SubjectKind::Portfolio => "portfolios",
    };
    let exists: bool = sqlx::query_scalar(AssertSqlSafe(format!(
        "SELECT EXISTS (SELECT 1 FROM {table} WHERE id = $1)"
    )))
    .bind(subject.id())
    .fetch_one(pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(TemporalError::UnknownSubject {
            subject: subject.subject_name().to_string(),
            id: subject.id().to_string(),
        })
    }
}

/// The value in force at `valid_at` under the latest knowledge.
pub async fn fact_at(
    pool: &PgPool,
    subject: FactSubject,
    fact_type: &str,
    valid_at: ValidAt,
) -> Result<Option<FactPoint>, TemporalError> {
    ensure_subject_exists(pool, subject).await?;
    let def = catalog_fact_type(fact_type).ok_or_else(|| TemporalError::UnknownFactType {
        fact_type: fact_type.to_string(),
    })?;
    if def.storage_kind == StorageKind::MarketObservation {
        return market_price_at(
            pool,
            subject,
            def.observation_kind,
            def.interval_seconds,
            valid_at,
            None,
        )
        .await;
    }
    resolve_one(pool, subject, fact_type, valid_at, None).await
}

/// The value as believed at `known_at` about `valid_at`.
pub async fn fact_as_known(
    pool: &PgPool,
    subject: FactSubject,
    fact_type: &str,
    valid_at: ValidAt,
    known_at: KnownAt,
) -> Result<Option<FactPoint>, TemporalError> {
    ensure_subject_exists(pool, subject).await?;
    let def = catalog_fact_type(fact_type).ok_or_else(|| TemporalError::UnknownFactType {
        fact_type: fact_type.to_string(),
    })?;
    if def.storage_kind == StorageKind::MarketObservation {
        return market_price_at(
            pool,
            subject,
            def.observation_kind,
            def.interval_seconds,
            valid_at,
            Some(known_at),
        )
        .await;
    }
    resolve_one(pool, subject, fact_type, valid_at, Some(known_at)).await
}

/// Every revision of one fact, newest knowledge first.
///
/// Retractions are returned here; this is the only fact read that surfaces
/// them.
pub async fn fact_history(
    pool: &PgPool,
    subject: FactSubject,
    fact_type: &str,
    valid_at: ValidAt,
) -> Result<Vec<FactPoint>, TemporalError> {
    ensure_subject_exists(pool, subject).await?;
    let def = catalog_fact_type(fact_type).ok_or_else(|| TemporalError::UnknownFactType {
        fact_type: fact_type.to_string(),
    })?;
    if def.storage_kind == StorageKind::MarketObservation {
        return market_price_history(
            pool,
            subject,
            def.observation_kind,
            def.interval_seconds,
            valid_at,
        )
        .await;
    }

    let rows: Vec<FactRow> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT {FACT_COLUMNS} FROM temporal_facts \
         WHERE subject_kind = $1 AND subject_id = $2 AND fact_type = $3 AND valid_at = $4 \
         ORDER BY known_at DESC, id DESC"
    )))
    .bind(subject.kind().as_str())
    .bind(subject.id())
    .bind(fact_type)
    .bind(valid_at.as_datetime())
    .fetch_all(pool)
    .await?;

    let mut points = Vec::with_capacity(rows.len());
    for row in rows {
        points.push(row_to_point(pool, row).await?);
    }
    Ok(points)
}

/// The resolved series over a window, optionally as known at `known_at`.
pub async fn fact_series(
    pool: &PgPool,
    subject: FactSubject,
    fact_type: &str,
    window: TemporalWindow,
    known_at: Option<KnownAt>,
) -> Result<Vec<FactPoint>, TemporalError> {
    let page = fact_series_page(pool, subject, fact_type, window, known_at, None, 20_000).await?;
    Ok(page.points)
}

/// A paginated resolved series.
pub async fn fact_series_page(
    pool: &PgPool,
    subject: FactSubject,
    fact_type: &str,
    window: TemporalWindow,
    known_at: Option<KnownAt>,
    after: Option<ValidAt>,
    limit: u32,
) -> Result<FactSeriesPage, TemporalError> {
    ensure_subject_exists(pool, subject).await?;
    let def = catalog_fact_type(fact_type).ok_or_else(|| TemporalError::UnknownFactType {
        fact_type: fact_type.to_string(),
    })?;
    if def.storage_kind == StorageKind::MarketObservation {
        return market_price_series_page(
            pool,
            subject,
            def.observation_kind,
            def.interval_seconds,
            window,
            known_at,
            after,
            limit,
        )
        .await;
    }

    let bounded = limit.clamp(1, 20_000) as i64;
    let fetch = bounded + 1;
    let rows: Vec<FactRow> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT * FROM ( \
            SELECT DISTINCT ON (valid_at) {FACT_COLUMNS} FROM temporal_facts \
            WHERE subject_kind = $1 AND subject_id = $2 AND fact_type = $3 \
              AND valid_at >= $4 AND valid_at < $5 \
              AND ($6::timestamptz IS NULL OR known_at <= $6) \
            ORDER BY valid_at, known_at DESC, id DESC \
         ) resolved \
         WHERE NOT retracted \
           AND ($7::timestamptz IS NULL OR valid_at > $7) \
         ORDER BY valid_at ASC \
         LIMIT $8"
    )))
    .bind(subject.kind().as_str())
    .bind(subject.id())
    .bind(fact_type)
    .bind(window.from.as_datetime())
    .bind(window.to.as_datetime())
    .bind(known_at.map(|instant| instant.as_datetime()))
    .bind(after.map(|instant| instant.as_datetime()))
    .bind(fetch)
    .fetch_all(pool)
    .await?;

    let has_more = i64::try_from(rows.len()).unwrap_or(i64::MAX) > bounded;
    let rows: Vec<FactRow> = rows.into_iter().take(bounded as usize).collect();
    let mut points = Vec::with_capacity(rows.len());
    for row in rows {
        points.push(row_to_point(pool, row).await?);
    }
    Ok(FactSeriesPage { points, has_more })
}

/// Coverage for every fact type about one subject.
pub async fn fact_coverage(
    pool: &PgPool,
    subject: FactSubject,
) -> Result<Vec<FactCoverage>, TemporalError> {
    ensure_subject_exists(pool, subject).await?;
    let rows: Vec<FactCoverageRow> = sqlx::query_as(
        r#"
        SELECT fact_type,
               MIN(valid_at) AS first_valid_at,
               MAX(valid_at) AS last_valid_at,
               MIN(known_at) AS first_known_at,
               MAX(known_at) AS last_known_at,
               COUNT(*) AS revisions
        FROM temporal_facts
        WHERE subject_kind = $1 AND subject_id = $2
        GROUP BY fact_type
        ORDER BY fact_type
        "#,
    )
    .bind(subject.kind().as_str())
    .bind(subject.id())
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| FactCoverage {
            fact_type: row.fact_type,
            first_valid_at: row.first_valid_at,
            last_valid_at: row.last_valid_at,
            first_known_at: row.first_known_at,
            last_known_at: row.last_known_at,
            revisions: row.revisions,
        })
        .collect())
}

#[derive(Debug, FromRow)]
struct FactCoverageRow {
    fact_type: String,
    first_valid_at: Option<DateTime<Utc>>,
    last_valid_at: Option<DateTime<Utc>>,
    first_known_at: Option<DateTime<Utc>>,
    last_known_at: Option<DateTime<Utc>>,
    revisions: i64,
}

async fn resolve_one(
    pool: &PgPool,
    subject: FactSubject,
    fact_type: &str,
    valid_at: ValidAt,
    known_at: Option<KnownAt>,
) -> Result<Option<FactPoint>, TemporalError> {
    let row: Option<FactRow> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT {FACT_COLUMNS} FROM temporal_facts \
         WHERE subject_kind = $1 AND subject_id = $2 AND fact_type = $3 AND valid_at <= $4 \
           AND ($5::timestamptz IS NULL OR known_at <= $5) \
         ORDER BY valid_at DESC, known_at DESC, id DESC LIMIT 1"
    )))
    .bind(subject.kind().as_str())
    .bind(subject.id())
    .bind(fact_type)
    .bind(valid_at.as_datetime())
    .bind(known_at.map(|instant| instant.as_datetime()))
    .fetch_optional(pool)
    .await?;

    match row {
        Some(row) => {
            let point = row_to_point(pool, row).await?;
            if point.retracted {
                Ok(None)
            } else {
                Ok(Some(point))
            }
        }
        None => Ok(None),
    }
}

async fn row_to_point(pool: &PgPool, row: FactRow) -> Result<FactPoint, TemporalError> {
    let def = catalog_fact_type(&row.fact_type).ok_or_else(|| TemporalError::UnknownFactType {
        fact_type: row.fact_type.clone(),
    })?;
    let subject = FactSubject::from_parts(&row.subject_kind, row.subject_id)?;
    let value = if row.retracted {
        None
    } else {
        Some(load_value(pool, row.id, def.storage_kind).await?)
    };

    Ok(FactPoint {
        id: row.id,
        subject,
        fact_type: row.fact_type,
        schema_version: def.schema_version,
        valid_at: ValidAt::new(row.valid_at)?,
        known_at: KnownAt::new(row.known_at)?,
        value,
        source: row.source,
        retracted: row.retracted,
        retraction_reason: row.retraction_reason,
        content_hash: row.content_hash,
    })
}

async fn load_value(
    pool: &PgPool,
    fact_id: Uuid,
    storage: StorageKind,
) -> Result<FactValue, TemporalError> {
    let missing = || TemporalError::MissingPayload { fact_id };
    match storage {
        StorageKind::Text => {
            let value: Option<String> =
                sqlx::query_scalar("SELECT value FROM fact_text_values WHERE fact_id = $1")
                    .bind(fact_id)
                    .fetch_optional(pool)
                    .await?;
            Ok(FactValue::Text {
                value: value.ok_or_else(missing)?,
            })
        }
        StorageKind::Decimal => {
            let value: Option<rust_decimal::Decimal> =
                sqlx::query_scalar("SELECT value FROM fact_decimal_values WHERE fact_id = $1")
                    .bind(fact_id)
                    .fetch_optional(pool)
                    .await?;
            Ok(FactValue::Decimal {
                value: value.ok_or_else(missing)?,
            })
        }
        StorageKind::Integer => {
            let value: Option<i64> =
                sqlx::query_scalar("SELECT value FROM fact_integer_values WHERE fact_id = $1")
                    .bind(fact_id)
                    .fetch_optional(pool)
                    .await?;
            Ok(FactValue::Integer {
                value: value.ok_or_else(missing)?,
            })
        }
        StorageKind::Boolean => {
            let value: Option<bool> =
                sqlx::query_scalar("SELECT value FROM fact_boolean_values WHERE fact_id = $1")
                    .bind(fact_id)
                    .fetch_optional(pool)
                    .await?;
            Ok(FactValue::Boolean {
                value: value.ok_or_else(missing)?,
            })
        }
        StorageKind::Date => {
            let value: Option<chrono::NaiveDate> =
                sqlx::query_scalar("SELECT value FROM fact_date_values WHERE fact_id = $1")
                    .bind(fact_id)
                    .fetch_optional(pool)
                    .await?;
            Ok(FactValue::Date {
                value: value.ok_or_else(missing)?,
            })
        }
        StorageKind::Timestamp => {
            let value: Option<DateTime<Utc>> =
                sqlx::query_scalar("SELECT value FROM fact_timestamp_values WHERE fact_id = $1")
                    .bind(fact_id)
                    .fetch_optional(pool)
                    .await?;
            Ok(FactValue::Timestamp {
                value: value.ok_or_else(missing)?,
            })
        }
        StorageKind::MarketObservation => Err(TemporalError::InvalidFactValue {
            fact_type: "market_price".to_string(),
            detail: "market observations are not stored in temporal_facts".to_string(),
        }),
    }
}

/// Loads one fact point by envelope id.
pub async fn fact_point_by_id(pool: &PgPool, id: Uuid) -> Result<Option<FactPoint>, TemporalError> {
    let row: Option<FactRow> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT {FACT_COLUMNS} FROM temporal_facts WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    match row {
        Some(row) => Ok(Some(row_to_point(pool, row).await?)),
        None => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// Market-price facade over market_observations.
// ---------------------------------------------------------------------------

async fn market_price_at(
    pool: &PgPool,
    subject: FactSubject,
    kind: Option<&str>,
    interval_seconds: Option<i32>,
    valid_at: ValidAt,
    known_at: Option<KnownAt>,
) -> Result<Option<FactPoint>, TemporalError> {
    let FactSubject::Asset(asset_id) = subject else {
        return Err(TemporalError::InvalidSubject {
            subject: "market_price is only defined for assets".to_string(),
        });
    };
    let kind = kind.unwrap_or("eod");
    let interval = interval_seconds.unwrap_or(0);

    let row: Option<Observation> = sqlx::query_as(
        r#"
        SELECT * FROM market_observations
        WHERE asset_id = $1 AND kind = $2 AND interval_seconds = $3
          AND valid_at <= $4
          AND ($5::timestamptz IS NULL OR known_at <= $5)
        ORDER BY valid_at DESC, known_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(asset_id.as_uuid())
    .bind(kind)
    .bind(interval)
    .bind(valid_at.as_datetime())
    .bind(known_at.map(|instant| instant.as_datetime()))
    .fetch_optional(pool)
    .await?;

    match row {
        Some(row) if !row.retracted => Ok(Some(row.to_fact_point()?)),
        _ => Ok(None),
    }
}

async fn market_price_history(
    pool: &PgPool,
    subject: FactSubject,
    kind: Option<&str>,
    interval_seconds: Option<i32>,
    valid_at: ValidAt,
) -> Result<Vec<FactPoint>, TemporalError> {
    let FactSubject::Asset(asset_id) = subject else {
        return Err(TemporalError::InvalidSubject {
            subject: "market_price is only defined for assets".to_string(),
        });
    };
    let kind = kind.unwrap_or("eod");
    let interval = interval_seconds.unwrap_or(0);

    let rows: Vec<Observation> = sqlx::query_as(
        r#"
        SELECT * FROM market_observations
        WHERE asset_id = $1 AND kind = $2 AND interval_seconds = $3 AND valid_at = $4
        ORDER BY known_at DESC, id DESC
        "#,
    )
    .bind(asset_id.as_uuid())
    .bind(kind)
    .bind(interval)
    .bind(valid_at.as_datetime())
    .fetch_all(pool)
    .await?;

    rows.into_iter().map(|row| row.to_fact_point()).collect()
}

#[allow(clippy::too_many_arguments)]
async fn market_price_series_page(
    pool: &PgPool,
    subject: FactSubject,
    kind: Option<&str>,
    interval_seconds: Option<i32>,
    window: TemporalWindow,
    known_at: Option<KnownAt>,
    after: Option<ValidAt>,
    limit: u32,
) -> Result<FactSeriesPage, TemporalError> {
    let FactSubject::Asset(asset_id) = subject else {
        return Err(TemporalError::InvalidSubject {
            subject: "market_price is only defined for assets".to_string(),
        });
    };
    let kind = kind.unwrap_or("eod");
    let interval = interval_seconds.unwrap_or(0);
    let bounded = limit.clamp(1, 20_000) as i64;
    let fetch = bounded + 1;

    let rows: Vec<Observation> = sqlx::query_as(
        r#"
        SELECT * FROM (
            SELECT DISTINCT ON (valid_at) *
            FROM market_observations
            WHERE asset_id = $1 AND kind = $2 AND interval_seconds = $3
              AND valid_at >= $4 AND valid_at < $5
              AND ($6::timestamptz IS NULL OR known_at <= $6)
            ORDER BY valid_at, known_at DESC, id DESC
        ) resolved
        WHERE NOT retracted
          AND ($7::timestamptz IS NULL OR valid_at > $7)
        ORDER BY valid_at ASC
        LIMIT $8
        "#,
    )
    .bind(asset_id.as_uuid())
    .bind(kind)
    .bind(interval)
    .bind(window.from.as_datetime())
    .bind(window.to.as_datetime())
    .bind(known_at.map(|instant| instant.as_datetime()))
    .bind(after.map(|instant| instant.as_datetime()))
    .bind(fetch)
    .fetch_all(pool)
    .await?;

    let has_more = i64::try_from(rows.len()).unwrap_or(i64::MAX) > bounded;
    let rows: Vec<Observation> = rows.into_iter().take(bounded as usize).collect();
    let points = rows
        .into_iter()
        .map(|row| row.to_fact_point())
        .collect::<Result<Vec<_>, _>>()?;
    Ok(FactSeriesPage { points, has_more })
}

/// Observation series for a specific source, used by the observation API.
pub async fn observation_series(
    pool: &PgPool,
    subject: FactSubject,
    source: &str,
    window: TemporalWindow,
    known_at: Option<KnownAt>,
) -> Result<Vec<FactPoint>, TemporalError> {
    ensure_subject_exists(pool, subject).await?;
    let FactSubject::Asset(asset_id) = subject else {
        return Err(TemporalError::InvalidSubject {
            subject: "market observations are only defined for assets".to_string(),
        });
    };
    let rows = observation_range(pool, asset_id, "eod", 0, source, window, known_at).await?;
    rows.into_iter().map(|row| row.to_fact_point()).collect()
}
