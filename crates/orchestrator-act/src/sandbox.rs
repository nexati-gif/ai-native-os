//! Sandbox execution using Firecracker microVMs and QEMU

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command as TokioCommand;
use tracing::{info, warn, debug, error};
use orchestrator_common::{Error, Result, MetricsRecorder};
use orchestrator_common::types::{Proposal, TestPlan, BenchmarkSpec};

/// Sandbox backend type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxBackend {
    Firecracker,
    Qemu,
}

/// Sandbox configuration
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    pub backend: SandboxBackend,
    pub firecracker: FirecrackerConfig,
    pub qemu: QemuConfig,
    pub warm_pool_size: usize,
}

#[derive(Debug, Clone)]
pub struct FirecrackerConfig {
    pub vcpus: u32,
    pub memory_mib: u64,
    pub kernel_path: PathBuf,
    pub initrd_path: PathBuf,
    pub rootfs_path: PathBuf,
    pub boot_args: String,
    pub socket_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct QemuConfig {
    pub vcpus: u32,
    pub memory_mib: u64,
    pub kernel_path: PathBuf,
    pub initrd_path: PathBuf,
    pub disk_path: PathBuf,
    pub enable_kvm: bool,
}

/// Sandbox manager
pub struct SandboxManager {
    config: SandboxConfig,
    metrics: MetricsRecorder,
    warm_pool: Arc<Mutex<Vec<Box<dyn SandboxInstance>>>>,
}

impl SandboxManager {
    pub fn new(config: SandboxConfig) -> Self {
        Self {
            config,
            metrics: MetricsRecorder::new("sandbox"),
            warm_pool: Arc::new(Mutex::new(Vec::new())),
        }
    }
    
    /// Initialize warm pool
    pub async fn init_warm_pool(&self) -> Result<()> {
        if self.config.warm_pool_size == 0 {
            return Ok(());
        }
        
        info!("Initializing warm pool of {} sandboxes", self.config.warm_pool_size);
        
        for _ in 0..self.config.warm_pool_size {
            let instance = self.create_sandbox().await?;
            self.warm_pool.lock().await.push(instance);
        }
        
        Ok(())
    }
    
    /// Run tests for a proposal
    pub async fn run_tests(&self, proposal: &Proposal, test_plan: &TestPlan, generation: &crate::nix::GenerationResult) -> Result<TestResults> {
        let start = std::time::Instant::now();
        info!("Running tests for proposal: {}", proposal.id);
        
        // Get or create sandbox
        let sandbox = self.get_sandbox().await?;
        
        // Prepare test environment
        self.prepare_test_env(&sandbox, proposal, generation).await?;
        
        // Run unit tests
        let unit_results = if !test_plan.unit_tests.is_empty() {
            self.run_unit_tests(&sandbox, &test_plan.unit_tests).await?
        } else {
            vec![]
        };
        
        // Run integration tests
        let integration_results = if !test_plan.integration_tests.is_empty() {
            self.run_integration_tests(&sandbox, &test_plan.integration_tests).await?
        } else {
            vec![]
        };
        
        // Run benchmarks
        let benchmark_results = if !test_plan.benchmarks.is_empty() {
            self.run_benchmarks(&sandbox, &test_plan.benchmarks).await?
        } else {
            vec![]
        };
        
        // Release sandbox back to pool or destroy
        self.release_sandbox(sandbox).await?;
        
        let results = TestResults {
            unit_tests: unit_results,
            integration_tests: integration_results,
            benchmarks: benchmark_results,
            total_duration: start.elapsed(),
            passed: true, // Will be set based on criteria
        };
        
        self.metrics.timing("test_duration", start.elapsed());
        self.metrics.counter("test_runs", 1);
        
        Ok(results)
    }
    
