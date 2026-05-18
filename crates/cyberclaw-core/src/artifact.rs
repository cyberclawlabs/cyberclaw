use crate::ids::ArtifactId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArtifactKind {
    Report,
    Evidence,
    Patch,
    Summary,
    Memory,
    Log,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub id: ArtifactId,
    pub kind: ArtifactKind,
    pub title: String,
    pub uri: String,
    pub content_type: String,
    /// Parent artifact for lineage tracking (e.g. patch chain).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_artifact_id: Option<ArtifactId>,
    /// Base version identifier (e.g. git commit hash) for diff-based artifacts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_version: Option<String>,
}
