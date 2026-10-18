//! Shared scheme vocabulary and key validation for the external-identifier
//! entities (`AssetIdentifier`, `IssuerIdentifier`).
//!
//! The scheme vocabulary is **open**: the constants below are well-known
//! schemes, not an enum gate (FR-026). Keys are validated and normalized per
//! FR-025 — the scheme is trimmed and uppercased on write and on lookup, the
//! value is trimmed and matched exactly.
//!
//! Implements FR-025 and FR-026 of `specs/016-implement-domain-model/spec.md`
//! (predecessor FR-031, FR-032 of `specs/015-database-uml-domains/spec.md`).

use crate::domain::error::DomainError;

/// Well-known external identifier schemes.
///
/// This is a **non-exhaustive** set of constants (FR-026): any scheme string
/// is storable and look-up-able. Venue-qualified schemes such as
/// `"TICKER:XETRA"` are ordinary scheme values (predecessor Assumption 24).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Implements FR-026 of spec 016-implement-domain-model.
pub struct IdentifierScheme;

impl IdentifierScheme {
    /// ISIN (ISO 6166) — international securities identifier.
    pub const ISIN: &'static str = "ISIN";
    /// CUSIP — North American securities identifier.
    pub const CUSIP: &'static str = "CUSIP";
    /// Ticker / symbol as listed on an exchange (unqualified).
    pub const TICKER: &'static str = "TICKER";
    /// Bloomberg identifier (vendor-specific).
    pub const BBGID: &'static str = "BBGID";
    /// FIGI (OpenFIGI / Bloomberg) financial instrument identifier.
    pub const FIGI: &'static str = "FIGI";
    /// SEDOL — London Stock Exchange identifier.
    pub const SEDOL: &'static str = "SEDOL";
    /// LEI (ISO 17442) — legal entity identifier; typically an issuer key.
    pub const LEI: &'static str = "LEI";
    /// CIK — SEC Central Index Key; typically an issuer key.
    pub const CIK: &'static str = "CIK";
    /// RIC — Reuters instrument code.
    pub const RIC: &'static str = "RIC";
    /// WKN — German securities identifier.
    pub const WKN: &'static str = "WKN";
    /// VALOR — Swiss securities identifier.
    pub const VALOR: &'static str = "VALOR";

    /// Normalize a scheme for storage and lookup: trim, then uppercase
    /// (FR-025). `" isin "` and `"Isin"` both normalize to `"ISIN"`.
    /// Implements FR-025 of spec 016-implement-domain-model.
    pub fn normalize(scheme: &str) -> String {
        scheme.trim().to_uppercase()
    }
}

/// Validate and normalize a raw `(scheme, value)` pair for storage (FR-025).
///
/// Returns the normalized pair, or `DomainError::Validation` when the scheme
/// or the value is empty after trimming. The scheme is trimmed and uppercased;
/// the value is trimmed and kept otherwise as given (matching is exact and
/// case-sensitive — see Assumption 13 of the spec).
///
/// Implements FR-025 of spec 016-implement-domain-model.
pub fn normalize_pair(scheme: &str, value: &str) -> Result<(String, String), DomainError> {
    let scheme = IdentifierScheme::normalize(scheme);
    let value = value.trim().to_string();
    if scheme.is_empty() {
        return Err(DomainError::Validation(
            "identifier scheme must not be empty".to_string(),
        ));
    }
    if value.is_empty() {
        return Err(DomainError::Validation(
            "identifier value must not be empty".to_string(),
        ));
    }
    Ok((scheme, value))
}

/// Normalize a raw pair for **lookup** (FR-025): same normalization as
/// [`normalize_pair`], but an empty scheme or value is not an error — an
/// empty pair can never be claimed by a record (writes reject it), so a
/// lookup of an empty pair simply matches nothing. Callers treat `None` as
/// "nobody claims this pair".
///
/// Implements FR-025 of spec 016-implement-domain-model.
pub fn normalize_lookup(scheme: &str, value: &str) -> (String, String) {
    (
        IdentifierScheme::normalize(scheme),
        value.trim().to_string(),
    )
}

/// The typed error for a violated `(scheme, value)` uniqueness key
/// (FR-024). Used both for the pre-insert application check and for mapping
/// a database unique-constraint violation, so a race cannot leak a raw
/// `Infrastructure` error.
///
/// `domain` names the subject domain (`"Asset"` or `"Issuer"`).
///
/// Implements FR-024 of spec 016-implement-domain-model.
pub fn uniqueness_violation(
    domain: &'static str,
    scheme: &str,
    value: &str,
    claimed_by: impl std::fmt::Display,
) -> DomainError {
    DomainError::invariant(
        "identifier_uniqueness",
        format!("({scheme}, {value}) is already claimed by {domain} identifier {claimed_by}"),
    )
}
