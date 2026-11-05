//! The fact catalog: the machine-readable data dictionary.
//!
//! The Rust catalog is the source of truth. [`crate::temporal::catalog::sync_fact_catalog`] projects it
//! into `fact_types`, `fact_type_fields`, and `fact_type_subjects`, which are
//! used for SQL introspection and the HTTP data-dictionary endpoint.
//!
//! A fact type names a storage shape, not a provider. Adding a data source
//! never changes this catalog; adding a genuinely new fact kind does.

use sqlx::PgPool;

use crate::temporal::error::TemporalError;

/// The kind of subject a fact can describe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubjectKind {
    /// An asset.
    Asset,
    /// An issuer.
    Issuer,
    /// A portfolio.
    Portfolio,
}

impl SubjectKind {
    /// The stable wire/SQL value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Asset => "asset",
            Self::Issuer => "issuer",
            Self::Portfolio => "portfolio",
        }
    }

    /// Parses a stable wire/SQL value.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "asset" => Some(Self::Asset),
            "issuer" => Some(Self::Issuer),
            "portfolio" => Some(Self::Portfolio),
            _ => None,
        }
    }
}

/// The physical shape of a fact's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageKind {
    /// A `TEXT` scalar in `fact_text_values`.
    Text,
    /// A `NUMERIC(38,18)` scalar in `fact_decimal_values`.
    Decimal,
    /// A `BIGINT` scalar in `fact_integer_values`.
    Integer,
    /// A `BOOLEAN` scalar in `fact_boolean_values`.
    Boolean,
    /// A `DATE` scalar in `fact_date_values`.
    Date,
    /// A `TIMESTAMPTZ` scalar in `fact_timestamp_values`.
    Timestamp,
    /// A typed row in `market_observations`.
    MarketObservation,
}

impl StorageKind {
    /// The stable SQL/wire value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Decimal => "decimal",
            Self::Integer => "integer",
            Self::Boolean => "boolean",
            Self::Date => "date",
            Self::Timestamp => "timestamp",
            Self::MarketObservation => "market_observation",
        }
    }

    /// The payload table used by scalar kinds.
    pub const fn payload_table(self) -> Option<&'static str> {
        match self {
            Self::Text => Some("fact_text_values"),
            Self::Decimal => Some("fact_decimal_values"),
            Self::Integer => Some("fact_integer_values"),
            Self::Boolean => Some("fact_boolean_values"),
            Self::Date => Some("fact_date_values"),
            Self::Timestamp => Some("fact_timestamp_values"),
            Self::MarketObservation => None,
        }
    }
}

/// One field of a fact type.
#[derive(Debug, Clone, Copy)]
pub struct FactFieldDef {
    /// Field name within the value object.
    pub name: &'static str,
    /// Logical data type (`text`, `decimal`, `integer`, `boolean`, `date`,
    /// `timestamp`).
    pub data_type: &'static str,
    /// Whether the field may be absent.
    pub nullable: bool,
    /// Unit of measure, when meaningful.
    pub unit: Option<&'static str>,
    /// Decimal precision, when meaningful.
    pub precision: Option<i32>,
    /// Decimal scale, when meaningful.
    pub scale: Option<i32>,
    /// Minimum value as a decimal string, when meaningful.
    pub min_value: Option<&'static str>,
    /// Maximum value as a decimal string, when meaningful.
    pub max_value: Option<&'static str>,
    /// Closed value vocabulary, when the field is an enum-like value.
    pub allowed_values: &'static [&'static str],
    /// Aggregates that apply to this field.
    pub aggregates: &'static [&'static str],
    /// Human-readable description.
    pub description: &'static str,
}

/// One fact type in the catalog.
#[derive(Debug, Clone, Copy)]
pub struct FactTypeDef {
    /// Stable fact type key.
    pub fact_type: &'static str,
    /// Human-readable label.
    pub display_name: &'static str,
    /// One-sentence description.
    pub description: &'static str,
    /// Physical storage shape.
    pub storage_kind: StorageKind,
    /// Aggregate that applies to the fact as a whole.
    pub aggregation: &'static str,
    /// Unit of the fact as a whole, when meaningful.
    pub unit: Option<&'static str>,
    /// Observation kind for `MarketObservation` facts.
    pub observation_kind: Option<&'static str>,
    /// Observation interval for `MarketObservation` facts.
    pub interval_seconds: Option<i32>,
    /// Catalog schema version.
    pub schema_version: i32,
    /// Subject kinds this fact applies to.
    pub subjects: &'static [SubjectKind],
    /// Typed fields of the value.
    pub fields: &'static [FactFieldDef],
}

