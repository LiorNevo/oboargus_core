//! The single bitemporal resolution rule.
//!
//! Every read in [`crate::temporal`] uses the same winner: the candidate with
//! the greatest transaction time, ties broken by the greatest row id. The
//! ordering is intentional and public — changing it changes historical
//! answers, so it is a breaking change.
//!
//! For position versions the valid time participates first
//! (`valid_at DESC, known_at DESC, id DESC`); callers pre-filter by
//! `valid_at <= T` and hand the survivors to [`resolve`](crate::temporal::resolution::resolve), which applies the
//! shared `known_at, id` tail.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// One resolution candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    /// The row id, used as the deterministic tie-break.
    pub id: Uuid,
    /// The transaction time of this belief.
    pub known_at: DateTime<Utc>,
    /// Whether this belief retracts the value.
    pub retracted: bool,
}

/// The outcome of resolving a candidate set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution<'a> {
    /// The winner carries a value.
    Value(&'a Candidate),
    /// The winner is a retraction: the answer is "no value".
    Retracted(&'a Candidate),
    /// There were no candidates.
    None,
}

impl<'a> Resolution<'a> {
    /// The winning candidate, if any (including a retraction).
    pub fn winner(self) -> Option<&'a Candidate> {
        match self {
            Self::Value(candidate) | Self::Retracted(candidate) => Some(candidate),
            Self::None => None,
        }
    }

    /// The value-carrying winner, or `None` when absent or retracted.
    pub fn value(self) -> Option<&'a Candidate> {
        match self {
            Self::Value(candidate) => Some(candidate),
            Self::Retracted(_) | Self::None => None,
        }
    }
}

/// Picks the greatest `(known_at, id)` candidate and reports whether it
/// retracts the value.
pub fn resolve(candidates: &[Candidate]) -> Resolution<'_> {
    let winner = candidates
        .iter()
        .max_by(|a, b| a.known_at.cmp(&b.known_at).then_with(|| a.id.cmp(&b.id)));
    match winner {
        Some(candidate) if candidate.retracted => Resolution::Retracted(candidate),
        Some(candidate) => Resolution::Value(candidate),
        None => Resolution::None,
    }
}

/// The SQL ordering that implements [`resolve`] for fact-like tables.
pub const FACT_WINNER_ORDER: &str = "known_at DESC, id DESC";

/// The SQL ordering that implements resolution for position versions.
pub const POSITION_WINNER_ORDER: &str = "valid_at DESC, known_at DESC, id DESC";

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn candidate(id: u128, seconds: i64, retracted: bool) -> Candidate {
        Candidate {
            id: Uuid::from_u128(id),
            known_at: DateTime::<Utc>::UNIX_EPOCH + Duration::seconds(seconds),
            retracted,
        }
    }

    #[test]
    fn later_known_at_supersedes_earlier() {
        let older = candidate(1, 10, false);
        let newer = candidate(2, 20, false);
        assert_eq!(resolve(&[older, newer]), Resolution::Value(&newer));
    }

    #[test]
    fn a_retracted_winner_yields_no_value() {
        let older = candidate(1, 10, false);
        let retraction = candidate(2, 20, true);
        let candidates = [older, retraction];
        let resolution = resolve(&candidates);
        assert!(matches!(resolution, Resolution::Retracted(_)));
        assert_eq!(resolution.value(), None);
        assert_eq!(resolution.winner().map(|c| c.id), Some(retraction.id));
    }

    #[test]
    fn uuid_breaks_ties_deterministically_regardless_of_input_order() {
        let low = candidate(1, 10, false);
        let high = candidate(2, 10, false);
        assert_eq!(resolve(&[low, high]), Resolution::Value(&high));
        assert_eq!(resolve(&[high, low]), Resolution::Value(&high));
    }

    #[test]
    fn an_empty_candidate_set_resolves_to_none() {
        assert_eq!(resolve(&[]), Resolution::None);
    }
}
