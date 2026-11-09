//! Flow document validation.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::automation::error::{FlowValidationFinding, ValidationReport};
use crate::automation::flow::document::FlowDocument;
use crate::automation::nodes::NodeRegistry;

/// Validates a flow document against the node registry.
pub fn validate_flow(document: &FlowDocument, registry: &NodeRegistry) -> ValidationReport {
    let mut report = ValidationReport::default();

    if document.schema_version != crate::automation::flow::document::FLOW_SCHEMA_VERSION {
        report.push_error(FlowValidationFinding::error(
            "flow.schema_version",
            format!(
                "unsupported schema_version {}; expected {}",
                document.schema_version,
                crate::automation::flow::document::FLOW_SCHEMA_VERSION
            ),
        ));
    }
    if document.name.trim().is_empty() || document.name.chars().count() > 200 {
        report.push_error(FlowValidationFinding::error(
            "flow.name_empty",
            "flow name must be non-empty and at most 200 characters",
        ));
    }
    if document.description.chars().count() > 4_000 {
        report.push_error(FlowValidationFinding::error(
            "flow.name_empty",
            "flow description must be at most 4,000 characters",
        ));
    }
    if document.nodes.is_empty() {
        report.push_error(FlowValidationFinding::error(
            "flow.empty",
            "a flow must contain at least one node",
        ));
    }
    if document.nodes.len() > 1_000 {
        report.push_error(FlowValidationFinding::error(
            "flow.definition_too_large",
            "a flow may contain at most 1,000 nodes",
        ));
    }
    if document.wires.len() > 5_000 {
        report.push_error(FlowValidationFinding::error(
            "flow.definition_too_large",
            "a flow may contain at most 5,000 wires",
        ));
    }

    validate_inputs(document, &mut report);
    validate_unique_ids(document, &mut report);
    validate_nodes(document, registry, &mut report);
    validate_wires(document, registry, &mut report);
    validate_subflows(document, registry, &mut report);

    report
}

fn validate_inputs(document: &FlowDocument, report: &mut ValidationReport) {
    let mut names = BTreeSet::new();
    for input in &document.inputs {
        if !is_identifier(&input.name) {
            report.push_error(
                FlowValidationFinding::error(
                    "flow.inputs_invalid",
                    format!("runtime input '{}' is not a valid identifier", input.name),
                )
                .with_node(&input.name),
            );
        }
        if !names.insert(input.name.clone()) {
            report.push_error(FlowValidationFinding::error(
                "flow.inputs_invalid",
                format!("duplicate runtime input '{}'", input.name),
            ));
        }
        if !matches!(
            input.input_type.as_str(),
            "string" | "number" | "integer" | "boolean" | "array" | "object"
        ) {
            report.push_error(FlowValidationFinding::error(
                "flow.inputs_invalid",
                format!(
                    "runtime input '{}' has unsupported type '{}'",
                    input.name, input.input_type
                ),
            ));
        }
        if input.name.len() > 100 {
            report.push_error(FlowValidationFinding::error(
                "flow.inputs_invalid",
                "runtime input names must be at most 100 characters",
            ));
        }
        if input
            .description
            .as_deref()
            .is_some_and(|value| value.chars().count() > 200)
        {
            report.push_error(FlowValidationFinding::error(
                "flow.inputs_invalid",
                "runtime input descriptions must be at most 200 characters",
            ));
        }
    }
}

