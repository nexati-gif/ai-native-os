//! Persistent state management using redb (embedded ACID database)

use std::sync::Arc;
use redb::{Database, TableDefinition, ReadableTable, WriteTransaction};
use serde::{Serialize, Deserialize, de::DeserializeOwned};
use crate::{Error, Result};
use crate::types::*;

/// Table definitions
const STATE_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("state");
const PROPOSALS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("proposals");
const VALIDATIONS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("validations");
const APPROVALS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("approvals");
const METRICS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("metrics");
const FEDERATION_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("federation");
const MODEL_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("model_state");

/// Persistent state manager
pub struct StateManager {
    db: Arc<Database>,
}

impl StateManager {
    pub fn new(path: &str) -> Result<Self> {
        let db = Database::create(path)?;
        let manager = Self { db: Arc::new(db) };
        manager.init_tables()?;
        Ok(manager)
    }

    fn init_tables(&self) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            write_txn.open_table(STATE_TABLE)?;
            write_txn.open_table(PROPOSALS_TABLE)?;
            write_txn.open_table(VALIDATIONS_TABLE)?;
            write_txn.open_table(APPROVALS_TABLE)?;
            write_txn.open_table(METRICS_TABLE)?;
            write_txn.open_table(FEDERATION_TABLE)?;
            write_txn.open_table(MODEL_TABLE)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    /// Store a serializable value
    pub fn set<K: AsRef<str>, V: Serialize>(&self, table: &TableDefinition<&str, &[u8]>, key: K, value: &V) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(*table)?;
            let encoded = bincode::serialize(value)?;
            table.insert(key.as_ref(), encoded.as_slice())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    /// Get a deserializable value
    pub fn get<K: AsRef<str>, V: DeserializeOwned>(&self, table: &TableDefinition<&str, &[u8]>, key: K) -> Result<Option<V>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(*table)?;
        if let Some(value) = table.get(key.as_ref())? {
            let decoded: V = bincode::deserialize(value.value())?;
            Ok(Some(decoded))
        } else {
            Ok(None)
        }
    }

    /// Delete a key
    pub fn delete<K: AsRef<str>>(&self, table: &TableDefinition<&str, &[u8]>, key: K) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(*table)?;
            table.remove(key.as_ref())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    /// List all keys in a table
    pub fn list_keys(&self, table: &TableDefinition<&str, &[u8]>) -> Result<Vec<String>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(*table)?;
        let mut keys = Vec::new();
        for entry in table.iter()? {
            let (key, _) = entry?;
            keys.push(key.value().to_string());
        }
        Ok(keys)
    }

    // Convenience methods for specific types

    pub fn set_state<V: Serialize>(&self, key: &str, value: &V) -> Result<()> {
        self.set(STATE_TABLE, key, value)
    }

    pub fn get_state<V: DeserializeOwned>(&self, key: &str) -> Result<Option<V>> {
        self.get(STATE_TABLE, key)
    }

    pub fn set_proposal(&self, proposal: &Proposal) -> Result<()> {
        self.set(PROPOSALS_TABLE, &proposal.id.to_string(), proposal)
    }

    pub fn get_proposal(&self, id: ProposalId) -> Result<Option<Proposal>> {
        self.get(PROPOSALS_TABLE, &id.to_string())
    }

    pub fn list_proposals(&self) -> Result<Vec<Proposal>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(PROPOSALS_TABLE)?;
        let mut proposals = Vec::new();
        for entry in table.iter()? {
            let (_, value) = entry?;
            let proposal: Proposal = bincode::deserialize(value.value())?;
            proposals.push(proposal);
        }
        Ok(proposals)
    }

    pub fn set_validation(&self, validation: &ValidationResult) -> Result<()> {
        self.set(VALIDATIONS_TABLE, &validation.proposal_id.to_string(), validation)
    }

    pub fn get_validation(&self, id: ProposalId) -> Result<Option<ValidationResult>> {
        self.get(VALIDATIONS_TABLE, &id.to_string())
    }

    pub fn set_approval(&self, approval: &Approval) -> Result<()> {
        self.set(APPROVALS_TABLE, &approval.proposal_id.to_string(), approval)
    }

    pub fn get_approval(&self, id: ProposalId) -> Result<Option<Approval>> {
        self.get(APPROVALS_TABLE, &id.to_string())
    }

    pub fn set_metric(&self, key: &str, value: &f64) -> Result<()> {
        self.set(METRICS_TABLE, key, value)
    }

    pub fn get_metric(&self, key: &str) -> Result<Option<f64>> {
        self.get(METRICS_TABLE, key)
    }

    pub fn set_federation_state(&self, state: &FederatedState) -> Result<()> {
        self.set(FEDERATION_TABLE, "current", state)
    }

    pub fn get_federation_state(&self) -> Result<Option<FederatedState>> {
        self.get(FEDERATION_TABLE, "current")
    }

    pub fn set_model_state(&self, key: &str, value: &impl Serialize) -> Result<()> {
        self.set(MODEL_TABLE, key, value)
    }

    pub fn get_model_state<V: DeserializeOwned>(&self, key: &str) -> Result<Option<V>> {
        self.get(MODEL_TABLE, key)
    }

    /// Compact the database (reclaim space)
    pub fn compact(&self) -> Result<()> {
        self.db.compact()?;
        Ok(())
    }

    /// Get database size in bytes
    pub fn size(&self) -> Result<u64> {
        Ok(self.db.size()?)
    }
}

/// Orchestrator runtime state (in-memory, persisted periodically)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OrchestratorState {
    pub device_id: DeviceId,
    pub started_at: DateTime<Utc>,
    pub last_cycle_at: Option<DateTime<Utc>>,
    pub total_cycles: u64,
    pub successful_cycles: u64,
    pub failed_cycles: u64,
    pub current_proposal: Option<ProposalId>,
    pub passive_metrics: BTreeMap<String, f64>,
    pub active_anomalies: Vec<String>,
    pub loaded_components: Vec<String>,
    pub memory_usage_mib: u64,
    pub gpu_memory_usage_mib: u64,
}

impl OrchestratorState {
    pub fn new(device_id: DeviceId) -> Self {
        Self {
            device_id,
            started_at: Utc::now(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use crate::types::*;

    #[test]
    fn test_state_manager() {
        let tmp = NamedTempFile::new().unwrap();
        let manager = StateManager::new(tmp.path().to_str().unwrap()).unwrap();
        
        let state = OrchestratorState::new(DeviceId::new("test"));
        manager.set_state("current", &state).unwrap();
        
        let retrieved: Option<OrchestratorState> = manager.get_state("current").unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().device_id.0, "test");
    }
}