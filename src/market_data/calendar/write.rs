//! Calendar document validation and atomic reference-data writes.

use std::collections::BTreeSet;
use std::str::FromStr;

use chrono::NaiveTime;
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::error::DomainError;

use super::entity::{
    CalendarAssignment, CalendarDocument, CalendarException, CalendarExceptionKind,
    CalendarWeekday, ExchangeCalendar, parse_local_time,
};
use super::read::{affected_assets, get_calendar};

const MAX_COVERAGE_DAYS: i64 = 366 * 50;

fn validate_mic(mic: &str) -> Result<String, DomainError> {
    let mic = mic.trim().to_ascii_uppercase();
    let valid = (3..=4).contains(&mic.len())
        && mic
            .chars()
            .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit());
    if !valid {
        return Err(DomainError::Validation(format!(
            "'{mic}' is not a valid 3- or 4-character venue code"
        )));
    }
    Ok(mic)
}

fn required_time(value: &str, field: &str) -> Result<NaiveTime, DomainError> {
    parse_local_time(value).ok_or_else(|| {
        DomainError::Validation(format!("{field} '{value}' is not a valid HH:MM time"))
    })
}

fn optional_time(value: Option<&str>, field: &str) -> Result<Option<NaiveTime>, DomainError> {
    match value {
        Some(value) => Ok(Some(required_time(value, field)?)),
        None => Ok(None),
    }
}

/// Validates one document and returns the calendar aggregate to store.
pub fn validate_document(mut document: CalendarDocument) -> Result<ExchangeCalendar, DomainError> {
    let mic = validate_mic(&document.mic)?;
    document.mic = mic.clone();
    if document.name.trim().is_empty() {
        return Err(DomainError::Validation(
            "calendar name must not be empty".to_string(),
        ));
    }
    if document.source.trim().is_empty() || document.provenance.trim().is_empty() {
        return Err(DomainError::Validation(
            "calendar source and provenance must not be empty".to_string(),
        ));
    }
    chrono_tz::Tz::from_str(document.time_zone.trim()).map_err(|_| {
        DomainError::Validation(format!(
            "'{}' is not a valid IANA time zone",
            document.time_zone
        ))
    })?;
    if document.coverage_from > document.coverage_to {
        return Err(DomainError::Validation(
            "calendar coverage start is later than its end".to_string(),
        ));
    }
    if (document.coverage_to - document.coverage_from).num_days() > MAX_COVERAGE_DAYS {
        return Err(DomainError::Validation(
            "calendar coverage exceeds the 50-year bound".to_string(),
        ));
    }
    if document.weekdays.is_empty() {
        return Err(DomainError::Validation(
            "calendar needs at least one weekly session".to_string(),
        ));
    }

    let mut weekday_seen = BTreeSet::new();
    let mut weekdays = Vec::with_capacity(document.weekdays.len());
    for weekday in &document.weekdays {
        if !(1..=7).contains(&weekday.iso_weekday) {
            return Err(DomainError::Validation(format!(
                "weekday {} is outside 1..=7",
                weekday.iso_weekday
            )));
        }
        if !weekday_seen.insert(weekday.iso_weekday) {
            return Err(DomainError::Validation(format!(
                "weekday {} is duplicated",
                weekday.iso_weekday
            )));
        }
        let open = required_time(&weekday.open, "weekday open")?;
        let close = required_time(&weekday.close, "weekday close")?;
        if open >= close {
            return Err(DomainError::Validation(format!(
                "weekday {} open time must be before its close time",
                weekday.iso_weekday
            )));
        }
        weekdays.push(CalendarWeekday {
            iso_weekday: weekday.iso_weekday,
            open_local: open,
            close_local: close,
        });
    }
    weekdays.sort_by_key(|weekday| weekday.iso_weekday);

    let mut exception_dates = BTreeSet::new();
    let mut exceptions = Vec::with_capacity(document.exceptions.len());
    for exception in &document.exceptions {
        if exception.date < document.coverage_from || exception.date > document.coverage_to {
            return Err(DomainError::Validation(format!(
                "exception {} is outside calendar coverage",
                exception.date
            )));
        }
        if !exception_dates.insert(exception.date) {
            return Err(DomainError::Validation(format!(
                "exception {} is duplicated",
                exception.date
            )));
        }
        let open = optional_time(exception.open.as_deref(), "exception open")?;
        let close = optional_time(exception.close.as_deref(), "exception close")?;
        match exception.kind {
            CalendarExceptionKind::Closure => {
                if open.is_some() || close.is_some() {
                    return Err(DomainError::Validation(format!(
                        "closure {} must not have times",
                        exception.date
                    )));
                }
            }
            CalendarExceptionKind::Session => {
                let (Some(open), Some(close)) = (open, close) else {
                    return Err(DomainError::Validation(format!(
                        "session {} needs open and close times",
                        exception.date
                    )));
                };
                if open >= close {
                    return Err(DomainError::Validation(format!(
                        "session {} open time must be before its close time",
                        exception.date
                    )));
                }
            }
            CalendarExceptionKind::EarlyClose => {
                let Some(close) = close else {
                    return Err(DomainError::Validation(format!(
                        "early close {} needs a close time",
                        exception.date
                    )));
                };
                if open.is_some_and(|open| open >= close) {
                    return Err(DomainError::Validation(format!(
                        "early close {} open time must be before its close time",
                        exception.date
                    )));
                }
            }
        }
        exceptions.push(CalendarException {
            date: exception.date,
            kind: exception.kind,
            open,
            close,
            reason: exception.reason.clone(),
            source: document.source.clone(),
        });
    }
    exceptions.sort_by_key(|exception| exception.date);

    Ok(ExchangeCalendar {
        mic,
        name: document.name.trim().to_string(),
        time_zone: document.time_zone.trim().to_string(),
        coverage_from: document.coverage_from,
        coverage_to: document.coverage_to,
        weekdays,
        exceptions,
        source: document.source.trim().to_string(),
        provenance: document.provenance.trim().to_string(),
        version: document.version.unwrap_or(1).max(1),
        active: document.active,
        updated_by: String::new(),
        updated_at: chrono::Utc::now(),
    })
}

