//! Planner: LLM + RAG for proposal generation

pub mod model;
pub mod rag;
pub mod prompts;
pub mod planner;

pub use planner::{Planner, PlannerContext, PlannerConfig};
pub use model::{InferenceEngine, ModelManager, CandleEngine, LlamaCppEngine};
pub use rag::{RagIndex, RagQuery, RagChunk};