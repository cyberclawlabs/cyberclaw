//! RL Training Connector - Export execution traces and deploy model weights
//!
//! Provides RL training data export and weight deployment capabilities.
//!
//! ## Capabilities
//!
//! - `rl.export_traces`: Export execution traces in JSONL format
//! - `rl.deploy_weights`: Deploy model weights (requires governance approval)

use crate::types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, Connector,
    ExecutionStatus as ConnectorExecutionStatus,
};
use cyberclaw_core::manifests::{CapabilityContract, ConnectorRuntime};
use cyberclaw_core::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, error, info};

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// Outcome of a traced execution
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TraceOutcome {
    /// Execution succeeded with a score
    Success { score: f64 },
    /// Execution failed with a reason
    Failure { reason: String },
    /// Execution timed out
    Timeout,
}

/// A single step within an execution trace
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceStep {
    pub action: String,
    pub observation: String,
    pub reward: f64,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// JSONL-exportable execution trace for RL training
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionTrace {
    pub trace_id: String,
    pub agent_id: String,
    pub steps: Vec<TraceStep>,
    pub outcome: TraceOutcome,
    pub total_duration_ms: u64,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Filter criteria for trace queries
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TraceFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_range: Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_reward: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome_type: Option<String>,
}

/// Weight deployment descriptor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightDeployment {
    pub model_id: String,
    pub version: String,
    pub weights_uri: String,
    pub checksum: String,
    pub risk_level: RiskLevel,
}

// ---------------------------------------------------------------------------
// TraceExporter trait
// ---------------------------------------------------------------------------

/// Pluggable backend for trace storage and export
#[async_trait::async_trait]
pub trait TraceExporter: Send + Sync + std::fmt::Debug {
    /// Export traces matching the filter
    async fn export_traces(&self, filter: TraceFilter) -> anyhow::Result<Vec<ExecutionTrace>>;

    /// Export traces in JSONL format
    async fn export_jsonl(&self, filter: TraceFilter) -> anyhow::Result<String>;
}

// ---------------------------------------------------------------------------
// InMemoryTraceStore
// ---------------------------------------------------------------------------

/// Simple in-memory trace store for testing
#[derive(Debug)]
pub struct InMemoryTraceStore {
    store: tokio::sync::RwLock<Vec<ExecutionTrace>>,
}

impl InMemoryTraceStore {
    /// Create a new empty in-memory trace store
    pub fn new() -> Self {
        Self {
            store: tokio::sync::RwLock::new(Vec::new()),
        }
    }

    /// Add a trace to the store
    pub async fn add_trace(&self, trace: ExecutionTrace) {
        let mut store = self.store.write().await;
        store.push(trace);
    }

    fn matches_filter(trace: &ExecutionTrace, filter: &TraceFilter) -> bool {
        if let Some(ref agent_id) = filter.agent_id {
            if trace.agent_id != *agent_id {
                return false;
            }
        }

        if let Some((start, end)) = filter.date_range {
            if trace.created_at < start || trace.created_at > end {
                return false;
            }
        }

        if let Some(min_reward) = filter.min_reward {
            let total_reward: f64 = trace.steps.iter().map(|s| s.reward).sum();
            if total_reward < min_reward {
                return false;
            }
        }

        if let Some(ref outcome_type) = filter.outcome_type {
            let outcome_matches = matches!(
                (&trace.outcome, outcome_type.as_str()),
                (TraceOutcome::Success { .. }, "success")
                    | (TraceOutcome::Failure { .. }, "failure")
                    | (TraceOutcome::Timeout, "timeout")
            );
            if !outcome_matches {
                return false;
            }
        }

        true
    }
}

impl Default for InMemoryTraceStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl TraceExporter for InMemoryTraceStore {
    async fn export_traces(&self, filter: TraceFilter) -> anyhow::Result<Vec<ExecutionTrace>> {
        let store = self.store.read().await;
        let results = store
            .iter()
            .filter(|t| Self::matches_filter(t, &filter))
            .cloned()
            .collect();
        Ok(results)
    }

