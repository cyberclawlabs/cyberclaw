//! # Task Lifecycle Connector
//!
//! A [`Connector`] implementation that exposes task lifecycle management as
//! CyberClaw capabilities. This is the facade layer through which an agent
//! manages spawned tasks via the `Connector→Capability` chain without touching
//! any orchestrator internals directly.
//!
//! ## Capabilities
//!
//! | Capability ID    | Description                                   | Risk   |
//! |------------------|-----------------------------------------------|--------|
//! | `task.create`    | Create a new task record                      | Medium |
//! | `task.list`      | List active/all tasks                         | Low    |
//! | `task.get`       | Fetch a task record by id                     | Low    |
//! | `task.update`    | Update task status or metadata                | Medium |
//! | `task.stop`      | Cancel a running task                         | Medium |
//! | `task.output`    | Fetch captured output for a task              | Low    |
//!
//! ## Design
//!
//! `LocalTaskConnector` accepts an `Arc<dyn TaskBackend>` at construction
//! (EVOLUTION_IDIOMS §1 — Injected Trait Dispatcher). The `InMemoryTaskBackend`
//! provides a v1 in-process store. Real backends (wiring to SubAgentOrchestrator
//! or cyberclaw-store) are Sprint 5+.
//!
//! ## Object model classification
//!
//! This is a **Connector** in the five-object taxonomy. It is NOT a sixth object.
//! Task records exposed here are runtime state, not first-class platform objects.

use crate::types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, Connector,
    ExecutionStatus as ConnectorExecutionStatus,
};
use cyberclaw_core::facade::{CapabilityFacade, FacadeExposure, ToolsetCategory};
use cyberclaw_core::ids::{CapabilityId, ConnectorId};
use cyberclaw_core::manifests::{CapabilityContract, ConnectorRuntime};
use cyberclaw_core::prelude::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info};

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Unique identifier for a managed task.
pub type TaskId = String;

/// Lifecycle status of a task.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Created but not yet started.
    Pending,
    /// Actively executing.
    Running,
    /// Completed successfully.
    Completed,
    /// Terminated with an error.
    Failed,
    /// Cancelled before or during execution.
    Cancelled,
}

/// Full task record returned by `task.get`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskRecord {
    /// Opaque task identifier.
    pub id: TaskId,
    /// Caller-supplied task type label (e.g. `"code_review"`, `"build"`).
    pub task_type: String,
    /// Human-readable description of the task.
    pub description: String,
    /// Caller-supplied input payload (arbitrary JSON).
    pub inputs: serde_json::Value,
    /// Current lifecycle status.
    pub status: TaskStatus,
    /// ISO-8601 creation timestamp.
    pub created_at: String,
    /// ISO-8601 start timestamp, if started.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// ISO-8601 completion timestamp, if finished.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    /// Elapsed milliseconds since start (0 if not started).
    pub elapsed_ms: u64,
    /// Arbitrary key-value metadata.
    pub metadata: HashMap<String, String>,
    /// Captured text output appended by the task executor.
    pub output_text: String,
    /// Structured result payload, set on completion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
}

/// Abbreviated view returned by `task.list`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskSummary {
    /// Opaque task identifier.
    pub id: TaskId,
    /// Caller-supplied task type label.
    pub task_type: String,
    /// Human-readable description.
    pub description: String,
    /// Current lifecycle status.
    pub status: TaskStatus,
    /// Elapsed milliseconds since start (0 if not started).
    pub elapsed_ms: u64,
}

// ---------------------------------------------------------------------------
// Input / output shapes
// ---------------------------------------------------------------------------

/// Input for `task.create`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskCreateInput {
    /// Caller-supplied task type label.
    pub task_type: String,
    /// Human-readable description of the work to do.
    pub description: String,
    /// Arbitrary input payload for the task executor.
    #[serde(default)]
    pub inputs: serde_json::Value,
    /// Optional initial metadata.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Output for `task.create`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskCreateOutput {
    /// Assigned task identifier.
    pub task_id: TaskId,
}

/// Input for `task.list`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskListInput {
    /// If `true`, include only tasks with `Running` or `Pending` status.
    #[serde(default)]
    pub active_only: bool,
}

