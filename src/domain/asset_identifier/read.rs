//! Read `AssetIdentifier` records and resolve identifiers to assets.
//!
//! Implements FR-023 of `specs/016-implement-domain-model/spec.md`.

use sqlx::{PgPool, QueryBuilder};

use crate::domain::asset::entity::Asset;
use crate::domain::asset_identifier::entity::AssetIdentifier;
use crate::domain::error::DomainError;
use crate::domain::id::{AssetId, DomainId, IdentifierId};
use crate::domain::identifier::normalize_lookup;

/// A read-only projection of an identifier that the system already knows.
///
/// Used by the Data Load identifier search to label a candidate as known
/// without writing anything. Implements FR-019 of
/// `specs/026-identifier-search/spec.md`.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct KnownIdentifier {
    /// The asset that claims the identifier.
    pub asset_id: uuid::Uuid,
    /// The asset's canonical type.
    pub asset_type: String,
    /// The asset's display name.
    pub display_name: String,
    /// The asset's currency.
    pub currency: String,
    /// The matching identifier scheme.
    pub scheme: String,
    /// The matching identifier value.
    pub value: String,
}

/// Finds every known identifier whose normalized `(scheme, value)` pair is in
/// `pairs`, using one batched query.
///
/// Implements FR-019 of `specs/026-identifier-search/spec.md`.
pub async fn find_identifiers_by_pairs(
    pool: &PgPool,
    pairs: &[(String, String)],
) -> Result<Vec<KnownIdentifier>, DomainError> {
    let normalized: Vec<(String, String)> = pairs
        .iter()
        .map(|(scheme, value)| normalize_lookup(scheme, value))
        .filter(|(scheme, value)| !scheme.is_empty() && !value.is_empty())
        .collect();
    if normalized.is_empty() {
        return Ok(Vec::new());
    }

    let mut qb: QueryBuilder<sqlx::Postgres> = QueryBuilder::new(
        "SELECT a.id AS asset_id, a.asset_type, a.display_name, a.currency, i.scheme, i.value \
         FROM asset_identifiers i \
         JOIN assets a ON a.id = i.asset_id \
         WHERE (i.scheme, i.value) IN (",
    );
    for (index, (scheme, value)) in normalized.into_iter().enumerate() {
        if index > 0 {
            qb.push(", ");
        }
        qb.push("(")
            .push_bind(scheme)
            .push(", ")
            .push_bind(value)
            .push(")");
    }
    qb.push(") ORDER BY a.id, i.scheme, i.value");
    let rows: Vec<KnownIdentifier> = qb.build_query_as().fetch_all(pool).await?;
    Ok(rows)
}

/// Finds assets whose ticker value matches any of `values`, ignoring case and
/// venue qualification, using one batched query.
///
/// This mirrors the load's equivalent-ticker adoption rule: `TICKER:QQQM`,
/// `TICKER:XNAS:QQQM`, and case variants name the same instrument.
///
/// Implements FR-019 of `specs/026-identifier-search/spec.md`.
pub async fn find_assets_by_ticker_values(
    pool: &PgPool,
    values: &[String],
) -> Result<Vec<KnownIdentifier>, DomainError> {
    let values: Vec<String> = values
        .iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect();
    if values.is_empty() {
        return Ok(Vec::new());
    }

    let mut qb: QueryBuilder<sqlx::Postgres> = QueryBuilder::new(
        "SELECT a.id AS asset_id, a.asset_type, a.display_name, a.currency, i.scheme, i.value \
         FROM assets a \
         JOIN asset_identifiers i ON i.asset_id = a.id \
         WHERE (upper(i.scheme) = 'TICKER' OR upper(i.scheme) LIKE 'TICKER:%') \
           AND upper(i.value) IN (",
    );
    for (index, value) in values.into_iter().enumerate() {
        if index > 0 {
            qb.push(", ");
        }
        qb.push_bind(value.to_uppercase());
    }
    qb.push(") ORDER BY a.id, i.scheme, i.value");
    let rows: Vec<KnownIdentifier> = qb.build_query_as().fetch_all(pool).await?;
    Ok(rows)
}

/// Fetch an identifier record by id.
/// Implements FR-023 of spec 016-implement-domain-model.
pub async fn get_asset_identifier(
    pool: &PgPool,
    id: IdentifierId,
) -> Result<AssetIdentifier, DomainError> {
    let row: Option<AssetIdentifier> = sqlx::query_as(
        "SELECT id, asset_id, scheme, value, created_at FROM asset_identifiers WHERE id = $1",
    )
    .bind(id.as_uuid())
    .fetch_optional(pool)
    .await?;
    row.ok_or_else(|| DomainError::not_found("AssetIdentifier", id))
}

/// List every identifier registered for an asset (zero is the normal case).
/// Implements FR-023 of spec 016-implement-domain-model.
pub async fn list_asset_identifiers(
    pool: &PgPool,
    asset_id: AssetId,
) -> Result<Vec<AssetIdentifier>, DomainError> {
    let rows: Vec<AssetIdentifier> = sqlx::query_as(
        "SELECT id, asset_id, scheme, value, created_at \
         FROM asset_identifiers WHERE asset_id = $1 ORDER BY created_at ASC, id ASC",
    )
    .bind(asset_id.as_uuid())
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Find the identifier record claiming `(scheme, value)`, if any.
///
/// The pair is normalized exactly as on write (FR-025). An empty scheme or
/// value can never be claimed (writes reject it), so an empty pair simply
/// matches nothing and returns `Ok(None)`.
///
/// Implements FR-023 of spec 016-implement-domain-model.
pub async fn find_asset_identifier(
    pool: &PgPool,
    scheme: &str,
    value: &str,
) -> Result<Option<AssetIdentifier>, DomainError> {
    let (scheme, value) = normalize_lookup(scheme, value);
    if scheme.is_empty() || value.is_empty() {
        return Ok(None);
    }
    let row: Option<AssetIdentifier> = sqlx::query_as(
        "SELECT id, asset_id, scheme, value, created_at \
         FROM asset_identifiers WHERE scheme = $1 AND value = $2",
    )
    .bind(scheme)
    .bind(value)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Resolve an external identifier to the asset that carries it.
///
/// Because `(scheme, value)` is unique within the asset-identifier domain
/// (FR-024), the lookup resolves **at most one** asset. `Ok(None)` means no
/// asset claims the pair (which is not an error).
///
/// Implements FR-023 of spec 016-implement-domain-model.
pub async fn find_asset_by_identifier(
    pool: &PgPool,
    scheme: &str,
    value: &str,
) -> Result<Option<Asset>, DomainError> {
    let (scheme, value) = normalize_lookup(scheme, value);
    if scheme.is_empty() || value.is_empty() {
        return Ok(None);
    }
    let row: Option<Asset> = sqlx::query_as(
        "SELECT a.id, a.asset_type, a.display_name, a.currency, a.created_at, \
                a.backed_by_portfolio_id, a.issuer_id \
         FROM assets a \
         JOIN asset_identifiers i ON i.asset_id = a.id \
         WHERE i.scheme = $1 AND i.value = $2",
    )
    .bind(scheme)
    .bind(value)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}
