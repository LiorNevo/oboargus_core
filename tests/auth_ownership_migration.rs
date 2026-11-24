//! Database-backed tests for migration 0014 (auth + ownership).
//!
//! Proves the migration applies cleanly, the actor/group tables and their
//! constraints exist, the rewritten `data_load_history` view returns group
//! rows and ungrouped runs, and the legacy owner/status backfills are
//! correct. Skipped as a passing no-op when no test database is configured.
//!
//! The fixture already applies every migration. This test re-executes the
//! 0014 SQL to exercise its backfill statements against rows inserted with
//! legacy values (just as a database upgraded from migration 0013 would
//! look). The file is written to be idempotent, so re-execution proves that
//! property too.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use obolargus_core::common::tx_pool;

const MIGRATION_0014: &str = include_str!("../migrations/0014_auth_ownership.sql");

/// `information_schema.columns` row: name, data type, nullability, default.
type ColumnRow = (String, String, String, Option<String>);

async fn columns(pool: &PgPool, table: &str) -> Vec<ColumnRow> {
    sqlx::query_as(
        "SELECT column_name, data_type, is_nullable, column_default \
         FROM information_schema.columns \
         WHERE table_schema = 'public' AND table_name = $1 \
         ORDER BY ordinal_position",
    )
    .bind(table)
    .fetch_all(pool)
    .await
    .expect("columns")
}

async fn column_names(pool: &PgPool, table: &str) -> Vec<String> {
    columns(pool, table)
        .await
        .into_iter()
        .map(|row| row.0)
        .collect()
}

async fn index_definitions(pool: &PgPool, table: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT indexdef FROM pg_indexes \
         WHERE schemaname = 'public' AND tablename = $1",
    )
    .bind(table)
    .fetch_all(pool)
    .await
    .expect("index definitions")
}

async fn primary_key_columns(pool: &PgPool, table: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT a.attname \
         FROM pg_constraint c \
         JOIN LATERAL unnest(c.conkey) WITH ORDINALITY AS k(attnum, ord) ON TRUE \
         JOIN pg_attribute a \
           ON a.attrelid = c.conrelid AND a.attnum = k.attnum \
         WHERE c.conrelid = $1::regclass AND c.contype = 'p' \
         ORDER BY k.ord",
    )
    .bind(table)
    .fetch_all(pool)
    .await
    .expect("primary key columns")
}

/// Inserts a `reference_data_load_runs` row shaped like a pre-0014 run.
/// `no_op` absorbs the remaining attempted identifiers so the 0007
/// partition checks hold.
#[allow(clippy::too_many_arguments)]
async fn insert_legacy_run(
    pool: &PgPool,
    attempted: i32,
    loaded: i32,
    rejected: i32,
    ended: bool,
    owner_id: Option<&str>,
    range_group_id: Option<Uuid>,
    range: Option<(chrono::NaiveDate, chrono::NaiveDate)>,
) -> Uuid {
    let id = Uuid::new_v4();
    let no_op = attempted - loaded - rejected;
    let (from, to) = match range {
        Some((from, to)) => (Some(from), Some(to)),
        None => (None, None),
    };
    sqlx::query(
        "INSERT INTO reference_data_load_runs \
             (id, trigger, identifiers_requested_raw, identifiers_attempted, \
              loaded, no_op, rejected, ended_at, owner_id, range_group_id, \
              range_from, range_to, range_concurrency) \
         VALUES ($1, 'test', $2, $2, $3, $4, $5, \
                 CASE WHEN $6 THEN NOW() ELSE NULL END, $7, $8, $9, $10, $11)",
    )
    .bind(id)
    .bind(attempted)
    .bind(loaded)
    .bind(no_op)
    .bind(rejected)
    .bind(ended)
    .bind(owner_id)
    .bind(range_group_id)
    .bind(from)
    .bind(to)
    .bind(range.map(|_| 1_i32))
    .execute(pool)
    .await
    .expect("insert legacy run");
    id
}

