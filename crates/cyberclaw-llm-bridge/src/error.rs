//! Error types for LLM Bridge

use thiserror::Error;

/// Bridge 错误类型
#[derive(Debug, Error)]
pub enum BridgeError {
    /// Tool 未找到映射
    #[error("Unknown tool: {tool_name}")]
    UnknownTool { tool_name: String },

    /// 参数解析失败
    #[error("Failed to parse tool arguments: {message}")]
    ArgumentParseFailed { message: String },

    /// Capability 执行失败
    #[error("Capability execution failed: {message}")]
    ExecutionFailed { message: String },

    /// 超时错误
    #[error("Tool execution timeout after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    /// 速率限制错误
    #[error("Rate limit exceeded for capability: {capability_id}")]
    RateLimitExceeded { capability_id: String },

    /// 内部错误
    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<anyhow::Error> for BridgeError {
    fn from(err: anyhow::Error) -> Self {
        BridgeError::Internal(err.to_string())
    }
}

impl From<serde_json::Error> for BridgeError {
    fn from(err: serde_json::Error) -> Self {
        BridgeError::ArgumentParseFailed {
            message: err.to_string(),
        }
    }
}

/// Bridge 结果类型
pub type BridgeResult<T> = Result<T, BridgeError>;
