//! Core management tests for calendars and assignments.
//!
//! Skipped as a passing no-op when no test database is configured.

use chrono::NaiveDate;

use obolargus_core::common::tx_pool;
use obolargus_core::domain::asset::create::create_asset_with_identifier;
use obolargus_core::domain::asset::entity::NewAsset;
use obolargus_core::market_data::calendar::{
    CalendarCache, affected_assets, asset_closed_on, count_assignments_for_calendar,
    create_assignment, export_document, get_calendar, get_calendar_detail, list_assignment_views,
    list_assignments_for_calendar, list_calendar_audit, list_calendar_summaries, list_calendars,
    remove_assignment, replace_calendar, retire_calendar, seed_bundled_calendars,
    validate_document,
};

fn date(value: &str) -> NaiveDate {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("date")
}

#[tokio::test]
async fn import_is_atomic_versioned_and_audited() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    seed_bundled_calendars(&pool).await.expect("seed");
    let stored = get_calendar(&pool, "XNAS")
        .await
        .expect("read")
        .expect("calendar");
    let mut document = obolargus_core::market_data::calendar::export_document(&stored);
    document.name = "Nasdaq Renamed".to_string();
    let next = replace_calendar(&pool, document.clone(), Some(1), "owner")
        .await
        .expect("import");
    assert_eq!(next.version, 2);
    assert_eq!(next.name, "Nasdaq Renamed");

    let unchanged = replace_calendar(&pool, document, Some(2), "owner")
        .await
        .expect("re-import");
    assert_eq!(unchanged.version, 2, "identical import must not churn");

    let conflict = replace_calendar(
        &pool,
        obolargus_core::market_data::calendar::export_document(&stored),
        Some(1),
        "owner",
    )
    .await
    .expect_err("stale version");
    assert!(conflict.to_string().contains("version"));

    let mut invalid = obolargus_core::market_data::calendar::export_document(&stored);
    invalid.mic = "xn".to_string();
    assert!(validate_document(invalid).is_err());

    let audit = list_calendar_audit(&pool, "XNAS", 20).await.expect("audit");
    assert!(audit.iter().any(|entry| entry.action == "imported"));
    guard.cleanup().await;
}

#[tokio::test]
async fn assignments_reject_overlaps_and_retire_requires_confirmation() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    seed_bundled_calendars(&pool).await.expect("seed");
    let asset = create_asset_with_identifier(
        &pool,
        NewAsset {
            asset_type: "stock".into(),
            display_name: "Managed Equity".into(),
            currency: "USD".into(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
        "TICKER:XNAS",
        "MGMT",
    )
    .await
    .expect("asset")
    .id();

    let first = create_assignment(&pool, asset.0, "XNAS", date("2020-01-01"), None, "owner")
        .await
        .expect("assignment");
    let overlap = create_assignment(&pool, asset.0, "XNYS", date("2021-01-01"), None, "owner")
        .await
        .expect_err("overlap");
    assert!(overlap.to_string().contains("overlap"));

    let detail = get_calendar_detail(&pool, "XNAS", date("2026-11-24"))
        .await
        .expect("detail");
    assert!(detail.assigned_asset_count >= 1);
    let in_use = affected_assets(&pool, "XNAS", date("2026-11-24"))
        .await
        .expect("affected");
    assert!(in_use.affected_count >= 1);

    let refused = retire_calendar(&pool, "XNAS", "owner", false)
        .await
        .expect_err("confirmation required");
    assert!(refused.to_string().contains("calendar_in_use"));
    let retired = retire_calendar(&pool, "XNAS", "owner", true)
        .await
        .expect("retire");
    assert!(!retired.active);

    remove_assignment(&pool, first.id, "owner")
        .await
        .expect("remove");
    let observations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM market_observations")
        .fetch_one(&pool)
        .await
        .expect("observations");
    assert_eq!(observations, 0, "calendar writes never create price rows");
    guard.cleanup().await;
}

#[tokio::test]
async fn validation_rejects_each_invalid_document_shape() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    seed_bundled_calendars(&pool).await.expect("seed");
    let stored = get_calendar(&pool, "XNAS")
        .await
        .expect("read")
        .expect("calendar");
    let base = export_document(&stored);

    let mut cases: Vec<(
        &str,
        obolargus_core::market_data::calendar::CalendarDocument,
    )> = Vec::new();
    let mut doc = base.clone();
    doc.name = "  ".into();
    cases.push(("empty name", doc));
    let mut doc = base.clone();
    doc.time_zone = "Mars/Olympus".into();
    cases.push(("bad zone", doc));
    let mut doc = base.clone();
    doc.coverage_from = date("2027-01-01");
    doc.coverage_to = date("2026-01-01");
    cases.push(("reversed coverage", doc));
    let mut doc = base.clone();
    doc.weekdays.clear();
    cases.push(("no weekdays", doc));
    let mut doc = base.clone();
    doc.weekdays[0].iso_weekday = 0;
    cases.push(("weekday range", doc));
    let mut doc = base.clone();
    doc.weekdays.push(doc.weekdays[0].clone());
    cases.push(("duplicate weekday", doc));
    let mut doc = base.clone();
    doc.weekdays[0].open = "17:00".into();
    doc.weekdays[0].close = "16:00".into();
    cases.push(("weekday order", doc));
    let mut doc = base.clone();
    doc.weekdays[0].open = "not-a-time".into();
    cases.push(("bad time", doc));
    let mut doc = base.clone();
    doc.exceptions[0].date = date("1999-01-01");
    cases.push(("exception outside coverage", doc));
    let mut doc = base.clone();
    doc.exceptions.push(doc.exceptions[0].clone());
    cases.push(("duplicate exception", doc));
    let mut doc = base.clone();
    doc.exceptions[0].open = Some("09:30".into());
    cases.push(("closure with time", doc));
    let mut doc = base.clone();
    doc.exceptions[0].kind = obolargus_core::market_data::calendar::CalendarExceptionKind::Session;
    doc.exceptions[0].open = None;
    doc.exceptions[0].close = Some("13:00".into());
    cases.push(("session missing open", doc));
    let mut doc = base.clone();
    doc.coverage_to = date("2080-01-01");
    cases.push(("over 50 years", doc));

    for (label, document) in cases {
        assert!(
            validate_document(document).is_err(),
            "{label} must be rejected"
        );
    }
    guard.cleanup().await;
}

