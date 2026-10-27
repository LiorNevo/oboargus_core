//! Schema-shape assertions for migration `0009_temporal_facts.sql`.
//!
//! Implements T017 of `specs/021-bitemporal-market-data/tasks.md`: after the
//! migration the three fact tables must carry both instants, a retraction
//! flag, generated compatibility dates, and the source-scoped identity key.

use sqlx::{PgPool, Row};

const TABLES: [&str; 3] = [
    "asset_reference_data",
    "issuer_reference_data",
    "portfolio_reference_data",
];

#[derive(Debug)]
struct ColumnShape {
    name: String,
    nullable: bool,
    generated: String,
}

async fn columns(pool: &PgPool, table: &str) -> Vec<ColumnShape> {
    sqlx::query(
        "SELECT column_name, is_nullable, is_generated \
         FROM information_schema.columns WHERE table_name = $1",
    )
    .bind(table)
    .fetch_all(pool)
    .await
    .expect("read information_schema")
    .into_iter()
    .map(|row| ColumnShape {
        name: row.get("column_name"),
        nullable: row.get::<String, _>("is_nullable") == "YES",
        generated: row.get("is_generated"),
    })
    .collect()
}

async fn unique_constraints(pool: &PgPool, table: &str) -> Vec<(String, String)> {
    sqlx::query(
        "SELECT conname, pg_get_constraintdef(oid) AS definition \
         FROM pg_constraint WHERE conrelid = $1::regclass AND contype = 'u'",
    )
    .bind(table)
    .fetch_all(pool)
    .await
    .expect("read constraints")
    .into_iter()
    .map(|row| (row.get("conname"), row.get("definition")))
    .collect()
}

async fn indexes(pool: &PgPool, table: &str) -> Vec<String> {
    sqlx::query("SELECT indexname FROM pg_indexes WHERE tablename = $1")
        .bind(table)
        .fetch_all(pool)
        .await
        .expect("read indexes")
        .into_iter()
        .map(|row| row.get("indexname"))
        .collect()
}

#[tokio::test]
async fn every_fact_table_has_both_instants_and_a_retraction_flag() {
    let Some((pool, guard)) = obolargus_core::common::tx_pool().await else {
        return;
    };

    for table in TABLES {
        let columns = columns(&pool, table).await;
        let by_name = |name: &str| columns.iter().find(|column| column.name == name);

        for instant in ["valid_at", "known_at"] {
            let column = by_name(instant).unwrap_or_else(|| panic!("{table} is missing {instant}"));
            assert!(!column.nullable, "{table}.{instant} must be NOT NULL");
        }

        let retracted = by_name("retracted").expect("retracted column");
        assert!(!retracted.nullable, "{table}.retracted must be NOT NULL");
    }

    guard.cleanup().await;
}

#[tokio::test]
async fn legacy_date_columns_are_generated_from_the_instants() {
    let Some((pool, guard)) = obolargus_core::common::tx_pool().await else {
        return;
    };

    for table in TABLES {
        let columns = columns(&pool, table).await;
        for date in ["as_of_date", "knowledge_date"] {
            let column = columns
                .iter()
                .find(|column| column.name == date)
                .unwrap_or_else(|| panic!("{table} is missing {date}"));
            assert_eq!(
                column.generated, "ALWAYS",
                "{table}.{date} must be a generated compatibility column"
            );
        }
    }

    guard.cleanup().await;
}

#[tokio::test]
async fn the_identity_key_includes_source_and_the_old_key_is_gone() {
    let Some((pool, guard)) = obolargus_core::common::tx_pool().await else {
        return;
    };

    let legacy = [
        "asset_ref_data_dual_temporal",
        "issuer_ref_data_dual_temporal",
        "portfolio_ref_data_dual_temporal",
    ];

    for (table, old_name) in TABLES.iter().zip(legacy) {
        let constraints = unique_constraints(&pool, table).await;
        let names: Vec<&str> = constraints.iter().map(|(name, _)| name.as_str()).collect();
        assert!(
            !names.contains(&old_name),
            "{table} still carries the legacy key {old_name}"
        );

        let (_, definition) = constraints
            .iter()
            .find(|(_, definition)| definition.contains("UNIQUE"))
            .unwrap_or_else(|| panic!("{table} has no unique constraint"));
        assert!(
            definition.contains("source"),
            "{table} identity key must include source: {definition}"
        );
    }

    guard.cleanup().await;
}

#[tokio::test]
async fn both_read_paths_are_indexed() {
    let Some((pool, guard)) = obolargus_core::common::tx_pool().await else {
        return;
    };

    for table in TABLES {
        let names = indexes(&pool, table).await;
        let winner = format!("idx_{table}_winner");
        assert!(
            names.iter().any(|name| name == &winner),
            "{table} is missing the winner index {winner}: {names:?}"
        );
        assert!(
            names.iter().any(|name| name.contains("_known")),
            "{table} is missing the as-known index: {names:?}"
        );
    }

    guard.cleanup().await;
}

#[tokio::test]
async fn re_running_the_migration_is_a_no_op() {
    let Some((pool, guard)) = obolargus_core::common::tx_pool().await else {
        return;
    };

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("a second migration run must succeed");

    guard.cleanup().await;
}
