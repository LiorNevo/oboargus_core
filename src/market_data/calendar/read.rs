//! Calendar and assignment reads.

use std::collections::BTreeSet;

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::domain::error::DomainError;
use crate::domain::id::{AssetId, DomainId};

use super::entity::{
    AffectedAsset, CalendarAssignment, CalendarAssignmentPage, CalendarAssignmentView,
    CalendarAuditEntry, CalendarDetail, CalendarException, CalendarExceptionKind, CalendarInUse,
    CalendarSummary, CalendarWeekday, ExchangeCalendar,
};

#[derive(Debug, FromRow)]
struct ExchangeCalendarRow {
    mic: String,
    name: String,
    time_zone: String,
    coverage_from: NaiveDate,
    coverage_to: NaiveDate,
    source: String,
    provenance: String,
    version: i32,
    active: bool,
    updated_by: String,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct CalendarExceptionRow {
    exception_date: NaiveDate,
    kind: String,
    open_local: Option<chrono::NaiveTime>,
    close_local: Option<chrono::NaiveTime>,
    reason: Option<String>,
    source: String,
}

/// Reads one calendar, or `None` when the venue code is unknown.
pub async fn get_calendar(
    pool: &PgPool,
    mic: &str,
) -> Result<Option<ExchangeCalendar>, DomainError> {
    let mic = mic.trim().to_ascii_uppercase();
    let row: Option<ExchangeCalendarRow> = sqlx::query_as(
        "SELECT mic, name, time_zone, coverage_from, coverage_to, source, provenance, \
                version, active, updated_by, updated_at \
         FROM exchange_calendars WHERE mic = $1",
    )
    .bind(&mic)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };

    let weekdays: Vec<CalendarWeekday> = sqlx::query_as(
        "SELECT iso_weekday, open_local, close_local \
         FROM exchange_calendar_weekdays WHERE mic = $1 ORDER BY iso_weekday",
    )
    .bind(&mic)
    .fetch_all(pool)
    .await?;

    let exception_rows: Vec<CalendarExceptionRow> = sqlx::query_as(
        "SELECT exception_date, kind, open_local, close_local, reason, source \
         FROM exchange_calendar_exceptions WHERE mic = $1 ORDER BY exception_date",
    )
    .bind(&mic)
    .fetch_all(pool)
    .await?;

    let mut exceptions = Vec::with_capacity(exception_rows.len());
    for row in exception_rows {
        let kind = CalendarExceptionKind::parse(&row.kind).ok_or_else(|| {
            DomainError::Validation(format!(
                "calendar {mic} has unknown exception kind '{}'",
                row.kind
            ))
        })?;
        exceptions.push(CalendarException {
            date: row.exception_date,
            kind,
            open: row.open_local,
            close: row.close_local,
            reason: row.reason,
            source: row.source,
        });
    }

    Ok(Some(ExchangeCalendar {
        mic: row.mic,
        name: row.name,
        time_zone: row.time_zone,
        coverage_from: row.coverage_from,
        coverage_to: row.coverage_to,
        weekdays,
        exceptions,
        source: row.source,
        provenance: row.provenance,
        version: row.version,
        active: row.active,
        updated_by: row.updated_by,
        updated_at: row.updated_at,
    }))
}

/// Reads every calendar. The seeded and managed venue set is small, so the
/// per-calendar child reads are intentionally simple.
pub async fn list_calendars(pool: &PgPool) -> Result<Vec<ExchangeCalendar>, DomainError> {
    let mics: Vec<(String,)> = sqlx::query_as("SELECT mic FROM exchange_calendars ORDER BY mic")
        .fetch_all(pool)
        .await?;
    let mut calendars = Vec::with_capacity(mics.len());
    for (mic,) in mics {
        if let Some(calendar) = get_calendar(pool, &mic).await? {
            calendars.push(calendar);
        }
    }
    Ok(calendars)
}

