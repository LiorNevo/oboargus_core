use super::*;
use crate::domain::id::new_uuid;

#[test]
fn subject_round_trips_through_parts() {
    for (kind, subject) in [
        (
            "asset",
            FactSubject::Asset(crate::domain::id::AssetId::from_uuid(new_uuid())),
        ),
        (
            "issuer",
            FactSubject::Issuer(crate::domain::id::IssuerId::from_uuid(new_uuid())),
        ),
        (
            "portfolio",
            FactSubject::Portfolio(crate::domain::id::PortfolioId::from_uuid(new_uuid())),
        ),
    ] {
        let rebuilt = FactSubject::from_parts(kind, subject.id()).expect("subject");
        assert_eq!(rebuilt, subject);
        assert_eq!(subject.subject_name(), kind);
    }
}

#[test]
fn unknown_subject_kind_is_rejected() {
    assert!(FactSubject::from_parts("currency", new_uuid()).is_err());
}

#[test]
fn fact_point_hides_a_retracted_value() {
    let point = FactPoint {
        id: new_uuid(),
        subject: FactSubject::Asset(crate::domain::id::AssetId::from_uuid(new_uuid())),
        fact_type: "name".to_string(),
        schema_version: 1,
        valid_at: ValidAt::midnight_utc(chrono::NaiveDate::from_ymd_opt(2026, 1, 2).expect("date")),
        known_at: KnownAt::now(),
        value: Some(FactValue::Text { value: "x".into() }),
        source: "test".into(),
        retracted: true,
        retraction_reason: Some("gone".into()),
        content_hash: "hash".into(),
    };
    assert!(point.resolved_value().is_none());
}
