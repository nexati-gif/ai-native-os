//! Tailscale client for mesh VPN

use std::collections::HashMap;
use tokio::process::Command;
use orchestrator_common::{Error, Result, utils::run_command};
use orchestrator_common::types::DeviceId;

/// Tailscale client for device mesh
pub struct TailscaleClient {
    auth_key: Option<String>,
    local_device_id: DeviceId,
}

impl TailscaleClient {
    pub async fn new(auth_key: Option<&str>) -> Result<Self> {
        let local_device_id = DeviceId::new(&hostname::get()?.to_string_lossy());
        
        let client = Self {
            auth_key: auth_key.map(|s| s.to_string()),
            local_device_id,
        };

        // Ensure tailscaled is running
        client.ensure_tailscaled().await?;

        // Authenticate if key provided
        if client.auth_key.is_some() {
            client.authenticate().await?;
        }

        Ok(client)
    }

    async fn ensure_tailscaled(&self) -> Result<()> {
        let output = run_command("systemctl", &["is-active", "tailscaled"], std::time::Duration::from_secs(5), None).await?;
        if !output.success() {
            run_command("systemctl", &["start", "tailscaled"], std::time::Duration::from_secs(10), None).await?;
        }
        Ok(())
    }

    async fn authenticate(&self) -> Result<()> {
        if let Some(key) = &self.auth_key {
            let output = run_command("tailscale", &["up", "--auth-key", key, "--accept-routes"], std::time::Duration::from_secs(30), None).await?;
            output.ensure_success()?;
        }
        Ok(())
    }

    /// Get list of peer devices
    pub async fn get_peers(&self) -> Result<Vec<PeerInfo>> {
        let output = run_command("tailscale", &["status", "--json"], std::time::Duration::from_secs(10), None).await?;
        
        let status: TailscaleStatus = serde_json::from_str(&output.stdout)?;
        
        let peers = status.peer.iter()
            .map(|(_, peer)| PeerInfo {
                device_id: DeviceId::new(&peer.hostname),
                tailscale_ip: peer.tailscale_ips.first().cloned().unwrap_or_default(),
                online: peer.online,
                last_seen: peer.last_seen,
            })
            .collect();

        Ok(peers)
    }

    /// Announce sync availability to peers
    pub async fn announce_sync(&self, device_id: &DeviceId) -> Result<()> {
        // In real implementation, would use Tailscale's funnel or custom protocol
        // For now, just log
        tracing::debug!("Announcing sync from {}", device_id.0);
        Ok(())
    }

    /// Send data to peer
    pub async fn send_to_peer(&self, peer_id: &DeviceId, data: &[u8]) -> Result<()> {
        // Would use direct connection over Tailscale mesh
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub device_id: DeviceId,
    pub tailscale_ip: String,
    pub online: bool,
    pub last_seen: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct TailscaleStatus {
    peer: HashMap<String, TailscalePeer>,
}

#[derive(Debug, serde::Deserialize)]
struct TailscalePeer {
    hostname: String,
    tailscale_ips: Vec<String>,
    online: bool,
    last_seen: Option<String>,
}