//! NixOS configuration builder and manager

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn, debug, error};
use orchestrator_common::{Error, Result, MetricsRecorder};
use orchestrator_common::types::{Proposal, ProposalKind, DeviceId};

/// NixOS builder for generating and applying configurations
pub struct NixBuilder {
    config_dir: PathBuf,
    flake_dir: PathBuf,
    device_id: DeviceId,
    metrics: MetricsRecorder,
    build_lock: Arc<Mutex<()>>,
}

impl NixBuilder {
    pub fn new(config_dir: PathBuf, flake_dir: PathBuf, device_id: DeviceId) -> Self {
        Self {
            config_dir,
            flake_dir,
            device_id,
            metrics: MetricsRecorder::new("nix_builder"),
            build_lock: Arc::new(Mutex::new(())),
        }
    }
    
    /// Build a new NixOS generation with the proposed changes
    pub async fn build_generation(&self, proposal: &Proposal) -> Result<GenerationResult> {
        let _guard = self.build_lock.lock().await;
        
        info!("Building NixOS generation for proposal: {}", proposal.id);
        let start = std::time::Instant::now();
        
        // 1. Apply proposal changes to configuration
        self.apply_proposal(proposal).await?;
        
        // 2. Build the new generation
        let generation = self.nixos_build().await?;
        
        // 3. Run basic validation
        self.validate_generation(&generation).await?;
        
        let duration = start.elapsed();
        self.metrics.timing("build_duration", duration);
        self.metrics.counter("builds", 1);
        
        info!("Generation built in {:?}: {}", duration, generation.generation_id);
        
        Ok(generation)
    }
    
    /// Apply proposal changes to NixOS configuration
    async fn apply_proposal(&self, proposal: &Proposal) -> Result<()> {
        match proposal.kind {
            ProposalKind::NixConfig => self.apply_nix_config(proposal).await,
            ProposalKind::KernelConfig => self.apply_kernel_config(proposal).await,
            ProposalKind::SysctlTuning => self.apply_sysctl_config(proposal).await,
            ProposalKind::ServiceConfig => self.apply_service_config(proposal).await,
            ProposalKind::ModuleConfig => self.apply_module_config(proposal).await,
            _ => Err(Error::Config(format!("Unsupported proposal kind for Nix: {:?}", proposal.kind))),
        }
    }
    
    /// Apply Nix configuration changes
    async fn apply_nix_config(&self, proposal: &Proposal) -> Result<()> {
        // Parse the diff and apply to configuration.nix or host config
        let config_file = self.config_dir.join("hosts").join(format!("{}.nix", self.device_id));
        
        if !config_file.exists() {
            // Try common locations
            let alt = self.config_dir.join("configuration.nix");
            if alt.exists() {
                return self.apply_diff_to_file(&alt, &proposal.diff).await;
            }
            return Err(Error::Config("No Nix configuration file found".to_string()));
        }
        
        self.apply_diff_to_file(&config_file, &proposal.diff).await
    }
    
    /// Apply kernel configuration changes
    async fn apply_kernel_config(&self, proposal: &Proposal) -> Result<()> {
        // Kernel config changes go to kernel.nix module
        let kernel_file = self.config_dir.join("modules").join("kernel.nix");
        self.apply_diff_to_file(&kernel_file, &proposal.diff).await
    }
    
    /// Apply sysctl configuration changes
    async fn apply_sysctl_config(&self, proposal: &Proposal) -> Result<()> {
        // Sysctl changes go to a dedicated sysctl.nix or host config
        let sysctl_file = self.config_dir.join("modules").join("sysctl.nix");
        if sysctl_file.exists() {
            self.apply_diff_to_file(&sysctl_file, &proposal.diff).await
        } else {
            // Apply to host config
            let config_file = self.config_dir.join("hosts").join(format!("{}.nix", self.device_id));
            self.apply_diff_to_file(&config_file, &proposal.diff).await
        }
    }
    
