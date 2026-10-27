//! Bridges the legacy `NaiveDate` reference-data API onto the instant model.
//!
//! The public per-entity functions keep their `NaiveDate` signatures
//! (`specs/021-bitemporal-market-data/contracts/rust-api-contract.md` §7), so
//! they map a date to the single instant it denotes: midnight UTC. Precise
//! callers use the `*_with_known_at` entry points instead.

use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};

/// The midnight-UTC instant of a calendar date.
pub(crate) fn utc_midnight(date: NaiveDate) -> DateTime<Utc> {
    // `NaiveTime::MIN` is always a valid time, so this cannot fail.
    DateTime::<Utc>::from_naive_utc_and_offset(date.and_time(NaiveTime::MIN), Utc)
}

/// The current instant, truncated to the microsecond precision PostgreSQL
/// stores, so the value read back is exactly the value written.
pub(crate) fn now_micros() -> DateTime<Utc> {
    let now = Utc::now();
    now - Duration::nanoseconds(i64::from(now.timestamp_subsec_nanos() % 1_000))
}

/// One microsecond after `instant`, used to keep same-instant corrections
/// distinct under the append-only identity key.
pub(crate) fn one_microsecond_after(instant: DateTime<Utc>) -> DateTime<Utc> {
    instant + Duration::microseconds(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midnight_is_lossless_and_has_no_sub_second_component() {
        let date = NaiveDate::from_ymd_opt(2026, 3, 4).expect("date");
        let instant = utc_midnight(date);
        assert_eq!(instant.date_naive(), date);
        assert_eq!(instant.timestamp_subsec_nanos(), 0);
    }

    #[test]
    fn now_is_already_microsecond_aligned_and_can_be_bumped() {
        let now = now_micros();
        assert!(now.timestamp_subsec_nanos().is_multiple_of(1_000));
        assert!(one_microsecond_after(now) > now);
    }
}