impl FactTypeDef {
    /// Whether this fact type applies to `subject`.
    pub fn applies_to(&self, subject: SubjectKind) -> bool {
        self.subjects.contains(&subject)
    }
}

const MARKET_PRICE_FIELDS: &[FactFieldDef] = &[
    FactFieldDef {
        name: "open",
        data_type: "decimal",
        nullable: false,
        unit: Some("USD"),
        precision: Some(38),
        scale: Some(18),
        min_value: Some("0"),
        max_value: None,
        allowed_values: &[],
        aggregates: &["min", "max", "last"],
        description: "Opening price for the observation window.",
    },
    FactFieldDef {
        name: "high",
        data_type: "decimal",
        nullable: false,
        unit: Some("USD"),
        precision: Some(38),
        scale: Some(18),
        min_value: Some("0"),
        max_value: None,
        allowed_values: &[],
        aggregates: &["min", "max", "last"],
        description: "Highest traded price for the observation window.",
    },
    FactFieldDef {
        name: "low",
        data_type: "decimal",
        nullable: false,
        unit: Some("USD"),
        precision: Some(38),
        scale: Some(18),
        min_value: Some("0"),
        max_value: None,
        allowed_values: &[],
        aggregates: &["min", "max", "last"],
        description: "Lowest traded price for the observation window.",
    },
    FactFieldDef {
        name: "close",
        data_type: "decimal",
        nullable: false,
        unit: Some("USD"),
        precision: Some(38),
        scale: Some(18),
        min_value: Some("0"),
        max_value: None,
        allowed_values: &[],
        aggregates: &["min", "max", "last"],
        description: "Unadjusted closing price for the observation window.",
    },
    FactFieldDef {
        name: "adjclose",
        data_type: "decimal",
        nullable: true,
        unit: Some("USD"),
        precision: Some(38),
        scale: Some(18),
        min_value: Some("0"),
        max_value: None,
        allowed_values: &[],
        aggregates: &["min", "max", "last"],
        description: "Split/dividend-adjusted closing price, when supplied.",
    },
    FactFieldDef {
        name: "volume",
        data_type: "decimal",
        nullable: true,
        unit: Some("shares"),
        precision: Some(38),
        scale: Some(0),
        min_value: Some("0"),
        max_value: None,
        allowed_values: &[],
        aggregates: &["sum", "min", "max", "last"],
        description: "Traded volume in shares or contracts.",
    },
    FactFieldDef {
        name: "currency",
        data_type: "text",
        nullable: false,
        unit: None,
        precision: None,
        scale: None,
        min_value: None,
        max_value: None,
        allowed_values: &[],
        aggregates: &["none"],
        description: "ISO 4217 quote currency.",
    },
    FactFieldDef {
        name: "adjustment",
        data_type: "text",
        nullable: false,
        unit: None,
        precision: None,
        scale: None,
        min_value: None,
        max_value: None,
        allowed_values: &["adjusted", "unadjusted"],
        aggregates: &["none"],
        description: "Basis of the close/adjclose pair.",
    },
];

const VALUE_TEXT_FIELDS: &[FactFieldDef] = &[FactFieldDef {
    name: "value",
    data_type: "text",
    nullable: false,
    unit: None,
    precision: None,
    scale: None,
    min_value: None,
    max_value: None,
    allowed_values: &[],
    aggregates: &["none"],
    description: "The text value of the fact.",
}];

const VALUE_DECIMAL_FIELDS: &[FactFieldDef] = &[FactFieldDef {
    name: "value",
    data_type: "decimal",
    nullable: false,
    unit: None,
    precision: Some(38),
    scale: Some(18),
    min_value: None,
    max_value: None,
    allowed_values: &[],
    aggregates: &["last", "min", "max", "sum", "avg"],
    description: "The decimal value of the fact.",
}];

