//! Telemetry and observability utilities

use tracing::{span, Level, Subscriber};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};
use std::sync::Once;
use crate::Result;

static INIT: Once = Once::new();

/// Initialize global tracing subscriber
pub fn init_tracing() -> Result<()> {
    INIT.call_once(|| {
        let env_filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info,orchestrator=debug"));

        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_target(true)
            .with_thread_ids(true)
            .with_file(true)
            .with_line_number(true)
            .with_filter(env_filter);

        tracing_subscriber::registry()
            .with(fmt_layer)
            .init();
    });
    Ok(())
}

/// Initialize JSON tracing for production
pub fn init_tracing_json() -> Result<()> {
    INIT.call_once(|| {
        let env_filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info,orchestrator=debug"));

        let json_layer = tracing_subscriber::fmt::layer()
            .json()
            .with_current_span(true)
            .with_span_list(true)
            .with_filter(env_filter);

        tracing_subscriber::registry()
            .with(json_layer)
            .init();
    });
    Ok(())
}

/// Create a traced span for an operation
pub fn traced_span(name: &str, fields: &[(&str, &dyn std::fmt::Debug)]) -> tracing::Span {
    let span = span!(Level::INFO, name);
    for (key, value) in fields {
        span.record(key, &format!("{:?}", value));
    }
    span
}

/// Macro for timing an async operation
#[macro_export]
macro_rules! trace_async {
    ($name:expr, $fut:expr) => {{
        let span = $crate::telemetry::traced_span($name, &[]);
        let _enter = span.enter();
        let start = std::time::Instant::now();
        let result = $fut.await;
        tracing::info!(duration_ms = %start.elapsed().as_millis(), "Completed");
        result
    }};
}

/// Metrics recording helper
pub struct MetricsRecorder {
    prefix: String,
}

impl MetricsRecorder {
    pub fn new(prefix: &str) -> Self {
        Self { prefix: prefix.to_string() }
    }

    pub fn gauge(&self, name: &str, value: f64) {
        tracing::debug!(metric = %format!("{}.{}", self.prefix, name), value, "gauge");
    }

    pub fn counter(&self, name: &str, value: u64) {
        tracing::debug!(metric = %format!("{}.{}", self.prefix, name), value, "counter");
    }

    pub fn histogram(&self, name: &str, value: f64) {
        tracing::debug!(metric = %format!("{}.{}", self.prefix, name), value, "histogram");
    }

    pub fn timing(&self, name: &str, duration: std::time::Duration) {
        self.histogram(name, duration.as_secs_f64() * 1000.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_recorder() {
        let _ = init_tracing();
        let recorder = MetricsRecorder::new("test");
        recorder.gauge("cpu", 50.0);
        recorder.counter("requests", 100);
        recorder.timing("latency", std::time::Duration::from_millis(50));
    }
}