async fn write_children(
    tx: &mut sqlx::PgTransaction<'_>,
    calendar: &ExchangeCalendar,
) -> Result<(), DomainError> {
    for weekday in &calendar.weekdays {
        sqlx::query(
            "INSERT INTO exchange_calendar_weekdays (mic, iso_weekday, open_local, close_local) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(&calendar.mic)
        .bind(weekday.iso_weekday)
        .bind(weekday.open_local)
        .bind(weekday.close_local)
        .execute(&mut **tx)
        .await?;
    }
    for exception in &calendar.exceptions {
        sqlx::query(
            "INSERT INTO exchange_calendar_exceptions \
             (id, mic, exception_date, kind, open_local, close_local, reason, source) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(Uuid::new_v4())
        .bind(&calendar.mic)
        .bind(exception.date)
        .bind(exception.kind.as_str())
        .bind(exception.open)
        .bind(exception.close)
        .bind(&exception.reason)
        .bind(&exception.source)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn audit(
    tx: &mut sqlx::PgTransaction<'_>,
    calendar: &ExchangeCalendar,
    action: &str,
    actor: &str,
    summary: &str,
) -> Result<(), DomainError> {
    let payload = serde_json::to_value(calendar)
        .map_err(|error| DomainError::Validation(format!("calendar audit payload: {error}")))?;
    sqlx::query(
        "INSERT INTO exchange_calendar_audit (id, mic, action, actor_id, summary, payload) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(&calendar.mic)
    .bind(action)
    .bind(actor)
    .bind(summary)
    .bind(payload)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Inserts one calendar only when its venue code is absent.
///
/// Returns `true` when the row was inserted. Existing operator-managed rows are
/// never overwritten.
pub async fn insert_calendar_if_absent(
    pool: &PgPool,
    document: CalendarDocument,
    actor: &str,
) -> Result<bool, DomainError> {
    let mut calendar = validate_document(document)?;
    calendar.updated_by = actor.to_string();
    calendar.updated_at = chrono::Utc::now();

    let mut tx = pool.begin().await?;
    let inserted = sqlx::query(
        "INSERT INTO exchange_calendars \
         (mic, name, time_zone, coverage_from, coverage_to, source, provenance, \
          version, active, updated_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, 1, $8, $9) \
         ON CONFLICT (mic) DO NOTHING",
    )
    .bind(&calendar.mic)
    .bind(&calendar.name)
    .bind(&calendar.time_zone)
    .bind(calendar.coverage_from)
    .bind(calendar.coverage_to)
    .bind(&calendar.source)
    .bind(&calendar.provenance)
    .bind(calendar.active)
    .bind(actor)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        > 0;
    if !inserted {
        tx.rollback().await?;
        return Ok(false);
    }
    write_children(&mut tx, &calendar).await?;
    audit(
        &mut tx,
        &calendar,
        "seeded",
        actor,
        &format!(
            "Seeded {} ({} weekdays, {} exceptions)",
            calendar.mic,
            calendar.weekdays.len(),
            calendar.exceptions.len()
        ),
    )
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// Creates or replaces one calendar atomically.
///
/// `expected_version` is the optimistic-concurrency guard used by the management
/// surface. A mismatch is a validation error for the caller to map to 409.
pub async fn replace_calendar(
    pool: &PgPool,
    document: CalendarDocument,
    expected_version: Option<i32>,
    actor: &str,
) -> Result<ExchangeCalendar, DomainError> {
    let mut calendar = validate_document(document)?;
    calendar.updated_by = actor.to_string();
    calendar.updated_at = chrono::Utc::now();
    if !calendar.active {
        return Err(DomainError::Validation(
            "deactivate a calendar through the retire action".to_string(),
        ));
    }

    let existing = get_calendar(pool, &calendar.mic).await?;
    if let Some(existing) = &existing {
        if let Some(expected) = expected_version
            && expected != existing.version
        {
            return Err(DomainError::invariant(
                "calendar_version_conflict",
                format!(
                    "calendar {} is at version {}; expected {}",
                    calendar.mic, existing.version, expected
                ),
            ));
        }
        if unchanged(existing, &calendar) {
            return Ok(existing.clone());
        }
        calendar.version = existing.version.saturating_add(1);
    } else if expected_version.is_some_and(|expected| expected != 0) {
        return Err(DomainError::invariant(
            "calendar_version_conflict",
            format!("calendar {} does not exist", calendar.mic),
        ));
    } else {
        calendar.version = 1;
    }

    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO exchange_calendars \
         (mic, name, time_zone, coverage_from, coverage_to, source, provenance, \
          version, active, updated_by, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, TRUE, $9, NOW()) \
         ON CONFLICT (mic) DO UPDATE SET \
           name = EXCLUDED.name, time_zone = EXCLUDED.time_zone, \
           coverage_from = EXCLUDED.coverage_from, coverage_to = EXCLUDED.coverage_to, \
           source = EXCLUDED.source, provenance = EXCLUDED.provenance, \
           version = EXCLUDED.version, active = TRUE, updated_by = EXCLUDED.updated_by, \
           updated_at = NOW()",
    )
    .bind(&calendar.mic)
    .bind(&calendar.name)
    .bind(&calendar.time_zone)
    .bind(calendar.coverage_from)
    .bind(calendar.coverage_to)
    .bind(&calendar.source)
    .bind(&calendar.provenance)
    .bind(calendar.version)
    .bind(actor)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM exchange_calendar_weekdays WHERE mic = $1")
        .bind(&calendar.mic)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM exchange_calendar_exceptions WHERE mic = $1")
        .bind(&calendar.mic)
        .execute(&mut *tx)
        .await?;
    write_children(&mut tx, &calendar).await?;
    audit(
        &mut tx,
        &calendar,
        "imported",
        actor,
        &format!(
            "Imported {} version {} ({} weekdays, {} exceptions)",
            calendar.mic,
            calendar.version,
            calendar.weekdays.len(),
            calendar.exceptions.len()
        ),
    )
    .await?;
    tx.commit().await?;
    get_calendar(pool, &calendar.mic)
        .await?
        .ok_or_else(|| DomainError::not_found("ExchangeCalendar", calendar.mic))
}

fn unchanged(existing: &ExchangeCalendar, next: &ExchangeCalendar) -> bool {
    existing.name == next.name
        && existing.time_zone == next.time_zone
        && existing.coverage_from == next.coverage_from
        && existing.coverage_to == next.coverage_to
        && existing.weekdays == next.weekdays
        && existing.exceptions == next.exceptions
        && existing.source == next.source
        && existing.provenance == next.provenance
        && existing.active == next.active
}

/// Creates one effective-dated assignment after checking for overlaps.
pub async fn create_assignment(
    pool: &PgPool,
    asset_id: Uuid,
    mic: &str,
    valid_from: chrono::NaiveDate,
    valid_to: Option<chrono::NaiveDate>,
    actor: &str,
) -> Result<CalendarAssignment, DomainError> {
    if valid_to.is_some_and(|to| to <= valid_from) {
        return Err(DomainError::Validation(
            "assignment end must be later than its start".to_string(),
        ));
    }
    let mic = mic.trim().to_ascii_uppercase();
    let calendar = get_calendar(pool, &mic)
        .await?
        .ok_or_else(|| DomainError::not_found("ExchangeCalendar", mic.clone()))?;
    if !calendar.active {
        return Err(DomainError::Validation(format!(
            "calendar {mic} is inactive; reactivate it before assigning instruments"
        )));
    }

    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
        .bind(asset_id)
        .execute(&mut *tx)
        .await?;
    let conflict: Option<(Uuid, chrono::NaiveDate, Option<chrono::NaiveDate>)> = sqlx::query_as(
        "SELECT id, valid_from, valid_to FROM asset_calendar_assignments \
         WHERE asset_id = $1 AND valid_from < $3 \
           AND (valid_to IS NULL OR valid_to > $2) LIMIT 1",
    )
    .bind(asset_id)
    .bind(valid_from)
    .bind(valid_to.unwrap_or(chrono::NaiveDate::MAX))
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((id, from, to)) = conflict {
        return Err(DomainError::invariant(
            "calendar_assignment_overlap",
            format!(
                "id={id};from={from};to={}",
                to.map(|to| to.to_string()).unwrap_or_default()
            ),
        ));
    }

    let assignment = CalendarAssignment {
        id: Uuid::new_v4(),
        asset_id,
        mic: mic.clone(),
        valid_from,
        valid_to,
        source: "operator".to_string(),
        created_by: actor.to_string(),
        created_at: chrono::Utc::now(),
    };
    sqlx::query(
        "INSERT INTO asset_calendar_assignments \
         (id, asset_id, mic, valid_from, valid_to, source, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(assignment.id)
    .bind(assignment.asset_id)
    .bind(&assignment.mic)
    .bind(assignment.valid_from)
    .bind(assignment.valid_to)
    .bind(&assignment.source)
    .bind(&assignment.created_by)
    .execute(&mut *tx)
    .await?;
    let payload = serde_json::json!({
        "assignment_id": assignment.id,
        "asset_id": assignment.asset_id,
        "mic": assignment.mic,
        "valid_from": assignment.valid_from,
        "valid_to": assignment.valid_to,
    });
    sqlx::query(
        "INSERT INTO exchange_calendar_audit (id, mic, action, actor_id, summary, payload) \
         VALUES ($1, $2, 'assigned', $3, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(&assignment.mic)
    .bind(actor)
    .bind(format!(
        "Assigned instrument {} to {} from {}",
        assignment.asset_id, assignment.mic, assignment.valid_from
    ))
    .bind(payload)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(assignment)
}

/// Removes one assignment and audits the removal.
pub async fn remove_assignment(pool: &PgPool, id: Uuid, actor: &str) -> Result<(), DomainError> {
    let mut tx = pool.begin().await?;
    let row: Option<(Uuid, String, chrono::NaiveDate, Option<chrono::NaiveDate>)> = sqlx::query_as(
        "DELETE FROM asset_calendar_assignments WHERE id = $1 \
             RETURNING asset_id, mic, valid_from, valid_to",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((asset_id, mic, from, to)) = row else {
        return Err(DomainError::not_found("CalendarAssignment", id.to_string()));
    };
    sqlx::query(
        "INSERT INTO exchange_calendar_audit (id, mic, action, actor_id, summary, payload) \
         VALUES ($1, $2, 'unassigned', $3, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(&mic)
    .bind(actor)
    .bind(format!(
        "Removed assignment {id} for instrument {asset_id} from {mic}"
    ))
    .bind(serde_json::json!({
        "assignment_id": id,
        "asset_id": asset_id,
        "mic": mic,
        "valid_from": from,
        "valid_to": to,
    }))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Retires one calendar after the caller confirms assigned instruments.
pub async fn retire_calendar(
    pool: &PgPool,
    mic: &str,
    actor: &str,
    confirm: bool,
) -> Result<ExchangeCalendar, DomainError> {
    let mic = mic.trim().to_ascii_uppercase();
    get_calendar(pool, &mic)
        .await?
        .ok_or_else(|| DomainError::not_found("ExchangeCalendar", mic.clone()))?;
    let in_use = affected_assets(pool, &mic, chrono::Utc::now().date_naive()).await?;
    if in_use.affected_count > 0 && !confirm {
        return Err(DomainError::invariant(
            "calendar_in_use",
            format!("affected_count={}", in_use.affected_count),
        ));
    }

    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE exchange_calendars SET active = FALSE, updated_by = $2, updated_at = NOW() WHERE mic = $1")
        .bind(&mic)
        .bind(actor)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO exchange_calendar_audit (id, mic, action, actor_id, summary, payload) \
         VALUES ($1, $2, 'retired', $3, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(&mic)
    .bind(actor)
    .bind(format!(
        "Retired {mic}; {} instrument(s) fall back to the weekday estimate",
        in_use.affected_count
    ))
    .bind(serde_json::json!({
        "affected_count": in_use.affected_count,
        "confirmed": confirm,
    }))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    get_calendar(pool, &mic)
        .await?
        .ok_or_else(|| DomainError::not_found("ExchangeCalendar", mic))
}

/// Renders a stored calendar back into the importable document shape.
pub fn export_document(calendar: &ExchangeCalendar) -> CalendarDocument {
    CalendarDocument {
        mic: calendar.mic.clone(),
        name: calendar.name.clone(),
        time_zone: calendar.time_zone.clone(),
        coverage_from: calendar.coverage_from,
        coverage_to: calendar.coverage_to,
        weekdays: calendar
            .weekdays
            .iter()
            .map(|weekday| super::entity::CalendarWeekdayDocument {
                iso_weekday: weekday.iso_weekday,
                open: super::entity::format_local_time(weekday.open_local),
                close: super::entity::format_local_time(weekday.close_local),
            })
            .collect(),
        exceptions: calendar
            .exceptions
            .iter()
            .map(|exception| super::entity::CalendarExceptionDocument {
                date: exception.date,
                kind: exception.kind,
                open: exception.open.map(super::entity::format_local_time),
                close: exception.close.map(super::entity::format_local_time),
                reason: exception.reason.clone(),
            })
            .collect(),
        source: calendar.source.clone(),
        provenance: calendar.provenance.clone(),
        version: Some(calendar.version),
        active: calendar.active,
    }
}
