//! 共享类型定义
//!
//! 定义在多个模块间共享的数据类型

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use cyberclaw_core::enums::Priority;
use cyberclaw_core::identity::ActorRef;
use cyberclaw_core::task::Task;

// ===== Agent 相关类型 =====

/// Agent 执行状态
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentExecutionStatus {
    Idle,
    Running,
    Completed,
    Failed,
}

/// Agent 状态存储类型
pub type AgentStatusStore = Arc<RwLock<HashMap<String, AgentExecutionStatus>>>;

// ===== Task 相关类型 =====

/// 任务状态
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// 带状态的任务
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskWithStatus {
    pub task: Task,
    pub execution_id: Option<String>,
    pub status: TaskStatus,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

/// 任务存储类型
pub type TaskStore = Arc<RwLock<HashMap<String, TaskWithStatus>>>;

// ===== 响应类型 =====

/// 任务响应
#[derive(Debug, Serialize)]
pub struct TaskResponse {
    pub id: String,
    pub execution_id: Option<String>,
    pub case_id: Option<String>,
    pub title: String,
    pub summary: String,
    pub kind: cyberclaw_core::task::TaskKind,
    pub priority: Priority,
    pub status: TaskStatus,
    pub requested_by: ActorRef,
    pub requested_at: String,
    pub labels: Vec<String>,
}

impl From<TaskWithStatus> for TaskResponse {
    fn from(tws: TaskWithStatus) -> Self {
        TaskResponse {
            id: tws.task.id.as_str().to_string(),
            execution_id: tws.execution_id,
            case_id: tws.task.case_id.as_ref().map(|c| c.as_str().to_string()),
            title: tws.task.title.clone(),
            summary: tws.task.summary.clone(),
            kind: tws.task.kind.clone(),
            priority: tws.task.priority.clone(),
            status: tws.status,
            requested_by: tws.task.requested_by.clone(),
            requested_at: tws.task.requested_at.to_rfc3339(),
            labels: tws.task.labels.clone(),
        }
    }
}

/// 任务状态响应
#[derive(Debug, Serialize)]
pub struct TaskStatusResponse {
    pub id: String,
    pub execution_id: Option<String>,
    pub status: TaskStatus,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

/// Agent 信息响应
#[derive(Debug, Serialize)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub status: AgentExecutionStatus,
}