fn validate_unique_ids(document: &FlowDocument, report: &mut ValidationReport) {
    let mut node_ids = BTreeSet::new();
    for node in &document.nodes {
        if !node_ids.insert(node.id.clone()) {
            report.push_error(
                FlowValidationFinding::error(
                    "flow.duplicate_id",
                    format!("duplicate node id '{}'", node.id),
                )
                .with_node(&node.id),
            );
        }
    }
    let mut wire_ids = BTreeSet::new();
    for wire in &document.wires {
        if !wire_ids.insert(wire.id.clone()) {
            report.push_error(
                FlowValidationFinding::error(
                    "flow.duplicate_id",
                    format!("duplicate wire id '{}'", wire.id),
                )
                .with_wire(&wire.id),
            );
        }
    }
    let mut subflow_ids = BTreeSet::new();
    for subflow in &document.subflows {
        if !subflow_ids.insert(subflow.id.clone()) {
            report.push_error(FlowValidationFinding::error(
                "flow.duplicate_id",
                format!("duplicate subflow id '{}'", subflow.id),
            ));
        }
    }
}

fn validate_nodes(document: &FlowDocument, registry: &NodeRegistry, report: &mut ValidationReport) {
    for node in &document.nodes {
        validate_node(node, registry, report);
    }
}

fn validate_node(
    node: &crate::automation::flow::document::FlowNode,
    registry: &NodeRegistry,
    report: &mut ValidationReport,
) {
    if crate::automation::runtime::subflow::is_structural(&node.node_type) {
        return;
    }
    let key = node.node_type_key();
    let Some(descriptor) = registry.get(&key) else {
        let known_id = registry
            .descriptors()
            .iter()
            .any(|descriptor| descriptor.id == node.node_type);
        report.push_error(
            FlowValidationFinding::error(
                if known_id {
                    "flow.unknown_version"
                } else {
                    "flow.unknown_type"
                },
                format!(
                    "node '{}' uses {} type '{}' version {}",
                    node.id,
                    if known_id { "an unknown" } else { "unknown" },
                    node.node_type,
                    node.type_version
                ),
            )
            .with_node(&node.id),
        );
        return;
    };
    let descriptor = descriptor.descriptor();
    if descriptor.deprecated {
        report.push_warning(
            FlowValidationFinding::warning(
                "node.deprecated",
                format!(
                    "node type '{}' version {} is deprecated",
                    descriptor.id, descriptor.version
                ),
            )
            .with_node(&node.id),
        );
    }
    let denied = registry.denied_capabilities(&descriptor);
    if !denied.is_empty() {
        report.push_error(
            FlowValidationFinding::error(
                "node.capability_missing",
                format!(
                    "node '{}' requires capabilities this deployment does not allow: {}",
                    node.id,
                    denied.join(", ")
                ),
            )
            .with_node(&node.id),
        );
    }
    if node.id.is_empty() || node.id.chars().count() > 100 {
        report.push_error(
            FlowValidationFinding::error(
                "flow.duplicate_id",
                format!("node id '{}' must be 1-100 characters", node.id),
            )
            .with_node(&node.id),
        );
    }
    if node.label.chars().count() > 200 {
        report.push_error(
            FlowValidationFinding::error(
                "node.config_invalid",
                format!("node '{}' label must be at most 200 characters", node.id),
            )
            .with_node(&node.id),
        );
    }
    if node.retry.max_attempts == 0 || node.retry.max_attempts > 10 {
        report.push_error(
            FlowValidationFinding::error(
                "node.policy_invalid",
                "retry.max_attempts must be between 1 and 10",
            )
            .with_node(&node.id),
        );
    }
    if node.retry.backoff_seconds > 3_600 {
        report.push_error(
            FlowValidationFinding::error(
                "node.policy_invalid",
                "retry.backoff_seconds must be at most 3,600",
            )
            .with_node(&node.id),
        );
    }
    if node.timeout_seconds == 0 || node.timeout_seconds > 86_400 {
        report.push_error(
            FlowValidationFinding::error(
                "node.policy_invalid",
                "timeout_seconds must be between 1 and 86,400",
            )
            .with_node(&node.id),
        );
    }
    if !matches!(node.error_policy.as_str(), "fail" | "continue" | "catch") {
        report.push_error(
            FlowValidationFinding::error(
                "node.policy_invalid",
                "error_policy must be fail, continue, or catch",
            )
            .with_node(&node.id),
        );
    }
    validate_trigger(node, report);
    validate_config(node, &descriptor.config_schema, report);
    validate_data_load_config(node, report);
}

