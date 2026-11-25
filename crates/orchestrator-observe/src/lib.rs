//! Passive observation collectors

pub mod collector;
pub mod ebpf_loader;
pub mod ebpf_programs;
pub mod metrics;
pub mod perf;

pub use collector::{ObserveCollector, CollectorConfig};
pub use ebpf_loader::EbpfManager;
pub use metrics::MetricSample;