//! Self-describing artifact for the high-level domain model.
//!
//! Implements FR-014 of `specs/016-implement-domain-model/spec.md`. The
//! `model_description()` function returns a structured description of the
//! model — every entity, every field, every per-field aggregate, every
//! relationship, and every invariant — so downstream tools can consume the
//! model without parsing prose or SQL.

use serde::Serialize;

/// A canonical description of the high-level domain model.
#[derive(Debug, Clone, Serialize)]
/// Implements FR-014 of spec 016-implement-domain-model.
pub struct ModelDescription {
    /// The entities in the model.
    pub entities: Vec<EntityDescription>,
    /// The relationships between entities.
    pub relationships: Vec<RelationshipDescription>,
    /// The invariants the model relies on.
    pub invariants: Vec<&'static str>,
}

/// A description of a single entity.
#[derive(Debug, Clone, Serialize)]
/// Implements FR-014 of spec 016-implement-domain-model.
pub struct EntityDescription {
    /// The entity name (e.g., `"Portfolio"`).
    pub name: &'static str,
    /// A one-sentence description of the entity.
    pub description: &'static str,
    /// The fields of the entity.
    pub fields: Vec<FieldDescription>,
}

/// A description of a single field.
#[derive(Debug, Clone, Serialize)]
/// Implements FR-014 of spec 016-implement-domain-model.
pub struct FieldDescription {
    /// The field name (e.g., `"name"`).
    pub name: &'static str,
    /// The field type in domain terms (e.g., `"text"`, `"decimal"`, `"uuid"`).
    pub type_domain: &'static str,
    /// The declared aggregate set (drawn from `{none, sum, avg, min, max, last, count}`).
    pub aggregates: &'static [&'static str],
    /// Whether the field may be absent or null in the persisted row.
    pub nullable: bool,
    /// Additional constraints that the domain model declares for the field.
    pub constraints: &'static [FieldConstraint],
}

/// A constraint declared by the domain model for one field.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FieldConstraint {
    /// A standard string format such as `uuid`, `date`, or `date-time`.
    Format {
        /// The format name.
        format: &'static str,
    },
    /// A regular-expression pattern.
    Pattern {
        /// The pattern.
        pattern: &'static str,
    },
    /// A minimum string length.
    MinLength {
        /// The minimum length.
        value: u32,
    },
    /// A maximum string length.
    MaxLength {
        /// The maximum length.
        value: u32,
    },
    /// A numeric minimum.
    Minimum {
        /// The minimum value.
        value: &'static str,
    },
    /// A numeric maximum.
    Maximum {
        /// The maximum value.
        value: &'static str,
    },
    /// A closed value vocabulary.
    AllowedValues {
        /// The accepted values.
        values: &'static [&'static str],
    },
    /// An open value vocabulary whose well-known values are informational.
    KnownValues {
        /// The well-known values.
        values: &'static [&'static str],
    },
    /// A cross-field rule name.
    CrossField {
        /// The rule name.
        rule: &'static str,
    },
}

/// A description of a single relationship between two entities.
#[derive(Debug, Clone, Serialize)]
/// Implements FR-014 of spec 016-implement-domain-model.
pub struct RelationshipDescription {
    /// The source entity name.
    pub from: &'static str,
    /// The destination entity name.
    pub to: &'static str,
    /// The verb-phrase label of the relationship.
    pub label: &'static str,
    /// The cardinality on the `from` side (e.g., `"1"`, `"0..*"`).
    pub cardinality_from: &'static str,
    /// The cardinality on the `to` side (e.g., `"0..*"`, `"1"`).
    pub cardinality_to: &'static str,
}