fn has_identifiers(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(map)) => !map.is_empty(),
        _ => false,
    }
}

fn has_date_expression(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(Value::Object(map)) => !map.is_empty(),
        _ => false,
    }
}

fn validate_data_load_config(
    node: &crate::automation::flow::document::FlowNode,
    report: &mut ValidationReport,
) {
    let date_fields: &[&str] = match node.node_type.as_str() {
        "data_load" => &["as_of_date"],
        "data_load_range" => &["from", "to"],
        _ => return,
    };
    if !has_identifiers(node.config.get("identifiers")) {
        report.push_error(
            FlowValidationFinding::error(
                "node.config_invalid",
                format!(
                    "data-load node '{}' needs identifiers (a non-empty array or an expression object)",
                    node.id
                ),
            )
            .with_node(&node.id),
        );
    }
    for field in date_fields {
        if !has_date_expression(node.config.get(*field)) {
            report.push_error(
                FlowValidationFinding::error(
                    "node.config_invalid",
                    format!(
                        "data-load node '{}' needs '{}' (an ISO date or an expression object)",
                        node.id, field
                    ),
                )
                .with_node(&node.id),
            );
        }
    }
    let creates_assets = node
        .config
        .get("create_missing_assets")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if creates_assets
        && !node
            .config
            .get("default_currency")
            .and_then(Value::as_str)
            .is_some_and(|currency| !currency.trim().is_empty())
    {
        report.push_error(
            FlowValidationFinding::error(
                "node.config_invalid",
                format!(
                    "data-load node '{}' needs a default_currency when create_missing_assets is enabled",
                    node.id
                ),
            )
            .with_node(&node.id),
        );
    }
}

fn validate_trigger(
    node: &crate::automation::flow::document::FlowNode,
    report: &mut ValidationReport,
) {
    if node.node_type == "inject" {
        let repeat = node
            .config
            .get("repeat")
            .and_then(Value::as_str)
            .unwrap_or("none");
        match repeat {
            "interval" => {
                if node
                    .config
                    .get("interval_seconds")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    == 0
                {
                    report.push_error(
                        FlowValidationFinding::error(
                            "flow.trigger_invalid",
                            format!(
                                "inject node '{}' needs a positive interval_seconds",
                                node.id
                            ),
                        )
                        .with_node(&node.id),
                    );
                }
            }
            "cron" => {
                let expression = node
                    .config
                    .get("cron_expression")
                    .or_else(|| node.config.get("cron"))
                    .and_then(Value::as_str);
                let timezone = node
                    .config
                    .get("timezone")
                    .and_then(Value::as_str)
                    .unwrap_or("UTC");
                match expression {
                    Some(expression) => {
                        if crate::automation::triggers::next_cron(
                            expression,
                            timezone,
                            chrono::Utc::now(),
                        )
                        .is_err()
                        {
                            report.push_error(
                                FlowValidationFinding::error(
                                    "flow.trigger_invalid",
                                    format!(
                                        "inject node '{}' has an invalid cron expression or timezone",
                                        node.id
                                    ),
                                )
                                .with_node(&node.id),
                            );
                        }
                    }
                    None => report.push_error(
                        FlowValidationFinding::error(
                            "flow.trigger_invalid",
                            format!("inject node '{}' needs a cron_expression", node.id),
                        )
                        .with_node(&node.id),
                    ),
                }
            }
            "none" => {}
            other => report.push_error(
                FlowValidationFinding::error(
                    "flow.trigger_invalid",
                    format!(
                        "inject node '{}' has unknown repeat kind '{other}'",
                        node.id
                    ),
                )
                .with_node(&node.id),
            ),
        }
        if let Some(payload_type) = node.config.get("payload_type").and_then(Value::as_str)
            && !matches!(
                payload_type,
                "none" | "string" | "number" | "boolean" | "json" | "timestamp"
            )
        {
            report.push_error(
                FlowValidationFinding::error(
                    "flow.trigger_invalid",
                    format!(
                        "inject node '{}' has unknown payload_type '{}'",
                        node.id, payload_type
                    ),
                )
                .with_node(&node.id),
            );
        }
    }
    if contains_secret_key(&node.config) {
        report.push_error(
            FlowValidationFinding::error(
                "flow.secret_in_config",
                format!(
                    "node '{}' embeds a secret-shaped config key; use a credential reference",
                    node.id
                ),
            )
            .with_node(&node.id),
        );
    }
}

