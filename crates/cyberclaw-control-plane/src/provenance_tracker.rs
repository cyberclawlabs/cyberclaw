//! Provenance tracking service for execution lineage and audit trails
//!
//! This module provides automatic provenance collection during capability execution,
//! tracking the complete execution→capability→artifact chain for audit and debugging.

use async_trait::async_trait;
use cyberclaw_core::prelude::*;
use cyberclaw_core::provenance::{ProvenanceRecord, RuntimeProvenance, SecurityContext};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::{debug, info};

/// Provenance tracking service
///
/// # Responsibilities
///
/// - Create initial ProvenanceRecord for each execution
/// - Collect RuntimeProvenance from capability execution
/// - Integrate SecurityContext from M3 governance
/// - Persist complete provenance records for audit
///
/// # Integration Points
///
/// - ExecutionService: Creates initial provenance on execution start
/// - CapabilityDispatcher: Collects runtime execution details
/// - SecurityEventStore: Retrieves governance decisions and security events
/// - ArtifactStore: Stores provenance records alongside artifacts
///
/// # Examples
///
/// ```no_run
/// use cyberclaw_control_plane::provenance_tracker::{ProvenanceTracker, InMemoryProvenanceTracker};
/// use cyberclaw_core::prelude::*;
///
/// # async fn example() -> anyhow::Result<()> {
/// let tracker = InMemoryProvenanceTracker::new();
///
/// // Start tracking execution
/// let execution_id = ExecutionId::new();
/// let agent_id = AgentId::new();
/// let trace_id = TraceId::new();
/// tracker.start_tracking(execution_id.clone(), agent_id, None, trace_id, None).await?;
///
/// // Record runtime provenance
/// // ... execute capability ...
///
/// // Finalize provenance
/// let record = tracker.finalize(&execution_id).await?;
/// # Ok(())
/// # }
/// ```
#[async_trait]
pub trait ProvenanceTracker: Send + Sync {
    /// Start tracking a new execution
    ///
    /// Creates initial ProvenanceRecord with execution context.
    async fn start_tracking(
        &self,
        execution_id: ExecutionId,
        agent_id: AgentId,
        case_id: Option<CaseId>,
        trace_id: TraceId,
        node_id: Option<NodeId>,
    ) -> anyhow::Result<ProvenanceId>;

    /// Record artifact production
    ///
    /// Associates an artifact with the current execution provenance.
    async fn record_artifact(
        &self,
        execution_id: &ExecutionId,
        artifact_id: ArtifactId,
    ) -> anyhow::Result<()>;

    /// Record capability invocation
    ///
    /// Adds capability reference to the provenance chain.
    async fn record_capability(
        &self,
        execution_id: &ExecutionId,
        capability_id: CapabilityId,
    ) -> anyhow::Result<()>;

    /// Record connector usage
    ///
    /// Adds connector reference to the provenance chain.
    async fn record_connector(
        &self,
        execution_id: &ExecutionId,
        connector_id: ConnectorId,
    ) -> anyhow::Result<()>;

    /// Record skill usage
    ///
    /// Adds skill reference to the provenance chain.
    async fn record_skill(
        &self,
        execution_id: &ExecutionId,
        skill_id: SkillId,
    ) -> anyhow::Result<()>;

    /// Record runtime provenance
    ///
    /// Captures runtime isolation details (mode, execution results, configuration).
    async fn record_runtime_provenance(
        &self,
        execution_id: &ExecutionId,
        runtime_prov: RuntimeProvenance,
    ) -> anyhow::Result<()>;

    /// Record security context
    ///
    /// Captures governance decision and security events from M3 integration.
    async fn record_security_context(
        &self,
        execution_id: &ExecutionId,
        security_ctx: SecurityContext,
    ) -> anyhow::Result<()>;

    /// Finalize provenance record
    ///
    /// Completes the provenance record and returns it for persistence.
    /// After finalization, no further updates are allowed.
    async fn finalize(&self, execution_id: &ExecutionId) -> anyhow::Result<ProvenanceRecord>;

    /// Get current provenance record (for inspection)
    async fn get(&self, execution_id: &ExecutionId) -> anyhow::Result<Option<ProvenanceRecord>>;

