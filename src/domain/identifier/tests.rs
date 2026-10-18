//! Unit tests for the shared identifier scheme module (no database needed).
//!
//! Implements FR-025 and FR-026 of
//! `specs/016-implement-domain-model/spec.md`.

use super::{IdentifierScheme, normalize_lookup, normalize_pair, uniqueness_violation};
use crate::domain::error::DomainError;

#[test]
fn scheme_is_trimmed_and_uppercased() {
    let (scheme, value) = normalize_pair(" isin ", " US0378331005 ").expect("valid pair");
    assert_eq!(scheme, "ISIN");
    assert_eq!(value, "US0378331005");
}

#[test]
fn value_matching_is_exact() {
    // The value is trimmed but not case-folded (Assumption 13).
    let (_, value) = normalize_pair("TICKER", "aapl").expect("valid pair");
    assert_eq!(value, "aapl");
    assert_ne!(value, "AAPL");
}

#[test]
fn empty_scheme_or_value_is_rejected() {
    assert!(matches!(
        normalize_pair("   ", "US0378331005"),
        Err(DomainError::Validation(_))
    ));
    assert!(matches!(
        normalize_pair("ISIN", "   "),
        Err(DomainError::Validation(_))
    ));
}

#[test]
fn well_known_constants_are_normalized_forms() {
    for scheme in [
        IdentifierScheme::ISIN,
        IdentifierScheme::CUSIP,
        IdentifierScheme::TICKER,
        IdentifierScheme::BBGID,
        IdentifierScheme::FIGI,
        IdentifierScheme::SEDOL,
        IdentifierScheme::LEI,
        IdentifierScheme::CIK,
        IdentifierScheme::RIC,
        IdentifierScheme::WKN,
        IdentifierScheme::VALOR,
    ] {
        assert_eq!(IdentifierScheme::normalize(scheme), scheme);
    }
}

#[test]
fn open_vocabulary_accepts_unknown_schemes() {
    // FR-026: an unknown scheme is stored exactly like a well-known one.
    let (scheme, _) = normalize_pair("TICKER:XETRA", "SAP").expect("valid pair");
    assert_eq!(scheme, "TICKER:XETRA");
}

#[test]
fn lookup_normalization_matches_write_normalization() {
    let (written_scheme, written_value) = normalize_pair("isin", "US0378331005").expect("pair");
    let (lookup_scheme, lookup_value) = normalize_lookup(" ISIN ", " US0378331005 ");
    assert_eq!(written_scheme, lookup_scheme);
    assert_eq!(written_value, lookup_value);
}

#[test]
fn uniqueness_violation_is_a_typed_invariant_error() {
    let err = uniqueness_violation("Asset", "ISIN", "US0378331005", "id-1");
    assert!(matches!(
        err,
        DomainError::InvariantViolation {
            invariant: "identifier_uniqueness",
            ..
        }
    ));
}
