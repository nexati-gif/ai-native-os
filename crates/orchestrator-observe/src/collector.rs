//! Main observe collector coordinating all data sources

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{info, warn, debug, error};
use crate::{ObserveCollector, CollectorConfig, MetricSample, EbpfManager};
use orchestrator_common::{EventBus, Event, Error, Result, MetricsRecorder};
use orchestrator_common::types::{AnomalySeverity, DeviceId};

/// Observe collector managing all passive data collection
pub struct ObserveCollector {
    config: CollectorConfig,
    event_bus: EventBus,
    metrics: MetricsRecorder,
    device_id: DeviceId,
    running: Arc<RwLock<bool>>,
    /// eBPF manager for all tracepoints
    ebpf_manager: Option<EbpfManager>,
}

impl ObserveCollector {
    pub async fn new(config: &CollectorConfig, event_bus: EventBus) -> Result<Self> {
        let device_id = DeviceId::new(&hostname::get()?.to_string_lossy());
        
        // Create eBPF manager
        let ebpf_manager = EbpfManager::new(event_bus.clone()).await?;
        
        Ok(Self {
            config: config.clone(),
            event_bus: event_bus.clone(),
            metrics: MetricsRecorder::new("observe"),
            device_id,
            running: Arc::new(RwLock::new(false)),
            ebpf_manager: Some(ebpf_manager),
        })
    }

    /// Start all enabled collectors
    pub async fn start(&mut self) -> Result<()> {
        *self.running.write().await = true;
        info!("Starting observe collectors");

        // Load eBPF programs
        if let Some(ref mut ebpf) = self.ebpf_manager {
            ebpf.load_programs().await?;
            self.metrics.counter("ebpf_programs_loaded", 1);
        }

        // Emit initial system metrics
        self.collect_system_metrics().await?;

        Ok(())
    }

    /// Poll events from all collectors
    pub async fn poll_events(&mut self) -> Result<()> {
        if !*self.running.read().await {
            return Ok(());
        }

        // Poll eBPF ring buffers
        if let Some(ref mut ebpf) = self.ebpf_manager {
            ebpf.poll_events().await?;
        }

        // Also collect system metrics (non-eBPF)
        self.collect_system_metrics().await?;

        Ok(())
    }

    /// Collect basic system metrics (CPU, memory, disk, network)
    async fn collect_system_metrics(&self) -> Result<()> {
        // CPU usage
        if let Ok(cpu) = self.read_cpu_usage().await {
            self.emit_metric("cpu_usage_percent", cpu).await?;
        }

        // Memory usage
        if let Ok(mem) = self.read_memory_usage().await {
            self.emit_metric("memory_usage_percent", mem.percent).await?;
            self.emit_metric("memory_available_mib", mem.available_mib as f64).await?;
        }

        // Disk usage
        if let Ok(disk) = self.read_disk_usage().await {
            self.emit_metric("disk_usage_percent", disk.percent).await?;
        }

        // Network stats
        if let Ok(net) = self.read_network_stats().await {
            self.emit_metric("network_rx_bytes_sec", net.rx_bytes_sec).await?;
            self.emit_metric("network_tx_bytes_sec", net.tx_bytes_sec).await?;
        }

        // Load average
        if let Ok(load) = self.read_load_avg().await {
            self.emit_metric("load_1m", load.0).await?;
            self.emit_metric("load_5m", load.1).await?;
            self.emit_metric("load_15m", load.2).await?;
        }

        Ok(())
    }

    async fn read_cpu_usage(&self) -> Result<f64> {
        // Read /proc/stat for CPU usage
        let content = tokio::fs::read_to_string("/proc/stat").await?;
        let first_line = content.lines().next().unwrap_or("");
        let parts: Vec<&str> = first_line.split_whitespace().collect();
        
        if parts.len() >= 8 {
            let user: u64 = parts[1].parse().unwrap_or(0);
            let nice: u64 = parts[2].parse().unwrap_or(0);
            let system: u64 = parts[3].parse().unwrap_or(0);
            let idle: u64 = parts[4].parse().unwrap_or(0);
            let iowait: u64 = parts[5].parse().unwrap_or(0);
            let irq: u64 = parts[6].parse().unwrap_or(0);
            let softirq: u64 = parts[7].parse().unwrap_or(0);
            
            let total = user + nice + system + idle + iowait + irq + softirq;
            let non_idle = user + nice + system + irq + softirq;
            
            if total > 0 {
                return Ok((non_idle as f64 / total as f64) * 100.0);
            }
        }
        
        Ok(0.0)
    }

