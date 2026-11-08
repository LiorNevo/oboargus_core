//! Trigger schedule helpers.

use std::str::FromStr;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use cron::Schedule;

use crate::automation::error::AutomationError;

/// Computes the next occurrence for an interval schedule.
pub fn next_interval(
    from: DateTime<Utc>,
    interval_seconds: u64,
) -> Result<DateTime<Utc>, AutomationError> {
    if interval_seconds == 0 {
        return Err(AutomationError::Validation(
            "interval_seconds must be >= 1".to_string(),
        ));
    }
    let seconds = i64::try_from(interval_seconds)
        .map_err(|_| AutomationError::Validation("interval is too large".to_string()))?;
    Ok(from + chrono::Duration::seconds(seconds))
}

/// Computes the next occurrence for a cron expression in an IANA timezone.
pub fn next_cron(
    expression: &str,
    timezone: &str,
    from: DateTime<Utc>,
) -> Result<DateTime<Utc>, AutomationError> {
    let zone: Tz = timezone
        .parse()
        .map_err(|_| AutomationError::Validation(format!("unknown timezone '{timezone}'")))?;
    let schedule = Schedule::from_str(expression).map_err(|error| {
        AutomationError::Validation(format!("invalid cron expression: {error}"))
    })?;
    let local = from.with_timezone(&zone);
    schedule
        .after(&local)
        .next()
        .map(|next| next.with_timezone(&Utc))
        .ok_or_else(|| {
            AutomationError::Validation("cron expression has no next occurrence".to_string())
        })
}

/// Computes the next occurrence for either repeat kind.
pub fn next_occurrence(
    repeat_kind: &str,
    interval_seconds: Option<u64>,
    cron_expression: Option<&str>,
    timezone: &str,
    from: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, AutomationError> {
    match repeat_kind {
        "interval" => {
            let seconds = interval_seconds.ok_or_else(|| {
                AutomationError::Validation("interval_seconds required".to_string())
            })?;
            next_interval(from, seconds).map(Some)
        }
        "cron" => {
            let expression = cron_expression.ok_or_else(|| {
                AutomationError::Validation("cron expression required".to_string())
            })?;
            next_cron(expression, timezone, from).map(Some)
        }
        other => Err(AutomationError::Validation(format!(
            "unknown repeat kind '{other}'"
        ))),
    }
}
