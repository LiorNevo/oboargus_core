//! Expected-session calculation.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use chrono::{DateTime, Datelike, LocalResult, NaiveDate, NaiveTime, TimeZone, Timelike, Utc};
use chrono_tz::Tz;

use super::entity::{
    CalendarException, CalendarExceptionKind, CalendarSegment, CalendarWeekday, ExchangeCalendar,
};

fn date_iter(from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
    let mut dates = Vec::new();
    let mut current = from;
    loop {
        dates.push(current);
        if current >= to {
            break;
        }
        let Some(next) = current.succ_opt() else {
            break;
        };
        current = next;
    }
    dates
}

fn weekday_for(date: NaiveDate) -> i16 {
    date.weekday().num_days_from_monday() as i16 + 1
}

fn exception_for(exceptions: &[CalendarException], date: NaiveDate) -> Option<&CalendarException> {
    exceptions.iter().find(|exception| exception.date == date)
}

fn regular_weekday(weekdays: &[CalendarWeekday], date: NaiveDate) -> Option<&CalendarWeekday> {
    weekdays
        .iter()
        .find(|weekday| weekday.iso_weekday == weekday_for(date))
}

fn session_times(calendar: &ExchangeCalendar, date: NaiveDate) -> Option<(NaiveTime, NaiveTime)> {
    if date < calendar.coverage_from || date > calendar.coverage_to {
        return None;
    }
    if let Some(exception) = exception_for(&calendar.exceptions, date) {
        match exception.kind {
            CalendarExceptionKind::Closure => return None,
            CalendarExceptionKind::Session => {
                return Some((exception.open?, exception.close?));
            }
            CalendarExceptionKind::EarlyClose => {
                let regular = regular_weekday(&calendar.weekdays, date);
                let open = exception
                    .open
                    .or(regular.map(|weekday| weekday.open_local))?;
                return exception.close.map(|close| (open, close));
            }
        }
    }
    regular_weekday(&calendar.weekdays, date)
        .map(|weekday| (weekday.open_local, weekday.close_local))
}

fn completed(time_zone: &Tz, date: NaiveDate, close: NaiveTime, now: DateTime<Utc>) -> bool {
    let local = time_zone.with_ymd_and_hms(
        date.year(),
        date.month(),
        date.day(),
        close.hour(),
        close.minute(),
        close.second(),
    );
    let close_utc = match local {
        LocalResult::Single(value) => value.with_timezone(&Utc),
        LocalResult::Ambiguous(earliest, _) => earliest.with_timezone(&Utc),
        LocalResult::None => return false,
    };
    close_utc <= now
}

/// Whether `date` has a session on this exact calendar, ignoring whether the
/// session has closed yet.
pub fn is_trading_day(calendar: &ExchangeCalendar, date: NaiveDate) -> bool {
    calendar.active && session_times(calendar, date).is_some()
}

/// Returns the completed expected sessions for one exact calendar.
pub fn expected_sessions(
    calendar: &ExchangeCalendar,
    from: NaiveDate,
    to: NaiveDate,
    now: DateTime<Utc>,
) -> Vec<NaiveDate> {
    if !calendar.active || from > to {
        return Vec::new();
    }
    let Ok(time_zone) = Tz::from_str(&calendar.time_zone) else {
        return Vec::new();
    };
    date_iter(from, to)
        .into_iter()
        .filter(|date| {
            let Some((_, close)) = session_times(calendar, *date) else {
                return false;
            };
            completed(&time_zone, *date, close, now)
        })
        .collect()
}

fn fallback_sessions(from: NaiveDate, to: NaiveDate, now: DateTime<Utc>) -> Vec<NaiveDate> {
    let today = now.date_naive();
    date_iter(from, to)
        .into_iter()
        .filter(|date| {
            let weekday = date.weekday().number_from_monday();
            weekday <= 5 && *date < today
        })
        .collect()
}

