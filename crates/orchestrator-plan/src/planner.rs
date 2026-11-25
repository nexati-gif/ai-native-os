//! Planner: uses LLM + RAG to generate optimization proposals

use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::Mutex;
use tracing::{info, warn, debug, error};
use orchestrator_common::{Error, Result, EventBus, Event};
use orchestrator_common::types::{Proposal, ProposalKind, RiskLevel, Trigger, DeviceId, EstimatedImpact, TestPlan, RollbackPlan};
use crate::{ModelManager, GenerationParams, RagIndex, DocumentChunk};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Planner configuration
#[derive(Debug, Clone)]
pub struct PlannerConfig {
    pub model: Arc<ModelManager>,
    pub rag: Arc<RagIndex>,
    pub event_bus: EventBus,
    pub device_id: DeviceId,
    pub system_prompt: String,
    pub few_shot_examples: Vec<FewShotExample>,
}

/// Few-shot example for prompting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FewShotExample {
    pub context: String,
    pub proposal: Proposal,
}

/// Planner for generating optimization proposals
pub struct Planner {
    config: PlannerConfig,
    metrics: orchestrator_common::MetricsRecorder,
}

impl Planner {
    pub fn new(config: PlannerConfig) -> Self {
        Self {
            config,
            metrics: orchestrator_common::MetricsRecorder::new("planner"),
        }
    }
    
    /// Generate a proposal based on trigger and context
    pub async fn generate_proposal(&self, trigger: Trigger, context: &PlannerContext) -> Result<Proposal> {
        info!("Generating proposal for trigger: {:?}", trigger);
        
        // 1. Build context from system metrics
        let system_context = self.build_system_context(context).await?;
        
        // 2. Retrieve relevant knowledge from RAG
        let rag_context = self.retrieve_rag_context(&trigger, &system_context).await?;
        
        // 3. Build prompt
        let prompt = self.build_prompt(&trigger, &system_context, &rag_context)?;
        
        // 4. Generate proposal using LLM
        let proposal_json = self.config.model.generate(&prompt, None).await?;
        
        // 5. Parse and validate proposal
        let mut proposal = self.parse_proposal(&proposal_json)?;
        
        // 6. Enrich with metadata
        proposal.id = Uuid::new_v4().to_string();
        proposal.trigger = trigger.clone();
        proposal.device_id = self.config.device_id.clone();
        proposal.timestamp = chrono::Utc::now();
        
        // 7. Estimate impact
        proposal.estimated_impact = self.estimate_impact(&proposal, context).await?;
        
        // 8. Generate test plan
        proposal.test_plan = self.generate_test_plan(&proposal)?;
        
        // 9. Generate rollback plan
        proposal.rollback_plan = self.generate_rollback_plan(&proposal)?;
        
        // 10. Emit event
        self.config.event_bus.send(Event::ProposalGenerated {
            proposal: proposal.clone(),
        })?;
        
        self.metrics.counter("proposals_generated", 1);
        info!("Generated proposal: {} ({:?})", proposal.id, proposal.kind);
        
        Ok(proposal)
    }
    
    /// Build system context from metrics
    async fn build_system_context(&self, context: &PlannerContext) -> Result<String> {
        let mut parts = Vec::new();
        
        parts.push("=== SYSTEM METRICS ===".to_string());
        parts.push(format!("CPU Usage: {:.1}%", context.cpu_usage));
        parts.push(format!("Memory Usage: {:.1}%", context.memory_usage));
        parts.push(format!("Memory Available: {} MiB", context.memory_available_mib));
        parts.push(format!("Disk Usage: {:.1}%", context.disk_usage));
        parts.push(format!("Load Average: {:.2}, {:.2}, {:.2}", context.load_1m, context.load_5m, context.load_15m));
        parts.push(format!("Uptime: {}", context.uptime));
        
        if !context.recent_anomalies.is_empty() {
            parts.push("\n=== RECENT ANOMALIES ===".to_string());
            for anomaly in &context.recent_anomalies {
                parts.push(format!("- {}: {} ({:?})", anomaly.collector, anomaly.description, anomaly.severity));
            }
        }
        
        if !context.top_processes.is_empty() {
            parts.push("\n=== TOP PROCESSES ===".to_string());
            for proc in &context.top_processes {
                parts.push(format!("- {} (PID {}): CPU {:.1}% MEM {:.1}%", proc.name, proc.pid, proc.cpu_percent, proc.mem_percent));
            }
        }
        
        Ok(parts.join("\n"))
    }
    
