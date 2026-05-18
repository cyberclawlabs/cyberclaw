/// LLM 错误类型
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    /// HTTP 请求错误
    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),

    /// JSON 序列化/反序列化错误
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    /// API 响应错误
    #[error("API error: {status} - {message}")]
    ApiError { status: u16, message: String },

    /// 无效的响应格式
    #[error("Invalid response format: {0}")]
    InvalidResponse(String),

    /// 配置错误
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// 超时错误
    #[error("Request timeout")]
    Timeout,

    /// 未知提供商
    #[error("Unknown provider: {0}")]
    UnknownProvider(String),

    /// 其他错误
    #[error("Internal error: {0}")]
    Internal(String),
}

/// LLM Result 类型别名
pub type LlmResult<T> = Result<T, LlmError>;