/// Output for `task.list`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskListOutput {
    /// Ordered list of task summaries (newest first).
    pub tasks: Vec<TaskSummary>,
}

/// Input for `task.get`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskGetInput {
    /// Task identifier to fetch.
    pub task_id: TaskId,
}

/// Output for `task.get`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskGetOutput {
    /// Full task record.
    pub task: TaskRecord,
}

/// Input for `task.update`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskUpdateInput {
    /// Task identifier to update.
    pub task_id: TaskId,
    /// New status to set. `None` = no change.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
    /// Metadata key-value pairs to merge (existing keys are overwritten).
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Output for `task.update`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskUpdateOutput {
    /// `true` if the record was found and modified.
    pub updated: bool,
}

/// Input for `task.stop`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskStopInput {
    /// Task identifier to cancel.
    pub task_id: TaskId,
}

/// Output for `task.stop`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskStopOutput {
    /// `true` if the task was found and transitioned to `Cancelled`.
    pub cancelled: bool,
}

/// Input for `task.output`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskOutputInput {
    /// Task identifier whose output to fetch.
    pub task_id: TaskId,
}

/// Output for `task.output`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskOutputOutput {
    /// Task identifier.
    pub task_id: TaskId,
    /// Captured text output.
    pub output_text: String,
    /// Structured result payload if the task has completed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// TaskBackend trait  (EVOLUTION_IDIOMS §1 — Injected Trait Dispatcher)
// ---------------------------------------------------------------------------

/// Async backend trait for task lifecycle operations.
///
/// `LocalTaskConnector` depends on this trait only — never on any concrete
/// store or orchestrator type. Unit tests use `InMemoryTaskBackend`; Sprint 5+
/// will wire in a persistent backend or SubAgentOrchestrator bridge.
#[async_trait::async_trait]
pub trait TaskBackend: Send + Sync + std::fmt::Debug {
    /// Create a new task and return its assigned `TaskId`.
    async fn create(
        &self,
        task_type: String,
        description: String,
        inputs: serde_json::Value,
        metadata: HashMap<String, String>,
    ) -> anyhow::Result<TaskId>;

    /// Return summaries for all tasks, optionally filtered to active ones.
    async fn list(&self, active_only: bool) -> anyhow::Result<Vec<TaskSummary>>;

    /// Return the full record for a task, or `None` if not found.
    async fn get(&self, task_id: &str) -> anyhow::Result<Option<TaskRecord>>;

    /// Update the status and/or metadata of an existing task.
    ///
    /// Returns `true` if the task was found and modified.
    async fn update(
        &self,
        task_id: &str,
        status: Option<TaskStatus>,
        metadata: HashMap<String, String>,
    ) -> anyhow::Result<bool>;

    /// Transition a task to `Cancelled`, no-op if already terminal.
    ///
    /// Returns `true` if the task was found and cancelled.
    async fn stop(&self, task_id: &str) -> anyhow::Result<bool>;

    /// Return the captured output for a task.
    ///
    /// Returns `None` if the task does not exist.
    async fn output(&self, task_id: &str) -> anyhow::Result<Option<TaskOutputOutput>>;
}

// ---------------------------------------------------------------------------
// InMemoryTaskBackend
// ---------------------------------------------------------------------------

/// In-memory task backend for v1 and unit tests.
///
/// Stores all task records in a `Mutex<HashMap<TaskId, TaskRecord>>`.
/// Thread-safe; suitable for concurrent test use.
#[derive(Debug, Default)]
pub struct InMemoryTaskBackend {
    store: Mutex<HashMap<TaskId, TaskRecord>>,
}

impl InMemoryTaskBackend {
    /// Create an empty backend.
    pub fn new() -> Self {
        Self::default()
    }

    /// Return a snapshot of all records (for test assertions).
    pub fn snapshot(&self) -> Vec<TaskRecord> {
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }
}

fn now_iso() -> String {
    // Use a simple ISO-8601 UTC timestamp. chrono is available in this crate.
    use chrono::Utc;
    Utc::now().to_rfc3339()
}

fn new_task_id() -> TaskId {
    // Use uuid v4 for uniqueness without needing extra deps (uuid is in Cargo.toml).
    uuid::Uuid::new_v4().to_string()
}

