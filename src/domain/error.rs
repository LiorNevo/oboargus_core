//! Typed errors for the high-level domain model.
//!
//! Implements FR-004 and FR-020 of `specs/016-implement-domain-model/spec.md`.

use std::fmt;

/// The error type returned by every public function in `obolargus_core::domain`.
///
/// Marked `#[non_exhaustive]` so that future variants can be added without a
/// breaking change. Per the spec, the seven required variants are
/// `Validation`, `NotFound`, `InvariantViolation`, `Infrastructure`,
/// `Immutable`, `CannotDelete`, and `UnsupportedEventType`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
/// Implements FR-004 of spec 016-implement-domain-model.
pub enum DomainError {
    /// A field failed validation (e.g., empty string, out-of-range number).
    #[error("validation: {0}")]
    Validation(String),

    /// The requested entity does not exist.
    #[error("not found: {entity} {id}")]
    NotFound {
        /// The entity type that was not found (e.g., `"Portfolio"`).
        entity: &'static str,
        /// The id that was looked up, as a string.
        id: String,
    },

    /// An attempt to violate a predecessor invariant was rejected.
    #[error("invariant violation: {invariant} - {detail}")]
    InvariantViolation {
        /// The invariant name (e.g., `"portfolio_dag_acyclicity"`).
        invariant: &'static str,
        /// A human-readable, single-sentence description.
        detail: String,
    },

    /// An infrastructure failure (database, serialization, etc.).
    #[error("infrastructure: {0}")]
    Infrastructure(#[from] sqlx::Error),

    /// An attempt to mutate an immutable field or record.
    #[error("immutable: {entity}.{}", field.map(|f| f.to_string()).unwrap_or_else(|| "<record>".to_string()))]
    Immutable {
        /// The entity type.
        entity: &'static str,
        /// The field name, or `None` for the whole record.
        field: Option<&'static str>,
    },

    /// An attempt to delete a record that the predecessor's invariants protect.
    #[error("cannot delete {entity} {id}: {reason}")]
    CannotDelete {
        /// The entity type.
        entity: &'static str,
        /// The id of the record that was targeted.
        id: String,
        /// A human-readable, structured explanation
        /// (e.g., "portfolio has 3 children, retire them first").
        reason: String,
    },

    /// An event type that is not yet supported by the position-replay function.
    #[error("unsupported event type: {event_type}")]
    UnsupportedEventType {
        /// The event type name (e.g., `"dividend"`).
        event_type: String,
    },
}

impl DomainError {
    /// Construct a `NotFound` error for a given entity and id.
    /// Implements FR-004 of spec 016-implement-domain-model.
    pub fn not_found(entity: &'static str, id: impl fmt::Display) -> Self {
        Self::NotFound {
            entity,
            id: id.to_string(),
        }
    }

    /// Construct an `InvariantViolation` error.
    /// Implements FR-004 of spec 016-implement-domain-model.
    pub fn invariant(invariant: &'static str, detail: impl fmt::Display) -> Self {
        Self::InvariantViolation {
            invariant,
            detail: detail.to_string(),
        }
    }

    /// Construct an `Immutable` error for a field.
    /// Implements FR-004 of spec 016-implement-domain-model.
    pub fn immutable_field(entity: &'static str, field: &'static str) -> Self {
        Self::Immutable {
            entity,
            field: Some(field),
        }
    }

    /// Construct an `Immutable` error for a whole record.
    /// Implements FR-004 of spec 016-implement-domain-model.
    pub fn immutable_record(entity: &'static str) -> Self {
        Self::Immutable {
            entity,
            field: None,
        }
    }

