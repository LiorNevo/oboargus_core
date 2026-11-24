//! Per-date venue resolution and coalesced calendar segments.

use std::collections::{BTreeMap, BTreeSet};

use chrono::NaiveDate;
use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::AssetId;
use crate::market_data::identifier_mapping::ticker_venue;

use super::entity::{CalendarAssignment, CalendarSegment, ExchangeCalendar, SegmentReason};
use super::read::{get_calendar, list_assignments_for_asset};

/// Reads the distinct venue codes named by an asset's ticker identifiers.
pub async fn asset_ticker_mics(
    pool: &PgPool,
    asset_id: AssetId,
) -> Result<Vec<String>, DomainError> {
    let identifiers =
        crate::domain::asset_identifier::read::list_asset_identifiers(pool, asset_id).await?;
    let mut mics = BTreeSet::new();
    for identifier in identifiers {
        if let Some(Some(venue)) = ticker_venue(&identifier.scheme) {
            let venue = venue.trim().to_ascii_uppercase();
            if !venue.is_empty() {
                mics.insert(venue);
            }
        }
    }
    Ok(mics.into_iter().collect())
}

async fn calendar_map(
    pool: &PgPool,
    assignments: &[CalendarAssignment],
    ticker_mics: &[String],
) -> Result<BTreeMap<String, ExchangeCalendar>, DomainError> {
    let mut mics = BTreeSet::new();
    for assignment in assignments {
        mics.insert(assignment.mic.clone());
    }
    for mic in ticker_mics {
        mics.insert(mic.clone());
    }
    let mut calendars = BTreeMap::new();
    for mic in mics {
        if let Some(calendar) = get_calendar(pool, &mic).await? {
            calendars.insert(mic, calendar);
        }
    }
    Ok(calendars)
}

fn assignment_at(
    assignments: &[CalendarAssignment],
    date: NaiveDate,
) -> Option<&CalendarAssignment> {
    assignments
        .iter()
        .filter(|assignment| {
            assignment.valid_from <= date
                && assignment.valid_to.is_none_or(|valid_to| date < valid_to)
        })
        .max_by_key(|assignment| assignment.valid_from)
}

fn calendar_resolution(
    mic: Option<&str>,
    date: NaiveDate,
    calendars: &BTreeMap<String, ExchangeCalendar>,
) -> (Option<String>, Option<String>, bool, Option<SegmentReason>) {
    let Some(mic) = mic else {
        return (None, None, false, Some(SegmentReason::NoVenue));
    };
    let Some(calendar) = calendars.get(mic) else {
        return (None, None, false, Some(SegmentReason::NoCalendar));
    };
    if !calendar.active {
        return (None, None, false, Some(SegmentReason::InactiveCalendar));
    }
    if date < calendar.coverage_from || date > calendar.coverage_to {
        return (None, None, false, Some(SegmentReason::OutsideCoverage));
    }
    (
        Some(calendar.mic.clone()),
        Some(calendar.name.clone()),
        true,
        None,
    )
}

/// Resolves a date range to coalesced calendar segments.
///
/// Explicit effective-dated assignments win. Otherwise a unique
/// venue-qualified ticker scheme is used. Ambiguous or absent venues fall back
/// to the weekday approximation and disclose why.
pub fn resolve_segments(
    from: NaiveDate,
    to: NaiveDate,
    assignments: &[CalendarAssignment],
    ticker_mics: &[String],
    calendars: &BTreeMap<String, ExchangeCalendar>,
) -> Vec<CalendarSegment> {
    if from > to {
        return Vec::new();
    }
    let unique_ticker_mic = if ticker_mics.len() == 1 {
        ticker_mics.first().map(String::as_str)
    } else {
        None
    };
    let ambiguous = ticker_mics.len() > 1;

    let mut segments: Vec<CalendarSegment> = Vec::new();
    let mut current = from;
    loop {
        let explicit = assignment_at(assignments, current);
        let (mic, name, exact, reason) = if let Some(assignment) = explicit {
            calendar_resolution(Some(&assignment.mic), current, calendars)
        } else if ambiguous {
            (None, None, false, Some(SegmentReason::AmbiguousVenue))
        } else {
            calendar_resolution(unique_ticker_mic, current, calendars)
        };

        let extend = segments.last().is_some_and(|last| {
            last.mic == mic
                && last.exact == exact
                && last.reason == reason
                && last.to.succ_opt() == Some(current)
        });
        if extend {
            if let Some(last) = segments.last_mut() {
                last.to = current;
            }
        } else {
            segments.push(CalendarSegment {
                mic,
                name,
                from: current,
                to: current,
                exact,
                reason,
            });
        }

        if current >= to {
            break;
        }
        let Some(next) = current.succ_opt() else {
            break;
        };
        current = next;
    }
    segments
}