    async fn export_jsonl(&self, filter: TraceFilter) -> anyhow::Result<String> {
        let traces = self.export_traces(filter).await?;
        let lines: Vec<String> = traces
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(lines.join("\n"))
    }
}

// ---------------------------------------------------------------------------
// Capability I/O types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportTracesInput {
    #[serde(default)]
    pub filter: TraceFilter,
    /// If true, return JSONL string; otherwise return structured traces
    #[serde(default)]
    pub jsonl: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportTracesOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traces: Option<Vec<ExecutionTrace>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jsonl: Option<String>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployWeightsInput {
    pub deployment: WeightDeployment,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployWeightsOutput {
    pub model_id: String,
    pub version: String,
    pub status: String,
}

// ---------------------------------------------------------------------------
// RlTrainingConnector
// ---------------------------------------------------------------------------

/// Connector providing RL training data export and weight deployment capabilities
#[derive(Debug)]
pub struct RlTrainingConnector {
    id: ConnectorId,
    exporter: Arc<dyn TraceExporter>,
    capabilities: Vec<CapabilityContract>,
}

impl RlTrainingConnector {
    /// Create a new RlTrainingConnector with the given trace exporter
    pub fn new(connector_id: &str, exporter: Arc<dyn TraceExporter>) -> Self {
        let id = ConnectorId::from_string(connector_id.to_string())
            .unwrap_or_else(|_| ConnectorId::from_string("rl-training".to_string()).unwrap());
        let capabilities = Self::build_capabilities();

        Self {
            id,
            exporter,
            capabilities,
        }
    }

    /// Create an RlTrainingConnector with the default in-memory trace store
    pub fn in_memory(connector_id: &str) -> Self {
        Self::new(connector_id, Arc::new(InMemoryTraceStore::new()))
    }

    /// Get a reference to the underlying trace exporter
    pub fn exporter(&self) -> &Arc<dyn TraceExporter> {
        &self.exporter
    }

    fn build_capabilities() -> Vec<CapabilityContract> {
        vec![
            CapabilityContract {
                id: "rl.export_traces".to_string(),
                title: "RL Export Traces".to_string(),
                description: Some("Export execution traces in JSONL format".to_string()),
                input_schema: "ExportTracesInput".to_string(),
                output_schema: "ExportTracesOutput".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(60000),
                },
            },
            CapabilityContract {
                id: "rl.deploy_weights".to_string(),
                title: "RL Deploy Weights".to_string(),
                description: Some(
                    "Deploy model weights (requires governance approval)".to_string(),
                ),
                input_schema: "DeployWeightsInput".to_string(),
                output_schema: "DeployWeightsOutput".to_string(),
                risk: RiskLevel::Critical,
                effects: vec![CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(120000),
                },
            },
        ]
    }

    async fn handle_export_traces(
        &self,
        input: ExportTracesInput,
    ) -> anyhow::Result<serde_json::Value> {
        if input.jsonl {
            let jsonl = self.exporter.export_jsonl(input.filter).await?;
            let count = if jsonl.is_empty() {
                0
            } else {
                jsonl.lines().count()
            };
            let output = ExportTracesOutput {
                traces: None,
                jsonl: Some(jsonl),
                count,
            };
            Ok(serde_json::to_value(output)?)
        } else {
            let traces = self.exporter.export_traces(input.filter).await?;
            let count = traces.len();
            let output = ExportTracesOutput {
                traces: Some(traces),
                jsonl: None,
                count,
            };
            Ok(serde_json::to_value(output)?)
        }
    }

    async fn handle_deploy_weights(
        &self,
        input: DeployWeightsInput,
    ) -> anyhow::Result<serde_json::Value> {
        // Weight deployment is a critical operation - in production this would
        // require governance approval. Here we validate and acknowledge.
        let deployment = input.deployment;

        if deployment.model_id.is_empty() {
            anyhow::bail!("model_id must not be empty");
        }
        if deployment.version.is_empty() {
            anyhow::bail!("version must not be empty");
        }
        if deployment.weights_uri.is_empty() {
            anyhow::bail!("weights_uri must not be empty");
        }
        if deployment.checksum.is_empty() {
            anyhow::bail!("checksum must not be empty");
        }

        let output = DeployWeightsOutput {
            model_id: deployment.model_id,
            version: deployment.version,
            status: "accepted".to_string(),
        };
        Ok(serde_json::to_value(output)?)
    }
}

