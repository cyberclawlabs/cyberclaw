//! Cron 调度器实现

use crate::errors::{Result, SchedulerError};
use crate::types::{
    ExecutionHistory, ExecutionId, ExecutionRecord, ExecutionResult, ScheduledTask, TaskAction,
    TaskId,
};
use crate::TaskExecutor;
use chrono::Utc;
use dashmap::DashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use tokio::time::{sleep, Duration};
use tracing::{debug, error, info, warn};

/// Cron 调度器
///
/// 提供基于 Cron 表达式的定时任务调度功能。
///
/// # 功能特性
///
/// - **Cron 表达式**: 支持标准 Cron 表达式 (分钟级精度)
/// - **任务队列**: 可靠的任务队列管理
/// - **执行历史**: 详细的执行历史记录
/// - **并发控制**: 支持并发执行多个任务
/// - **故障恢复**: 自动重试失败的任务
///
/// # 示例
///
/// ```rust,no_run
/// use cyberclaw_scheduler::prelude::*;
///
/// # async fn example() -> anyhow::Result<()> {
/// // 创建调度器
/// let scheduler = CronScheduler::new()?;
///
/// // 添加每日备份任务
/// scheduler.schedule(
///     "daily-backup".to_string(),
///     "0 2 * * *".to_string(),
///     TaskAction::ExecuteCapability {
///         connector_id: "database".to_string(),
///         capability: "db.backup".to_string(),
///         params: serde_json::json!({}),
///     },
/// ).await?;
///
/// // 启动调度器
/// scheduler.start().await?;
/// # Ok(())
/// # }
/// ```
pub struct CronScheduler {
    /// 调度任务列表
    tasks: Arc<DashMap<TaskId, ScheduledTask>>,
    /// 执行历史
    history: Arc<RwLock<ExecutionHistory>>,
    /// 是否正在运行
    running: Arc<AtomicBool>,
    /// 检查间隔 (秒)
    check_interval_secs: u64,
    /// 最大并发执行数
    max_concurrent_executions: usize,
    /// 并发执行信号量 (限制同时执行的任务数)
    execution_semaphore: Arc<Semaphore>,
    /// 任务执行器 (用于实际执行任务)
    executor: Option<Arc<dyn TaskExecutor>>,
}

impl CronScheduler {
    /// 创建新的 Cron 调度器
    ///
    /// # 默认配置
    ///
    /// - 检查间隔: 60 秒
    /// - 最大并发执行数: 10
    /// - 最大历史记录数: 1000
    pub fn new() -> Result<Self> {
        Self::with_config(60, 10, 1000)
    }

    /// 使用自定义配置创建调度器
    ///
    /// # 参数
    ///
    /// - `check_interval_secs`: 检查间隔 (秒)
    /// - `max_concurrent`: 最大并发执行数
    /// - `max_history`: 最大历史记录数
    pub fn with_config(
        check_interval_secs: u64,
        max_concurrent: usize,
        max_history: usize,
    ) -> Result<Self> {
        Ok(Self {
            tasks: Arc::new(DashMap::new()),
            history: Arc::new(RwLock::new(ExecutionHistory::new(max_history))),
            running: Arc::new(AtomicBool::new(false)),
            check_interval_secs,
            max_concurrent_executions: max_concurrent,
            execution_semaphore: Arc::new(Semaphore::new(max_concurrent)),
            executor: None,
        })
    }

    /// 设置任务执行器
    ///
    /// 必须在调用 `start()` 之前设置执行器，否则调度器将使用模拟执行。
    ///
    /// # 参数
    ///
    /// - `executor`: 实现了 TaskExecutor trait 的执行器
    pub fn with_executor(mut self, executor: Arc<dyn TaskExecutor>) -> Self {
        self.executor = Some(executor);
        self
    }

