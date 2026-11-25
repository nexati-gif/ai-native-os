//! Policy engine using OPA/Rego

pub mod engine;
pub mod policies;

pub use engine::{PolicyEngine, PolicyConfig};
pub use policies::DEFAULT_POLICIES;