//! Calendar-aware asset coverage.

use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::{AssetId, DomainId};

use super::entity::AssetCoverage;
use super::read::stored_daily_dates;
use super::resolve::{calendars_for_asset, resolve_for_asset, resolve_timeline};
use super::sessions::{current_session_for_segments, expected_sessions_for_segments};

/// Computes the calendar-aware coverage for one asset and inclusive date range.
///
/// The range's `expected_session_count` and `missing_sessions` describe the
/// requested range. `latest_completed_session` is independent of the range end
/// and is computed against the current date for freshness.
pub async fn asset_coverage(
    pool: &PgPool,
    asset_id: AssetId,
    from: chrono::NaiveDate,
    to: chrono::NaiveDate,
    now: DateTime<Utc>,
) -> Result<AssetCoverage, DomainError> {
    if from > to {
        return Err(DomainError::Validation(
            "coverage start is later than its end".to_string(),
        ));
    }
    let segments = resolve_for_asset(pool, asset_id, from, to).await?;
    let calendars = calendars_for_asset(pool, asset_id, from, to).await?;
    let expected = expected_sessions_for_segments(&segments, &calendars, now);
    let stored = stored_daily_dates(pool, asset_id.as_uuid(), from, to).await?;
    let missing = expected
        .iter()
        .copied()
        .filter(|date| !stored.contains(date))
        .collect::<Vec<_>>();
    let latest_completed_session = latest_completed_session(pool, asset_id, now).await?;

    Ok(AssetCoverage {
        asset_id: asset_id.as_uuid(),
        from,
        to,
        as_of: now,
        segments,
        expected_session_count: expected.len(),
        missing_sessions: missing,
        latest_completed_session,
    })
}

/// Resolves the latest completed session and the current session candidate.
///
/// `current_session_candidate` is `Some(today)` when a session is scheduled
/// for today even if it has not closed. It never counts as a completed session.
/// Implements FR-015 / FR-020 of spec 035.
pub async fn session_state(
    pool: &PgPool,
    asset_id: AssetId,
    now: DateTime<Utc>,
) -> Result<(Option<chrono::NaiveDate>, Option<chrono::NaiveDate>), DomainError> {
    let today = now.date_naive();
    let from = today
        .checked_sub_signed(Duration::days(366))
        .ok_or_else(|| DomainError::Validation("date range underflow".to_string()))?;
    let (segments, calendars) = resolve_timeline(pool, asset_id, from, today).await?;
    let sessions = expected_sessions_for_segments(&segments, &calendars, now);
    let current = current_session_for_segments(&segments, &calendars, today);
    Ok((sessions.last().copied(), current))
}

/// Finds the latest completed expected session on or before `now`.
///
/// The walk is bounded to one year plus a day; a longer continuous closure is
/// outside the supported calendar model.
pub async fn latest_completed_session(
    pool: &PgPool,
    asset_id: AssetId,
    now: DateTime<Utc>,
) -> Result<Option<chrono::NaiveDate>, DomainError> {
    Ok(session_state(pool, asset_id, now).await?.0)
}

/// Returns today when a session is scheduled for it, even before it closes.
/// Implements FR-015 of spec 035.
pub async fn current_session_candidate(
    pool: &PgPool,
    asset_id: AssetId,
    now: DateTime<Utc>,
) -> Result<Option<chrono::NaiveDate>, DomainError> {
    Ok(session_state(pool, asset_id, now).await?.1)
}