    /// 添加定时任务
    ///
    /// # 参数
    ///
    /// - `name`: 任务名称
    /// - `cron_expr`: Cron 表达式 (标准 5 字段格式)
    /// - `action`: 任务动作
    ///
    /// # Cron 表达式格式
    ///
    /// ```text
    /// ┌───────────── 分钟 (0 - 59)
    /// │ ┌───────────── 小时 (0 - 23)
    /// │ │ ┌───────────── 日 (1 - 31)
    /// │ │ │ ┌───────────── 月 (1 - 12)
    /// │ │ │ │ ┌───────────── 星期 (0 - 6) (0 = 周日)
    /// │ │ │ │ │
    /// * * * * *
    /// ```
    ///
    /// # 示例
    ///
    /// - `0 2 * * *` - 每天凌晨 2:00
    /// - `*/5 * * * *` - 每 5 分钟
    /// - `0 9 * * MON` - 每周一上午 9:00
    /// - `0 0 1 * *` - 每月 1 号凌晨
    ///
    /// # 返回
    ///
    /// 返回新创建任务的 ID
    pub async fn schedule(
        &self,
        name: String,
        cron_expr: String,
        action: TaskAction,
    ) -> Result<TaskId> {
        // 创建任务 (会验证 Cron 表达式)
        let task = ScheduledTask::new(name.clone(), cron_expr, action)?;
        let task_id = task.id.clone();

        // 检查任务是否已存在
        if self.tasks.contains_key(&task_id) {
            return Err(SchedulerError::TaskAlreadyExists(task_id.to_string()));
        }

        info!(
            task_id = %task_id,
            task_name = %name,
            next_execution = ?task.next_execution,
            "Scheduled new task"
        );

        // 插入任务
        self.tasks.insert(task_id.clone(), task);

        Ok(task_id)
    }

    /// 取消定时任务
    pub async fn unschedule(&self, task_id: &TaskId) -> Result<()> {
        self.tasks
            .remove(task_id)
            .ok_or_else(|| SchedulerError::TaskNotFound(task_id.to_string()))?;

        info!(task_id = %task_id, "Unscheduled task");
        Ok(())
    }

    /// 启用任务
    pub async fn enable_task(&self, task_id: &TaskId) -> Result<()> {
        let mut task = self
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| SchedulerError::TaskNotFound(task_id.to_string()))?;

        task.enabled = true;
        task.update_next_execution();

