//! Create an `Asset` row.

use crate::domain::id::DomainId;
use sqlx::PgPool;

use crate::domain::asset::entity::{Asset, NewAsset};
use crate::domain::asset::no_self_investment;
use crate::domain::error::DomainError;
use crate::domain::id::{AssetId, new_uuid};

/// Insert one `assets` row on an arbitrary connection.
///
/// Extracted so the single-asset create path and the transactional
/// "asset + identifier" path share exactly one INSERT statement.
///
/// Implements FR-002 of spec 016-implement-domain-model and FR-015 of
/// `specs/018-load-reference-data/spec.md`.
pub(crate) async fn insert_asset(
    conn: &mut sqlx::PgConnection,
    id: uuid::Uuid,
    input: &NewAsset,
) -> Result<(), DomainError> {
    sqlx::query(
        r#"
        INSERT INTO assets (id, asset_type, display_name, currency, backed_by_portfolio_id, issuer_id)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(id)
    .bind(&input.asset_type)
    .bind(&input.display_name)
    .bind(&input.currency)
    .bind(input.backed_by_portfolio_id.map(|p| p.as_uuid()))
    .bind(input.issuer_id.map(|i| i.as_uuid()))
    .execute(conn)
    .await?;
    Ok(())
}

/// Create a new asset. Enforces the no-self-investment invariant before
/// insertion.
/// Implements FR-002 of spec 016-implement-domain-model.
pub async fn create_asset(pool: &PgPool, input: NewAsset) -> Result<Asset, DomainError> {
    if let Some(backed_by) = input.backed_by_portfolio_id {
        let new_id = AssetId::from_uuid(new_uuid());
        no_self_investment::check_no_self_investment(pool, new_id, backed_by).await?;
    }

    let new_id = new_uuid();
    let mut conn = pool.acquire().await?;
    insert_asset(&mut conn, new_id, &input).await?;
    drop(conn);

    super::read::get_asset(pool, AssetId::from_uuid(new_id)).await
}

/// Create an asset together with the identifier that names it, atomically.
///
/// Both rows are written in one transaction: a collision on the identifier's
/// `(scheme, value)` unique key rolls the asset row back instead of leaving
/// an orphan asset behind. The collision itself surfaces as the domain's
/// typed `identifier_uniqueness` invariant violation, so the caller can
/// re-read the winner and adopt it.
///
/// Implements FR-015 of `specs/018-load-reference-data/spec.md`.
pub async fn create_asset_with_identifier(
    pool: &PgPool,
    input: NewAsset,
    scheme: &str,
    value: &str,
) -> Result<Asset, DomainError> {
    use crate::domain::asset_identifier::create::insert_identifier_row;
    use crate::domain::identifier::normalize_pair;

    let (scheme, value) = normalize_pair(scheme, value)?;
    let new_id = new_uuid();

    let mut tx = pool.begin().await?;
    insert_asset(&mut tx, new_id, &input).await?;

    if let Err(error) = insert_identifier_row(&mut tx, new_uuid(), new_id, &scheme, &value).await {
        tx.rollback().await?;
        return Err(error);
    }

    tx.commit().await?;
    super::read::get_asset(pool, AssetId::from_uuid(new_id)).await
}
