//! Unit tests for the position-replay function. Integration tests that
//! require a real PostgreSQL database are gated by `DATABASE_URL` and live
//! in `obolargus_core/tests/integration.rs` (out of scope for this
//! iteration because no `DATABASE_URL` is available in the CI sandbox).

use chrono::{NaiveDate, TimeZone, Utc};
use rust_decimal::Decimal;
use uuid::Uuid;

use obolargus_core::domain::event::entity::{Event, EventType};
use obolargus_core::domain::id::{AssetId, DomainId, PortfolioId};
use obolargus_core::domain::position::replay::replay_events;

fn make_event(
    event_type: EventType,
    effective: NaiveDate,
    qty: i64,
    portfolio: PortfolioId,
    asset: AssetId,
    to: Option<PortfolioId>,
) -> Event {
    Event {
        id: Uuid::new_v4(),
        event_type: event_type.to_string(),
        effective_date: effective,
        quantity_delta: Decimal::from(qty),
        recorded_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        portfolio_id: portfolio.as_uuid(),
        asset_id: asset.as_uuid(),
        to_portfolio_id: to.map(|p| p.as_uuid()),
    }
}

#[test]
fn replay_buy_sell_produces_correct_quantity() {
    let p = PortfolioId::from_uuid(Uuid::new_v4());
    let a = AssetId::from_uuid(Uuid::new_v4());
    let today = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
    let events = vec![
        make_event(EventType::Buy, today, 100, p, a, None),
        make_event(EventType::Buy, today, 50, p, a, None),
        make_event(EventType::Sell, today, -30, p, a, None),
    ];
    let summary = replay_events(p, a, &events).expect("replay");
    assert_eq!(summary.position.quantity, Decimal::from(120));
    assert_eq!(summary.events_applied, 3);
}

#[test]
fn replay_split_doubles_quantity() {
    let p = PortfolioId::from_uuid(Uuid::new_v4());
    let a = AssetId::from_uuid(Uuid::new_v4());
    let today = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
    // Per the replay function, a split's quantity_delta carries the
    // additional units created by the split (a 2-for-1 split of 100 → +100).
    let events = vec![
        make_event(EventType::Buy, today, 100, p, a, None),
        make_event(EventType::Split, today, 100, p, a, None),
    ];
    let summary = replay_events(p, a, &events).expect("replay");
    assert_eq!(summary.position.quantity, Decimal::from(200));
}

#[test]
fn replay_transfer_skips_other_portfolios() {
    let src = PortfolioId::from_uuid(Uuid::new_v4());
    let dst = PortfolioId::from_uuid(Uuid::new_v4());
    let a = AssetId::from_uuid(Uuid::new_v4());
    let today = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
    // A transfer FROM src TO dst with quantity_delta = -30 (sign convention:
    // the source side of a transfer has a negative delta).
    let events = vec![make_event(EventType::Transfer, today, -30, src, a, Some(dst))];
    let src_summary = replay_events(src, a, &events).expect("replay src");
    assert_eq!(src_summary.position.quantity, Decimal::from(-30));
    assert_eq!(src_summary.events_applied, 1);

    let dst_summary = replay_events(dst, a, &events).expect("replay dst");
    // The transfer event has portfolio_id = src, so when replaying for dst
    // it is skipped (it does not affect the dst position in this iteration;
    // dst materialization is handled separately by the event::create path).
    assert_eq!(dst_summary.events_skipped, 1);
}

#[test]
fn replay_unsupported_event_type_returns_error() {
    let p = PortfolioId::from_uuid(Uuid::new_v4());
    let a = AssetId::from_uuid(Uuid::new_v4());
    let today = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
    let events = vec![make_event(EventType::Dividend, today, 1, p, a, None)];
    let err = replay_events(p, a, &events).expect_err("should reject dividend");
    match err {
        obolargus_core::domain::DomainError::UnsupportedEventType { event_type } => {
            assert_eq!(event_type, "dividend");
        }
        other => panic!("expected UnsupportedEventType, got {:?}", other),
    }
}

#[test]
fn replay_is_deterministic_for_same_date_events() {
    let p = PortfolioId::from_uuid(Uuid::new_v4());
    let a = AssetId::from_uuid(Uuid::new_v4());
    let today = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
    let e1 = make_event(EventType::Buy, today, 10, p, a, None);
    let e2 = make_event(EventType::Buy, today, 20, p, a, None);
    // Build the two slices from references; no `.clone()` needed.
    let s1 = replay_events(p, a, &[&e1, &e2]).expect("s1");
    let reversed: [&Event; 2] = [&e2, &e1];
    let s2 = replay_events(p, a, reversed).expect("s2");
    assert_eq!(s1.position.quantity, s2.position.quantity);
    assert_eq!(s1.position.quantity, Decimal::from(30));
}
