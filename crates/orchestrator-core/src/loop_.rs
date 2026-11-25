//! Main orchestrator loop with passive/active modes and lazy component loading

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Mutex};
use tokio::task::JoinHandle;
use tracing::{info, warn, error, debug, instrument};
use async_trait::async_trait;
use crate::{
    OrchestratorLoop, CycleResult, ComponentLoader, SignalHandler,
    OrchestratorConfig, OrchestratorState, DeviceId, ProposalId, Trigger, Approval,
    Proposal, ValidationResult, ValidationFailure,
    Event, EventBus,
    Error, Result,
};
use orchestrator_common::{StateManager, MetricsRecorder};
use orchestrator_observe::ObserveCollector;
use orchestrator_plan::Planner;
use orchestrator_act::Actor;
use orchestrator_validate::Validator;
use orchestrator_federate::Federator;
use orchestrator_ui::{UiServer, UiMode};
use orchestrator_policy::PolicyEngine;

/// Main orchestrator loop
pub struct OrchestratorLoop {
    config: OrchestratorConfig,
    state: Arc<RwLock<OrchestratorState>>,
    state_manager: Arc<StateManager>,
    event_bus: EventBus,
    policy_engine: Arc<PolicyEngine>,
    metrics: MetricsRecorder,

    // Lazily loaded components (Option = not loaded until needed)
    observe: Arc<Mutex<Option<ObserveCollector>>>,
    planner: Arc<Mutex<Option<Planner>>>,
    actor: Arc<Mutex<Option<Actor>>>,
    validator: Arc<Mutex<Option<Validator>>>,
    federate: Arc<Mutex<Option<Federator>>>,
    ui: Arc<Mutex<Option<UiServer>>>,

