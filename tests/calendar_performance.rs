//! Reference-deployment performance bound for calendar coverage reads.
//!
//! Skipped as a passing no-op when no test database is configured.

use std::time::Instant;

use chrono::{NaiveDate, Utc};

use obolargus_core::common::tx_pool;
use obolargus_core::domain::asset::create::create_asset_with_identifier;
use obolargus_core::domain::asset::entity::NewAsset;
use obolargus_core::market_data::calendar::{asset_coverage, seed_bundled_calendars};

#[tokio::test]
async fn a_366_day_coverage_read_stays_under_the_ci_ceiling() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    seed_bundled_calendars(&pool).await.expect("seed");
    let asset = create_asset_with_identifier(
        &pool,
        NewAsset {
            asset_type: "stock".into(),
            display_name: "Performance Equity".into(),
            currency: "USD".into(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
        "TICKER:XNAS",
        "PERF",
    )
    .await
    .expect("asset")
    .id();
    let to = NaiveDate::from_ymd_opt(2025, 12, 31).expect("date");
    let from = NaiveDate::from_ymd_opt(2024, 12, 31).expect("date");

    // Warm the calendar cache and the connection.
    asset_coverage(&pool, asset, from, to, Utc::now())
        .await
        .expect("warm coverage");

    let mut samples = Vec::new();
    for _ in 0..5 {
        let started = Instant::now();
        let coverage = asset_coverage(&pool, asset, from, to, Utc::now())
            .await
            .expect("coverage");
        samples.push(started.elapsed().as_millis());
        assert!(!coverage.missing_sessions.is_empty());
    }
    samples.sort_unstable();
    let median = samples[samples.len() / 2];
    eprintln!("calendar coverage median {median} ms; samples {samples:?}");
    assert!(
        median < 500,
        "median {median} ms exceeds the 500 ms CI-safe ceiling: {samples:?}"
    );
    guard.cleanup().await;
}
