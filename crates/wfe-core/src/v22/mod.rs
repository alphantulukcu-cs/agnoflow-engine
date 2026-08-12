//! WFD v2.2 runtime — Named Nodes, Single-Rule C_A.
//! Spec: docs/spec/runtime-semantics.md §3, §4, §7, §8.

pub mod display;
pub mod dollar;
pub mod duration;
pub mod effects;
pub mod env;
pub mod eval;
pub mod grants;
pub mod matcher;
pub mod pipeline;
pub mod ports;
pub mod resolver;
pub mod visibility;
