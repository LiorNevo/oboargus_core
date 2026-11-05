//! Typed, validated fact writes.
//!
//! Every producer (refresh, load, reconcile, API, CLI) goes through
//! [`write_fact`]. The function validates against the catalog and writes the
//! envelope plus exactly one typed payload row in one transaction.

use chrono::Utc;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::domain::id::{DomainId, new_uuid};
use crate::temporal::catalog::{StorageKind, fact_type};
use crate::temporal::error::TemporalError;
use crate::temporal::facts::read;
use crate::temporal::facts::value::{FactValue, content_hash, retraction_hash};
use crate::temporal::facts::{FactPoint, FactSubject};
use crate::temporal::instant::{KnownAt, ValidAt};
use crate::temporal::observations::{
    NewMarketObservation, append_observation_outcome, retract_observation,
};

/// A typed fact to append.
#[derive(Debug, Clone)]
pub struct FactInput {
    /// The fact subject.
    pub subject: FactSubject,
    /// The catalog fact type.
    pub fact_type: String,
    /// When the fact is true in the world.
    pub valid_at: ValidAt,
    /// When the system recorded it.
    pub known_at: KnownAt,
    /// Reporting source.
    pub source: String,
    /// Typed value.
    pub value: FactValue,
}

/// A retraction to append.
#[derive(Debug, Clone)]
pub struct RetractionInput {
    /// The fact subject.
    pub subject: FactSubject,
    /// The catalog fact type.
    pub fact_type: String,
    /// When the retraction is in force.
    pub valid_at: ValidAt,
    /// When the system recorded it.
    pub known_at: KnownAt,
    /// Reporting source.
    pub source: String,
    /// Why the value is retracted.
    pub reason: String,
}

/// Appends a typed fact.
///
/// A duplicate `(identity, content_hash)` is an idempotent no-op. A later
/// belief with a new hash is a correction. An append whose `known_at` is not
/// later than the current winner is a conflict.
pub async fn write_fact(pool: &PgPool, input: FactInput) -> Result<FactPoint, TemporalError> {
    write_fact_outcome(pool, input)
        .await
        .map(|(point, _)| point)
}

/// Appends a typed fact and reports whether a new row was inserted.
///
/// `false` means the identical content hash already existed and the existing
/// point was returned.
pub async fn write_fact_outcome(
    pool: &PgPool,
    input: FactInput,
) -> Result<(FactPoint, bool), TemporalError> {
    let def = fact_type(&input.fact_type).ok_or_else(|| TemporalError::UnknownFactType {
        fact_type: input.fact_type.clone(),
    })?;
    if !def.applies_to(input.subject.kind()) {
        return Err(TemporalError::InvalidSubject {
            subject: format!(
                "{} is not defined for {}",
                def.fact_type,
                input.subject.kind().as_str()
            ),
        });
    }
    if input.source.trim().is_empty() {
        return Err(TemporalError::InvalidFactValue {
            fact_type: input.fact_type.clone(),
            detail: "source must not be empty".to_string(),
        });
    }

    let value = match input.value.clone() {
        FactValue::MarketPrice(value) => FactValue::MarketPrice(value.normalize()),
        other => other,
    };
    value.validate(def)?;

    if def.storage_kind == StorageKind::MarketObservation {
        return write_market_price(
            pool,
            input,
            value,
            def.observation_kind,
            def.interval_seconds,
        )
        .await;
    }

    write_scalar(pool, input, value, def.schema_version).await
}

async fn write_scalar(
    pool: &PgPool,
    input: FactInput,
    value: FactValue,
    schema_version: i32,
) -> Result<(FactPoint, bool), TemporalError> {
    let hash = content_hash(
        input.subject,
        &input.fact_type,
        input.valid_at,
        &input.source,
        &value,
    );
    let current = current_belief(
        pool,
        input.subject,
        &input.fact_type,
        input.valid_at,
        &input.source,
    )
    .await?;

    if let Some((id, known_at, current_hash, retracted)) = current.as_ref() {
        if *current_hash == hash && !*retracted {
            let point = read::fact_point_by_id(pool, *id)
                .await?
                .ok_or(TemporalError::MissingPayload { fact_id: *id })?;
            return Ok((point, false));
        }
        if input.known_at.as_datetime() <= *known_at {
            return Err(TemporalError::conflict(format!(
                "{} {} {} at {} from {}",
                input.subject.subject_name(),
                input.subject.id(),
                input.fact_type,
                input.valid_at,
                input.source
            )));
        }
    }

    let id = new_uuid();
    let (asset_id, issuer_id, portfolio_id) = input.subject.columns();
    let mut tx = pool.begin().await?;

    sqlx::query(
        r#"
        INSERT INTO temporal_facts
            (id, asset_id, issuer_id, portfolio_id, fact_type, valid_at,
             known_at, source, retracted, retraction_reason, content_hash)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, FALSE, NULL, $9)
        "#,
    )
    .bind(id)
    .bind(asset_id)
    .bind(issuer_id)
    .bind(portfolio_id)
    .bind(&input.fact_type)
    .bind(input.valid_at.as_datetime())
    .bind(input.known_at.as_datetime())
    .bind(&input.source)
    .bind(&hash)
    .execute(&mut *tx)
    .await?;

    insert_payload(&mut tx, &value, id).await?;
    tx.commit().await?;

    Ok((
        FactPoint {
            id,
            subject: input.subject,
            fact_type: input.fact_type,
            schema_version,
            valid_at: input.valid_at,
            known_at: input.known_at,
            value: Some(value),
            source: input.source,
            retracted: false,
            retraction_reason: None,
            content_hash: hash,
        },
        true,
    ))
}