fn contains_secret_key(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, entry)| {
            let lower = key.to_ascii_lowercase();
            lower.contains("secret")
                || lower.contains("password")
                || lower.contains("api_key")
                || lower.contains("apikey")
                || lower == "authorization"
                || lower == "token"
                || contains_secret_key(entry)
        }),
        Value::Array(items) => items.iter().any(contains_secret_key),
        _ => false,
    }
}

fn validate_config(
    node: &crate::automation::flow::document::FlowNode,
    schema: &Value,
    report: &mut ValidationReport,
) {
    let Ok(validator) = jsonschema::validator_for(schema) else {
        report.push_error(
            FlowValidationFinding::error(
                "node.config_invalid",
                "node type has an invalid configuration schema",
            )
            .with_node(&node.id),
        );
        return;
    };
    if !validator.is_valid(&node.config) {
        let messages: Vec<String> = validator
            .iter_errors(&node.config)
            .map(|error| error.to_string())
            .collect();
        report.push_error(
            FlowValidationFinding::error(
                "node.config_invalid",
                format!(
                    "node '{}' configuration is invalid: {}",
                    node.id,
                    messages.join("; ")
                ),
            )
            .with_node(&node.id),
        );
    }
}

fn validate_wires(document: &FlowDocument, registry: &NodeRegistry, report: &mut ValidationReport) {
    let nodes: BTreeMap<&str, &crate::automation::flow::document::FlowNode> = document.node_map();
    let mut seen = BTreeSet::new();
    for wire in &document.wires {
        let Some(source) = nodes.get(wire.source.as_str()) else {
            report.push_error(
                FlowValidationFinding::error(
                    "flow.dangling_wire",
                    format!(
                        "wire '{}' references missing source '{}'",
                        wire.id, wire.source
                    ),
                )
                .with_wire(&wire.id),
            );
            continue;
        };
        if !nodes.contains_key(wire.target.as_str()) {
            report.push_error(
                FlowValidationFinding::error(
                    "flow.dangling_wire",
                    format!(
                        "wire '{}' references missing target '{}'",
                        wire.id, wire.target
                    ),
                )
                .with_wire(&wire.id),
            );
            continue;
        }
        if wire.source == wire.target {
            report.push_error(
                FlowValidationFinding::error(
                    "flow.self_wire",
                    format!(
                        "wire '{}' connects node '{}' to itself",
                        wire.id, wire.source
                    ),
                )
                .with_wire(&wire.id),
            );
        }
        if let Some(target) = nodes.get(wire.target.as_str())
            && crate::automation::runtime::subflow::is_structural(&target.node_type)
            && target.node_type != crate::automation::runtime::subflow::OUTPUT_TYPE
        {
            report.push_error(
                FlowValidationFinding::error(
                    "flow.subflow_io",
                    format!(
                        "wire '{}' targets structural node '{}'",
                        wire.id, wire.target
                    ),
                )
                .with_wire(&wire.id),
            );
        }
        if let Some(source) = nodes.get(wire.source.as_str())
            && source.node_type == crate::automation::runtime::subflow::OUTPUT_TYPE
        {
            report.push_error(
                FlowValidationFinding::error(
                    "flow.subflow_io",
                    format!(
                        "wire '{}' leaves structural output node '{}'",
                        wire.id, wire.source
                    ),
                )
                .with_wire(&wire.id),
            );
        }
        let key = (
            wire.source.clone(),
            wire.source_port.clone(),
            wire.target.clone(),
            wire.target_port.clone(),
        );
        if !seen.insert(key) {
            report.push_error(
                FlowValidationFinding::error(
                    "flow.duplicate_wire",
                    format!("duplicate wire from '{}' to '{}'", wire.source, wire.target),
                )
                .with_wire(&wire.id),
            );
        }
        if let Some(descriptor) = registry
            .get(&source.node_type_key())
            .map(|node| node.descriptor())
            && !descriptor
                .outputs
                .iter()
                .any(|port| port.name == wire.source_port)
        {
            report.push_error(
                FlowValidationFinding::error(
                    "flow.dangling_wire",
                    format!(
                        "wire '{}' uses unknown output '{}' on node '{}'",
                        wire.id, wire.source_port, wire.source
                    ),
                )
                .with_wire(&wire.id),
            );
        }
        if wire.target_port != "in" {
            report.push_error(
                FlowValidationFinding::error(
                    "flow.dangling_wire",
                    format!(
                        "wire '{}' targets unknown input '{}'; only 'in' is supported",
                        wire.id, wire.target_port
                    ),
                )
                .with_wire(&wire.id),
            );
        }
    }

    for node in &document.nodes {
        if node.node_type == "debug" {
            continue;
        }
        if let Some(descriptor) = registry.get(&node.node_type_key()) {
            let descriptor = descriptor.descriptor();
            if descriptor.input.is_some() && document.incoming(&node.id).is_empty() {
                report.push_warning(
                    FlowValidationFinding::warning(
                        "flow.no_input",
                        format!("node '{}' has an input but no incoming wire", node.id),
                    )
                    .with_node(&node.id),
                );
            }
            if !descriptor.outputs.is_empty()
                && document.wires.iter().all(|wire| wire.source != node.id)
            {
                report.push_warning(
                    FlowValidationFinding::warning(
                        "flow.output_unused",
                        format!("node '{}' has no outgoing wire", node.id),
                    )
                    .with_node(&node.id),
                );
            }
        }
    }
}

