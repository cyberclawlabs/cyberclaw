/// 统一安全错误类型模块
///
/// 提供明确的安全错误分类，区分安全错误与业务/系统错误
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 安全错误类型
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum SecurityError {
    /// 认证失败
    #[error("Authentication failed: {message}")]
    AuthenticationFailed {
        message: String,
        trace_id: Option<String>,
    },

    /// 授权失败
    #[error("Authorization failed: {message}")]
    AuthorizationFailed {
        message: String,
        trace_id: Option<String>,
    },

    /// 溯源追踪失败
    #[error("Provenance tracking failed: {message}")]
    ProvenanceFailed {
        message: String,
        operation: String,
        required: bool,
        trace_id: Option<String>,
    },

    /// 审计日志失败
    #[error("Audit logging failed: {message}")]
    AuditFailed {
        message: String,
        trace_id: Option<String>,
    },

    /// trace_id 传播失败
    #[error("Trace ID propagation failed: {message}")]
    TraceIdPropagationFailed {
        message: String,
        expected_trace_id: Option<String>,
        actual_trace_id: Option<String>,
    },

    /// 运行时隔离违规
    #[error("Runtime isolation violated: {message}")]
    RuntimeIsolationViolated {
        message: String,
        risk_level: String,
        expected_runtime: String,
        actual_runtime: String,
        trace_id: Option<String>,
    },

    /// 能力检查失败
    #[error("Capability check failed: {message}")]
    CapabilityCheckFailed {
        message: String,
        capability_id: String,
        trace_id: Option<String>,
    },

    /// 速率限制超出
    #[error("Rate limit exceeded: {message}")]
    RateLimitExceeded {
        message: String,
        capability_id: String,
        current_rate: u64,
        limit: u64,
        trace_id: Option<String>,
    },

    /// 容量限制超出
    #[error("Capacity limit exceeded: {message}")]
    CapacityLimitExceeded {
        message: String,
        current: usize,
        limit: usize,
        trace_id: Option<String>,
    },

    /// 数据泄漏风险
    #[error("Data leakage risk: {message}")]
    DataLeakageRisk {
        message: String,
        data_type: String,
        trace_id: Option<String>,
    },

    /// 时间检查到使用时间（TOCTOU）竞争条件
    #[error("TOCTOU race condition detected: {message}")]
    ToctouRaceCondition {
        message: String,
        resource: String,
        trace_id: Option<String>,
    },

    /// 锁中毒错误
    #[error("Lock poisoned: {message}")]
    LockPoisoned {
        message: String,
        lock_type: String,
        trace_id: Option<String>,
    },

    /// trace_id 注入攻击
    #[error("Trace ID injection attempt: {message}")]
    TraceIdInjection {
        message: String,
        malicious_trace_id: String,
        trace_id: Option<String>,
    },

    /// 配置违规
    #[error("Security configuration violated: {message}")]
    ConfigViolation {
        message: String,
        config_key: String,
        expected: String,
        actual: String,
        trace_id: Option<String>,
    },

    /// 其他安全错误
    #[error("Security error: {message}")]
    Other {
        message: String,
        trace_id: Option<String>,
    },
}

impl SecurityError {
    /// 判断错误是否为关键级别（需要立即fail-fast）
    pub fn is_critical(&self) -> bool {
        matches!(
            self,
            SecurityError::AuthenticationFailed { .. }
                | SecurityError::AuthorizationFailed { .. }
                | SecurityError::RuntimeIsolationViolated { .. }
                | SecurityError::TraceIdInjection { .. }
        )
    }

    /// 获取 trace_id（如果存在）
    pub fn trace_id(&self) -> Option<&str> {
        match self {
            SecurityError::AuthenticationFailed { trace_id, .. }
            | SecurityError::AuthorizationFailed { trace_id, .. }
            | SecurityError::ProvenanceFailed { trace_id, .. }
            | SecurityError::AuditFailed { trace_id, .. }
            | SecurityError::RuntimeIsolationViolated { trace_id, .. }
            | SecurityError::CapabilityCheckFailed { trace_id, .. }
            | SecurityError::RateLimitExceeded { trace_id, .. }
            | SecurityError::CapacityLimitExceeded { trace_id, .. }
            | SecurityError::DataLeakageRisk { trace_id, .. }
            | SecurityError::ToctouRaceCondition { trace_id, .. }
            | SecurityError::LockPoisoned { trace_id, .. }
            | SecurityError::TraceIdInjection { trace_id, .. }
            | SecurityError::ConfigViolation { trace_id, .. }
            | SecurityError::Other { trace_id, .. } => trace_id.as_deref(),
            SecurityError::TraceIdPropagationFailed { .. } => None,
        }
    }