const VALUE_BOOLEAN_FIELDS: &[FactFieldDef] = &[FactFieldDef {
    name: "value",
    data_type: "boolean",
    nullable: false,
    unit: None,
    precision: None,
    scale: None,
    min_value: None,
    max_value: None,
    allowed_values: &[],
    aggregates: &["last", "count"],
    description: "The boolean value of the fact.",
}];

const VALUE_INTEGER_FIELDS: &[FactFieldDef] = &[FactFieldDef {
    name: "value",
    data_type: "integer",
    nullable: false,
    unit: None,
    precision: None,
    scale: None,
    min_value: None,
    max_value: None,
    allowed_values: &[],
    aggregates: &["last", "min", "max", "sum"],
    description: "The integer value of the fact.",
}];

const VALUE_TIMESTAMP_FIELDS: &[FactFieldDef] = &[FactFieldDef {
    name: "value",
    data_type: "timestamp",
    nullable: false,
    unit: None,
    precision: None,
    scale: None,
    min_value: None,
    max_value: None,
    allowed_values: &[],
    aggregates: &["min", "max", "last"],
    description: "The timestamp value of the fact.",
}];

const VALUE_DATE_FIELDS: &[FactFieldDef] = &[FactFieldDef {
    name: "value",
    data_type: "date",
    nullable: false,
    unit: None,
    precision: None,
    scale: None,
    min_value: None,
    max_value: None,
    allowed_values: &[],
    aggregates: &["min", "max", "last"],
    description: "The date value of the fact.",
}];

const CATALOG: &[FactTypeDef] = &[
    FactTypeDef {
        fact_type: "market_price",
        display_name: "Market price",
        description: "End-of-day OHLCV bar for an asset.",
        storage_kind: StorageKind::MarketObservation,
        aggregation: "last",
        unit: Some("USD"),
        observation_kind: Some("eod"),
        interval_seconds: Some(0),
        schema_version: 1,
        subjects: &[SubjectKind::Asset],
        fields: MARKET_PRICE_FIELDS,
    },
    FactTypeDef {
        fact_type: "name",
        display_name: "Name",
        description: "A source-reported name for an asset, issuer, or portfolio.",
        storage_kind: StorageKind::Text,
        aggregation: "none",
        unit: None,
        observation_kind: None,
        interval_seconds: None,
        schema_version: 1,
        subjects: &[
            SubjectKind::Asset,
            SubjectKind::Issuer,
            SubjectKind::Portfolio,
        ],
        fields: VALUE_TEXT_FIELDS,
    },
    FactTypeDef {
        fact_type: "gics_sector",
        display_name: "GICS sector",
        description: "The asset's GICS sector classification.",
        storage_kind: StorageKind::Text,
        aggregation: "none",
        unit: None,
        observation_kind: None,
        interval_seconds: None,
        schema_version: 1,
        subjects: &[SubjectKind::Asset],
        fields: VALUE_TEXT_FIELDS,
    },
    FactTypeDef {
        fact_type: "gics_industry",
        display_name: "GICS industry",
        description: "The asset's GICS industry classification.",
        storage_kind: StorageKind::Text,
        aggregation: "none",
        unit: None,
        observation_kind: None,
        interval_seconds: None,
        schema_version: 1,
        subjects: &[SubjectKind::Asset],
        fields: VALUE_TEXT_FIELDS,
    },
    FactTypeDef {
        fact_type: "nav",
        display_name: "Net asset value",
        description: "A fund's net asset value per unit.",
        storage_kind: StorageKind::Decimal,
        aggregation: "last",
        unit: Some("USD"),
        observation_kind: None,
        interval_seconds: None,
        schema_version: 1,
        subjects: &[SubjectKind::Asset],
        fields: VALUE_DECIMAL_FIELDS,
    },
    FactTypeDef {
        fact_type: "credit_rating",
        display_name: "Credit rating",
        description: "An issuer's reported credit rating.",
        storage_kind: StorageKind::Text,
        aggregation: "last",
        unit: None,
        observation_kind: None,
        interval_seconds: None,
        schema_version: 1,
        subjects: &[SubjectKind::Issuer],
        fields: VALUE_TEXT_FIELDS,
    },
    FactTypeDef {
        fact_type: "benchmark",
        display_name: "Benchmark",
        description: "A portfolio's declared benchmark.",
        storage_kind: StorageKind::Text,
        aggregation: "none",
        unit: None,
        observation_kind: None,
        interval_seconds: None,
        schema_version: 1,
        subjects: &[SubjectKind::Portfolio],
        fields: VALUE_TEXT_FIELDS,
    },
    FactTypeDef {
        fact_type: "strategy_label",
        display_name: "Strategy label",
        description: "A portfolio's strategy classification.",
        storage_kind: StorageKind::Text,
        aggregation: "none",
        unit: None,
        observation_kind: None,
        interval_seconds: None,
        schema_version: 1,
        subjects: &[SubjectKind::Portfolio],
        fields: VALUE_TEXT_FIELDS,
    },
    FactTypeDef {
        fact_type: "is_active",
        display_name: "Active flag",
        description: "Whether the subject is currently active.",
        storage_kind: StorageKind::Boolean,
        aggregation: "last",
        unit: None,
        observation_kind: None,
        interval_seconds: None,
        schema_version: 1,
        subjects: &[
            SubjectKind::Asset,
            SubjectKind::Issuer,
            SubjectKind::Portfolio,
        ],
        fields: VALUE_BOOLEAN_FIELDS,
    },
    FactTypeDef {
        fact_type: "rank",
        display_name: "Rank",
        description: "A source-assigned integer rank for an asset.",
        storage_kind: StorageKind::Integer,
        aggregation: "last",
        unit: None,
        observation_kind: None,
        interval_seconds: None,
        schema_version: 1,
        subjects: &[SubjectKind::Asset],
        fields: VALUE_INTEGER_FIELDS,
    },
    FactTypeDef {
        fact_type: "last_verified_at",
        display_name: "Last verified at",
        description: "When the asset's reference data was last verified.",
        storage_kind: StorageKind::Timestamp,
        aggregation: "max",
        unit: None,
        observation_kind: None,
        interval_seconds: None,
        schema_version: 1,
        subjects: &[SubjectKind::Asset],
        fields: VALUE_TIMESTAMP_FIELDS,
    },
    FactTypeDef {
        fact_type: "inception_date",
        display_name: "Inception date",
        description: "A portfolio's reported inception date.",
        storage_kind: StorageKind::Date,
        aggregation: "min",
        unit: None,
        observation_kind: None,
        interval_seconds: None,
        schema_version: 1,
        subjects: &[SubjectKind::Portfolio],
        fields: VALUE_DATE_FIELDS,
    },
];