async fn run_status(pool: &PgPool, id: Uuid) -> (String, Option<DateTime<Utc>>) {
    sqlx::query_as("SELECT terminal_status, ended_at FROM reference_data_load_runs WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("run status")
}

async fn view_row(pool: &PgPool, entry_id: Uuid) -> (String, String, i32, i32, Option<String>) {
    sqlx::query_as(
        "SELECT entry_type, status, dates_requested, dates_completed, owner_id \
         FROM data_load_history WHERE entry_id = $1",
    )
    .bind(entry_id)
    .fetch_one(pool)
    .await
    .expect("view row")
}

#[tokio::test]
async fn migration_creates_auth_ownership_schema_and_backfills_legacy_rows() {
    let Some((pool, _guard)) = tx_pool().await else {
        eprintln!("SKIPPED: no test database configured");
        return;
    };

    // Normalize a database left behind by an interrupted earlier test run,
    // then prove a fresh execution is a no-op. The current history view has
    // columns added by a later migration, so stash it while 0014 recreates its
    // own shape.
    let view_exists: bool =
        sqlx::query_scalar("SELECT to_regclass('data_load_history') IS NOT NULL")
            .fetch_one(&pool)
            .await
            .expect("view exists");
    if view_exists {
        sqlx::query("ALTER VIEW data_load_history RENAME TO data_load_history_saved")
            .execute(&pool)
            .await
            .expect("stash history view");
    }
    sqlx::raw_sql(MIGRATION_0014)
        .execute(&pool)
        .await
        .expect("re-apply migration 0014");
    sqlx::raw_sql(MIGRATION_0014)
        .execute(&pool)
        .await
        .expect("migration 0014 is idempotent");

    // ---------------------------------------------------------------------
    // actors
    // ---------------------------------------------------------------------
    assert_eq!(
        column_names(&pool, "actors").await,
        vec![
            "id",
            "google_sub",
            "email",
            "created_at",
            "updated_at",
            "last_login_at",
        ]
    );
    let actor_columns = columns(&pool, "actors").await;
    let google_sub = actor_columns
        .iter()
        .find(|row| row.0 == "google_sub")
        .expect("google_sub");
    assert_eq!(google_sub.1, "text");
    assert_eq!(google_sub.2, "YES");
    let email = actor_columns
        .iter()
        .find(|row| row.0 == "email")
        .expect("email");
    assert_eq!(email.2, "NO");

    let actor_indexes = index_definitions(&pool, "actors").await;
    assert!(
        actor_indexes
            .iter()
            .any(|def| def.contains("UNIQUE") && def.contains("google_sub")),
        "google_sub unique index missing: {actor_indexes:?}"
    );
    assert!(
        actor_indexes
            .iter()
            .any(|def| def.contains("UNIQUE") && def.contains("lower(email)")),
        "lower(email) unique index missing: {actor_indexes:?}"
    );

    let actor_email = format!("actor-{}@example.test", Uuid::new_v4());
    sqlx::query("INSERT INTO actors (email) VALUES ($1)")
        .bind(&actor_email)
        .execute(&pool)
        .await
        .expect("insert actor");
    sqlx::query("INSERT INTO actors (email) VALUES ($1)")
        .bind(actor_email.to_uppercase())
        .execute(&pool)
        .await
        .expect_err("lower(email) must be unique");

    // ---------------------------------------------------------------------
    // load_range_groups
    // ---------------------------------------------------------------------
    assert_eq!(
        column_names(&pool, "load_range_groups").await,
        vec![
            "id",
            "owner_id",
            "client_group_id",
            "from_date",
            "to_date",
            "concurrency",
            "status",
            "dates_requested",
            "cancel_requested_at",
            "started_at",
            "ended_at",
            "created_at",
        ]
    );

    let client_group = Uuid::new_v4();
    let from = chrono::NaiveDate::from_ymd_opt(2026, 1, 1).expect("from");
    let to = chrono::NaiveDate::from_ymd_opt(2026, 1, 10).expect("to");
    let group_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO load_range_groups \
             (id, owner_id, client_group_id, from_date, to_date, concurrency, \
              status, dates_requested, ended_at) \
         VALUES ($1, 'owner-a', $2, $3, $4, 4, 'cancelled', 10, NOW())",
    )
    .bind(group_id)
    .bind(client_group)
    .bind(from)
    .bind(to)
    .execute(&pool)
    .await
    .expect("insert group");

    sqlx::query(
        "INSERT INTO load_range_groups \
             (owner_id, client_group_id, from_date, to_date, concurrency, dates_requested) \
         VALUES ('owner-a', $1, $2, $2, 1, 1)",
    )
    .bind(client_group)
    .bind(from)
    .execute(&pool)
    .await
    .expect_err("(owner_id, client_group_id) must be unique");

    sqlx::query(
        "INSERT INTO load_range_groups \
             (owner_id, client_group_id, from_date, to_date, concurrency, dates_requested) \
         VALUES ('owner-b', $1, $2, $2, 1, 1)",
    )
    .bind(client_group)
    .bind(from)
    .execute(&pool)
    .await
    .expect("the client group id is unique per owner, not globally");

    for (label, from_date, to_date, concurrency, status) in [
        ("reversed range", to, from, 1, "running"),
        ("zero concurrency", from, to, 0, "running"),
        ("excess concurrency", from, to, 33, "running"),
        ("unknown status", from, to, 1, "bogus"),
    ] {
        sqlx::query(
            "INSERT INTO load_range_groups \
                 (owner_id, client_group_id, from_date, to_date, concurrency, status, dates_requested) \
             VALUES ('owner-c', $1, $2, $3, $4, $5, 1)",
        )
        .bind(Uuid::new_v4())
        .bind(from_date)
        .bind(to_date)
        .bind(concurrency)
        .bind(status)
        .execute(&pool)
        .await
        .expect_err(&format!("{label} must be rejected"));
    }

    sqlx::query(
        "INSERT INTO load_range_groups \
             (owner_id, client_group_id, from_date, to_date, concurrency, dates_requested, \
              started_at, ended_at) \
         VALUES ('owner-c', $1, $2, $3, 1, 1, NOW(), NOW() - INTERVAL '1 second')",
    )
    .bind(Uuid::new_v4())
    .bind(from)
    .bind(to)
    .execute(&pool)
    .await
    .expect_err("ended_at must not precede started_at");

    // A completed child for the cancelled group: the view must keep the
    // group's requested count and status while aggregating dated work.
    insert_legacy_run(
        &pool,
        2,
        2,
        0,
        true,
        Some("owner-a"),
        Some(group_id),
        Some((from, to)),
    )
    .await;

    // ---------------------------------------------------------------------
    // reference_data_load_runs / automation_runs / automation_context
    // ---------------------------------------------------------------------
    let run_columns = columns(&pool, "reference_data_load_runs").await;
    let terminal = run_columns
        .iter()
        .find(|row| row.0 == "terminal_status")
        .expect("terminal_status");
    assert_eq!(terminal.2, "NO");
    assert!(
        terminal
            .3
            .as_deref()
            .is_some_and(|default| default.contains("running")),
        "terminal_status must default to running: {terminal:?}"
    );
    let owner = run_columns
        .iter()
        .find(|row| row.0 == "owner_id")
        .expect("owner_id");
    assert_eq!(owner.1, "text");
    assert_eq!(owner.2, "YES");
    assert!(
        index_definitions(&pool, "reference_data_load_runs")
            .await
            .iter()
            .any(|def| def.contains("owner_id") && def.contains("started_at")),
        "reference_data_load_runs owner/started index missing"
    );

    sqlx::query(
        "INSERT INTO reference_data_load_runs \
             (trigger, identifiers_requested_raw, identifiers_attempted, terminal_status) \
         VALUES ('test', 0, 0, 'bogus')",
    )
    .execute(&pool)
    .await
    .expect_err("terminal_status must be constrained");

    let automation_run_columns = columns(&pool, "automation_runs").await;
    assert!(
        automation_run_columns.iter().any(|row| row.0 == "owner_id"),
        "automation_runs.owner_id missing"
    );
    assert!(
        index_definitions(&pool, "automation_runs")
            .await
            .iter()
            .any(|def| def.contains("owner_id") && def.contains("created_at")),
        "automation_runs owner/created index missing"
    );

    let context_columns = columns(&pool, "automation_context").await;
    let context_owner = context_columns
        .iter()
        .find(|row| row.0 == "owner_id")
        .expect("automation_context.owner_id");
    assert_eq!(context_owner.2, "NO");
    assert_eq!(
        primary_key_columns(&pool, "automation_context").await,
        vec!["owner_id", "scope", "scope_id", "key"]
    );
    sqlx::query(
        "INSERT INTO automation_context (owner_id, scope, scope_id, key, value) \
         VALUES ('owner-a', 'global', 'workspace', 'k', '{}'::jsonb), \
                ('owner-b', 'global', 'workspace', 'k', '{}'::jsonb)",
    )
    .execute(&pool)
    .await
    .expect("workspace context is per owner");
    sqlx::query(
        "INSERT INTO automation_context (owner_id, scope, scope_id, key, value) \
         VALUES ('owner-a', 'global', 'workspace', 'k', '{}'::jsonb)",
    )
    .execute(&pool)
    .await
    .expect_err("context key must be unique per owner");

    // ---------------------------------------------------------------------
    // data_load_history view
    // ---------------------------------------------------------------------
    let view_columns = column_names(&pool, "data_load_history").await;
    assert_eq!(view_columns.last().map(String::as_str), Some("owner_id"));
    assert_eq!(
        view_columns.len(),
        35,
        "existing view columns plus owner_id"
    );

    assert_eq!(
        view_row(&pool, group_id).await,
        (
            "range_run".to_string(),
            "cancelled".to_string(),
            10,
            1,
            Some("owner-a".to_string()),
        )
    );

    let ungrouped = insert_legacy_run(&pool, 2, 2, 0, true, Some("owner-a"), None, None).await;
    assert_eq!(
        view_row(&pool, ungrouped).await,
        (
            "load_run".to_string(),
            "complete".to_string(),
            1,
            1,
            Some("owner-a".to_string()),
        )
    );

    let legacy_group = Uuid::new_v4();
    insert_legacy_run(
        &pool,
        1,
        1,
        0,
        true,
        Some("owner-z"),
        Some(legacy_group),
        Some((from, to)),
    )
    .await;
    assert_eq!(
        view_row(&pool, legacy_group).await,
        (
            "range_run".to_string(),
            "complete".to_string(),
            10,
            1,
            Some("owner-z".to_string()),
        )
    );

    let change_id: Uuid = sqlx::query_scalar(
        "INSERT INTO data_source_state_changes \
             (source_id, before_enabled, after_enabled) \
         VALUES ('yahoo_finance', true, false) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .expect("source change");
    assert_eq!(
        view_row(&pool, change_id).await,
        (
            "source_change".to_string(),
            "source_disabled".to_string(),
            0,
            0,
            None,
        )
    );

    // ---------------------------------------------------------------------
    // Legacy backfill: terminal statuses, owner inheritance, context key
    // ---------------------------------------------------------------------
    let complete = insert_legacy_run(&pool, 2, 2, 0, true, Some("legacy-owner"), None, None).await;
    let partial = insert_legacy_run(&pool, 2, 1, 1, true, None, None, None).await;
    let failed = insert_legacy_run(&pool, 2, 0, 2, true, None, None, None).await;
    let open = insert_legacy_run(&pool, 1, 0, 0, false, None, None, None).await;

    let flow_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO automation_flows (id, owner_id, name) VALUES ($1, 'flow-owner', 'Migration Flow')",
    )
    .bind(flow_id)
    .execute(&pool)
    .await
    .expect("flow");
    let version_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO automation_flow_versions \
             (id, flow_id, status, flow_json, created_by) \
         VALUES ($1, $2, 'draft', '{}'::jsonb, 'test')",
    )
    .bind(version_id)
    .bind(flow_id)
    .execute(&pool)
    .await
    .expect("flow version");
    let automation_run: Uuid = sqlx::query_scalar(
        "INSERT INTO automation_runs \
             (id, flow_id, flow_version_id, trigger_kind, status, owner_id) \
         VALUES ($1, $2, $3, 'manual', 'succeeded', NULL) RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(flow_id)
    .bind(version_id)
    .fetch_one(&pool)
    .await
    .expect("automation run");

    // Recreate the pre-0014 shape to seed unresolved context rows: no
    // per-owner key and a nullable owner column.
    sqlx::query("ALTER TABLE automation_context DROP CONSTRAINT automation_context_pkey")
        .execute(&pool)
        .await
        .expect("drop per-owner key");
    sqlx::query("ALTER TABLE automation_context ALTER COLUMN owner_id DROP NOT NULL")
        .execute(&pool)
        .await
        .expect("make owner nullable for the fixture");
    sqlx::query(
        "INSERT INTO automation_context (owner_id, scope, scope_id, key, value) \
         VALUES (NULL, 'flow', $1, 'flow-key', '{}'::jsonb), \
                (NULL, 'global', 'workspace', 'global-key', '{}'::jsonb)",
    )
    .bind(flow_id.to_string())
    .execute(&pool)
    .await
    .expect("legacy contexts");

    sqlx::raw_sql(MIGRATION_0014)
        .execute(&pool)
        .await
        .expect("migration backfill");

    assert_eq!(run_status(&pool, complete).await.0, "complete");
    assert_eq!(run_status(&pool, partial).await.0, "partial");
    assert_eq!(run_status(&pool, failed).await.0, "failed");
    let (open_status, open_ended) = run_status(&pool, open).await;
    assert_eq!(open_status, "interrupted");
    assert!(open_ended.is_some(), "open rows are closed as interrupted");

    assert_eq!(
        view_row(&pool, open).await.1,
        "interrupted",
        "the history view exposes the interrupted status"
    );
    assert_eq!(
        view_row(&pool, complete).await.4.as_deref(),
        Some("legacy-owner"),
        "existing owner ids are preserved"
    );

    let automation_owner: Option<String> =
        sqlx::query_scalar("SELECT owner_id FROM automation_runs WHERE id = $1")
            .bind(automation_run)
            .fetch_one(&pool)
            .await
            .expect("automation owner");
    assert_eq!(automation_owner.as_deref(), Some("flow-owner"));

    let context_owners: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT scope, key, owner_id FROM automation_context \
         WHERE scope_id = $1 OR key = 'global-key' ORDER BY key",
    )
    .bind(flow_id.to_string())
    .fetch_all(&pool)
    .await
    .expect("context owners");
    assert_eq!(
        context_owners,
        vec![
            (
                "flow".to_string(),
                "flow-key".to_string(),
                "flow-owner".to_string()
            ),
            (
                "global".to_string(),
                "global-key".to_string(),
                "dev-user".to_string()
            ),
        ]
    );
    assert_eq!(
        primary_key_columns(&pool, "automation_context").await,
        vec!["owner_id", "scope", "scope_id", "key"]
    );
    let context_columns = columns(&pool, "automation_context").await;
    assert_eq!(
        context_columns
            .iter()
            .find(|row| row.0 == "owner_id")
            .expect("owner_id")
            .2,
        "NO"
    );

    // The cancelled group still tracks its requested range and its child.
    assert_eq!(
        view_row(&pool, group_id).await,
        (
            "range_run".to_string(),
            "cancelled".to_string(),
            10,
            1,
            Some("owner-a".to_string()),
        )
    );

    if view_exists {
        sqlx::query("DROP VIEW data_load_history")
            .execute(&pool)
            .await
            .expect("drop 0014 view");
        sqlx::query("ALTER VIEW data_load_history_saved RENAME TO data_load_history")
            .execute(&pool)
            .await
            .expect("restore history view");
    }
}