/// Resolves an asset's calendar segments and the calendars they reference.
pub async fn resolve_timeline(
    pool: &PgPool,
    asset_id: AssetId,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<(Vec<CalendarSegment>, BTreeMap<String, ExchangeCalendar>), DomainError> {
    let assignments = list_assignments_for_asset(pool, asset_id, from, to).await?;
    let ticker_mics = asset_ticker_mics(pool, asset_id).await?;
    let calendars = calendar_map(pool, &assignments, &ticker_mics).await?;
    let segments = resolve_segments(from, to, &assignments, &ticker_mics, &calendars);
    Ok((segments, calendars))
}

/// Resolves an asset's calendar segments for `[from, to]`.
pub async fn resolve_for_asset(
    pool: &PgPool,
    asset_id: AssetId,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<CalendarSegment>, DomainError> {
    Ok(resolve_timeline(pool, asset_id, from, to).await?.0)
}

/// Loads the calendars referenced by an asset's assignments and tickers.
pub async fn calendars_for_asset(
    pool: &PgPool,
    asset_id: AssetId,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<BTreeMap<String, ExchangeCalendar>, DomainError> {
    let assignments = list_assignments_for_asset(pool, asset_id, from, to).await?;
    let ticker_mics = asset_ticker_mics(pool, asset_id).await?;
    calendar_map(pool, &assignments, &ticker_mics).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market_data::calendar::entity::{
        CalendarAssignment, CalendarWeekday, ExchangeCalendar, SegmentReason,
    };

    fn date(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("date")
    }

    fn calendar(mic: &str, active: bool) -> ExchangeCalendar {
        ExchangeCalendar {
            mic: mic.into(),
            name: format!("{mic} venue"),
            time_zone: "UTC".into(),
            coverage_from: date("2026-01-01"),
            coverage_to: date("2026-12-31"),
            weekdays: (1..=5)
                .map(|iso_weekday| CalendarWeekday {
                    iso_weekday,
                    open_local: chrono::NaiveTime::from_hms_opt(9, 30, 0).expect("time"),
                    close_local: chrono::NaiveTime::from_hms_opt(16, 0, 0).expect("time"),
                })
                .collect(),
            exceptions: Vec::new(),
            source: "test".into(),
            provenance: "test".into(),
            version: 1,
            active,
            updated_by: "test".into(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn explicit_effective_assignment_wins() {
        let calendar = calendar("XNYS", true);
        let calendars = BTreeMap::from([("XNYS".to_string(), calendar)]);
        let assignments = vec![CalendarAssignment {
            id: uuid::Uuid::new_v4(),
            asset_id: uuid::Uuid::new_v4(),
            mic: "XNYS".into(),
            valid_from: date("2026-01-01"),
            valid_to: None,
            source: "test".into(),
            created_by: "test".into(),
            created_at: chrono::Utc::now(),
        }];
        let segments = resolve_segments(
            date("2026-11-24"),
            date("2026-11-24"),
            &assignments,
            &["XNAS".into()],
            &calendars,
        );
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].mic.as_deref(), Some("XNYS"));
        assert!(segments[0].exact);
    }

    #[test]
    fn a_unique_ticker_venue_resolves_exactly() {
        let calendars = BTreeMap::from([("XNAS".to_string(), calendar("XNAS", true))]);
        let segments = resolve_segments(
            date("2026-11-24"),
            date("2026-11-24"),
            &[],
            &["XNAS".into()],
            &calendars,
        );
        assert_eq!(segments[0].mic.as_deref(), Some("XNAS"));
        assert!(segments[0].exact);
    }

    #[test]
    fn ambiguity_and_missing_venues_fall_back() {
        let calendars = BTreeMap::new();
        let ambiguity = resolve_segments(
            date("2026-11-24"),
            date("2026-11-24"),
            &[],
            &["XNAS".into(), "XNYS".into()],
            &calendars,
        );
        assert_eq!(ambiguity[0].reason, Some(SegmentReason::AmbiguousVenue));
        let no_venue =
            resolve_segments(date("2026-11-24"), date("2026-11-24"), &[], &[], &calendars);
        assert_eq!(no_venue[0].reason, Some(SegmentReason::NoVenue));
    }

    #[test]
    fn inactive_and_outside_coverage_calendars_fall_back() {
        let calendars = BTreeMap::from([
            ("XNAS".to_string(), calendar("XNAS", false)),
            ("XNYS".to_string(), calendar("XNYS", true)),
        ]);
        let inactive = resolve_segments(
            date("2026-11-24"),
            date("2026-11-24"),
            &[],
            &["XNAS".into()],
            &calendars,
        );
        assert_eq!(inactive[0].reason, Some(SegmentReason::InactiveCalendar));
        let outside = resolve_segments(
            date("2027-01-04"),
            date("2027-01-04"),
            &[],
            &["XNYS".into()],
            &calendars,
        );
        assert_eq!(outside[0].reason, Some(SegmentReason::OutsideCoverage));
    }
}
