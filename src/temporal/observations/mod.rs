//! Typed market observations: end-of-day bars, intraday bars, and quotes.
//!
//! Observations are deliberately separate from
//! [`crate::temporal::facts`]: they are high-volume,
//! partitioned by valid time, and identified by a content hash. The fact
//! catalog still names them (`market_price` maps to `kind = 'eod'`), so one
//! dictionary covers both stores.

#![forbid(unsafe_code)]

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{AssertSqlSafe, FromRow, PgPool};
use uuid::Uuid;

use crate::domain::id::{AssetId, DomainId, new_uuid};
use crate::temporal::error::TemporalError;
use crate::temporal::facts::value::{Adjustment, FactValue, MarketPriceValue, hash_parts};
use crate::temporal::instant::{KnownAt, TemporalWindow, ValidAt};

/// A new observation to append.
#[derive(Debug, Clone)]
pub struct NewMarketObservation {
    /// The asset the observation is about.
    pub asset_id: AssetId,
    /// Observation kind (`eod`, `bar`, `quote`).
    pub kind: String,
    /// Bar interval in seconds; 0 for eod/quote.
    pub interval_seconds: i32,
    /// When the observation is true in the world.
    pub valid_at: ValidAt,
    /// When the system recorded it.
    pub known_at: KnownAt,
    /// Reporting source.
    pub source: String,
    /// Opening price.
    pub open: Option<Decimal>,
    /// Highest price.
    pub high: Option<Decimal>,
    /// Lowest price.
    pub low: Option<Decimal>,
    /// Unadjusted close.
    pub close: Option<Decimal>,
    /// Adjusted close.
    pub adjclose: Option<Decimal>,
    /// Single price for a quote.
    pub price: Option<Decimal>,
    /// Volume.
    pub volume: Option<Decimal>,
    /// ISO 4217 quote currency.
    pub currency: String,
    /// Adjustment basis.
    pub adjustment: Adjustment,
    /// Source-provided sequence, when available.
    pub sequence: Option<i64>,
}

/// A stored observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct Observation {
    /// Row id.
    pub id: Uuid,
    /// Subject asset.
    pub asset_id: Uuid,
    /// Observation kind.
    pub kind: String,
    /// Interval in seconds.
    pub interval_seconds: i32,
    /// Valid time.
    pub valid_at: DateTime<Utc>,
    /// Reporting source.
    pub source: String,
    /// Transaction time.
    pub known_at: DateTime<Utc>,
    /// Canonical content hash.
    pub content_hash: String,
    /// Opening price.
    pub open: Option<Decimal>,
    /// Highest price.
    pub high: Option<Decimal>,
    /// Lowest price.
    pub low: Option<Decimal>,
    /// Unadjusted close.
    pub close: Option<Decimal>,
    /// Adjusted close.
    pub adjclose: Option<Decimal>,
    /// Quote price.
    pub price: Option<Decimal>,
    /// Volume.
    pub volume: Option<Decimal>,
    /// ISO 4217 quote currency.
    pub currency: String,
    /// Adjustment basis.
    pub adjustment: String,
    /// Source sequence.
    pub sequence: Option<i64>,
    /// Whether this belief retracts the value.
    pub retracted: bool,
    /// Why the value was retracted.
    pub retraction_reason: Option<String>,
}

impl Observation {
    /// The subject asset id.
    pub fn asset_id(&self) -> AssetId {
        AssetId::from_uuid(self.asset_id)
    }

    /// The valid-time newtype.
    pub fn valid_at(&self) -> Result<ValidAt, TemporalError> {
        ValidAt::new(self.valid_at)
    }

    /// The known-time newtype.
    pub fn known_at(&self) -> Result<KnownAt, TemporalError> {
        KnownAt::new(self.known_at)
    }

    /// The typed market-price value, or `None` for a retraction.
    pub fn market_price(&self) -> Result<Option<FactValue>, TemporalError> {
        if self.retracted {
            return Ok(None);
        }
        let adjustment = Adjustment::from_wire(&self.adjustment).ok_or_else(|| {
            TemporalError::InvalidFactValue {
                fact_type: "market_price".to_string(),
                detail: format!("unknown adjustment {:?}", self.adjustment),
            }
        })?;
        let (Some(open), Some(high), Some(low), Some(close)) =
            (self.open, self.high, self.low, self.close)
        else {
            return Err(TemporalError::InvalidFactValue {
                fact_type: "market_price".to_string(),
                detail: "observation is missing OHLC fields".to_string(),
            });
        };
        Ok(Some(FactValue::MarketPrice(MarketPriceValue {
            open,
            high,
            low,
            close,
            adjclose: self.adjclose,
            volume: self.volume,
            currency: self.currency.trim().to_string(),
            adjustment,
        })))
    }