/// Return a canonical description of the high-level domain model.
///
/// This function is the source of truth for the model from the Rust side; it
/// is the machine-checkable counterpart of the predecessor's self-describing
/// JSON shape (`specs/015-database-uml-domains/contracts/domain-contract.md`).
/// Implements FR-014 of spec 016-implement-domain-model.
pub fn model_description() -> ModelDescription {
    ModelDescription {
        entities: vec![
            EntityDescription {
                name: "Portfolio",
                description: "A node in the portfolio DAG that groups positions and rules.",
                fields: vec![
                    FieldDescription {
                        name: "id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format { format: "uuid" }],
                    },
                    FieldDescription {
                        name: "name",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::MinLength { value: 1 }],
                    },
                    FieldDescription {
                        name: "purpose",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::MinLength { value: 1 }],
                    },
                    FieldDescription {
                        name: "created_at",
                        type_domain: "timestamp",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format {
                            format: "date-time",
                        }],
                    },
                    FieldDescription {
                        name: "created_by",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::MinLength { value: 1 }],
                    },
                    FieldDescription {
                        name: "parent_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                        nullable: true,
                        constraints: &[FieldConstraint::Format { format: "uuid" }],
                    },
                ],
            },
            EntityDescription {
                name: "Asset",
                description: "A unit of legal property that can be held in a portfolio.",
                fields: vec![
                    FieldDescription {
                        name: "id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format { format: "uuid" }],
                    },
                    FieldDescription {
                        name: "asset_type",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::MinLength { value: 1 }],
                    },
                    FieldDescription {
                        name: "display_name",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::MinLength { value: 1 }],
                    },
                    FieldDescription {
                        name: "currency",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Pattern {
                            pattern: "^[A-Z]{3}$",
                        }],
                    },
                    FieldDescription {
                        name: "created_at",
                        type_domain: "timestamp",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format {
                            format: "date-time",
                        }],
                    },
                    FieldDescription {
                        name: "backed_by_portfolio_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                        nullable: true,
                        constraints: &[FieldConstraint::Format { format: "uuid" }],
                    },
                    FieldDescription {
                        name: "issuer_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                        nullable: true,
                        constraints: &[FieldConstraint::Format { format: "uuid" }],
                    },
                ],
            },
            EntityDescription {
                name: "Position",
                description: "The quantity of one asset held by one portfolio at an instant.",
                fields: vec![
                    FieldDescription {
                        name: "portfolio_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format { format: "uuid" }],
                    },
                    FieldDescription {
                        name: "asset_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format { format: "uuid" }],
                    },
                    FieldDescription {
                        name: "quantity",
                        type_domain: "decimal",
                        aggregates: &["sum", "avg", "min", "max", "last"],
                        nullable: false,
                        constraints: &[FieldConstraint::Pattern {
                            pattern: r"^-?(?:0|[1-9][0-9]{0,19})(?:\.[0-9]{1,18})?$",
                        }],
                    },
                    FieldDescription {
                        name: "as_of",
                        type_domain: "timestamp",
                        aggregates: &["min", "max", "last"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format {
                            format: "date-time",
                        }],
                    },
                ],
            },
            EntityDescription {
                name: "Event",
                description: "An immutable record of a state change to a position.",
                fields: vec![
                    FieldDescription {
                        name: "id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format { format: "uuid" }],
                    },
                    FieldDescription {
                        name: "event_type",
                        type_domain: "text",
                        aggregates: &["count"],
                        nullable: false,
                        constraints: &[FieldConstraint::AllowedValues {
                            values: &[
                                "buy",
                                "sell",
                                "transfer",
                                "split",
                                "merger",
                                "spin-off",
                                "dividend",
                                "stock-based-compensation",
                            ],
                        }],
                    },
                    FieldDescription {
                        name: "effective_date",
                        type_domain: "date",
                        aggregates: &["min", "max", "last"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format { format: "date" }],
                    },
                    FieldDescription {
                        name: "quantity_delta",
                        type_domain: "decimal",
                        aggregates: &["sum", "avg", "min", "max"],
                        nullable: false,
                        constraints: &[FieldConstraint::Pattern {
                            pattern: r"^-?(?:0|[1-9][0-9]{0,19})(?:\.[0-9]{1,18})?$",
                        }],
                    },
                    FieldDescription {
                        name: "recorded_at",
                        type_domain: "timestamp",
                        aggregates: &["min", "max", "last"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format {
                            format: "date-time",
                        }],
                    },
                    FieldDescription {
                        name: "portfolio_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format { format: "uuid" }],
                    },
                    FieldDescription {
                        name: "asset_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format { format: "uuid" }],
                    },
                    FieldDescription {
                        name: "to_portfolio_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                        nullable: true,
                        constraints: &[FieldConstraint::Format { format: "uuid" }],
                    },
                ],
            },
            EntityDescription {
                name: "Issuer",
                description: "The legal entity that issues or guarantees assets.",
                fields: vec![
                    FieldDescription {
                        name: "id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format { format: "uuid" }],
                    },
                    FieldDescription {
                        name: "name",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::MinLength { value: 1 }],
                    },
                    FieldDescription {
                        name: "issuer_type",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::MinLength { value: 1 }],
                    },
                    FieldDescription {
                        name: "legal_jurisdiction",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::MinLength { value: 1 }],
                    },
                    FieldDescription {
                        name: "created_at",
                        type_domain: "timestamp",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format {
                            format: "date-time",
                        }],
                    },
                    // US2b: parent_id for the issuer DAG
                    FieldDescription {
                        name: "parent_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                        nullable: true,
                        constraints: &[FieldConstraint::Format { format: "uuid" }],
                    },
                ],
            },
            // Feature 027: the normalized fact model. One envelope holds
            // identity, time and provenance; the catalog defines the value.
            EntityDescription {
                name: "TemporalFact",
                description: "An append-only bitemporal belief about a subject.",
                fields: vec![
                    FieldDescription {
                        name: "id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format { format: "uuid" }],
                    },
                    FieldDescription {
                        name: "subject_kind",
                        type_domain: "text",
                        aggregates: &["count"],
                        nullable: false,
                        constraints: &[FieldConstraint::AllowedValues {
                            values: &["asset", "issuer", "portfolio"],
                        }],
                    },
                    FieldDescription {
                        name: "subject_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format { format: "uuid" }],
                    },
                    FieldDescription {
                        name: "fact_type",
                        type_domain: "text",
                        aggregates: &["count"],
                        nullable: false,
                        constraints: &[],
                    },
                    FieldDescription {
                        name: "valid_at",
                        type_domain: "timestamp",
                        aggregates: &["min", "max"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format {
                            format: "date-time",
                        }],
                    },
                    FieldDescription {
                        name: "known_at",
                        type_domain: "timestamp",
                        aggregates: &["min", "max", "last"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format {
                            format: "date-time",
                        }],
                    },
                    FieldDescription {
                        name: "source",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::MinLength { value: 1 }],
                    },
                    FieldDescription {
                        name: "retracted",
                        type_domain: "boolean",
                        aggregates: &["count"],
                        nullable: false,
                        constraints: &[],
                    },
                    FieldDescription {
                        name: "retraction_reason",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: true,
                        constraints: &[],
                    },
                    FieldDescription {
                        name: "content_hash",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Pattern {
                            pattern: "^[0-9a-f]{64}$",
                        }],
                    },
                ],
            },
            EntityDescription {
                name: "FactType",
                description: "One fact type in the machine-readable catalog.",
                fields: vec![
                    FieldDescription {
                        name: "fact_type",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[],
                    },
                    FieldDescription {
                        name: "display_name",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::MinLength { value: 1 }],
                    },
                    FieldDescription {
                        name: "description",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::MinLength { value: 1 }],
                    },
                    FieldDescription {
                        name: "storage_kind",
                        type_domain: "text",
                        aggregates: &["count"],
                        nullable: false,
                        constraints: &[FieldConstraint::AllowedValues {
                            values: &[
                                "text",
                                "decimal",
                                "integer",
                                "boolean",
                                "date",
                                "timestamp",
                                "market_observation",
                            ],
                        }],
                    },
                    FieldDescription {
                        name: "aggregation",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[],
                    },
                    FieldDescription {
                        name: "unit",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: true,
                        constraints: &[],
                    },
                    FieldDescription {
                        name: "observation_kind",
                        type_domain: "text",
                        aggregates: &["count"],
                        nullable: true,
                        constraints: &[],
                    },
                    FieldDescription {
                        name: "interval_seconds",
                        type_domain: "integer",
                        aggregates: &["min", "max"],
                        nullable: true,
                        constraints: &[FieldConstraint::Minimum { value: "0" }],
                    },
                    FieldDescription {
                        name: "schema_version",
                        type_domain: "integer",
                        aggregates: &["max"],
                        nullable: false,
                        constraints: &[FieldConstraint::Minimum { value: "1" }],
                    },
                    FieldDescription {
                        name: "active",
                        type_domain: "boolean",
                        aggregates: &["count"],
                        nullable: false,
                        constraints: &[],
                    },
                ],
            },
            EntityDescription {
                name: "FactField",
                description: "One typed field of a fact type value.",
                fields: vec![
                    FieldDescription {
                        name: "fact_type",
                        type_domain: "text",
                        aggregates: &["count"],
                        nullable: false,
                        constraints: &[],
                    },
                    FieldDescription {
                        name: "field_name",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::MinLength { value: 1 }],
                    },
                    FieldDescription {
                        name: "ordinal",
                        type_domain: "integer",
                        aggregates: &["min", "max"],
                        nullable: false,
                        constraints: &[FieldConstraint::Minimum { value: "1" }],
                    },
                    FieldDescription {
                        name: "data_type",
                        type_domain: "text",
                        aggregates: &["count"],
                        nullable: false,
                        constraints: &[FieldConstraint::AllowedValues {
                            values: &["text", "decimal", "integer", "boolean", "date", "timestamp"],
                        }],
                    },
                    FieldDescription {
                        name: "nullable",
                        type_domain: "boolean",
                        aggregates: &["count"],
                        nullable: false,
                        constraints: &[],
                    },
                    FieldDescription {
                        name: "unit",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: true,
                        constraints: &[],
                    },
                    FieldDescription {
                        name: "precision",
                        type_domain: "integer",
                        aggregates: &["none"],
                        nullable: true,
                        constraints: &[
                            FieldConstraint::Minimum { value: "0" },
                            FieldConstraint::Maximum { value: "38" },
                        ],
                    },
                    FieldDescription {
                        name: "scale",
                        type_domain: "integer",
                        aggregates: &["none"],
                        nullable: true,
                        constraints: &[
                            FieldConstraint::Minimum { value: "0" },
                            FieldConstraint::Maximum { value: "18" },
                        ],
                    },
                    FieldDescription {
                        name: "min_value",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: true,
                        constraints: &[FieldConstraint::Pattern {
                            pattern: r"^-?(?:0|[1-9][0-9]{0,37})(?:\.[0-9]{1,18})?$",
                        }],
                    },
                    FieldDescription {
                        name: "max_value",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: true,
                        constraints: &[FieldConstraint::Pattern {
                            pattern: r"^-?(?:0|[1-9][0-9]{0,37})(?:\.[0-9]{1,18})?$",
                        }],
                    },
                    FieldDescription {
                        name: "allowed_values",
                        type_domain: "text[]",
                        aggregates: &["none"],
                        nullable: true,
                        constraints: &[],
                    },
                    FieldDescription {
                        name: "aggregates",
                        type_domain: "text[]",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::AllowedValues {
                            values: &["none", "sum", "avg", "min", "max", "last", "count"],
                        }],
                    },
                    FieldDescription {
                        name: "description",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::MinLength { value: 1 }],
                    },
                ],
            },
            EntityDescription {
                name: "MarketObservation",
                description: "A typed market price, bar, or quote observation.",
                fields: vec![
                    FieldDescription {
                        name: "id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format { format: "uuid" }],
                    },
                    FieldDescription {
                        name: "asset_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format { format: "uuid" }],
                    },
                    FieldDescription {
                        name: "kind",
                        type_domain: "text",
                        aggregates: &["count"],
                        nullable: false,
                        constraints: &[FieldConstraint::AllowedValues {
                            values: &["eod", "bar", "quote"],
                        }],
                    },
                    FieldDescription {
                        name: "interval_seconds",
                        type_domain: "integer",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Minimum { value: "0" }],
                    },
                    FieldDescription {
                        name: "valid_at",
                        type_domain: "timestamp",
                        aggregates: &["min", "max"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format {
                            format: "date-time",
                        }],
                    },
                    FieldDescription {
                        name: "known_at",
                        type_domain: "timestamp",
                        aggregates: &["min", "max", "last"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format {
                            format: "date-time",
                        }],
                    },
                    FieldDescription {
                        name: "source",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::MinLength { value: 1 }],
                    },
                    FieldDescription {
                        name: "content_hash",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Pattern {
                            pattern: "^[0-9a-f]{64}$",
                        }],
                    },
                    FieldDescription {
                        name: "open",
                        type_domain: "decimal",
                        aggregates: &["min", "max", "last"],
                        nullable: true,
                        constraints: &[FieldConstraint::Pattern {
                            pattern: r"^-?(?:0|[1-9][0-9]{0,19})(?:\.[0-9]{1,18})?$",
                        }],
                    },
                    FieldDescription {
                        name: "high",
                        type_domain: "decimal",
                        aggregates: &["min", "max", "last"],
                        nullable: true,
                        constraints: &[FieldConstraint::Pattern {
                            pattern: r"^-?(?:0|[1-9][0-9]{0,19})(?:\.[0-9]{1,18})?$",
                        }],
                    },
                    FieldDescription {
                        name: "low",
                        type_domain: "decimal",
                        aggregates: &["min", "max", "last"],
                        nullable: true,
                        constraints: &[FieldConstraint::Pattern {
                            pattern: r"^-?(?:0|[1-9][0-9]{0,19})(?:\.[0-9]{1,18})?$",
                        }],
                    },
                    FieldDescription {
                        name: "close",
                        type_domain: "decimal",
                        aggregates: &["min", "max", "last"],
                        nullable: true,
                        constraints: &[FieldConstraint::Pattern {
                            pattern: r"^-?(?:0|[1-9][0-9]{0,19})(?:\.[0-9]{1,18})?$",
                        }],
                    },
                    FieldDescription {
                        name: "adjclose",
                        type_domain: "decimal",
                        aggregates: &["min", "max", "last"],
                        nullable: true,
                        constraints: &[FieldConstraint::Pattern {
                            pattern: r"^-?(?:0|[1-9][0-9]{0,19})(?:\.[0-9]{1,18})?$",
                        }],
                    },
                    FieldDescription {
                        name: "price",
                        type_domain: "decimal",
                        aggregates: &["last"],
                        nullable: true,
                        constraints: &[FieldConstraint::Pattern {
                            pattern: r"^-?(?:0|[1-9][0-9]{0,19})(?:\.[0-9]{1,18})?$",
                        }],
                    },
                    FieldDescription {
                        name: "volume",
                        type_domain: "decimal",
                        aggregates: &["sum", "min", "max", "last"],
                        nullable: true,
                        constraints: &[FieldConstraint::Pattern {
                            pattern: r"^-?(?:0|[1-9][0-9]{0,37})$",
                        }],
                    },
                    FieldDescription {
                        name: "currency",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Pattern {
                            pattern: "^[A-Z]{3}$",
                        }],
                    },
                    FieldDescription {
                        name: "adjustment",
                        type_domain: "text",
                        aggregates: &["count"],
                        nullable: false,
                        constraints: &[FieldConstraint::AllowedValues {
                            values: &["adjusted", "unadjusted", "none"],
                        }],
                    },
                    FieldDescription {
                        name: "sequence",
                        type_domain: "integer",
                        aggregates: &["max"],
                        nullable: true,
                        constraints: &[FieldConstraint::Minimum { value: "0" }],
                    },
                    FieldDescription {
                        name: "retracted",
                        type_domain: "boolean",
                        aggregates: &["count"],
                        nullable: false,
                        constraints: &[],
                    },
                    FieldDescription {
                        name: "retraction_reason",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: true,
                        constraints: &[],
                    },
                ],
            },
            EntityDescription {
                name: "AssetIdentifier",
                description: "An external identifier scheme value for an asset.",
                fields: vec![
                    FieldDescription {
                        name: "id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format { format: "uuid" }],
                    },
                    FieldDescription {
                        name: "asset_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format { format: "uuid" }],
                    },
                    FieldDescription {
                        name: "scheme",
                        type_domain: "text",
                        aggregates: &["count"],
                        nullable: false,
                        constraints: &[FieldConstraint::KnownValues {
                            values: &[
                                "ISIN", "CUSIP", "TICKER", "BBGID", "FIGI", "SEDOL", "LEI", "CIK",
                                "RIC", "WKN", "VALOR",
                            ],
                        }],
                    },
                    FieldDescription {
                        name: "value",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::MinLength { value: 1 }],
                    },
                    FieldDescription {
                        name: "created_at",
                        type_domain: "timestamp",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format {
                            format: "date-time",
                        }],
                    },
                ],
            },
            EntityDescription {
                name: "IssuerIdentifier",
                description: "An external identifier scheme value for an issuer.",
                fields: vec![
                    FieldDescription {
                        name: "id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format { format: "uuid" }],
                    },
                    FieldDescription {
                        name: "issuer_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format { format: "uuid" }],
                    },
                    FieldDescription {
                        name: "scheme",
                        type_domain: "text",
                        aggregates: &["count"],
                        nullable: false,
                        constraints: &[FieldConstraint::KnownValues {
                            values: &[
                                "ISIN", "CUSIP", "TICKER", "BBGID", "FIGI", "SEDOL", "LEI", "CIK",
                                "RIC", "WKN", "VALOR",
                            ],
                        }],
                    },
                    FieldDescription {
                        name: "value",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::MinLength { value: 1 }],
                    },
                    FieldDescription {
                        name: "created_at",
                        type_domain: "timestamp",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format {
                            format: "date-time",
                        }],
                    },
                ],
            },
            // US9 (2026-10-20): portfolio rules. A shared rule catalog —
            // current-state configuration, not dual-temporal facts, so no
            // as_of_date / knowledge_date.
            EntityDescription {
                name: "Rule",
                description: "A named rule expression attached to a portfolio.",
                fields: vec![
                    FieldDescription {
                        name: "id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format { format: "uuid" }],
                    },
                    FieldDescription {
                        name: "name",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::MinLength { value: 1 }],
                    },
                    FieldDescription {
                        name: "category",
                        type_domain: "text",
                        aggregates: &["count"],
                        nullable: false,
                        constraints: &[FieldConstraint::KnownValues {
                            values: &["all_assets", "portfolio_data", "positions", "events"],
                        }],
                    },
                    FieldDescription {
                        name: "expression",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::MinLength { value: 1 }],
                    },
                    FieldDescription {
                        name: "description",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::MinLength { value: 1 }],
                    },
                    FieldDescription {
                        name: "created_at",
                        type_domain: "timestamp",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::Format {
                            format: "date-time",
                        }],
                    },
                    FieldDescription {
                        name: "created_by",
                        type_domain: "text",
                        aggregates: &["none"],
                        nullable: false,
                        constraints: &[FieldConstraint::MinLength { value: 1 }],
                    },
                ],
            },
        ],
        relationships: vec![
            RelationshipDescription {
                from: "Portfolio",
                to: "Portfolio",
                label: "composes",
                cardinality_from: "1",
                cardinality_to: "0..*",
            },
            RelationshipDescription {
                from: "Portfolio",
                to: "Position",
                label: "holds",
                cardinality_from: "1",
                cardinality_to: "0..*",
            },
            RelationshipDescription {
                from: "Portfolio",
                to: "TemporalFact",
                label: "described_by",
                cardinality_from: "1",
                cardinality_to: "0..*",
            },
            RelationshipDescription {
                from: "Position",
                to: "Asset",
                label: "is_quantity_of",
                cardinality_from: "0..*",
                cardinality_to: "1",
            },
            RelationshipDescription {
                from: "Position",
                to: "Event",
                label: "derived_from",
                cardinality_from: "1",
                cardinality_to: "0..*",
            },
            RelationshipDescription {
                from: "Event",
                to: "Portfolio",
                label: "targets",
                cardinality_from: "0..*",
                cardinality_to: "1",
            },
            RelationshipDescription {
                from: "Event",
                to: "Asset",
                label: "targets",
                cardinality_from: "0..*",
                cardinality_to: "1",
            },
            RelationshipDescription {
                from: "Asset",
                to: "Portfolio",
                label: "is_backed_by",
                cardinality_from: "0..*",
                cardinality_to: "0..1",
            },
            RelationshipDescription {
                from: "Asset",
                to: "Issuer",
                label: "issued_by",
                cardinality_from: "0..*",
                cardinality_to: "1",
            },
            RelationshipDescription {
                from: "Asset",
                to: "TemporalFact",
                label: "described_by",
                cardinality_from: "1",
                cardinality_to: "0..*",
            },
            RelationshipDescription {
                from: "Asset",
                to: "MarketObservation",
                label: "observed_by",
                cardinality_from: "1",
                cardinality_to: "0..*",
            },
            RelationshipDescription {
                from: "Issuer",
                to: "TemporalFact",
                label: "described_by",
                cardinality_from: "1",
                cardinality_to: "0..*",
            },
            // US2b: issuer DAG
            RelationshipDescription {
                from: "Issuer",
                to: "Issuer",
                label: "parent_of",
                cardinality_from: "0..*",
                cardinality_to: "0..1",
            },
            // US8 (2026-10-18): external identifiers
            RelationshipDescription {
                from: "Asset",
                to: "AssetIdentifier",
                label: "identified_by",
                cardinality_from: "1",
                cardinality_to: "0..*",
            },
            RelationshipDescription {
                from: "Issuer",
                to: "IssuerIdentifier",
                label: "identified_by",
                cardinality_from: "1",
                cardinality_to: "0..*",
            },
            // Feature 027: the catalog types every fact and observation.
            RelationshipDescription {
                from: "FactType",
                to: "FactField",
                label: "defined_by",
                cardinality_from: "1",
                cardinality_to: "0..*",
            },
            RelationshipDescription {
                from: "TemporalFact",
                to: "FactType",
                label: "typed_by",
                cardinality_from: "0..*",
                cardinality_to: "1",
            },
            RelationshipDescription {
                from: "MarketObservation",
                to: "FactType",
                label: "typed_by",
                cardinality_from: "0..*",
                cardinality_to: "1",
            },
            // US9 (2026-10-20): portfolio rules — many-to-many, because a
            // rule is shared and governs every portfolio it is attached to.
            RelationshipDescription {
                from: "Portfolio",
                to: "Rule",
                label: "governed_by",
                cardinality_from: "0..*",
                cardinality_to: "0..*",
            },
        ],
        invariants: vec![
            "Portfolio graph is a DAG.",
            "An asset may not be backed by a portfolio that contains itself.",
            "Any level may carry accounting.",
            "Positions are derived from events.",
            "Events are immutable.",
            "Every fact and observation is dual-temporal and append-only.",
            "Conflict resolution is by known_at.",
            "Per-field aggregates are declared, not inferred.",
            "`last` is time-coordinated.",
            "Numeric-only constraint: no sum/avg on non-numeric fields.",
            "Identity fields are none.",
            // US2b: issuer DAG acyclicity (Invariant 12 from predecessor spec)
            "Issuer graph is a DAG.",
            // US8 (2026-10-18): identifier uniqueness (Invariant 9 / FR-032
            // of the predecessor spec)
            "An external identifier names at most one subject within its domain.",
            // US9 (2026-10-20): portfolio rules (Invariants 14, 15 and 16 of
            // the predecessor spec)
            "Rule names are unique across the rule catalog.",
            "Rules are not inherited from ancestor portfolios.",
            "A portfolio carries a given rule at most once.",
            "A fact has exactly one subject reference.",
            "Every fact value is typed by the catalog.",
            "Market prices are observations, not reference facts.",
            "No canonical fact value is stored as text or JSON.",
        ],
    }
}

#[cfg(test)]
mod cross_check;
