use crate::artifact::ArtifactRef;
use crate::enums::Priority;
use crate::execution::{ExecutionBudget, ExecutionMetrics, ExecutionStatus};
use crate::ids::{AgentId, ExecutionId};
use crate::task::Task;
use crate::workflow::WorkflowRef;
use crate::workspace::{SessionRef, WorkspaceMode, WorkspaceRef};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContextPack {
    pub artifact_refs: Vec<ArtifactRef>,
    pub memory_refs: Vec<ArtifactRef>,
    pub policy_refs: Vec<String>,
    pub workflow_ref: Option<WorkflowRef>,
    pub session: Option<SessionRef>,
    pub workspace: Option<WorkspaceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnRequest {
    pub parent_execution_id: ExecutionId,
    pub requesting_agent_id: AgentId,
    pub target_agent_id: AgentId,
    pub task: Task,
    pub context: ContextPack,
    pub budget: ExecutionBudget,
    pub workspace_mode: WorkspaceMode,
    pub priority: Priority,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultEnvelope {
    pub execution_id: ExecutionId,
    pub status: ExecutionStatus,
    pub summary: String,
    pub artifacts: Vec<ArtifactRef>,
    pub evidence_refs: Vec<ArtifactRef>,
    pub output: serde_json::Value,
    pub metrics: ExecutionMetrics,
    pub failure_reason: Option<String>,
}