    // ============ Autopilot 专用方法（Iteration-aware methods） ============

    /// Start tracking a new iteration within an execution
    ///
    /// Creates iteration-specific ProvenanceRecord for multi-iteration workflows like Autopilot.
    async fn start_iteration_tracking(
        &self,
        execution_id: ExecutionId,
        iteration: u32,
        step: String,
    ) -> anyhow::Result<ProvenanceId>;

    /// Finalize iteration provenance
    ///
    /// Completes the provenance record for a specific iteration.
    async fn finalize_iteration(
        &self,
        execution_id: &ExecutionId,
        iteration: u32,
    ) -> anyhow::Result<ProvenanceRecord>;

    /// Get provenance records by iteration
    ///
    /// Returns all provenance records for a specific iteration.
    async fn get_by_iteration(
        &self,
        execution_id: &ExecutionId,
        iteration: u32,
    ) -> anyhow::Result<Vec<ProvenanceRecord>>;

    /// Get complete iteration chain
    ///
    /// Returns all iteration provenance records in chronological order.
    async fn get_iteration_chain(
        &self,
        execution_id: &ExecutionId,
    ) -> anyhow::Result<Vec<(u32, ProvenanceRecord)>>;

    /// Record iteration-specific metadata
    ///
    /// Adds custom metadata to an iteration's provenance record.
    async fn record_iteration_metadata(
        &self,
        execution_id: &ExecutionId,
        iteration: u32,
        key: String,
        value: String,
    ) -> anyhow::Result<()>;
}

/// In-memory provenance tracker implementation
///
/// # Storage
///
/// Stores provenance records in memory during execution.
/// Records should be persisted to ArtifactStore after finalization.
///
/// # Concurrency
///
/// Thread-safe via RwLock. Multiple threads can track different executions concurrently.
#[derive(Clone)]
pub struct InMemoryProvenanceTracker {
    /// Active provenance records indexed by execution_id
    records: Arc<RwLock<HashMap<ExecutionId, ProvenanceRecord>>>,

    /// Finalized records (immutable)
    finalized: Arc<RwLock<HashMap<ExecutionId, ProvenanceRecord>>>,

    /// Iteration-level provenance records indexed by (execution_id, iteration)
    iteration_records: Arc<RwLock<HashMap<(ExecutionId, u32), ProvenanceRecord>>>,

    /// Finalized iteration records (immutable)
    iteration_finalized: Arc<RwLock<HashMap<(ExecutionId, u32), ProvenanceRecord>>>,
}

