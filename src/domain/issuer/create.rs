use crate::domain::id::DomainId;
use sqlx::PgPool;

use crate::domain::error::DomainError;
use crate::domain::id::{IssuerId, new_uuid};
use crate::domain::issuer::dag;
use crate::domain::issuer::entity::{Issuer, NewIssuer};

/// Implements FR-008 of spec 016-implement-domain-model.
pub async fn create_issuer(pool: &PgPool, input: NewIssuer) -> Result<Issuer, DomainError> {
    let id = new_uuid();

    // Enforce issuer DAG acyclicity before inserting.
    if let Some(parent_id) = input.parent_id {
        // Trivial self-loop check (O(1))
        if dag::is_self_parent(IssuerId::from_uuid(id), parent_id) {
            return Err(DomainError::invariant(
                "issuer_dag_acyclicity",
                format!("issuer {} cannot be its own parent", id),
            ));
        }
        // Cycle check via recursive CTE
        dag::detect_cycle(pool, IssuerId::from_uuid(id), parent_id).await?;
    }

    sqlx::query(
        "INSERT INTO issuers (id, name, issuer_type, legal_jurisdiction, parent_id) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(&input.name)
    .bind(&input.issuer_type)
    .bind(&input.legal_jurisdiction)
    .bind(input.parent_id.map(|p| p.as_uuid()))
    .execute(pool)
    .await?;
    super::read::get_issuer(pool, IssuerId::from_uuid(id)).await
}
