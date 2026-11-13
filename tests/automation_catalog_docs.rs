//! Coverage for the automation catalog, templates, and model helpers.
//!
//! These are pure functions with no database dependency, but they were not
//! exercised directly by the existing integration suites.

use obolargus_core::automation::docs;
use obolargus_core::automation::error::{
    FlowValidationFinding, NodeExecutionError, ValidationReport, ValidationSeverity,
};
use obolargus_core::automation::model::{
    FlowStatus, NodeRunStatus, RunStatus, TriggerKind, VersionStatus,
};
use obolargus_core::automation::templates;

#[test]
fn templates_are_discoverable_and_have_a_canonical_flow_document() {
    let all = templates::templates();
    assert_eq!(all.len(), 1);
    let template = templates::get_template("data-load-single-date").expect("known template");
    assert_eq!(template.id, "data-load-single-date");
    assert_eq!(template.version, 1);
    assert_eq!(template.category, "data");
    assert_eq!(template.node_count, 3);
    assert_eq!(
        template.flow_json["nodes"].as_array().expect("nodes").len(),
        3
    );
    assert_eq!(
        template.flow_json["wires"].as_array().expect("wires").len(),
        2
    );
    assert!(templates::get_template("missing").is_none());

    let direct = templates::data_load::template();
    assert_eq!(direct, template);
}

#[test]
fn model_enums_round_trip_their_database_text() {
    assert_eq!(FlowStatus::Active.as_str(), "active");
    assert_eq!(FlowStatus::Archived.as_str(), "archived");

    assert_eq!(VersionStatus::Draft.as_str(), "draft");
    assert_eq!(VersionStatus::Published.as_str(), "published");
    assert_eq!(VersionStatus::Archived.as_str(), "archived");

    assert_eq!(NodeRunStatus::Pending.as_str(), "pending");
    assert_eq!(NodeRunStatus::Ready.as_str(), "ready");
    assert_eq!(NodeRunStatus::Running.as_str(), "running");
    assert_eq!(NodeRunStatus::Succeeded.as_str(), "succeeded");
    assert_eq!(NodeRunStatus::Failed.as_str(), "failed");
    assert_eq!(NodeRunStatus::Skipped.as_str(), "skipped");
    assert_eq!(NodeRunStatus::Cancelled.as_str(), "cancelled");

    assert_eq!(TriggerKind::Manual.as_str(), "manual");
    assert_eq!(TriggerKind::Schedule.as_str(), "schedule");
    assert_eq!(TriggerKind::Api.as_str(), "api");

    for status in [
        RunStatus::Queued,
        RunStatus::Running,
        RunStatus::Succeeded,
        RunStatus::Partial,
        RunStatus::Failed,
        RunStatus::Cancelled,
        RunStatus::Interrupted,
    ] {
        assert_eq!(
            RunStatus::parse(status.as_str()),
            Some(status),
            "round trip {}",
            status.as_str()
        );
    }
    assert!(!RunStatus::Queued.is_terminal());
    assert!(!RunStatus::Running.is_terminal());
    for status in [
        RunStatus::Succeeded,
        RunStatus::Partial,
        RunStatus::Failed,
        RunStatus::Cancelled,
        RunStatus::Interrupted,
    ] {
        assert!(
            status.is_terminal(),
            "{} should be terminal",
            status.as_str()
        );
    }
    assert_eq!(RunStatus::parse("unknown"), None);
}

#[test]
fn validation_findings_and_reports_keep_their_metadata() {
    let error = FlowValidationFinding::error("flow.dangling_wire", "wire has no source")
        .with_node("n1")
        .with_wire("w1");
    assert_eq!(error.severity, ValidationSeverity::Error);
    assert_eq!(error.node_id.as_deref(), Some("n1"));
    assert_eq!(error.wire_id.as_deref(), Some("w1"));

    let warning = FlowValidationFinding::warning("flow.unused_node", "node is not wired");
    assert_eq!(warning.severity, ValidationSeverity::Warning);

    let mut report = ValidationReport::default();
    assert!(report.is_valid());
    report.push_warning(warning);
    assert!(report.is_valid());
    report.push_error(error);
    assert!(!report.is_valid());
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.warnings.len(), 1);

    let retryable = NodeExecutionError::retryable("node.timeout", "timed out");
    assert!(retryable.retryable);
    let fatal = NodeExecutionError::new("node.failed", "failed");
    assert!(!fatal.retryable);
    assert_eq!(fatal.to_string(), "node.failed: failed");
}

#[test]
fn node_type_catalog_is_generated_and_canonical() {
    let document = docs::node_types_document();
    assert_eq!(document["schema_version"], 1);
    assert_eq!(
        document["generated_by"],
        "obolargus_core::automation::docs::node_types_document"
    );
    let items = document["items"].as_array().expect("items");
    assert!(!items.is_empty());
    let ids: Vec<&str> = items
        .iter()
        .map(|item| item["id"].as_str().expect("node id"))
        .collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted, "catalog ids are sorted");

    let first = docs::node_types_json();
    let second = docs::node_types_json();
    assert_eq!(first, second, "serialization is deterministic");
    assert!(first.ends_with('\n'));
    assert!(first.starts_with("{\n"));
    let reparsed: serde_json::Value = serde_json::from_str(&first).expect("valid JSON");
    assert_eq!(reparsed, document);
}
