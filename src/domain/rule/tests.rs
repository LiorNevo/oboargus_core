//! Tests for US9 — portfolio rules: the shared rule catalog, the
//! many-to-many link to portfolios, effect categorization, uniqueness,
//! validation, open category vocabulary, non-inheritance, update, detach,
//! delete, and lifecycle.
//!
//! Implements FR-040 and the US9 acceptance scenarios. Gated by
//! `DATABASE_URL`.

use crate::common::*;
use crate::domain::error::DomainError;
use crate::domain::id::{DomainId, PortfolioId};
use crate::domain::portfolio::create_portfolio;
use crate::domain::portfolio::entity::NewPortfolio;
use crate::domain::rule::attach::{attach_rule_to_portfolio, detach_rule_from_portfolio};
use crate::domain::rule::entity::{NewRule, RuleCategory, RulePatch};
use crate::domain::rule::{
    create_rule, delete_rule, get_rule, list_rules, list_rules_by_category,
    list_rules_for_portfolio, update_rule,
};

/// A rule expression that filters the portfolio's positions down to the ones
/// whose weight exceeds 25%.
const CONCENTRATION_EXPRESSION: &str = r#"{"filter": {"data": {"get": {"selector": "positions"}}, "condition": {">": [{"get": {"selector": "weight_pct"}}, 25]}}}"#;

/// A rule expression that reads the portfolio's benchmark.
const BENCHMARK_EXPRESSION: &str = r#"{"get": {"selector": "benchmark"}}"#;

async fn new_portfolio(pool: &sqlx::PgPool, name: &str) -> PortfolioId {
    create_portfolio(
        pool,
        NewPortfolio {
            name: name.into(),
            purpose: "test".into(),
            created_by: "test".into(),
            parent_id: None,
        },
    )
    .await
    .expect("create portfolio")
    .id()
}

fn new_rule(name: &str, category: &str) -> NewRule {
    NewRule {
        name: name.into(),
        category: category.into(),
        expression: CONCENTRATION_EXPRESSION.into(),
        description: "a test rule".into(),
        created_by: "test".into(),
    }
}

#[tokio::test]
async fn create_rule_persists_in_the_catalog() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let created = create_rule(
        &pool,
        NewRule {
            name: "  single-asset concentration cap  ".into(),
            category: " ALL_ASSETS ".into(),
            expression: format!("  {CONCENTRATION_EXPRESSION}  "),
            ..new_rule("unused", "unused")
        },
    )
    .await
    .expect("create rule");

    // FR-032: the category is trimmed and lowercased; the name and the
    // expression are trimmed.
    assert_eq!(created.name, "single-asset concentration cap");
    assert_eq!(created.category, "all_assets");
    assert_eq!(created.expression, CONCENTRATION_EXPRESSION);

    let fetched = get_rule(&pool, created.id()).await.expect("read");
    assert_eq!(fetched, created);

    // The rule is written to the catalog; it is not attached to anything yet.
    let catalog = list_rules(&pool).await.expect("list catalog");
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog[0].id(), created.id());

    guard.cleanup().await;
}

#[tokio::test]
async fn attach_rule_to_portfolio_links_the_rule() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let portfolio_id = new_portfolio(&pool, "Attach Portfolio").await;
    let rule = create_rule(&pool, new_rule("a rule", RuleCategory::ALL_ASSETS))
        .await
        .expect("create rule");

    attach_rule_to_portfolio(&pool, portfolio_id, rule.id())
        .await
        .expect("attach rule");

    let listed = list_rules_for_portfolio(&pool, portfolio_id)
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id(), rule.id());

    guard.cleanup().await;
}

#[tokio::test]
async fn one_rule_governs_many_portfolios() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let first = new_portfolio(&pool, "First Portfolio").await;
    let second = new_portfolio(&pool, "Second Portfolio").await;
    let rule = create_rule(
        &pool,
        new_rule("single-asset concentration cap", RuleCategory::ALL_ASSETS),
    )
    .await
    .expect("create rule");

    attach_rule_to_portfolio(&pool, first, rule.id())
        .await
        .expect("attach to first");
    attach_rule_to_portfolio(&pool, second, rule.id())
        .await
        .expect("attach to second");

    // The same rule record appears in both portfolios' rule lists.
    let first_rules = list_rules_for_portfolio(&pool, first).await.expect("list");
    let second_rules = list_rules_for_portfolio(&pool, second).await.expect("list");
    assert_eq!(first_rules.len(), 1);
    assert_eq!(second_rules.len(), 1);
    assert_eq!(first_rules[0].id(), second_rules[0].id());
    assert_eq!(first_rules[0].id(), rule.id());

    // There is still exactly ONE rule — sharing did not fork it.
    let catalog = list_rules(&pool).await.expect("list catalog");
    assert_eq!(catalog.len(), 1);

    guard.cleanup().await;
}

