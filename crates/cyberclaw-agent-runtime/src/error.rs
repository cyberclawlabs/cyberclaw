//! Error types for agent runtime operations.

use cyberclaw_core::ids::AgentId;
use thiserror::Error;

/// Errors that can occur during agent runtime operations.
#[derive(Debug, Error)]
pub enum AgentRuntimeError {
    /// Agent not found or not registered.
    #[error("agent '{0}' is not registered")]
    AgentNotFound(AgentId),

    /// Configuration error.
    #[error("configuration error: {0}")]
    ConfigError(String),

    /// Agent execution exceeded the configured timeout.
    #[error("agent '{agent_id}' execution timed out after {timeout_secs}s")]
    ExecutionTimeout {
        agent_id: AgentId,
        timeout_secs: u64,
    },

    /// Agent execution failed with an underlying error.
    #[error("agent '{agent_id}' execution failed: {source}")]
    ExecutionFailed {
        agent_id: AgentId,
        #[source]
        source: anyhow::Error,
    },

    /// Input validation failed due to a security constraint violation.
    #[error("input validation error: {0}")]
    ValidationError(String),

    /// Resource limit exceeded (e.g. max concurrent executions reached).
    #[error("resource limit exceeded: {0}")]
    ResourceLimitExceeded(String),

    /// Internal error with a message.
    #[error("internal error: {0}")]
    Internal(String),
}

/// Convenience type alias for Results with AgentRuntimeError.
pub type AgentRuntimeResult<T> = Result<T, AgentRuntimeError>;
