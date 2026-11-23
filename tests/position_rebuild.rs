//! Rebuild/verify and rollback tests for the internal position path
//! (FR-028/FR-029).
//!
//! Database-gated through the shared `tx_pool` fixture.

use chrono::{NaiveDate, Utc};
use obolargus_core::common::tx_pool;
use obolargus_core::domain::asset::create_asset;
use obolargus_core::domain::asset::entity::NewAsset;
use obolargus_core::domain::event::create::create_event;
use obolargus_core::domain::event::entity::{EventType, NewEvent};
use obolargus_core::domain::event::read::list_events_for_position;
use obolargus_core::domain::id::{AssetId, PortfolioId};
use obolargus_core::domain::portfolio::create_portfolio;
use obolargus_core::domain::portfolio::entity::NewPortfolio;
use obolargus_core::domain::position::get_position;
use obolargus_core::domain::position::rebuild::{rebuild_position, verify_position};
use obolargus_core::domain::position::replay::replay_events;
use rust_decimal::Decimal;
use sqlx::{AssertSqlSafe, PgPool};

async fn portfolio(pool: &PgPool, name: &str) -> PortfolioId {
    create_portfolio(
        pool,
        NewPortfolio {
            name: name.to_string(),
            purpose: "rebuild-test".to_string(),
            created_by: "tester".to_string(),
            parent_id: None,
        },
    )
    .await
    .expect("create portfolio")
    .id()
}

async fn asset(pool: &PgPool) -> AssetId {
    create_asset(
        pool,
        NewAsset {
            asset_type: "stock".to_string(),
            display_name: format!("Rebuild {}", uuid::Uuid::new_v4()),
            currency: "USD".to_string(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
    )
    .await
    .expect("create asset")
    .id()
}

fn event(
    event_type: EventType,
    effective_date: NaiveDate,
    quantity: i64,
    portfolio_id: PortfolioId,
    asset_id: AssetId,
    to_portfolio_id: Option<PortfolioId>,
) -> NewEvent {
    NewEvent {
        event_type,
        effective_date,
        quantity_delta: Decimal::from(quantity),
        portfolio_id,
        asset_id,
        to_portfolio_id,
    }
}

#[tokio::test]
async fn rebuild_repairs_drift_and_is_idempotent() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let portfolio_id = portfolio(&pool, "Drift").await;
    let asset_id = asset(&pool).await;
    let today = Utc::now().date_naive();

    create_event(
        &pool,
        event(EventType::Buy, today, 100, portfolio_id, asset_id, None),
    )
    .await
    .expect("buy");
    create_event(
        &pool,
        event(EventType::Buy, today, 50, portfolio_id, asset_id, None),
    )
    .await
    .expect("buy");
    create_event(
        &pool,
        event(EventType::Sell, today, -30, portfolio_id, asset_id, None),
    )
    .await
    .expect("sell");

    sqlx::query("UPDATE positions SET quantity = 999 WHERE portfolio_id = $1 AND asset_id = $2")
        .bind(portfolio_id.0)
        .bind(asset_id.0)
        .execute(&pool)
        .await
        .expect("corrupt position");

    assert!(
        !verify_position(&pool, portfolio_id, asset_id)
            .await
            .expect("verify drift"),
        "the corrupted row must be detected"
    );

    let repaired = rebuild_position(&pool, portfolio_id, asset_id)
        .await
        .expect("rebuild");
    assert_eq!(repaired.quantity, Decimal::from(120));
    assert!(
        verify_position(&pool, portfolio_id, asset_id)
            .await
            .expect("verify repaired"),
        "rebuild repairs the row"
    );

    let again = rebuild_position(&pool, portfolio_id, asset_id)
        .await
        .expect("rebuild again");
    assert_eq!(again, repaired, "rebuild is idempotent");

    guard.cleanup().await;
}

#[tokio::test]
async fn incremental_materialization_matches_replay_for_transfers() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let source = portfolio(&pool, "Equivalence source").await;
    let destination = portfolio(&pool, "Equivalence destination").await;
    let asset_id = asset(&pool).await;
    let today = Utc::now().date_naive();
    let backdated = NaiveDate::from_ymd_opt(2020, 1, 2).expect("date");

    create_event(
        &pool,
        event(EventType::Buy, today, 100, source, asset_id, None),
    )
    .await
    .expect("buy");
    create_event(
        &pool,
        event(EventType::Split, today, 100, source, asset_id, None),
    )
    .await
    .expect("split");
    create_event(
        &pool,
        event(
            EventType::Transfer,
            today,
            -30,
            source,
            asset_id,
            Some(destination),
        ),
    )
    .await
    .expect("transfer");
    // Backdated after the fact: as_of must stay at the latest knowledge time.
    create_event(
        &pool,
        event(EventType::Buy, backdated, 10, source, asset_id, None),
    )
    .await
    .expect("backdated buy");

    let incremental_source = get_position(&pool, source, asset_id)
        .await
        .expect("source position");
    let incremental_destination = get_position(&pool, destination, asset_id)
        .await
        .expect("destination position");
    assert_eq!(incremental_source.quantity, Decimal::from(180));
    assert_eq!(incremental_destination.quantity, Decimal::from(30));

    let rebuilt_source = rebuild_position(&pool, source, asset_id)
        .await
        .expect("rebuild source");
    let rebuilt_destination = rebuild_position(&pool, destination, asset_id)
        .await
        .expect("rebuild destination");
    assert_eq!(rebuilt_source.quantity, incremental_source.quantity);
    assert_eq!(
        rebuilt_destination.quantity,
        incremental_destination.quantity
    );
    assert_eq!(rebuilt_source.as_of, incremental_source.as_of);
    assert_eq!(rebuilt_destination.as_of, incremental_destination.as_of);

    // The source replay is the same arithmetic the incremental path applied.
    let events = list_events_for_position(&pool, source, asset_id)
        .await
        .expect("events");
    let replayed = replay_events(source, asset_id, &events).expect("replay");
    assert_eq!(replayed.position.quantity, incremental_source.quantity);
    assert!(
        verify_position(&pool, source, asset_id)
            .await
            .expect("verify source")
    );
    assert!(
        verify_position(&pool, destination, asset_id)
            .await
            .expect("verify destination")
    );

    guard.cleanup().await;
}

