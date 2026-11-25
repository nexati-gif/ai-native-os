//! Federation: sync state across devices via Tailscale + Automerge + Git

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{info, warn, debug, error};
use orchestrator_common::{Error, Result, MetricsRecorder, EventBus, Event};
use orchestrator_common::types::{DeviceId, Proposal, LoRAAdapter, PolicyUpdate};
use automerge::{Automerge, ObjId, ROOT, Value};
use git2::{Repository, Cred, RemoteCallbacks, PushOptions, FetchOptions, Signature};

/// Federator for multi-device sync
pub struct Federator {
    config: FederatorConfig,
    metrics: MetricsRecorder,
    event_bus: EventBus,
    device_id: DeviceId,
    automerge_doc: Arc<Mutex<Automerge>>,
    git_repo: Arc<Mutex<Option<Repository>>>,
    tailscale_client: Arc<Mutex<Option<TailscaleClient>>>,
    sync_lock: Arc<Mutex<()>>,
}

#[derive(Debug, Clone)]
pub struct FederatorConfig {
    pub enabled: bool,
    pub tailscale_auth_key: Option<String>,
    pub sync_interval_secs: u64,
    pub automerge_doc_path: PathBuf,
    pub git_remote: Option<String>,
    pub git_branch: String,
    pub peer_devices: Vec<DeviceId>,
}

impl Federator {
    pub async fn new(config: FederatorConfig, event_bus: EventBus, device_id: DeviceId) -> Result<Self> {
        // Load or create Automerge document
        let automerge_doc = if config.automerge_doc_path.exists() {
            let data = tokio::fs::read(&config.automerge_doc_path).await?;
            Automerge::load(&data)?
        } else {
            Automerge::new()
        };
        
        // Initialize git repo
        let git_repo = if let Some(remote) = &config.git_remote {
            let repo = Self::init_git_repo(&config.automerge_doc_path.parent().unwrap(), remote).await?;
            Some(repo)
        } else {
            None
        };
        
        // Initialize Tailscale
        let tailscale_client = if config.enabled && config.tailscale_auth_key.is_some() {
            Some(TailscaleClient::new(config.tailscale_auth_key.as_ref().unwrap()).await?)
        } else {
            None
        };
        
        let federator = Self {
            config,
            metrics: MetricsRecorder::new("federator"),
            event_bus,
            device_id,
            automerge_doc: Arc::new(Mutex::new(automerge_doc)),
            git_repo: Arc::new(Mutex::new(git_repo)),
            tailscale_client: Arc::new(Mutex::new(tailscale_client)),
            sync_lock: Arc::new(Mutex::new(())),
        };
        
        // Start sync loop
        federator.start_sync_loop().await;
        
        Ok(federator)
    }
    
    async fn init_git_repo(local_path: &PathBuf, remote: &str) -> Result<Repository> {
        let repo = if local_path.join(".git").exists() {
            Repository::open(local_path)?
        } else {
            Repository::init(local_path)?
        };
        
        // Add remote if not exists
        let remotes = repo.remotes()?;
        if !remotes.iter().any(|r| r == Some("origin")) {
            repo.remote("origin", remote)?;
        }
        
        Ok(repo)
    }
    
