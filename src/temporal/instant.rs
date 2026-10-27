//! UTC instant newtypes and the precision guard.
//!
//! Both time axes use [`chrono::DateTime<Utc>`]. PostgreSQL stores
//! `TIMESTAMPTZ` at microsecond precision, so an instant whose sub-second
//! component is not a whole number of microseconds would be silently
//! truncated on write. The constructor rejects that instead: a truncation the
//! caller cannot see is a correctness bug in a financial history.

use std::fmt;

use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use super::error::TemporalError;

/// Valid time: when a fact is true in the world. Always UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ValidAt(DateTime<Utc>);

/// Transaction time: when the system recorded the belief. Always UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct KnownAt(DateTime<Utc>);

fn guard(instant: DateTime<Utc>, axis: &str) -> Result<DateTime<Utc>, TemporalError> {
    if !instant.timestamp_subsec_nanos().is_multiple_of(1_000) {
        return Err(TemporalError::InvalidTime {
            detail: format!(
                "{axis} {instant} has sub-microsecond precision and would be truncated on write"
            ),
        });
    }
    Ok(instant)
}

impl ValidAt {
    /// Wraps an instant, rejecting sub-microsecond precision.
    pub fn new(instant: DateTime<Utc>) -> Result<Self, TemporalError> {
        guard(instant, "valid_at").map(Self)
    }

    /// The midnight-UTC instant of a calendar date.
    ///
    /// Day-granular callers keep working: the date becomes a single instant,
    /// which is the only lossless mapping the model allows.
    pub fn midnight_utc(date: NaiveDate) -> Self {
        // `00:00:00` always exists, so the fallback is unreachable; keeping it
        // avoids `unwrap` per the project's no-panic rule.
        let naive = date.and_hms_opt(0, 0, 0).unwrap_or_else(|| {
            NaiveDate::from_ymd_opt(1970, 1, 1)
                .unwrap_or_default()
                .and_hms_opt(0, 0, 0)
                .unwrap_or_default()
        });
        Self(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc))
    }

    /// The wrapped instant.
    pub fn as_datetime(&self) -> DateTime<Utc> {
        self.0
    }

    /// Shift by a duration, used for window stepping.
    pub fn shifted(self, duration: Duration) -> Self {
        Self(self.0 + duration)
    }
}

impl KnownAt {
    /// Wraps an instant, rejecting sub-microsecond precision.
    pub fn new(instant: DateTime<Utc>) -> Result<Self, TemporalError> {
        guard(instant, "known_at").map(Self)
    }

    /// The current instant, truncated to microsecond precision.
    ///
    /// Truncation is explicit here, before the precision guard runs, so the
    /// common "now" path never fails.
    pub fn now() -> Self {
        let now = Utc::now();
        Self(now - Duration::nanoseconds(i64::from(now.timestamp_subsec_nanos() % 1_000)))
    }

    /// The midnight-UTC instant of a calendar date.
    pub fn midnight_utc(date: NaiveDate) -> Self {
        Self(ValidAt::midnight_utc(date).as_datetime())
    }

    /// The wrapped instant.
    pub fn as_datetime(&self) -> DateTime<Utc> {
        self.0
    }

    /// One microsecond later, used to keep same-instant writes distinct.
    pub fn plus_microsecond(self) -> Self {
        Self(self.0 + Duration::microseconds(1))
    }
}

impl Default for ValidAt {
    fn default() -> Self {
        Self(DateTime::<Utc>::UNIX_EPOCH)
    }
}

impl Default for KnownAt {
    fn default() -> Self {
        Self(DateTime::<Utc>::UNIX_EPOCH)
    }
}

impl fmt::Display for ValidAt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for KnownAt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A half-open valid-time window `[from, to)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporalWindow {
    /// Inclusive lower bound.
    pub from: ValidAt,
    /// Exclusive upper bound.
    pub to: ValidAt,
}

impl TemporalWindow {
    /// Builds a window, rejecting reversed and empty windows.
    pub fn new(from: ValidAt, to: ValidAt) -> Result<Self, TemporalError> {
        if from >= to {
            return Err(TemporalError::InvalidTime {
                detail: format!("window [{from}, {to}) is empty or reversed"),
            });
        }
        Ok(Self { from, to })
    }

    /// Whether `instant` falls inside `[from, to)`.
    pub fn contains(&self, instant: ValidAt) -> bool {
        instant >= self.from && instant < self.to
    }
}

impl fmt::Display for TemporalWindow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}, {})", self.from, self.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64, millis: u32) -> DateTime<Utc> {
        DateTime::<Utc>::UNIX_EPOCH
            + Duration::seconds(seconds)
            + Duration::milliseconds(i64::from(millis))
    }

    #[test]
    fn millisecond_separation_is_preserved_and_ordered() {
        let a = ValidAt::new(at(0, 0)).expect("midnight");
        let b = ValidAt::new(at(0, 1)).expect("one millisecond later");
        assert_ne!(a, b);
        assert!(a < b);
    }

    #[test]
    fn sub_microsecond_precision_is_rejected_on_both_axes() {
        let nanos = DateTime::<Utc>::UNIX_EPOCH + Duration::nanoseconds(1);
        assert!(matches!(
            ValidAt::new(nanos),
            Err(TemporalError::InvalidTime { .. })
        ));
        assert!(matches!(
            KnownAt::new(nanos),
            Err(TemporalError::InvalidTime { .. })
        ));
    }

    #[test]
    fn midnight_utc_is_lossless_for_day_granular_callers() {
        let date = NaiveDate::from_ymd_opt(2026, 1, 2).expect("date");
        let instant = ValidAt::midnight_utc(date);
        assert_eq!(
            instant.as_datetime().format("%Y-%m-%d").to_string(),
            "2026-01-02"
        );
        assert_eq!(instant.as_datetime().timestamp_subsec_nanos(), 0);
    }

    #[test]
    fn window_rejects_reversed_and_empty_bounds() {
        let from = ValidAt::new(at(10, 0)).expect("instant");
        let earlier = ValidAt::new(at(5, 0)).expect("instant");

        assert!(TemporalWindow::new(from, from).is_err());
        assert!(TemporalWindow::new(from, earlier).is_err());

        let to = ValidAt::new(at(20, 0)).expect("instant");
        let window = TemporalWindow::new(from, to).expect("valid window");
        assert!(window.contains(from));
        assert!(!window.contains(to));
    }

    #[test]
    fn known_at_now_is_microsecond_aligned_and_monotonic_stepping_works() {
        let now = KnownAt::now();
        assert!(
            now.as_datetime()
                .timestamp_subsec_nanos()
                .is_multiple_of(1_000)
        );
        assert!(now.plus_microsecond() > now);
    }
}
