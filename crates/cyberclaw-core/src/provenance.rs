use crate::ids::{
    AgentId, ArtifactId, CapabilityId, CaseId, ConnectorId, ExecutionId, NodeId, ProvenanceId,
    SkillId, TraceId,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Base provenance record tracking execution→capability→artifact chain
///
/// # Purpose
///
/// Provides complete lineage tracking for audit, debugging, and security analysis:
/// - What was executed (capability_refs)
/// - Who executed it (agent_id, connector_refs)
/// - What was produced (artifact_id)
/// - When and where (execution_id, node_lineage)
/// - Why (case_id, parent_execution_id)
///
/// # Examples
///
/// ```
/// use cyberclaw_core::provenance::ProvenanceRecord;
/// use cyberclaw_core::ids::*;
///
/// let record = ProvenanceRecord {
///     id: ProvenanceId::new(),
///     artifact_id: ArtifactId::new(),
///     execution_id: ExecutionId::new(),
///     parent_execution_id: None,
///     execution_owner_node_id: Some(NodeId::new()),
///     node_lineage: vec![NodeId::new()],
///     case_id: None,
///     agent_id: AgentId::new(),
///     skill_refs: vec![],
///     connector_refs: vec![],
///     capability_refs: vec![],
///     trace_id: TraceId::new(),
///     iteration_id: None,
///     runtime_provenance: None,
///     security_context: None,
///     metadata: std::collections::HashMap::new(),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceRecord {
    /// Unique identifier for this provenance record
    pub id: ProvenanceId,

    /// Artifact produced by this execution
    pub artifact_id: ArtifactId,

    /// Execution that produced this artifact
    pub execution_id: ExecutionId,

    /// Parent execution (for nested/composed operations)
    pub parent_execution_id: Option<ExecutionId>,

    /// Node that owned the execution
    pub execution_owner_node_id: Option<NodeId>,

    /// Complete node lineage for distributed execution tracking
    pub node_lineage: Vec<NodeId>,

    /// Case context (for case-based workflows)
    pub case_id: Option<CaseId>,

    /// Agent that initiated the execution
    pub agent_id: AgentId,

    /// Skills involved in producing this artifact
    pub skill_refs: Vec<SkillId>,

    /// Connectors used during execution
    pub connector_refs: Vec<ConnectorId>,

    /// Capabilities invoked during execution
    pub capability_refs: Vec<CapabilityId>,

    /// Distributed tracing ID for correlation
    pub trace_id: TraceId,

    /// Iteration ID for Autopilot multi-iteration tracking (None for non-iterative executions)
    pub iteration_id: Option<u32>,

    /// Runtime-specific provenance (M4 extension)
    pub runtime_provenance: Option<RuntimeProvenance>,

    /// Security context for audit (M3 integration)
    pub security_context: Option<SecurityContext>,

    /// Extensible metadata for custom tracking
    pub metadata: HashMap<String, String>,
}

/// Runtime-specific provenance for capability execution (M4 extension)
///
/// # Purpose
///
/// Captures runtime isolation details for security analysis and debugging:
/// - What runtime mode was used (Native, Process, Container)
/// - What configuration was applied (timeout, limits, env)
/// - How it executed (exit code, duration, resource usage)
/// - Why this runtime was chosen (selection reason)
///
/// # Security Implications
///
/// This information is critical for:
/// - Post-execution security analysis
/// - Incident response and forensics
/// - Compliance audits
/// - Debugging runtime-related issues
///
/// # Examples
///
/// ```
/// use cyberclaw_core::provenance::{RuntimeProvenance, RuntimeMode, ProcessExecutionResult};
///
/// let runtime_prov = RuntimeProvenance {
///     runtime_mode: RuntimeMode::Process,
///     selection_reason: "Medium risk capability requires process isolation".to_string(),
///     process_result: Some(ProcessExecutionResult {
///         exit_code: 0,
///         timed_out: false,
///         force_killed: false,
///         duration_ms: 1234,
///         stdout_digest: Some("sha256:abc123...".to_string()),
///         stderr_digest: None,
///     }),
///     container_result: None,
///     configuration_digest: Some("sha256:def456...".to_string()),
///     resource_usage: std::collections::HashMap::new(),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeProvenance {
    /// Runtime mode used for execution
    pub runtime_mode: RuntimeMode,

    /// Why this runtime was selected (for audit/debugging)
    pub selection_reason: String,

    /// Process runtime execution result (if runtime_mode == Process)
    pub process_result: Option<ProcessExecutionResult>,

    /// Container runtime execution result (if runtime_mode == Container)
    pub container_result: Option<ContainerExecutionResult>,

    /// Configuration digest (for verifying runtime policy compliance)
    pub configuration_digest: Option<String>,

    /// Resource usage metrics
    pub resource_usage: HashMap<String, String>,
}

