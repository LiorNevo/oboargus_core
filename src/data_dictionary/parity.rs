//! Parity checks between the exported schema and the authoritative sources.

use std::collections::BTreeSet;

use serde_json::Value;
use sqlx::PgPool;

use crate::data_dictionary::document::SystemSchema;
use crate::domain::model_description::model_description;
use crate::temporal::catalog::fact_catalog;

/// Checks code-level parity between the schema and the authoritative model.
pub fn check_code_parity(schema: &SystemSchema) -> Result<(), Vec<String>> {
    let document = schema.document();
    let Some(definitions) = document.get("$defs").and_then(Value::as_object) else {
        return Err(vec!["document has no $defs object".to_string()]);
    };
    let mut errors = Vec::new();
    let model = model_description();

    for entity in &model.entities {
        let Some(definition) = definitions.get(entity.name) else {
            errors.push(format!("missing entity definition: {}", entity.name));
            continue;
        };
        let properties = definition
            .get("properties")
            .and_then(Value::as_object)
            .map(|properties| properties.keys().cloned().collect::<BTreeSet<_>>())
            .unwrap_or_default();
        let expected = entity
            .fields
            .iter()
            .map(|field| field.name.to_string())
            .collect::<BTreeSet<_>>();
        if properties != expected {
            errors.push(format!("entity field drift: {}", entity.name));
        }
        let required = definition
            .get("required")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let expected_required = entity
            .fields
            .iter()
            .filter(|field| !field.nullable)
            .map(|field| field.name.to_string())
            .collect::<BTreeSet<_>>();
        if required != expected_required {
            errors.push(format!("required-field drift: {}", entity.name));
        }
    }

    let catalog = fact_catalog();
    let expected_fact_types = catalog
        .iter()
        .map(|entry| format!("FactType.{}", entry.fact_type))
        .collect::<BTreeSet<_>>();
    let actual_fact_types = definitions
        .keys()
        .filter(|name| name.starts_with("FactType."))
        .cloned()
        .collect::<BTreeSet<_>>();
    for missing in expected_fact_types.difference(&actual_fact_types) {
        errors.push(format!("missing fact-type definition: {missing}"));
    }
    for extra in actual_fact_types.difference(&expected_fact_types) {
        errors.push(format!("extra fact-type definition: {extra}"));
    }

    let relationship_count = document
        .get("x-obolargus-relationships")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    if relationship_count != model.relationships.len() {
        errors.push(format!(
            "relationship drift: expected {}, found {relationship_count}",
            model.relationships.len()
        ));
    }
    let invariant_count = document
        .get("x-obolargus-invariants")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    if invariant_count != model.invariants.len() {
        errors.push(format!(
            "invariant drift: expected {}, found {invariant_count}",
            model.invariants.len()
        ));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Checks the database-projected catalog against the in-code catalog.
///
/// The caller supplies a pool; when no pool is configured the code-level
/// parity check remains the authoritative gate.
pub async fn check_database_parity(pool: &PgPool) -> Result<(), Vec<String>> {
    let rows: Vec<(String, i32)> =
        sqlx::query_as("SELECT fact_type, schema_version FROM fact_types ORDER BY fact_type")
            .fetch_all(pool)
            .await
            .map_err(|error| vec![format!("database catalog read failed: {error}")])?;
    let catalog = fact_catalog();
    let mut errors = Vec::new();
    let expected = catalog
        .iter()
        .map(|entry| (entry.fact_type.to_string(), entry.schema_version))
        .collect::<BTreeSet<_>>();
    let actual = rows.into_iter().collect::<BTreeSet<_>>();
    for missing in expected.difference(&actual) {
        errors.push(format!("database catalog missing: {}", missing.0));
    }
    for extra in actual.difference(&expected) {
        errors.push(format!("database catalog extra: {}", extra.0));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
