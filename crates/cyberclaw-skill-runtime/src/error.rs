//! Error types for the skill runtime.

use cyberclaw_core::ids::SkillId;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SkillRuntimeError {
    #[error("skill not found: {0}")]
    SkillNotFound(SkillId),

    #[error("skill invocation failed for '{skill_id}': {source}")]
    InvocationFailed {
        skill_id: SkillId,
        #[source]
        source: anyhow::Error,
    },

    #[error("invalid input for skill '{skill_id}': {message}")]
    InvalidInput { skill_id: SkillId, message: String },

    #[error("skill execution timed out for '{skill_id}' after {timeout_secs}s")]
    ExecutionTimeout {
        skill_id: SkillId,
        timeout_secs: u64,
    },

    #[error("failed to load skill from '{path}': {source}")]
    LoadFailed {
        path: PathBuf,
        #[source]
        source: anyhow::Error,
    },

    #[error("unsupported skill format at '{path}'")]
    UnsupportedFormat { path: PathBuf },

    #[error("invalid skill manifest at '{path}': {message}")]
    InvalidManifest { path: PathBuf, message: String },

    #[error("skill metadata parsing error: {0}")]
    MetadataParseError(String),

    #[error("file system error: {0}")]
    FileSystemError(#[from] std::io::Error),
}