/// Runtime mode for capability execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RuntimeMode {
    /// Direct execution in current process (no isolation)
    Native,

    /// Subprocess execution with process isolation
    Process,

    /// Container execution with full isolation
    Container,
}

/// Process execution result summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessExecutionResult {
    /// Exit code from subprocess
    pub exit_code: i32,

    /// Whether execution timed out
    pub timed_out: bool,

    /// Whether SIGKILL was needed (after SIGTERM grace period)
    pub force_killed: bool,

    /// Execution duration in milliseconds
    pub duration_ms: u64,

    /// SHA-256 digest of stdout (for large outputs, store digest only)
    pub stdout_digest: Option<String>,

    /// SHA-256 digest of stderr
    pub stderr_digest: Option<String>,
}

/// Container execution result summary (placeholder for M4.3)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerExecutionResult {
    /// Container ID
    pub container_id: String,

    /// Exit code from container
    pub exit_code: i32,

    /// Whether execution timed out
    pub timed_out: bool,

    /// Execution duration in milliseconds
    pub duration_ms: u64,

    /// Container image digest
    pub image_digest: String,

    /// Network mode used
    pub network_mode: String,
}

/// Security context for provenance (M3 integration)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityContext {
    /// Governance decision that allowed execution
    pub governance_decision: String,

    /// Security events generated during execution
    pub security_events: Vec<String>,

    /// Secret references used (NOT the secrets themselves)
    pub secret_refs: Vec<String>,

    /// Policy violations detected (if any)
    pub policy_violations: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provenance_record_creation() {
        let record = ProvenanceRecord {
            id: ProvenanceId::new(),
            artifact_id: ArtifactId::new(),
            execution_id: ExecutionId::new(),
            parent_execution_id: None,
            execution_owner_node_id: Some(NodeId::new()),
            node_lineage: vec![NodeId::new()],
            case_id: None,
            agent_id: AgentId::new(),
            skill_refs: vec![],
            connector_refs: vec![],
            capability_refs: vec![],
            trace_id: TraceId::new(),
            iteration_id: None,
            runtime_provenance: None,
            security_context: None,
            metadata: HashMap::new(),
        };

        assert!(record.iteration_id.is_none());
        assert!(record.runtime_provenance.is_none());
        assert!(record.security_context.is_none());
    }

    #[test]
    fn test_runtime_provenance_process() {
        let runtime_prov = RuntimeProvenance {
            runtime_mode: RuntimeMode::Process,
            selection_reason: "Medium risk capability requires process isolation".to_string(),
            process_result: Some(ProcessExecutionResult {
                exit_code: 0,
                timed_out: false,
                force_killed: false,
                duration_ms: 1234,
                stdout_digest: Some("sha256:abc123".to_string()),
                stderr_digest: None,
            }),
            container_result: None,
            configuration_digest: Some("sha256:def456".to_string()),
            resource_usage: HashMap::new(),
        };

        assert_eq!(runtime_prov.runtime_mode, RuntimeMode::Process);
        assert!(runtime_prov.process_result.is_some());
        assert!(runtime_prov.container_result.is_none());

        let process_result = runtime_prov.process_result.unwrap();
        assert_eq!(process_result.exit_code, 0);
        assert!(!process_result.timed_out);
        assert!(!process_result.force_killed);
        assert_eq!(process_result.duration_ms, 1234);
    }

    #[test]
    fn test_runtime_provenance_container() {
        let runtime_prov = RuntimeProvenance {
            runtime_mode: RuntimeMode::Container,
            selection_reason: "High risk capability requires full isolation".to_string(),
            process_result: None,
            container_result: Some(ContainerExecutionResult {
                container_id: "container123".to_string(),
                exit_code: 0,
                timed_out: false,
                duration_ms: 2345,
                image_digest: "sha256:image123".to_string(),
                network_mode: "none".to_string(),
            }),
            configuration_digest: Some("sha256:config789".to_string()),
            resource_usage: HashMap::new(),
        };

        assert_eq!(runtime_prov.runtime_mode, RuntimeMode::Container);
        assert!(runtime_prov.container_result.is_some());
        assert!(runtime_prov.process_result.is_none());
    }

    #[test]
    fn test_security_context() {
        let sec_ctx = SecurityContext {
            governance_decision: "Allow (Low risk, read-only)".to_string(),
            security_events: vec!["PromptScanned".to_string()],
            secret_refs: vec!["secret_123".to_string()],
            policy_violations: vec![],
        };

        assert_eq!(sec_ctx.security_events.len(), 1);
        assert_eq!(sec_ctx.secret_refs.len(), 1);
        assert!(sec_ctx.policy_violations.is_empty());
    }

    #[test]
    fn test_full_provenance_record_with_runtime() {
        let runtime_prov = RuntimeProvenance {
            runtime_mode: RuntimeMode::Process,
            selection_reason: "Policy-based runtime selection".to_string(),
            process_result: Some(ProcessExecutionResult {
                exit_code: 0,
                timed_out: false,
                force_killed: false,
                duration_ms: 987,
                stdout_digest: Some("sha256:stdout".to_string()),
                stderr_digest: None,
            }),
            container_result: None,
            configuration_digest: Some("sha256:config".to_string()),
            resource_usage: HashMap::new(),
        };

        let sec_ctx = SecurityContext {
            governance_decision: "Allow (governance approved)".to_string(),
            security_events: vec![],
            secret_refs: vec![],
            policy_violations: vec![],
        };

        let record = ProvenanceRecord {
            id: ProvenanceId::new(),
            artifact_id: ArtifactId::new(),
            execution_id: ExecutionId::new(),
            parent_execution_id: None,
            execution_owner_node_id: Some(NodeId::new()),
            node_lineage: vec![],
            case_id: None,
            agent_id: AgentId::new(),
            skill_refs: vec![],
            connector_refs: vec![],
            capability_refs: vec![],
            trace_id: TraceId::new(),
            iteration_id: None,
            runtime_provenance: Some(runtime_prov),
            security_context: Some(sec_ctx),
            metadata: HashMap::new(),
        };

        assert!(record.runtime_provenance.is_some());
        assert!(record.security_context.is_some());

        let runtime = record.runtime_provenance.unwrap();
        assert_eq!(runtime.runtime_mode, RuntimeMode::Process);
    }

    #[test]
    fn test_serde_round_trip_provenance_record() {
        let record = ProvenanceRecord {
            id: ProvenanceId::new(),
            artifact_id: ArtifactId::new(),
            execution_id: ExecutionId::new(),
            parent_execution_id: None,
            execution_owner_node_id: Some(NodeId::new()),
            node_lineage: vec![],
            case_id: None,
            agent_id: AgentId::new(),
            skill_refs: vec![],
            connector_refs: vec![],
            capability_refs: vec![],
            trace_id: TraceId::new(),
            iteration_id: None,
            runtime_provenance: None,
            security_context: None,
            metadata: HashMap::new(),
        };

        let json = serde_json::to_string(&record).unwrap();
        let deserialized: ProvenanceRecord = serde_json::from_str(&json).unwrap();

        assert_eq!(record.id, deserialized.id);
        assert_eq!(record.execution_id, deserialized.execution_id);
    }

    #[test]
    fn test_runtime_mode_serialization() {
        let modes = vec![
            RuntimeMode::Native,
            RuntimeMode::Process,
            RuntimeMode::Container,
        ];

        for mode in modes {
            let json = serde_json::to_string(&mode).unwrap();
            let deserialized: RuntimeMode = serde_json::from_str(&json).unwrap();
            assert_eq!(mode, deserialized);
        }
    }

    #[test]
    fn test_process_execution_result_timeout() {
        let result = ProcessExecutionResult {
            exit_code: 143, // SIGTERM
            timed_out: true,
            force_killed: false,
            duration_ms: 30000,
            stdout_digest: None,
            stderr_digest: None,
        };

        assert!(result.timed_out);
        assert!(!result.force_killed);
        assert_eq!(result.exit_code, 143);
    }

    #[test]
    fn test_process_execution_result_force_killed() {
        let result = ProcessExecutionResult {
            exit_code: 137, // SIGKILL
            timed_out: true,
            force_killed: true,
            duration_ms: 32000,
            stdout_digest: None,
            stderr_digest: None,
        };

        assert!(result.timed_out);
        assert!(result.force_killed);
        assert_eq!(result.exit_code, 137);
    }
}
