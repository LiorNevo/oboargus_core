//! Bundled exchange-calendar seed.

use sqlx::PgPool;

use crate::domain::error::DomainError;

use super::entity::CalendarSeedBundle;
use super::write::insert_calendar_if_absent;

const SEED_JSON: &str = include_str!("../../../seed/exchange_calendars/us_equities_2016_2028.json");
const SEED_ACTOR: &str = "system";

/// Parses and validates the committed seed bundle without touching the database.
pub fn parse_bundled_seed() -> Result<CalendarSeedBundle, DomainError> {
    let bundle: CalendarSeedBundle = serde_json::from_str(SEED_JSON).map_err(|error| {
        DomainError::Validation(format!("bundled calendar seed is invalid: {error}"))
    })?;
    if bundle.seed_version != 1 {
        return Err(DomainError::Validation(format!(
            "unsupported bundled calendar seed version {}",
            bundle.seed_version
        )));
    }
    Ok(bundle)
}

/// Inserts every bundled calendar that is not already present.
///
/// The operation is idempotent and never overwrites an operator-managed row.
/// Returns the number of calendars inserted.
pub async fn seed_bundled_calendars(pool: &PgPool) -> Result<usize, DomainError> {
    let bundle = parse_bundled_seed()?;
    let mut inserted = 0;
    for document in bundle.calendars {
        if insert_calendar_if_absent(pool, document, SEED_ACTOR).await? {
            inserted += 1;
        }
    }
    Ok(inserted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_seed_is_valid_and_covers_the_initial_venues() {
        let bundle = parse_bundled_seed().expect("seed");
        assert_eq!(bundle.seed_version, 1);
        let mics: Vec<_> = bundle
            .calendars
            .iter()
            .map(|calendar| calendar.mic.as_str())
            .collect();
        assert_eq!(mics, vec!["XNAS", "XNYS", "ARCX", "XASE", "BATS", "OTC"]);
        for calendar in &bundle.calendars {
            assert!(calendar.weekdays.len() >= 5);
            assert!(!calendar.exceptions.is_empty());
            assert_eq!(calendar.coverage_from.to_string(), "2016-01-01");
            assert_eq!(calendar.coverage_to.to_string(), "2028-12-31");
        }
    }
}