    async fn read_memory_usage(&self) -> Result<MemoryInfo> {
        let content = tokio::fs::read_to_string("/proc/meminfo").await?;
        let mut total = 0u64;
        let mut available = 0u64;
        
        for line in content.lines() {
            if line.starts_with("MemTotal:") {
                total = line.split_whitespace().nth(1).unwrap_or("0").parse().unwrap_or(0);
            } else if line.starts_with("MemAvailable:") {
                available = line.split_whitespace().nth(1).unwrap_or("0").parse().unwrap_or(0);
            }
        }
        
        let percent = if total > 0 {
            ((total - available) as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        
        Ok(MemoryInfo { percent, available_mib: available / 1024 })
    }

    async fn read_disk_usage(&self) -> Result<DiskInfo> {
        // Use df to get root filesystem usage
        let output = tokio::process::Command::new("df")
            .args(["-B1", "/"])
            .output()
            .await?;
        
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 5 {
                let total: u64 = parts[1].parse().unwrap_or(0);
                let used: u64 = parts[2].parse().unwrap_or(0);
                let percent = if total > 0 { (used as f64 / total as f64) * 100.0 } else { 0.0 };
                return Ok(DiskInfo { percent });
            }
        }
        
        Ok(DiskInfo { percent: 0.0 })
    }

    async fn read_network_stats(&self) -> Result<NetworkInfo> {
        let content = tokio::fs::read_to_string("/proc/net/dev").await?;
        let mut rx_bytes = 0u64;
        let mut tx_bytes = 0u64;
        
        for line in content.lines().skip(2) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 10 {
                // Skip loopback
                if parts[0].starts_with("lo") {
                    continue;
                }
                rx_bytes += parts[1].parse::<u64>().unwrap_or(0);
                tx_bytes += parts[9].parse::<u64>().unwrap_or(0);
            }
        }
        
        // Convert to bytes/sec (assuming poll interval ~100ms)
        let rx_bytes_sec = (rx_bytes as f64) * 10.0;
        let tx_bytes_sec = (tx_bytes as f64) * 10.0;
        
        Ok(NetworkInfo { rx_bytes_sec, tx_bytes_sec })
    }

    async fn read_load_avg(&self) -> Result<(f64, f64, f64)> {
        let content = tokio::fs::read_to_string("/proc/loadavg").await?;
        let parts: Vec<&str> = content.split_whitespace().collect();
        if parts.len() >= 3 {
            Ok((
                parts[0].parse().unwrap_or(0.0),
                parts[1].parse().unwrap_or(0.0),
                parts[2].parse().unwrap_or(0.0),
            ))
        } else {
            Ok((0.0, 0.0, 0.0))
        }
    }

    async fn emit_metric(&self, name: &str, value: f64) -> Result<()> {
        let sample = MetricSample {
            collector: "system".to_string(),
            metric: name.to_string(),
            value,
            timestamp: chrono::Utc::now(),
            device_id: self.device_id.clone(),
        };
        
        self.event_bus.send(Event::MetricSampled {
            collector: sample.collector,
            metric: sample.metric,
            value: sample.value,
            timestamp: sample.timestamp,
        })?;
        
        self.metrics.gauge(name, value);
        Ok(())
    }

    /// Shutdown all collectors
    pub async fn shutdown(&mut self) {
        *self.running.write().await = false;
        info!("Shutting down observe collectors");
        
        if let Some(mut ebpf) = self.ebpf_manager.take() {
            ebpf.shutdown().await;
        }
    }
    
    /// Get current eBPF stats
    pub async fn get_ebpf_stats(&self) -> Result<std::collections::HashMap<String, f64>> {
        if let Some(ref ebpf) = self.ebpf_manager {
            ebpf.read_stats().await
        } else {
            Ok(std::collections::HashMap::new())
        }
    }
}

struct MemoryInfo {
    percent: f64,
    available_mib: u64,
}

struct DiskInfo {
    percent: f64,
}

struct NetworkInfo {
    rx_bytes_sec: f64,
    tx_bytes_sec: f64,
}