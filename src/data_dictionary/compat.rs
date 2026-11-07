//! Compatibility classification against a committed schema baseline.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

/// Whether a schema change is breaking for consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// Existing consumers remain valid.
    NonBreaking,
    /// Existing consumers may break.
    Breaking,
}

/// One classified change between two schema documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// JSON Pointer-ish path to the changed item.
    pub path: String,
    /// The classification.
    pub kind: ChangeKind,
    /// A human-readable description.
    pub description: String,
}

impl Change {
    fn new(path: impl Into<String>, kind: ChangeKind, description: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            kind,
            description: description.into(),
        }
    }
}

/// Classifies all changes between a baseline and a current document.
pub fn classify_changes(baseline: &Value, current: &Value) -> Vec<Change> {
    let mut changes = Vec::new();
    compare_definitions(baseline, current, &mut changes);
    compare_array_member(
        baseline,
        current,
        "x-obolargus-relationships",
        "from",
        &mut changes,
    );
    compare_array_member(
        baseline,
        current,
        "x-obolargus-invariants",
        "",
        &mut changes,
    );
    changes
}

/// Fails when a breaking change is present without a schema-version change.
pub fn check_compatibility(baseline: &Value, current: &Value) -> Result<(), Vec<Change>> {
    let changes = classify_changes(baseline, current);
    let breaking = changes
        .iter()
        .filter(|change| change.kind == ChangeKind::Breaking)
        .cloned()
        .collect::<Vec<_>>();
    if breaking.is_empty() {
        return Ok(());
    }
    let baseline_version = baseline
        .get("x-obolargus-schema-version")
        .and_then(Value::as_str)
        .unwrap_or("");
    let current_version = current
        .get("x-obolargus-schema-version")
        .and_then(Value::as_str)
        .unwrap_or("");
    if baseline_version == current_version {
        Err(breaking)
    } else {
        Ok(())
    }
}

fn compare_definitions(baseline: &Value, current: &Value, changes: &mut Vec<Change>) {
    let Some(baseline_defs) = baseline.get("$defs").and_then(Value::as_object) else {
        return;
    };
    let current_defs = current
        .get("$defs")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (name, baseline_definition) in baseline_defs {
        let path = format!("/$defs/{name}");
        let Some(current_definition) = current_defs.get(name) else {
            changes.push(Change::new(
                path,
                ChangeKind::Breaking,
                "definition removed",
            ));
            continue;
        };
        compare_definition(&path, baseline_definition, current_definition, changes);
    }
    for name in current_defs.keys() {
        if !baseline_defs.contains_key(name) {
            changes.push(Change::new(
                format!("/$defs/{name}"),
                ChangeKind::NonBreaking,
                "definition added",
            ));
        }
    }
}

fn compare_definition(path: &str, baseline: &Value, current: &Value, changes: &mut Vec<Change>) {
    compare_scalar_keyword(path, baseline, current, "type", changes);
    compare_scalar_keyword(path, baseline, current, "format", changes);
    compare_scalar_keyword(path, baseline, current, "pattern", changes);
    compare_bound(path, baseline, current, "minLength", changes);
    compare_bound(path, baseline, current, "maxLength", changes);
    compare_bound(path, baseline, current, "minimum", changes);
    compare_bound(path, baseline, current, "maximum", changes);
    compare_enum(path, baseline, current, changes);
    compare_required(path, baseline, current, changes);
    compare_properties(path, baseline, current, changes);
}

fn compare_scalar_keyword(
    path: &str,
    baseline: &Value,
    current: &Value,
    keyword: &str,
    changes: &mut Vec<Change>,
) {
    let before = baseline.get(keyword);
    let after = current.get(keyword);
    if before != after {
        changes.push(Change::new(
            format!("{path}/{keyword}"),
            ChangeKind::Breaking,
            format!("{keyword} changed"),
        ));
    }
}