/// Returns the completed expected sessions across a resolved segment list.
pub fn expected_sessions_for_segments(
    segments: &[CalendarSegment],
    calendars: &BTreeMap<String, ExchangeCalendar>,
    now: DateTime<Utc>,
) -> Vec<NaiveDate> {
    let mut sessions = BTreeSet::new();
    for segment in segments {
        if segment.exact
            && let Some(mic) = &segment.mic
            && let Some(calendar) = calendars.get(mic)
        {
            sessions.extend(expected_sessions(calendar, segment.from, segment.to, now));
            continue;
        }
        sessions.extend(fallback_sessions(segment.from, segment.to, now));
    }
    sessions.into_iter().collect()
}

/// Returns `date` when a session is scheduled for it, whether or not it has
/// opened or closed yet.
///
/// Exact segments use the calendar; non-exact segments keep the weekday
/// approximation. This is used to allow loading and refreshing the current
/// session's in-progress daily bar without treating the date as a completed
/// expected session. Implements FR-015 of spec 035.
pub fn current_session_for_segments(
    segments: &[CalendarSegment],
    calendars: &BTreeMap<String, ExchangeCalendar>,
    date: NaiveDate,
) -> Option<NaiveDate> {
    for segment in segments {
        if date < segment.from || date > segment.to {
            continue;
        }
        if segment.exact
            && let Some(mic) = &segment.mic
            && let Some(calendar) = calendars.get(mic)
        {
            if is_trading_day(calendar, date) {
                return Some(date);
            }
            continue;
        }
        if date.weekday().number_from_monday() <= 5 {
            return Some(date);
        }
    }
    None
}