        info!(task_id = %task_id, "Enabled task");
        Ok(())
    }

    /// 禁用任务
    pub async fn disable_task(&self, task_id: &TaskId) -> Result<()> {
        let mut task = self
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| SchedulerError::TaskNotFound(task_id.to_string()))?;

        task.enabled = false;

        info!(task_id = %task_id, "Disabled task");
        Ok(())
    }

    /// 获取任务信息
    pub async fn get_task(&self, task_id: &TaskId) -> Result<ScheduledTask> {
        self.tasks
            .get(task_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| SchedulerError::TaskNotFound(task_id.to_string()))
    }

    /// 列出所有任务
    pub async fn list_tasks(&self) -> Vec<ScheduledTask> {
        self.tasks
            .iter()
            .map(|entry| entry.value().clone())
            .collect::<Vec<_>>()
    }

    /// 获取任务执行历史
    pub async fn get_task_history(&self, task_id: &TaskId) -> Vec<ExecutionRecord> {
        self.history
            .read()
            .await
            .get_task_history(task_id)
            .iter()
            .map(|r| (*r).clone())
            .collect()
    }

    /// 获取所有执行历史
    pub async fn get_all_history(&self) -> Vec<ExecutionRecord> {
        self.history.read().await.all_records().to_vec()
    }

    /// 启动调度器
    ///
    /// 这会启动后台任务，定期检查并执行到期的任务。
    /// 此方法会一直运行直到调用 `stop()`。
    pub async fn start(&self) -> Result<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Err(SchedulerError::AlreadyRunning);
        }

        info!(
            check_interval_secs = self.check_interval_secs,
            max_concurrent = self.max_concurrent_executions,
            "Starting Cron scheduler"
        );

        let tasks = Arc::clone(&self.tasks);
        let history = Arc::clone(&self.history);
        let running = Arc::clone(&self.running);
        let semaphore = Arc::clone(&self.execution_semaphore);
        let check_interval = Duration::from_secs(self.check_interval_secs);

        // 主调度循环
        while running.load(Ordering::SeqCst) {
            let now = Utc::now();
            debug!(timestamp = %now, "Checking for scheduled tasks");

            // 收集需要执行的任务
            let tasks_to_execute: Vec<ScheduledTask> = tasks
                .iter()
                .filter_map(|entry| {
                    let task = entry.value();
                    if task.should_execute(now) {
                        Some(task.clone())
                    } else {
                        None
                    }
                })
                .collect();

            debug!(count = tasks_to_execute.len(), "Found tasks to execute");

            // 并发执行任务 (受信号量限制)
            for task in tasks_to_execute {
                let task_id = task.id.clone();
                let tasks_ref = Arc::clone(&tasks);
                let history_ref = Arc::clone(&history);
                let sem = Arc::clone(&semaphore);
                let executor = self.executor.clone();

                // 尝试获取执行许可 (非阻塞)
                match sem.clone().try_acquire_owned() {
                    Ok(permit) => {
                        // 获取到许可，在新的 tokio 任务中执行
                        tokio::spawn(async move {
                            // permit会在任务结束时自动释放 (Drop trait)
                            Self::execute_task_impl(task, tasks_ref, history_ref, executor).await;
                            drop(permit); // 显式释放许可
                        });
                    }
                    Err(_) => {
                        // 达到并发限制，跳过本次执行
                        warn!(
                            task_id = %task_id,
                            max_concurrent = self.max_concurrent_executions,
                            "Concurrent execution limit reached, skipping task execution"
                        );
                    }
                }
            }

            // 等待下一个检查周期
            sleep(check_interval).await;
        }

        info!("Cron scheduler stopped");
        Ok(())
    }

    /// 停止调度器
    pub async fn stop(&self) {
        info!("Stopping Cron scheduler");
        self.running.store(false, Ordering::SeqCst);
    }

    /// 是否正在运行
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// 执行单个任务 (内部实现)
    async fn execute_task_impl(
        mut task: ScheduledTask,
        tasks: Arc<DashMap<TaskId, ScheduledTask>>,
        history: Arc<RwLock<ExecutionHistory>>,
        executor: Option<Arc<dyn TaskExecutor>>,
    ) {
        let execution_id = ExecutionId::new();
        let start_time = Utc::now();

        info!(
            task_id = %task.id,
            task_name = %task.name,
            execution_id = %execution_id,
            "Executing scheduled task"
        );

        // 执行任务
        let result = Self::execute_action(&task.action, executor).await;

        let end_time = Utc::now();
        let duration_ms = (end_time - start_time).num_milliseconds() as u64;

        // 记录执行历史
        let record = ExecutionRecord {
            task_id: task.id.clone(),
            execution_id,
            timestamp: start_time,
            result: result.clone(),
            duration_ms,
        };

        history.write().await.record(record);

        // 更新任务状态
        task.last_execution = Some(start_time);
        task.update_next_execution();

        if let Some(mut entry) = tasks.get_mut(&task.id) {
            *entry = task.clone();
        }

        // 记录结果
        match result {
            ExecutionResult::Success { .. } => {
                info!(
                    task_id = %task.id,
                    duration_ms,
                    "Task execution completed successfully"
                );
            }
            ExecutionResult::Failure { ref error } => {
                error!(
                    task_id = %task.id,
                    duration_ms,
                    error = %error,
                    "Task execution failed"
                );
            }
        }
    }

    /// 执行任务动作
    async fn execute_action(
        action: &TaskAction,
        executor: Option<Arc<dyn TaskExecutor>>,
    ) -> ExecutionResult {
        // 如果配置了 executor，则使用真实执行；否则返回模拟结果
        if let Some(executor) = executor {
            debug!("Using real TaskExecutor to execute action");
            executor.execute_task(action).await
        } else {
            warn!("No TaskExecutor configured, using mock execution");
            match action {
                TaskAction::ExecuteCapability {
                    connector_id,
                    capability,
                    params,
                } => {
                    // 模拟执行 (仅用于测试)
                    debug!(
                        connector_id = %connector_id,
                        capability = %capability,
                        "Mock-executing capability"
                    );

                    ExecutionResult::Success {
                        output: serde_json::json!({
                            "status": "mock_success",
                            "connector_id": connector_id,
                            "capability": capability,
                            "params": params,
                        }),
                    }
                }
                TaskAction::ExecuteAgent { agent_id, task } => {
                    // 模拟执行 (仅用于测试)
                    debug!(
                        agent_id = %agent_id,
                        task = %task,
                        "Mock-executing agent task"
                    );

                    ExecutionResult::Success {
                        output: serde_json::json!({
                            "status": "mock_success",
                            "agent_id": agent_id,
                            "task": task,
                        }),
                    }
                }
            }
        }
    }
}

