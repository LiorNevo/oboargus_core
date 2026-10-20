//! Per-field cross-check: confirms that the per-field aggregate declaration
//! in `model_description()` matches the in-code expectations. Implements
//! FR-014.

use crate::domain::model_description::model_description;

#[test]
fn portfolio_has_six_fields() {
    let desc = model_description();
    let p = desc
        .entities
        .iter()
        .find(|e| e.name == "Portfolio")
        .expect("Portfolio");
    assert_eq!(p.fields.len(), 6);
}

#[test]
fn asset_has_seven_fields() {
    let desc = model_description();
    let a = desc
        .entities
        .iter()
        .find(|e| e.name == "Asset")
        .expect("Asset");
    assert_eq!(a.fields.len(), 7);
}

#[test]
fn event_has_eight_fields() {
    let desc = model_description();
    let e = desc
        .entities
        .iter()
        .find(|e| e.name == "Event")
        .expect("Event");
    assert_eq!(e.fields.len(), 8);
}

#[test]
fn position_has_four_fields() {
    let desc = model_description();
    let p = desc
        .entities
        .iter()
        .find(|e| e.name == "Position")
        .expect("Position");
    assert_eq!(p.fields.len(), 4);
}

#[test]
fn reference_data_entities_have_seven_fields_each() {
    let desc = model_description();
    for name in &[
        "AssetReferenceData",
        "IssuerReferenceData",
        "PortfolioReferenceData",
    ] {
        let e = desc
            .entities
            .iter()
            .find(|e| e.name == *name)
            .unwrap_or_else(|| panic!("{}", name));
        assert_eq!(e.fields.len(), 7, "{} should have 7 fields", name);
    }
}

#[test]
fn identifier_entities_have_five_fields_each() {
    let desc = model_description();
    for name in &["AssetIdentifier", "IssuerIdentifier"] {
        let e = desc
            .entities
            .iter()
            .find(|e| e.name == *name)
            .unwrap_or_else(|| panic!("{}", name));
        assert_eq!(e.fields.len(), 5, "{} should have 5 fields", name);
    }
}

#[test]
fn rule_entity_has_seven_fields() {
    let desc = model_description();
    let r = desc
        .entities
        .iter()
        .find(|e| e.name == "Rule")
        .expect("Rule");
    assert_eq!(r.fields.len(), 7);
}

#[test]
fn fifteen_relationships() {
    let desc = model_description();
    // 11 original + issuer parent_of + 2 identified_by + governed_by
    assert_eq!(desc.relationships.len(), 15);
}

#[test]
fn sixteen_invariants() {
    let desc = model_description();
    // 11 original + issuer DAG acyclicity + identifier uniqueness
    // + 3 rule invariants
    assert_eq!(desc.invariants.len(), 16);
}
