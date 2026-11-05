//! Bridges the legacy `NaiveDate` reference-data API onto the instant model.
//!
//! The public per-entity functions keep their `NaiveDate` signatures
//! (`specs/021-bitemporal-market-data/contracts/rust-api-contract.md` §7), so
//! they map a date to the single instant it denotes: midnight UTC. Precise
//! callers use the `*_with_known_at` entry points instead.

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};

/// The midnight-UTC instant of a calendar date.
#[allow(dead_code)]
pub(crate) fn utc_midnight(date: NaiveDate) -> DateTime<Utc> {
    // `NaiveTime::MIN` is always a valid time, so this cannot fail.
    DateTime::<Utc>::from_naive_utc_and_offset(date.and_time(NaiveTime::MIN), Utc)
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
}