#[tokio::test]
async fn list_rules_by_category_filters_by_effect() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let portfolio_id = new_portfolio(&pool, "Categorized Portfolio").await;
    let concentration = create_rule(
        &pool,
        new_rule("single-asset concentration cap", RuleCategory::ALL_ASSETS),
    )
    .await
    .expect("create rule");
    let benchmark = create_rule(
        &pool,
        NewRule {
            expression: BENCHMARK_EXPRESSION.into(),
            ..new_rule("benchmark check", RuleCategory::PORTFOLIO_DATA)
        },
    )
    .await
    .expect("create rule");
    attach_rule_to_portfolio(&pool, portfolio_id, concentration.id())
        .await
        .expect("attach");
    attach_rule_to_portfolio(&pool, portfolio_id, benchmark.id())
        .await
        .expect("attach");

    let all_assets = list_rules_by_category(&pool, portfolio_id, RuleCategory::ALL_ASSETS)
        .await
        .expect("list by category");
    assert_eq!(all_assets.len(), 1);
    assert_eq!(all_assets[0].id(), concentration.id());

    let portfolio_data = list_rules_by_category(&pool, portfolio_id, RuleCategory::PORTFOLIO_DATA)
        .await
        .expect("list by category");
    assert_eq!(portfolio_data.len(), 1);
    assert_eq!(portfolio_data[0].id(), benchmark.id());

    // An unrecognized category matches nothing.
    let unknown = list_rules_by_category(&pool, portfolio_id, "no_such_category")
        .await
        .expect("list by category");
    assert!(unknown.is_empty());

    guard.cleanup().await;
}

#[tokio::test]
async fn duplicate_rule_name_in_the_catalog_is_rejected() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let first_portfolio = new_portfolio(&pool, "First Portfolio").await;
    let second_portfolio = new_portfolio(&pool, "Second Portfolio").await;
    let rule = create_rule(
        &pool,
        new_rule("single-asset concentration cap", RuleCategory::ALL_ASSETS),
    )
    .await
    .expect("create rule");
    attach_rule_to_portfolio(&pool, first_portfolio, rule.id())
        .await
        .expect("attach");

    // Uniqueness is global: it does not matter which portfolios the two
    // rules are attached to.
    let err = create_rule(
        &pool,
        new_rule("single-asset concentration cap", RuleCategory::POSITIONS),
    )
    .await
    .expect_err("duplicate name must be rejected");
    assert!(
        matches!(
            &err,
            DomainError::InvariantViolation { invariant, .. } if *invariant == "rule_name_uniqueness"
        ),
        "expected rule_name_uniqueness, got {err:?}"
    );

    // The second rule was never created, so its portfolio carries nothing.
    let listed = list_rules_for_portfolio(&pool, second_portfolio)
        .await
        .expect("list");
    assert!(listed.is_empty());

    guard.cleanup().await;
}

#[tokio::test]
async fn attaching_the_same_rule_twice_is_rejected() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let portfolio_id = new_portfolio(&pool, "Duplicate Attach Portfolio").await;
    let rule = create_rule(&pool, new_rule("a rule", RuleCategory::ALL_ASSETS))
        .await
        .expect("create rule");

    attach_rule_to_portfolio(&pool, portfolio_id, rule.id())
        .await
        .expect("attach");

    let err = attach_rule_to_portfolio(&pool, portfolio_id, rule.id())
        .await
        .expect_err("attaching the same rule twice must be rejected");
    assert!(
        matches!(
            &err,
            DomainError::InvariantViolation { invariant, .. } if *invariant == "rule_attachment_uniqueness"
        ),
        "expected rule_attachment_uniqueness, got {err:?}"
    );

    // The portfolio still carries the rule exactly once.
    let listed = list_rules_for_portfolio(&pool, portfolio_id)
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);

    guard.cleanup().await;
}

#[tokio::test]
async fn empty_name_category_or_expression_is_rejected() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };

    for (label, name, category, expression) in [
        (
            "empty name",
            "  ",
            RuleCategory::ALL_ASSETS,
            CONCENTRATION_EXPRESSION,
        ),
        ("empty category", "a rule", "   ", CONCENTRATION_EXPRESSION),
        (
            "empty expression",
            "a rule",
            RuleCategory::ALL_ASSETS,
            "   ",
        ),
    ] {
        let err = create_rule(
            &pool,
            NewRule {
                name: name.into(),
                category: category.into(),
                expression: expression.into(),
                description: "d".into(),
                created_by: "test".into(),
            },
        )
        .await
        .expect_err(label);
        assert!(
            matches!(&err, DomainError::Validation(_)),
            "expected Validation for {label}, got {err:?}"
        );
    }

    guard.cleanup().await;
}