    /// Retrieve relevant context from RAG
    async fn retrieve_rag_context(&self, trigger: &Trigger, system_context: &str) -> Result<String> {
        let mut queries = Vec::new();
        
        // Build queries based on trigger type
        match trigger {
            Trigger::Scheduled { .. } => {
                queries.push("system optimization performance tuning".to_string());
            }
            Trigger::Manual { reason } => {
                queries.push(format!("optimization for: {}", reason));
            }
            Trigger::Anomaly { collector, metric, .. } => {
                queries.push(format!("{} {} anomaly remediation", collector, metric));
            }
            Trigger::UserRequest { request } => {
                queries.push(request.clone());
            }
        }
        
        // Add system context keywords
        if system_context.contains("CPU") && system_context.contains("80") {
            queries.push("high CPU usage optimization".to_string());
        }
        if system_context.contains("Memory") && system_context.contains("85") {
            queries.push("high memory usage optimization".to_string());
        }
        
        // Search RAG for each query
        let mut all_chunks = Vec::new();
        for query in queries {
            let chunks = self.config.rag.search(&query, 5, None).await?;
            all_chunks.extend(chunks);
        }
        
        // Deduplicate by ID
        let mut seen = std::collections::HashSet::new();
        let mut unique_chunks = Vec::new();
        for chunk in all_chunks {
            if seen.insert(chunk.id.clone()) {
                unique_chunks.push(chunk);
            }
        }
        
        // Format as context
        if unique_chunks.is_empty() {
            return Ok("No relevant knowledge found.".to_string());
        }
        
        let mut parts = vec!["=== RELEVANT KNOWLEDGE ===".to_string()];
        for (i, chunk) in unique_chunks.into_iter().take(10).enumerate() {
            parts.push(format!("\n[{}] Source: {}", i + 1, chunk.source));
            parts.push(chunk.content.chars().take(500).collect::<String>());
            if chunk.content.len() > 500 {
                parts.push("...".to_string());
            }
        }
        
        Ok(parts.join("\n"))
    }
    
    /// Build the full prompt for the LLM
    fn build_prompt(&self, trigger: &Trigger, system_context: &str, rag_context: &str) -> Result<String> {
        let mut prompt = String::new();
        
        // System prompt
        prompt.push_str(&self.config.system_prompt);
        prompt.push_str("\n\n");
        
        // Few-shot examples
        if !self.config.few_shot_examples.is_empty() {
            prompt.push_str("=== EXAMPLES ===\n");
            for (i, example) in self.config.few_shot_examples.iter().enumerate() {
                prompt.push_str(&format!("\n--- Example {} ---\n", i + 1));
                prompt.push_str(&format!("Context:\n{}\n", example.context));
                prompt.push_str(&format!("Proposal:\n{}\n", serde_json::to_string_pretty(&example.proposal)?));
            }
            prompt.push_str("\n");
        }
        
        // Current context
        prompt.push_str("=== CURRENT SYSTEM STATE ===\n");
        prompt.push_str(system_context);
        prompt.push_str("\n\n");
        
        prompt.push_str(rag_context);
        prompt.push_str("\n\n");
        
        // Trigger info
        prompt.push_str(&format!("=== TRIGGER ===\n{:?}\n\n", trigger));
        
        // Instructions
        prompt.push_str("=== TASK ===\n");
        prompt.push_str("Generate a single optimization proposal as JSON. The proposal must include:\n");
        prompt.push_str("1. kind: type of change (SysctlTuning, KernelConfig, NixConfig, EbpfProgram, ServiceConfig, etc.)\n");
        prompt.push_str("2. title: brief descriptive title\n");
        prompt.push_str("3. description: detailed explanation of the change\n");
        prompt.push_str("4. rationale: why this change is expected to help\n");
        prompt.push_str("5. diff: unified diff showing the exact changes\n");
        prompt.push_str("6. risk_level: Low, Medium, High, or Critical\n");
        prompt.push_str("7. estimated_impact: performance, memory, latency, stability, security estimates\n");
        prompt.push_str("8. test_plan: benchmarks and integration tests to run\n");
        prompt.push_str("9. rollback_plan: how to revert if issues arise\n\n");
        prompt.push_str("Output ONLY the JSON proposal, no extra text.\n");
        prompt.push_str("</proposal>");
        
        Ok(prompt)
    }
    
