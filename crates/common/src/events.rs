//! Event bus for inter-component communication

use crossbeam::channel::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use crate::types::*;
use crate::Result;

/// Event types for the orchestrator
#[derive(Debug, Clone)]
pub enum Event {
    // Passive observation events
    MetricSampled { collector: String, metric: String, value: f64, timestamp: DateTime<Utc> },
    AnomalyDetected { anomaly_id: String, severity: AnomalySeverity, description: String, metrics: BTreeMap<String, f64> },
    LogEntry { level: String, target: String, message: String, timestamp: DateTime<Utc> },
    
    // Active cycle events
    CycleStarted { proposal_id: ProposalId, trigger: Trigger },
    ProposalGenerated { proposal: Proposal },
    ApprovalRequested { proposal_id: ProposalId },
    ApprovalReceived { approval: Approval },
    SandboxStarted { proposal_id: ProposalId, sandbox_id: String },
    SandboxCompleted { proposal_id: ProposalId, sandbox_id: String, success: bool },
    ValidationStarted { proposal_id: ProposalId },
    ValidationCompleted { result: ValidationResult },
    PromotionStarted { proposal_id: ProposalId },
    PromotionCompleted { proposal_id: ProposalId, success: bool },
    RollbackStarted { proposal_id: ProposalId, reason: String },
    RollbackCompleted { proposal_id: ProposalId, success: bool },
    FederationSyncStarted { peer_count: usize },
    FederationSyncCompleted { peer_count: usize, synced_items: usize },
    
    // System events
    ComponentLoaded { component: String },
    ComponentUnloaded { component: String },
    MemoryPressure { current_mib: u64, limit_mib: u64 },
    ShutdownRequested,
}

/// Event bus for decoupled communication
#[derive(Clone)]
pub struct EventBus {
    tx: Sender<Event>,
    rx: Arc<Receiver<Event>>,
}

impl EventBus {
    pub fn new(buffer_size: usize) -> Self {
        let (tx, rx) = channel::bounded(buffer_size);
        Self { tx, rx: Arc::new(rx) }
    }

    pub fn unbounded() -> Self {
        let (tx, rx) = channel::unbounded();
        Self { tx, rx: Arc::new(rx) }
    }

    pub fn send(&self, event: Event) -> Result<()> {
        self.tx.send(event).map_err(|e| crate::Error::Internal(format!("Event send failed: {}", e)))
    }

    pub fn try_send(&self, event: Event) -> Result<()> {
        self.tx.try_send(event).map_err(|e| crate::Error::Internal(format!("Event try_send failed: {}", e)))
    }

    pub fn try_recv(&self) -> Result<Event> {
        self.rx.try_recv().map_err(|e| match e {
            TryRecvError::Empty => crate::Error::NotFound("No events available".to_string()),
            TryRecvError::Disconnected => crate::Error::Internal("Event bus disconnected".to_string()),
        })
    }

    pub fn receiver(&self) -> Arc<Receiver<Event>> {
        self.rx.clone()
    }

    pub fn len(&self) -> usize {
        self.rx.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rx.is_empty()
    }
}

/// Subscriber for specific event types
pub struct EventSubscriber {
    rx: Arc<Receiver<Event>>,
    filter: Box<dyn Fn(&Event) -> bool + Send + Sync>,
}

impl EventSubscriber {
    pub fn new(rx: Arc<Receiver<Event>>, filter: impl Fn(&Event) -> bool + Send + Sync + 'static) -> Self {
        Self { rx, filter: Box::new(filter) }
    }

    pub fn try_recv(&self) -> Result<Event> {
        loop {
            match self.rx.try_recv() {
                Ok(event) => {
                    if (self.filter)(&event) {
                        return Ok(event);
                    }
                }
                Err(TryRecvError::Empty) => return Err(crate::Error::NotFound("No matching events".to_string())),
                Err(TryRecvError::Disconnected) => return Err(crate::Error::Internal("Event bus disconnected".to_string())),
            }
        }
    }

    pub async fn recv_async(&self) -> Result<Event> {
        // For async contexts, we'd need a different approach
        // This is a placeholder - in real implementation, use tokio::sync::mpsc or similar
        self.try_recv()
    }
}

impl EventBus {
    pub fn subscribe(&self, filter: impl Fn(&Event) -> bool + Send + Sync + 'static) -> EventSubscriber {
        EventSubscriber::new(self.rx.clone(), filter)
    }

    pub fn subscribe_all(&self) -> EventSubscriber {
        EventSubscriber::new(self.rx.clone(), |_| true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    #[test]
    fn test_event_bus() {
        let bus = EventBus::new(100);
        let sub = bus.subscribe_all();
        
        let event = Event::MetricSampled {
            collector: "test".to_string(),
            metric: "cpu".to_string(),
            value: 50.0,
            timestamp: Utc::now(),
        };
        
        bus.send(event.clone()).unwrap();
        let received = sub.try_recv().unwrap();
        
        match received {
            Event::MetricSampled { metric, value, .. } => {
                assert_eq!(metric, "cpu");
                assert_eq!(value, 50.0);
            }
            _ => panic!("Wrong event type"),
        }
    }
}