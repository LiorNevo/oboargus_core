//! Run, flow, and global context.
//!
//! Run context is ephemeral. Flow and global context are durable and are
//! persisted by the executor after a node writes them. Writes are atomic per
//! key and last-write-wins under concurrent runs.
//!
//! A subflow instance receives its own flow-context namespace while sharing
//! the run and global namespaces with its parent. The scope id for a subflow
//! instance is `<flow scope>:<instance id>`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, RwLock};

use serde_json::Value;

use crate::automation::error::NodeExecutionError;

/// Context scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextScope {
    /// Ephemeral per-run context.
    Run,
    /// Durable per-flow context.
    Flow,
    /// Durable per-workspace context.
    Global,
}

impl ContextScope {
    /// Parses a scope name.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "run" => Some(Self::Run),
            "flow" => Some(Self::Flow),
            "global" => Some(Self::Global),
            _ => None,
        }
    }
}

type SharedMap = Arc<RwLock<BTreeMap<String, Value>>>;
type SharedDirty = Arc<Mutex<BTreeSet<String>>>;

/// A run's context store.
#[derive(Debug, Clone)]
pub struct ContextStore {
    run: SharedMap,
    flow: SharedMap,
    global: SharedMap,
    flow_scope_id: String,
    global_scope_id: String,
    dirty_flow: SharedDirty,
    dirty_global: SharedDirty,
    scoped: Arc<RwLock<BTreeMap<String, BTreeMap<String, Value>>>>,
}

impl ContextStore {
    /// Creates a store with optional preloaded flow/global values.
    pub fn new(
        flow_scope_id: impl Into<String>,
        global_scope_id: impl Into<String>,
        flow: BTreeMap<String, Value>,
        global: BTreeMap<String, Value>,
    ) -> Self {
        Self {
            run: Arc::new(RwLock::new(BTreeMap::new())),
            flow: Arc::new(RwLock::new(flow)),
            global: Arc::new(RwLock::new(global)),
            flow_scope_id: flow_scope_id.into(),
            global_scope_id: global_scope_id.into(),
            dirty_flow: Arc::new(Mutex::new(BTreeSet::new())),
            dirty_global: Arc::new(Mutex::new(BTreeSet::new())),
            scoped: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    /// Seeds durable context for subflow instance scopes.
    ///
    /// Keys are full scope ids (`<flow scope>:<instance id>`). The values are
    /// copied into the matching child store when the instance starts.
    pub fn with_scoped(self, scoped: BTreeMap<String, BTreeMap<String, Value>>) -> Self {
        if let Ok(mut guard) = self.scoped.write() {
            *guard = scoped;
        }
        self
    }

    /// Flow scope id.
    pub fn flow_scope_id(&self) -> &str {
        &self.flow_scope_id
    }

    /// Global scope id.
    pub fn global_scope_id(&self) -> &str {
        &self.global_scope_id
    }

    /// Creates a child store for a subflow instance.
    ///
    /// The child shares the run, global, and global-dirty state with its
    /// parent, starts with the durable context for its instance scope, and
    /// tracks its own flow writes separately.
    pub fn child_flow(&self, instance_id: &str) -> Self {
        let flow_scope_id = format!("{}:{instance_id}", self.flow_scope_id);
        let initial = self
            .scoped
            .read()
            .ok()
            .and_then(|guard| guard.get(&flow_scope_id).cloned())
            .unwrap_or_default();
        Self {
            run: Arc::clone(&self.run),
            flow: Arc::new(RwLock::new(initial)),
            global: Arc::clone(&self.global),
            flow_scope_id,
            global_scope_id: self.global_scope_id.clone(),
            dirty_flow: Arc::new(Mutex::new(BTreeSet::new())),
            dirty_global: Arc::clone(&self.dirty_global),
            scoped: Arc::clone(&self.scoped),
        }
    }

    /// Reads a context value.
    pub fn get(&self, scope: ContextScope, key: &str) -> Option<Value> {
        let map = self.map(scope);
        let data = map.read().ok()?;
        data.get(key).cloned()
    }

    /// Returns a snapshot of one scope.
    pub fn snapshot(&self, scope: ContextScope) -> BTreeMap<String, Value> {
        let map = self.map(scope);
        let Ok(data) = map.read() else {
            return BTreeMap::new();
        };
        data.clone()
    }

    /// Writes a context value.
    pub fn set(
        &self,
        scope: ContextScope,
        key: &str,
        value: Value,
    ) -> Result<(), NodeExecutionError> {
        if key.is_empty() {
            return Err(NodeExecutionError::new(
                "node.config_invalid",
                "context key must be non-empty",
            ));
        }
        let map = self.map(scope);
        let mut data = map
            .write()
            .map_err(|_| NodeExecutionError::new("node.internal", "context lock poisoned"))?;
        data.insert(key.to_string(), value);
        drop(data);
        match scope {
            ContextScope::Run => Ok(()),
            ContextScope::Flow => self.mark_dirty(&self.dirty_flow, key),
            ContextScope::Global => self.mark_dirty(&self.dirty_global, key),
        }
    }

    /// Deletes a context value.
    pub fn delete(&self, scope: ContextScope, key: &str) -> Result<(), NodeExecutionError> {
        let map = self.map(scope);
        let mut data = map
            .write()
            .map_err(|_| NodeExecutionError::new("node.internal", "context lock poisoned"))?;
        data.remove(key);
        drop(data);
        match scope {
            ContextScope::Run => Ok(()),
            ContextScope::Flow => self.mark_dirty(&self.dirty_flow, key),
            ContextScope::Global => self.mark_dirty(&self.dirty_global, key),
        }
    }

    /// Returns and clears the dirty flow keys.
    pub fn take_dirty_flow(&self) -> BTreeSet<String> {
        self.take_dirty(&self.dirty_flow)
    }

    /// Returns and clears the dirty global keys.
    pub fn take_dirty_global(&self) -> BTreeSet<String> {
        self.take_dirty(&self.dirty_global)
    }

    fn map(&self, scope: ContextScope) -> &SharedMap {
        match scope {
            ContextScope::Run => &self.run,
            ContextScope::Flow => &self.flow,
            ContextScope::Global => &self.global,
        }
    }

    fn mark_dirty(&self, dirty: &SharedDirty, key: &str) -> Result<(), NodeExecutionError> {
        dirty
            .lock()
            .map_err(|_| NodeExecutionError::new("node.internal", "context lock poisoned"))?
            .insert(key.to_string());
        Ok(())
    }

    fn take_dirty(&self, dirty: &SharedDirty) -> BTreeSet<String> {
        dirty
            .lock()
            .map(|mut guard| std::mem::take(&mut *guard))
            .unwrap_or_default()
    }
}
