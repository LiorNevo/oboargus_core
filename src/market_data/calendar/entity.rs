//! Exchange-calendar reference data and coverage types.
//!
//! Calendars describe venues, not assets. A calendar is keyed by the venue code
//! from the identifier grammar (normally an ISO 10383 MIC, plus the legacy
//! three-character `OTC` code), and carries the weekly session pattern, dated
//! exceptions, coverage window, and provenance.

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

/// A venue's weekly session definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CalendarWeekday {
    /// ISO weekday: 1 = Monday … 7 = Sunday.
    pub iso_weekday: i16,
    /// Local opening time.
    pub open_local: NaiveTime,
    /// Local closing time.
    pub close_local: NaiveTime,
}

/// The kind of a calendar exception.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum CalendarExceptionKind {
    /// A full-day closure.
    Closure,
    /// An extraordinary full session.
    Session,
    /// A session with an earlier local close.
    EarlyClose,
}

impl CalendarExceptionKind {
    /// Wire spelling used by the database and import document.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Closure => "closure",
            Self::Session => "session",
            Self::EarlyClose => "early_close",
        }
    }

    /// Parses a wire spelling.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "closure" => Some(Self::Closure),
            "session" => Some(Self::Session),
            "early_close" => Some(Self::EarlyClose),
            _ => None,
        }
    }
}

/// One effective-dated calendar exception.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CalendarException {
    /// Venue trading date.
    pub date: NaiveDate,
    /// Closure, extraordinary session, or early close.
    pub kind: CalendarExceptionKind,
    /// Local opening time, when the exception supplies one.
    pub open: Option<NaiveTime>,
    /// Local closing time, when the exception supplies one.
    pub close: Option<NaiveTime>,
    /// Optional human explanation.
    pub reason: Option<String>,
    /// Provenance of the exception.
    pub source: String,
}

/// A complete exchange calendar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ExchangeCalendar {
    /// Venue code.
    pub mic: String,
    /// Human-readable venue name.
    pub name: String,
    /// IANA time zone.
    pub time_zone: String,
    /// Inclusive first authoritative date.
    pub coverage_from: NaiveDate,
    /// Inclusive last authoritative date.
    pub coverage_to: NaiveDate,
    /// Regular weekly session pattern.
    pub weekdays: Vec<CalendarWeekday>,
    /// Dated exceptions ordered by date.
    pub exceptions: Vec<CalendarException>,
    /// Source label.
    pub source: String,
    /// Human-readable provenance.
    pub provenance: String,
    /// Current calendar version.
    pub version: i32,
    /// Whether the calendar is active.
    pub active: bool,
    /// Actor that last updated the calendar.
    pub updated_by: String,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
}

/// An effective-dated explicit asset-to-calendar assignment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CalendarAssignment {
    /// Assignment id.
    pub id: Uuid,
    /// Instrument id.
    pub asset_id: Uuid,
    /// Assigned venue code.
    pub mic: String,
    /// Inclusive first effective date.
    pub valid_from: NaiveDate,
    /// Exclusive last effective date; `None` is open-ended.
    pub valid_to: Option<NaiveDate>,
    /// Source label.
    pub source: String,
    /// Actor that created the assignment.
    pub created_by: String,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

/// Why a date fell back to the weekday approximation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum SegmentReason {
    /// The asset has no venue-qualified ticker and no explicit assignment.
    NoVenue,
    /// The asset's ticker identifiers name conflicting venues.
    AmbiguousVenue,
    /// The resolved MIC has no calendar.
    NoCalendar,
    /// The resolved calendar is retired.
    InactiveCalendar,
    /// The date is outside the resolved calendar's coverage.
    OutsideCoverage,
}

/// A contiguous date range sharing one calendar resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CalendarSegment {
    /// Resolved venue code, or `None` for the weekday fallback.
    pub mic: Option<String>,
    /// Resolved venue name, or `None` for the weekday fallback.
    pub name: Option<String>,
    /// Inclusive first date.
    pub from: NaiveDate,
    /// Inclusive last date.
    pub to: NaiveDate,
    /// True only when an active calendar covers the whole segment.
    pub exact: bool,
    /// Fallback reason when `exact` is false.
    pub reason: Option<SegmentReason>,
}

/// Calendar-aware coverage for one asset and date range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AssetCoverage {
    /// Instrument id.
    pub asset_id: Uuid,
    /// Inclusive requested first date.
    pub from: NaiveDate,
    /// Inclusive requested last date.
    pub to: NaiveDate,
    /// Server clock used for the completed-session decision.
    pub as_of: DateTime<Utc>,
    /// Ordered, non-overlapping resolution segments.
    pub segments: Vec<CalendarSegment>,
    /// Completed expected sessions in the range.
    pub expected_session_count: usize,
    /// Completed expected sessions with no stored bar.
    pub missing_sessions: Vec<NaiveDate>,
    /// Latest completed expected session on or before `as_of`.
    pub latest_completed_session: Option<NaiveDate>,
}