/// Returns the canonical fact catalog.
pub fn fact_catalog() -> &'static [FactTypeDef] {
    CATALOG
}

/// Looks up one fact type in the catalog.
pub fn fact_type(fact_type: &str) -> Option<&'static FactTypeDef> {
    CATALOG.iter().find(|entry| entry.fact_type == fact_type)
}

fn wire_schema(def: &FactTypeDef) -> serde_json::Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for field in def.fields {
        let (kind, format) = match field.data_type {
            "decimal" => ("string", Some("decimal")),
            "integer" => ("string", Some("int64")),
            "date" => ("string", Some("date")),
            "timestamp" => ("string", Some("date-time")),
            "boolean" => ("boolean", None),
            _ => ("string", None),
        };
        let mut property = serde_json::Map::new();
        property.insert("type".into(), serde_json::Value::String(kind.into()));
        if let Some(format) = format {
            property.insert("format".into(), serde_json::Value::String(format.into()));
        }
        if let Some(unit) = field.unit {
            property.insert("x-unit".into(), serde_json::Value::String(unit.into()));
        }
        if !field.allowed_values.is_empty() {
            property.insert(
                "enum".into(),
                serde_json::Value::Array(
                    field
                        .allowed_values
                        .iter()
                        .map(|value| serde_json::Value::String((*value).into()))
                        .collect(),
                ),
            );
        }
        properties.insert(field.name.into(), serde_json::Value::Object(property));
        if !field.nullable {
            required.push(serde_json::Value::String(field.name.into()));
        }
    }
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