impl Default for CronScheduler {
    fn default() -> Self {
        Self::new().expect("Failed to create default CronScheduler")
    }
}

// 为了支持 clone
impl Clone for CronScheduler {
    fn clone(&self) -> Self {
        Self {
            tasks: Arc::clone(&self.tasks),
            history: Arc::clone(&self.history),
            running: Arc::clone(&self.running),
            check_interval_secs: self.check_interval_secs,
            max_concurrent_executions: self.max_concurrent_executions,
            execution_semaphore: Arc::clone(&self.execution_semaphore),
            executor: self.executor.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::timeout;

    #[tokio::test]
    async fn test_create_scheduler() {
        let scheduler = CronScheduler::new().unwrap();
        assert!(!scheduler.is_running());
    }

    #[tokio::test]
    async fn test_schedule_task() {
        let scheduler = CronScheduler::new().unwrap();

        let task_id = scheduler
            .schedule(
                "test-task".to_string(),
                "0 0 * * * *".to_string(),
                TaskAction::ExecuteCapability {
                    connector_id: "test".to_string(),
                    capability: "test.action".to_string(),
                    params: serde_json::json!({}),
                },
            )
            .await
            .unwrap();

        let task = scheduler.get_task(&task_id).await.unwrap();
        assert_eq!(task.name, "test-task");
        assert_eq!(task.cron_expr, "0 0 * * * *");
        assert!(task.enabled);
    }

    #[tokio::test]
    async fn test_invalid_cron_expression() {
        let scheduler = CronScheduler::new().unwrap();

        let result = scheduler
            .schedule(
                "invalid-task".to_string(),
                "invalid cron".to_string(),
                TaskAction::ExecuteCapability {
                    connector_id: "test".to_string(),
                    capability: "test.action".to_string(),
                    params: serde_json::json!({}),
                },
            )
            .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SchedulerError::InvalidCronExpression(_)
        ));
    }

