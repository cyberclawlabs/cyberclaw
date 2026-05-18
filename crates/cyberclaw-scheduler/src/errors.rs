//! 调度器错误类型定义

use thiserror::Error;

/// 调度器错误类型
#[derive(Debug, Error)]
pub enum SchedulerError {
    /// 无效的 Cron 表达式
    #[error("Invalid cron expression: {0}")]
    InvalidCronExpression(String),

    /// 任务未找到
    #[error("Task not found: {0}")]
    TaskNotFound(String),

    /// 任务已存在
    #[error("Task already exists: {0}")]
    TaskAlreadyExists(String),

    /// 任务执行失败
    #[error("Task execution failed: {0}")]
    ExecutionFailed(String),

    /// 调度器未运行
    #[error("Scheduler not running")]
    NotRunning,

    /// 调度器已运行
    #[error("Scheduler already running")]
    AlreadyRunning,

    /// 序列化错误
    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    /// 其他错误
    #[error("Scheduler error: {0}")]
    Other(#[from] anyhow::Error),
}

/// Result 类型别名
pub type Result<T> = std::result::Result<T, SchedulerError>;