    // Background tasks
    passive_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    unload_timer: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl OrchestratorLoop {
    /// Create a new orchestrator loop
    pub async fn new(config: OrchestratorConfig) -> Result<Self> {
        let device_id = config.device_id.clone();
        let state_manager = Arc::new(StateManager::new(&config.state_db_path)?);
        let event_bus = EventBus::new(config.passive_mode.events_buffer_size);
        let policy_engine = Arc::new(PolicyEngine::new(&config.policy)?);
        let metrics = MetricsRecorder::new("orchestrator");

        // Load or create state
        let state = Arc::new(RwLock::new(
            state_manager.get_state("current")?
                .unwrap_or_else(|| OrchestratorState::new(device_id))
        ));

        let loop_ = Self {
            config,
            state,
            state_manager,
            event_bus,
            policy_engine,
            metrics,
            observe: Arc::new(Mutex::new(None)),
            planner: Arc::new(Mutex::new(None)),
            actor: Arc::new(Mutex::new(None)),
            validator: Arc::new(Mutex::new(None)),
            federate: Arc::new(Mutex::new(None)),
            ui: Arc::new(Mutex::new(None)),
            passive_handle: Arc::new(Mutex::new(None)),
            unload_timer: Arc::new(Mutex::new(None)),
        };

        // Restore persistent state
        loop_.restore_state().await?;

        Ok(loop_)
    }

    /// Restore state from database
    async fn restore_state(&self) -> Result<()> {
        if let Some(fed_state) = self.state_manager.get_federation_state()? {
            // Federation state will be loaded by federate component
        }
        info!("State restored");
        Ok(())
    }

    /// Persist current state
    pub async fn persist_state(&self) -> Result<()> {
        let state = self.state.read().await;
        self.state_manager.set_state("current", &*state)?;
        Ok(())
    }

    // ========== Lazy Loaders ==========

    async fn ensure_observe_loaded(&self) -> Result<()> {
        let mut guard = self.observe.lock().await;
        if guard.is_none() {
            info!("Loading observe collector");
            let collector = ObserveCollector::new(&self.config.passive_mode, self.event_bus.clone()).await?;
            *guard = Some(collector);
            self.metrics.counter("component_loaded", 1);
            self.event_bus.send(Event::ComponentLoaded { component: "observe".to_string() })?;
            self.state.write().await.loaded_components.push("observe".to_string());
        }
        Ok(())
    }

    async fn ensure_planner_loaded(&self) -> Result<()> {
        let mut guard = self.planner.lock().await;
        if guard.is_none() {
            info!("Loading planner (model + RAG)");
            let planner = Planner::new(&self.config, self.event_bus.clone()).await?;
            *guard = Some(planner);
            self.metrics.counter("component_loaded", 1);
            self.event_bus.send(Event::ComponentLoaded { component: "planner".to_string() })?;
            self.state.write().await.loaded_components.push("planner".to_string());
        }
        Ok(())
    }

    async fn ensure_actor_loaded(&self) -> Result<()> {
        let mut guard = self.actor.lock().await;
        if guard.is_none() {
            info!("Loading actor (Nix, sandbox, kexec)");
            let actor = Actor::new(&self.config.sandbox, self.event_bus.clone()).await?;
            *guard = Some(actor);
            self.metrics.counter("component_loaded", 1);
            self.event_bus.send(Event::ComponentLoaded { component: "actor".to_string() })?;
            self.state.write().await.loaded_components.push("actor".to_string());
        }
        Ok(())
    }

    async fn ensure_validator_loaded(&self) -> Result<()> {
        let mut guard = self.validator.lock().await;
        if guard.is_none() {
            info!("Loading validator");
            let validator = Validator::new(&self.config, self.event_bus.clone()).await?;
            *guard = Some(validator);
            self.metrics.counter("component_loaded", 1);
            self.event_bus.send(Event::ComponentLoaded { component: "validator".to_string() })?;
            self.state.write().await.loaded_components.push("validator".to_string());
        }
        Ok(())
    }

    async fn ensure_federate_loaded(&self) -> Result<()> {
        let mut guard = self.federate.lock().await;
        if guard.is_none() {
            info!("Loading federate");
            let federate = Federator::new(&self.config.federation, self.event_bus.clone()).await?;
            *guard = Some(federate);
            self.metrics.counter("component_loaded", 1);
            self.event_bus.send(Event::ComponentLoaded { component: "federate".to_string() })?;
            self.state.write().await.loaded_components.push("federate".to_string());
        }
        Ok(())
    }

    async fn ensure_ui_loaded(&self) -> Result<()> {
        let mut guard = self.ui.lock().await;
        if guard.is_none() {
            info!("Loading UI server");
            let ui = UiServer::new(&self.config.ui, self.event_bus.clone()).await?;
            *guard = Some(ui);
            self.metrics.counter("component_loaded", 1);
            self.event_bus.send(Event::ComponentLoaded { component: "ui".to_string() })?;
            self.state.write().await.loaded_components.push("ui".to_string());
        }
        Ok(())
    }

    // ========== Unloading ==========

    /// Unload all non-passive components to free memory
    pub async fn unload_all_except_passive(&self) {
        info!("Unloading non-passive components");
        
        // Planner (model + RAG) - biggest memory consumer
        if let Some(mut planner) = self.planner.lock().await.take() {
            planner.shutdown().await;
            self.metrics.counter("component_unloaded", 1);
            self.event_bus.send(Event::ComponentUnloaded { component: "planner".to_string() }).ok();
        }

        // Actor (sandbox processes)
        if let Some(mut actor) = self.actor.lock().await.take() {
            actor.shutdown().await;
            self.metrics.counter("component_unloaded", 1);
            self.event_bus.send(Event::ComponentUnloaded { component: "actor".to_string() }).ok();
        }

        // Validator
        if let Some(_) = self.validator.lock().await.take() {
            self.metrics.counter("component_unloaded", 1);
            self.event_bus.send(Event::ComponentUnloaded { component: "validator".to_string() }).ok();
        }

        // Federate
        if let Some(_) = self.federate.lock().await.take() {
            self.metrics.counter("component_unloaded", 1);
            self.event_bus.send(Event::ComponentUnloaded { component: "federate".to_string() }).ok();
        }

        // UI
        if let Some(mut ui) = self.ui.lock().await.take() {
            ui.shutdown().await;
            self.metrics.counter("component_unloaded", 1);
            self.event_bus.send(Event::ComponentUnloaded { component: "ui".to_string() }).ok();
        }

        // Force memory reclaim
        self.state.write().await.loaded_components.retain(|c| c == "observe");
        
        debug!("All non-passive components unloaded");
    }

    // ========== Passive Loop ==========

    /// Start passive observation loop (runs continuously)
    pub async fn start_passive_loop(&self) -> Result<()> {
        if !self.config.passive_mode.enabled {
            info!("Passive mode disabled");
            return Ok(());
        }

        self.ensure_observe_loaded().await?;

        let observe = self.observe.clone();
        let event_bus = self.event_bus.clone();
        let state = self.state.clone();
        let config = self.config.passive_mode.clone();
        let persist_interval = config.persist_interval_secs;

        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(config.poll_interval_ms));
            let mut persist_timer = tokio::time::interval(Duration::from_secs(persist_interval));

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if let Some(collector) = observe.lock().await.as_mut() {
                            if let Err(e) = collector.poll_events().await {
                                error!("Observe poll error: {}", e);
                            }
                        }
                    }
                    _ = persist_timer.tick() => {
                        // Persist state periodically
                        if let Some(manager) = state.try_read() {
                            // Note: can't persist from here without StateManager access
                            // This would need a channel to the main loop
                        }
                    }
                }
            }
        });

        *self.passive_handle.lock().await = Some(handle);
        info!("Passive loop started");
        Ok(())
    }

    /// Stop passive loop
    pub async fn stop_passive_loop(&self) {
        if let Some(handle) = self.passive_handle.lock().await.take() {
            handle.abort();
            info!("Passive loop stopped");
        }
    }

    // ========== Active Cycle ==========

    /// Run a single active cycle (triggered by user, anomaly, schedule, etc.)
    #[instrument(skip(self, trigger))]
    pub async fn active_cycle(&self, trigger: Trigger) -> Result<CycleResult> {
        let cycle_start = Instant::now();
        let proposal_id = ProposalId::new();

        info!(%proposal_id, ?trigger, "Starting active cycle");

        // Update state
        {
            let mut state = self.state.write().await;
            state.current_proposal = Some(proposal_id);
            state.total_cycles += 1;
        }

        self.event_bus.send(Event::CycleStarted { proposal_id, trigger: trigger.clone() })?;

        // 1. Load planner (model + RAG)
        self.ensure_planner_loaded().await?;
        let planner = self.planner.lock().await;
        let planner = planner.as_ref().unwrap();

        // 2. Build context from observation + RAG + history
        let context = self.build_context(&trigger).await?;

        // 3. Generate proposal
        let proposal = planner.propose(context, self.policy_engine.clone()).await?;
        proposal.id = proposal_id; // Ensure ID matches

        self.event_bus.send(Event::ProposalGenerated { proposal: proposal.clone() })?;

        // 4. Policy check BEFORE showing to user
        if !self.policy_engine.allow(&proposal)? {
            warn!(%proposal_id, "Proposal denied by policy");
            self.event_bus.send(Event::ApprovalReceived { 
                approval: Approval {
                    proposal_id,
                    approved: false,
                    modified_proposal: None,
                    reason: Some("Denied by policy".to_string()),
                    timestamp: chrono::Utc::now(),
                    approver: "policy_engine".to_string(),
                }
            })?;
            return Ok(CycleResult::Rejected);
        }

        // 5. Request human approval (blocks until response)
        self.ensure_ui_loaded().await?;
        let ui = self.ui.lock().await;
        let ui = ui.as_ref().unwrap();

        self.event_bus.send(Event::ApprovalRequested { proposal_id })?;
        let approval = ui.request_approval(proposal.clone()).await?;

        self.event_bus.send(Event::ApprovalReceived { approval: approval.clone() })?;

        if !approval.approved {
            info!(%proposal_id, "Proposal rejected by user");
            self.unload_all_except_passive().await;
            return Ok(CycleResult::Rejected);
        }

        let final_proposal = approval.modified_proposal.unwrap_or(proposal);

        // 6. Load actor and execute in sandbox
        self.ensure_actor_loaded().await?;
        let actor = self.actor.lock().await;
        let actor = actor.as_ref().unwrap();

        self.event_bus.send(Event::SandboxStarted { 
            proposal_id, 
            sandbox_id: final_proposal.id.to_string() 
        })?;

        let exec_result = actor.execute_in_sandbox(&final_proposal).await?;

        self.event_bus.send(Event::SandboxCompleted { 
            proposal_id, 
            sandbox_id: final_proposal.id.to_string(),
            success: exec_result.success,
        })?;

        if !exec_result.success {
            error!(%proposal_id, "Sandbox execution failed");
            self.unload_all_except_passive().await;
            return Ok(CycleResult::Failed(Error::Sandbox("Execution failed".to_string())));
        }

        // 7. Load validator and validate
        self.ensure_validator_loaded().await?;
        let validator = self.validator.lock().await;
        let validator = validator.as_ref().unwrap();

        self.event_bus.send(Event::ValidationStarted { proposal_id })?;

        let validation = validator.validate(&final_proposal, &exec_result).await?;

        self.event_bus.send(Event::ValidationCompleted { result: validation.clone() })?;

        // 8. Handle validation result
        let result = if validation.passed {
            // Promote to production
            self.event_bus.send(Event::PromotionStarted { proposal_id })?;
            self.promote_to_production(&final_proposal).await?;
            self.event_bus.send(Event::PromotionCompleted { proposal_id, success: true })?;

            // Federate sync
            if self.config.federation.enabled {
                self.ensure_federate_loaded().await?;
                let federate = self.federate.lock().await;
                if let Some(federate) = federate.as_ref() {
                    self.event_bus.send(Event::FederationSyncStarted { peer_count: 0 })?;
                    federate.sync(&final_proposal, &validation).await?;
                    self.event_bus.send(Event::FederationSyncCompleted { peer_count: 0, synced_items: 1 })?;
                }
            }

            // Update stats
            self.state.write().await.successful_cycles += 1;
            self.unload_all_except_passive().await;
            CycleResult::Promoted(validation.metrics)
        } else {
            // Rollback
            self.event_bus.send(Event::RollbackStarted { 
                proposal_id, 
                reason: validation.failure.as_ref().map(|f| f.message.clone()).unwrap_or_default()
            })?;
            
            self.rollback(&final_proposal, &validation.failure).await?;
            
            self.event_bus.send(Event::RollbackCompleted { proposal_id, success: true })?;
            
            self.state.write().await.failed_cycles += 1;
            self.unload_all_except_passive().await;
            CycleResult::RolledBack(validation.failure.unwrap_or_else(|| ValidationFailure {
                kind: crate::types::FailureKind::TestFailure,
                message: "Unknown validation failure".to_string(),
                component: crate::types::Component::Custom(0),
                recoverable: true,
            }))
        };

        // Persist state
        self.persist_state().await?;

        let duration = cycle_start.elapsed();
        self.metrics.timing("active_cycle_duration", duration);
        info!(%proposal_id, ?result, duration_ms = %duration.as_millis(), "Active cycle completed");

        Ok(result)
    }

    /// Build context for planner from current state
    async fn build_context(&self, trigger: &Trigger) -> Result<orchestrator_plan::PlannerContext> {
        let state = self.state.read().await;
        
        // Get recent metrics from passive observation
        let metrics = state.passive_metrics.clone();
        
        // Get recent proposals for few-shot
        let recent_proposals = self.state_manager.list_proposals()?
            .into_iter()
            .rev()
            .take(10)
            .collect();

        Ok(orchestrator_plan::PlannerContext {
            trigger: trigger.clone(),
            current_metrics: metrics,
            device_id: state.device_id.clone(),
            recent_proposals,
            active_anomalies: state.active_anomalies.clone(),
            loaded_components: state.loaded_components.clone(),
        })
    }

    /// Promote proposal to production (NixOS switch generation)
    async fn promote_to_production(&self, proposal: &Proposal) -> Result<()> {
        info!(%proposal.id, "Promoting to production");
        // Actor handles the actual nixos-rebuild switch
        // This is a placeholder for any post-promotion steps
        Ok(())
    }

    /// Rollback failed proposal
    async fn rollback(&self, proposal: &Proposal, failure: &Option<ValidationFailure>) -> Result<()> {
        warn!(%proposal.id, "Rolling back");
        // Actor handles rollback via NixOS generation rollback
        Ok(())
    }

    /// Shutdown the orchestrator
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down orchestrator");
        
        self.stop_passive_loop().await;
        self.unload_all_except_passive().await;
        
        // Unload observe last
        if let Some(mut observe) = self.observe.lock().await.take() {
            observe.shutdown().await;
        }
        
        self.persist_state().await?;
        
        info!("Orchestrator shutdown complete");
        Ok(())
    }
}

impl Drop for OrchestratorLoop {
    fn drop(&mut self) {
        // Best effort cleanup
        if let Ok(mut handle) = self.passive_handle.try_lock() {
            if let Some(h) = handle.take() {
                h.abort();
            }
        }
    }
}