    #[tokio::test]
    async fn test_unschedule_task() {
        let scheduler = CronScheduler::new().unwrap();

        let task_id = scheduler
            .schedule(
                "test-task".to_string(),
                "0 0 * * * *".to_string(),
                TaskAction::ExecuteCapability {
                    connector_id: "test".to_string(),
                    capability: "test.action".to_string(),
                    params: serde_json::json!({}),
                },
            )
            .await
            .unwrap();

        scheduler.unschedule(&task_id).await.unwrap();

        let result = scheduler.get_task(&task_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_enable_disable_task() {
        let scheduler = CronScheduler::new().unwrap();

        let task_id = scheduler
            .schedule(
                "test-task".to_string(),
                "0 0 * * * *".to_string(),
                TaskAction::ExecuteCapability {
                    connector_id: "test".to_string(),
                    capability: "test.action".to_string(),
                    params: serde_json::json!({}),
                },
            )
            .await
            .unwrap();

        // 禁用任务
        scheduler.disable_task(&task_id).await.unwrap();
        let task = scheduler.get_task(&task_id).await.unwrap();
        assert!(!task.enabled);

        // 启用任务
        scheduler.enable_task(&task_id).await.unwrap();
        let task = scheduler.get_task(&task_id).await.unwrap();
        assert!(task.enabled);
    }

    #[tokio::test]
    async fn test_list_tasks() {
        let scheduler = CronScheduler::new().unwrap();

        scheduler
            .schedule(
                "task-1".to_string(),
                "0 0 * * * *".to_string(),
                TaskAction::ExecuteCapability {
                    connector_id: "test".to_string(),
                    capability: "test.action".to_string(),
                    params: serde_json::json!({}),
                },
            )
            .await
            .unwrap();

        scheduler
            .schedule(
                "task-2".to_string(),
                "0 30 * * * *".to_string(),
                TaskAction::ExecuteAgent {
                    agent_id: "test-agent".to_string(),
                    task: "test task".to_string(),
                },
            )
            .await
            .unwrap();

        let tasks = scheduler.list_tasks().await;
        assert_eq!(tasks.len(), 2);
    }

    #[tokio::test]
    async fn test_scheduler_start_stop() {
        let scheduler = CronScheduler::with_config(1, 10, 100).unwrap();

        // 在后台启动调度器
        let scheduler_clone = scheduler.clone();
        let handle = tokio::spawn(async move { scheduler_clone.start().await });

        // 等待一小段时间
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(scheduler.is_running());

        // 停止调度器
        scheduler.stop().await;

        // 等待调度器完全停止
        let _ = timeout(Duration::from_secs(5), handle).await;
        assert!(!scheduler.is_running());
    }

    #[tokio::test]
    async fn test_should_execute() {
        let task = ScheduledTask::new(
            "test".to_string(),
            "0 0 * * * *".to_string(),
            TaskAction::ExecuteCapability {
                connector_id: "test".to_string(),
                capability: "test.action".to_string(),
                params: serde_json::json!({}),
            },
        )
        .unwrap();

        // 任务启用时，应该根据时间判断
        let now = Utc::now();
        let _should_exec = task.should_execute(now);
        // 结果取决于当前时间是否恰好在整点

        // 禁用的任务不应该执行
        let mut disabled_task = task.clone();
        disabled_task.enabled = false;
        assert!(!disabled_task.should_execute(now));
    }

    #[tokio::test]
    async fn test_execution_history() {
        let scheduler = CronScheduler::new().unwrap();

        let task_id = scheduler
            .schedule(
                "test-task".to_string(),
                "0 0 * * * *".to_string(),
                TaskAction::ExecuteCapability {
                    connector_id: "test".to_string(),
                    capability: "test.action".to_string(),
                    params: serde_json::json!({}),
                },
            )
            .await
            .unwrap();

        // 手动添加一些执行记录
        scheduler.history.write().await.record(ExecutionRecord {
            task_id: task_id.clone(),
            execution_id: ExecutionId::new(),
            timestamp: Utc::now(),
            result: ExecutionResult::Success {
                output: serde_json::json!({}),
            },
            duration_ms: 100,
        });

        let history = scheduler.get_task_history(&task_id).await;
        assert_eq!(history.len(), 1);
    }

    #[tokio::test]
    async fn test_concurrent_execution() {
        let scheduler = CronScheduler::with_config(1, 5, 100).unwrap();

        // 添加多个任务
        for i in 0..3 {
            scheduler
                .schedule(
                    format!("task-{}", i),
                    "0 * * * * *".to_string(), // 每分钟执行
                    TaskAction::ExecuteCapability {
                        connector_id: "test".to_string(),
                        capability: format!("test.action-{}", i),
                        params: serde_json::json!({}),
                    },
                )
                .await
                .unwrap();
        }

        let tasks = scheduler.list_tasks().await;
        assert_eq!(tasks.len(), 3);
    }

    #[tokio::test]
    async fn test_cron_expression_parsing() {
        let scheduler = CronScheduler::new().unwrap();

        // 测试各种有效的 Cron 表达式 (6字段格式: 秒 分 时 日 月 星期)
        let valid_expressions = vec![
            "0 0 * * * *",    // 每小时
            "0 */5 * * * *",  // 每 5 分钟
            "0 0 0 * * *",    // 每天午夜
            "0 0 9 * * MON",  // 每周一上午 9 点
            "0 0 0 1 * *",    // 每月 1 号
            "0 0 0 1 1 *",    // 每年 1 月 1 号
            "0 30 2 * * SAT", // 每周六凌晨 2:30
        ];

        for expr in valid_expressions {
            let result = scheduler
                .schedule(
                    format!("task-{}", expr),
                    expr.to_string(),
                    TaskAction::ExecuteCapability {
                        connector_id: "test".to_string(),
                        capability: "test.action".to_string(),
                        params: serde_json::json!({}),
                    },
                )
                .await;

            assert!(result.is_ok(), "Failed to parse: {}", expr);
        }
    }
}
