//! Metric sample types

use chrono::{DateTime, Utc};
use serde::{Serialize, Deserialize};
use crate::types::DeviceId;

/// A single metric sample
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricSample {
    pub collector: String,
    pub metric: String,
    pub value: f64,
    pub timestamp: DateTime<Utc>,
    pub device_id: DeviceId,
}

/// Aggregated metrics for a time window
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatedMetrics {
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub samples: Vec<MetricSample>,
    pub summaries: std::collections::BTreeMap<String, MetricSummary>,
}

/// Statistical summary of a metric
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricSummary {
    pub count: u64,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub median: f64,
    pub std_dev: f64,
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
}