    /// Start background sync loop
    async fn start_sync_loop(&self) {
        let sync_interval = Duration::from_secs(self.config.sync_interval_secs);
        let federator = self.clone_for_sync();
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(sync_interval);
            loop {
                interval.tick().await;
                if let Err(e) = federator.sync().await {
                    warn!("Federation sync failed: {}", e);
                }
            }
        });
    }
    
    fn clone_for_sync(&self) -> Self {
        Self {
            config: self.config.clone(),
            metrics: self.metrics.clone(),
            event_bus: self.event_bus.clone(),
            device_id: self.device_id.clone(),
            automerge_doc: self.automerge_doc.clone(),
            git_repo: self.git_repo.clone(),
            tailscale_client: self.tailscale_client.clone(),
            sync_lock: self.sync_lock.clone(),
        }
    }
    
    /// Perform full sync
    pub async fn sync(&self) -> Result<()> {
        let _guard = self.sync_lock.lock().await;
        
        info!("Starting federation sync");
        let start = std::time::Instant::now();
        
        // 1. Sync via Tailscale (peer-to-peer)
        if self.config.enabled {
            self.sync_peers().await?;
        }
        
        // 2. Sync via Git (history)
        if self.config.git_remote.is_some() {
            self.sync_git().await?;
        }
        
        // 3. Persist Automerge doc
        self.persist_automerge().await?;
        
        let duration = start.elapsed();
        self.metrics.timing("sync_duration", duration);
        self.metrics.counter("syncs", 1);
        
        info!("Federation sync completed in {:?}", duration);
        Ok(())
    }
    
    /// Sync with peer devices via Tailscale
    async fn sync_peers(&self) -> Result<()> {
        let tailscale = self.tailscale_client.lock().await;
        if let Some(client) = tailscale.as_ref() {
            let peers = client.get_peers().await?;
            
            for peer in peers {
                if self.config.peer_devices.contains(&peer.device_id) {
                    // Pull changes from peer
                    let their_doc = client.pull_document(&peer.device_id).await?;
                    self.merge_automerge(their_doc).await?;
                    
                    // Push our changes
                    let our_doc = self.automerge_doc.lock().await;
                    let changes = our_doc.get_changes(&[]);
                    client.push_document(&peer.device_id, &changes).await?;
                }
            }
        }
        Ok(())
    }
    
    /// Sync via Git
    async fn sync_git(&self) -> Result<()> {
        let mut repo_guard = self.git_repo.lock().await;
        if let Some(repo) = repo_guard.as_mut() {
            // Fetch
            let mut fetch_opts = FetchOptions::new();
            if let Some(auth_key) = &self.config.tailscale_auth_key {
                let mut callbacks = RemoteCallbacks::new();
                callbacks.credentials(|_url, _username, _allowed| {
                    Cred::ssh_key_from_agent("git")
                });
                fetch_opts.remote_callbacks(callbacks);
            }
            
            repo.find_remote("origin")?.fetch(&[self.config.git_branch.clone()], Some(&mut fetch_opts), None)?;
            
            // Merge
            let fetch_head = repo.find_reference("FETCH_HEAD")?.target().unwrap();
            let merge_result = repo.merge_analysis(&[repo.find_commit(fetch_head)?])?;
            
            if merge_result.0.is_fast_forward() {
                let mut reference = repo.find_reference(&format!("refs/heads/{}", self.config.git_branch))?;
                reference.set_target(fetch_head, "Fast-forward merge")?;
            } else if merge_result.0.is_normal() {
                // Normal merge - would need conflict resolution
                warn!("Normal merge required, manual intervention may be needed");
            }
            
            // Push local changes
            let mut push_opts = PushOptions::new();
            repo.find_remote("origin")?.push(&[format!("refs/heads/{}:refs/heads/{}", self.config.git_branch, self.config.git_branch)], Some(&mut push_opts))?;
        }
        Ok(())
    }
    
    /// Merge another Automerge document
    async fn merge_automerge(&self, other: Automerge) -> Result<()> {
        let mut doc = self.automerge_doc.lock().await;
        doc.merge(other)?;
        Ok(())
    }
    
    /// Persist Automerge document to disk
    async fn persist_automerge(&self) -> Result<()> {
        let doc = self.automerge_doc.lock().await;
        let data = doc.save();
        tokio::fs::write(&self.config.automerge_doc_path, data).await?;
        Ok(())
    }
    
    /// Add approved proposal to federation state
    pub async fn add_proposal(&self, proposal: &Proposal) -> Result<()> {
        let mut doc = self.automerge_doc.lock().await;
        
        let proposals_obj = doc.get_or_create_map(ROOT, "proposals")?;
        
        doc.put_object(&proposals_obj, &proposal.id, Value::Object(automerge::ObjType::Map))?;
        let proposal_obj = doc.get_object(&proposals_obj, &proposal.id)?;
        
        doc.put(&proposal_obj, "id", proposal.id.clone())?;
        doc.put(&proposal_obj, "kind", format!("{:?}", proposal.kind))?;
        doc.put(&proposal_obj, "title", proposal.title.clone())?;
        doc.put(&proposal_obj, "description", proposal.description.clone())?;
        doc.put(&proposal_obj, "rationale", proposal.rationale.clone())?;
        doc.put(&proposal_obj, "diff", proposal.diff.clone())?;
        doc.put(&proposal_obj, "risk_level", format!("{:?}", proposal.risk_level))?;
        doc.put(&proposal_obj, "timestamp", proposal.timestamp.to_rfc3339())?;
        doc.put(&proposal_obj, "device_id", proposal.device_id.0.clone())?;
        doc.put(&proposal_obj, "status", "approved")?;
        
        Ok(())
    }
    
    /// Add LoRA adapter to federation
    pub async fn add_lora_adapter(&self, adapter: &LoRAAdapter) -> Result<()> {
        let mut doc = self.automerge_doc.lock().await;
        
        let adapters_obj = doc.get_or_create_map(ROOT, "lora_adapters")?;
        
        doc.put_object(&adapters_obj, &adapter.id, Value::Object(automerge::ObjType::Map))?;
        let adapter_obj = doc.get_object(&adapters_obj, &adapter.id)?;
        
        doc.put(&adapter_obj, "id", adapter.id.clone())?;
        doc.put(&adapter_obj, "device_id", adapter.device_id.clone())?;
        doc.put(&adapter_obj, "base_model", adapter.base_model.clone())?;
        doc.put(&adapter_obj, "adapter_data", adapter.adapter_data.clone())?;
        doc.put(&adapter_obj, "training_steps", adapter.training_steps as i64)?;
        doc.put(&adapter_obj, "loss", adapter.loss)?;
        doc.put(&adapter_obj, "timestamp", adapter.timestamp.to_rfc3339())?;
        
        Ok(())
    }
    
    /// Add policy update to federation
    pub async fn add_policy_update(&self, update: &PolicyUpdate) -> Result<()> {
        let mut doc = self.automerge_doc.lock().await;
        
        let policies_obj = doc.get_or_create_map(ROOT, "policies")?;
        
        doc.put_object(&policies_obj, &update.id, Value::Object(automerge::ObjType::Map))?;
        let policy_obj = doc.get_object(&policies_obj, &update.id)?;
        
        doc.put(&policy_obj, "id", update.id.clone())?;
        doc.put(&policy_obj, "policy_name", update.policy_name.clone())?;
        doc.put(&policy_obj, "rego_content", update.rego_content.clone())?;
        doc.put(&policy_obj, "version", update.version as i64)?;
        doc.put(&policy_obj, "timestamp", update.timestamp.to_rfc3339())?;
        doc.put(&policy_obj, "device_id", update.device_id.clone())?;
        
        Ok(())
    }
    
    /// Get all known LoRA adapters
    pub async fn get_lora_adapters(&self) -> Result<Vec<LoRAAdapter>> {
        let doc = self.automerge_doc.lock().await;
        
        let adapters_obj = match doc.get(ROOT, "lora_adapters") {
            Some(Value::Object(obj_id)) => obj_id,
            _ => return Ok(Vec::new()),
        };
        
        let mut adapters = Vec::new();
        for (key, value) in doc.iter(&adapters_obj) {
            if let Value::Object(adapter_obj) = value {
                let adapter = LoRAAdapter {
                    id: key.to_string(),
                    device_id: doc.get(&adapter_obj, "device_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    base_model: doc.get(&adapter_obj, "base_model").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    adapter_data: doc.get(&adapter_obj, "adapter_data").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    training_steps: doc.get(&adapter_obj, "training_steps").and_then(|v| v.as_int()).unwrap_or(0) as u64,
                    loss: doc.get(&adapter_obj, "loss").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    timestamp: chrono::DateTime::parse_from_rfc3339(
                        doc.get(&adapter_obj, "timestamp").and_then(|v| v.as_str()).unwrap_or("")
                    ).unwrap_or_default().with_timezone(&chrono::Utc),
                };
                adapters.push(adapter);
            }
        }
        
        Ok(adapters)
    }
    
    /// Get federation status
    pub async fn status(&self) -> Result<FederationStatus> {
        let doc = self.automerge_doc.lock().await;
        
        let proposal_count = doc.get(ROOT, "proposals")
            .and_then(|v| v.as_object())
            .map(|obj| doc.length(obj).unwrap_or(0))
            .unwrap_or(0);
        
        let adapter_count = doc.get(ROOT, "lora_adapters")
            .and_then(|v| v.as_object())
            .map(|obj| doc.length(obj).unwrap_or(0))
            .unwrap_or(0);
        
        let policy_count = doc.get(ROOT, "policies")
            .and_then(|v| v.as_object())
            .map(|obj| doc.length(obj).unwrap_or(0))
            .unwrap_or(0);
        
        let tailscale = self.tailscale_client.lock().await;
        let peer_count = if let Some(client) = tailscale.as_ref() {
            client.get_peers().await?.len()
        } else {
            0
        };
        
        Ok(FederationStatus {
            device_id: self.device_id.clone(),
            proposal_count,
            adapter_count,
            policy_count,
            peer_count,
            last_sync: chrono::Utc::now(), // Would track actual last sync
            git_synced: self.config.git_remote.is_some(),
        })
    }
    
    /// Shutdown federator
    pub async fn shutdown(&self) {
        self.persist_automerge().await.ok();
    }
}

