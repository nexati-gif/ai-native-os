//! Automerge CRDT synchronization

use std::path::Path;
use automerge::{Automerge, AutoCommit, ReadDoc, RootedObject};
use serde::{Serialize, Deserialize};
use orchestrator_common::{Error, Result};
use orchestrator_common::types::{FederatedState, Proposal, ValidationResult, ApprovedProposal, LoRAAdapterMeta, RagChunk, PolicySet, KernelConfig};

/// Automerge sync for federated state
pub struct AutomergeSync {
    doc: Automerge,
    path: String,
}

impl AutomergeSync {
    pub async fn new(path: &str) -> Result<Self> {
        let doc = if Path::new(path).exists() {
            let bytes = tokio::fs::read(path).await?;
            Automerge::load(&bytes)?
        } else {
            let mut doc = Automerge::new();
            // Initialize empty federated state
            let state = FederatedState {
                lora_adapters: std::collections::BTreeMap::new(),
                rag_chunks: std::collections::BTreeMap::new(),
                policies: PolicySet { version: 0, policies: vec![], updated_at: chrono::Utc::now() },
                kernel_configs: std::collections::BTreeMap::new(),
                approved_proposals: vec![],
            };
            doc.put(automerge::ROOT, "state", state)?;
            doc
        };

        Ok(Self { doc, path: path.to_string() })
    }

    pub async fn add_approved_proposal(&mut self, proposal: &Proposal, validation: &ValidationResult) -> Result<()> {
        let approved = ApprovedProposal {
            proposal: proposal.clone(),
            approval: orchestrator_common::types::Approval {
                proposal_id: proposal.id,
                approved: true,
                modified_proposal: None,
                reason: None,
                timestamp: chrono::Utc::now(),
                approver: "local".to_string(),
            },
            validation: validation.clone(),
            device_id: orchestrator_common::types::DeviceId::new(&hostname::get()?.to_string_lossy()),
        };

        // Append to approved_proposals list
        let mut proposals = self.doc.get_list(automerge::ROOT, "state.approved_proposals")?;
        proposals.push_back(&mut self.doc, approved)?;
        
        Ok(())
    }

    pub async fn update_lora_adapter(&mut self, adapter: LoRAAdapterMeta) -> Result<()> {
        let device_id = orchestrator_common::types::DeviceId::new(&hostname::get()?.to_string_lossy());
        self.doc.put(automerge::ROOT, "state.lora_adapters", (device_id.0, adapter))?;
        Ok(())
    }

    pub async fn update_rag_chunk(&mut self, chunk: RagChunk) -> Result<()> {
        self.doc.put(automerge::ROOT, "state.rag_chunks", (chunk.id.clone(), chunk))?;
        Ok(())
    }

    pub async fn update_policy(&mut self, policies: PolicySet) -> Result<()> {
        self.doc.put(automerge::ROOT, "state.policies", policies)?;
        Ok(())
    }

    pub async fn load_state(&self) -> Result<FederatedState> {
        // In real implementation, would deserialize from Automerge
        // For now, return empty
        Ok(FederatedState {
            lora_adapters: std::collections::BTreeMap::new(),
            rag_chunks: std::collections::BTreeMap::new(),
            policies: PolicySet { version: 0, policies: vec![], updated_at: chrono::Utc::now() },
            kernel_configs: std::collections::BTreeMap::new(),
            approved_proposals: vec![],
        })
    }

    pub async fn save(&self) -> Result<()> {
        let bytes = self.doc.save()?;
        tokio::fs::write(&self.path, bytes).await?;
        Ok(())
    }

    pub fn merge(&mut self, other: &Automerge) -> Result<()> {
        self.doc.merge(other)?;
        Ok(())
    }
}