    /// Run canary test (gradual rollout)
    pub async fn run_canary(&self, proposal: &Proposal, test_plan: &TestPlan, generation: &crate::nix::GenerationResult) -> Result<CanaryResults> {
        info!("Running canary for proposal: {} ({}s)", proposal.id, test_plan.canary_duration_secs);
        
        let sandbox = self.get_sandbox().await?;
        self.prepare_test_env(&sandbox, proposal, generation).await?;
        
        // Start workload
        let workload_handle = self.start_workload(&sandbox, proposal).await?;
        
        // Monitor for duration
        let mut metrics = Vec::new();
        let start = std::time::Instant::now();
        let duration = Duration::from_secs(test_plan.canary_duration_secs);
        
        while start.elapsed() < duration {
            let m = self.collect_metrics(&sandbox).await?;
            metrics.push(m);
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        
        // Stop workload
        self.stop_workload(&sandbox, workload_handle).await?;
        self.release_sandbox(sandbox).await?;
        
        // Analyze metrics
        let analysis = self.analyze_canary_metrics(&metrics, &test_plan.success_criteria)?;
        
        Ok(CanaryResults {
            metrics,
            analysis,
            passed: analysis.all_passed,
        })
    }
    
    /// Get sandbox from pool or create new
    async fn get_sandbox(&self) -> Result<Box<dyn SandboxInstance>> {
        let mut pool = self.warm_pool.lock().await;
        
        if let Some(instance) = pool.pop() {
            // Verify it's still healthy
            if instance.is_healthy().await {
                return Ok(instance);
            }
        }
        
        drop(pool);
        self.create_sandbox().await
    }
    
    /// Release sandbox back to pool
    async fn release_sandbox(&self, instance: Box<dyn SandboxInstance>) -> Result<()> {
        if self.config.warm_pool_size > 0 {
            let mut pool = self.warm_pool.lock().await;
            if pool.len() < self.config.warm_pool_size {
                pool.push(instance);
                return Ok(());
            }
        }
        
        // Pool full or disabled - destroy
        instance.destroy().await
    }
    
    /// Create a new sandbox instance
    async fn create_sandbox(&self) -> Result<Box<dyn SandboxInstance>> {
        match self.config.backend {
            SandboxBackend::Firecracker => FirecrackerSandbox::new(&self.config.firecracker).await,
            SandboxBackend::Qemu => QemuSandbox::new(&self.config.qemu).await,
        }
    }
    
    /// Prepare test environment in sandbox
    async fn prepare_test_env(&self, sandbox: &dyn SandboxInstance, _proposal: &Proposal, generation: &crate::nix::GenerationResult) -> Result<()> {
        // Copy test files, benchmarks, etc. to sandbox
        // This would use vsock or SSH to transfer files
        sandbox.prepare(generation).await
    }
    
    /// Run unit tests
    async fn run_unit_tests(&self, sandbox: &dyn SandboxInstance, tests: &[String]) -> Result<Vec<TestResult>> {
        let mut results = Vec::new();
        
        for test in tests {
            let start = std::time::Instant::now();
            let output = sandbox.exec(&format!("cargo test {}", test)).await?;
            let duration = start.elapsed();
            
            results.push(TestResult {
                name: test.clone(),
                passed: output.status.success(),
                duration,
                output: output.stdout,
                error: if output.status.success() { None } else { Some(output.stderr) },
            });
        }
        
        Ok(results)
    }
    
    /// Run integration tests
    async fn run_integration_tests(&self, sandbox: &dyn SandboxInstance, tests: &[String]) -> Result<Vec<TestResult>> {
        let mut results = Vec::new();
        
        for test in tests {
            let start = std::time::Instant::now();
            let output = sandbox.exec(&format!("/usr/local/bin/{}", test)).await?;
            let duration = start.elapsed();
            
            results.push(TestResult {
                name: test.clone(),
                passed: output.status.success(),
                duration,
                output: output.stdout,
                error: if output.status.success() { None } else { Some(output.stderr) },
            });
        }
        
        Ok(results)
    }
    
    /// Run benchmarks
    async fn run_benchmarks(&self, sandbox: &dyn SandboxInstance, benchmarks: &[String]) -> Result<Vec<BenchmarkResult>> {
        let mut results = Vec::new();
        
        for bench in benchmarks {
            let start = std::time::Instant::now();
            let output = sandbox.exec(&format!("/usr/local/bin/{}", bench)).await?;
            let duration = start.elapsed();
            
            // Parse benchmark output for metrics
            let metrics = parse_benchmark_output(&output.stdout);
            
            results.push(BenchmarkResult {
                name: bench.clone(),
                duration,
                metrics,
                passed: output.status.success(),
                output: output.stdout,
            });
        }
        
        Ok(results)
    }
    
    /// Start workload for canary
    async fn start_workload(&self, sandbox: &dyn SandboxInstance, _proposal: &Proposal) -> Result<WorkloadHandle> {
        // Start representative workload
        let handle = sandbox.exec_bg("systemd-run --scope --user workload-generator").await?;
        Ok(handle)
    }
    
    /// Stop workload
    async fn stop_workload(&self, sandbox: &dyn SandboxInstance, handle: WorkloadHandle) -> Result<()> {
        sandbox.kill_process(handle.pid).await
    }
    
    /// Collect metrics from sandbox
    async fn collect_metrics(&self, sandbox: &dyn SandboxInstance) -> Result<CanaryMetrics> {
        // Collect CPU, memory, latency, etc.
        let output = sandbox.exec("cat /proc/stat /proc/meminfo /proc/loadavg").await?;
        
        Ok(CanaryMetrics {
            timestamp: chrono::Utc::now(),
            cpu_usage: parse_cpu_usage(&output.stdout),
            memory_usage: parse_memory_usage(&output.stdout),
            load_avg: parse_load_avg(&output.stdout),
        })
    }
    
    /// Analyze canary metrics
    fn analyze_canary_metrics(&self, metrics: &[CanaryMetrics], criteria: &SuccessCriteria) -> Result<CanaryAnalysis> {
        // Statistical analysis of metrics
        let cpu_avg = if metrics.is_empty() { 0.0 } else { metrics.iter().map(|m| m.cpu_usage).sum::<f64>() / metrics.len() as f64 };
        let mem_avg = if metrics.is_empty() { 0.0 } else { metrics.iter().map(|m| m.memory_usage).sum::<f64>() / metrics.len() as f64 };
        
        let mut all_passed = true;
        let mut violations = Vec::new();
        
        if cpu_avg > criteria.max_cpu_pct.unwrap_or(90.0) {
            all_passed = false;
            violations.push(format!("CPU usage {:.1}% exceeds threshold", cpu_avg));
        }
        
        if mem_avg > criteria.max_memory_pct.unwrap_or(90.0) {
            all_passed = false;
            violations.push(format!("Memory usage {:.1}% exceeds threshold", mem_avg));
        }
        
        Ok(CanaryAnalysis {
            all_passed,
            violations,
            cpu_avg,
            mem_avg,
            sample_count: metrics.len(),
        })
    }
}

/// Sandbox instance trait
#[async_trait::async_trait]
pub trait SandboxInstance: Send + Sync {
    async fn prepare(&self, generation: &crate::nix::GenerationResult) -> Result<()>;
    async fn exec(&self, cmd: &str) -> Result<SandboxOutput>;
    async fn exec_bg(&self, cmd: &str) -> Result<WorkloadHandle>;
    async fn kill_process(&self, pid: u32) -> Result<()>;
    async fn is_healthy(&self) -> bool;
    async fn destroy(&self) -> Result<()>;
    fn id(&self) -> &str;
}

/// Firecracker microVM sandbox
pub struct FirecrackerSandbox {
    id: String,
    config: FirecrackerConfig,
    process: Arc<Mutex<Option<tokio::process::Child>>>,
    socket_path: PathBuf,
}

impl FirecrackerSandbox {
    pub async fn new(config: &FirecrackerConfig) -> Result<Box<dyn SandboxInstance>> {
        let id = uuid::Uuid::new_v4().to_string();
        let socket_path = config.socket_dir.join(format!("fc-{}.sock", id));
        
        // Create Firecracker config
        let fc_config = serde_json::json!({
            "boot-source": {
                "kernel_image_path": config.kernel_path,
                "boot_args": config.boot_args
            },
            "drives": [{
                "drive_id": "rootfs",
                "path_on_host": config.rootfs_path,
                "is_root_device": true,
                "is_read_only": false
            }],
            "machine-config": {
                "vcpu_count": config.vcpus,
                "mem_size_mib": config.memory_mib,
                "ht_enabled": false
            },
            "vsock": {
                "vsock_id": "vsock0",
                "guest_cid": 3
            }
        });
        
        // Write config file
        let config_path = socket_path.with_extension("json");
        tokio::fs::write(&config_path, serde_json::to_string_pretty(&fc_config)?).await?;
        
        // Start Firecracker
        let mut child = TokioCommand::new("firecracker")
            .args(["--api-sock", socket_path.to_str().unwrap(), "--config-file", config_path.to_str().unwrap()])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        
        // Wait for socket to be ready
        tokio::time::sleep(Duration::from_millis(100)).await;
        
        Ok(Box::new(Self {
            id,
            config: config.clone(),
            process: Arc::new(Mutex::new(Some(child))),
            socket_path,
        }))
    }
}

#[async_trait::async_trait]
impl SandboxInstance for FirecrackerSandbox {
    async fn prepare(&self, _generation: &crate::nix::GenerationResult) -> Result<()> {
        // Update kernel/initrd in sandbox via vsock or rebuild
        // For now, assume rootfs has the new generation
        Ok(())
    }
    