fn compare_bound(
    path: &str,
    baseline: &Value,
    current: &Value,
    keyword: &str,
    changes: &mut Vec<Change>,
) {
    let before = baseline.get(keyword).and_then(Value::as_f64);
    let after = current.get(keyword).and_then(Value::as_f64);
    match (before, after) {
        (Some(before), Some(after)) if before == after => {}
        (None, None) => {}
        (Some(before), Some(after)) => {
            let tightened = if keyword.contains("min") {
                after > before
            } else {
                after < before
            };
            changes.push(Change::new(
                format!("{path}/{keyword}"),
                if tightened {
                    ChangeKind::Breaking
                } else {
                    ChangeKind::NonBreaking
                },
                format!("{keyword} changed from {before} to {after}"),
            ));
        }
        _ => changes.push(Change::new(
            format!("{path}/{keyword}"),
            ChangeKind::Breaking,
            format!("{keyword} added or removed"),
        )),
    }
}

fn compare_enum(path: &str, baseline: &Value, current: &Value, changes: &mut Vec<Change>) {
    let before = string_set(baseline.get("enum"));
    let after = string_set(current.get("enum"));
    if before.is_empty() && after.is_empty() {
        return;
    }
    for removed in before.difference(&after) {
        changes.push(Change::new(
            format!("{path}/enum"),
            ChangeKind::Breaking,
            format!("enum value removed: {removed}"),
        ));
    }
    for added in after.difference(&before) {
        changes.push(Change::new(
            format!("{path}/enum"),
            ChangeKind::NonBreaking,
            format!("enum value added: {added}"),
        ));
    }
}

fn compare_required(path: &str, baseline: &Value, current: &Value, changes: &mut Vec<Change>) {
    let before = string_set(baseline.get("required"));
    let after = string_set(current.get("required"));
    for added in after.difference(&before) {
        changes.push(Change::new(
            format!("{path}/required"),
            ChangeKind::Breaking,
            format!("required property added: {added}"),
        ));
    }
    for removed in before.difference(&after) {
        changes.push(Change::new(
            format!("{path}/required"),
            ChangeKind::NonBreaking,
            format!("required property removed: {removed}"),
        ));
    }
}

fn compare_properties(path: &str, baseline: &Value, current: &Value, changes: &mut Vec<Change>) {
    let Some(before) = baseline.get("properties").and_then(Value::as_object) else {
        return;
    };
    let after = current
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (name, before_schema) in before {
        let child = format!("{path}/properties/{name}");
        let Some(after_schema) = after.get(name) else {
            changes.push(Change::new(child, ChangeKind::Breaking, "property removed"));
            continue;
        };
        compare_definition(&child, before_schema, after_schema, changes);
    }
    for name in after.keys() {
        if !before.contains_key(name) {
            changes.push(Change::new(
                format!("{path}/properties/{name}"),
                ChangeKind::NonBreaking,
                "property added",
            ));
        }
    }
}

fn compare_array_member(
    baseline: &Value,
    current: &Value,
    key: &str,
    identity: &str,
    changes: &mut Vec<Change>,
) {
    let before = baseline.get(key).and_then(Value::as_array);
    let after = current.get(key).and_then(Value::as_array);
    let (Some(before), Some(after)) = (before, after) else {
        return;
    };
    let before_set = array_members(before, identity);
    let after_set = array_members(after, identity);
    for removed in before_set.difference(&after_set) {
        changes.push(Change::new(
            format!("/{key}"),
            ChangeKind::Breaking,
            format!("member removed: {removed}"),
        ));
    }
    for added in after_set.difference(&before_set) {
        changes.push(Change::new(
            format!("/{key}"),
            ChangeKind::NonBreaking,
            format!("member added: {added}"),
        ));
    }
}

fn array_members(values: &[Value], identity: &str) -> BTreeSet<String> {
    values
        .iter()
        .map(|value| {
            if identity.is_empty() {
                value.as_str().unwrap_or_default().to_string()
            } else {
                value
                    .get(identity)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            }
        })
        .collect()
}

fn string_set(value: Option<&Value>) -> BTreeSet<String> {
    value
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Loads a baseline document from a JSON string.
pub fn parse_document(raw: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(raw)
}

/// Returns the definitions map for inspection.
pub fn definitions(document: &Value) -> Option<&Map<String, Value>> {
    document.get("$defs").and_then(Value::as_object)
}