fn validate_subflows(
    document: &FlowDocument,
    registry: &NodeRegistry,
    report: &mut ValidationReport,
) {
    use crate::automation::runtime::subflow as sf;

    let mut references: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for subflow in &document.subflows {
        let inputs: Vec<&crate::automation::flow::document::FlowNode> = subflow
            .nodes
            .iter()
            .filter(|node| node.node_type == sf::INPUT_TYPE)
            .collect();
        let outputs: Vec<&crate::automation::flow::document::FlowNode> = subflow
            .nodes
            .iter()
            .filter(|node| node.node_type == sf::OUTPUT_TYPE)
            .collect();
        if inputs.len() != 1 {
            report.push_error(FlowValidationFinding::error(
                "flow.subflow_io",
                format!(
                    "subflow '{}' must contain exactly one subflow_input node",
                    subflow.id
                ),
            ));
        }
        if subflow.inputs.len() != 1 {
            report.push_error(FlowValidationFinding::error(
                "flow.subflow_io",
                format!("subflow '{}' must declare exactly one input", subflow.id),
            ));
        } else if let Some(input) = inputs.first()
            && subflow.inputs[0].id != input.id
        {
            report.push_error(FlowValidationFinding::error(
                "flow.subflow_io",
                format!(
                    "subflow '{}' input '{}' must reference subflow_input node '{}'",
                    subflow.id, subflow.inputs[0].id, input.id
                ),
            ));
        }
        if outputs.is_empty() {
            report.push_error(FlowValidationFinding::error(
                "flow.subflow_io",
                format!(
                    "subflow '{}' must contain at least one subflow_output node",
                    subflow.id
                ),
            ));
        }
        if subflow.outputs.is_empty() {
            report.push_error(FlowValidationFinding::error(
                "flow.subflow_io",
                format!("subflow '{}' must declare at least one output", subflow.id),
            ));
        }
        for declared in &subflow.outputs {
            if !outputs.iter().any(|node| node.id == declared.id) {
                report.push_error(FlowValidationFinding::error(
                    "flow.subflow_io",
                    format!(
                        "subflow '{}' output '{}' does not reference a subflow_output node",
                        subflow.id, declared.id
                    ),
                ));
            }
        }
        if subflow.outputs.len() != outputs.len() {
            report.push_error(FlowValidationFinding::error(
                "flow.subflow_io",
                format!(
                    "subflow '{}' declares {} outputs but contains {} subflow_output nodes",
                    subflow.id,
                    subflow.outputs.len(),
                    outputs.len()
                ),
            ));
        }

        let mut node_ids = BTreeSet::new();
        let mut targets = Vec::new();
        for node in &subflow.nodes {
            if !node_ids.insert(node.id.as_str()) {
                report.push_error(
                    FlowValidationFinding::error(
                        "flow.duplicate_id",
                        format!(
                            "duplicate node id '{}' in subflow '{}'",
                            node.id, subflow.id
                        ),
                    )
                    .with_node(&node.id),
                );
            }
            validate_node(node, registry, report);
            if node.node_type == sf::NODE_TYPE {
                if let Some(id) = node.config.get("subflow_id").and_then(Value::as_str) {
                    targets.push(id);
                    if document.subflow(id).is_none() {
                        report.push_error(
                            FlowValidationFinding::error(
                                "flow.subflow_missing",
                                format!(
                                    "subflow node '{}' in subflow '{}' references missing subflow '{}'",
                                    node.id, subflow.id, id
                                ),
                            )
                            .with_node(&node.id),
                        );
                    }
                } else {
                    report.push_error(
                        FlowValidationFinding::error(
                            "flow.subflow_missing",
                            format!(
                                "subflow node '{}' in subflow '{}' is missing config.subflow_id",
                                node.id, subflow.id
                            ),
                        )
                        .with_node(&node.id),
                    );
                }
            }
        }
        references.insert(subflow.id.as_str(), targets);
        validate_subflow_wires(subflow, registry, report);
    }
    for node in &document.nodes {
        if node.node_type == sf::NODE_TYPE {
            let Some(id) = node.config.get("subflow_id").and_then(Value::as_str) else {
                report.push_error(
                    FlowValidationFinding::error(
                        "flow.subflow_missing",
                        format!("subflow node '{}' is missing config.subflow_id", node.id),
                    )
                    .with_node(&node.id),
                );
                continue;
            };
            if document.subflow(id).is_none() {
                report.push_error(
                    FlowValidationFinding::error(
                        "flow.subflow_missing",
                        format!(
                            "subflow node '{}' references missing subflow '{}'",
                            node.id, id
                        ),
                    )
                    .with_node(&node.id),
                );
            }
        }
    }
    for id in references.keys() {
        if reaches(id, id, &references, &mut BTreeSet::new()) {
            report.push_error(FlowValidationFinding::error(
                "flow.subflow_recursive",
                format!("subflow '{}' is recursive", id),
            ));
        }
    }
}

