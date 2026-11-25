//! Prompt templates for the planner

use crate::PlannerContext;
use orchestrator_common::types::{Proposal, ProposalKind, RiskLevel, ImpactEstimate, TestPlan, RollbackPlan, Component, SuccessCriteria, BenchmarkSpec, RollbackTrigger};
use std::collections::BTreeMap;

/// System prompt for the planner
pub const SYSTEM_PROMPT: &str = r#"
You are the AI-Native OS Orchestrator Planner. Your role is to analyze system state, user requests, and anomalies to generate concrete, testable proposals for system optimization and improvement.

CORE PRINCIPLES:
1. MINIMAL CHANGE - Propose the smallest change that achieves the goal
2. REVERSIBILITY - Every proposal must have a tested rollback plan
3. MEASURABLE IMPACT - Define clear success criteria with statistical significance
4. SAFETY FIRST - Kernel changes require highest scrutiny; userspace changes are lower risk
5. HUMAN APPROVAL - You propose, human decides. Never execute without explicit approval.

PROPOSAL TYPES (in order of risk):
- SysctlTuning: Kernel parameter adjustments (low risk, reversible)
- EbpfProgram: eBPF programs for scheduling, networking, security (medium risk, verifiable)
- KernelModule: Custom kernel modules (high risk, requires signing)
- KernelPatch: Core kernel modifications (critical risk, requires reboot)
- NixConfig: NixOS configuration changes (medium risk, atomic rollback)
- ServiceConfig: systemd service modifications (low-medium risk)
- ModelUpdate: LLM/LoRA weight updates (medium risk)
- PolicyUpdate: OPA/Rego policy changes (low risk)
- FederationSync: Cross-device knowledge sync (low risk)

OUTPUT FORMAT: Structured JSON matching the Proposal schema exactly.
"#;

/// User prompt template
pub fn build_user_prompt(context: &PlannerContext) -> String {
    format!(r#"
CURRENT CONTEXT:
- Device: {}
- Trigger: {:?}
- Active Anomalies: {:?}
- Loaded Components: {:?}

CURRENT METRICS:
{}

RECENT PROPOSALS (for few-shot learning):
{}

YOUR TASK:
Analyze the current state and trigger. Generate a SINGLE proposal that addresses the most impactful opportunity or issue. 

Consider:
1. What is the root cause of any anomalies?
2. What optimization would yield the best improvement/risk ratio?
3. What can be tested safely in a microVM sandbox?
4. How will you measure success statistically?

Respond with a COMPLETE Proposal JSON object.
"#,
        context.device_id.0,
        context.trigger,
        context.active_anomalies,
        context.loaded_components,
        format_metrics(&context.current_metrics),
        format_recent_proposals(&context.recent_proposals)
    )
}

fn format_metrics(metrics: &BTreeMap<String, f64>) -> String {
    if metrics.is_empty() {
        "No metrics available".to_string()
    } else {
        metrics.iter()
            .map(|(k, v)| format!("  - {}: {:.2}", k, v))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn format_recent_proposals(proposals: &[Proposal]) -> String {
    if proposals.is_empty() {
        "No recent proposals".to_string()
    } else {
        proposals.iter()
            .map(|p| format!("  - {}: {} (risk: {:?}, approved: {})", 
                p.id, p.description, p.risk_level, 
                p.metadata.get("approved").unwrap_or(&"unknown".to_string())))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Few-shot examples for common proposal types
pub const FEW_SHOT_EXAMPLES: &str = r#"
EXAMPLE 1 - Sysctl Tuning:
{
  "id": "proposal-uuid",
  "kind": "SysctlTuning",
  "description": "Reduce swappiness from 60 to 10 to improve interactive latency",
  "rationale": "High swap activity detected during interactive workloads. Lower swappiness keeps more pages in RAM.",
  "risk_level": "Low",
  "estimated_impact": {
    "performance_delta_pct": 5.0,
    "memory_delta_mib": 0,
    "cpu_delta_pct": -1.0,
    "latency_delta_pct": -15.0,
    "battery_delta_pct": -2.0,
    "stability_risk": "Low",
    "security_risk": "None",
    "compatibility_risk": "None"
  },
  "test_plan": {
    "unit_tests": [],
    "integration_tests": ["test_swap_behavior"],
    "benchmarks": [
      {"name": "interactive_latency", "command": "cyclictest", "args": ["-m", "-p90"], "baseline_metric": 5000.0, "target_improvement_pct": 15.0}
    ],
    "canary_duration_secs": 300,
    "success_criteria": {
      "min_tests_passing": 1,
      "max_regression_pct": 5.0,
      "min_improvement_pct": 10.0,
      "statistical_significance": 0.05,
      "require_all_benchmarks": true
    }
  },
  "rollback_plan": {
    "automatic": true,
    "trigger_conditions": [
      {"MetricRegression": {"metric": "interactive_latency", "threshold_pct": 10.0}}
    ],
    "estimated_downtime_secs": 5,
    "data_loss_risk": "None"
  },
  "affected_components": ["KernelMemory"],
  "diff": "vm.swappiness = 10"
}

EXAMPLE 2 - eBPF Scheduler:
{
  "id": "proposal-uuid",
  "kind": "EbpfProgram",
  "description": "Deploy custom sched_ext scheduler for latency-sensitive workloads",
  "rationale": "CFS shows high tail latency for interactive tasks. Custom scheduler can prioritize interactive threads.",
  "risk_level": "Medium",
  "estimated_impact": {
    "performance_delta_pct": 10.0,
    "memory_delta_mib": 2,
    "cpu_delta_pct": 0.0,
    "latency_delta_pct": -30.0,
    "battery_delta_pct": 0.0,
    "stability_risk": "Medium",
    "security_risk": "Low",
    "compatibility_risk": "Low"
  },
  "test_plan": {
    "unit_tests": ["scheduler_unit_tests"],
    "integration_tests": ["scheduler_integration_test"],
    "benchmarks": [
      {"name": "sched_latency", "command": "sched_latency_benchmark", "args": [], "baseline_metric": 1000.0, "target_improvement_pct": 30.0}
    ],
    "canary_duration_secs": 600,
    "success_criteria": {
      "min_tests_passing": 2,
      "max_regression_pct": 5.0,
      "min_improvement_pct": 20.0,
      "statistical_significance": 0.01,
      "require_all_benchmarks": true
    }
  },
  "rollback_plan": {
    "automatic": true,
    "trigger_conditions": [
      {"Crash": {"component": "scheduler"}},
      {"MetricRegression": {"metric": "sched_latency", "threshold_pct": 10.0}}
    ],
    "estimated_downtime_secs": 10,
    "data_loss_risk": "None"
  },
  "affected_components": ["KernelScheduler", "EbpfPrograms"],
  "diff": "// eBPF scheduler code..."
}
"#;