/// Tailscale client for peer-to-peer communication
struct TailscaleClient {
    auth_key: String,
    local_client: reqwest::Client,
}

impl TailscaleClient {
    async fn new(auth_key: &str) -> Result<Self> {
        // Start tailscale daemon if not running
        let _ = tokio::process::Command::new("tailscale")
            .args(["up", "--auth-key", auth_key, "--accept-routes"])
            .output()
            .await;
        
        Ok(Self {
            auth_key: auth_key.to_string(),
            local_client: reqwest::Client::new(),
        })
    }
    
    async fn get_peers(&self) -> Result<Vec<PeerInfo>> {
        // Use tailscale CLI or API to get peers
        let output = tokio::process::Command::new("tailscale")
            .args(["status", "--json"])
            .output()
            .await?;
        
        let status: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        let mut peers = Vec::new();
        
        if let Some(peer_map) = status.get("Peer").and_then(|v| v.as_object()) {
            for (_, peer_info) in peer_map {
                peers.push(PeerInfo {
                    device_id: DeviceId::new(
                        peer_info.get("HostName").and_then(|v| v.as_str()).unwrap_or("")
                    ),
                    address: peer_info.get("TailscaleIPs").and_then(|v| v.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    online: peer_info.get("Online").and_then(|v| v.as_bool()).unwrap_or(false),
                });
            }
        }
        
        Ok(peers)
    }
    
    async fn pull_document(&self, _peer: &DeviceId) -> Result<Automerge> {
        // Pull Automerge document from peer via HTTP over Tailscale
        // Placeholder
        Ok(Automerge::new())
    }
    
    async fn push_document(&self, _peer: &DeviceId, _changes: &[Vec<u8>]) -> Result<()> {
        // Push Automerge changes to peer
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct PeerInfo {
    device_id: DeviceId,
    address: String,
    online: bool,
}

/// Federation status
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FederationStatus {
    pub device_id: DeviceId,
    pub proposal_count: usize,
    pub adapter_count: usize,
    pub policy_count: usize,
    pub peer_count: usize,
    pub last_sync: chrono::DateTime<chrono::Utc>,
    pub git_synced: bool,
}