#[async_trait::async_trait]
impl Connector for RlTrainingConnector {
    fn id(&self) -> &ConnectorId {
        &self.id
    }

    fn runtime(&self) -> ConnectorRuntime {
        ConnectorRuntime::Native
    }

    fn capabilities(&self) -> Vec<CapabilityContract> {
        self.capabilities.clone()
    }

    async fn execute(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<CapabilityExecutionResult> {
        debug!(
            "RlTrainingConnector executing capability {} for execution {}",
            request.capability_id, request.execution_id
        );

        let result = match request.capability_id.as_str() {
            "rl.export_traces" => {
                let input: ExportTracesInput = serde_json::from_value(request.input.clone())?;
                self.handle_export_traces(input).await
            }
            "rl.deploy_weights" => {
                let input: DeployWeightsInput = serde_json::from_value(request.input.clone())?;
                self.handle_deploy_weights(input).await
            }
            _ => {
                let error_msg = format!("Unknown capability: {}", request.capability_id);
                error!("{}", error_msg);
                return Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: serde_json::json!({ "error": error_msg.clone() }),
                    status: ConnectorExecutionStatus::Failed,
                    error: Some(error_msg),
                    actual_runtime: None,
                });
            }
        };

        match result {
            Ok(output) => {
                info!("Capability {} executed successfully", request.capability_id);
                Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output,
                    status: ConnectorExecutionStatus::Success,
                    error: None,
                    actual_runtime: None,
                })
            }
            Err(e) => {
                error!("Capability {} failed: {:?}", request.capability_id, e);
                Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: serde_json::json!({ "error": e.to_string() }),
                    status: ConnectorExecutionStatus::Failed,
                    error: Some(e.to_string()),
                    actual_runtime: None,
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_actor() -> ActorRef {
        ActorRef {
            id: ActorId::from_string("test-actor".to_string()).unwrap(),
            actor_type: ActorType::System,
            tenant_id: None,
            home_node_id: None,
            display_name: "Test Actor".to_string(),
        }
    }

    fn test_workspace() -> WorkspaceRef {
        WorkspaceRef {
            id: WorkspaceId::from_string("test-ws".to_string()).unwrap(),
            mode: WorkspaceMode::Ephemeral,
            materialization_mode: None,
            home_node_id: None,
            backing_store: None,
            root: "/tmp/test".to_string(),
            writable_roots: vec![],
        }
    }

    fn test_request(capability: &str, input: serde_json::Value) -> CapabilityExecutionRequest {
        CapabilityExecutionRequest {
            execution_id: ExecutionId::new(),
            trace_id: "test-trace".to_string(),
            actor: test_actor(),
            workspace: test_workspace(),
            connector_id: ConnectorId::from_string("test-rl".to_string()).unwrap(),
            capability_id: CapabilityId::from_string(capability.to_string()).unwrap(),
            input,
        }
    }

    fn sample_trace(agent_id: &str, outcome: TraceOutcome, reward: f64) -> ExecutionTrace {
        ExecutionTrace {
            trace_id: uuid::Uuid::new_v4().to_string(),
            agent_id: agent_id.to_string(),
            steps: vec![
                TraceStep {
                    action: "observe".to_string(),
                    observation: "state-1".to_string(),
                    reward,
                    timestamp: chrono::Utc::now(),
                },
                TraceStep {
                    action: "act".to_string(),
                    observation: "state-2".to_string(),
                    reward: reward * 0.5,
                    timestamp: chrono::Utc::now(),
                },
            ],
            outcome,
            total_duration_ms: 1500,
            created_at: chrono::Utc::now(),
        }
    }

    // -----------------------------------------------------------------------
    // Test 1: Trace creation and storage
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_trace_creation_and_storage() {
        let store = InMemoryTraceStore::new();
        let trace = sample_trace("agent-1", TraceOutcome::Success { score: 0.95 }, 1.0);
        store.add_trace(trace.clone()).await;

        let results = store.export_traces(TraceFilter::default()).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].agent_id, "agent-1");
        assert_eq!(results[0].steps.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Test 2: JSONL export format validation
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_jsonl_export_format() {
        let store = InMemoryTraceStore::new();
        store
            .add_trace(sample_trace(
                "agent-1",
                TraceOutcome::Success { score: 0.9 },
                1.0,
            ))
            .await;
        store
            .add_trace(sample_trace(
                "agent-2",
                TraceOutcome::Failure {
                    reason: "timeout".to_string(),
                },
                0.1,
            ))
            .await;

        let jsonl = store.export_jsonl(TraceFilter::default()).await.unwrap();
        let lines: Vec<&str> = jsonl.lines().collect();
        assert_eq!(lines.len(), 2);

        // Each line must be valid JSON
        for line in &lines {
            let parsed: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(parsed.get("trace_id").is_some());
            assert!(parsed.get("agent_id").is_some());
            assert!(parsed.get("steps").is_some());
        }
    }

    // -----------------------------------------------------------------------
    // Test 3: Filter by agent_id
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_filter_by_agent_id() {
        let store = InMemoryTraceStore::new();
        store
            .add_trace(sample_trace(
                "agent-1",
                TraceOutcome::Success { score: 0.9 },
                1.0,
            ))
            .await;
        store
            .add_trace(sample_trace(
                "agent-2",
                TraceOutcome::Success { score: 0.8 },
                0.5,
            ))
            .await;
        store
            .add_trace(sample_trace("agent-1", TraceOutcome::Timeout, 0.2))
            .await;

        let filter = TraceFilter {
            agent_id: Some("agent-1".to_string()),
            ..Default::default()
        };
        let results = store.export_traces(filter).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|t| t.agent_id == "agent-1"));
    }

    // -----------------------------------------------------------------------
    // Test 4: Filter by date range
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_filter_by_date_range() {
        let store = InMemoryTraceStore::new();
        let now = chrono::Utc::now();

        let mut old_trace = sample_trace("agent-1", TraceOutcome::Success { score: 0.9 }, 1.0);
        old_trace.created_at = now - chrono::Duration::hours(48);
        store.add_trace(old_trace).await;

        let recent_trace = sample_trace("agent-1", TraceOutcome::Success { score: 0.95 }, 1.5);
        store.add_trace(recent_trace).await;

        let filter = TraceFilter {
            date_range: Some((
                now - chrono::Duration::hours(1),
                now + chrono::Duration::hours(1),
            )),
            ..Default::default()
        };
        let results = store.export_traces(filter).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Test 5: Filter by min_reward
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_filter_by_min_reward() {
        let store = InMemoryTraceStore::new();
        // Total reward = 1.0 + 0.5 = 1.5
        store
            .add_trace(sample_trace(
                "agent-1",
                TraceOutcome::Success { score: 0.9 },
                1.0,
            ))
            .await;
        // Total reward = 0.1 + 0.05 = 0.15
        store
            .add_trace(sample_trace(
                "agent-2",
                TraceOutcome::Failure {
                    reason: "bad".to_string(),
                },
                0.1,
            ))
            .await;

        let filter = TraceFilter {
            min_reward: Some(1.0),
            ..Default::default()
        };
        let results = store.export_traces(filter).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].agent_id, "agent-1");
    }

    // -----------------------------------------------------------------------
    // Test 6: Filter by outcome_type
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_filter_by_outcome_type() {
        let store = InMemoryTraceStore::new();
        store
            .add_trace(sample_trace(
                "agent-1",
                TraceOutcome::Success { score: 0.9 },
                1.0,
            ))
            .await;
        store
            .add_trace(sample_trace(
                "agent-2",
                TraceOutcome::Failure {
                    reason: "err".to_string(),
                },
                0.1,
            ))
            .await;
        store
            .add_trace(sample_trace("agent-3", TraceOutcome::Timeout, 0.0))
            .await;

        let filter = TraceFilter {
            outcome_type: Some("failure".to_string()),
            ..Default::default()
        };
        let results = store.export_traces(filter).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].agent_id, "agent-2");
    }

    // -----------------------------------------------------------------------
    // Test 7: Weight deployment struct validation
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_weight_deployment_validation() {
        let connector = RlTrainingConnector::in_memory("test-rl");

        // Valid deployment
        let input = serde_json::json!({
            "deployment": {
                "model_id": "model-v1",
                "version": "1.0.0",
                "weights_uri": "s3://bucket/weights.bin",
                "checksum": "sha256:abc123",
                "risk_level": "critical"
            }
        });
        let request = test_request("rl.deploy_weights", input);
        let result = connector.execute(request).await.unwrap();
        assert_eq!(result.status, ConnectorExecutionStatus::Success);

        let output: DeployWeightsOutput = serde_json::from_value(result.output).unwrap();
        assert_eq!(output.model_id, "model-v1");
        assert_eq!(output.status, "accepted");
    }

    // -----------------------------------------------------------------------
    // Test 8: Weight deployment rejects empty fields
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_weight_deployment_rejects_empty_model_id() {
        let connector = RlTrainingConnector::in_memory("test-rl");

        let input = serde_json::json!({
            "deployment": {
                "model_id": "",
                "version": "1.0.0",
                "weights_uri": "s3://bucket/weights.bin",
                "checksum": "sha256:abc123",
                "risk_level": "critical"
            }
        });
        let request = test_request("rl.deploy_weights", input);
        let result = connector.execute(request).await.unwrap();
        assert_eq!(result.status, ConnectorExecutionStatus::Failed);
        assert!(result.error.unwrap().contains("model_id"));
    }

    // -----------------------------------------------------------------------
    // Test 9: Connector capability listing
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_connector_capabilities() {
        let connector = RlTrainingConnector::in_memory("test-rl");
        assert_eq!(connector.id().as_str(), "test-rl");

        let caps = connector.capabilities();
        assert_eq!(caps.len(), 2);
        assert!(caps.iter().any(|c| c.id == "rl.export_traces"));
        assert!(caps.iter().any(|c| c.id == "rl.deploy_weights"));

        // deploy_weights should be Critical risk
        let deploy_cap = caps.iter().find(|c| c.id == "rl.deploy_weights").unwrap();
        assert_eq!(deploy_cap.risk, RiskLevel::Critical);
    }

    // -----------------------------------------------------------------------
    // Test 10: Empty store export
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_empty_store_export() {
        let store = InMemoryTraceStore::new();

        let results = store.export_traces(TraceFilter::default()).await.unwrap();
        assert!(results.is_empty());

        let jsonl = store.export_jsonl(TraceFilter::default()).await.unwrap();
        assert!(jsonl.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 11: Connector execute export via connector interface
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_connector_execute_export_traces() {
        let store = Arc::new(InMemoryTraceStore::new());
        store
            .add_trace(sample_trace(
                "agent-1",
                TraceOutcome::Success { score: 0.9 },
                1.0,
            ))
            .await;

        let connector = RlTrainingConnector::new("test-rl", store);

        let request = test_request(
            "rl.export_traces",
            serde_json::json!({ "filter": {}, "jsonl": false }),
        );
        let result = connector.execute(request).await.unwrap();
        assert_eq!(result.status, ConnectorExecutionStatus::Success);

        let output: ExportTracesOutput = serde_json::from_value(result.output).unwrap();
        assert_eq!(output.count, 1);
        assert!(output.traces.is_some());
    }

    // -----------------------------------------------------------------------
    // Test 12: Unknown capability returns error
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_connector_unknown_capability() {
        let connector = RlTrainingConnector::in_memory("test-rl");

        let request = test_request("rl.unknown", serde_json::json!({}));
        let result = connector.execute(request).await.unwrap();
        assert_eq!(result.status, ConnectorExecutionStatus::Failed);
        assert!(result.error.is_some());
    }
}
