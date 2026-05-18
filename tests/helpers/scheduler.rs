// tests/helpers/scheduler.rs
// Scheduler 测试辅助工具

use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Mock 心跳监控器
pub struct MockHeartbeatMonitor {
    nodes: Arc<RwLock<HashMap<String, NodeInfo>>>,
    heartbeat_interval: std::time::Duration,
}

#[derive(Clone, Debug)]
pub struct NodeInfo {
    pub id: String,
    pub last_heartbeat: DateTime<Utc>,
    pub cpu_usage: f64,
    pub memory_usage: f64,
    pub disk_usage: f64,
    pub status: NodeStatus,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NodeStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Offline,
}

impl MockHeartbeatMonitor {
    pub async fn new(interval_secs: u64) -> Self {
        Self {
            nodes: Arc::new(RwLock::new(HashMap::new())),
            heartbeat_interval: std::time::Duration::from_secs(interval_secs),
        }
    }

    /// 注册节点
    pub async fn register_node(&self, node_id: &str) -> Result<()> {
        let mut nodes = self.nodes.write().await;
        nodes.insert(
            node_id.to_string(),
            NodeInfo {
                id: node_id.to_string(),
                last_heartbeat: Utc::now(),
                cpu_usage: 0.0,
                memory_usage: 0.0,
                disk_usage: 0.0,
                status: NodeStatus::Healthy,
            },
        );
        Ok(())
    }

    /// 更新心跳
    pub async fn heartbeat(
        &self,
        node_id: &str,
        cpu: f64,
        memory: f64,
        disk: f64,
    ) -> Result<()> {
        let mut nodes = self.nodes.write().await;
        if let Some(node) = nodes.get_mut(node_id) {
            node.last_heartbeat = Utc::now();
            node.cpu_usage = cpu;
            node.memory_usage = memory;
            node.disk_usage = disk;

            // 根据资源使用情况更新状态
            node.status = if cpu > 90.0 || memory > 90.0 || disk > 90.0 {
                NodeStatus::Unhealthy
            } else if cpu > 70.0 || memory > 70.0 || disk > 70.0 {
                NodeStatus::Degraded
            } else {
                NodeStatus::Healthy
            };

            Ok(())
        } else {
            Err(anyhow::anyhow!("Node not registered"))
        }
    }

    /// 检查节点状态
    pub async fn check_node(&self, node_id: &str) -> Option<NodeStatus> {
        let nodes = self.nodes.read().await;
        nodes.get(node_id).map(|n| n.status.clone())
    }

    /// 获取所有节点状态
    pub async fn get_all_nodes(&self) -> Vec<NodeInfo> {
        self.nodes.read().await.values().cloned().collect()
    }

    /// 模拟节点离线
    pub async fn simulate_offline(&self, node_id: &str) -> Result<()> {
        let mut nodes = self.nodes.write().await;
        if let Some(node) = nodes.get_mut(node_id) {
            node.status = NodeStatus::Offline;
            Ok(())
        } else {
            Err(anyhow::anyhow!("Node not found"))
        }
    }
}

/// Mock Cron 调度器
pub struct MockCronScheduler {
    tasks: Arc<RwLock<HashMap<String, ScheduledTask>>>,
    execution_history: Arc<RwLock<Vec<ExecutionRecord>>>,
}

#[derive(Clone, Debug)]
pub struct ScheduledTask {
    pub id: String,
    pub name: String,
    pub cron_expr: String,
    pub action: TaskAction,
    pub enabled: bool,
    pub next_execution: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub enum TaskAction {
    ExecuteCapability {
        connector_id: String,
        capability: String,
        params: serde_json::Value,
    },
    ExecuteAgent {
        agent_id: String,
        task: String,
    },
    ExecuteCallback {
        callback: Arc<dyn Fn() -> Result<()> + Send + Sync>,
    },
}

#[derive(Clone, Debug)]
pub struct ExecutionRecord {
    pub task_id: String,
    pub execution_id: String,
    pub timestamp: DateTime<Utc>,
    pub success: bool,
    pub error: Option<String>,
}

impl MockCronScheduler {
    pub async fn new() -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            execution_history: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// 添加定时任务
    pub async fn schedule_task(
        &self,
        name: &str,
        cron_expr: &str,
        action: TaskAction,
    ) -> Result<String> {
        let task_id = Uuid::new_v4().to_string();

        let task = ScheduledTask {
            id: task_id.clone(),
            name: name.to_string(),
            cron_expr: cron_expr.to_string(),
            action,
            enabled: true,
            next_execution: Self::calculate_next_execution(cron_expr)?,
        };

        let mut tasks = self.tasks.write().await;
        tasks.insert(task_id.clone(), task);

        Ok(task_id)
    }

    /// 计算下次执行时间
    fn calculate_next_execution(cron_expr: &str) -> Result<Option<DateTime<Utc>>> {
        // 简化实现，实际应该使用 cron 解析库
        match cron_expr {
            "* * * * *" => Ok(Some(Utc::now() + chrono::Duration::minutes(1))),
            "*/5 * * * *" => Ok(Some(Utc::now() + chrono::Duration::minutes(5))),
            "0 * * * *" => Ok(Some(Utc::now() + chrono::Duration::hours(1))),
            _ => Ok(None),
        }
    }

