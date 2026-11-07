//! Generates the committed system data-dictionary artifact.
//!
//! Usage:
//!
//! ```text
//! cargo run -p obolargus-core --bin generate_system_schema -- <PATH>
//! ```

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use obolargus_core::data_dictionary::export_system_schema;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("generate_system_schema: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut arguments = env::args_os().skip(1);
    let Some(path) = arguments.next().map(PathBuf::from) else {
        return Err("usage: generate_system_schema <PATH>".to_string());
    };
    if arguments.next().is_some() {
        return Err("usage: generate_system_schema <PATH>".to_string());
    }
    let schema = export_system_schema().map_err(|error| error.to_string())?;
    fs::write(&path, schema.canonical_json())
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    Ok(())
}
