//! Policy engine implementation

use std::path::Path;
use std::sync::Arc;
use opa_wasm::{Engine, PolicyDecision};
use serde_json::json;
use tracing::{info, debug, warn};
use crate::{PolicyEngine, PolicyConfig};
use orchestrator_common::{Error, Result};
use orchestrator_common::types::{Proposal, RiskLevel, ProposalKind, Component, PolicyEngine as PolicyEngineType};

/// Policy engine using OPA/Rego
pub struct PolicyEngine {
    engine: Engine,
    config: PolicyConfig,
}

#[derive(Debug, Clone)]
pub struct PolicyConfig {
    pub policies_dir: String,
    pub default_deny: bool,
}

impl PolicyEngine {
    pub fn new(config: &orchestrator_common::types::PolicyConfig) -> Result<Self> {
        let mut engine = Engine::new();
        
        // Load policies from directory
        let policies_path = Path::new(&config.policies_dir);
        if policies_path.exists() {
            for entry in std::fs::read_dir(policies_path)? {
                let entry = entry?;
                if entry.path().extension().map_or(false, |ext| ext == "rego") {
                    let policy = std::fs::read_to_string(entry.path())?;
                    engine.add_policy(entry.file_name().to_string_lossy().as_ref(), &policy)
                        .map_err(|e| Error::Internal(format!("Failed to add policy: {}", e)))?;
                }
            }
        } else {
            warn!("Policies directory not found: {}", config.policies_dir);
        }

        // Add default policies
        for (name, policy) in crate::policies::DEFAULT_POLICIES {
            engine.add_policy(name, policy)
                .map_err(|e| Error::Internal(format!("Failed to add default policy: {}", e)))?;
        }

        Ok(Self {
            engine,
            config: PolicyConfig {
                policies_dir: config.policies_dir.clone(),
                default_deny: config.default_deny,
            },
        })
    }

    /// Check if proposal is allowed by policy
    pub fn allow(&self, proposal: &Proposal) -> Result<bool> {
        let input = json!({
            "proposal": {
                "id": proposal.id.to_string(),
                "kind": format!("{:?}", proposal.kind),
                "risk_level": format!("{:?}", proposal.risk_level),
                "description": proposal.description,
                "rationale": proposal.rationale,
                "estimated_impact": proposal.estimated_impact,
                "affected_components": proposal.affected_components.iter().map(|c| format!("{:?}", c)).collect::<Vec<_>>(),
                "test_plan": proposal.test_plan,
                "rollback_plan": proposal.rollback_plan,
            }
        });

        let decision = self.engine.evaluate("orchestrator/allow", &input)
            .map_err(|e| Error::Internal(format!("Policy evaluation failed: {}", e)))?;

        let allowed = decision.result.get("result").and_then(|r| r.get("allow")).and_then(|a| a.as_bool()).unwrap_or(false);
        
        if !allowed {
            warn!("Proposal {} denied by policy", proposal.id);
            if let Some(reason) = decision.result.get("result").and_then(|r| r.get("reason")).and_then(|r| r.as_str()) {
                warn!("Denial reason: {}", reason);
            }
        }

        Ok(allowed)
    }

    /// Get policy decision with details
    pub fn decide(&self, proposal: &Proposal) -> Result<PolicyDecisionResult> {
        let input = json!({
            "proposal": {
                "id": proposal.id.to_string(),
                "kind": format!("{:?}", proposal.kind),
                "risk_level": format!("{:?}", proposal.risk_level),
                "description": proposal.description,
                "rationale": proposal.rationale,
                "estimated_impact": proposal.estimated_impact,
                "affected_components": proposal.affected_components.iter().map(|c| format!("{:?}", c)).collect::<Vec<_>>(),
            }
        });

        let decision = self.engine.evaluate("orchestrator/decide", &input)
            .map_err(|e| Error::Internal(format!("Policy evaluation failed: {}", e)))?;

        let result = decision.result.get("result").ok_or_else(|| Error::Internal("No result in policy decision".to_string()))?;
        
        Ok(PolicyDecisionResult {
            allow: result.get("allow").and_then(|a| a.as_bool()).unwrap_or(false),
            reason: result.get("reason").and_then(|r| r.as_str()).map(|s| s.to_string()),
            required_approvals: result.get("required_approvals").and_then(|r| r.as_u64()).unwrap_or(1),
            max_risk_level: result.get("max_risk_level").and_then(|r| r.as_str()).map(|s| s.to_string()),
        })
    }
}

