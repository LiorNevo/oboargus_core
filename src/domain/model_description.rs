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
                fields: vec![
                    FieldDescription {
                        name: "id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "name",
                        type_domain: "text",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "purpose",
                        type_domain: "text",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "created_at",
                        type_domain: "timestamp",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "created_by",
                        type_domain: "text",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "parent_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                ],
            },
            EntityDescription {
                name: "Asset",
                fields: vec![
                    FieldDescription {
                        name: "id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "asset_type",
                        type_domain: "text",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "display_name",
                        type_domain: "text",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "currency",
                        type_domain: "text",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "created_at",
                        type_domain: "timestamp",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "backed_by_portfolio_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "issuer_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                ],
            },
            EntityDescription {
                name: "Position",
                fields: vec![
                    FieldDescription {
                        name: "portfolio_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "asset_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "quantity",
                        type_domain: "decimal",
                        aggregates: &["sum", "avg", "min", "max", "last"],
                    },
                    FieldDescription {
                        name: "as_of",
                        type_domain: "timestamp",
                        aggregates: &["min", "max", "last"],
                    },
                ],
            },
            EntityDescription {
                name: "Event",
                fields: vec![
                    FieldDescription {
                        name: "id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "event_type",
                        type_domain: "text",
                        aggregates: &["count"],
                    },
                    FieldDescription {
                        name: "effective_date",
                        type_domain: "date",
                        aggregates: &["min", "max", "last"],
                    },
                    FieldDescription {
                        name: "quantity_delta",
                        type_domain: "decimal",
                        aggregates: &["sum", "avg", "min", "max"],
                    },
                    FieldDescription {
                        name: "recorded_at",
                        type_domain: "timestamp",
                        aggregates: &["min", "max", "last"],
                    },
                    FieldDescription {
                        name: "portfolio_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "asset_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "to_portfolio_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                ],
            },
            EntityDescription {
                name: "AssetReferenceData",
                fields: vec![
                    FieldDescription {
                        name: "id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "asset_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "fact_type",
                        type_domain: "text",
                        aggregates: &["count"],
                    },
                    FieldDescription {
                        name: "as_of_date",
                        type_domain: "date",
                        aggregates: &["min", "max"],
                    },
                    FieldDescription {
                        name: "knowledge_date",
                        type_domain: "date",
                        aggregates: &["min", "max", "last"],
                    },
                    FieldDescription {
                        name: "value",
                        type_domain: "text",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "source",
                        type_domain: "text",
                        aggregates: &["none"],
                    },
                ],
            },
            EntityDescription {
                name: "Issuer",
                fields: vec![
                    FieldDescription {
                        name: "id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "name",
                        type_domain: "text",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "issuer_type",
                        type_domain: "text",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "legal_jurisdiction",
                        type_domain: "text",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "created_at",
                        type_domain: "timestamp",
                        aggregates: &["none"],
                    },
                    // US2b: parent_id for the issuer DAG
                    FieldDescription {
                        name: "parent_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                ],
            },
            EntityDescription {
                name: "IssuerReferenceData",
                fields: vec![
                    FieldDescription {
                        name: "id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "issuer_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "fact_type",
                        type_domain: "text",
                        aggregates: &["count"],
                    },
                    FieldDescription {
                        name: "as_of_date",
                        type_domain: "date",
                        aggregates: &["min", "max"],
                    },
                    FieldDescription {
                        name: "knowledge_date",
                        type_domain: "date",
                        aggregates: &["min", "max", "last"],
                    },
                    FieldDescription {
                        name: "value",
                        type_domain: "text",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "source",
                        type_domain: "text",
                        aggregates: &["none"],
                    },
                ],
            },
            EntityDescription {
                name: "PortfolioReferenceData",
                fields: vec![
                    FieldDescription {
                        name: "id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "portfolio_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "fact_type",
                        type_domain: "text",
                        aggregates: &["count"],
                    },
                    FieldDescription {
                        name: "as_of_date",
                        type_domain: "date",
                        aggregates: &["min", "max"],
                    },
                    FieldDescription {
                        name: "knowledge_date",
                        type_domain: "date",
                        aggregates: &["min", "max", "last"],
                    },
                    FieldDescription {
                        name: "value",
                        type_domain: "text",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "source",
                        type_domain: "text",
                        aggregates: &["none"],
                    },
                ],
            },
            // US8 (2026-10-18): external identifiers. Current-state keys,
            // not dual-temporal facts — no as_of_date / knowledge_date.
            EntityDescription {
                name: "AssetIdentifier",
                fields: vec![
                    FieldDescription {
                        name: "id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "asset_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "scheme",
                        type_domain: "text",
                        aggregates: &["count"],
                    },
                    FieldDescription {
                        name: "value",
                        type_domain: "text",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "created_at",
                        type_domain: "timestamp",
                        aggregates: &["none"],
                    },
                ],
            },
            EntityDescription {
                name: "IssuerIdentifier",
                fields: vec![
                    FieldDescription {
                        name: "id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "issuer_id",
                        type_domain: "uuid",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "scheme",
                        type_domain: "text",
                        aggregates: &["count"],
                    },
                    FieldDescription {
                        name: "value",
                        type_domain: "text",
                        aggregates: &["none"],
                    },
                    FieldDescription {
                        name: "created_at",
                        type_domain: "timestamp",
                        aggregates: &["none"],
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
                to: "PortfolioReferenceData",
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
                to: "AssetReferenceData",
                label: "described_by",
                cardinality_from: "0..*",
                cardinality_to: "0..*",
            },
            RelationshipDescription {
                from: "Issuer",
                to: "IssuerReferenceData",
                label: "described_by",
                cardinality_from: "0..*",
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
        ],
        invariants: vec![
            "Portfolio graph is a DAG.",
            "An asset may not be backed by a portfolio that contains itself.",
            "Any level may carry accounting.",
            "Positions are derived from events.",
            "Events are immutable.",
            "Reference data is dual-temporal.",
            "Conflict resolution is by knowledge_date.",
            "Per-field aggregates are declared, not inferred.",
            "`last` is time-coordinated.",
            "Numeric-only constraint: no sum/avg on non-numeric fields.",
            "Identity fields are none.",
            // US2b: issuer DAG acyclicity (Invariant 12 from predecessor spec)
            "Issuer graph is a DAG.",
            // US8 (2026-10-18): identifier uniqueness (Invariant 9 / FR-032
            // of the predecessor spec)
            "An external identifier names at most one subject within its domain.",
        ],
    }
}

#[cfg(test)]
mod cross_check;