    /// Converts this observation into a `FactPoint`.
    pub fn to_fact_point(self) -> Result<crate::temporal::facts::FactPoint, TemporalError> {
        let subject = crate::temporal::facts::FactSubject::Asset(self.asset_id());
        let value = self.market_price()?;
        Ok(crate::temporal::facts::FactPoint {
            id: self.id,
            subject,
            fact_type: "market_price".to_string(),
            schema_version: 1,
            valid_at: self.valid_at()?,
            known_at: self.known_at()?,
            value,
            source: self.source,
            retracted: self.retracted,
            retraction_reason: self.retraction_reason,
            content_hash: self.content_hash,
        })
    }

    /// The canonical hash for the substantive payload.
    pub fn hash_for(input: &NewMarketObservation) -> String {
        hash_parts(&[
            "observation",
            &input.asset_id.as_uuid().to_string(),
            &input.kind,
            &input.interval_seconds.to_string(),
            &input.valid_at.to_string(),
            &input.source,
            &decimal_part(input.open),
            &decimal_part(input.high),
            &decimal_part(input.low),
            &decimal_part(input.close),
            &decimal_part(input.adjclose),
            &decimal_part(input.price),
            &decimal_part(input.volume),
            &input.currency.trim().to_uppercase(),
            input.adjustment.as_str(),
            &input.sequence.map(|v| v.to_string()).unwrap_or_default(),
        ])
    }
}

fn decimal_part(value: Option<Decimal>) -> String {
    value.map(|v| v.normalize().to_string()).unwrap_or_default()
}

/// Appends an observation. A duplicate content hash is an idempotent no-op.
pub async fn append_observation(
    pool: &PgPool,
    input: NewMarketObservation,
) -> Result<Observation, TemporalError> {
    append_observation_outcome(pool, input)
        .await
        .map(|(row, _)| row)
}

/// Appends an observation and reports whether a new row was inserted.
///
/// `false` means the identical content hash already existed and the existing
/// row was returned.
pub async fn append_observation_outcome(
    pool: &PgPool,
    mut input: NewMarketObservation,
) -> Result<(Observation, bool), TemporalError> {
    input.currency = input.currency.trim().to_uppercase();
    let hash = Observation::hash_for(&input);

    if let Some((current_hash, current_known_at, current_id)) =
        current_observation(pool, &input).await?
    {
        if current_hash == hash {
            let existing = fetch_by_hash(
                pool,
                input.asset_id,
                &input.kind,
                input.interval_seconds,
                input.valid_at,
                &input.source,
                &hash,
            )
            .await?
            .ok_or(TemporalError::MissingPayload {
                fact_id: current_id,
            })?;
            return Ok((existing, false));
        }
        if input.known_at.as_datetime() <= current_known_at {
            return Err(TemporalError::conflict(format!(
                "observation {} {} at {} from {}",
                input.asset_id.as_uuid(),
                input.kind,
                input.valid_at,
                input.source
            )));
        }
    }

    let id = new_uuid();

    let inserted: Option<Observation> = sqlx::query_as(
        r#"
        INSERT INTO market_observations
            (id, asset_id, kind, interval_seconds, valid_at, source, known_at,
             content_hash, open, high, low, close, adjclose, price, volume,
             currency, adjustment, sequence, retracted)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                $15, $16, $17, $18, FALSE)
        ON CONFLICT (asset_id, kind, interval_seconds, valid_at, source, content_hash)
        DO NOTHING
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(input.asset_id.as_uuid())
    .bind(&input.kind)
    .bind(input.interval_seconds)
    .bind(input.valid_at.as_datetime())
    .bind(&input.source)
    .bind(input.known_at.as_datetime())
    .bind(&hash)
    .bind(input.open)
    .bind(input.high)
    .bind(input.low)
    .bind(input.close)
    .bind(input.adjclose)
    .bind(input.price)
    .bind(input.volume)
    .bind(&input.currency)
    .bind(input.adjustment.as_str())
    .bind(input.sequence)
    .fetch_optional(pool)
    .await?;

    if let Some(row) = inserted {
        return Ok((row, true));
    }

    let existing = fetch_by_hash(
        pool,
        input.asset_id,
        &input.kind,
        input.interval_seconds,
        input.valid_at,
        &input.source,
        &hash,
    )
    .await?
    .ok_or_else(|| TemporalError::Infrastructure(sqlx::Error::RowNotFound))?;
    Ok((existing, false))
}