    /// Apply service configuration changes
    async fn apply_service_config(&self, proposal: &Proposal) -> Result<()> {
        let config_file = self.config_dir.join("hosts").join(format!("{}.nix", self.device_id));
        self.apply_diff_to_file(&config_file, &proposal.diff).await
    }
    
    /// Apply kernel module configuration changes
    async fn apply_module_config(&self, proposal: &Proposal) -> Result<()> {
        let module_file = self.config_dir.join("modules").join("kernel-modules.nix");
        if module_file.exists() {
            self.apply_diff_to_file(&module_file, &proposal.diff).await
        } else {
            let config_file = self.config_dir.join("hosts").join(format!("{}.nix", self.device_id));
            self.apply_diff_to_file(&config_file, &proposal.diff).await
        }
    }
    
    /// Apply a unified diff to a file
    async fn apply_diff_to_file(&self, file: &Path, diff: &str) -> Result<()> {
        // For simplicity, we'll use the `patch` command
        // In production, would use a proper diff library
        
        // Create backup
        let backup = file.with_extension("nix.bak");
        tokio::fs::copy(file, &backup).await?;
        
        // Write diff to temp file
        let diff_file = file.with_extension("patch");
        tokio::fs::write(&diff_file, diff).await?;
        
        // Apply patch
        let output = Command::new("patch")
            .args(["-p1", "-i", diff_file.to_str().unwrap()])
            .current_dir(file.parent().unwrap())
            .output()?;
        
        if !output.status.success() {
            // Restore backup
            tokio::fs::copy(&backup, file).await?;
            return Err(Error::Config(format!("Patch failed: {}", String::from_utf8_lossy(&output.stderr))));
        }
        
        // Clean up
        let _ = tokio::fs::remove_file(&diff_file).await;
        let _ = tokio::fs::remove_file(&backup).await;
        
        Ok(())
    }
    
    /// Build NixOS configuration
    async fn nixos_build(&self) -> Result<GenerationResult> {
        let output = Command::new("nix")
            .args([
                "build",
                ".#nixosConfigurations.laptop.config.system.build.toplevel",
                "--print-out-paths",
                "--no-link"
            ])
            .current_dir(&self.flake_dir)
            .output()?;
        
        if !output.status.success() {
            return Err(Error::Config(format!("Nix build failed: {}", String::from_utf8_lossy(&output.stderr))));
        }
        
        let store_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        
        // Get generation ID
        let gen_output = Command::new("nixos-rebuild")
            .args(["list-generations"])
            .output()?;
        
        let generation_id = parse_latest_generation(&String::from_utf8_lossy(&gen_output.stdout))?;
        
        Ok(GenerationResult {
            generation_id,
            store_path,
            kernel_path: format!("{}/kernel", store_path),
            initrd_path: format!("{}/initrd", store_path),
        })
    }
    
    /// Validate the built generation
    async fn validate_generation(&self, generation: &GenerationResult) -> Result<()> {
        // Check that kernel and initrd exist
        if !Path::new(&generation.kernel_path).exists() {
            return Err(Error::Config("Kernel not found in generation".to_string()));
        }
        if !Path::new(&generation.initrd_path).exists() {
            return Err(Error::Config("Initrd not found in generation".to_string()));
        }
        
        // Could run additional validation like nixos-rebuild dry-build
        Ok(())
    }
    
    /// Switch to a generation (for rollback)
    pub async fn switch_generation(&self, generation_id: u64) -> Result<()> {
        let output = Command::new("nixos-rebuild")
            .args(["switch", "--generation", &generation_id.to_string()])
            .output()?;
        
        if !output.status.success() {
            return Err(Error::Config(format!("Generation switch failed: {}", String::from_utf8_lossy(&output.stderr))));
        }
        
        Ok(())
    }
    
    /// Rollback to previous generation
    pub async fn rollback(&self) -> Result<()> {
        let output = Command::new("nixos-rebuild")
            .args(["switch", "--rollback"])
            .output()?;
        
        if !output.status.success() {
            return Err(Error::Config(format!("Rollback failed: {}", String::from_utf8_lossy(&output.stderr))));
        }
        
        Ok(())
    }
    
