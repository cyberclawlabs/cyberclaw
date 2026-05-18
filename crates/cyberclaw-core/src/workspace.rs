use crate::ids::{CaseId, NodeId, SessionId, WorkspaceId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkspaceMode {
    Shared,
    Isolated,
    Ephemeral,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMaterializationMode {
    LocalEphemeral,
    LocalPersistent,
    SharedRemote,
    CachedReplica,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceRef {
    pub id: WorkspaceId,
    pub mode: WorkspaceMode,
    pub materialization_mode: Option<WorkspaceMaterializationMode>,
    pub home_node_id: Option<NodeId>,
    pub backing_store: Option<String>,
    pub root: String,
    pub writable_roots: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRef {
    pub id: SessionId,
    pub case_id: Option<CaseId>,
    pub workspace_id: Option<WorkspaceId>,
}
