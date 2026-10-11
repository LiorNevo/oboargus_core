//! Doc-comment enforcement test for the `obolargus_core::domain` module.
//!
//! Implements FR-014: every public function in `domain::` must have a doc
//! comment that names at least one `FR-###`. This is a mechanical guard; the
//! `FR-` reference is the contract between the spec and the code.
//!
//! Note: this test parses Rust source text directly (rather than via
//! `rustdoc-json` or `syn`) to avoid adding a build-time dependency.

use std::fs;
use std::path::Path;

fn public_functions_mention_fr(path: &Path) -> Result<(), String> {
    let src = fs::read_to_string(path).map_err(|e| format!("read {}: {}", path.display(), e))?;
    if !src.contains("pub fn ") && !src.contains("pub async fn ") {
        return Ok(());
    }
    // Very simple heuristic: each file that contains a `pub fn` must also
    // contain the literal string "FR-" somewhere in a doc comment (lines
    // starting with `///` or `//!`).
    let mut in_doc = false;
    let mut saw_pub_fn = false;
    let mut saw_fr = false;
    for line in src.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("///") || trimmed.starts_with("//!") {
            in_doc = true;
            if line.contains("FR-") {
                saw_fr = true;
            }
        } else if in_doc && trimmed.is_empty() {
            // blank line within a doc block
        } else {
            in_doc = false;
        }
        if line.contains("pub fn ") || line.contains("pub async fn ") {
            saw_pub_fn = true;
        }
    }
    if saw_pub_fn && !saw_fr {
        return Err(format!("{}: public function(s) found but no doc comment mentions 'FR-'", path.display()));
    }
    Ok(())
}

#[test]
fn public_functions_in_domain_module_reference_fr() {
    let domain_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/domain");
    let mut errors = Vec::new();
    visit_rs(&domain_dir, &mut |p| {
        if let Err(e) = public_functions_mention_fr(p) {
            errors.push(e);
        }
    });
    if !errors.is_empty() {
        panic!(
            "the following files have public functions without an FR-### reference in a doc comment:\n  - {}\n\n\
             Add `/// Implements FR-XXX of spec 016-implement-domain-model.` to each public function.",
            errors.join("\n  - ")
        );
    }
}

fn visit_rs(dir: &Path, cb: &mut dyn FnMut(&Path)) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                visit_rs(&p, cb);
            } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
                cb(&p);
            }
        }
    }
}