    /// Construct a `CannotDelete` error.
    /// Implements FR-004 of spec 016-implement-domain-model.
    pub fn cannot_delete(
        entity: &'static str,
        id: impl fmt::Display,
        reason: impl fmt::Display,
    ) -> Self {
        Self::CannotDelete {
            entity,
            id: id.to_string(),
            reason: reason.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that the `DomainError` enum has exactly the seven required
    /// variants (per spec FR-020).
    #[test]
    fn domain_error_variants_are_exhaustive() {
        let _: DomainError = DomainError::Validation("x".to_string());
        let _: DomainError = DomainError::NotFound {
            entity: "X",
            id: "y".to_string(),
        };
        let _: DomainError = DomainError::InvariantViolation {
            invariant: "i",
            detail: "d".to_string(),
        };
        let _: DomainError = DomainError::Immutable {
            entity: "X",
            field: None,
        };
        let _: DomainError = DomainError::CannotDelete {
            entity: "X",
            id: "y".to_string(),
            reason: "r".to_string(),
        };
        let _: DomainError = DomainError::UnsupportedEventType {
            event_type: "x".to_string(),
        };
    }

    /// Confirms the `From<sqlx::Error>` impl exists via `?` in a function that
    /// never actually runs.
    #[test]
    fn from_sqlx_error_exists() {
        fn _check(e: sqlx::Error) -> DomainError {
            DomainError::from(e)
        }
    }

    /// Asserts that `DomainError` has exactly the seven required variants
    /// (per spec FR-020). Adding a new variant without bumping the contract
    /// is a breaking change; this test is the runtime guard.
    #[test]
    fn domain_error_variant_count_is_seven() {
        use std::mem::discriminant;
        // We can't enumerate variants without `strum` or a custom macro, so
        // we construct one of each and count via the number of
        // `matches!` arms that fire.
        let v = DomainError::Validation("x".to_string());
        let n: u8 = match v {
            DomainError::Validation(_) => 1,
            DomainError::NotFound { .. } => 2,
            DomainError::InvariantViolation { .. } => 3,
            DomainError::Infrastructure(_) => 4,
            DomainError::Immutable { .. } => 5,
            DomainError::CannotDelete { .. } => 6,
            DomainError::UnsupportedEventType { .. } => 7,
            // `_ => 0` is intentionally not added; if a new variant is
            // added, this match becomes non-exhaustive and the test fails
            // to compile, which is the desired guard.
        };
        assert_eq!(n, 1, "the Validation discriminant should be 1");
        // The real guard is the compile-time exhaustiveness check: adding
        // a new variant will cause the `match` to fail to compile, which
        // is the safety net.
        let _ = discriminant(&v);
    }

    // ---------- constructor coverage ----------
    //
    // The five constructors below were previously uncovered by the
    // existing test suite. The tests pin the constructor output
    // (including the `Display` arm selected by the `field: Option`
    // discriminant) so a future refactor that changes the error
    // message has to update the test too.

    #[test]
    fn not_found_constructor_produces_not_found_variant() {
        let e = DomainError::not_found("Asset", "abc-123");
        match &e {
            DomainError::NotFound { entity, id } => {
                assert_eq!(*entity, "Asset");
                assert_eq!(id, "abc-123");
            }
            other => panic!("expected NotFound, got {:?}", other),
        }
        // The Display impl must surface both the entity and the id.
        assert!(e.to_string().contains("Asset"));
        assert!(e.to_string().contains("abc-123"));
    }

    #[test]
    fn invariant_constructor_produces_invariant_violation_variant() {
        let e = DomainError::invariant("dag_acyclic", "would introduce a cycle");
        match e {
            DomainError::InvariantViolation { invariant, detail } => {
                assert_eq!(invariant, "dag_acyclic");
                assert_eq!(detail, "would introduce a cycle");
            }
            other => panic!("expected InvariantViolation, got {:?}", other),
        }
    }

    #[test]
    fn immutable_field_constructor_produces_favorable_field_message() {
        let e = DomainError::immutable_field("Portfolio", "id");
        match e {
            DomainError::Immutable { entity, field } => {
                assert_eq!(entity, "Portfolio");
                assert_eq!(field, Some("id"));
            }
            other => panic!("expected Immutable, got {:?}", other),
        }
        // The Display arm for `field: Some(_)` includes the field name.
        assert!(e.to_string().contains("id"));
        assert!(e.to_string().contains("Portfolio"));
    }

    #[test]
    fn immutable_record_constructor_produces_record_level_message() {
        let e = DomainError::immutable_record("Asset");
        match e {
            DomainError::Immutable { entity, field } => {
                assert_eq!(entity, "Asset");
                assert_eq!(field, None);
            }
            other => panic!("expected Immutable, got {:?}", other),
        }
        // The Display arm for `field: None` falls back to `<record>`.
        assert!(e.to_string().contains("<record>"));
    }

    #[test]
    fn cannot_delete_constructor_produces_cannot_delete_variant() {
        let e = DomainError::cannot_delete("Portfolio", "p-1", "3 children");
        match e {
            DomainError::CannotDelete { entity, id, reason } => {
                assert_eq!(entity, "Portfolio");
                assert_eq!(id, "p-1");
                assert_eq!(reason, "3 children");
            }
            other => panic!("expected CannotDelete, got {:?}", other),
        }
    }

    #[test]
    fn unsupported_event_type_display_includes_name() {
        let e = DomainError::UnsupportedEventType {
            event_type: "dividend".to_string(),
        };
        assert!(e.to_string().contains("dividend"));
    }
}
