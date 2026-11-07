use crate::data_dictionary::canonical;
use crate::data_dictionary::export_system_schema;

#[test]
fn two_exports_are_byte_identical() {
    let first = export_system_schema().expect("first export");
    let second = export_system_schema().expect("second export");
    assert_eq!(first.canonical_json(), second.canonical_json());
    assert_eq!(first.sha256(), second.sha256());
}

#[test]
fn canonical_bytes_use_two_spaces_and_one_trailing_newline() {
    let schema = export_system_schema().expect("schema");
    let canonical_json = schema.canonical_json();
    assert!(canonical_json.starts_with("{\n  \"$defs\""));
    assert!(canonical_json.ends_with("}\n"));
    assert!(!canonical_json.ends_with("\n\n"));
    assert!(!canonical_json.contains("\r\n"));
    assert!(!canonical_json.contains('\t'));
}

#[test]
fn canonical_hash_matches_the_canonical_bytes() {
    let schema = export_system_schema().expect("schema");
    assert_eq!(schema.sha256(), canonical::sha256(schema.canonical_json()));
    assert_eq!(schema.sha256().len(), 64);
    assert!(
        schema
            .sha256()
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
    );
}

#[test]
fn document_contains_no_run_dependent_content() {
    let schema = export_system_schema().expect("schema");
    let canonical_json = schema.canonical_json();
    for forbidden in [
        "generated_at",
        "generatedAt",
        "hostname",
        "localhost",
        "DATABASE_URL",
        "postgres://",
        "postgresql://",
        "password",
        "secret",
        "token",
        "127.0.0.1",
    ] {
        assert!(
            !canonical_json.contains(forbidden),
            "canonical document contains {forbidden}"
        );
    }
}