async fn insert_payload(
    tx: &mut Transaction<'_, Postgres>,
    value: &FactValue,
    fact_id: Uuid,
) -> Result<(), TemporalError> {
    match value {
        FactValue::Text { value } => {
            sqlx::query("INSERT INTO fact_text_values (fact_id, value) VALUES ($1, $2)")
                .bind(fact_id)
                .bind(value)
                .execute(&mut **tx)
                .await?;
        }
        FactValue::Decimal { value } => {
            sqlx::query("INSERT INTO fact_decimal_values (fact_id, value) VALUES ($1, $2)")
                .bind(fact_id)
                .bind(value)
                .execute(&mut **tx)
                .await?;
        }
        FactValue::Integer { value } => {
            sqlx::query("INSERT INTO fact_integer_values (fact_id, value) VALUES ($1, $2)")
                .bind(fact_id)
                .bind(value)
                .execute(&mut **tx)
                .await?;
        }
        FactValue::Boolean { value } => {
            sqlx::query("INSERT INTO fact_boolean_values (fact_id, value) VALUES ($1, $2)")
                .bind(fact_id)
                .bind(value)
                .execute(&mut **tx)
                .await?;
        }
        FactValue::Date { value } => {
            sqlx::query("INSERT INTO fact_date_values (fact_id, value) VALUES ($1, $2)")
                .bind(fact_id)
                .bind(value)
                .execute(&mut **tx)
                .await?;
        }
        FactValue::Timestamp { value } => {
            sqlx::query("INSERT INTO fact_timestamp_values (fact_id, value) VALUES ($1, $2)")
                .bind(fact_id)
                .bind(value)
                .execute(&mut **tx)
                .await?;
        }
        FactValue::MarketPrice(_) => {
            return Err(TemporalError::InvalidFactValue {
                fact_type: "market_price".to_string(),
                detail: "market prices are stored in market_observations".to_string(),
            });
        }
    }
    Ok(())
}

async fn write_market_price(
    pool: &PgPool,
    input: FactInput,
    value: FactValue,
    observation_kind: Option<&'static str>,
    interval_seconds: Option<i32>,
) -> Result<(FactPoint, bool), TemporalError> {
    let FactValue::MarketPrice(price) = value else {
        return Err(TemporalError::InvalidFactValue {
            fact_type: input.fact_type,
            detail: "expected a market_price value".to_string(),
        });
    };
    let FactSubject::Asset(asset_id) = input.subject else {
        return Err(TemporalError::InvalidSubject {
            subject: "market_price is only defined for assets".to_string(),
        });
    };
    let (observation, inserted) = append_observation_outcome(
        pool,
        NewMarketObservation {
            asset_id,
            kind: observation_kind.unwrap_or("eod").to_string(),
            interval_seconds: interval_seconds.unwrap_or(0),
            valid_at: input.valid_at,
            known_at: input.known_at,
            source: input.source,
            open: Some(price.open),
            high: Some(price.high),
            low: Some(price.low),
            close: Some(price.close),
            adjclose: price.adjclose,
            price: None,
            volume: price.volume,
            currency: price.currency,
            adjustment: price.adjustment,
            sequence: None,
        },
    )
    .await?;
    Ok((observation.to_fact_point()?, inserted))
}

