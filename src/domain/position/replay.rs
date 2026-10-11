//! Position-replay function. Implements FR-005 and the deterministic
//! tie-breaking rule from the predecessor's D2 (per Assumption 5 of this
//! spec).
//!
//! The four supported event types in this iteration are `Buy`, `Sell`,
//! `Transfer`, and `Split`. The other four (`Merger`, `SpinOff`, `Dividend`,
//! `StockBasedCompensation`) return a typed `UnsupportedEventType` error
//! because their per-type attribute schemas (D1) are deferred.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use crate::domain::id::DomainId;

use crate::domain::error::DomainError;
use crate::domain::event::entity::Event;
use crate::domain::event::entity::EventType;
use crate::domain::id::{AssetId, PortfolioId};
use crate::domain::position::entity::Position;

/// The result of replaying a sequence of events. The summary carries
/// statistics useful for tests and observability.
#[derive(Debug, PartialEq, Eq)]
/// Implements FR-005 of spec 016-implement-domain-model.
pub struct ReplaySummary {
    /// The final position.
    pub position: Position,
    /// The number of events that were applied.
    pub events_applied: usize,
    /// The number of events that were skipped (e.g., a destination-side
    /// transfer that does not affect the source position).
    pub events_skipped: usize,
}

/// Replay an event stream into a `Position`. The events are sorted by
/// `(effective_date, recorded_at, id)` per the deterministic tie-breaking
/// rule (Assumption 5).
///
/// Per FR-005, this function is the only sanctioned way to construct a
/// `Position`. There is no public field-level constructor.
///
/// Implements FR-005 of `specs/016-implement-domain-model/spec.md`.
pub fn replay_events<'a, I, T>(
    portfolio_id: PortfolioId,
    asset_id: AssetId,
    events: I,
) -> Result<ReplaySummary, DomainError>
where
    I: IntoIterator<Item = &'a T>,
    T: AsRef<Event> + 'a,
{
    let mut sorted: Vec<&Event> = events.into_iter().map(|e| e.as_ref()).collect();
    sorted.sort_by(|a, b| {
        a.effective_date
            .cmp(&b.effective_date)
            .then_with(|| a.recorded_at.cmp(&b.recorded_at))
            .then_with(|| a.id.cmp(&b.id))
    });

    let mut quantity = Decimal::ZERO;
    let mut as_of: Option<DateTime<Utc>> = None;
    let mut applied = 0usize;
    let mut skipped = 0usize;

    for ev in sorted {
        as_of = Some(ev.recorded_at);
        let et = ev.event_type().map_err(DomainError::Validation)?;
        match et {
            EventType::Buy => {
                quantity += ev.quantity_delta;
                applied += 1;
            }
            EventType::Sell => {
                quantity += ev.quantity_delta;
                applied += 1;
            }
            EventType::Transfer => {
                // A transfer from THIS portfolio subtracts. Transfers whose
                // source is a different portfolio are skipped (they affect
                // the destination, not us).
                if ev.portfolio_id() == portfolio_id {
                    quantity += ev.quantity_delta;
                    applied += 1;
                } else {
                    skipped += 1;
                }
            }
            EventType::Split => {
                // First iteration: assume a 2-for-1 split. The per-type
                // attribute schema (split ratio) is D1 and is deferred.
                // The quantity_delta column carries the additional units
                // created by the split (i.e., 1 split of 100 → +100 = 200).
                quantity += ev.quantity_delta;
                applied += 1;
            }
            EventType::Merger
            | EventType::SpinOff
            | EventType::Dividend
            | EventType::StockBasedCompensation => {
                return Err(DomainError::UnsupportedEventType {
                    event_type: et.to_string(),
                });
            }
        }
    }

    let position = Position {
        portfolio_id: portfolio_id.as_uuid(),
        asset_id: asset_id.as_uuid(),
        quantity,
        as_of: as_of.unwrap_or_else(Utc::now),
    };
    Ok(ReplaySummary { position, events_applied: applied, events_skipped: skipped })
}
