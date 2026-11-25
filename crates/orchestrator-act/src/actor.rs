//! Actor: coordinates proposal execution in sandbox

use std::sync::Arc;
use std::time::Instant;
use tracing::{info, debug, warn, error};
use crate::{Actor, ActorConfig, ExecutionResult, SandboxBackend, Sandbox, NixBuilder, KexecManager};
use orchestrator_common::{Error, Result, EventBus, MetricsRecorder};
use orchestrator_common::types::{Proposal, SandboxConfig, FirecrackerConfig, QemuConfig};

/// Actor for executing proposals
pub struct Actor {
    config: ActorConfig,
    nix_builder: NixBuilder,
    firecracker: Option<FirecrackerSandbox>,
    qemu: Option<QemuSandbox>,
    kexec: KexecManager,
    event_bus: EventBus,
    metrics: MetricsRecorder,
}

#[derive(Debug, Clone)]
pub struct ActorConfig {
    pub sandbox: SandboxConfig,
    pub nix_config_path: String,
    pub generations_dir: String,
}

impl Actor {
    pub async fn new(sandbox_config: &SandboxConfig, event_bus: EventBus) -> Result<Self> {
        let config = ActorConfig {
            sandbox: sandbox_config.clone(),
            nix_config_path: "/etc/nixos".to_string(),
            generations_dir: "/nix/var/nix/profiles/system".to_string(),
        };

        let nix_builder = NixBuilder::new(&config.nix_config_path).await?;
        let kexec = KexecManager::new().await?;

        // Initialize sandbox backends lazily
        let firecracker = if matches!(sandbox_config.backend, SandboxBackend::Firecracker | SandboxBackend::Both) {
            Some(FirecrackerSandbox::new(&sandbox_config.firecracker).await?)
        } else {
            None
        };

        let qemu = if matches!(sandbox_config.backend, SandboxBackend::Qemu | SandboxBackend::Both) {
            Some(QemuSandbox::new(&sandbox_config.qemu).await?)
        } else {
            None
        };

        Ok(Self {
            config,
            nix_builder,
            firecracker,
            qemu,
            kexec,
            event_bus,
            metrics: MetricsRecorder::new("actor"),
        })
    }

    /// Execute proposal in sandbox
    pub async fn execute_in_sandbox(&self, proposal: &Proposal) -> Result<ExecutionResult> {
        let start = Instant::now();
        info!(%proposal.id, "Executing proposal in sandbox");

        // 1. Build NixOS configuration with proposed changes
        let generation = self.nix_builder.build_with_proposal(proposal).await?;

        // 2. Select sandbox backend
        let sandbox = self.select_sandbox().await?;

        // 3. Boot sandbox with new generation
        let sandbox_result = sandbox.boot(&generation).await?;

        // 4. Run tests inside sandbox
        let test_results = sandbox.run_tests(&proposal.test_plan).await?;

        // 5. Run benchmarks
        let benchmark_results = sandbox.run_benchmarks(&proposal.test_plan.benchmarks).await?;

        let duration = start.elapsed();
        self.metrics.timing("sandbox_execution_time", duration);

        Ok(ExecutionResult {
            success: sandbox_result.success && test_results.iter().all(|t| t.passed),
            generation,
            sandbox_result,
            test_results,
            benchmark_results,
            duration,
        })
    }

    async fn select_sandbox(&self) -> Result<&dyn Sandbox> {
        match self.config.sandbox.backend {
            SandboxBackend::Firecracker => {
                self.firecracker.as_ref().ok_or_else(|| Error::Sandbox("Firecracker not initialized".to_string()))
            }
            SandboxBackend::Qemu => {
                self.qemu.as_ref().ok_or_else(|| Error::Sandbox("QEMU not initialized".to_string()))
            }
            SandboxBackend::Both => {
                // Prefer Firecracker for speed
                self.firecracker.as_ref()
                    .ok_or_else(|| Error::Sandbox("No sandbox available".to_string()))
            }
        }
    }

    /// Promote generation to production (atomic switch)
    pub async fn promote_generation(&self, generation: &NixOSGeneration) -> Result<()> {
        info!("Promoting generation {} to production", generation.id);
        self.nix_builder.switch_generation(generation).await?;
        self.kexec.load_kernel(&generation.kernel_path).await?;
        Ok(())
    }

    /// Rollback to previous generation
    pub async fn rollback(&self) -> Result<()> {
        warn!("Rolling back to previous generation");
        self.nix_builder.rollback().await?;
        Ok(())
    }

    pub async fn shutdown(&self) {
        // Cleanup sandbox processes
    }
}

/// Sandbox execution result
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub success: bool,
    pub generation: NixOSGeneration,
    pub sandbox_result: SandboxResult,
    pub test_results: Vec<orchestrator_common::types::TestResult>,
    pub benchmark_results: Vec<orchestrator_common::types::BenchmarkResult>,
    pub duration: std::time::Duration,
}

/// Sandbox boot result
#[derive(Debug, Clone)]
pub struct SandboxResult {
    pub success: bool,
    pub boot_time_ms: u64,
    pub memory_used_mib: u64,
    pub error: Option<String>,
}

/// NixOS generation
#[derive(Debug, Clone)]
pub struct NixOSGeneration {
    pub id: u64,
    pub kernel_path: String,
    pub initrd_path: String,
    pub config_hash: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}