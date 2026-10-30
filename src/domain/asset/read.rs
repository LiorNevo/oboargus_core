//! Read `Asset` rows.

use crate::domain::id::DomainId;
use sqlx::{PgPool, QueryBuilder};

use crate::domain::asset::entity::{Asset, AssetFilter, AssetOption, AssetSearchQuery};
use crate::domain::error::DomainError;
use crate::domain::id::AssetId;

/// Fetch an asset by id.
/// Implements FR-008 of spec 016-implement-domain-model.
pub async fn get_asset(pool: &PgPool, id: AssetId) -> Result<Asset, DomainError> {
    let row: Option<Asset> = sqlx::query_as(
        r#"
        SELECT id, asset_type, display_name, currency, created_at, backed_by_portfolio_id, issuer_id
        FROM assets WHERE id = $1
        "#,
    )
    .bind(id.as_uuid())
    .fetch_optional(pool)
    .await?;
    row.ok_or_else(|| DomainError::not_found("Asset", id))
}

/// List assets, optionally filtered.
/// Implements FR-008 of spec 016-implement-domain-model.
pub async fn list_assets(pool: &PgPool, filter: AssetFilter) -> Result<Vec<Asset>, DomainError> {
    let mut qb: QueryBuilder<sqlx::Postgres> = QueryBuilder::new(
        "SELECT id, asset_type, display_name, currency, created_at, backed_by_portfolio_id, issuer_id FROM assets WHERE TRUE",
    );
    if let Some(t) = filter.asset_type {
        qb.push(" AND asset_type = ").push_bind(t);
    }
    if let Some(c) = filter.currency {
        qb.push(" AND currency = ").push_bind(c);
    }
    if let Some(i) = filter.issuer_id {
        qb.push(" AND issuer_id = ").push_bind(i.as_uuid());
    }
    qb.push(" ORDER BY created_at ASC");
    let rows: Vec<Asset> = qb.build_query_as().fetch_all(pool).await?;
    Ok(rows)
}

fn escape_like(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Searches assets by display name or identifier for the instrument picker.
///
/// Matching is case-insensitive; user wildcards are escaped. Results prefer an
/// exact identifier match, then identifier prefix, name prefix, and name
/// substring, with display name and id as stable tie-breakers. The query is
/// read-only.
pub async fn search_asset_options(
    pool: &PgPool,
    query: AssetSearchQuery,
) -> Result<Vec<AssetOption>, DomainError> {
    let trimmed = query.q.unwrap_or_default().trim().to_string();
    let escaped = escape_like(&trimmed);
    let contains = format!("%{escaped}%");
    let prefix = format!("{escaped}%");
    let limit = query.limit.clamp(1, 50) as i64;

    let rows: Vec<AssetOption> = sqlx::query_as(
        r#"
        SELECT a.id, a.display_name, a.asset_type, a.currency,
               (SELECT ai.value
                  FROM asset_identifiers ai
                 WHERE ai.asset_id = a.id
                 ORDER BY (upper(ai.scheme) = 'TICKER') DESC, ai.created_at ASC, ai.id ASC
                 LIMIT 1) AS primary_identifier
          FROM assets a
         WHERE $1 = ''
            OR a.display_name ILIKE $2 ESCAPE '\'
            OR EXISTS (
                 SELECT 1 FROM asset_identifiers ai2
                  WHERE ai2.asset_id = a.id AND ai2.value ILIKE $2 ESCAPE '\'
               )
         ORDER BY
           CASE
             WHEN $1 <> '' AND EXISTS (
               SELECT 1 FROM asset_identifiers ai3
                WHERE ai3.asset_id = a.id AND upper(ai3.value) = upper($1)
             ) THEN 0
             WHEN $1 <> '' AND EXISTS (
               SELECT 1 FROM asset_identifiers ai4
                WHERE ai4.asset_id = a.id AND ai4.value ILIKE $3 ESCAPE '\'
             ) THEN 1
             WHEN $1 <> '' AND a.display_name ILIKE $3 ESCAPE '\' THEN 2
             WHEN $1 <> '' AND a.display_name ILIKE $2 ESCAPE '\' THEN 3
             ELSE 4
           END,
           a.display_name ASC,
           a.id ASC
         LIMIT $4
        "#,
    )
    .bind(&trimmed)
    .bind(&contains)
    .bind(&prefix)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}
