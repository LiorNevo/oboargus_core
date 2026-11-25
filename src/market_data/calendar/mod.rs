//! Exchange-calendar reference data and calendar-aware coverage.
//!
//! Calendars describe venue sessions and dated exceptions. Effective-dated
//! asset assignments and venue-qualified ticker schemes resolve an instrument's
//! calendar per date. Expected sessions, missing sessions, and freshness are
//! computed here and reused by the chart coverage read, loaders, and refresh.

#![forbid(unsafe_code)]

pub mod cache;
pub mod coverage;
pub mod entity;
pub mod read;
pub mod resolve;
pub mod seed;
pub mod sessions;
pub mod write;

pub use cache::{CalendarCache, asset_closed_on};
pub use coverage::{
    asset_coverage, current_session_candidate, latest_completed_session, session_state,
};
pub use entity::{
    AffectedAsset, AssetCoverage, CalendarAssignment, CalendarAssignmentPage,
    CalendarAssignmentView, CalendarAuditEntry, CalendarDetail, CalendarDocument,
    CalendarException, CalendarExceptionDocument, CalendarExceptionKind, CalendarInUse,
    CalendarSeedBundle, CalendarSegment, CalendarSummary, CalendarWeekday, CalendarWeekdayDocument,
    ExchangeCalendar, SegmentReason,
};
pub use read::{
    affected_assets, count_assignments_for_calendar, get_calendar, get_calendar_detail,
    list_assignment_views, list_assignments_for_asset, list_assignments_for_calendar,
    list_calendar_audit, list_calendar_summaries, list_calendars, stored_daily_dates,
};
pub use resolve::{
    asset_ticker_mics, calendars_for_asset, resolve_for_asset, resolve_segments, resolve_timeline,
};
pub use seed::{parse_bundled_seed, seed_bundled_calendars};
pub use sessions::{
    current_session_for_segments, expected_sessions, expected_sessions_for_segments, is_trading_day,
};
pub use write::{
    create_assignment, export_document, insert_calendar_if_absent, remove_assignment,
    replace_calendar, retire_calendar, validate_document,
};
