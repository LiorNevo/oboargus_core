//! Morpheus agent domain: conversations, turns, approvals, tools, and vision.
//!
//! See `specs/032-morpheus-agent-chat/`. HTTP handlers and the durable engine
//! live in `obolargus_server`; this module owns records, SQL, prompt assembly,
//! tool contracts, approval policy, and observation validation.

/// Approval policy and scope fingerprints.
pub mod approvals;
/// Prompt/context assembly.
pub mod context;
/// Typed failures.
pub mod error;
/// Conversation, turn, action, and approval records.
pub mod model;
/// Owner-scoped SQL.
pub mod persistence;
/// Conversation/message retention pruning.
pub mod retention;
/// Deterministic conversation titles.
pub mod title;
/// Governed tool catalog.
pub mod tools;
/// Screen-observation validation.
pub mod vision;
