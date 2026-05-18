//! Plugin runtime error types

use thiserror::Error;

/// Plugin runtime error types
#[derive(Error, Debug)]
pub enum PluginError {
    #[error("Plugin not found: {id}")]
    NotFound { id: String },

    #[error("Plugin loading failed: {message}")]
    LoadFailed { message: String },

    #[error("Plugin execution failed: {message}")]
    ExecutionFailed { message: String },

    #[error("Invalid plugin manifest: {message}")]
    InvalidManifest { message: String },

    #[error("Permission denied: {operation}")]
    PermissionDenied { operation: String },

    #[error("Dependency not satisfied: {dependency}")]
    DependencyNotSatisfied { dependency: String },

    #[error("Version conflict: {message}")]
    VersionConflict { message: String },

    #[error("Plugin already registered: {id}")]
    AlreadyRegistered { id: String },

    #[error("Invalid plugin kind: {kind}")]
    InvalidKind { kind: String },

    #[error("Sandbox violation: {message}")]
    SandboxViolation { message: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Library loading error: {0}")]
    LibraryLoading(String),

    #[error("Other error: {0}")]
    Other(#[from] anyhow::Error),
}

/// Result type for plugin operations
pub type Result<T> = std::result::Result<T, PluginError>;
