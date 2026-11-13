//! Deterministic expression evaluator for flow node configuration.

mod eval;

pub use eval::{EvalContext, ExpressionError, eval_rule, eval_str, resolve_value};