impl InMemoryProvenanceTracker {
    /// Create a new in-memory provenance tracker
    pub fn new() -> Self {
        Self {
            records: Arc::new(RwLock::new(HashMap::new())),
            finalized: Arc::new(RwLock::new(HashMap::new())),
            iteration_records: Arc::new(RwLock::new(HashMap::new())),
            iteration_finalized: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryProvenanceTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ProvenanceTracker for InMemoryProvenanceTracker {
    async fn start_tracking(
        &self,
        execution_id: ExecutionId,
        agent_id: AgentId,
        case_id: Option<CaseId>,
        trace_id: TraceId,
        node_id: Option<NodeId>,
    ) -> anyhow::Result<ProvenanceId> {
        let provenance_id = ProvenanceId::new();

        let record = ProvenanceRecord {
            id: provenance_id.clone(),
            artifact_id: ArtifactId::new(), // Initial sentinel; caller must invoke record_artifact() once the artifact is produced
            execution_id: execution_id.clone(),
            parent_execution_id: None,
            execution_owner_node_id: node_id.clone(),
            node_lineage: node_id.iter().cloned().collect(),
            case_id,
            agent_id,
            skill_refs: vec![],
            connector_refs: vec![],
            capability_refs: vec![],
            trace_id,
            iteration_id: None, // Default to None for non-iterative executions
            runtime_provenance: None,
            security_context: None,
            metadata: HashMap::new(),
        };

        let mut records = self
            .records
            .write()
            .map_err(|_| anyhow::anyhow!("provenance store poisoned"))?;

        if records.contains_key(&execution_id) {
            anyhow::bail!("provenance already started for execution: {}", execution_id);
        }

        records.insert(execution_id.clone(), record);

        debug!(
            "Started provenance tracking: {} for execution: {}",
            provenance_id, execution_id
        );
        Ok(provenance_id)
    }

    async fn record_artifact(
        &self,
        execution_id: &ExecutionId,
        artifact_id: ArtifactId,
    ) -> anyhow::Result<()> {
        let mut records = self
            .records
            .write()
            .map_err(|_| anyhow::anyhow!("provenance store poisoned"))?;

        let record = records.get_mut(execution_id).ok_or_else(|| {
            anyhow::anyhow!("provenance not found for execution: {}", execution_id)
        })?;

        record.artifact_id = artifact_id.clone();
        debug!(
            "Recorded artifact {} for execution {}",
            artifact_id, execution_id
        );
        Ok(())
    }

    async fn record_capability(
        &self,
        execution_id: &ExecutionId,
        capability_id: CapabilityId,
    ) -> anyhow::Result<()> {
        let mut records = self
            .records
            .write()
            .map_err(|_| anyhow::anyhow!("provenance store poisoned"))?;

        let record = records.get_mut(execution_id).ok_or_else(|| {
            anyhow::anyhow!("provenance not found for execution: {}", execution_id)
        })?;

        if !record.capability_refs.contains(&capability_id) {
            record.capability_refs.push(capability_id.clone());
            debug!(
                "Recorded capability {} for execution {}",
                capability_id, execution_id
            );
        }
        Ok(())
    }

    async fn record_connector(
        &self,
        execution_id: &ExecutionId,
        connector_id: ConnectorId,
    ) -> anyhow::Result<()> {
        let mut records = self
            .records
            .write()
            .map_err(|_| anyhow::anyhow!("provenance store poisoned"))?;

        let record = records.get_mut(execution_id).ok_or_else(|| {
            anyhow::anyhow!("provenance not found for execution: {}", execution_id)
        })?;

        if !record.connector_refs.contains(&connector_id) {
            record.connector_refs.push(connector_id.clone());
            debug!(
                "Recorded connector {} for execution {}",
                connector_id, execution_id
            );
        }
        Ok(())
    }

    async fn record_skill(
        &self,
        execution_id: &ExecutionId,
        skill_id: SkillId,
    ) -> anyhow::Result<()> {
        let mut records = self
            .records
            .write()
            .map_err(|_| anyhow::anyhow!("provenance store poisoned"))?;

        let record = records.get_mut(execution_id).ok_or_else(|| {
            anyhow::anyhow!("provenance not found for execution: {}", execution_id)
        })?;

        if !record.skill_refs.contains(&skill_id) {
            record.skill_refs.push(skill_id.clone());
            debug!("Recorded skill {} for execution {}", skill_id, execution_id);
        }
        Ok(())
    }

    async fn record_runtime_provenance(
        &self,
        execution_id: &ExecutionId,
        runtime_prov: RuntimeProvenance,
    ) -> anyhow::Result<()> {
        let mut records = self
            .records
            .write()
            .map_err(|_| anyhow::anyhow!("provenance store poisoned"))?;

        let record = records.get_mut(execution_id).ok_or_else(|| {
            anyhow::anyhow!("provenance not found for execution: {}", execution_id)
        })?;

        record.runtime_provenance = Some(runtime_prov.clone());
        info!(
            "Recorded runtime provenance ({:?}) for execution {}",
            runtime_prov.runtime_mode, execution_id
        );
        Ok(())
    }

    async fn record_security_context(
        &self,
        execution_id: &ExecutionId,
        security_ctx: SecurityContext,
    ) -> anyhow::Result<()> {
        let mut records = self
            .records
            .write()
            .map_err(|_| anyhow::anyhow!("provenance store poisoned"))?;

        let record = records.get_mut(execution_id).ok_or_else(|| {
            anyhow::anyhow!("provenance not found for execution: {}", execution_id)
        })?;

        record.security_context = Some(security_ctx.clone());
        info!(
            "Recorded security context (decision: {}) for execution {}",
            security_ctx.governance_decision, execution_id
        );
        Ok(())
    }

    async fn finalize(&self, execution_id: &ExecutionId) -> anyhow::Result<ProvenanceRecord> {
        // Remove from active records
        let record = {
            let mut records = self
                .records
                .write()
                .map_err(|_| anyhow::anyhow!("provenance store poisoned"))?;

            records.remove(execution_id).ok_or_else(|| {
                anyhow::anyhow!("provenance not found for execution: {}", execution_id)
            })?
        };

        // Move to finalized (immutable)
        {
            let mut finalized = self
                .finalized
                .write()
                .map_err(|_| anyhow::anyhow!("finalized store poisoned"))?;
            finalized.insert(execution_id.clone(), record.clone());
        }

        info!(
            "Finalized provenance {} for execution {}",
            record.id, execution_id
        );
        Ok(record)
    }

    async fn get(&self, execution_id: &ExecutionId) -> anyhow::Result<Option<ProvenanceRecord>> {
        // Check active records first
        {
            let records = self
                .records
                .read()
                .map_err(|_| anyhow::anyhow!("provenance store poisoned"))?;
            if let Some(record) = records.get(execution_id) {
                return Ok(Some(record.clone()));
            }
        }

        // Check finalized records
        let finalized = self
            .finalized
            .read()
            .map_err(|_| anyhow::anyhow!("finalized store poisoned"))?;
        Ok(finalized.get(execution_id).cloned())
    }

    // ============ Autopilot 专用方法实现 ============

    async fn start_iteration_tracking(
        &self,
        execution_id: ExecutionId,
        iteration: u32,
        step: String,
    ) -> anyhow::Result<ProvenanceId> {
        let provenance_id = ProvenanceId::new();

        // Inherit agent_id from the parent execution's provenance record.
        // Check active records first, then finalized records.
        // Falls back to a sentinel value when iteration tracking is used standalone
        // (i.e. without a prior start_tracking call for this execution_id).
        let agent_id = {
            let records = self
                .records
                .read()
                .map_err(|_| anyhow::anyhow!("provenance store poisoned"))?;
            if let Some(parent) = records.get(&execution_id) {
                parent.agent_id.clone()
            } else {
                drop(records);
                let finalized = self
                    .finalized
                    .read()
                    .map_err(|_| anyhow::anyhow!("finalized store poisoned"))?;
                if let Some(parent) = finalized.get(&execution_id) {
                    parent.agent_id.clone()
                } else {
                    // No parent execution record found; use a well-known sentinel so
                    // the field is never a random UUID that cannot be traced back.
                    AgentId::from_string("unset-iteration-agent".to_string())
                        .expect("sentinel is a valid ID string")
                }
            }
        };

        let record = ProvenanceRecord {
            id: provenance_id.clone(),
            artifact_id: ArtifactId::new(), // Initial sentinel; caller must invoke record_artifact() once the artifact is produced
            execution_id: execution_id.clone(),
            parent_execution_id: None,
            execution_owner_node_id: None,
            node_lineage: vec![],
            case_id: None,
            agent_id,
            skill_refs: vec![],
            connector_refs: vec![],
            capability_refs: vec![],
            trace_id: TraceId::new(),
            iteration_id: Some(iteration),
            runtime_provenance: None,
            security_context: None,
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("step".to_string(), step);
                meta
            },
        };

        let mut iteration_records = self
            .iteration_records
            .write()
            .map_err(|_| anyhow::anyhow!("iteration records store poisoned"))?;

        let key = (execution_id.clone(), iteration);
        if iteration_records.contains_key(&key) {
            anyhow::bail!(
                "iteration provenance already started for execution: {}, iteration: {}",
                execution_id,
                iteration
            );
        }

        iteration_records.insert(key, record);

        debug!(
            "Started iteration {} tracking for execution: {}",
            iteration, execution_id
        );
        Ok(provenance_id)
    }

    async fn finalize_iteration(
        &self,
        execution_id: &ExecutionId,
        iteration: u32,
    ) -> anyhow::Result<ProvenanceRecord> {
        let key = (execution_id.clone(), iteration);

        // Remove from active iteration records
        let record = {
            let mut iteration_records = self
                .iteration_records
                .write()
                .map_err(|_| anyhow::anyhow!("iteration records store poisoned"))?;

            iteration_records.remove(&key).ok_or_else(|| {
                anyhow::anyhow!(
                    "iteration provenance not found for execution: {}, iteration: {}",
                    execution_id,
                    iteration
                )
            })?
        };

        // Move to finalized
        {
            let mut iteration_finalized = self
                .iteration_finalized
                .write()
                .map_err(|_| anyhow::anyhow!("iteration finalized store poisoned"))?;
            iteration_finalized.insert(key, record.clone());
        }

        info!(
            "Finalized iteration {} provenance {} for execution {}",
            iteration, record.id, execution_id
        );
        Ok(record)
    }

    async fn get_by_iteration(
        &self,
        execution_id: &ExecutionId,
        iteration: u32,
    ) -> anyhow::Result<Vec<ProvenanceRecord>> {
        let key = (execution_id.clone(), iteration);

        // Check active records first
        {
            let iteration_records = self
                .iteration_records
                .read()
                .map_err(|_| anyhow::anyhow!("iteration records store poisoned"))?;

            if let Some(record) = iteration_records.get(&key) {
                return Ok(vec![record.clone()]);
            }
        }

        // Check finalized records
        let iteration_finalized = self
            .iteration_finalized
            .read()
            .map_err(|_| anyhow::anyhow!("iteration finalized store poisoned"))?;

        if let Some(record) = iteration_finalized.get(&key) {
            Ok(vec![record.clone()])
        } else {
            Ok(vec![])
        }
    }

    async fn get_iteration_chain(
        &self,
        execution_id: &ExecutionId,
    ) -> anyhow::Result<Vec<(u32, ProvenanceRecord)>> {
        let mut chain = Vec::new();

        // Collect from active records
        {
            let iteration_records = self
                .iteration_records
                .read()
                .map_err(|_| anyhow::anyhow!("iteration records store poisoned"))?;

            for ((exec_id, iteration), record) in iteration_records.iter() {
                if exec_id == execution_id {
                    chain.push((*iteration, record.clone()));
                }
            }
        }

        // Collect from finalized records
        {
            let iteration_finalized = self
                .iteration_finalized
                .read()
                .map_err(|_| anyhow::anyhow!("iteration finalized store poisoned"))?;

            for ((exec_id, iteration), record) in iteration_finalized.iter() {
                if exec_id == execution_id {
                    // Avoid duplicates (shouldn't happen, but be safe)
                    if !chain.iter().any(|(i, _)| i == iteration) {
                        chain.push((*iteration, record.clone()));
                    }
                }
            }
        }

        // Sort by iteration number
        chain.sort_by_key(|(iteration, _)| *iteration);

        Ok(chain)
    }

    async fn record_iteration_metadata(
        &self,
        execution_id: &ExecutionId,
        iteration: u32,
        key: String,
        value: String,
    ) -> anyhow::Result<()> {
        let record_key = (execution_id.clone(), iteration);

        let mut iteration_records = self
            .iteration_records
            .write()
            .map_err(|_| anyhow::anyhow!("iteration records store poisoned"))?;

        let record = iteration_records.get_mut(&record_key).ok_or_else(|| {
            anyhow::anyhow!(
                "iteration provenance not found for execution: {}, iteration: {}",
                execution_id,
                iteration
            )
        })?;

        record.metadata.insert(key.clone(), value.clone());
        debug!(
            "Recorded iteration {} metadata: {}={} for execution {}",
            iteration, key, value, execution_id
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_start_tracking() {
        let tracker = InMemoryProvenanceTracker::new();
        let execution_id = ExecutionId::new();
        let agent_id = AgentId::new();
        let trace_id = TraceId::new();

        let prov_id = tracker
            .start_tracking(
                execution_id.clone(),
                agent_id.clone(),
                None,
                trace_id.clone(),
                None,
            )
            .await
            .unwrap();

        let record = tracker.get(&execution_id).await.unwrap().unwrap();
        assert_eq!(record.id, prov_id);
        assert_eq!(record.execution_id, execution_id);
        assert_eq!(record.agent_id, agent_id);
        assert_eq!(record.trace_id, trace_id);
    }

    #[tokio::test]
    async fn test_duplicate_tracking() {
        let tracker = InMemoryProvenanceTracker::new();
        let execution_id = ExecutionId::new();
        let agent_id = AgentId::new();
        let trace_id = TraceId::new();

        tracker
            .start_tracking(
                execution_id.clone(),
                agent_id.clone(),
                None,
                trace_id.clone(),
                None,
            )
            .await
            .unwrap();

        // Duplicate should fail
        let result = tracker
            .start_tracking(execution_id.clone(), agent_id, None, trace_id, None)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_record_capability() {
        let tracker = InMemoryProvenanceTracker::new();
        let execution_id = ExecutionId::new();
        let agent_id = AgentId::new();
        let trace_id = TraceId::new();

        tracker
            .start_tracking(execution_id.clone(), agent_id, None, trace_id, None)
            .await
            .unwrap();

        let cap_id = CapabilityId::from_string("cmd.exec".to_string()).unwrap();
        tracker
            .record_capability(&execution_id, cap_id.clone())
            .await
            .unwrap();

        let record = tracker.get(&execution_id).await.unwrap().unwrap();
        assert_eq!(record.capability_refs.len(), 1);
        assert_eq!(record.capability_refs[0], cap_id);
    }

    #[tokio::test]
    async fn test_record_runtime_provenance() {
        let tracker = InMemoryProvenanceTracker::new();
        let execution_id = ExecutionId::new();
        let agent_id = AgentId::new();
        let trace_id = TraceId::new();

        tracker
            .start_tracking(execution_id.clone(), agent_id, None, trace_id, None)
            .await
            .unwrap();

        let runtime_prov = RuntimeProvenance {
            runtime_mode: RuntimeMode::Process,
            selection_reason: "Medium risk capability".to_string(),
            process_result: Some(ProcessExecutionResult {
                exit_code: 0,
                timed_out: false,
                force_killed: false,
                duration_ms: 1000,
                stdout_digest: Some("sha256:abc123".to_string()),
                stderr_digest: None,
            }),
            container_result: None,
            configuration_digest: Some("sha256:config123".to_string()),
            resource_usage: HashMap::new(),
        };

        tracker
            .record_runtime_provenance(&execution_id, runtime_prov.clone())
            .await
            .unwrap();

        let record = tracker.get(&execution_id).await.unwrap().unwrap();
        assert!(record.runtime_provenance.is_some());
        let stored_prov = record.runtime_provenance.unwrap();
        assert_eq!(stored_prov.runtime_mode, RuntimeMode::Process);
        assert_eq!(stored_prov.selection_reason, "Medium risk capability");
    }

    #[tokio::test]
    async fn test_finalize() {
        let tracker = InMemoryProvenanceTracker::new();
        let execution_id = ExecutionId::new();
        let agent_id = AgentId::new();
        let trace_id = TraceId::new();

        let prov_id = tracker
            .start_tracking(execution_id.clone(), agent_id, None, trace_id, None)
            .await
            .unwrap();

        // Add some data
        let cap_id = CapabilityId::from_string("fs.read".to_string()).unwrap();
        tracker
            .record_capability(&execution_id, cap_id)
            .await
            .unwrap();

        // Finalize
        let record = tracker.finalize(&execution_id).await.unwrap();
        assert_eq!(record.id, prov_id);
        assert_eq!(record.capability_refs.len(), 1);

        // After finalization, active record should be gone
        let active = tracker.get(&execution_id).await.unwrap();
        assert!(active.is_some()); // Still accessible from finalized store

        // But we can't finalize again
        let result = tracker.finalize(&execution_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_record_security_context() {
        let tracker = InMemoryProvenanceTracker::new();
        let execution_id = ExecutionId::new();
        let agent_id = AgentId::new();
        let trace_id = TraceId::new();

        tracker
            .start_tracking(execution_id.clone(), agent_id, None, trace_id, None)
            .await
            .unwrap();

        let sec_ctx = SecurityContext {
            governance_decision: "Allow (Low risk, read-only)".to_string(),
            security_events: vec!["PromptScanned".to_string()],
            secret_refs: vec![],
            policy_violations: vec![],
        };

        tracker
            .record_security_context(&execution_id, sec_ctx.clone())
            .await
            .unwrap();

        let record = tracker.get(&execution_id).await.unwrap().unwrap();
        assert!(record.security_context.is_some());
        let stored_ctx = record.security_context.unwrap();
        assert_eq!(
            stored_ctx.governance_decision,
            "Allow (Low risk, read-only)"
        );
        assert_eq!(stored_ctx.security_events.len(), 1);
    }

    // ============ Iteration-aware Provenance Tests ============

    #[tokio::test]
    async fn test_start_iteration_tracking() {
        let tracker = InMemoryProvenanceTracker::new();
        let execution_id = ExecutionId::new();

        let prov_id = tracker
            .start_iteration_tracking(execution_id.clone(), 1, "Plan".to_string())
            .await
            .unwrap();

        let records = tracker.get_by_iteration(&execution_id, 1).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, prov_id);
        assert_eq!(records[0].iteration_id, Some(1));
        assert_eq!(records[0].metadata.get("step"), Some(&"Plan".to_string()));
    }

    #[tokio::test]
    async fn test_duplicate_iteration_tracking() {
        let tracker = InMemoryProvenanceTracker::new();
        let execution_id = ExecutionId::new();

        tracker
            .start_iteration_tracking(execution_id.clone(), 1, "Plan".to_string())
            .await
            .unwrap();

        // Duplicate iteration should fail
        let result = tracker
            .start_iteration_tracking(execution_id.clone(), 1, "Execute".to_string())
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_multiple_iterations() {
        let tracker = InMemoryProvenanceTracker::new();
        let execution_id = ExecutionId::new();

        // Start 3 iterations
        tracker
            .start_iteration_tracking(execution_id.clone(), 1, "Plan".to_string())
            .await
            .unwrap();
        tracker
            .start_iteration_tracking(execution_id.clone(), 2, "Execute".to_string())
            .await
            .unwrap();
        tracker
            .start_iteration_tracking(execution_id.clone(), 3, "Verify".to_string())
            .await
            .unwrap();

        // Get iteration chain
        let chain = tracker.get_iteration_chain(&execution_id).await.unwrap();
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].0, 1);
        assert_eq!(chain[1].0, 2);
        assert_eq!(chain[2].0, 3);
    }

    #[tokio::test]
    async fn test_finalize_iteration() {
        let tracker = InMemoryProvenanceTracker::new();
        let execution_id = ExecutionId::new();

        let prov_id = tracker
            .start_iteration_tracking(execution_id.clone(), 1, "Plan".to_string())
            .await
            .unwrap();

        // Finalize
        let record = tracker.finalize_iteration(&execution_id, 1).await.unwrap();
        assert_eq!(record.id, prov_id);
        assert_eq!(record.iteration_id, Some(1));

        // After finalization, should still be accessible
        let records = tracker.get_by_iteration(&execution_id, 1).await.unwrap();
        assert_eq!(records.len(), 1);

        // But can't finalize again
        let result = tracker.finalize_iteration(&execution_id, 1).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_record_iteration_metadata() {
        let tracker = InMemoryProvenanceTracker::new();
        let execution_id = ExecutionId::new();

        tracker
            .start_iteration_tracking(execution_id.clone(), 1, "Plan".to_string())
            .await
            .unwrap();

        // Add metadata
        tracker
            .record_iteration_metadata(&execution_id, 1, "agent".to_string(), "planner".to_string())
            .await
            .unwrap();

        tracker
            .record_iteration_metadata(
                &execution_id,
                1,
                "duration_ms".to_string(),
                "1234".to_string(),
            )
            .await
            .unwrap();

        // Verify metadata
        let records = tracker.get_by_iteration(&execution_id, 1).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].metadata.get("agent"),
            Some(&"planner".to_string())
        );
        assert_eq!(
            records[0].metadata.get("duration_ms"),
            Some(&"1234".to_string())
        );
        assert_eq!(records[0].metadata.get("step"), Some(&"Plan".to_string()));
    }

    #[tokio::test]
    async fn test_iteration_metadata_on_nonexistent_iteration() {
        let tracker = InMemoryProvenanceTracker::new();
        let execution_id = ExecutionId::new();

        // Try to add metadata to non-existent iteration
        let result = tracker
            .record_iteration_metadata(&execution_id, 99, "key".to_string(), "value".to_string())
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_by_iteration_empty() {
        let tracker = InMemoryProvenanceTracker::new();
        let execution_id = ExecutionId::new();

        // Query non-existent iteration
        let records = tracker.get_by_iteration(&execution_id, 99).await.unwrap();
        assert_eq!(records.len(), 0);
    }

    #[tokio::test]
    async fn test_get_iteration_chain_empty() {
        let tracker = InMemoryProvenanceTracker::new();
        let execution_id = ExecutionId::new();

        // Query non-existent execution
        let chain = tracker.get_iteration_chain(&execution_id).await.unwrap();
        assert_eq!(chain.len(), 0);
    }

    #[tokio::test]
    async fn test_iteration_chain_sorting() {
        let tracker = InMemoryProvenanceTracker::new();
        let execution_id = ExecutionId::new();

        // Start iterations out of order
        tracker
            .start_iteration_tracking(execution_id.clone(), 3, "Step3".to_string())
            .await
            .unwrap();
        tracker
            .start_iteration_tracking(execution_id.clone(), 1, "Step1".to_string())
            .await
            .unwrap();
        tracker
            .start_iteration_tracking(execution_id.clone(), 2, "Step2".to_string())
            .await
            .unwrap();

        // Chain should be sorted by iteration number
        let chain = tracker.get_iteration_chain(&execution_id).await.unwrap();
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].0, 1);
        assert_eq!(chain[1].0, 2);
        assert_eq!(chain[2].0, 3);
    }

    #[tokio::test]
    async fn test_iteration_provenance_isolation() {
        let tracker = InMemoryProvenanceTracker::new();
        let exec1 = ExecutionId::new();
        let exec2 = ExecutionId::new();

        // Start same iteration number for different executions
        tracker
            .start_iteration_tracking(exec1.clone(), 1, "Exec1-Step1".to_string())
            .await
            .unwrap();
        tracker
            .start_iteration_tracking(exec2.clone(), 1, "Exec2-Step1".to_string())
            .await
            .unwrap();

        // Each execution should only see its own iterations
        let chain1 = tracker.get_iteration_chain(&exec1).await.unwrap();
        let chain2 = tracker.get_iteration_chain(&exec2).await.unwrap();

        assert_eq!(chain1.len(), 1);
        assert_eq!(chain2.len(), 1);
        assert_eq!(
            chain1[0].1.metadata.get("step"),
            Some(&"Exec1-Step1".to_string())
        );
        assert_eq!(
            chain2[0].1.metadata.get("step"),
            Some(&"Exec2-Step1".to_string())
        );
    }

    #[tokio::test]
    async fn test_mixed_iteration_and_regular_provenance() {
        let tracker = InMemoryProvenanceTracker::new();
        let execution_id = ExecutionId::new();
        let agent_id = AgentId::new();
        let trace_id = TraceId::new();

        // Start regular tracking
        tracker
            .start_tracking(
                execution_id.clone(),
                agent_id.clone(),
                None,
                trace_id.clone(),
                None,
            )
            .await
            .unwrap();

        // Start iteration tracking
        tracker
            .start_iteration_tracking(execution_id.clone(), 1, "Plan".to_string())
            .await
            .unwrap();

        // Both should be retrievable
        let regular = tracker.get(&execution_id).await.unwrap();
        assert!(regular.is_some());
        assert_eq!(regular.unwrap().iteration_id, None);

        let iterations = tracker.get_by_iteration(&execution_id, 1).await.unwrap();
        assert_eq!(iterations.len(), 1);
        assert_eq!(iterations[0].iteration_id, Some(1));
    }

    #[tokio::test]
    async fn test_iteration_id_serialization() {
        let tracker = InMemoryProvenanceTracker::new();
        let execution_id = ExecutionId::new();

        tracker
            .start_iteration_tracking(execution_id.clone(), 42, "Test".to_string())
            .await
            .unwrap();

        let records = tracker.get_by_iteration(&execution_id, 42).await.unwrap();
        let record = &records[0];

        // Serialize and deserialize
        let json = serde_json::to_string(record).unwrap();
        let deserialized: ProvenanceRecord = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.iteration_id, Some(42));
        assert_eq!(deserialized.execution_id, execution_id);
    }
}