/// Returns the completed session dates for a calendar in one range.
pub fn sessions_for_calendar(
    calendar: &ExchangeCalendar,
    from: NaiveDate,
    to: NaiveDate,
    now: DateTime<Utc>,
) -> Vec<NaiveDate> {
    expected_sessions(calendar, from, to, now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market_data::calendar::entity::{
        CalendarException, CalendarExceptionKind, CalendarWeekday, ExchangeCalendar, SegmentReason,
    };

    fn calendar(exceptions: Vec<CalendarException>) -> ExchangeCalendar {
        ExchangeCalendar {
            mic: "XNAS".into(),
            name: "Test venue".into(),
            time_zone: "UTC".into(),
            coverage_from: NaiveDate::from_ymd_opt(2026, 1, 1).expect("date"),
            coverage_to: NaiveDate::from_ymd_opt(2026, 12, 31).expect("date"),
            weekdays: (1..=5)
                .map(|iso_weekday| CalendarWeekday {
                    iso_weekday,
                    open_local: NaiveTime::from_hms_opt(9, 30, 0).expect("time"),
                    close_local: NaiveTime::from_hms_opt(16, 0, 0).expect("time"),
                })
                .collect(),
            exceptions,
            source: "test".into(),
            provenance: "test".into(),
            version: 1,
            active: true,
            updated_by: "test".into(),
            updated_at: Utc::now(),
        }
    }

    fn date(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("date")
    }

    fn now(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .expect("timestamp")
            .with_timezone(&Utc)
    }

    #[test]
    fn weekends_and_future_sessions_are_not_expected() {
        let sessions = expected_sessions(
            &calendar(Vec::new()),
            date("2026-11-20"),
            date("2026-11-24"),
            now("2026-11-24T17:00:00Z"),
        );
        assert_eq!(
            sessions,
            vec![date("2026-11-20"), date("2026-11-23"), date("2026-11-24")]
        );
    }

    #[test]
    fn current_session_is_available_before_its_close() {
        let calendar = calendar(Vec::new());
        let session_date = date("2026-11-25");
        let before_close = now("2026-11-25T15:00:00Z");
        assert!(expected_sessions(&calendar, session_date, session_date, before_close).is_empty());
        let segments = vec![CalendarSegment {
            mic: Some("XNAS".into()),
            name: Some("Test venue".into()),
            from: session_date,
            to: session_date,
            exact: true,
            reason: None,
        }];
        let calendars = BTreeMap::from([("XNAS".to_string(), calendar)]);
        assert_eq!(
            current_session_for_segments(&segments, &calendars, session_date),
            Some(session_date)
        );
    }

    #[test]
    fn current_session_is_not_available_on_a_weekend() {
        let session_date = date("2026-11-28");
        let segments = vec![CalendarSegment {
            mic: None,
            name: None,
            from: session_date,
            to: session_date,
            exact: false,
            reason: Some(SegmentReason::NoVenue),
        }];
        assert_eq!(
            current_session_for_segments(&segments, &BTreeMap::new(), session_date),
            None
        );
    }

    #[test]
    fn wrappers_and_invalid_calendars_fall_back_to_nothing() {
        let calendar = calendar(Vec::new());
        assert_eq!(
            sessions_for_calendar(
                &calendar,
                date("2026-11-20"),
                date("2026-11-20"),
                now("2026-11-24T17:00:00Z"),
            ),
            vec![date("2026-11-20")]
        );
        let mut invalid_zone = calendar.clone();
        invalid_zone.time_zone = "Not/AZone".into();
        assert!(
            expected_sessions(
                &invalid_zone,
                date("2026-11-20"),
                date("2026-11-20"),
                now("2026-11-24T17:00:00Z"),
            )
            .is_empty()
        );
        let mut inactive = calendar.clone();
        inactive.active = false;
        assert!(
            expected_sessions(
                &inactive,
                date("2026-11-20"),
                date("2026-11-20"),
                now("2026-11-24T17:00:00Z"),
            )
            .is_empty()
        );
        assert!(
            expected_sessions(
                &calendar,
                date("2026-11-24"),
                date("2026-11-20"),
                now("2026-11-24T17:00:00Z"),
            )
            .is_empty()
        );
    }

    #[test]
    fn a_session_is_not_expected_before_its_close() {
        let sessions = expected_sessions(
            &calendar(Vec::new()),
            date("2026-11-23"),
            date("2026-11-24"),
            now("2026-11-24T15:00:00Z"),
        );
        assert_eq!(sessions, vec![date("2026-11-23")]);
    }

    #[test]
    fn closures_extra_sessions_and_early_closes_are_applied() {
        let calendar = calendar(vec![
            CalendarException {
                date: date("2026-11-26"),
                kind: CalendarExceptionKind::Closure,
                open: None,
                close: None,
                reason: Some("holiday".into()),
                source: "test".into(),
            },
            CalendarException {
                date: date("2026-11-28"),
                kind: CalendarExceptionKind::Session,
                open: Some(NaiveTime::from_hms_opt(9, 30, 0).expect("time")),
                close: Some(NaiveTime::from_hms_opt(13, 0, 0).expect("time")),
                reason: Some("extra".into()),
                source: "test".into(),
            },
            CalendarException {
                date: date("2026-11-27"),
                kind: CalendarExceptionKind::EarlyClose,
                open: None,
                close: Some(NaiveTime::from_hms_opt(13, 0, 0).expect("time")),
                reason: Some("early".into()),
                source: "test".into(),
            },
        ]);
        let sessions = expected_sessions(
            &calendar,
            date("2026-11-25"),
            date("2026-11-28"),
            now("2026-11-28T14:00:00Z"),
        );
        assert_eq!(
            sessions,
            vec![date("2026-11-25"), date("2026-11-27"), date("2026-11-28")]
        );
        assert!(!sessions.contains(&date("2026-11-26")));
    }

    #[test]
    fn an_approximate_segment_uses_completed_weekdays() {
        let segments = vec![CalendarSegment {
            mic: None,
            name: None,
            from: date("2026-11-20"),
            to: date("2026-11-24"),
            exact: false,
            reason: Some(crate::market_data::calendar::entity::SegmentReason::NoVenue),
        }];
        let sessions = expected_sessions_for_segments(
            &segments,
            &BTreeMap::new(),
            now("2026-11-24T17:00:00Z"),
        );
        assert_eq!(sessions, vec![date("2026-11-20"), date("2026-11-23")]);
    }
}