    /// Get current generation info
    pub async fn current_generation(&self) -> Result<GenerationInfo> {
        let output = Command::new("nixos-rebuild")
            .args(["list-generations"])
            .output()?;
        
        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_current_generation(&stdout)
    }
    
    /// Build UKI (Unified Kernel Image)
    pub async fn build_uki(&self, generation: &GenerationResult) -> Result<PathBuf> {
        // Build UKI using systemd-uki or manual objcopy
        let uki_path = PathBuf::from(&generation.store_path).join("uki.efi");
        
        let output = Command::new("objcopy")
            .args([
                "--add-section", &format!(".osrel=/etc/os-release"),
                "--change-section-vma", ".osrel=0x20000",
                "--add-section", &format!(".cmdline=/etc/kernel/cmdline"),
                "--change-section-vma", ".cmdline=0x30000",
                "--add-section", &format!(".linux={}", &generation.kernel_path),
                "--change-section-vma", ".linux=0x40000",
                "--add-section", &format!(".initrd={}", &generation.initrd_path),
                "--change-section-vma", ".initrd=0x3000000",
                "/usr/lib/systemd/boot/efi/linuxx64.efi.stub",
                uki_path.to_str().unwrap()
            ])
            .output()?;
        
        if !output.status.success() {
            return Err(Error::Config(format!("UKI build failed: {}", String::from_utf8_lossy(&output.stderr))));
        }
        
        Ok(uki_path)
    }
}

#[derive(Debug, Clone)]
pub struct GenerationResult {
    pub generation_id: u64,
    pub store_path: String,
    pub kernel_path: String,
    pub initrd_path: String,
}

#[derive(Debug, Clone)]
pub struct GenerationInfo {
    pub generation_id: u64,
    pub date: String,
    pub kernel_version: String,
    pub is_current: bool,
}

fn parse_latest_generation(output: &str) -> Result<u64> {
    for line in output.lines().rev() {
        if let Some(id_str) = line.split_whitespace().next() {
            if let Ok(id) = id_str.parse::<u64>() {
                return Ok(id);
            }
        }
    }
    Err(Error::Config("Could not parse generation ID".to_string()))
}

fn parse_current_generation(output: &str) -> Result<GenerationInfo> {
    for line in output.lines() {
        if line.contains("current") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let gen_id = parts[0].parse().unwrap_or(0);
                return Ok(GenerationInfo {
                    generation_id: gen_id,
                    date: parts[1..].join(" "),
                    kernel_version: String::new(),
                    is_current: true,
                });
            }
        }
    }
    Err(Error::Config("Could not find current generation".to_string()))
}

/// Nix flake utilities
pub mod flake {
    use super::*;
    
    /// Update flake inputs
    pub async fn update_flake(flake_dir: &Path) -> Result<()> {
        let output = Command::new("nix")
            .args(["flake", "update"])
            .current_dir(flake_dir)
            .output()?;
        
        if !output.status.success() {
            return Err(Error::Config(format!("Flake update failed: {}", String::from_utf8_lossy(&output.stderr))));
        }
        Ok(())
    }
    
    /// Check flake
    pub async fn check_flake(flake_dir: &Path) -> Result<()> {
        let output = Command::new("nix")
            .args(["flake", "check"])
            .current_dir(flake_dir)
            .output()?;
        
        if !output.status.success() {
            return Err(Error::Config(format!("Flake check failed: {}", String::from_utf8_lossy(&output.stderr))));
        }
        Ok(())
    }
    
    /// Get flake metadata
    pub async fn flake_metadata(flake_dir: &Path) -> Result<serde_json::Value> {
        let output = Command::new("nix")
            .args(["flake", "metadata", "--json"])
            .current_dir(flake_dir)
            .output()?;
        
        if !output.status.success() {
            return Err(Error::Config(format!("Flake metadata failed: {}", String::from_utf8_lossy(&output.stderr))));
        }
        
        Ok(serde_json::from_slice(&output.stdout)?)
    }
}