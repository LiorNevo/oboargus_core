//! Assembly of the complete system schema document.

use serde_json::{Map, Value, json};

use crate::data_dictionary::canonical;
use crate::data_dictionary::entity::{build_entity_definitions, insert_unique};
use crate::data_dictionary::error::DataDictionaryError;
use crate::data_dictionary::fact_type::build_fact_type_definitions;
use crate::data_dictionary::fact_value::build_value_definitions;
use crate::data_dictionary::metadata;
use crate::data_dictionary::version::{SYSTEM_SCHEMA_VERSION, model_version};
use crate::domain::model_description::model_description;
use crate::temporal::catalog::fact_catalog;

/// The exported system data-dictionary schema and its canonical bytes.
#[derive(Debug, Clone)]
pub struct SystemSchema {
    document: Value,
    canonical: String,
    sha256: String,
}

impl SystemSchema {
    /// Builds the schema from the authoritative domain model and fact catalog.
    pub fn build() -> Result<Self, DataDictionaryError> {
        let model = model_description();
        let catalog = fact_catalog();
        if catalog.is_empty() {
            return Err(DataDictionaryError::EmptyCatalog);
        }

        let mut definitions = Map::new();
        merge_definitions(&mut definitions, build_entity_definitions(&model)?)?;
        merge_definitions(&mut definitions, build_value_definitions(catalog)?)?;
        merge_definitions(&mut definitions, build_fact_type_definitions(catalog)?)?;
        metadata::apply_fact_type_known_values(&mut definitions, catalog);

        let document = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$id": format!("urn:obolargus:schema:system:{SYSTEM_SCHEMA_VERSION}"),
            "title": "Obolargus System Data Dictionary",
            "description": metadata::DOCUMENT_DESCRIPTION,
            "x-obolargus-schema-version": SYSTEM_SCHEMA_VERSION,
            "x-obolargus-model-version": model_version(),
            "x-obolargus-relationships": metadata::build_relationships(&model),
            "x-obolargus-invariants": metadata::build_invariants(&model),
            "x-obolargus-cross-field-constraints": metadata::build_cross_field_constraints(),
            "$defs": definitions,
        });
        let canonical = canonical::canonical_json(&document)?;
        let sha256 = canonical::sha256(&canonical);
        Ok(Self {
            document,
            canonical,
            sha256,
        })
    }

    /// Returns the document as a JSON value.
    pub fn document(&self) -> &Value {
        &self.document
    }

    /// Returns the canonical JSON bytes.
    pub fn canonical_json(&self) -> &str {
        &self.canonical
    }

    /// Returns the lowercase SHA-256 hex digest of the canonical bytes.
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

fn merge_definitions(
    target: &mut Map<String, Value>,
    source: Map<String, Value>,
) -> Result<(), DataDictionaryError> {
    for (name, definition) in source {
        insert_unique(target, &name, definition)?;
    }
    Ok(())
}
