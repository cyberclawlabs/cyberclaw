//! 错误处理模块

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// API 错误类型
#[derive(Debug, Error)]
pub enum ApiError {
    #[error("LLM error: {0}")]
    LlmError(String),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Internal server error: {0}")]
    InternalError(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Forbidden: {0}")]
    Forbidden(String),

    /// 服务/连接器未配置或暂不可用——客户端请求了一个**已知但当前不可达**的能力。
    /// 区别于 `NotFound`（路径不存在）和 `InternalError`（服务端 bug）。
    /// 典型场景：feature-gated connector 未启用、依赖的外部服务未配置。
    #[error("Service unavailable: {0}")]
    ServiceUnavailable(String),
}

/// 错误响应格式
#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: ErrorDetail,
}

/// 错误详细信息，包含在 [`ErrorResponse`] 中
#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorDetail {
    /// 人类可读的错误消息
    pub message: String,
    /// 错误类型标识符 (如 `"invalid_request"`, `"llm_error"`)
    pub r#type: String,
    /// 可选的错误代码，用于客户端精确匹配
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error_type) = match &self {
            ApiError::LlmError(_) => (StatusCode::BAD_GATEWAY, "llm_error"),
            ApiError::InvalidRequest(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
            ApiError::InvalidInput(_) => (StatusCode::BAD_REQUEST, "invalid_input"),
            ApiError::InternalError(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
            ApiError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            ApiError::Unauthorized(_) => (StatusCode::UNAUTHORIZED, "unauthorized"),
            ApiError::Forbidden(_) => (StatusCode::FORBIDDEN, "forbidden"),
            ApiError::ServiceUnavailable(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, "service_unavailable")
            }
        };

        // P2-4 fix: sanitize InternalError messages to avoid leaking
        // implementation details to external clients
        let safe_message = match &self {
            ApiError::InternalError(_) => "Internal server error".to_string(),
            other => other.to_string(),
        };

        let error_response = ErrorResponse {
            error: ErrorDetail {
                message: safe_message,
                r#type: error_type.to_string(),
                code: None,
            },
        };

        (status, Json(error_response)).into_response()
    }
}