#[tokio::test]
async fn unparseable_expression_is_rejected() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let err = create_rule(
        &pool,
        NewRule {
            expression: "this is not a rule".into(),
            ..new_rule("not a rule", RuleCategory::ALL_ASSETS)
        },
    )
    .await
    .expect_err("an unparseable expression must be rejected");
    assert!(
        matches!(&err, DomainError::Validation(_)),
        "expected Validation, got {err:?}"
    );

    // Nothing was persisted.
    let catalog = list_rules(&pool).await.expect("list catalog");
    assert!(catalog.is_empty());

    guard.cleanup().await;
}

#[tokio::test]
async fn unknown_category_is_accepted_and_normalized() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let portfolio_id = new_portfolio(&pool, "Open Vocabulary Portfolio").await;
    let created = create_rule(&pool, new_rule("liquidity tier rule", "  Liquidity_Tier  "))
        .await
        .expect("an unrecognized category must be accepted");
    assert_eq!(created.category, "liquidity_tier");
    attach_rule_to_portfolio(&pool, portfolio_id, created.id())
        .await
        .expect("attach");

    let listed = list_rules_by_category(&pool, portfolio_id, "LIQUIDITY_TIER")
        .await
        .expect("list by category");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id(), created.id());

    guard.cleanup().await;
}

#[tokio::test]
async fn rules_are_not_inherited_from_ancestor_portfolios() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let parent_id = new_portfolio(&pool, "Parent Portfolio").await;
    let child_id = create_portfolio(
        &pool,
        NewPortfolio {
            name: "Child Portfolio".into(),
            purpose: "test".into(),
            created_by: "test".into(),
            parent_id: Some(parent_id),
        },
    )
    .await
    .expect("create child portfolio")
    .id();

    let rule = create_rule(&pool, new_rule("a rule", RuleCategory::ALL_ASSETS))
        .await
        .expect("create rule");
    attach_rule_to_portfolio(&pool, parent_id, rule.id())
        .await
        .expect("attach");

    let child_rules = list_rules_for_portfolio(&pool, child_id)
        .await
        .expect("list");
    assert!(
        child_rules.is_empty(),
        "a child portfolio must not inherit its ancestors' rules"
    );

    guard.cleanup().await;
}

#[tokio::test]
async fn update_rule_changes_only_updatable_fields() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let created = create_rule(
        &pool,
        new_rule("single-asset concentration cap", RuleCategory::ALL_ASSETS),
    )
    .await
    .expect("create rule");

    let updated = update_rule(
        &pool,
        created.id(),
        RulePatch {
            name: Some("renamed rule".into()),
            category: Some(" Positions ".into()),
            expression: Some(BENCHMARK_EXPRESSION.into()),
            description: Some("a new description".into()),
        },
    )
    .await
    .expect("update rule");

    assert_eq!(updated.name, "renamed rule");
    assert_eq!(updated.category, "positions");
    assert_eq!(updated.expression, BENCHMARK_EXPRESSION);
    assert_eq!(updated.description, "a new description");
    // The audit fields are unchanged.
    assert_eq!(updated.created_at, created.created_at);
    assert_eq!(updated.created_by, created.created_by);

    guard.cleanup().await;
}

#[tokio::test]
async fn update_to_a_shared_rule_is_visible_from_every_portfolio() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let first = new_portfolio(&pool, "First Portfolio").await;
    let second = new_portfolio(&pool, "Second Portfolio").await;
    let rule = create_rule(&pool, new_rule("a rule", RuleCategory::ALL_ASSETS))
        .await
        .expect("create rule");
    attach_rule_to_portfolio(&pool, first, rule.id())
        .await
        .expect("attach");
    attach_rule_to_portfolio(&pool, second, rule.id())
        .await
        .expect("attach");

    update_rule(
        &pool,
        rule.id(),
        RulePatch {
            name: Some("a renamed rule".into()),
            ..RulePatch::default()
        },
    )
    .await
    .expect("update rule");

    // The edit is visible from every portfolio the rule is attached to —
    // there is one rule, not two copies.
    for portfolio_id in [first, second] {
        let listed = list_rules_for_portfolio(&pool, portfolio_id)
            .await
            .expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "a renamed rule");
    }

    guard.cleanup().await;
}