    /// 执行任务
    pub async fn execute_task(&self, task_id: &str) -> Result<()> {
        let tasks = self.tasks.read().await;

        if let Some(task) = tasks.get(task_id) {
            if !task.enabled {
                return Err(anyhow::anyhow!("Task is disabled"));
            }

            let execution_id = Uuid::new_v4().to_string();
            let mut success = true;
            let mut error = None;

            // 执行任务动作
            match &task.action {
                TaskAction::ExecuteCallback { callback } => {
                    if let Err(e) = callback() {
                        success = false;
                        error = Some(e.to_string());
                    }
                }
                _ => {
                    // 其他类型的任务模拟执行
                    println!("Executing task: {}", task.name);
                }
            }

            // 记录执行历史
            let record = ExecutionRecord {
                task_id: task_id.to_string(),
                execution_id,
                timestamp: Utc::now(),
                success,
                error,
            };

            let mut history = self.execution_history.write().await;
            history.push(record);

            Ok(())
        } else {
            Err(anyhow::anyhow!("Task not found"))
        }
    }

    /// 启用/禁用任务
    pub async fn set_task_enabled(&self, task_id: &str, enabled: bool) -> Result<()> {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(task_id) {
            task.enabled = enabled;
            Ok(())
        } else {
            Err(anyhow::anyhow!("Task not found"))
        }
    }

    /// 获取执行历史
    pub async fn get_execution_history(&self, task_id: &str) -> Vec<ExecutionRecord> {
        let history = self.execution_history.read().await;
        history
            .iter()
            .filter(|r| r.task_id == task_id)
            .cloned()
            .collect()
    }

    /// 获取所有任务
    pub async fn get_all_tasks(&self) -> Vec<ScheduledTask> {
        self.tasks.read().await.values().cloned().collect()
    }

    /// 模拟触发所有到期任务
    pub async fn trigger_due_tasks(&self) -> Result<Vec<String>> {
        let now = Utc::now();
        let tasks = self.tasks.read().await;

        let mut executed = Vec::new();

        for task in tasks.values() {
            if let Some(next_exec) = task.next_execution {
                if next_exec <= now && task.enabled {
                    self.execute_task(&task.id).await?;
                    executed.push(task.id.clone());
                }
            }
        }

        Ok(executed)
    }
}

/// 创建测试用的 Cron 表达式
pub fn test_cron_expressions() -> Vec<(&'static str, &'static str)> {
    vec![
        ("* * * * *", "Every minute"),
        ("*/5 * * * *", "Every 5 minutes"),
        ("0 * * * *", "Every hour"),
        ("0 2 * * *", "Daily at 2 AM"),
        ("0 0 * * MON", "Weekly on Monday"),
        ("0 0 1 * *", "Monthly on the 1st"),
    ]
}

/// 测试异常检测器
pub struct TestAnomalyDetector {
    thresholds: AnomalyThresholds,
}

#[derive(Clone, Debug)]
pub struct AnomalyThresholds {
    pub cpu_threshold: f64,
    pub memory_threshold: f64,
    pub disk_threshold: f64,
}

impl Default for AnomalyThresholds {
    fn default() -> Self {
        Self {
            cpu_threshold: 80.0,
            memory_threshold: 85.0,
            disk_threshold: 90.0,
        }
    }
}

impl TestAnomalyDetector {
    pub fn new() -> Self {
        Self {
            thresholds: AnomalyThresholds::default(),
        }
    }

    pub fn with_thresholds(thresholds: AnomalyThresholds) -> Self {
        Self { thresholds }
    }

    pub fn detect(&self, node: &NodeInfo) -> Vec<String> {
        let mut anomalies = Vec::new();

        if node.cpu_usage > self.thresholds.cpu_threshold {
            anomalies.push(format!(
                "High CPU usage: {:.1}%",
                node.cpu_usage
            ));
        }

        if node.memory_usage > self.thresholds.memory_threshold {
            anomalies.push(format!(
                "High memory usage: {:.1}%",
                node.memory_usage
            ));
        }

        if node.disk_usage > self.thresholds.disk_threshold {
            anomalies.push(format!(
                "High disk usage: {:.1}%",
                node.disk_usage
            ));
        }

        anomalies
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_heartbeat_monitor() {
        let monitor = MockHeartbeatMonitor::new(5).await;

        // 注册节点
        monitor.register_node("node1").await.unwrap();

        // 更新心跳
        monitor.heartbeat("node1", 50.0, 60.0, 30.0).await.unwrap();

        // 检查状态
        let status = monitor.check_node("node1").await;
        assert_eq!(status, Some(NodeStatus::Healthy));

        // 模拟高负载
        monitor.heartbeat("node1", 95.0, 85.0, 40.0).await.unwrap();
        let status = monitor.check_node("node1").await;
        assert_eq!(status, Some(NodeStatus::Unhealthy));
    }

    #[tokio::test]
    async fn test_cron_scheduler() {
        let scheduler = MockCronScheduler::new().await;

        // 创建回调任务
        let called = Arc::new(std::sync::Mutex::new(false));
        let called_clone = called.clone();

        let action = TaskAction::ExecuteCallback {
            callback: Arc::new(move || {
                *called_clone.lock().unwrap() = true;
                Ok(())
            }),
        };

        let task_id = scheduler
            .schedule_task("test_task", "* * * * *", action)
            .await
            .unwrap();

        // 执行任务
        scheduler.execute_task(&task_id).await.unwrap();

        // 验证回调被调用
        assert!(*called.lock().unwrap());

        // 检查执行历史
        let history = scheduler.get_execution_history(&task_id).await;
        assert_eq!(history.len(), 1);
        assert!(history[0].success);
    }
}