    /// Parse proposal from JSON
    fn parse_proposal(&self, json_str: &str) -> Result<Proposal> {
        // Extract JSON from response (may have extra text)
        let json_start = json_str.find('{').ok_or_else(|| Error::Parse("No JSON object found".to_string()))?;
        let json_end = json_str.rfind('}').ok_or_else(|| Error::Parse("No closing brace".to_string()))? + 1;
        let json = &json_str[json_start..json_end];
        
        let proposal: Proposal = serde_json::from_str(json)
            .map_err(|e| Error::Parse(format!("Failed to parse proposal: {}", e)))?;
        
        Ok(proposal)
    }
    
    /// Estimate impact of proposal
    async fn estimate_impact(&self, proposal: &Proposal, context: &PlannerContext) -> Result<EstimatedImpact> {
        // Heuristic-based estimation - in reality would use ML model or historical data
        let mut impact = EstimatedImpact::default();
        
        match proposal.kind {
            ProposalKind::SysctlTuning => {
                impact.performance_delta_pct = 2.0;
                impact.memory_delta_mib = 0;
                impact.latency_delta_pct = -5.0;
                impact.stability_risk = "Low".to_string();
                impact.security_risk = "None".to_string();
            }
            ProposalKind::KernelConfig => {
                impact.performance_delta_pct = 5.0;
                impact.memory_delta_mib = 10;
                impact.latency_delta_pct = -10.0;
                impact.stability_risk = "Medium".to_string();
                impact.security_risk = "Low".to_string();
            }
            ProposalKind::NixConfig => {
                impact.performance_delta_pct = 3.0;
                impact.memory_delta_mib = 5;
                impact.latency_delta_pct = -3.0;
                impact.stability_risk = "Low".to_string();
                impact.security_risk = "None".to_string();
            }
            ProposalKind::EbpfProgram => {
                impact.performance_delta_pct = 1.0;
                impact.memory_delta_mib = 2;
                impact.latency_delta_pct = -2.0;
                impact.stability_risk = "Low".to_string();
                impact.security_risk = "Low".to_string();
            }
            ProposalKind::KernelPatch => {
                impact.performance_delta_pct = 10.0;
                impact.memory_delta_mib = 50;
                impact.latency_delta_pct = -15.0;
                impact.stability_risk = "High".to_string();
                impact.security_risk = "Medium".to_string();
            }
            _ => {
                impact.performance_delta_pct = 1.0;
                impact.memory_delta_mib = 0;
                impact.latency_delta_pct = -1.0;
                impact.stability_risk = "Medium".to_string();
                impact.security_risk = "Low".to_string();
            }
        }
        
        // Adjust based on risk level
        match proposal.risk_level {
            RiskLevel::Low => {
                impact.performance_delta_pct *= 0.5;
            }
            RiskLevel::High => {
                impact.performance_delta_pct *= 1.5;
                impact.stability_risk = "High".to_string();
            }
            RiskLevel::Critical => {
                impact.performance_delta_pct *= 2.0;
                impact.stability_risk = "Critical".to_string();
            }
            _ => {}
        }
        
        Ok(impact)
    }
    