fn validate_subflow_wires(
    subflow: &crate::automation::flow::document::Subflow,
    registry: &NodeRegistry,
    report: &mut ValidationReport,
) {
    use crate::automation::runtime::subflow as sf;

    let nodes: BTreeMap<&str, &crate::automation::flow::document::FlowNode> = subflow
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut seen = BTreeSet::new();
    for wire in &subflow.wires {
        let Some(source) = nodes.get(wire.source.as_str()) else {
            report.push_error(
                FlowValidationFinding::error(
                    "flow.dangling_wire",
                    format!(
                        "wire '{}' in subflow '{}' references missing source '{}'",
                        wire.id, subflow.id, wire.source
                    ),
                )
                .with_wire(&wire.id),
            );
            continue;
        };
        if !nodes.contains_key(wire.target.as_str()) {
            report.push_error(
                FlowValidationFinding::error(
                    "flow.dangling_wire",
                    format!(
                        "wire '{}' in subflow '{}' references missing target '{}'",
                        wire.id, subflow.id, wire.target
                    ),
                )
                .with_wire(&wire.id),
            );
            continue;
        }
        if let Some(target) = nodes.get(wire.target.as_str())
            && target.node_type == sf::INPUT_TYPE
        {
            report.push_error(
                FlowValidationFinding::error(
                    "flow.subflow_io",
                    format!(
                        "wire '{}' in subflow '{}' targets structural input node '{}'",
                        wire.id, subflow.id, wire.target
                    ),
                )
                .with_wire(&wire.id),
            );
        }
        if source.node_type == sf::OUTPUT_TYPE {
            report.push_error(
                FlowValidationFinding::error(
                    "flow.subflow_io",
                    format!(
                        "wire '{}' in subflow '{}' leaves structural output node '{}'",
                        wire.id, subflow.id, wire.source
                    ),
                )
                .with_wire(&wire.id),
            );
        }
        if wire.source == wire.target {
            report.push_error(
                FlowValidationFinding::error(
                    "flow.self_wire",
                    format!(
                        "wire '{}' in subflow '{}' connects node '{}' to itself",
                        wire.id, subflow.id, wire.source
                    ),
                )
                .with_wire(&wire.id),
            );
        }
        let key = (
            wire.source.clone(),
            wire.source_port.clone(),
            wire.target.clone(),
            wire.target_port.clone(),
        );
        if !seen.insert(key) {
            report.push_error(
                FlowValidationFinding::error(
                    "flow.duplicate_wire",
                    format!(
                        "duplicate wire from '{}' to '{}' in subflow '{}'",
                        wire.source, wire.target, subflow.id
                    ),
                )
                .with_wire(&wire.id),
            );
        }
        if source.node_type == sf::INPUT_TYPE {
            if wire.source_port != "out" {
                report.push_error(
                    FlowValidationFinding::error(
                        "flow.dangling_wire",
                        format!(
                            "wire '{}' uses unknown output '{}' on subflow input node",
                            wire.id, wire.source_port
                        ),
                    )
                    .with_wire(&wire.id),
                );
            }
        } else if let Some(descriptor) = registry
            .get(&source.node_type_key())
            .map(|node| node.descriptor())
            && !descriptor
                .outputs
                .iter()
                .any(|port| port.name == wire.source_port)
        {
            report.push_error(
                FlowValidationFinding::error(
                    "flow.dangling_wire",
                    format!(
                        "wire '{}' uses unknown output '{}' on node '{}'",
                        wire.id, wire.source_port, wire.source
                    ),
                )
                .with_wire(&wire.id),
            );
        }
        if wire.target_port != "in" {
            report.push_error(
                FlowValidationFinding::error(
                    "flow.dangling_wire",
                    format!(
                        "wire '{}' targets unknown input '{}'; only 'in' is supported",
                        wire.id, wire.target_port
                    ),
                )
                .with_wire(&wire.id),
            );
        }
    }
}

fn reaches(
    start: &str,
    current: &str,
    references: &BTreeMap<&str, Vec<&str>>,
    seen: &mut BTreeSet<String>,
) -> bool {
    let Some(targets) = references.get(current) else {
        return false;
    };
    for target in targets {
        if *target == start {
            return true;
        }
        if seen.insert((*target).to_string()) && reaches(start, target, references, seen) {
            return true;
        }
    }
    false
}

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}
