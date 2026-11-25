//! Kexec manager for fast kernel switching

use std::path::Path;
use tokio::process::Command;
use tracing::{info, debug, warn};
use crate::{KexecManager, BootEntry};
use orchestrator_common::{Error, Result, utils::run_command};

/// Kexec manager for atomic kernel switching without full reboot
pub struct KexecManager {
    current_kernel: Option<String>,
}

impl KexecManager {
    pub async fn new() -> Result<Self> {
        Ok(Self { current_kernel: None })
    }

    /// Load kernel for next boot (kexec -l)
    pub async fn load_kernel(&self, kernel_path: &str) -> Result<()> {
        info!("Loading kernel via kexec: {}", kernel_path);
        
        // Find initrd
        let initrd_path = kernel_path.replace("kernel", "initrd");
        if !Path::new(&initrd_path).exists() {
            return Err(Error::Sandbox(format!("Initrd not found: {}", initrd_path)));
        }

        // Get kernel command line
        let cmdline = tokio::fs::read_to_string("/proc/cmdline").await
            .unwrap_or_else(|_| "console=ttyS0".to_string());

        let output = run_command(
            "kexec",
            &["-l", kernel_path, "--initrd", &initrd_path, "--command-line", &cmdline],
            std::time::Duration::from_secs(30),
            None,
        ).await?;

        output.ensure_success()?;
        info!("Kernel loaded for kexec");
        Ok(())
    }

    /// Execute kexec (kexec -e) - switches kernel immediately
    pub async fn execute_kexec(&self) -> Result<()> {
        warn!("Executing kexec - system will switch kernels now");
        
        let output = run_command(
            "kexec",
            &["-e"],
            std::time::Duration::from_secs(10),
            None,
        ).await?;

        output.ensure_success()?;
        Ok(())
    }

    /// Check if kexec is available
    pub async fn is_available(&self) -> bool {
        Command::new("kexec")
            .arg("--version")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Get current boot entry
    pub async fn current_boot_entry(&self) -> Result<BootEntry> {
        let cmdline = tokio::fs::read_to_string("/proc/cmdline").await
            .unwrap_or_default();
        
        Ok(BootEntry {
            kernel: "current".to_string(),
            initrd: "current".to_string(),
            cmdline,
            is_kexec: false,
        })
    }
}

/// Boot entry for kexec
#[derive(Debug, Clone)]
pub struct BootEntry {
    pub kernel: String,
    pub initrd: String,
    pub cmdline: String,
    pub is_kexec: bool,
}