#[async_trait::async_trait]
impl TaskBackend for InMemoryTaskBackend {
    async fn create(
        &self,
        task_type: String,
        description: String,
        inputs: serde_json::Value,
        metadata: HashMap<String, String>,
    ) -> anyhow::Result<TaskId> {
        let id = new_task_id();
        let record = TaskRecord {
            id: id.clone(),
            task_type,
            description,
            inputs,
            status: TaskStatus::Pending,
            created_at: now_iso(),
            started_at: None,
            finished_at: None,
            elapsed_ms: 0,
            metadata,
            output_text: String::new(),
            result: None,
        };
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), record);
        Ok(id)
    }

    async fn list(&self, active_only: bool) -> anyhow::Result<Vec<TaskSummary>> {
        let guard = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let mut summaries: Vec<TaskSummary> = guard
            .values()
            .filter(|r| {
                if active_only {
                    matches!(r.status, TaskStatus::Pending | TaskStatus::Running)
                } else {
                    true
                }
            })
            .map(|r| TaskSummary {
                id: r.id.clone(),
                task_type: r.task_type.clone(),
                description: r.description.clone(),
                status: r.status.clone(),
                elapsed_ms: r.elapsed_ms,
            })
            .collect();
        // Newest first — sort by creation time string (ISO-8601 lexicographic).
        summaries.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(summaries)
    }

    async fn get(&self, task_id: &str) -> anyhow::Result<Option<TaskRecord>> {
        Ok(self
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(task_id)
            .cloned())
    }

    async fn update(
        &self,
        task_id: &str,
        status: Option<TaskStatus>,
        metadata: HashMap<String, String>,
    ) -> anyhow::Result<bool> {
        let mut guard = self.store.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(record) = guard.get_mut(task_id) {
            if let Some(new_status) = status {
                record.status = new_status;
            }
            for (k, v) in metadata {
                record.metadata.insert(k, v);
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn stop(&self, task_id: &str) -> anyhow::Result<bool> {
        let mut guard = self.store.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(record) = guard.get_mut(task_id) {
            // No-op if already terminal.
            if matches!(
                record.status,
                TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
            ) {
                return Ok(false);
            }
            record.status = TaskStatus::Cancelled;
            record.finished_at = Some(now_iso());
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn output(&self, task_id: &str) -> anyhow::Result<Option<TaskOutputOutput>> {
        Ok(self
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(task_id)
            .map(|r| TaskOutputOutput {
                task_id: r.id.clone(),
                output_text: r.output_text.clone(),
                result: r.result.clone(),
            }))
    }
}

// ---------------------------------------------------------------------------
// §4 Facade export — real cyberclaw_core::facade types (Phase 2)
// ---------------------------------------------------------------------------

/// Return facade entries for all task capabilities owned by this connector.
///
/// The host binary calls this to populate `BuiltinToolRegistry` with the
/// correct `CapabilityFacade` + `ToolsetCategory::SkillManagement` entries.
pub fn capability_facades() -> Vec<(CapabilityFacade, ToolsetCategory)> {
    let connector_id = ConnectorId::from_string("local_task".to_string()).unwrap();
    vec![
        (
            CapabilityFacade {
                name: "task_create".to_string(),
                description: "Create a new managed task. Returns a task_id for lifecycle tracking."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("task.create".to_string()).unwrap(),
                risk_level: RiskLevel::Medium,
                effects: vec!["write".to_string()],
                read_only: false,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "subject":     { "type": "string", "description": "Short imperative task title (e.g., 'Run tests')." },
                        "description": { "type": "string", "description": "Optional longer description of what needs to be done." },
                        "activeForm":  { "type": "string", "description": "Present continuous form shown while running (e.g., 'Running tests')." },
                        "metadata":    { "type": "object", "description": "Arbitrary structured metadata; opaque to platform." }
                    },
                    "required": ["subject"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::SkillManagement,
        ),
        (
            CapabilityFacade {
                name: "task_list".to_string(),
                description:
                    "List managed tasks. Optionally filter by status and limit result count."
                        .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("task.list".to_string()).unwrap(),
                risk_level: RiskLevel::Low,
                effects: vec!["read".to_string()],
                read_only: true,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "status": { "type": "string", "description": "Optional status filter: pending | running | completed | stopped." },
                        "limit":  { "type": "integer", "description": "Maximum rows to return (default 100).", "minimum": 1 }
                    }
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::SkillManagement,
        ),
        (
            CapabilityFacade {
                name: "task_get".to_string(),
                description: "Fetch the full record for a task by task_id.".to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("task.get".to_string()).unwrap(),
                risk_level: RiskLevel::Low,
                effects: vec!["read".to_string()],
                read_only: true,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string", "description": "Task identifier returned by task_create." }
                    },
                    "required": ["task_id"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::SkillManagement,
        ),
        (
            CapabilityFacade {
                name: "task_update".to_string(),
                description: "Update a task's status and/or metadata by task_id.".to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("task.update".to_string()).unwrap(),
                risk_level: RiskLevel::Medium,
                effects: vec!["write".to_string()],
                read_only: false,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "task_id":     { "type": "string", "description": "Task identifier to update." },
                        "status":      { "type": "string", "description": "New status: pending | running | completed | stopped." },
                        "subject":     { "type": "string", "description": "Replace the task subject." },
                        "description": { "type": "string", "description": "Replace the task description." },
                        "activeForm":  { "type": "string", "description": "Replace the active form label." },
                        "metadata":    { "type": "object", "description": "Replace the metadata blob." }
                    },
                    "required": ["task_id"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::SkillManagement,
        ),
        (
            CapabilityFacade {
                name: "task_stop".to_string(),
                description: "Cancel a running or pending task by task_id.".to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("task.stop".to_string()).unwrap(),
                risk_level: RiskLevel::Medium,
                effects: vec!["write".to_string()],
                read_only: false,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string", "description": "Task identifier to stop." }
                    },
                    "required": ["task_id"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::SkillManagement,
        ),
        (
            CapabilityFacade {
                name: "task_output".to_string(),
                description:
                    "Append captured text output to a task's stream. Returns the new output count."
                        .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("task.output".to_string()).unwrap(),
                risk_level: RiskLevel::Low,
                effects: vec!["read".to_string()],
                read_only: true,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string", "description": "Task identifier to append to." },
                        "output":  { "type": "string", "description": "Output chunk (text or stringified structured result)." }
                    },
                    "required": ["task_id", "output"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::SkillManagement,
        ),
    ]
}

// ---------------------------------------------------------------------------
// LocalTaskConnector
// ---------------------------------------------------------------------------

/// Connector that exposes task lifecycle capabilities to the
/// `Connector→Capability` execution chain.
///
/// Accepts an `Arc<dyn TaskBackend>` at construction so the execution logic
/// is fully testable without any real backend (EVOLUTION_IDIOMS §1).
#[derive(Debug, Clone)]
pub struct LocalTaskConnector {
    id: ConnectorId,
    backend: Arc<dyn TaskBackend>,
}

impl LocalTaskConnector {
    /// Create a new connector wrapping the given backend.
    pub fn new(backend: Arc<dyn TaskBackend>) -> Self {
        let id = ConnectorId::from_string("local_task".to_string())
            .expect("connector id 'local_task' must be valid");
        Self { id, backend }
    }

    /// Convenience constructor using the in-memory backend.
    pub fn in_memory() -> Self {
        Self::new(Arc::new(InMemoryTaskBackend::new()))
    }

    fn build_capabilities() -> Vec<CapabilityContract> {
        vec![
            CapabilityContract {
                id: "task.create".to_string(),
                title: "Create Task".to_string(),
                description: Some("Create a new managed task record. Returns task_id.".to_string()),
                input_schema: "TaskCreateInput".to_string(),
                output_schema: "TaskCreateOutput".to_string(),
                risk: RiskLevel::Medium,
                effects: vec![CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(5_000),
                },
            },
            CapabilityContract {
                id: "task.list".to_string(),
                title: "List Tasks".to_string(),
                description: Some("List managed tasks, optionally filtered to active.".to_string()),
                input_schema: "TaskListInput".to_string(),
                output_schema: "TaskListOutput".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(5_000),
                },
            },
            CapabilityContract {
                id: "task.get".to_string(),
                title: "Get Task".to_string(),
                description: Some("Fetch full task record by task_id.".to_string()),
                input_schema: "TaskGetInput".to_string(),
                output_schema: "TaskGetOutput".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(5_000),
                },
            },
            CapabilityContract {
                id: "task.update".to_string(),
                title: "Update Task".to_string(),
                description: Some("Update task status and/or metadata.".to_string()),
                input_schema: "TaskUpdateInput".to_string(),
                output_schema: "TaskUpdateOutput".to_string(),
                risk: RiskLevel::Medium,
                effects: vec![CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(5_000),
                },
            },
            CapabilityContract {
                id: "task.stop".to_string(),
                title: "Stop Task".to_string(),
                description: Some("Cancel a running or pending task.".to_string()),
                input_schema: "TaskStopInput".to_string(),
                output_schema: "TaskStopOutput".to_string(),
                risk: RiskLevel::Medium,
                effects: vec![CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(5_000),
                },
            },
            CapabilityContract {
                id: "task.output".to_string(),
                title: "Task Output".to_string(),
                description: Some("Fetch captured output and result for a task.".to_string()),
                input_schema: "TaskOutputInput".to_string(),
                output_schema: "TaskOutputOutput".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(5_000),
                },
            },
        ]
    }

    /// Execute `task.create`.
    async fn exec_create(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<serde_json::Value> {
        let input: TaskCreateInput = serde_json::from_value(request.input)?;
        debug!("task.create: type={}", input.task_type);
        let task_id = self
            .backend
            .create(
                input.task_type,
                input.description,
                input.inputs,
                input.metadata,
            )
            .await?;
        info!("task.create: created task_id={}", task_id);
        Ok(serde_json::to_value(TaskCreateOutput { task_id })?)
    }

    /// Execute `task.list`.
    async fn exec_list(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<serde_json::Value> {
        let input: TaskListInput = serde_json::from_value(request.input)?;
        debug!("task.list: active_only={}", input.active_only);
        let tasks = self.backend.list(input.active_only).await?;
        info!("task.list: returned {} tasks", tasks.len());
        Ok(serde_json::to_value(TaskListOutput { tasks })?)
    }

    /// Execute `task.get`.
    async fn exec_get(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<serde_json::Value> {
        let input: TaskGetInput = serde_json::from_value(request.input)?;
        debug!("task.get: task_id={}", input.task_id);
        match self.backend.get(&input.task_id).await? {
            Some(task) => {
                info!("task.get: found task_id={}", input.task_id);
                Ok(serde_json::to_value(TaskGetOutput { task })?)
            }
            None => Err(anyhow::anyhow!("task not found: {}", input.task_id)),
        }
    }

    /// Execute `task.update`.
    async fn exec_update(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<serde_json::Value> {
        let input: TaskUpdateInput = serde_json::from_value(request.input)?;
        debug!("task.update: task_id={}", input.task_id);
        let updated = self
            .backend
            .update(&input.task_id, input.status, input.metadata)
            .await?;
        info!("task.update: task_id={} updated={}", input.task_id, updated);
        Ok(serde_json::to_value(TaskUpdateOutput { updated })?)
    }

    /// Execute `task.stop`.
    async fn exec_stop(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<serde_json::Value> {
        let input: TaskStopInput = serde_json::from_value(request.input)?;
        debug!("task.stop: task_id={}", input.task_id);
        let cancelled = self.backend.stop(&input.task_id).await?;
        info!(
            "task.stop: task_id={} cancelled={}",
            input.task_id, cancelled
        );
        Ok(serde_json::to_value(TaskStopOutput { cancelled })?)
    }

    /// Execute `task.output`.
    async fn exec_output(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<serde_json::Value> {
        let input: TaskOutputInput = serde_json::from_value(request.input)?;
        debug!("task.output: task_id={}", input.task_id);
        match self.backend.output(&input.task_id).await? {
            Some(out) => {
                info!("task.output: found task_id={}", input.task_id);
                Ok(serde_json::to_value(out)?)
            }
            None => Err(anyhow::anyhow!("task not found: {}", input.task_id)),
        }
    }
}

#[async_trait::async_trait]
impl Connector for LocalTaskConnector {
    fn id(&self) -> &ConnectorId {
        &self.id
    }

    fn runtime(&self) -> ConnectorRuntime {
        ConnectorRuntime::Native
    }

    fn capabilities(&self) -> Vec<CapabilityContract> {
        Self::build_capabilities()
    }

    async fn execute(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<CapabilityExecutionResult> {
        debug!(
            "LocalTaskConnector executing capability {} for execution {}",
            request.capability_id, request.execution_id
        );

        let result = match request.capability_id.as_str() {
            "task.create" => self.exec_create(request.clone()).await,
            "task.list" => self.exec_list(request.clone()).await,
            "task.get" => self.exec_get(request.clone()).await,
            "task.update" => self.exec_update(request.clone()).await,
            "task.stop" => self.exec_stop(request.clone()).await,
            "task.output" => self.exec_output(request.clone()).await,
            _ => {
                let msg = format!("Unknown task capability: {}", request.capability_id);
                error!("{}", msg);
                return Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: serde_json::json!({ "error": msg }),
                    status: ConnectorExecutionStatus::Failed,
                    error: Some(msg),
                    actual_runtime: None,
                });
            }
        };

        match result {
            Ok(output) => {
                info!(
                    "LocalTaskConnector: capability {} succeeded",
                    request.capability_id
                );
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
                error!(
                    "LocalTaskConnector: capability {} failed: {:?}",
                    request.capability_id, e
                );
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
    use cyberclaw_core::identity::Identity;
    use cyberclaw_core::workspace::{WorkspaceMode, WorkspaceRef};

    fn mk_workspace() -> WorkspaceRef {
        WorkspaceRef {
            id: WorkspaceId::from_string("task-test".to_string()).unwrap(),
            mode: WorkspaceMode::Ephemeral,
            materialization_mode: None,
            home_node_id: None,
            backing_store: None,
            root: "/tmp/task-test".to_string(),
            writable_roots: vec![],
        }
    }

    fn mk_request(
        connector: &LocalTaskConnector,
        capability: &str,
        input: serde_json::Value,
    ) -> CapabilityExecutionRequest {
        CapabilityExecutionRequest {
            execution_id: ExecutionId::new(),
            trace_id: "trace-task-test".to_string(),
            actor: Identity::System.to_actor_ref(None).unwrap(),
            workspace: mk_workspace(),
            connector_id: connector.id().clone(),
            capability_id: CapabilityId::from_string(capability.to_string()).unwrap(),
            input,
        }
    }

    // Helper: create a connector backed by a shared InMemoryTaskBackend.
    fn make_connector() -> LocalTaskConnector {
        LocalTaskConnector::in_memory()
    }

    // -----------------------------------------------------------------------
    // Test 1: task.create returns a task_id
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn task_create_returns_id() {
        let c = make_connector();
        let req = mk_request(
            &c,
            "task.create",
            serde_json::json!({
                "task_type": "code_review",
                "description": "Review PR #42",
                "inputs": { "pr": 42 }
            }),
        );
        let res = c.execute(req).await.unwrap();
        assert_eq!(
            res.status,
            ConnectorExecutionStatus::Success,
            "create must succeed"
        );
        let out: TaskCreateOutput = serde_json::from_value(res.output).unwrap();
        assert!(!out.task_id.is_empty(), "task_id must not be empty");
    }

    // -----------------------------------------------------------------------
    // Test 2: task.list shows created tasks
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn task_list_shows_created_tasks() {
        let c = make_connector();

        // Create two tasks.
        for i in 0..2u32 {
            let req = mk_request(
                &c,
                "task.create",
                serde_json::json!({
                    "task_type": "build",
                    "description": format!("Build step {}", i),
                    "inputs": {}
                }),
            );
            c.execute(req).await.unwrap();
        }

        let list_req = mk_request(&c, "task.list", serde_json::json!({ "active_only": false }));
        let res = c.execute(list_req).await.unwrap();
        assert_eq!(res.status, ConnectorExecutionStatus::Success);
        let out: TaskListOutput = serde_json::from_value(res.output).unwrap();
        assert_eq!(out.tasks.len(), 2, "both tasks must appear in list");
    }

    // -----------------------------------------------------------------------
    // Test 3: task.get returns full record
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn task_get_returns_full_record() {
        let c = make_connector();

        let create_req = mk_request(
            &c,
            "task.create",
            serde_json::json!({
                "task_type": "deploy",
                "description": "Deploy v1.2.3",
                "inputs": { "version": "1.2.3" }
            }),
        );
        let create_res = c.execute(create_req).await.unwrap();
        let create_out: TaskCreateOutput = serde_json::from_value(create_res.output).unwrap();
        let task_id = create_out.task_id;

        let get_req = mk_request(&c, "task.get", serde_json::json!({ "task_id": task_id }));
        let get_res = c.execute(get_req).await.unwrap();
        assert_eq!(get_res.status, ConnectorExecutionStatus::Success);
        let get_out: TaskGetOutput = serde_json::from_value(get_res.output).unwrap();
        assert_eq!(get_out.task.task_type, "deploy");
        assert_eq!(get_out.task.status, TaskStatus::Pending);
        assert_eq!(
            get_out.task.inputs,
            serde_json::json!({ "version": "1.2.3" })
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: task.update changes status and metadata
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn task_update_changes_status_and_metadata() {
        let c = make_connector();

        let create_req = mk_request(
            &c,
            "task.create",
            serde_json::json!({
                "task_type": "test",
                "description": "Run test suite",
                "inputs": {}
            }),
        );
        let create_res = c.execute(create_req).await.unwrap();
        let create_out: TaskCreateOutput = serde_json::from_value(create_res.output).unwrap();
        let task_id = create_out.task_id.clone();

        // Update to Running + add metadata.
        let update_req = mk_request(
            &c,
            "task.update",
            serde_json::json!({
                "task_id": task_id,
                "status": "running",
                "metadata": { "runner": "ci-01" }
            }),
        );
        let update_res = c.execute(update_req).await.unwrap();
        assert_eq!(update_res.status, ConnectorExecutionStatus::Success);
        let update_out: TaskUpdateOutput = serde_json::from_value(update_res.output).unwrap();
        assert!(update_out.updated);

        // Verify via get.
        let get_req = mk_request(&c, "task.get", serde_json::json!({ "task_id": task_id }));
        let get_res = c.execute(get_req).await.unwrap();
        let get_out: TaskGetOutput = serde_json::from_value(get_res.output).unwrap();
        assert_eq!(get_out.task.status, TaskStatus::Running);
        assert_eq!(
            get_out.task.metadata.get("runner").map(|s| s.as_str()),
            Some("ci-01")
        );
    }

    // -----------------------------------------------------------------------
    // Test 5: task.stop cancels a running task; no-op on terminal
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn task_stop_cancels_task_and_noop_on_terminal() {
        let c = make_connector();

        let create_req = mk_request(
            &c,
            "task.create",
            serde_json::json!({
                "task_type": "migration",
                "description": "DB migration",
                "inputs": {}
            }),
        );
        let create_res = c.execute(create_req).await.unwrap();
        let create_out: TaskCreateOutput = serde_json::from_value(create_res.output).unwrap();
        let task_id = create_out.task_id.clone();

        // Mark running.
        c.execute(mk_request(
            &c,
            "task.update",
            serde_json::json!({ "task_id": task_id, "status": "running" }),
        ))
        .await
        .unwrap();

        // First stop: should cancel.
        let stop_req = mk_request(&c, "task.stop", serde_json::json!({ "task_id": task_id }));
        let stop_res = c.execute(stop_req).await.unwrap();
        assert_eq!(stop_res.status, ConnectorExecutionStatus::Success);
        let stop_out: TaskStopOutput = serde_json::from_value(stop_res.output).unwrap();
        assert!(stop_out.cancelled);

        // Second stop: already terminal, cancelled must be false.
        let stop_req2 = mk_request(&c, "task.stop", serde_json::json!({ "task_id": task_id }));
        let stop_res2 = c.execute(stop_req2).await.unwrap();
        let stop_out2: TaskStopOutput = serde_json::from_value(stop_res2.output).unwrap();
        assert!(
            !stop_out2.cancelled,
            "second stop on terminal task must return false"
        );
    }

    // -----------------------------------------------------------------------
    // Test 6: task.output returns text and result
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn task_output_returns_text_and_result() {
        // Directly manipulate backend to set output_text + result.
        let backend = Arc::new(InMemoryTaskBackend::new());
        let c = LocalTaskConnector::new(backend.clone());

        let task_id = backend
            .create(
                "analysis".to_string(),
                "Analyze codebase".to_string(),
                serde_json::json!({}),
                HashMap::new(),
            )
            .await
            .unwrap();

        // Set output_text and result directly via backend store.
        {
            let mut guard = backend.store.lock().unwrap();
            if let Some(record) = guard.get_mut(&task_id) {
                record.output_text = "Analysis complete: 3 issues found.".to_string();
                record.result = Some(serde_json::json!({ "issues": 3 }));
                record.status = TaskStatus::Completed;
            }
        }

        let out_req = mk_request(&c, "task.output", serde_json::json!({ "task_id": task_id }));
        let out_res = c.execute(out_req).await.unwrap();
        assert_eq!(out_res.status, ConnectorExecutionStatus::Success);
        let out: TaskOutputOutput = serde_json::from_value(out_res.output).unwrap();
        assert_eq!(out.output_text, "Analysis complete: 3 issues found.");
        assert_eq!(out.result, Some(serde_json::json!({ "issues": 3 })));
    }

    // -----------------------------------------------------------------------
    // Test 7: two concurrent creates produce distinct task ids
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn concurrent_creates_produce_distinct_ids() {
        let backend = Arc::new(InMemoryTaskBackend::new());
        let c = Arc::new(LocalTaskConnector::new(backend.clone()));

        let c1 = c.clone();
        let c2 = c.clone();

        let (res1, res2) = tokio::join!(
            async move {
                let req = mk_request(
                    &c1,
                    "task.create",
                    serde_json::json!({
                        "task_type": "alpha",
                        "description": "First",
                        "inputs": {}
                    }),
                );
                c1.execute(req).await.unwrap()
            },
            async move {
                let req = mk_request(
                    &c2,
                    "task.create",
                    serde_json::json!({
                        "task_type": "beta",
                        "description": "Second",
                        "inputs": {}
                    }),
                );
                c2.execute(req).await.unwrap()
            }
        );

        let id1: TaskCreateOutput = serde_json::from_value(res1.output).unwrap();
        let id2: TaskCreateOutput = serde_json::from_value(res2.output).unwrap();
        assert_ne!(
            id1.task_id, id2.task_id,
            "concurrent creates must yield distinct ids"
        );

        // Both should appear in the list.
        let list_req = mk_request(&c, "task.list", serde_json::json!({ "active_only": false }));
        let list_res = c.execute(list_req).await.unwrap();
        let list_out: TaskListOutput = serde_json::from_value(list_res.output).unwrap();
        assert_eq!(list_out.tasks.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Test 8: unknown capability returns Failed status
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn unknown_capability_returns_failed() {
        let c = make_connector();
        let mut req = mk_request(&c, "task.create", serde_json::json!({}));
        req.capability_id = CapabilityId::from_string("task.bogus".to_string()).unwrap();
        let res = c.execute(req).await.unwrap();
        assert_eq!(res.status, ConnectorExecutionStatus::Failed);
    }

    // -----------------------------------------------------------------------
    // Test 9: capability_facades() returns exactly 6 entries
    // -----------------------------------------------------------------------
    #[test]
    fn capability_facades_returns_six_entries() {
        let facades = capability_facades();
        assert_eq!(facades.len(), 6, "must expose exactly 6 task capabilities");
        let ids: Vec<String> = facades
            .iter()
            .map(|(d, _)| d.capability_id.as_str().to_string())
            .collect();
        assert!(ids.iter().any(|id| id == "task.create"));
        assert!(ids.iter().any(|id| id == "task.list"));
        assert!(ids.iter().any(|id| id == "task.get"));
        assert!(ids.iter().any(|id| id == "task.update"));
        assert!(ids.iter().any(|id| id == "task.stop"));
        assert!(ids.iter().any(|id| id == "task.output"));
    }

    // -----------------------------------------------------------------------
    // Test 10: connector exposes 6 capabilities via Connector trait
    // -----------------------------------------------------------------------
    #[test]
    fn connector_exposes_six_capabilities() {
        let c = make_connector();
        let caps = c.capabilities();
        assert_eq!(caps.len(), 6, "connector must expose 6 CapabilityContracts");
        assert_eq!(c.runtime(), ConnectorRuntime::Native);
    }
}