    async fn exec(&self, cmd: &str) -> Result<SandboxOutput> {
        // Execute command via vsock/SSH
        // Placeholder - would use firecracker's vsock or SSH
        let output = Command::new("ssh")
            .args(["-o", "StrictHostKeyChecking=no", "root@localhost", cmd])
            .output()?;
        
        Ok(SandboxOutput {
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }
    
    async fn exec_bg(&self, cmd: &str) -> Result<WorkloadHandle> {
        let output = self.exec(&format!("nohup {} > /dev/null 2>&1 & echo $!", cmd)).await?;
        let pid = output.stdout.trim().parse().unwrap_or(0);
        Ok(WorkloadHandle { pid })
    }
    
    async fn kill_process(&self, pid: u32) -> Result<()> {
        self.exec(&format!("kill {}", pid)).await?;
        Ok(())
    }
    
    async fn is_healthy(&self) -> bool {
        self.exec("echo healthcheck").await.map(|o| o.status.success()).unwrap_or(false)
    }
    
    async fn destroy(&self) -> Result<()> {
        let mut process_guard = self.process.lock().await;
        if let Some(mut child) = process_guard.take() {
            let _ = child.kill().await;
        }
        let _ = tokio::fs::remove_file(&self.socket_path).await;
        let _ = tokio::fs::remove_file(self.socket_path.with_extension("json")).await;
        Ok(())
    }
    
    fn id(&self) -> &str {
        &self.id
    }
}

/// QEMU sandbox (fallback)
pub struct QemuSandbox {
    id: String,
    config: QemuConfig,
    process: Arc<Mutex<Option<tokio::process::Child>>>,
}

impl QemuSandbox {
    pub async fn new(config: &QemuConfig) -> Result<Box<dyn SandboxInstance>> {
        let id = uuid::Uuid::new_v4().to_string();
        
        let mut args = vec![
            "-kernel", config.kernel_path.to_str().unwrap(),
            "-initrd", config.initrd_path.to_str().unwrap(),
            "-drive", &format!("file={},format=qcow2", config.disk_path.display()),
            "-m", &config.memory_mib.to_string(),
            "-smp", &config.vcpus.to_string(),
            "-nographic",
            "-serial", "stdio",
        ];
        
        if config.enable_kvm {
            args.extend(["-enable-kvm", "-cpu", "host"]);
        }
        
        let mut child = TokioCommand::new("qemu-system-x86_64")
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        
        // Wait for boot
        tokio::time::sleep(Duration::from_secs(10)).await;
        
        Ok(Box::new(Self {
            id,
            config: config.clone(),
            process: Arc::new(Mutex::new(Some(child))),
        }))
    }
}

#[async_trait::async_trait]
impl SandboxInstance for QemuSandbox {
    async fn prepare(&self, _generation: &crate::nix::GenerationResult) -> Result<()> {
        Ok(())
    }
    
    async fn exec(&self, cmd: &str) -> Result<SandboxOutput> {
        let output = Command::new("ssh")
            .args(["-o", "StrictHostKeyChecking=no", "root@localhost", cmd])
            .output()?;
        
        Ok(SandboxOutput {
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }
    
    async fn exec_bg(&self, cmd: &str) -> Result<WorkloadHandle> {
        let output = self.exec(&format!("nohup {} > /dev/null 2>&1 & echo $!", cmd)).await?;
        let pid = output.stdout.trim().parse().unwrap_or(0);
        Ok(WorkloadHandle { pid })
    }
    
    async fn kill_process(&self, pid: u32) -> Result<()> {
        self.exec(&format!("kill {}", pid)).await?;
        Ok(())
    }
    
    async fn is_healthy(&self) -> bool {
        self.exec("echo healthcheck").await.map(|o| o.status.success()).unwrap_or(false)
    }
    
    async fn destroy(&self) -> Result<()> {
        let mut process_guard = self.process.lock().await;
        if let Some(mut child) = process_guard.take() {
            let _ = child.kill().await;
        }
        Ok(())
    }
    
    fn id(&self) -> &str {
        &self.id
    }
}

/// Test result
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub duration: Duration,
    pub output: String,
    pub error: Option<String>,
}

/// Benchmark result
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BenchmarkResult {
    pub name: String,
    pub duration: Duration,
    pub metrics: std::collections::HashMap<String, f64>,
    pub passed: bool,
    pub output: String,
}

/// All test results
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TestResults {
    pub unit_tests: Vec<TestResult>,
    pub integration_tests: Vec<TestResult>,
    pub benchmarks: Vec<BenchmarkResult>,
    pub total_duration: Duration,
    pub passed: bool,
}

/// Canary metrics
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CanaryMetrics {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub cpu_usage: f64,
    pub memory_usage: f64,
    pub load_avg: (f64, f64, f64),
}

/// Canary analysis
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CanaryAnalysis {
    pub all_passed: bool,
    pub violations: Vec<String>,
    pub cpu_avg: f64,
    pub mem_avg: f64,
    pub sample_count: usize,
}

/// Canary results
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CanaryResults {
    pub metrics: Vec<CanaryMetrics>,
    pub analysis: CanaryAnalysis,
    pub passed: bool,
}

/// Workload handle
#[derive(Debug, Clone)]
pub struct WorkloadHandle {
    pub pid: u32,
}

/// Sandbox command output
#[derive(Debug, Clone)]
pub struct SandboxOutput {
    pub status: std::process::ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

/// Success criteria for validation
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SuccessCriteria {
    pub statistical_significance: f64,
    pub min_improvement_pct: f64,
    pub max_cpu_pct: Option<f64>,
    pub max_memory_pct: Option<f64>,
    pub max_latency_pct: Option<f64>,
}

fn parse_benchmark_output(output: &str) -> std::collections::HashMap<String, f64> {
    let mut metrics = std::collections::HashMap::new();
    // Parse benchmark output (format depends on benchmark tool)
    for line in output.lines() {
        if let Some((key, value)) = line.split_once(':') {
            if let Ok(v) = value.trim().parse::<f64>() {
                metrics.insert(key.trim().to_string(), v);
            }
        }
    }
    metrics
}

fn parse_cpu_usage(output: &str) -> f64 {
    // Parse /proc/stat
    for line in output.lines() {
        if line.starts_with("cpu ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 8 {
                let user: f64 = parts[1].parse().unwrap_or(0.0);
                let nice: f64 = parts[2].parse().unwrap_or(0.0);
                let system: f64 = parts[3].parse().unwrap_or(0.0);
                let idle: f64 = parts[4].parse().unwrap_or(0.0);
                let total = user + nice + system + idle;
                if total > 0.0 {
                    return ((user + nice + system) / total) * 100.0;
                }
            }
        }
    }
    0.0
}

fn parse_memory_usage(output: &str) -> f64 {
    // Parse /proc/meminfo
    let mut total = 0.0;
    let mut available = 0.0;
    for line in output.lines() {
        if line.starts_with("MemTotal:") {
            total = line.split_whitespace().nth(1).unwrap_or("0").parse().unwrap_or(0.0);
        } else if line.starts_with("MemAvailable:") {
            available = line.split_whitespace().nth(1).unwrap_or("0").parse().unwrap_or(0.0);
        }
    }
    if total > 0.0 {
        ((total - available) / total) * 100.0
    } else {
        0.0
    }
}

fn parse_load_avg(output: &str) -> (f64, f64, f64) {
    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 {
            return (
                parts[0].parse().unwrap_or(0.0),
                parts[1].parse().unwrap_or(0.0),
                parts[2].parse().unwrap_or(0.0),
            );
        }
    }
    (0.0, 0.0, 0.0)
}