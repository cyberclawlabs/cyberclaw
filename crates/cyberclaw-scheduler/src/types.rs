//! 调度器类型定义

use chrono::{DateTime, Utc};
use cron::Schedule;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// 任务 ID
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub String);

impl TaskId {
    /// 创建新的任务 ID
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// 从字符串创建任务 ID
    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 执行 ID
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExecutionId(pub String);

impl ExecutionId {
    /// 创建新的执行 ID
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl Default for ExecutionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ExecutionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 任务动作
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskAction {
    /// 执行 Capability
    ExecuteCapability {
        /// Connector ID
        connector_id: String,
        /// Capability 名称
        capability: String,
        /// 参数
        params: serde_json::Value,
    },
    /// 执行 Agent
    ExecuteAgent {
        /// Agent ID
        agent_id: String,
        /// 任务描述
        task: String,
    },
}

/// 调度任务
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    /// 任务 ID
    pub id: TaskId,
    /// 任务名称
    pub name: String,
    /// Cron 表达式
    pub cron_expr: String,
    /// 解析后的调度
    #[serde(skip)]
    pub schedule: Option<Schedule>,
    /// 任务动作
    pub action: TaskAction,
    /// 是否启用
    pub enabled: bool,
    /// 创建时间
    pub created_at: DateTime<Utc>,
    /// 最后执行时间
    pub last_execution: Option<DateTime<Utc>>,
    /// 下次执行时间
    pub next_execution: Option<DateTime<Utc>>,
}

impl ScheduledTask {
    /// 创建新的调度任务
    pub fn new(name: String, cron_expr: String, action: TaskAction) -> crate::errors::Result<Self> {
        // 解析 Cron 表达式
        let schedule = Schedule::from_str(&cron_expr)
            .map_err(|e| crate::errors::SchedulerError::InvalidCronExpression(e.to_string()))?;

        let now = Utc::now();
        let next_execution = schedule.upcoming(Utc).next();

        Ok(Self {
            id: TaskId::new(),
            name,
            cron_expr,
            schedule: Some(schedule),
            action,
            enabled: true,
            created_at: now,
            last_execution: None,
            next_execution,
        })
    }

    /// 更新下次执行时间
    pub fn update_next_execution(&mut self) {
        if let Some(schedule) = &self.schedule {
            self.next_execution = schedule.upcoming(Utc).next();
        }
    }

    /// 检查是否应该执行
    pub fn should_execute(&self, now: DateTime<Utc>) -> bool {
        if !self.enabled {
            return false;
        }

        if let Some(next_time) = self.next_execution {
            // 给予 1 分钟的容差
            next_time <= now
        } else {
            false
        }
    }
}

/// 执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExecutionResult {
    /// 成功
    Success {
        /// 输出数据
        output: serde_json::Value,
    },
    /// 失败
    Failure {
        /// 错误信息
        error: String,
    },
}

/// 执行记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRecord {
    /// 任务 ID
    pub task_id: TaskId,
    /// 执行 ID
    pub execution_id: ExecutionId,
    /// 执行时间
    pub timestamp: DateTime<Utc>,
    /// 执行结果
    pub result: ExecutionResult,
    /// 执行耗时 (毫秒)
    pub duration_ms: u64,
}

/// 执行历史
#[derive(Debug, Default)]
pub struct ExecutionHistory {
    /// 记录列表
    records: Vec<ExecutionRecord>,
    /// 最大记录数
    max_records: usize,
}

impl ExecutionHistory {
    /// 创建新的执行历史
    pub fn new(max_records: usize) -> Self {
        Self {
            records: Vec::new(),
            max_records,
        }
    }

    /// 记录执行
    pub fn record(&mut self, record: ExecutionRecord) {
        self.records.push(record);

        // 保持记录数量在限制内
        if self.records.len() > self.max_records {
            self.records.remove(0);
        }
    }

    /// 获取任务的执行历史
    pub fn get_task_history(&self, task_id: &TaskId) -> Vec<&ExecutionRecord> {
        self.records
            .iter()
            .filter(|r| &r.task_id == task_id)
            .collect()
    }

    /// 获取所有记录
    pub fn all_records(&self) -> &[ExecutionRecord] {
        &self.records
    }

    /// 获取最近的 N 条记录
    pub fn recent_records(&self, count: usize) -> &[ExecutionRecord] {
        let start = self.records.len().saturating_sub(count);
        &self.records[start..]
    }
}
