use crate::data_dictionary::export_system_schema;

#[test]
fn document_is_valid_against_the_2020_12_meta_schema() {
    let schema = export_system_schema().expect("schema");
    jsonschema::meta::validate(schema.document()).expect("meta-schema validation");
}