#[tokio::test]
async fn reads_and_cache_expose_management_lookups() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    seed_bundled_calendars(&pool).await.expect("seed");
    let asset = create_asset_with_identifier(
        &pool,
        NewAsset {
            asset_type: "stock".into(),
            display_name: "Lookup Equity".into(),
            currency: "USD".into(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
        "TICKER:XNAS",
        "LOOKUP",
    )
    .await
    .expect("asset")
    .id();
    create_assignment(&pool, asset.0, "XNAS", date("2020-01-01"), None, "owner")
        .await
        .expect("assignment");

    let calendars = list_calendars(&pool).await.expect("calendars");
    assert_eq!(calendars.len(), 6);
    let summaries = list_calendar_summaries(&pool, date("2026-11-24"))
        .await
        .expect("summaries");
    assert!(
        summaries
            .iter()
            .any(|summary| summary.mic == "XNAS" && summary.assigned_asset_count >= 1)
    );
    let assignments = list_assignments_for_calendar(&pool, "xnas", 50, 0)
        .await
        .expect("assignments");
    assert_eq!(assignments.len(), 1);
    assert_eq!(
        count_assignments_for_calendar(&pool, "XNAS")
            .await
            .expect("count"),
        1
    );
    let page = list_assignment_views(&pool, Some(asset.0), None, 50, 0)
        .await
        .expect("views");
    assert_eq!(page.total, 1);
    assert_eq!(page.assignments[0].mic, "XNAS");

    let mut cache = CalendarCache::new(date("2025-11-26"), date("2025-11-28"));
    assert!(!cache.is_closed(&pool, asset, date("2025-11-26")).await);
    assert!(cache.is_closed(&pool, asset, date("2025-11-27")).await);
    assert!(!cache.is_closed(&pool, asset, date("2025-11-28")).await);
    assert!(format!("{cache:?}").contains("assets"));
    assert!(asset_closed_on(&pool, asset, date("2025-11-27")).await);
    assert!(!asset_closed_on(&pool, asset, date("2025-11-26")).await);

    let no_venue = create_asset_with_identifier(
        &pool,
        NewAsset {
            asset_type: "stock".into(),
            display_name: "No Venue".into(),
            currency: "USD".into(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
        "ISIN",
        "US0000000009",
    )
    .await
    .expect("asset")
    .id();
    assert!(!asset_closed_on(&pool, no_venue, date("2025-11-27")).await);
    guard.cleanup().await;
}

#[tokio::test]
async fn more_document_and_write_edges_are_rejected_cleanly() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    seed_bundled_calendars(&pool).await.expect("seed");
    let stored = get_calendar(&pool, "XNAS")
        .await
        .expect("read")
        .expect("calendar");
    let base = export_document(&stored);

    let mut doc = base.clone();
    doc.source = " ".into();
    assert!(validate_document(doc).is_err());
    let mut doc = base.clone();
    doc.exceptions[0].kind = obolargus_core::market_data::calendar::CalendarExceptionKind::Session;
    doc.exceptions[0].open = Some("16:00".into());
    doc.exceptions[0].close = Some("09:30".into());
    assert!(validate_document(doc).is_err());
    let mut doc = base.clone();
    doc.exceptions[0].kind =
        obolargus_core::market_data::calendar::CalendarExceptionKind::EarlyClose;
    doc.exceptions[0].open = None;
    doc.exceptions[0].close = None;
    assert!(validate_document(doc).is_err());
    let mut doc = base.clone();
    doc.exceptions[0].kind =
        obolargus_core::market_data::calendar::CalendarExceptionKind::EarlyClose;
    doc.exceptions[0].open = Some("14:00".into());
    doc.exceptions[0].close = Some("13:00".into());
    assert!(validate_document(doc).is_err());

    let mut inactive = base.clone();
    inactive.active = false;
    assert!(
        replace_calendar(&pool, inactive, None, "owner")
            .await
            .is_err()
    );
    let created = replace_calendar(
        &pool,
        {
            let mut doc = base.clone();
            doc.mic = "XNEW".into();
            doc
        },
        Some(0),
        "owner",
    )
    .await
    .expect("create");
    assert_eq!(created.version, 1);
    assert!(
        replace_calendar(
            &pool,
            {
                let mut doc = base.clone();
                doc.mic = "XNEW".into();
                doc
            },
            Some(9),
            "owner",
        )
        .await
        .is_err()
    );

    let asset = create_asset_with_identifier(
        &pool,
        NewAsset {
            asset_type: "stock".into(),
            display_name: "Edge Equity".into(),
            currency: "USD".into(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
        "TICKER:XNAS",
        "EDGE",
    )
    .await
    .expect("asset")
    .id();
    assert!(
        create_assignment(
            &pool,
            asset.0,
            "XNAS",
            date("2026-01-02"),
            Some(date("2026-01-01")),
            "owner",
        )
        .await
        .is_err()
    );
    assert!(
        create_assignment(&pool, asset.0, "ZZZZ", date("2026-01-01"), None, "owner",)
            .await
            .is_err()
    );
    retire_calendar(&pool, "XNAS", "owner", true)
        .await
        .expect("retire");
    assert!(
        create_assignment(&pool, asset.0, "XNAS", date("2026-01-01"), None, "owner",)
            .await
            .is_err()
    );
    assert!(
        remove_assignment(&pool, uuid::Uuid::new_v4(), "owner")
            .await
            .is_err()
    );
    guard.cleanup().await;
}

#[tokio::test]
async fn coverage_and_enum_edges_are_explicit() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    seed_bundled_calendars(&pool).await.expect("seed");
    let asset = create_asset_with_identifier(
        &pool,
        NewAsset {
            asset_type: "stock".into(),
            display_name: "Coverage Edge".into(),
            currency: "USD".into(),
            backed_by_portfolio_id: None,
            issuer_id: None,
        },
        "TICKER:XNAS",
        "COVEDGE",
    )
    .await
    .expect("asset")
    .id();
    assert!(
        obolargus_core::market_data::calendar::asset_coverage(
            &pool,
            asset,
            date("2026-11-20"),
            date("2026-11-19"),
            chrono::Utc::now(),
        )
        .await
        .is_err()
    );
    assert!(get_calendar(&pool, "ZZZZ").await.expect("read").is_none());
    assert!(
        obolargus_core::market_data::calendar::resolve_segments(
            date("2026-11-20"),
            date("2026-11-19"),
            &[],
            &[],
            &std::collections::BTreeMap::new(),
        )
        .is_empty()
    );
    use obolargus_core::market_data::calendar::CalendarExceptionKind;
    assert_eq!(
        CalendarExceptionKind::parse("session"),
        Some(CalendarExceptionKind::Session)
    );
    assert_eq!(CalendarExceptionKind::Session.as_str(), "session");
    assert_eq!(CalendarExceptionKind::parse("bogus"), None);
    guard.cleanup().await;
}