    /// 转换为审计日志格式
    pub fn to_audit_log(&self) -> String {
        match self {
            SecurityError::AuthenticationFailed { message, trace_id } => {
                format!("[SECURITY:AUTH] {} | trace_id: {:?}", message, trace_id)
            }
            SecurityError::AuthorizationFailed { message, trace_id } => {
                format!("[SECURITY:AUTHZ] {} | trace_id: {:?}", message, trace_id)
            }
            SecurityError::ProvenanceFailed {
                message,
                operation,
                required,
                trace_id,
            } => {
                format!(
                    "[SECURITY:PROVENANCE] {} | operation: {} | required: {} | trace_id: {:?}",
                    message, operation, required, trace_id
                )
            }
            SecurityError::AuditFailed { message, trace_id } => {
                format!("[SECURITY:AUDIT] {} | trace_id: {:?}", message, trace_id)
            }
            SecurityError::TraceIdPropagationFailed {
                message,
                expected_trace_id,
                actual_trace_id,
            } => {
                format!(
                    "[SECURITY:TRACE] {} | expected: {:?} | actual: {:?}",
                    message, expected_trace_id, actual_trace_id
                )
            }
            SecurityError::RuntimeIsolationViolated {
                message,
                risk_level,
                expected_runtime,
                actual_runtime,
                trace_id,
            } => {
                format!(
                    "[SECURITY:RUNTIME] {} | risk: {} | expected: {} | actual: {} | trace_id: {:?}",
                    message, risk_level, expected_runtime, actual_runtime, trace_id
                )
            }
            SecurityError::CapabilityCheckFailed {
                message,
                capability_id,
                trace_id,
            } => {
                format!(
                    "[SECURITY:CAPABILITY] {} | capability: {} | trace_id: {:?}",
                    message, capability_id, trace_id
                )
            }
            SecurityError::RateLimitExceeded {
                message,
                capability_id,
                current_rate,
                limit,
                trace_id,
            } => {
                format!(
                    "[SECURITY:RATE_LIMIT] {} | capability: {} | current: {} | limit: {} | trace_id: {:?}",
                    message, capability_id, current_rate, limit, trace_id
                )
            }
            SecurityError::CapacityLimitExceeded {
                message,
                current,
                limit,
                trace_id,
            } => {
                format!(
                    "[SECURITY:CAPACITY] {} | current: {} | limit: {} | trace_id: {:?}",
                    message, current, limit, trace_id
                )
            }
            SecurityError::DataLeakageRisk {
                message,
                data_type,
                trace_id,
            } => {
                format!(
                    "[SECURITY:DATA_LEAK] {} | data_type: {} | trace_id: {:?}",
                    message, data_type, trace_id
                )
            }
            SecurityError::ToctouRaceCondition {
                message,
                resource,
                trace_id,
            } => {
                format!(
                    "[SECURITY:TOCTOU] {} | resource: {} | trace_id: {:?}",
                    message, resource, trace_id
                )
            }
            SecurityError::LockPoisoned {
                message,
                lock_type,
                trace_id,
            } => {
                format!(
                    "[SECURITY:LOCK] {} | lock_type: {} | trace_id: {:?}",
                    message, lock_type, trace_id
                )
            }
            SecurityError::TraceIdInjection {
                message,
                malicious_trace_id,
                trace_id,
            } => {
                format!(
                    "[SECURITY:INJECTION] {} | malicious: {} | trace_id: {:?}",
                    message, malicious_trace_id, trace_id
                )
            }
            SecurityError::ConfigViolation {
                message,
                config_key,
                expected,
                actual,
                trace_id,
            } => {
                format!(
                    "[SECURITY:CONFIG] {} | key: {} | expected: {} | actual: {} | trace_id: {:?}",
                    message, config_key, expected, actual, trace_id
                )
            }
            SecurityError::Other { message, trace_id } => {
                format!("[SECURITY:OTHER] {} | trace_id: {:?}", message, trace_id)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_critical() {
        let auth_error = SecurityError::AuthenticationFailed {
            message: "test".to_string(),
            trace_id: None,
        };
        assert!(auth_error.is_critical());

        let provenance_error = SecurityError::ProvenanceFailed {
            message: "test".to_string(),
            operation: "test".to_string(),
            required: true,
            trace_id: None,
        };
        assert!(!provenance_error.is_critical());
    }

    #[test]
    fn test_trace_id() {
        let error = SecurityError::AuthenticationFailed {
            message: "test".to_string(),
            trace_id: Some("trace-123".to_string()),
        };
        assert_eq!(error.trace_id(), Some("trace-123"));
    }

    #[test]
    fn test_to_audit_log() {
        let error = SecurityError::RateLimitExceeded {
            message: "Rate limit exceeded".to_string(),
            capability_id: "fs.read".to_string(),
            current_rate: 100,
            limit: 50,
            trace_id: Some("trace-123".to_string()),
        };
        let log = error.to_audit_log();
        assert!(log.contains("[SECURITY:RATE_LIMIT]"));
        assert!(log.contains("fs.read"));
        assert!(log.contains("100"));
        assert!(log.contains("50"));
    }
}