/// Synchronizes the SQL catalog projection with [`fact_catalog`].
///
/// Idempotent: safe to call after every migration. Fact types missing from the
/// code catalog are marked inactive rather than deleted, so historical rows
/// remain readable.
pub async fn sync_fact_catalog(pool: &PgPool) -> Result<(), TemporalError> {
    let known: Vec<String> = CATALOG
        .iter()
        .map(|entry| entry.fact_type.to_string())
        .collect();

    for def in CATALOG {
        let wire = serde_json::to_string(&wire_schema(def))
            .map_err(|error| TemporalError::invalid_time(format!("wire schema: {error}")))?;
        sqlx::query(
            r#"
            INSERT INTO fact_types
                (fact_type, display_name, description, storage_kind, aggregation,
                 unit, observation_kind, interval_seconds, schema_version,
                 wire_schema, active)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10::jsonb, TRUE)
            ON CONFLICT (fact_type) DO UPDATE SET
                display_name = EXCLUDED.display_name,
                description = EXCLUDED.description,
                storage_kind = EXCLUDED.storage_kind,
                aggregation = EXCLUDED.aggregation,
                unit = EXCLUDED.unit,
                observation_kind = EXCLUDED.observation_kind,
                interval_seconds = EXCLUDED.interval_seconds,
                schema_version = EXCLUDED.schema_version,
                wire_schema = EXCLUDED.wire_schema,
                active = TRUE
            "#,
        )
        .bind(def.fact_type)
        .bind(def.display_name)
        .bind(def.description)
        .bind(def.storage_kind.as_str())
        .bind(def.aggregation)
        .bind(def.unit)
        .bind(def.observation_kind)
        .bind(def.interval_seconds)
        .bind(def.schema_version)
        .bind(wire)
        .execute(pool)
        .await?;

        sqlx::query("DELETE FROM fact_type_fields WHERE fact_type = $1")
            .bind(def.fact_type)
            .execute(pool)
            .await?;
        for (ordinal, field) in def.fields.iter().enumerate() {
            sqlx::query(
                r#"
                INSERT INTO fact_type_fields
                    (fact_type, ordinal, field_name, data_type, nullable, unit,
                     precision, scale, min_value, max_value, allowed_values,
                     aggregates, description)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::numeric, $10::numeric,
                        $11, $12, $13)
                "#,
            )
            .bind(def.fact_type)
            .bind(i32::try_from(ordinal).unwrap_or(i32::MAX))
            .bind(field.name)
            .bind(field.data_type)
            .bind(field.nullable)
            .bind(field.unit)
            .bind(field.precision)
            .bind(field.scale)
            .bind(field.min_value)
            .bind(field.max_value)
            .bind(field.allowed_values)
            .bind(field.aggregates)
            .bind(field.description)
            .execute(pool)
            .await?;
        }

        sqlx::query("DELETE FROM fact_type_subjects WHERE fact_type = $1")
            .bind(def.fact_type)
            .execute(pool)
            .await?;
        for subject in def.subjects {
            sqlx::query("INSERT INTO fact_type_subjects (fact_type, subject_kind) VALUES ($1, $2)")
                .bind(def.fact_type)
                .bind(subject.as_str())
                .execute(pool)
                .await?;
        }
    }

    sqlx::query("UPDATE fact_types SET active = FALSE WHERE NOT (fact_type = ANY($1))")
        .bind(&known)
        .execute(pool)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_keys_are_unique_and_non_empty() {
        let mut seen = std::collections::HashSet::new();
        for def in fact_catalog() {
            assert!(!def.fact_type.is_empty());
            assert!(seen.insert(def.fact_type), "duplicate {}", def.fact_type);
            assert!(!def.fields.is_empty(), "{} has no fields", def.fact_type);
            assert!(
                def.fields.iter().all(|field| !field.name.is_empty()),
                "{} has an empty field",
                def.fact_type
            );
        }
    }

    #[test]
    fn market_price_is_an_eod_observation() {
        let price = fact_type("market_price").expect("market_price");
        assert_eq!(price.storage_kind, StorageKind::MarketObservation);
        assert_eq!(price.observation_kind, Some("eod"));
        assert_eq!(price.interval_seconds, Some(0));
        assert!(price.applies_to(SubjectKind::Asset));
        assert!(!price.applies_to(SubjectKind::Issuer));
    }

    #[test]
    fn scalar_fact_types_have_one_field_named_value() {
        for def in fact_catalog() {
            if def.storage_kind != StorageKind::MarketObservation {
                assert_eq!(def.fields.len(), 1, "{}", def.fact_type);
                assert_eq!(def.fields[0].name, "value", "{}", def.fact_type);
            }
        }
    }
}
