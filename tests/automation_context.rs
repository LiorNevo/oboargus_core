//! Context store tests: scopes, durability helpers, and last-write-wins.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::json;

use obolargus_core::automation::runtime::context::{ContextScope, ContextStore};

#[test]
fn scopes_are_isolated() {
    let store = ContextStore::new("flow-1", "global-1", BTreeMap::new(), BTreeMap::new());
    store
        .set(ContextScope::Run, "key", json!("run"))
        .expect("set run");
    store
        .set(ContextScope::Flow, "key", json!("flow"))
        .expect("set flow");
    store
        .set(ContextScope::Global, "key", json!("global"))
        .expect("set global");
    assert_eq!(store.get(ContextScope::Run, "key"), Some(json!("run")));
    assert_eq!(store.get(ContextScope::Flow, "key"), Some(json!("flow")));
    assert_eq!(
        store.get(ContextScope::Global, "key"),
        Some(json!("global"))
    );
}

#[test]
fn flow_and_global_writes_are_marked_dirty() {
    let store = ContextStore::new("flow-1", "global-1", BTreeMap::new(), BTreeMap::new());
    store
        .set(ContextScope::Flow, "a", json!(1))
        .expect("flow write");
    store
        .set(ContextScope::Global, "b", json!(2))
        .expect("global write");
    assert!(store.take_dirty_flow().contains("a"));
    assert!(store.take_dirty_global().contains("b"));
    assert!(store.take_dirty_flow().is_empty());
}

#[test]
fn delete_removes_a_key() {
    let mut initial = BTreeMap::new();
    initial.insert("key".to_string(), json!("value"));
    let store = ContextStore::new("flow-1", "global-1", initial, BTreeMap::new());
    store.delete(ContextScope::Flow, "key").expect("delete");
    assert_eq!(store.get(ContextScope::Flow, "key"), None);
}

#[test]
fn concurrent_writes_are_last_write_wins_per_key() {
    let store = Arc::new(ContextStore::new(
        "flow-1",
        "global-1",
        BTreeMap::new(),
        BTreeMap::new(),
    ));
    let mut handles = Vec::new();
    for value in 0..16 {
        let store = Arc::clone(&store);
        handles.push(std::thread::spawn(move || {
            store
                .set(ContextScope::Flow, "counter", json!(value))
                .expect("write");
        }));
    }
    for handle in handles {
        handle.join().expect("thread");
    }
    assert!(store.get(ContextScope::Flow, "counter").is_some());
}

#[test]
fn empty_keys_are_rejected() {
    let store = ContextStore::new("flow-1", "global-1", BTreeMap::new(), BTreeMap::new());
    let error = store
        .set(ContextScope::Run, "", json!(1))
        .expect_err("empty key");
    assert_eq!(error.code, "node.config_invalid");
}

#[test]
fn scope_parser_accepts_contract_values() {
    assert_eq!(ContextScope::parse("run"), Some(ContextScope::Run));
    assert_eq!(ContextScope::parse("flow"), Some(ContextScope::Flow));
    assert_eq!(ContextScope::parse("global"), Some(ContextScope::Global));
    assert_eq!(ContextScope::parse("other"), None);
}

#[test]
fn subflow_scopes_can_be_seeded_from_durable_context() {
    let mut scoped = BTreeMap::new();
    scoped.insert(
        "flow-1:instance-a".to_string(),
        BTreeMap::from([("seen".to_string(), serde_json::json!(7))]),
    );
    let store = ContextStore::new("flow-1", "workspace", BTreeMap::new(), BTreeMap::new())
        .with_scoped(scoped);
    let child = store.child_flow("instance-a");
    assert_eq!(
        child.get(ContextScope::Flow, "seen"),
        Some(serde_json::json!(7))
    );
    let other = store.child_flow("instance-b");
    assert_eq!(other.get(ContextScope::Flow, "seen"), None);
}
