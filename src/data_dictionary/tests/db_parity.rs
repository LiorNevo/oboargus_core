use std::collections::BTreeSet;

use sqlx::Row;

use crate::common::tx_pool;
use crate::data_dictionary::export_system_schema;
use crate::data_dictionary::parity::check_database_parity;
use crate::temporal::catalog::fact_catalog;

#[tokio::test]
async fn database_catalog_matches_the_code_catalog_when_configured() {
    let Some((pool, _guard)) = tx_pool().await else {
        return;
    };
    check_database_parity(&pool)
        .await
        .expect("database catalog parity");

    let schema = export_system_schema().expect("schema");
    let definitions = schema
        .document()
        .get("$defs")
        .and_then(serde_json::Value::as_object)
        .expect("$defs");

    let expected_fields = fact_catalog()
        .iter()
        .flat_map(|entry| {
            entry.fields.iter().map(move |field| {
                (
                    entry.fact_type.to_string(),
                    field.name.to_string(),
                    field.data_type.to_string(),
                )
            })
        })
        .collect::<BTreeSet<_>>();
    let rows = sqlx::query("SELECT fact_type, field_name, data_type FROM fact_type_fields")
        .fetch_all(&pool)
        .await
        .expect("fact_type_fields");
    let actual_fields = rows
        .into_iter()
        .map(|row| {
            (
                row.get::<String, _>("fact_type"),
                row.get::<String, _>("field_name"),
                row.get::<String, _>("data_type"),
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(actual_fields, expected_fields);

    let expected_subjects = fact_catalog()
        .iter()
        .flat_map(|entry| {
            entry
                .subjects
                .iter()
                .map(move |subject| (entry.fact_type.to_string(), subject.as_str().to_string()))
        })
        .collect::<BTreeSet<_>>();
    let rows = sqlx::query("SELECT fact_type, subject_kind FROM fact_type_subjects")
        .fetch_all(&pool)
        .await
        .expect("fact_type_subjects");
    let actual_subjects = rows
        .into_iter()
        .map(|row| {
            (
                row.get::<String, _>("fact_type"),
                row.get::<String, _>("subject_kind"),
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(actual_subjects, expected_subjects);

    for entry in fact_catalog() {
        assert!(
            definitions.contains_key(&format!("FactType.{}", entry.fact_type)),
            "missing definition for {}",
            entry.fact_type
        );
    }
}
