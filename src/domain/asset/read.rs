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
///
/// The display-name and identifier matches are separate `UNION` branches so
/// each keeps a leading-wildcard `ILIKE` predicate that trigram GIN indexes
/// can serve (migration `0015_asset_search_indexes.sql`). Combining them with
/// `OR` instead forces a sequential scan of every asset. `EXPLAIN (ANALYZE,
/// BUFFERS)` over 100k identifiers scans
/// `idx_assets_display_name_trgm` and `idx_asset_identifiers_value_trgm`.
/// No btree expression index is added: `lower(column)` cannot serve a
/// leading-wildcard pattern.
pub async fn search_asset_options(
    pool: &PgPool,
    query: AssetSearchQuery,
) -> Result<Vec<AssetOption>, DomainError> {
    let trimmed = query.q.unwrap_or_default().trim().to_string();
    let limit = query.limit.clamp(1, 50) as i64;

    if trimmed.is_empty() {
        let rows: Vec<AssetOption> = sqlx::query_as(
            r#"
            SELECT a.id, a.display_name, a.asset_type, a.currency,
                   (SELECT ai.value
                      FROM asset_identifiers ai
                     WHERE ai.asset_id = a.id
                     ORDER BY (upper(ai.scheme) = 'TICKER') DESC, ai.created_at ASC, ai.id ASC
                     LIMIT 1) AS primary_identifier
              FROM assets a
             ORDER BY a.display_name ASC, a.id ASC
             LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(pool)
        .await?;
        return Ok(rows);
    }

    let escaped = escape_like(&trimmed);
    let contains = format!("%{escaped}%");
    let prefix = format!("{escaped}%");

    let rows: Vec<AssetOption> = sqlx::query_as(
        r#"
        WITH matched AS (
            SELECT a.id, a.display_name, a.asset_type, a.currency
              FROM assets a
             WHERE a.display_name ILIKE $2 ESCAPE '\'
            UNION
            SELECT a.id, a.display_name, a.asset_type, a.currency
              FROM asset_identifiers ai
              JOIN assets a ON a.id = ai.asset_id
             WHERE ai.value ILIKE $2 ESCAPE '\'
        )
        SELECT m.id, m.display_name, m.asset_type, m.currency,
               (SELECT ai.value
                  FROM asset_identifiers ai
                 WHERE ai.asset_id = m.id
                 ORDER BY (upper(ai.scheme) = 'TICKER') DESC, ai.created_at ASC, ai.id ASC
                 LIMIT 1) AS primary_identifier
          FROM matched m
         ORDER BY
           CASE
             WHEN EXISTS (
               SELECT 1 FROM asset_identifiers ai3
                WHERE ai3.asset_id = m.id AND upper(ai3.value) = upper($1)
             ) THEN 0
             WHEN EXISTS (
               SELECT 1 FROM asset_identifiers ai4
                WHERE ai4.asset_id = m.id AND ai4.value ILIKE $3 ESCAPE '\'
             ) THEN 1
             WHEN m.display_name ILIKE $3 ESCAPE '\' THEN 2
             WHEN m.display_name ILIKE $2 ESCAPE '\' THEN 3
             ELSE 4
           END,
           m.display_name ASC,
           m.id ASC
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
