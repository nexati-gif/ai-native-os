//! Configuration loading and management

use std::path::Path;
use config::{Config, File, Environment};
use serde::de::DeserializeOwned;
use crate::{Error, Result, OrchestratorConfig};

pub fn load_config<T: DeserializeOwned>(config_path: &Path) -> Result<T> {
    let mut builder = Config::builder()
        .add_source(File::from(config_path).required(true))
        .add_source(Environment::with_prefix("ORCHESTRATOR").separator("_"));

    let config = builder.build()?;
    Ok(config.try_deserialize()?)
}

pub fn load_orchestrator_config(config_path: &Path) -> Result<OrchestratorConfig> {
    load_config(config_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use std::io::Write;

    #[test]
    fn test_load_config() {
        let mut tmp = NamedTempFile::new().unwrap();
        writeln!(tmp, r#"
            device_id = "test-device"
            model_path = "/models/qwen2.5-coder-32b-4bit"
            rag_path = "/var/lib/orchestrator/rag"
            lora_path = "/var/lib/orchestrator/lora"
            state_db_path = "/var/lib/orchestrator/state.redb"
            idle_timeout_secs = 30
            passive_mode = {{ enabled = true, poll_interval_ms = 100, events_buffer_size = 10000, persist_interval_secs = 60, collectors = [] }}
            active_mode = {{ max_concurrent_cycles = 1, proposal_timeout_secs = 300, sandbox_timeout_secs = 600, validation_timeout_secs = 300, auto_promote_on_success = false }}
            federation = {{ enabled = true, tailscale_auth_key = null, sync_interval_secs = 300, automerge_doc_path = "/var/lib/orchestrator/federation.automerge", git_remote = null, git_branch = "main" }}
            ui = {{ mode = "Both", tui_enabled = true, web_enabled = true, web_port = 8080, web_bind = "127.0.0.1", auth_token = null }}
            policy = {{ engine = "OpaRego", policies_dir = "/etc/orchestrator/policies", default_deny = true }}
            sandbox = {{ backend = "Firecracker", firecracker = {{ vcpus = 2, memory_mib = 512, kernel_path = "/boot/vmlinuz", initrd_path = "/boot/initrd", rootfs_path = "/var/lib/orchestrator/rootfs.ext4", boot_args = "console=ttyS0" }}, qemu = {{ vcpus = 2, memory_mib = 1024, kernel_path = "/boot/vmlinuz", initrd_path = "/boot/initrd", disk_path = "/var/lib/orchestrator/disk.qcow2", enable_kvm = true }}, warm_pool_size = 0 }}
        "#).unwrap();
        
        let config: OrchestratorConfig = load_orchestrator_config(tmp.path()).unwrap();
        assert_eq!(config.device_id.0, "test-device");
        assert_eq!(config.idle_timeout_secs, 30);
    }
}