    /// Generate test plan
    fn generate_test_plan(&self, proposal: &Proposal) -> Result<TestPlan> {
        let mut plan = TestPlan {
            unit_tests: vec![],
            integration_tests: vec![],
            benchmarks: vec![],
            canary_duration_secs: 300,
            success_criteria: Default::default(),
        };
        
        match proposal.kind {
            ProposalKind::SysctlTuning => {
                plan.integration_tests.push("sysctl_validation".to_string());
                plan.benchmarks.push("cpu_bench".to_string());
                plan.benchmarks.push("latency_bench".to_string());
                plan.canary_duration_secs = 180;
                plan.success_criteria.statistical_significance = 0.05;
                plan.success_criteria.min_improvement_pct = 1.0;
            }
            ProposalKind::KernelConfig => {
                plan.integration_tests.push("kernel_boot_test".to_string());
                plan.integration_tests.push("kunit_tests".to_string());
                plan.benchmarks.push("kernel_compile_bench".to_string());
                plan.benchmarks.push("sysbench_cpu".to_string());
                plan.canary_duration_secs = 600;
                plan.success_criteria.statistical_significance = 0.01;
                plan.success_criteria.min_improvement_pct = 2.0;
            }
            ProposalKind::NixConfig => {
                plan.integration_tests.push("nixos_build_test".to_string());
                plan.integration_tests.push("service_start_test".to_string());
                plan.benchmarks.push("boot_time_bench".to_string());
                plan.canary_duration_secs = 300;
                plan.success_criteria.statistical_significance = 0.05;
                plan.success_criteria.min_improvement_pct = 1.0;
            }
            ProposalKind::EbpfProgram => {
                plan.integration_tests.push("ebpf_load_test".to_string());
                plan.integration_tests.push("ebpf_functionality_test".to_string());
                plan.benchmarks.push("ebpf_overhead_bench".to_string());
                plan.canary_duration_secs = 300;
                plan.success_criteria.statistical_significance = 0.05;
                plan.success_criteria.min_improvement_pct = 0.5;
            }
            ProposalKind::KernelPatch => {
                plan.integration_tests.push("kernel_boot_test".to_string());
                plan.integration_tests.push("kunit_tests".to_string());
                plan.integration_tests.push("syzkaller_test".to_string());
                plan.benchmarks.push("sysbench_cpu".to_string());
                plan.benchmarks.push("sysbench_memory".to_string());
                plan.benchmarks.push("fio_bench".to_string());
                plan.benchmarks.push("netperf_bench".to_string());
                plan.canary_duration_secs = 3600;
                plan.success_criteria.statistical_significance = 0.01;
                plan.success_criteria.min_improvement_pct = 3.0;
            }
            _ => {
                plan.integration_tests.push("generic_integration_test".to_string());
                plan.benchmarks.push("generic_bench".to_string());
                plan.canary_duration_secs = 300;
                plan.success_criteria.statistical_significance = 0.05;
                plan.success_criteria.min_improvement_pct = 1.0;
            }
        }
        
        Ok(plan)
    }
    
    /// Generate rollback plan
    fn generate_rollback_plan(&self, proposal: &Proposal) -> Result<RollbackPlan> {
        let mut plan = RollbackPlan {
            automatic: true,
            steps: vec![],
            data_loss_risk: "None".to_string(),
            estimated_downtime_secs: 30,
        };
        
        match proposal.kind {
            ProposalKind::SysctlTuning => {
                plan.steps.push("sysctl -p /etc/sysctl.d/99-orchestrator-backup.conf".to_string());
                plan.estimated_downtime_secs = 5;
            }
            ProposalKind::KernelConfig | ProposalKind::KernelPatch => {
                plan.steps.push("kexec -l /run/current-system/kernel-backup --initrd=/run/current-system/initrd-backup".to_string());
                plan.steps.push("kexec -e".to_string());
                plan.automatic = true;
                plan.estimated_downtime_secs = 30;
            }
            ProposalKind::NixConfig => {
                plan.steps.push("nixos-rebuild switch --rollback".to_string());
                plan.estimated_downtime_secs = 60;
            }
            ProposalKind::EbpfProgram => {
                plan.steps.push("bpftool prog detach ...".to_string());
                plan.estimated_downtime_secs = 5;
            }
            _ => {
                plan.steps.push("generic_rollback".to_string());
                plan.estimated_downtime_secs = 60;
            }
        }
        
        Ok(plan)
    }
}

/// Context passed to planner
#[derive(Debug, Clone)]
pub struct PlannerContext {
    pub cpu_usage: f64,
    pub memory_usage: f64,
    pub memory_available_mib: u64,
    pub disk_usage: f64,
    pub load_1m: f64,
    pub load_5m: f64,
    pub load_15m: f64,
    pub uptime: String,
    pub recent_anomalies: Vec<AnomalyInfo>,
    pub top_processes: Vec<ProcessInfo>,
}

#[derive(Debug, Clone)]
pub struct AnomalyInfo {
    pub collector: String,
    pub metric: String,
    pub description: String,
    pub severity: orchestrator_common::types::AnomalySeverity,
}

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f64,
    pub mem_percent: f64,
}