#[tokio::test]
async fn materialization_failure_rolls_back_event_and_position() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let portfolio_id = portfolio(&pool, "Rollback").await;
    let asset_id = asset(&pool).await;

    // Force only this test's position write to fail, after the event insert
    // has already succeeded inside the transaction.
    sqlx::query("DROP TRIGGER IF EXISTS event_materialization_failure ON positions")
        .execute(&pool)
        .await
        .expect("drop trigger");
    sqlx::query("DROP FUNCTION IF EXISTS event_materialization_failure()")
        .execute(&pool)
        .await
        .expect("drop function");
    let create_function = format!(
        r#"
        CREATE FUNCTION event_materialization_failure() RETURNS trigger
        LANGUAGE plpgsql AS $$
        BEGIN
            IF NEW.asset_id = '{asset_id}'::uuid THEN
                RAISE EXCEPTION 'injected materialization failure';
            END IF;
            RETURN NEW;
        END;
        $$
        "#
    );
    sqlx::query(AssertSqlSafe(create_function))
        .execute(&pool)
        .await
        .expect("create function");
    sqlx::query(
        "CREATE TRIGGER event_materialization_failure \
         BEFORE INSERT OR UPDATE ON positions \
         FOR EACH ROW EXECUTE FUNCTION event_materialization_failure()",
    )
    .execute(&pool)
    .await
    .expect("create trigger");

    let failed = create_event(
        &pool,
        event(
            EventType::Buy,
            Utc::now().date_naive(),
            100,
            portfolio_id,
            asset_id,
            None,
        ),
    )
    .await;

    let events: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE portfolio_id = $1 AND asset_id = $2")
            .bind(portfolio_id.0)
            .bind(asset_id.0)
            .fetch_one(&pool)
            .await
            .expect("count events");
    let position: Option<Decimal> = sqlx::query_scalar(
        "SELECT quantity FROM positions WHERE portfolio_id = $1 AND asset_id = $2",
    )
    .bind(portfolio_id.0)
    .bind(asset_id.0)
    .fetch_optional(&pool)
    .await
    .expect("position row");

    sqlx::query("DROP TRIGGER event_materialization_failure ON positions")
        .execute(&pool)
        .await
        .expect("drop trigger");
    sqlx::query("DROP FUNCTION event_materialization_failure()")
        .execute(&pool)
        .await
        .expect("drop function");

    assert!(failed.is_err(), "materialization failure surfaces");
    assert_eq!(events, 0, "the event insert rolled back");
    assert!(position.is_none(), "no partial position remains");

    guard.cleanup().await;
}
