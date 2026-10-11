//! Unit tests for the entity accessors. The `entity.rs` modules are pure
//! data structs with no DB access; coverage here is cheap to add and
//! raises the overall project coverage without needing a database.
//!
//! This file is `#[cfg(test)]` so the imports and helper code are only
//! compiled when running `cargo test`, and `cargo clippy` on the lib
//! does not see the test-only code.

#[cfg(test)]
mod tests {
    use crate::domain::id::{AssetId, DomainId, IssuerId, PortfolioId};
    use chrono::Utc;

    #[test]
    fn asset_accessors() {
        let asset_id = AssetId::from_uuid(uuid::Uuid::new_v4());
        let portfolio_id = PortfolioId::from_uuid(uuid::Uuid::new_v4());
        let issuer_id = IssuerId::from_uuid(uuid::Uuid::new_v4());
        let asset = crate::domain::asset::entity::Asset {
            id: asset_id.as_uuid(),
            asset_type: "stock".into(),
            display_name: "X".into(),
            currency: "USD".into(),
            created_at: Utc::now(),
            backed_by_portfolio_id: Some(portfolio_id.as_uuid()),
            issuer_id: Some(issuer_id.as_uuid()),
        };
        assert_eq!(asset.id(), asset_id);
        assert_eq!(asset.backed_by(), Some(portfolio_id));
        assert_eq!(asset.issuer(), Some(issuer_id));

        let no_back = crate::domain::asset::entity::Asset {
            backed_by_portfolio_id: None,
            issuer_id: None,
            ..asset
        };
        assert!(no_back.backed_by().is_none());
        assert!(no_back.issuer().is_none());
    }

    #[test]
    fn issuer_accessor() {
        let issuer_id = IssuerId::from_uuid(uuid::Uuid::new_v4());
        let issuer = crate::domain::issuer::entity::Issuer {
            id: issuer_id.as_uuid(),
            name: "X".into(),
            issuer_type: "t".into(),
            legal_jurisdiction: "US".into(),
            created_at: Utc::now(),
        };
        assert_eq!(issuer.id(), issuer_id);
    }

    #[test]
    fn event_accessors_and_parsing() {
        use crate::domain::event::entity::{Event, EventType};
        use std::str::FromStr;
        let portfolio_id = PortfolioId::from_uuid(uuid::Uuid::new_v4());
        let asset_id = AssetId::from_uuid(uuid::Uuid::new_v4());
        let event_id = uuid::Uuid::new_v4();
        let event = Event {
            id: event_id,
            event_type: "buy".to_string(),
            effective_date: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            quantity_delta: rust_decimal::Decimal::from(100),
            recorded_at: Utc::now(),
            portfolio_id: portfolio_id.as_uuid(),
            asset_id: asset_id.as_uuid(),
            to_portfolio_id: None,
        };
        assert_eq!(event.id(), crate::domain::id::EventId::from_uuid(event_id));
        assert_eq!(event.portfolio_id(), portfolio_id);
        assert_eq!(event.asset_id(), asset_id);
        assert!(event.to_portfolio_id_opt().is_none());
        assert_eq!(event.event_type().unwrap(), EventType::Buy);
        assert_eq!(EventType::from_str("transfer").unwrap(), EventType::Transfer);
        assert!(EventType::from_str("nope").is_err());
        assert_eq!(EventType::Transfer.as_str(), "transfer");
        assert_eq!(EventType::Buy.to_string(), "buy");
    }

    #[test]
    fn position_accessors() {
        use crate::domain::position::entity::Position;
        let p = PortfolioId::from_uuid(uuid::Uuid::new_v4());
        let a = AssetId::from_uuid(uuid::Uuid::new_v4());
        let pos = Position {
            portfolio_id: p.as_uuid(),
            asset_id: a.as_uuid(),
            quantity: rust_decimal::Decimal::from(100),
            as_of: Utc::now(),
        };
        assert_eq!(pos.portfolio_id(), p);
        assert_eq!(pos.asset_id(), a);
    }

    #[test]
    fn portfolio_accessor() {
        let p = PortfolioId::from_uuid(uuid::Uuid::new_v4());
        let portfolio = crate::domain::portfolio::entity::Portfolio {
            id: p.as_uuid(),
            name: "X".into(),
            purpose: "p".into(),
            created_at: Utc::now(),
            created_by: "t".into(),
            parent_id: None,
        };
        assert_eq!(portfolio.id(), p);
        assert!(portfolio.parent_id_opt().is_none());
        let parent = PortfolioId::from_uuid(uuid::Uuid::new_v4());
        let with_parent = crate::domain::portfolio::entity::Portfolio {
            parent_id: Some(parent.as_uuid()),
            ..portfolio
        };
        assert_eq!(with_parent.parent_id_opt(), Some(parent));
    }

    #[test]
    fn reference_data_id_is_distinct() {
        use crate::domain::id::ReferenceDataId;
        let id = ReferenceDataId::from_uuid(uuid::Uuid::new_v4());
        assert_eq!(id.as_uuid(), id.as_uuid());
    }
}