/// Reads explicit assignments overlapping the inclusive `[from, to]` range.
pub async fn list_assignments_for_asset(
    pool: &PgPool,
    asset_id: AssetId,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<CalendarAssignment>, DomainError> {
    let rows: Vec<CalendarAssignment> = sqlx::query_as(
        "SELECT id, asset_id, mic, valid_from, valid_to, source, created_by, created_at \
         FROM asset_calendar_assignments \
         WHERE asset_id = $1 AND valid_from <= $3 \
           AND (valid_to IS NULL OR valid_to > $2) \
         ORDER BY valid_from, id",
    )
    .bind(asset_id.as_uuid())
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Lists all assignments for one calendar, optionally paginated.
pub async fn list_assignments_for_calendar(
    pool: &PgPool,
    mic: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<CalendarAssignment>, DomainError> {
    let rows: Vec<CalendarAssignment> = sqlx::query_as(
        "SELECT id, asset_id, mic, valid_from, valid_to, source, created_by, created_at \
         FROM asset_calendar_assignments WHERE mic = $1 \
         ORDER BY valid_from, asset_id LIMIT $2 OFFSET $3",
    )
    .bind(mic.trim().to_ascii_uppercase())
    .bind(limit.clamp(1, 200))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Counts explicit assignments for one calendar.
pub async fn count_assignments_for_calendar(pool: &PgPool, mic: &str) -> Result<i64, DomainError> {
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM asset_calendar_assignments WHERE mic = $1")
            .bind(mic.trim().to_ascii_uppercase())
            .fetch_one(pool)
            .await?;
    Ok(count)
}

/// Reads the distinct stored end-of-day dates for one asset in `[from, to]`.
///
/// The query mirrors the chart series resolver: newest `known_at`, then id,
/// per valid time, excluding retracted winners.
pub async fn stored_daily_dates(
    pool: &PgPool,
    asset_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<BTreeSet<NaiveDate>, DomainError> {
    let upper = to
        .succ_opt()
        .ok_or_else(|| DomainError::Validation("date range overflow".to_string()))?;
    let rows: Vec<(DateTime<Utc>, bool)> = sqlx::query_as(
        "SELECT valid_at, retracted FROM ( \
            SELECT DISTINCT ON (valid_at) valid_at, retracted \
            FROM market_observations \
            WHERE asset_id = $1 AND kind = 'eod' AND interval_seconds = 0 \
              AND valid_at >= $2 AND valid_at < $3 \
            ORDER BY valid_at, known_at DESC, id DESC \
         ) resolved WHERE NOT retracted",
    )
    .bind(asset_id)
    .bind(DateTime::<Utc>::from_naive_utc_and_offset(
        from.and_hms_opt(0, 0, 0)
            .ok_or_else(|| DomainError::Validation("invalid date".to_string()))?,
        Utc,
    ))
    .bind(DateTime::<Utc>::from_naive_utc_and_offset(
        upper
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| DomainError::Validation("invalid date".to_string()))?,
        Utc,
    ))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(valid_at, _)| valid_at.date_naive())
        .collect())
}

/// Lists calendars with assigned/resolved counts and exception counts.
pub async fn list_calendar_summaries(
    pool: &PgPool,
    today: chrono::NaiveDate,
) -> Result<Vec<CalendarSummary>, DomainError> {
    let rows: Vec<CalendarSummaryRow> = sqlx::query_as(
        "SELECT c.mic, c.name, c.time_zone, c.coverage_from, c.coverage_to, \
                c.source, c.provenance, c.version, c.active, c.updated_by, c.updated_at, \
                (SELECT COUNT(DISTINCT a.asset_id) FROM asset_calendar_assignments a \
                  WHERE a.mic = c.mic AND (a.valid_to IS NULL OR a.valid_to > $1)) \
                    AS assigned_asset_count, \
                (SELECT COUNT(DISTINCT x.asset_id) FROM asset_calendar_assignments x \
                  WHERE x.mic = c.mic AND x.valid_from <= $1 \
                    AND (x.valid_to IS NULL OR $1 < x.valid_to)) \
                + (SELECT COUNT(DISTINCT ai.asset_id) FROM asset_identifiers ai \
                    WHERE upper(split_part(ai.scheme, ':', 1)) = 'TICKER' \
                      AND upper(trim(split_part(ai.scheme, ':', 2))) = c.mic \
                      AND NOT EXISTS (SELECT 1 FROM asset_calendar_assignments e \
                        WHERE e.asset_id = ai.asset_id AND e.valid_from <= $1 \
                          AND (e.valid_to IS NULL OR $1 < e.valid_to)) \
                      AND 1 = (SELECT COUNT(DISTINCT upper(trim(split_part(s.scheme, ':', 2)))) \
                               FROM asset_identifiers s WHERE s.asset_id = ai.asset_id \
                                 AND upper(split_part(s.scheme, ':', 1)) = 'TICKER' \
                                 AND trim(split_part(s.scheme, ':', 2)) <> '')) \
                    AS resolved_asset_count, \
                (SELECT COUNT(*) FROM exchange_calendar_exceptions e WHERE e.mic = c.mic) \
                    AS exception_count \
         FROM exchange_calendars c ORDER BY c.mic",
    )
    .bind(today)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(CalendarSummaryRow::into_summary)
        .collect())
}

/// Reads one calendar with usage counts and its 20 most recent audit rows.
pub async fn get_calendar_detail(
    pool: &PgPool,
    mic: &str,
    today: chrono::NaiveDate,
) -> Result<CalendarDetail, DomainError> {
    let calendar = get_calendar(pool, mic)
        .await?
        .ok_or_else(|| DomainError::not_found("ExchangeCalendar", mic.to_string()))?;
    let summary = list_calendar_summaries(pool, today)
        .await?
        .into_iter()
        .find(|summary| summary.mic == calendar.mic);
    let audit = list_calendar_audit(pool, &calendar.mic, 20).await?;
    Ok(CalendarDetail {
        calendar,
        assigned_asset_count: summary
            .as_ref()
            .map(|summary| summary.assigned_asset_count)
            .unwrap_or(0),
        resolved_asset_count: summary
            .map(|summary| summary.resolved_asset_count)
            .unwrap_or(0),
        audit,
    })
}

/// Reads the most recent audit rows for one calendar.
pub async fn list_calendar_audit(
    pool: &PgPool,
    mic: &str,
    limit: i64,
) -> Result<Vec<CalendarAuditEntry>, DomainError> {
    Ok(sqlx::query_as(
        "SELECT id, action, actor_id, summary, created_at \
         FROM exchange_calendar_audit WHERE mic = $1 \
         ORDER BY created_at DESC, id DESC LIMIT $2",
    )
    .bind(mic)
    .bind(limit.clamp(1, 100))
    .fetch_all(pool)
    .await?)
}

/// Lists assignments with instrument display context.
pub async fn list_assignment_views(
    pool: &PgPool,
    asset_id: Option<Uuid>,
    mic: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<CalendarAssignmentPage, DomainError> {
    let limit = limit.clamp(1, 200);
    let offset = offset.max(0);
    type Row = (
        Uuid,
        Uuid,
        String,
        Option<String>,
        String,
        chrono::NaiveDate,
        Option<chrono::NaiveDate>,
        String,
        String,
        chrono::DateTime<chrono::Utc>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT a.id, a.asset_id, assets.display_name, \
                (SELECT CASE WHEN ai.scheme IS NULL THEN NULL \
                             ELSE ai.scheme || ':' || ai.value END \
                 FROM asset_identifiers ai WHERE ai.asset_id = a.asset_id \
                 ORDER BY (upper(ai.scheme) LIKE 'TICKER%') DESC, ai.created_at, ai.id LIMIT 1), \
                a.mic, a.valid_from, a.valid_to, a.source, a.created_by, a.created_at \
         FROM asset_calendar_assignments a JOIN assets ON assets.id = a.asset_id \
         WHERE ($1::uuid IS NULL OR a.asset_id = $1) \
           AND ($2::text IS NULL OR a.mic = $2) \
         ORDER BY a.asset_id, a.valid_from LIMIT $3 OFFSET $4",
    )
    .bind(asset_id)
    .bind(mic)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM asset_calendar_assignments \
         WHERE ($1::uuid IS NULL OR asset_id = $1) AND ($2::text IS NULL OR mic = $2)",
    )
    .bind(asset_id)
    .bind(mic)
    .fetch_one(pool)
    .await?;
    Ok(CalendarAssignmentPage {
        assignments: rows
            .into_iter()
            .map(|row| CalendarAssignmentView {
                id: row.0,
                asset_id: row.1,
                asset_display_name: row.2,
                asset_identifier: row.3,
                mic: row.4,
                valid_from: row.5,
                valid_to: row.6,
                source: row.7,
                created_by: row.8,
                created_at: row.9,
            })
            .collect(),
        total,
        limit,
        offset,
    })
}

/// Instruments affected by retiring one calendar, capped at 100 names.
pub async fn affected_assets(
    pool: &PgPool,
    mic: &str,
    today: chrono::NaiveDate,
) -> Result<CalendarInUse, DomainError> {
    type Row = (Uuid, String, Option<String>);
    let rows: Vec<Row> = sqlx::query_as(
        "WITH affected AS ( \
           SELECT DISTINCT asset_id FROM asset_calendar_assignments \
           WHERE mic = $1 AND (valid_to IS NULL OR valid_to > $2) \
           UNION \
           SELECT DISTINCT ai.asset_id FROM asset_identifiers ai \
           WHERE upper(split_part(ai.scheme, ':', 1)) = 'TICKER' \
             AND upper(trim(split_part(ai.scheme, ':', 2))) = $1 \
             AND NOT EXISTS (SELECT 1 FROM asset_calendar_assignments e \
               WHERE e.asset_id = ai.asset_id AND e.valid_from <= $2 \
                 AND (e.valid_to IS NULL OR $2 < e.valid_to)) \
             AND 1 = (SELECT COUNT(DISTINCT upper(trim(split_part(s.scheme, ':', 2)))) \
                      FROM asset_identifiers s WHERE s.asset_id = ai.asset_id \
                        AND upper(split_part(s.scheme, ':', 1)) = 'TICKER' \
                        AND trim(split_part(s.scheme, ':', 2)) <> '') \
         ) \
         SELECT assets.id, assets.display_name, \
                (SELECT CASE WHEN ai.scheme IS NULL THEN NULL \
                             ELSE ai.scheme || ':' || ai.value END \
                 FROM asset_identifiers ai WHERE ai.asset_id = assets.id \
                 ORDER BY (upper(ai.scheme) LIKE 'TICKER%') DESC, ai.created_at, ai.id LIMIT 1) \
         FROM affected JOIN assets ON assets.id = affected.asset_id \
         ORDER BY assets.display_name, assets.id LIMIT 100",
    )
    .bind(mic)
    .bind(today)
    .fetch_all(pool)
    .await?;
    let count: i64 = sqlx::query_scalar(
        "WITH affected AS ( \
           SELECT DISTINCT asset_id FROM asset_calendar_assignments \
           WHERE mic = $1 AND (valid_to IS NULL OR valid_to > $2) \
           UNION \
           SELECT DISTINCT ai.asset_id FROM asset_identifiers ai \
           WHERE upper(split_part(ai.scheme, ':', 1)) = 'TICKER' \
             AND upper(trim(split_part(ai.scheme, ':', 2))) = $1 \
             AND NOT EXISTS (SELECT 1 FROM asset_calendar_assignments e \
               WHERE e.asset_id = ai.asset_id AND e.valid_from <= $2 \
                 AND (e.valid_to IS NULL OR $2 < e.valid_to)) \
             AND 1 = (SELECT COUNT(DISTINCT upper(trim(split_part(s.scheme, ':', 2)))) \
                      FROM asset_identifiers s WHERE s.asset_id = ai.asset_id \
                        AND upper(split_part(s.scheme, ':', 1)) = 'TICKER' \
                        AND trim(split_part(s.scheme, ':', 2)) <> '') \
         ) SELECT COUNT(*) FROM affected",
    )
    .bind(mic)
    .bind(today)
    .fetch_one(pool)
    .await?;
    Ok(CalendarInUse {
        affected_count: count,
        affected_assets: rows
            .into_iter()
            .map(|row| AffectedAsset {
                asset_id: row.0,
                display_name: row.1,
                identifier: row.2,
            })
            .collect(),
    })
}

#[derive(Debug, sqlx::FromRow)]
struct CalendarSummaryRow {
    mic: String,
    name: String,
    time_zone: String,
    coverage_from: chrono::NaiveDate,
    coverage_to: chrono::NaiveDate,
    source: String,
    provenance: String,
    version: i32,
    active: bool,
    updated_by: String,
    updated_at: chrono::DateTime<chrono::Utc>,
    assigned_asset_count: i64,
    resolved_asset_count: i64,
    exception_count: i64,
}

impl CalendarSummaryRow {
    fn into_summary(self) -> CalendarSummary {
        CalendarSummary {
            mic: self.mic,
            name: self.name,
            time_zone: self.time_zone,
            coverage_from: self.coverage_from,
            coverage_to: self.coverage_to,
            source: self.source,
            provenance: self.provenance,
            version: self.version,
            active: self.active,
            updated_by: self.updated_by,
            updated_at: self.updated_at,
            assigned_asset_count: self.assigned_asset_count,
            resolved_asset_count: self.resolved_asset_count,
            exception_count: self.exception_count,
        }
    }
}
