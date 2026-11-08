//! Flow document, schema, and validation.

pub mod document;
pub mod schema;
pub mod validate;

pub use crate::automation::error::ValidationReport;
pub use document::{
    FLOW_SCHEMA_VERSION, FlowDocument, FlowNode, NodePosition, RetryPolicy, RuntimeInput, Subflow,
    SubflowInput, SubflowOutput, Wire,
};
pub use schema::flow_schema;
pub use validate::validate_flow;
