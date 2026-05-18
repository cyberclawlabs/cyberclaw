use crate::ids::WorkflowId;
use crate::manifests::PackageRef;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRef {
    pub id: WorkflowId,
    pub version: String,
    pub source: PackageRef,
}
