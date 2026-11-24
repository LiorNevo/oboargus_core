//! Run-scoped calendar lookups for the loaders.

use std::collections::{BTreeMap, HashMap};

use chrono::NaiveDate;
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::id::AssetId;

use super::entity::{CalendarSegment, ExchangeCalendar};
use super::resolve::resolve_timeline;
use super::sessions::is_trading_day;

struct Timeline {
    segments: Vec<CalendarSegment>,
    calendars: BTreeMap<String, ExchangeCalendar>,
}

/// Resolves each asset at most once per range and answers per-date closures.
pub struct CalendarCache {
    from: NaiveDate,
    to: NaiveDate,
    timelines: HashMap<Uuid, Timeline>,
}

impl CalendarCache {
    /// Builds an empty cache for one load range.
    pub fn new(from: NaiveDate, to: NaiveDate) -> Self {
        Self {
            from,
            to,
            timelines: HashMap::new(),
        }
    }

    /// Whether `date` is an exact full-day closure for `asset_id`.
    ///
    /// A fallback segment, missing calendar, or resolution error is not a
    /// closure: the loader keeps its every-day behavior.
    pub async fn is_closed(&mut self, pool: &PgPool, asset_id: AssetId, date: NaiveDate) -> bool {
        if !self.timelines.contains_key(&asset_id.0) {
            match resolve_timeline(pool, asset_id, self.from, self.to).await {
                Ok((segments, calendars)) => {
                    self.timelines.insert(
                        asset_id.0,
                        Timeline {
                            segments,
                            calendars,
                        },
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        asset_id = %asset_id.0,
                        error = %error,
                        "calendar resolution failed; keeping every-day loading"
                    );
                    return false;
                }
            }
        }
        let Some(timeline) = self.timelines.get(&asset_id.0) else {
            return false;
        };
        let Some(segment) = timeline
            .segments
            .iter()
            .find(|segment| segment.from <= date && date <= segment.to)
        else {
            return false;
        };
        if !segment.exact {
            return false;
        }
        let Some(calendar) = segment
            .mic
            .as_ref()
            .and_then(|mic| timeline.calendars.get(mic))
        else {
            return false;
        };
        !is_trading_day(calendar, date)
    }
}

impl std::fmt::Debug for CalendarCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CalendarCache")
            .field("assets", &self.timelines.len())
            .finish()
    }
}

/// Convenience wrapper that owns a one-date cache.
pub async fn asset_closed_on(pool: &PgPool, asset_id: AssetId, date: NaiveDate) -> bool {
    let mut cache = CalendarCache::new(date, date);
    cache.is_closed(pool, asset_id, date).await
}
