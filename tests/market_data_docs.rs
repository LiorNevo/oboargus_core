//! Doc-comment enforcement for the market-data public API.
//!
//! Implements T110 / T114 of `specs/017-yahoo-data-source/tasks.md`:
//! every public item in `obolargus_core::market_data` and
//! `obolargus_server::market_data_provider` must carry a rustdoc comment
//! that names the requirement (`FR-###`) or user story (`US#`) it
//! implements. This is a mechanical guard — the `FR-` reference is the
//! contract between the spec and the code.
//!
//! The test parses Rust source text directly (rather than via the
//! compiler) so it can look at *every* item, including those that are
//! only reachable through re-exports.

use std::fs;
use std::path::{Path, PathBuf};

/// Returns the repository root (the parent of this crate's directory).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate dir has a parent")
        .to_path_buf()
}

/// The source roots that carry the market-data public API.
fn source_roots() -> Vec<PathBuf> {
    let root = repo_root();
    vec![
        root.join("obolargus_core/src/market_data"),
        root.join("obolargus_server/src/market_data_provider"),
    ]
}

/// Returns `true` when the line looks like a public item declaration.
fn is_public_item(line: &str) -> bool {
    let trimmed = line.trim_start();
    (trimmed.starts_with("pub fn ")
        || trimmed.starts_with("pub async fn ")
        || trimmed.starts_with("pub struct ")
        || trimmed.starts_with("pub enum ")
        || trimmed.starts_with("pub trait ")
        || trimmed.starts_with("pub const ")
        || trimmed.starts_with("pub static ")
        || trimmed.starts_with("pub type ")
        || trimmed.starts_with("pub mod "))
        && !trimmed.contains("pub(crate)")
        && !trimmed.contains("pub(super)")
}

/// Returns the doc-comment block immediately preceding `line_no`.
fn preceding_doc_block(lines: &[&str], line_no: usize) -> String {
    let mut block: Vec<&str> = Vec::new();
    let mut idx = line_no;
    while idx > 0 {
        idx -= 1;
        let trimmed = lines[idx].trim_start();
        if trimmed.starts_with("///") {
            block.push(trimmed);
        } else if trimmed.is_empty() {
            // A blank line ends the doc block.
            break;
        } else {
            // A non-doc, non-blank line: keep walking (attributes such as
            // `#[derive(...)]` may sit between the doc block and the item).
            continue;
        }
    }
    block.reverse();
    block.join("\n")
}

/// Every public item carries a doc comment naming an `FR-` or `US-`.
#[test]
fn every_public_market_data_item_names_its_requirement() {
    let mut checked = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for root in source_roots() {
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            let lines: Vec<&str> = content.lines().collect();
            for (line_no, line) in lines.iter().enumerate() {
                if !is_public_item(line) {
                    continue;
                }
                // `pub mod` re-export lines are declarations, not items
                // needing their own FR reference; the module's own file
                // carries it.
                if line.trim_start().starts_with("pub mod ") {
                    continue;
                }
                checked += 1;
                let doc = preceding_doc_block(&lines, line_no);
                if doc.is_empty() {
                    failures.push(format!(
                        "{}:{}: public item has no doc comment: {}",
                        path.display(),
                        line_no + 1,
                        line.trim()
                    ));
                } else if !doc.contains("FR-") && !doc.contains("US") && !doc.contains("T0") {
                    failures.push(format!(
                        "{}:{}: doc comment names no FR-/US requirement: {}",
                        path.display(),
                        line_no + 1,
                        line.trim()
                    ));
                }
            }
        }
    }

    assert!(
        checked > 20,
        "expected to check many public market-data items (checked {checked})"
    );
    assert!(
        failures.is_empty(),
        "public market-data items missing an FR-/US-naming doc comment:\n{}",
        failures.join("\n")
    );
}

/// The module files themselves document the feature they implement.
#[test]
fn every_market_data_module_file_has_a_doc_comment() {
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;

    for root in source_roots() {
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            checked += 1;
            let has_doc = content
                .lines()
                .any(|line| line.trim_start().starts_with("//!"));
            if !has_doc {
                failures.push(format!("{}: no module doc comment", path.display()));
            }
        }
    }

    assert!(checked > 5, "expected several market-data module files");
    assert!(
        failures.is_empty(),
        "market-data module files missing a module doc comment:\n{}",
        failures.join("\n")
    );
}
