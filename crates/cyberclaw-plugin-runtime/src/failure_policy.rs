//! # Hook 失败策略 (Failure Policy)
//!
//! 定义 Platform Plugin Hook 执行失败时的处理策略。
//!
//! ## 策略类型
//!
//! - **Ignore**: 忽略失败，继续执行后续 Hook
//! - **Retry**: 重试失败的 Hook，最多 N 次
//! - **Abort**: 立即中止整个 Hook 链执行

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Hook 执行失败策略
///
/// 定义当 Hook 执行失败时应该如何处理。
///
/// # 示例
///
/// ```rust
/// use cyberclaw_plugin_runtime::failure_policy::FailurePolicy;
///
/// // 忽略失败
/// let ignore = FailurePolicy::Ignore;
///
/// // 重试最多 3 次，每次间隔 100ms
/// let retry = FailurePolicy::Retry {
///     max_attempts: 3,
///     backoff_ms: 100,
/// };
///
/// // 立即中止
/// let abort = FailurePolicy::Abort;
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FailurePolicy {
    /// 忽略失败，继续执行后续 Hook
    ///
    /// 适用场景：
    /// - 可选的监控或日志 Hook
    /// - 非关键的通知 Hook
    Ignore,

    /// 重试失败的 Hook
    ///
    /// # 字段
    ///
    /// - `max_attempts`: 最大重试次数（包括首次尝试）
    /// - `backoff_ms`: 重试间隔（毫秒）
    ///
    /// 适用场景：
    /// - 网络不稳定的远程调用
    /// - 临时资源不可用的情况
    Retry {
        /// 最大尝试次数（必须 >= 1）
        max_attempts: u32,
        /// 重试间隔（毫秒）
        backoff_ms: u64,
    },

    /// 立即中止整个 Hook 链执行
    ///
    /// 适用场景：
    /// - 安全检查失败
    /// - 关键前置条件不满足
    /// - 数据一致性校验失败
    Abort,
}

impl Default for FailurePolicy {
    /// 默认策略为 Ignore（向后兼容）
    fn default() -> Self {
        Self::Ignore
    }
}

impl FailurePolicy {
    /// 应用失败策略，返回是否应该继续执行
    ///
    /// # 参数
    ///
    /// - `attempt`: 当前尝试次数（从 1 开始）
    /// - `error`: 失败的错误信息
    ///
    /// # 返回值
    ///
    /// - `Ok(true)`: 继续执行后续 Hook
    /// - `Ok(false)`: 等待重试
    /// - `Err(error)`: 中止执行
    pub fn apply(&self, attempt: u32, error: &str) -> Result<bool, String> {
        match self {
            FailurePolicy::Ignore => {
                tracing::warn!("Hook failed but ignored (attempt {}): {}", attempt, error);
                Ok(true) // 继续执行
            }
            FailurePolicy::Retry {
                max_attempts,
                backoff_ms: _,
            } => {
                if attempt >= *max_attempts {
                    tracing::error!("Hook failed after {} attempts: {}", max_attempts, error);
                    Err(format!(
                        "Hook failed after {} attempts: {}",
                        max_attempts, error
                    ))
                } else {
                    tracing::warn!("Hook failed (attempt {}), will retry: {}", attempt, error);
                    Ok(false) // 需要重试
                }
            }
            FailurePolicy::Abort => {
                tracing::error!("Hook failed and aborted (attempt {}): {}", attempt, error);
                Err(format!("Hook execution aborted: {}", error))
            }
        }
    }

    /// 获取重试延迟时间
    ///
    /// # 返回值
    ///
    /// - `Some(Duration)`: 重试策略的延迟时间
    /// - `None`: 非重试策略
    pub fn backoff_duration(&self) -> Option<Duration> {
        match self {
            FailurePolicy::Retry { backoff_ms, .. } => Some(Duration::from_millis(*backoff_ms)),
            _ => None,
        }
    }

    /// 判断是否需要重试
    pub fn should_retry(&self, attempt: u32) -> bool {
        match self {
            FailurePolicy::Retry { max_attempts, .. } => attempt < *max_attempts,
            _ => false,
        }
    }

    /// 获取最大尝试次数
    pub fn max_attempts(&self) -> u32 {
        match self {
            FailurePolicy::Retry { max_attempts, .. } => *max_attempts,
            _ => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy() {
        assert_eq!(FailurePolicy::default(), FailurePolicy::Ignore);
    }

    #[test]
    fn test_ignore_policy() {
        let policy = FailurePolicy::Ignore;
        let result = policy.apply(1, "test error");
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[test]
    fn test_retry_policy_first_attempt() {
        let policy = FailurePolicy::Retry {
            max_attempts: 3,
            backoff_ms: 100,
        };
        let result = policy.apply(1, "test error");
        assert!(result.is_ok());
        assert!(!result.unwrap()); // 需要重试
    }

    #[test]
    fn test_retry_policy_max_attempts_exceeded() {
        let policy = FailurePolicy::Retry {
            max_attempts: 3,
            backoff_ms: 100,
        };
        let result = policy.apply(3, "test error");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("failed after 3 attempts"));
    }

    #[test]
    fn test_abort_policy() {
        let policy = FailurePolicy::Abort;
        let result = policy.apply(1, "test error");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("aborted"));
    }

    #[test]
    fn test_backoff_duration() {
        let retry = FailurePolicy::Retry {
            max_attempts: 3,
            backoff_ms: 500,
        };
        assert_eq!(retry.backoff_duration(), Some(Duration::from_millis(500)));

        let ignore = FailurePolicy::Ignore;
        assert_eq!(ignore.backoff_duration(), None);

        let abort = FailurePolicy::Abort;
        assert_eq!(abort.backoff_duration(), None);
    }

    #[test]
    fn test_should_retry() {
        let retry = FailurePolicy::Retry {
            max_attempts: 3,
            backoff_ms: 100,
        };
        assert!(retry.should_retry(1));
        assert!(retry.should_retry(2));
        assert!(!retry.should_retry(3));
        assert!(!retry.should_retry(4));

        let ignore = FailurePolicy::Ignore;
        assert!(!ignore.should_retry(1));

        let abort = FailurePolicy::Abort;
        assert!(!abort.should_retry(1));
    }

    #[test]
    fn test_max_attempts() {
        let retry = FailurePolicy::Retry {
            max_attempts: 5,
            backoff_ms: 100,
        };
        assert_eq!(retry.max_attempts(), 5);

        let ignore = FailurePolicy::Ignore;
        assert_eq!(ignore.max_attempts(), 1);

        let abort = FailurePolicy::Abort;
        assert_eq!(abort.max_attempts(), 1);
    }

    #[test]
    fn test_serde_ignore() {
        let policy = FailurePolicy::Ignore;
        let json = serde_json::to_string(&policy).unwrap();
        assert_eq!(json, r#"{"type":"ignore"}"#);

        let deserialized: FailurePolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, policy);
    }

    #[test]
    fn test_serde_retry() {
        let policy = FailurePolicy::Retry {
            max_attempts: 3,
            backoff_ms: 200,
        };
        let json = serde_json::to_string(&policy).unwrap();
        assert!(json.contains("retry"));
        assert!(json.contains("max_attempts"));
        assert!(json.contains("backoff_ms"));

        let deserialized: FailurePolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, policy);
    }

    #[test]
    fn test_serde_abort() {
        let policy = FailurePolicy::Abort;
        let json = serde_json::to_string(&policy).unwrap();
        assert_eq!(json, r#"{"type":"abort"}"#);

        let deserialized: FailurePolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, policy);
    }
}