/// Appends a retraction belief for an observation.
pub async fn retract_observation(
    pool: &PgPool,
    input: NewMarketObservation,
    reason: &str,
) -> Result<Observation, TemporalError> {
    if reason.trim().is_empty() {
        return Err(TemporalError::InvalidFactValue {
            fact_type: "market_price".to_string(),
            detail: "retraction reason must not be empty".to_string(),
        });
    }
    let hash = hash_parts(&[
        "observation-retract",
        &input.asset_id.as_uuid().to_string(),
        &input.kind,
        &input.interval_seconds.to_string(),
        &input.valid_at.to_string(),
        &input.source,
        reason,
    ]);
    if let Some((current_hash, current_known_at, _)) = current_observation(pool, &input).await? {
        if current_hash == hash {
            let existing = fetch_by_hash(
                pool,
                input.asset_id,
                &input.kind,
                input.interval_seconds,
                input.valid_at,
                &input.source,
                &hash,
            )
            .await?
            .ok_or(TemporalError::Infrastructure(sqlx::Error::RowNotFound))?;
            return Ok(existing);
        }
        if input.known_at.as_datetime() <= current_known_at {
            return Err(TemporalError::conflict(format!(
                "observation {} {} at {} from {}",
                input.asset_id.as_uuid(),
                input.kind,
                input.valid_at,
                input.source
            )));
        }
    }

    let id = new_uuid();
    let currency = input.currency.trim().to_uppercase();

    let inserted: Option<Observation> = sqlx::query_as(
        r#"
        INSERT INTO market_observations
            (id, asset_id, kind, interval_seconds, valid_at, source, known_at,
             content_hash, currency, adjustment, retracted, retraction_reason)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, TRUE, $11)
        ON CONFLICT (asset_id, kind, interval_seconds, valid_at, source, content_hash)
        DO NOTHING
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(input.asset_id.as_uuid())
    .bind(&input.kind)
    .bind(input.interval_seconds)
    .bind(input.valid_at.as_datetime())
    .bind(&input.source)
    .bind(input.known_at.as_datetime())
    .bind(&hash)
    .bind(&currency)
    .bind(input.adjustment.as_str())
    .bind(reason)
    .fetch_optional(pool)
    .await?;

    if let Some(row) = inserted {
        return Ok(row);
    }

    let existing = fetch_by_hash(
        pool,
        input.asset_id,
        &input.kind,
        input.interval_seconds,
        input.valid_at,
        &input.source,
        &hash,
    )
    .await?
    .ok_or_else(|| TemporalError::Infrastructure(sqlx::Error::RowNotFound))?;
    Ok(existing)
}