/// One calendar list row with usage counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CalendarSummary {
    /// Venue code.
    pub mic: String,
    /// Human-readable venue name.
    pub name: String,
    /// IANA time zone.
    pub time_zone: String,
    /// Inclusive first authoritative date.
    pub coverage_from: NaiveDate,
    /// Inclusive last authoritative date.
    pub coverage_to: NaiveDate,
    /// Source label.
    pub source: String,
    /// Human-readable provenance.
    pub provenance: String,
    /// Current version.
    pub version: i32,
    /// Whether the calendar is active.
    pub active: bool,
    /// Actor that last updated it.
    pub updated_by: String,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
    /// Explicitly assigned instruments (current or future-dated).
    pub assigned_asset_count: i64,
    /// Instruments resolving here on the current date.
    pub resolved_asset_count: i64,
    /// Number of dated exceptions.
    pub exception_count: i64,
}

/// One audit row shown on a calendar detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CalendarAuditEntry {
    /// Audit row id.
    pub id: Uuid,
    /// Action label.
    pub action: String,
    /// Actor id.
    pub actor_id: String,
    /// Human summary.
    pub summary: String,
    /// When it happened.
    pub created_at: DateTime<Utc>,
}

/// A calendar plus its usage counts and recent audit trail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CalendarDetail {
    /// The stored calendar.
    pub calendar: ExchangeCalendar,
    /// Explicitly assigned instruments.
    pub assigned_asset_count: i64,
    /// Instruments resolving here on the current date.
    pub resolved_asset_count: i64,
    /// Most recent audit entries.
    pub audit: Vec<CalendarAuditEntry>,
}

/// One calendar assignment with display context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CalendarAssignmentView {
    /// Assignment id.
    pub id: Uuid,
    /// Instrument id.
    pub asset_id: Uuid,
    /// Instrument display name.
    pub asset_display_name: String,
    /// Best available identifier.
    pub asset_identifier: Option<String>,
    /// Assigned venue code.
    pub mic: String,
    /// Inclusive first effective date.
    pub valid_from: NaiveDate,
    /// Exclusive last effective date.
    pub valid_to: Option<NaiveDate>,
    /// Source label.
    pub source: String,
    /// Actor that created it.
    pub created_by: String,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

/// One page of calendar assignments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CalendarAssignmentPage {
    /// Matching assignments.
    pub assignments: Vec<CalendarAssignmentView>,
    /// Total matching assignments.
    pub total: i64,
    /// Page size.
    pub limit: i64,
    /// Offset used.
    pub offset: i64,
}

/// One instrument affected by retiral.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AffectedAsset {
    /// Instrument id.
    pub asset_id: Uuid,
    /// Display name.
    pub display_name: String,
    /// Best identifier.
    pub identifier: Option<String>,
}

/// Retiral outcome when confirmation is required.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CalendarInUse {
    /// Instruments resolving to the calendar.
    pub affected_count: i64,
    /// Up to 100 affected instruments.
    pub affected_assets: Vec<AffectedAsset>,
}

/// One weekday in an import/export document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CalendarWeekdayDocument {
    /// ISO weekday: 1 = Monday … 7 = Sunday.
    pub iso_weekday: i16,
    /// Local opening time (`HH:MM[:SS]`).
    pub open: String,
    /// Local closing time (`HH:MM[:SS]`).
    pub close: String,
}

/// One exception in an import/export document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CalendarExceptionDocument {
    /// Venue trading date.
    pub date: NaiveDate,
    /// Exception kind.
    pub kind: CalendarExceptionKind,
    /// Optional local opening time.
    pub open: Option<String>,
    /// Optional local closing time.
    pub close: Option<String>,
    /// Optional reason.
    pub reason: Option<String>,
}

/// A single calendar document used by the seed and import/export surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CalendarDocument {
    /// Venue code.
    pub mic: String,
    /// Human-readable name.
    pub name: String,
    /// IANA time zone.
    pub time_zone: String,
    /// Inclusive first authoritative date.
    pub coverage_from: NaiveDate,
    /// Inclusive last authoritative date.
    pub coverage_to: NaiveDate,
    /// Regular weekly pattern.
    pub weekdays: Vec<CalendarWeekdayDocument>,
    /// Dated exceptions.
    #[serde(default)]
    pub exceptions: Vec<CalendarExceptionDocument>,
    /// Source label.
    pub source: String,
    /// Provenance.
    pub provenance: String,
    /// Read-only stored version.
    #[serde(default)]
    pub version: Option<i32>,
    /// Whether the calendar is active.
    #[serde(default = "default_active")]
    pub active: bool,
}

fn default_active() -> bool {
    true
}

/// The committed seed wrapper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CalendarSeedBundle {
    /// Seed schema version.
    pub seed_version: u32,
    /// Source/provenance summary.
    pub provenance: String,
    /// Calendar documents.
    pub calendars: Vec<CalendarDocument>,
}

/// Parses an `HH:MM` or `HH:MM:SS` local time.
pub fn parse_local_time(value: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(value, "%H:%M:%S")
        .or_else(|_| NaiveTime::parse_from_str(value, "%H:%M"))
        .ok()
}

/// Formats a local time as `HH:MM`.
pub fn format_local_time(value: NaiveTime) -> String {
    value.format("%H:%M").to_string()
}