#[tokio::test]
async fn update_rule_rejects_a_duplicate_name() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    create_rule(&pool, new_rule("first rule", RuleCategory::ALL_ASSETS))
        .await
        .expect("create rule");
    let second = create_rule(&pool, new_rule("second rule", RuleCategory::POSITIONS))
        .await
        .expect("create rule");

    let err = update_rule(
        &pool,
        second.id(),
        RulePatch {
            name: Some("first rule".into()),
            ..RulePatch::default()
        },
    )
    .await
    .expect_err("renaming onto a taken name must be rejected");
    assert!(
        matches!(
            &err,
            DomainError::InvariantViolation { invariant, .. } if *invariant == "rule_name_uniqueness"
        ),
        "expected rule_name_uniqueness, got {err:?}"
    );

    guard.cleanup().await;
}

#[tokio::test]
async fn detach_rule_removes_only_the_link() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let first = new_portfolio(&pool, "First Portfolio").await;
    let second = new_portfolio(&pool, "Second Portfolio").await;
    let rule = create_rule(&pool, new_rule("a rule", RuleCategory::ALL_ASSETS))
        .await
        .expect("create rule");
    attach_rule_to_portfolio(&pool, first, rule.id())
        .await
        .expect("attach");
    attach_rule_to_portfolio(&pool, second, rule.id())
        .await
        .expect("attach");

    detach_rule_from_portfolio(&pool, first, rule.id())
        .await
        .expect("detach");

    let first_rules = list_rules_for_portfolio(&pool, first).await.expect("list");
    assert!(first_rules.is_empty());

    // The rule is still attached to the other portfolio and still in the
    // catalog.
    let second_rules = list_rules_for_portfolio(&pool, second).await.expect("list");
    assert_eq!(second_rules.len(), 1);
    let catalog = list_rules(&pool).await.expect("list catalog");
    assert_eq!(catalog.len(), 1);

    // Detaching a link that does not exist is a NotFound.
    let err = detach_rule_from_portfolio(&pool, first, rule.id())
        .await
        .expect_err("detaching a missing link must be rejected");
    assert!(
        matches!(&err, DomainError::NotFound { entity, .. } if *entity == "PortfolioRule"),
        "expected NotFound, got {err:?}"
    );

    guard.cleanup().await;
}

#[tokio::test]
async fn delete_rule_cascades_to_every_portfolio() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let first = new_portfolio(&pool, "First Portfolio").await;
    let second = new_portfolio(&pool, "Second Portfolio").await;
    let rule = create_rule(&pool, new_rule("a rule", RuleCategory::ALL_ASSETS))
        .await
        .expect("create rule");
    attach_rule_to_portfolio(&pool, first, rule.id())
        .await
        .expect("attach");
    attach_rule_to_portfolio(&pool, second, rule.id())
        .await
        .expect("attach");

    delete_rule(&pool, rule.id()).await.expect("delete rule");

    for portfolio_id in [first, second] {
        let listed = list_rules_for_portfolio(&pool, portfolio_id)
            .await
            .expect("list");
        assert!(
            listed.is_empty(),
            "the rule must be removed from every portfolio that used it"
        );
    }
    let catalog = list_rules(&pool).await.expect("list catalog");
    assert!(catalog.is_empty());

    let err = get_rule(&pool, rule.id())
        .await
        .expect_err("a deleted rule must be gone");
    assert!(
        matches!(&err, DomainError::NotFound { entity, .. } if *entity == "Rule"),
        "expected NotFound, got {err:?}"
    );

    let err = delete_rule(&pool, rule.id())
        .await
        .expect_err("deleting an unknown rule must be rejected");
    assert!(
        matches!(&err, DomainError::NotFound { entity, .. } if *entity == "Rule"),
        "expected NotFound, got {err:?}"
    );

    guard.cleanup().await;
}

#[tokio::test]
async fn deleting_portfolio_keeps_the_rules_in_the_catalog() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let portfolio_id = new_portfolio(&pool, "Cascade Portfolio").await;
    let rule = create_rule(&pool, new_rule("a rule", RuleCategory::ALL_ASSETS))
        .await
        .expect("create rule");
    attach_rule_to_portfolio(&pool, portfolio_id, rule.id())
        .await
        .expect("attach");

    // Rules are current-state configuration: they do NOT block the
    // portfolio's deletion, the way reference data does.
    crate::domain::portfolio::delete_portfolio(&pool, portfolio_id)
        .await
        .expect("delete portfolio");

    // The attachment is gone, but the rule stays in the shared catalog for
    // other portfolios to use.
    let listed = list_rules_for_portfolio(&pool, portfolio_id)
        .await
        .expect("list");
    assert!(listed.is_empty());
    let catalog = list_rules(&pool).await.expect("list catalog");
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog[0].id(), rule.id());
    assert!(get_rule(&pool, rule.id()).await.is_ok());

    guard.cleanup().await;
}

