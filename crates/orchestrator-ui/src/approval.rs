//! Approval interface types

use orchestrator_common::types::{Proposal, ProposalId, Approval};

/// Approval response from UI
#[derive(Debug, Clone)]
pub struct ApprovalResponse {
    pub proposal_id: ProposalId,
    pub approved: bool,
    pub modified_proposal: Option<Proposal>,
    pub reason: Option<String>,
}

/// UI server trait for approval requests
#[async_trait::async_trait]
pub trait UiServer: Send + Sync {
    async fn request_approval(&self, proposal: Proposal) -> orchestrator_common::Result<ApprovalResponse>;
    async fn shutdown(&self);
}