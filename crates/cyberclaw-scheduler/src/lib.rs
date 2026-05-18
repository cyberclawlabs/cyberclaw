//! # CyberClaw Scheduler
//!
//! `cyberclaw-scheduler` 提供 CyberClaw 平台的调度系统，包括：
//!
//! - **Heartbeat 监控**: 节点健康检查、异常检测和状态管理
//! - **Cron 调度器**: 基于 Cron 表达式的定时任务调度
//! - **任务队列**: 可靠的任务队列管理
//! - **执行历史**: 详细的执行历史记录和审计
//!
//! ## 快速开始
//!
//! ```rust,ignore
//! // 注意: CronScheduler 尚未实现,此示例展示未来的 API 设计
//! use cyberclaw_scheduler::prelude::*;
//! use std::sync::Arc;
//!
//! # async fn example() -> anyhow::Result<()> {
//! // 创建调度器
//! let scheduler = CronScheduler::new()?;
//!
//! // 添加定时任务
//! let task_id = scheduler.schedule(
//!     "daily-backup".to_string(),
//!     "0 2 * * *".to_string(), // 每天凌晨 2 点
//!     TaskAction::ExecuteCapability {
//!         connector_id: "database".to_string(),
//!         capability: "db.backup".to_string(),
//!         params: serde_json::json!({}),
//!     },
//! ).await?;
//!
//! // 启动调度器
//! scheduler.start().await?;
//! # Ok(())
//! # }
//! ```

// 核心模块
pub mod cron_scheduler;
pub mod errors;
pub mod heartbeat;
pub mod types;

use async_trait::async_trait;
use types::{ExecutionResult, TaskAction};

/// TaskExecutor trait - 用于执行调度任务的抽象接口
///
/// 这个 trait 使得 CronScheduler 与具体的执行实现解耦，
/// 符合依赖倒置原则（DIP）。具体的执行逻辑由外部实现。
#[async_trait]
pub trait TaskExecutor: Send + Sync {
    /// 执行任务动作
    ///
    /// # 参数
    ///
    /// - `action`: 要执行的任务动作
    ///
    /// # 返回
    ///
    /// 执行结果（Success 或 Failure）
    async fn execute_task(&self, action: &TaskAction) -> ExecutionResult;
}

pub mod prelude {
    //! 常用类型的便捷导入

    pub use crate::cron_scheduler::*;
    pub use crate::errors::*;
    pub use crate::heartbeat::*;
    pub use crate::types::*;
    pub use crate::TaskExecutor;
}