/// Default system prompt
pub fn default_system_prompt() -> String {
    r#"You are the AI-Native OS Orchestrator, an intelligent agent that observes system behavior and proposes optimizations.

Your role:
- Analyze system metrics, anomalies, and user requests
- Propose specific, actionable changes to improve performance, efficiency, or stability
- Every proposal must be safe, testable, and reversible
- You NEVER apply changes directly - all proposals require human approval

Output format: JSON proposal with the following fields:
- kind: "SysctlTuning" | "KernelConfig" | "NixConfig" | "EbpfProgram" | "ServiceConfig" | "KernelPatch" | "ModuleConfig" | "SchedulerTuning" | "MemoryTuning" | "NetworkTuning" | "Custom"
- title: string (max 100 chars)
- description: string (detailed explanation)
- rationale: string (why this helps)
- diff: string (unified diff format)
- risk_level: "Low" | "Medium" | "High" | "Critical"
- estimated_impact: { performance_delta_pct, memory_delta_mib, latency_delta_pct, stability_risk, security_risk }
- test_plan: { unit_tests[], integration_tests[], benchmarks[], canary_duration_secs, success_criteria }
- rollback_plan: { automatic, steps[], data_loss_risk, estimated_downtime_secs }

Guidelines:
- Prefer Low/Medium risk changes
- Always provide automatic rollback
- Include measurable success criteria
- Reference specific kernel/sysctl/Nix options
- Keep diffs minimal and focused"#.to_string()
}

/// Default few-shot examples
pub fn default_few_shot_examples() -> Vec<FewShotExample> {
    vec![
        FewShotExample {
            context: "High CPU usage (85%) from kernel threads, high context switch rate".to_string(),
            proposal: Proposal {
                id: "example-1".to_string(),
                kind: ProposalKind::SysctlTuning,
                title: "Reduce scheduler latency via sched_latency_ns".to_string(),
                description: "Tune scheduler latency parameters to reduce context switch overhead".to_string(),
                rationale: "High context switch rate indicates scheduler overhead. Reducing sched_latency_ns allows more granular scheduling.".to_string(),
                diff: "- kernel.sched_latency_ns = 24000000\n+ kernel.sched_latency_ns = 12000000\n- kernel.sched_min_granularity_ns = 3000000\n+ kernel.sched_min_granularity_ns = 1500000".to_string(),
                risk_level: RiskLevel::Low,
                estimated_impact: EstimatedImpact {
                    performance_delta_pct: 3.0,
                    memory_delta_mib: 0,
                    latency_delta_pct: -10.0,
                    stability_risk: "Low".to_string(),
                    security_risk: "None".to_string(),
                },
                test_plan: TestPlan {
                    unit_tests: vec![],
                    integration_tests: vec!["sysctl_validation".to_string()],
                    benchmarks: vec!["context_switch_bench".to_string()],
                    canary_duration_secs: 180,
                    success_criteria: Default::default(),
                },
                rollback_plan: RollbackPlan {
                    automatic: true,
                    steps: vec!["sysctl -p /etc/sysctl.d/99-orchestrator-backup.conf".to_string()],
                    data_loss_risk: "None".to_string(),
                    estimated_downtime_secs: 5,
                },
                trigger: Trigger::Manual { reason: "example".to_string() },
                device_id: "example".to_string(),
                timestamp: chrono::Utc::now(),
            },
        },
        FewShotExample {
            context: "High memory pressure (92%), frequent kswapd activity, swap usage increasing".to_string(),
            proposal: Proposal {
                id: "example-2".to_string(),
                kind: ProposalKind::KernelConfig,
                title: "Increase watermark_scale_factor for earlier reclaim".to_string(),
                description: "Adjust VM watermarks to start memory reclaim earlier".to_string(),
                rationale: "System is waiting too long before starting reclaim, causing latency spikes during memory pressure.".to_string(),
                diff: "- vm.watermark_scale_factor = 10\n+ vm.watermark_scale_factor = 20\n- vm.dirty_background_ratio = 10\n+ vm.dirty_background_ratio = 5".to_string(),
                risk_level: RiskLevel::Medium,
                estimated_impact: EstimatedImpact {
                    performance_delta_pct: 2.0,
                    memory_delta_mib: -50,
                    latency_delta_pct: -15.0,
                    stability_risk: "Medium".to_string(),
                    security_risk: "None".to_string(),
                },
                test_plan: TestPlan {
                    unit_tests: vec![],
                    integration_tests: vec!["memory_pressure_test".to_string()],
                    benchmarks: vec!["stream_bench".to_string()],
                    canary_duration_secs: 600,
                    success_criteria: Default::default(),
                },
                rollback_plan: RollbackPlan {
                    automatic: true,
                    steps: vec!["sysctl -p /etc/sysctl.d/99-orchestrator-backup.conf".to_string()],
                    data_loss_risk: "None".to_string(),
                    estimated_downtime_secs: 10,
                },
                trigger: Trigger::Manual { reason: "example".to_string() },
                device_id: "example".to_string(),
                timestamp: chrono::Utc::now(),
            },
        },
    ]
}