/// Appends a retraction belief.
pub async fn retract_fact(
    pool: &PgPool,
    input: RetractionInput,
) -> Result<FactPoint, TemporalError> {
    let def = fact_type(&input.fact_type).ok_or_else(|| TemporalError::UnknownFactType {
        fact_type: input.fact_type.clone(),
    })?;
    if !def.applies_to(input.subject.kind()) {
        return Err(TemporalError::InvalidSubject {
            subject: format!(
                "{} is not defined for {}",
                def.fact_type,
                input.subject.kind().as_str()
            ),
        });
    }
    if input.reason.trim().is_empty() {
        return Err(TemporalError::InvalidFactValue {
            fact_type: input.fact_type.clone(),
            detail: "retraction reason must not be empty".to_string(),
        });
    }

    if def.storage_kind == StorageKind::MarketObservation {
        return retract_market_price(pool, input, def.observation_kind, def.interval_seconds).await;
    }

    let hash = retraction_hash(
        input.subject,
        &input.fact_type,
        input.valid_at,
        &input.source,
        &input.reason,
    );
    let current = current_belief(
        pool,
        input.subject,
        &input.fact_type,
        input.valid_at,
        &input.source,
    )
    .await?;

    let Some((id, known_at, current_hash, retracted)) = current else {
        return Err(TemporalError::NoValue {
            subject: format!("{} {}", input.subject.subject_name(), input.fact_type),
            at: input.valid_at.to_string(),
        });
    };
    if current_hash == hash && retracted {
        return read::fact_point_by_id(pool, id)
            .await?
            .ok_or(TemporalError::MissingPayload { fact_id: id });
    }
    if input.known_at.as_datetime() <= known_at {
        return Err(TemporalError::conflict(format!(
            "{} {} {} at {} from {}",
            input.subject.subject_name(),
            input.subject.id(),
            input.fact_type,
            input.valid_at,
            input.source
        )));
    }

    let id = new_uuid();
    let (asset_id, issuer_id, portfolio_id) = input.subject.columns();
    let mut tx = pool.begin().await?;
    sqlx::query(
        r#"
        INSERT INTO temporal_facts
            (id, asset_id, issuer_id, portfolio_id, fact_type, valid_at,
             known_at, source, retracted, retraction_reason, content_hash)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, TRUE, $9, $10)
        "#,
    )
    .bind(id)
    .bind(asset_id)
    .bind(issuer_id)
    .bind(portfolio_id)
    .bind(&input.fact_type)
    .bind(input.valid_at.as_datetime())
    .bind(input.known_at.as_datetime())
    .bind(&input.source)
    .bind(&input.reason)
    .bind(&hash)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(FactPoint {
        id,
        subject: input.subject,
        fact_type: input.fact_type,
        schema_version: def.schema_version,
        valid_at: input.valid_at,
        known_at: input.known_at,
        value: None,
        source: input.source,
        retracted: true,
        retraction_reason: Some(input.reason),
        content_hash: hash,
    })
}

async fn retract_market_price(
    pool: &PgPool,
    input: RetractionInput,
    observation_kind: Option<&'static str>,
    interval_seconds: Option<i32>,
) -> Result<FactPoint, TemporalError> {
    let FactSubject::Asset(asset_id) = input.subject else {
        return Err(TemporalError::InvalidSubject {
            subject: "market_price is only defined for assets".to_string(),
        });
    };
    let kind = observation_kind.unwrap_or("eod");
    let interval = interval_seconds.unwrap_or(0);
    let current: Option<(String, String)> = sqlx::query_as(
        r#"
        SELECT currency, adjustment FROM market_observations
        WHERE asset_id = $1 AND kind = $2 AND interval_seconds = $3
        ORDER BY known_at DESC, id DESC LIMIT 1
        "#,
    )
    .bind(asset_id.as_uuid())
    .bind(kind)
    .bind(interval)
    .fetch_optional(pool)
    .await?;
    let (currency, adjustment) = current.unwrap_or_else(|| ("USD".to_string(), "none".to_string()));
    let adjustment = crate::temporal::facts::value::Adjustment::from_wire(&adjustment)
        .unwrap_or(crate::temporal::facts::value::Adjustment::None);

    let observation = retract_observation(
        pool,
        NewMarketObservation {
            asset_id,
            kind: kind.to_string(),
            interval_seconds: interval,
            valid_at: input.valid_at,
            known_at: input.known_at,
            source: input.source,
            open: None,
            high: None,
            low: None,
            close: None,
            adjclose: None,
            price: None,
            volume: None,
            currency,
            adjustment,
            sequence: None,
        },
        &input.reason,
    )
    .await?;
    observation.to_fact_point()
}

async fn current_belief(
    pool: &PgPool,
    subject: FactSubject,
    fact_type: &str,
    valid_at: ValidAt,
    source: &str,
) -> Result<Option<(Uuid, chrono::DateTime<Utc>, String, bool)>, TemporalError> {
    let row: Option<(Uuid, chrono::DateTime<Utc>, String, bool)> = sqlx::query_as(
        r#"
        SELECT id, known_at, content_hash, retracted
        FROM temporal_facts
        WHERE subject_kind = $1 AND subject_id = $2 AND fact_type = $3
          AND valid_at = $4 AND source = $5
        ORDER BY known_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(subject.kind().as_str())
    .bind(subject.id())
    .bind(fact_type)
    .bind(valid_at.as_datetime())
    .bind(source)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}
