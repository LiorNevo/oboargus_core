//! Dataset-level checks for the bundled US-equity calendar seed.
//!
//! These tests read the committed JSON only; they never touch a database.

use chrono::NaiveDate;

use obolargus_core::market_data::calendar::CalendarExceptionKind;
use obolargus_core::market_data::calendar::parse_bundled_seed;

fn date(value: &str) -> NaiveDate {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("date")
}

#[test]
fn seed_covers_the_six_initial_venues_for_thirteen_years() {
    let bundle = parse_bundled_seed().expect("seed");
    let mics: Vec<_> = bundle
        .calendars
        .iter()
        .map(|calendar| calendar.mic.as_str())
        .collect();
    assert_eq!(mics, vec!["XNAS", "XNYS", "ARCX", "XASE", "BATS", "OTC"]);
    for calendar in &bundle.calendars {
        assert_eq!(calendar.time_zone, "America/New_York");
        assert_eq!(calendar.coverage_from, date("2016-01-01"));
        assert_eq!(calendar.coverage_to, date("2028-12-31"));
        assert!(
            calendar
                .weekdays
                .iter()
                .all(|weekday| weekday.open == "09:30" && weekday.close == "16:00")
        );
        for exception in &calendar.exceptions {
            assert!(
                exception.date >= calendar.coverage_from && exception.date <= calendar.coverage_to,
                "{} exception outside coverage: {}",
                calendar.mic,
                exception.date
            );
            match exception.kind {
                CalendarExceptionKind::Closure => {
                    assert!(exception.open.is_none() && exception.close.is_none());
                }
                CalendarExceptionKind::Session => {
                    assert!(exception.open.is_some() && exception.close.is_some());
                }
                CalendarExceptionKind::EarlyClose => {
                    assert!(exception.close.is_some());
                }
            }
        }
    }
}

#[test]
fn seed_records_known_full_day_closures() {
    let bundle = parse_bundled_seed().expect("seed");
    for calendar in &bundle.calendars {
        let closures: Vec<_> = calendar
            .exceptions
            .iter()
            .filter(|exception| exception.kind == CalendarExceptionKind::Closure)
            .map(|exception| exception.date)
            .collect();
        for closed in [
            "2016-01-01",
            "2018-12-05",
            "2021-01-01",
            "2025-01-09",
            "2026-11-26",
            "2027-07-05",
            "2028-12-25",
        ] {
            assert!(
                closures.contains(&date(closed)),
                "{} is missing closure {closed}",
                calendar.mic
            );
        }
        for open in ["2021-12-31", "2027-12-31"] {
            assert!(
                !closures.contains(&date(open)),
                "{} must not close for New Year's Day on {open}",
                calendar.mic
            );
        }
    }
}

#[test]
fn seed_records_known_early_closes_and_no_phantom_sessions() {
    let bundle = parse_bundled_seed().expect("seed");
    for calendar in &bundle.calendars {
        let early: Vec<_> = calendar
            .exceptions
            .iter()
            .filter(|exception| exception.kind == CalendarExceptionKind::EarlyClose)
            .map(|exception| (exception.date, exception.close.as_deref()))
            .collect();
        for (closed, expected) in [
            ("2016-11-25", "13:00"),
            ("2026-11-27", "13:00"),
            ("2028-07-03", "13:00"),
        ] {
            assert!(
                early.contains(&(date(closed), Some(expected))),
                "{} is missing early close {closed}",
                calendar.mic
            );
        }
        assert!(
            !calendar
                .exceptions
                .iter()
                .any(|exception| exception.kind == CalendarExceptionKind::Session),
            "{} has an undocumented extraordinary session",
            calendar.mic
        );
    }
}

#[test]
fn deterministic_fixture_covers_all_exception_kinds() {
    let document: obolargus_core::market_data::calendar::CalendarDocument =
        serde_json::from_str(include_str!("fixtures/exchange_calendars.json")).expect("fixture");
    let calendar =
        obolargus_core::market_data::calendar::validate_document(document).expect("valid");
    assert_eq!(calendar.mic, "XTST");
    assert_eq!(calendar.weekdays.len(), 5);
    assert_eq!(calendar.exceptions.len(), 3);
    assert!(
        calendar
            .exceptions
            .iter()
            .any(|exception| { exception.kind == CalendarExceptionKind::Session })
    );
    assert!(calendar.exceptions.iter().any(|exception| {
        exception.kind == CalendarExceptionKind::EarlyClose
            && exception.close == chrono::NaiveTime::from_hms_opt(13, 0, 0)
    }));
}