#[tokio::test]
async fn attach_to_unknown_portfolio_or_rule_is_rejected() {
    let Some((pool, guard)) = tx_pool().await else {
        return;
    };
    let portfolio_id = new_portfolio(&pool, "Known Portfolio").await;
    let unknown_portfolio = PortfolioId::from_uuid(uuid::Uuid::new_v4());
    let unknown_rule = crate::domain::id::RuleId::from_uuid(uuid::Uuid::new_v4());

    let err = attach_rule_to_portfolio(&pool, unknown_portfolio, unknown_rule)
        .await
        .expect_err("attaching to an unknown portfolio must be rejected");
    assert!(
        matches!(&err, DomainError::NotFound { entity, .. } if *entity == "Portfolio"),
        "expected NotFound, got {err:?}"
    );

    let err = attach_rule_to_portfolio(&pool, portfolio_id, unknown_rule)
        .await
        .expect_err("attaching an unknown rule must be rejected");
    assert!(
        matches!(&err, DomainError::NotFound { entity, .. } if *entity == "Rule"),
        "expected NotFound, got {err:?}"
    );

    guard.cleanup().await;
}

#[tokio::test]
async fn model_description_includes_rule_entity() {
    use crate::domain::model_description::model_description;
    let desc = model_description();

    let rule = desc
        .entities
        .iter()
        .find(|e| e.name == "Rule")
        .expect("the model description must contain the Rule entity");
    assert_eq!(rule.fields.len(), 7);

    let relationship = desc
        .relationships
        .iter()
        .find(|r| r.from == "Portfolio" && r.to == "Rule")
        .expect("the model description must contain the governed_by relationship");
    assert_eq!(relationship.label, "governed_by");
    assert_eq!(relationship.cardinality_from, "0..*");
    assert_eq!(relationship.cardinality_to, "0..*");

    assert!(
        desc.invariants
            .iter()
            .any(|i| i.contains("Rule names are unique across the rule catalog")),
        "the rule-name uniqueness invariant must be present"
    );
    assert!(
        desc.invariants
            .iter()
            .any(|i| i.contains("Rules are not inherited from ancestor portfolios")),
        "the non-inheritance invariant must be present"
    );
    assert!(
        desc.invariants
            .iter()
            .any(|i| i.contains("A portfolio carries a given rule at most once")),
        "the attachment uniqueness invariant must be present"
    );
}

#[cfg(test)]
mod unit {
    //! Pure-function tests that do not need a database.
    use super::*;
    use crate::domain::rule::entity::{normalize_category, normalize_category_for_lookup};

    #[test]
    fn category_normalization_trims_and_lowercases() {
        assert_eq!(RuleCategory::normalize(" ALL_ASSETS "), "all_assets");
        assert_eq!(
            RuleCategory::normalize(RuleCategory::PORTFOLIO_DATA),
            "portfolio_data"
        );
        assert_eq!(normalize_category_for_lookup("  Positions  "), "positions");
    }

    #[test]
    fn empty_category_is_rejected() {
        let err = normalize_category("   ").expect_err("an empty category is invalid");
        assert!(matches!(&err, DomainError::Validation(_)));
    }

    #[test]
    fn unknown_category_is_accepted() {
        let normalized =
            normalize_category("liquidity_tier").expect("an unrecognized category is valid");
        assert_eq!(normalized, "liquidity_tier");
    }

    #[test]
    fn seed_expressions_parse_as_rules() {
        // The canonical seed's rule bodies must be accepted by the rule
        // engine's deserializer — `create_rule` rejects anything else.
        use crate::domain::rule::create::validate_expression;
        for expression in [
            r#"{"get": {"selector": "benchmark"}}"#,
            r#"{"filter": {"data": {"get": {"selector": "positions"}}, "condition": {">": [{"get": {"selector": "weight_pct"}}, 25]}}}"#,
            r#"{"filter": {"data": {"get": {"selector": "events"}}, "condition": {">": [{"get": {"selector": "quantity_delta"}}, 1000]}}}"#,
        ] {
            validate_expression(expression)
                .unwrap_or_else(|e| panic!("`{expression}` should parse: {e}"));
        }
    }
}