async fn current_observation(
    pool: &PgPool,
    input: &NewMarketObservation,
) -> Result<Option<(String, DateTime<Utc>, Uuid)>, TemporalError> {
    let row: Option<(String, DateTime<Utc>, Uuid)> = sqlx::query_as(
        r#"
        SELECT content_hash, known_at, id
        FROM market_observations
        WHERE asset_id = $1 AND kind = $2 AND interval_seconds = $3
          AND valid_at = $4 AND source = $5
        ORDER BY known_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(input.asset_id.as_uuid())
    .bind(&input.kind)
    .bind(input.interval_seconds)
    .bind(input.valid_at.as_datetime())
    .bind(&input.source)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

async fn fetch_by_hash(
    pool: &PgPool,
    asset_id: AssetId,
    kind: &str,
    interval_seconds: i32,
    valid_at: ValidAt,
    source: &str,
    hash: &str,
) -> Result<Option<Observation>, TemporalError> {
    let row: Option<Observation> = sqlx::query_as(
        r#"
        SELECT * FROM market_observations
        WHERE asset_id = $1 AND kind = $2 AND interval_seconds = $3
          AND valid_at = $4 AND source = $5 AND content_hash = $6
        "#,
    )
    .bind(asset_id.as_uuid())
    .bind(kind)
    .bind(interval_seconds)
    .bind(valid_at.as_datetime())
    .bind(source)
    .bind(hash)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Resolves an observation series over a window.
///
/// One point per valid instant: the greatest `known_at` wins, ties broken by
/// `id`. Retracted winners are omitted.
pub async fn observation_range(
    pool: &PgPool,
    asset_id: AssetId,
    kind: &str,
    interval_seconds: i32,
    source: &str,
    window: TemporalWindow,
    known_at: Option<KnownAt>,
) -> Result<Vec<Observation>, TemporalError> {
    let rows: Vec<Observation> = sqlx::query_as(
        r#"
        SELECT DISTINCT ON (valid_at) *
        FROM market_observations
        WHERE asset_id = $1 AND kind = $2 AND interval_seconds = $3
          AND source = $4 AND valid_at >= $5 AND valid_at < $6
          AND ($7::timestamptz IS NULL OR known_at <= $7)
        ORDER BY valid_at, known_at DESC, id DESC
        "#,
    )
    .bind(asset_id.as_uuid())
    .bind(kind)
    .bind(interval_seconds)
    .bind(source)
    .bind(window.from.as_datetime())
    .bind(window.to.as_datetime())
    .bind(known_at.map(|instant| instant.as_datetime()))
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().filter(|row| !row.retracted).collect())
}

/// Resolves one observation at an instant, using latest knowledge.
pub async fn observation_at(
    pool: &PgPool,
    asset_id: AssetId,
    kind: &str,
    interval_seconds: i32,
    source: &str,
    valid_at: ValidAt,
) -> Result<Option<Observation>, TemporalError> {
    let row: Option<Observation> = sqlx::query_as(
        r#"
        SELECT * FROM market_observations
        WHERE asset_id = $1 AND kind = $2 AND interval_seconds = $3 AND source = $4
          AND valid_at <= $5
        ORDER BY valid_at DESC, known_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(asset_id.as_uuid())
    .bind(kind)
    .bind(interval_seconds)
    .bind(source)
    .bind(valid_at.as_datetime())
    .fetch_optional(pool)
    .await?;
    Ok(row.filter(|row| !row.retracted))
}

/// Creates the monthly partitions covering `from` through `to` (inclusive).
///
/// Idempotent. The default partition protects against a gap if this helper is
/// not called before a write.
pub async fn ensure_market_observation_partitions(
    pool: &PgPool,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<(), TemporalError> {
    if from > to {
        return Err(TemporalError::invalid_time("partition range is reversed"));
    }
    let mut cursor = NaiveDate::from_ymd_opt(from.year(), from.month(), 1)
        .ok_or_else(|| TemporalError::invalid_time("invalid partition start"))?;
    let end = NaiveDate::from_ymd_opt(to.year(), to.month(), 1)
        .ok_or_else(|| TemporalError::invalid_time("invalid partition end"))?;

    while cursor <= end {
        let next = add_month(cursor)?;
        let name = format!(
            "market_observations_{}_{:02}",
            cursor.year(),
            cursor.month()
        );
        let sql = format!(
            "CREATE TABLE IF NOT EXISTS {name} PARTITION OF market_observations \
             FOR VALUES FROM ('{}') TO ('{}')",
            cursor.format("%Y-%m-%d"),
            next.format("%Y-%m-%d")
        );
        sqlx::query(AssertSqlSafe(sql)).execute(pool).await?;
        cursor = next;
    }
    Ok(())
}

fn add_month(date: NaiveDate) -> Result<NaiveDate, TemporalError> {
    let (year, month) = if date.month() == 12 {
        (date.year() + 1, 1)
    } else {
        (date.year(), date.month() + 1)
    };
    NaiveDate::from_ymd_opt(year, month, 1)
        .ok_or_else(|| TemporalError::invalid_time("month overflow"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_month_rolls_over_the_year() {
        let december = NaiveDate::from_ymd_opt(2026, 12, 1).expect("date");
        assert_eq!(
            add_month(december).expect("next"),
            NaiveDate::from_ymd_opt(2027, 1, 1).expect("date")
        );
    }

    #[test]
    fn hash_is_stable_for_equivalent_decimal_scales() {
        let subject = AssetId::from_uuid(Uuid::new_v4());
        let valid = ValidAt::midnight_utc(NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"));
        let known = KnownAt::midnight_utc(NaiveDate::from_ymd_opt(2026, 1, 2).expect("date"));
        let mut a = NewMarketObservation {
            asset_id: subject,
            kind: "eod".into(),
            interval_seconds: 0,
            valid_at: valid,
            known_at: known,
            source: "test".into(),
            open: Some(Decimal::from_str_exact("1.0").expect("decimal")),
            high: Some(Decimal::from_str_exact("1.0").expect("decimal")),
            low: Some(Decimal::from_str_exact("1.0").expect("decimal")),
            close: Some(Decimal::from_str_exact("1.0").expect("decimal")),
            adjclose: None,
            price: None,
            volume: Some(Decimal::ONE),
            currency: "usd".into(),
            adjustment: Adjustment::Unadjusted,
            sequence: None,
        };
        let b = {
            let mut b = a.clone();
            b.open = Some(Decimal::from_str_exact("1.00").expect("decimal"));
            b.high = Some(Decimal::from_str_exact("1.00").expect("decimal"));
            b.low = Some(Decimal::from_str_exact("1.00").expect("decimal"));
            b.close = Some(Decimal::from_str_exact("1.00").expect("decimal"));
            b
        };
        a.currency = "USD".into();
        assert_eq!(Observation::hash_for(&a), Observation::hash_for(&b));
    }

    #[test]
    fn date_helpers_are_exposed() {
        let date = NaiveDate::from_ymd_opt(2026, 1, 2).expect("date");
        assert_eq!(date.year(), 2026);
    }
}
