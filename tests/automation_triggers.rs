//! Scheduler helper coverage for interval and cron triggers.

use chrono::{TimeZone, Utc};

use obolargus_core::automation::error::AutomationError;
use obolargus_core::automation::triggers::{next_cron, next_interval, next_occurrence};

fn at() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5)
        .single()
        .expect("date")
}

#[test]
fn interval_schedules_reject_zero_and_overflow() {
    let error = next_interval(at(), 0).expect_err("zero");
    assert!(matches!(error, AutomationError::Validation(_)));

    let error = next_interval(at(), u64::MAX).expect_err("overflow");
    assert!(matches!(error, AutomationError::Validation(_)));

    assert_eq!(
        next_interval(at(), 90).expect("interval"),
        at() + chrono::Duration::seconds(90)
    );
}

#[test]
fn cron_schedules_validate_expressions_and_timezones() {
    let next = next_cron("0 * * * * *", "UTC", at()).expect("cron");
    assert!(next > at());

    let error = next_cron("0 * * * * *", "Mars/Olympus", at()).expect_err("timezone");
    assert!(matches!(error, AutomationError::Validation(_)));

    let error = next_cron("not a cron", "UTC", at()).expect_err("expression");
    assert!(matches!(error, AutomationError::Validation(_)));
}

#[test]
fn next_occurrence_covers_every_repeat_kind() {
    let interval = next_occurrence("interval", Some(30), None, "UTC", at())
        .expect("interval")
        .expect("some interval");
    assert_eq!(interval, at() + chrono::Duration::seconds(30));

    let missing_interval =
        next_occurrence("interval", None, None, "UTC", at()).expect_err("missing");
    assert!(matches!(missing_interval, AutomationError::Validation(_)));

    let cron = next_occurrence("cron", None, Some("0 * * * * *"), "UTC", at())
        .expect("cron")
        .expect("some cron");
    assert!(cron > at());

    let missing_cron = next_occurrence("cron", None, None, "UTC", at()).expect_err("missing cron");
    assert!(matches!(missing_cron, AutomationError::Validation(_)));

    let unknown = next_occurrence("hourly", None, None, "UTC", at()).expect_err("unknown");
    assert!(matches!(unknown, AutomationError::Validation(_)));
}