/// Policy decision result
#[derive(Debug, Clone)]
pub struct PolicyDecisionResult {
    pub allow: bool,
    pub reason: Option<String>,
    pub required_approvals: u64,
    pub max_risk_level: Option<String>,
}

/// Default policies
pub mod policies {
    pub const DEFAULT_POLICIES: &[(&str, &str)] = &[
        ("base.rego", BASE_POLICY),
        ("kernel.rego", KERNEL_POLICY),
        ("safety.rego", SAFETY_POLICY),
    ];

    const BASE_POLICY: &str = r#"
package orchestrator

default allow = false
default reason = "No matching policy"
default required_approvals = 1
default max_risk_level = "critical"

# Allow low-risk sysctl tuning
allow {
    input.proposal.kind == "SysctlTuning"
    input.proposal.risk_level == "Low"
    input.proposal.estimated_impact.stability_risk == "Low"
    input.proposal.estimated_impact.security_risk == "None"
}

# Allow eBPF programs with medium risk if tests exist
allow {
    input.proposal.kind == "EbpfProgram"
    input.proposal.risk_level == "Medium"
    count(input.proposal.test_plan.integration_tests) > 0
    input.proposal.test_plan.canary_duration_secs >= 300
}

# Allow Nix config changes with proper rollback
allow {
    input.proposal.kind == "NixConfig"
    input.proposal.rollback_plan.automatic == true
    input.proposal.test_plan.canary_duration_secs >= 180
}

# Deny critical kernel patches without extensive testing
allow {
    input.proposal.kind == "KernelPatch"
    input.proposal.risk_level == "Critical"
    count(input.proposal.test_plan.integration_tests) >= 3
    count(input.proposal.test_plan.benchmarks) >= 2
    input.proposal.test_plan.canary_duration_secs >= 3600
    input.proposal.test_plan.success_criteria.statistical_significance <= 0.01
}

# Require multiple approvals for high-risk changes
required_approvals = 2 {
    input.proposal.risk_level == "High"
}

required_approvals = 3 {
    input.proposal.risk_level == "Critical"
}

# Max risk level based on component
max_risk_level = "low" {
    input.proposal.affected_components[_] == "KernelScheduler"
}

max_risk_level = "low" {
    input.proposal.affected_components[_] == "KernelMemory"
}
"#;

    const KERNEL_POLICY: &str = r#"
package orchestrator

# Kernel-specific policies
deny_kernel_modification {
    input.proposal.kind == "KernelModule"
    not input.proposal.test_plan.unit_tests[_] == "kernel_module_test"
}

deny_scheduler_change {
    input.proposal.affected_components[_] == "KernelScheduler"
    input.proposal.test_plan.canary_duration_secs < 600
}

deny_memory_change {
    input.proposal.affected_components[_] == "KernelMemory"
    input.proposal.estimated_impact.stability_risk != "Low"
}
"#;

    const SAFETY_POLICY: &str = r#"
package orchestrator

# Safety policies - always deny these
deny_unsafe {
    # No changes that could cause data loss
    input.proposal.rollback_plan.data_loss_risk != "None"
}

deny_unsafe {
    # No changes without rollback plan
    not input.proposal.rollback_plan.automatic
    input.proposal.risk_level != "Low"
}

deny_unsafe {
    # No changes that bypass canary
    input.proposal.test_plan.canary_duration_secs == 0
    input.proposal.risk_level != "Low"
}

# Override allow if any deny_unsafe matches
allow = false {
    deny_unsafe
    reason = "Safety violation: " + deny_reason
}

deny_reason = "Data loss risk" {
    input.proposal.rollback_plan.data_loss_risk != "None"
}

deny_reason = "No automatic rollback" {
    not input.proposal.rollback_plan.automatic
    input.proposal.risk_level != "Low"
}

deny_reason = "No canary period" {
    input.proposal.test_plan.canary_duration_secs == 0
    input.proposal.risk_level != "Low"
}
"#;
}