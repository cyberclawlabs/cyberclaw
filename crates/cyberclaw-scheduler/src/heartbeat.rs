//! Heartbeat 监控模块
//!
//! 提供节点健康检查、异常检测和状态管理功能。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// 节点 ID
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(String);

impl NodeId {
    /// 创建新的节点 ID
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    /// 从字符串创建节点 ID
    pub fn from_string(s: String) -> Self {
        Self(s)
    }

    /// 获取内部字符串
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for NodeId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 节点状态
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeStatus {
    /// 健康
    Healthy,
    /// 降级（部分功能受限）
    Degraded,
    /// 不健康
    Unhealthy,
    /// 离线
    Offline,
}

/// 节点信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    /// 节点 ID
    pub id: NodeId,
    /// 最后心跳时间
    #[serde(skip, default = "Instant::now")]
    pub last_heartbeat: Instant,
    /// CPU 使用率 (0.0-100.0)
    pub cpu_usage: f64,
    /// 内存使用率 (0.0-100.0)
    pub memory_usage: f64,
    /// 磁盘使用率 (0.0-100.0)
    pub disk_usage: f64,
    /// 节点状态
    pub status: NodeStatus,
    /// 额外的元数据
    pub metadata: HashMap<String, String>,
}

impl NodeInfo {
    /// 创建新的节点信息
    pub fn new(id: NodeId) -> Self {
        Self {
            id,
            last_heartbeat: Instant::now(),
            cpu_usage: 0.0,
            memory_usage: 0.0,
            disk_usage: 0.0,
            status: NodeStatus::Healthy,
            metadata: HashMap::new(),
        }
    }

    /// 更新心跳
    pub fn update_heartbeat(&mut self) {
        self.last_heartbeat = Instant::now();
    }

    /// 更新资源使用率
    pub fn update_resources(&mut self, cpu: f64, memory: f64, disk: f64) {
        self.cpu_usage = cpu;
        self.memory_usage = memory;
        self.disk_usage = disk;
        self.update_heartbeat();
    }
}

/// 异常类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Anomaly {
    /// 高 CPU 使用率
    HighCpuUsage(f64),
    /// 高内存使用率
    HighMemoryUsage(f64),
    /// 高磁盘使用率
    HighDiskUsage(f64),
    /// 心跳不规律
    HeartbeatIrregular,
    /// 资源使用率突增
    ResourceSpike { resource: String, value: f64 },
}

/// Heartbeat 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatConfig {
    /// 监控间隔
    pub interval_secs: u64,
    /// 心跳超时倍数（基于 interval）
    pub timeout_multiplier: u32,
    /// CPU 使用率告警阈值
    pub cpu_threshold: f64,
    /// 内存使用率告警阈值
    pub memory_threshold: f64,
    /// 磁盘使用率告警阈值
    pub disk_threshold: f64,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            interval_secs: 30,
            timeout_multiplier: 3,
            cpu_threshold: 80.0,
            memory_threshold: 85.0,
            disk_threshold: 90.0,
        }
    }
}

/// 健康检查器
#[derive(Debug)]
pub struct HealthChecker {
    config: HeartbeatConfig,
}

impl HealthChecker {
    /// 创建新的健康检查器
    pub fn new(config: HeartbeatConfig) -> Self {
        Self { config }
    }

    /// 检查节点健康状态
    pub async fn check(&self, node: &NodeInfo) -> Result<NodeStatus> {
        // 1. 检查心跳超时
        let timeout =
            Duration::from_secs(self.config.interval_secs) * self.config.timeout_multiplier;
        if node.last_heartbeat.elapsed() > timeout {
            return Ok(NodeStatus::Offline);
        }

        // 2. 检查资源使用率
        let mut degraded_count = 0;
        let mut unhealthy_count = 0;

        if node.cpu_usage > self.config.cpu_threshold {
            if node.cpu_usage > self.config.cpu_threshold + 10.0 {
                unhealthy_count += 1;
            } else {
                degraded_count += 1;
            }
        }

        if node.memory_usage > self.config.memory_threshold {
            if node.memory_usage > self.config.memory_threshold + 10.0 {
                unhealthy_count += 1;
            } else {
                degraded_count += 1;
            }
        }

        if node.disk_usage > self.config.disk_threshold {
            if node.disk_usage > self.config.disk_threshold + 5.0 {
                unhealthy_count += 1;
            } else {
                degraded_count += 1;
            }
        }

        // 3. 确定状态
        if unhealthy_count > 0 {
            Ok(NodeStatus::Unhealthy)
        } else if degraded_count > 0 {
            Ok(NodeStatus::Degraded)
        } else {
            Ok(NodeStatus::Healthy)
        }
    }
}

/// 异常检测器
#[derive(Debug)]
pub struct AnomalyDetector {
    config: HeartbeatConfig,
    /// 历史数据（用于检测突变）
    history: Arc<RwLock<HashMap<NodeId, Vec<NodeSnapshot>>>>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct NodeSnapshot {
    timestamp: Instant,
    cpu_usage: f64,
    memory_usage: f64,
    disk_usage: f64,
}

impl AnomalyDetector {
    /// 创建新的异常检测器
    pub fn new(config: HeartbeatConfig) -> Self {
        Self {
            config,
            history: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 检测异常
    pub async fn detect(&self, node: &NodeInfo) -> Result<Option<Anomaly>> {
        // 1. 检查资源使用率阈值
        if node.cpu_usage > self.config.cpu_threshold {
            return Ok(Some(Anomaly::HighCpuUsage(node.cpu_usage)));
        }

        if node.memory_usage > self.config.memory_threshold {
            return Ok(Some(Anomaly::HighMemoryUsage(node.memory_usage)));
        }

        if node.disk_usage > self.config.disk_threshold {
            return Ok(Some(Anomaly::HighDiskUsage(node.disk_usage)));
        }

        // 2. 检查资源使用率突增
        let mut history = self.history.write().await;
        let snapshots = history.entry(node.id.clone()).or_insert_with(Vec::new);

        // 保存当前快照
        snapshots.push(NodeSnapshot {
            timestamp: Instant::now(),
            cpu_usage: node.cpu_usage,
            memory_usage: node.memory_usage,
            disk_usage: node.disk_usage,
        });

        // 只保留最近 10 个快照
        if snapshots.len() > 10 {
            snapshots.remove(0);
        }

        // 检测突增（与平均值相比增长超过 50%）
        if snapshots.len() >= 3 {
            // usize → f64 精度损失是可接受的 (用于统计计算)
            #[allow(clippy::cast_precision_loss)]
            let avg_cpu: f64 =
                snapshots.iter().map(|s| s.cpu_usage).sum::<f64>() / snapshots.len() as f64;
            #[allow(clippy::cast_precision_loss)]
            let avg_memory: f64 =
                snapshots.iter().map(|s| s.memory_usage).sum::<f64>() / snapshots.len() as f64;

            if node.cpu_usage > avg_cpu * 1.5 && node.cpu_usage > 50.0 {
                return Ok(Some(Anomaly::ResourceSpike {
                    resource: "CPU".to_string(),
                    value: node.cpu_usage,
                }));
            }

            if node.memory_usage > avg_memory * 1.5 && node.memory_usage > 50.0 {
                return Ok(Some(Anomaly::ResourceSpike {
                    resource: "Memory".to_string(),
                    value: node.memory_usage,
                }));
            }
        }

        Ok(None)
    }
}

/// Heartbeat 监控器
pub struct HeartbeatMonitor {
    /// 配置
    config: HeartbeatConfig,
    /// 节点注册表
    nodes: Arc<RwLock<HashMap<NodeId, NodeInfo>>>,
    /// 健康检查器
    health_checker: Arc<HealthChecker>,
    /// 异常检测器
    anomaly_detector: Arc<AnomalyDetector>,
    /// 是否运行中
    running: Arc<RwLock<bool>>,
}

impl HeartbeatMonitor {
    /// 创建新的 Heartbeat 监控器
    pub fn new(config: HeartbeatConfig) -> Self {
        let health_checker = Arc::new(HealthChecker::new(config.clone()));
        let anomaly_detector = Arc::new(AnomalyDetector::new(config.clone()));

        Self {
            config,
            nodes: Arc::new(RwLock::new(HashMap::new())),
            health_checker,
            anomaly_detector,
            running: Arc::new(RwLock::new(false)),
        }
    }

    /// 使用默认配置创建
    pub fn with_default() -> Self {
        Self::new(HeartbeatConfig::default())
    }

    /// 注册节点
    pub async fn register_node(&self, node_id: NodeId) -> Result<()> {
        let node = NodeInfo::new(node_id.clone());
        self.nodes.write().await.insert(node_id.clone(), node);
        info!("Node {} registered", node_id);
        Ok(())
    }

    /// 注销节点
    pub async fn unregister_node(&self, node_id: &NodeId) -> Result<()> {
        self.nodes.write().await.remove(node_id);
        info!("Node {} unregistered", node_id);
        Ok(())
    }

    /// 上报心跳
    pub async fn report_heartbeat(
        &self,
        node_id: &NodeId,
        cpu_usage: f64,
        memory_usage: f64,
        disk_usage: f64,
    ) -> Result<()> {
        let mut nodes = self.nodes.write().await;
        if let Some(node) = nodes.get_mut(node_id) {
            node.update_resources(cpu_usage, memory_usage, disk_usage);
            debug!(
                "Heartbeat from node {}: CPU={:.2}%, Memory={:.2}%, Disk={:.2}%",
                node_id, cpu_usage, memory_usage, disk_usage
            );
        } else {
            warn!("Heartbeat from unregistered node: {}", node_id);
        }
        Ok(())
    }

    /// 获取节点状态
    pub async fn get_node_status(&self, node_id: &NodeId) -> Result<Option<NodeStatus>> {
        let nodes = self.nodes.read().await;
        Ok(nodes.get(node_id).map(|n| n.status.clone()))
    }

    /// 获取所有节点
    pub async fn list_nodes(&self) -> Result<Vec<NodeInfo>> {
        let nodes = self.nodes.read().await;
        Ok(nodes.values().cloned().collect())
    }

    /// 标记节点为离线
    async fn mark_node_offline(&self, node_id: &NodeId) -> Result<()> {
        let mut nodes = self.nodes.write().await;
        if let Some(node) = nodes.get_mut(node_id) {
            node.status = NodeStatus::Offline;
            warn!("Node {} marked as offline", node_id);
        }
        Ok(())
    }

    /// 更新节点状态
    async fn update_node_status(&self, node_id: &NodeId, status: NodeStatus) -> Result<()> {
        let mut nodes = self.nodes.write().await;
        if let Some(node) = nodes.get_mut(node_id) {
            if node.status != status {
                info!(
                    "Node {} status changed: {:?} -> {:?}",
                    node_id, node.status, status
                );
                node.status = status;
            }
        }
        Ok(())
    }

    /// 处理异常
    async fn handle_anomaly(&self, node_id: &NodeId, anomaly: Anomaly) -> Result<()> {
        match &anomaly {
            Anomaly::HighCpuUsage(usage) => {
                warn!("Node {} high CPU usage: {:.2}%", node_id, usage);
            }
            Anomaly::HighMemoryUsage(usage) => {
                warn!("Node {} high memory usage: {:.2}%", node_id, usage);
            }
            Anomaly::HighDiskUsage(usage) => {
                warn!("Node {} high disk usage: {:.2}%", node_id, usage);
            }
            Anomaly::HeartbeatIrregular => {
                warn!("Node {} heartbeat irregular", node_id);
            }
            Anomaly::ResourceSpike { resource, value } => {
                warn!("Node {} {} spike: {:.2}%", node_id, resource, value);
            }
        }
        Ok(())
    }

    /// 启动监控
    pub async fn start(&self) -> Result<()> {
        // 检查是否已经在运行
        {
            let mut running = self.running.write().await;
            if *running {
                return Err(anyhow::anyhow!("HeartbeatMonitor is already running"));
            }
            *running = true;
        }

        info!(
            "Starting HeartbeatMonitor with interval {}s",
            self.config.interval_secs
        );

        let mut interval = tokio::time::interval(Duration::from_secs(self.config.interval_secs));

        loop {
            interval.tick().await;

            // 检查是否应该停止
            if !*self.running.read().await {
                info!("HeartbeatMonitor stopped");
                break;
            }

            // 获取所有节点（克隆以释放锁）
            let nodes = self.nodes.read().await.clone();

            for (node_id, node_info) in nodes {
                // 检查心跳超时
                let timeout =
                    Duration::from_secs(self.config.interval_secs) * self.config.timeout_multiplier;
                if node_info.last_heartbeat.elapsed() > timeout {
                    self.mark_node_offline(&node_id).await?;
                    continue;
                }

                // 健康检查
                match self.health_checker.check(&node_info).await {
                    Ok(status) => {
                        self.update_node_status(&node_id, status).await?;
                    }
                    Err(e) => {
                        error!("Health check failed for node {}: {}", node_id, e);
                    }
                }

                // 异常检测
                match self.anomaly_detector.detect(&node_info).await {
                    Ok(Some(anomaly)) => {
                        self.handle_anomaly(&node_id, anomaly).await?;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        error!("Anomaly detection failed for node {}: {}", node_id, e);
                    }
                }
            }
        }

        Ok(())
    }

    /// 停止监控
    pub async fn stop(&self) -> Result<()> {
        let mut running = self.running.write().await;
        *running = false;
        info!("Stopping HeartbeatMonitor");
        Ok(())
    }

    /// 检查是否正在运行
    pub async fn is_running(&self) -> bool {
        *self.running.read().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_node_id_creation() {
        let id1 = NodeId::new();
        let id2 = NodeId::new();
        assert_ne!(id1, id2);

        let id3 = NodeId::from_string("test-node".to_string());
        assert_eq!(id3.as_str(), "test-node");
    }

    #[tokio::test]
    async fn test_node_info_creation() {
        let id = NodeId::new();
        let node = NodeInfo::new(id.clone());

        assert_eq!(node.id, id);
        assert_eq!(node.status, NodeStatus::Healthy);
        assert_eq!(node.cpu_usage, 0.0);
        assert_eq!(node.memory_usage, 0.0);
        assert_eq!(node.disk_usage, 0.0);
    }

    #[tokio::test]
    async fn test_node_update_resources() {
        let id = NodeId::new();
        let mut node = NodeInfo::new(id);

        node.update_resources(50.0, 60.0, 70.0);

        assert_eq!(node.cpu_usage, 50.0);
        assert_eq!(node.memory_usage, 60.0);
        assert_eq!(node.disk_usage, 70.0);
    }

    #[tokio::test]
    async fn test_health_checker_healthy() {
        let config = HeartbeatConfig::default();
        let checker = HealthChecker::new(config);

        let id = NodeId::new();
        let mut node = NodeInfo::new(id);
        node.update_resources(50.0, 50.0, 50.0);

        let status = checker.check(&node).await.unwrap();
        assert_eq!(status, NodeStatus::Healthy);
    }

    #[tokio::test]
    async fn test_health_checker_degraded() {
        let config = HeartbeatConfig::default();
        let checker = HealthChecker::new(config);

        let id = NodeId::new();
        let mut node = NodeInfo::new(id);
        node.update_resources(85.0, 50.0, 50.0);

        let status = checker.check(&node).await.unwrap();
        assert_eq!(status, NodeStatus::Degraded);
    }

    #[tokio::test]
    async fn test_health_checker_unhealthy() {
        let config = HeartbeatConfig::default();
        let checker = HealthChecker::new(config);

        let id = NodeId::new();
        let mut node = NodeInfo::new(id);
        node.update_resources(95.0, 50.0, 50.0);

        let status = checker.check(&node).await.unwrap();
        assert_eq!(status, NodeStatus::Unhealthy);
    }

    #[tokio::test]
    async fn test_anomaly_detector_high_cpu() {
        let config = HeartbeatConfig::default();
        let detector = AnomalyDetector::new(config);

        let id = NodeId::new();
        let mut node = NodeInfo::new(id);
        node.update_resources(85.0, 50.0, 50.0);

        let anomaly = detector.detect(&node).await.unwrap();
        assert!(matches!(anomaly, Some(Anomaly::HighCpuUsage(_))));
    }

    #[tokio::test]
    async fn test_anomaly_detector_high_memory() {
        let config = HeartbeatConfig::default();
        let detector = AnomalyDetector::new(config);

        let id = NodeId::new();
        let mut node = NodeInfo::new(id);
        node.update_resources(50.0, 90.0, 50.0);

        let anomaly = detector.detect(&node).await.unwrap();
        assert!(matches!(anomaly, Some(Anomaly::HighMemoryUsage(_))));
    }

    #[tokio::test]
    async fn test_heartbeat_monitor_register_node() {
        let monitor = HeartbeatMonitor::with_default();
        let node_id = NodeId::new();

        monitor.register_node(node_id.clone()).await.unwrap();

        let nodes = monitor.list_nodes().await.unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, node_id);
    }

    #[tokio::test]
    async fn test_heartbeat_monitor_unregister_node() {
        let monitor = HeartbeatMonitor::with_default();
        let node_id = NodeId::new();

        monitor.register_node(node_id.clone()).await.unwrap();
        monitor.unregister_node(&node_id).await.unwrap();

        let nodes = monitor.list_nodes().await.unwrap();
        assert_eq!(nodes.len(), 0);
    }

    #[tokio::test]
    async fn test_heartbeat_monitor_report_heartbeat() {
        let monitor = HeartbeatMonitor::with_default();
        let node_id = NodeId::new();

        monitor.register_node(node_id.clone()).await.unwrap();
        monitor
            .report_heartbeat(&node_id, 50.0, 60.0, 70.0)
            .await
            .unwrap();

        let nodes = monitor.list_nodes().await.unwrap();
        assert_eq!(nodes[0].cpu_usage, 50.0);
        assert_eq!(nodes[0].memory_usage, 60.0);
        assert_eq!(nodes[0].disk_usage, 70.0);
    }

    #[tokio::test]
    async fn test_heartbeat_monitor_get_node_status() {
        let monitor = HeartbeatMonitor::with_default();
        let node_id = NodeId::new();

        monitor.register_node(node_id.clone()).await.unwrap();

        let status = monitor.get_node_status(&node_id).await.unwrap();
        assert_eq!(status, Some(NodeStatus::Healthy));
    }

    #[tokio::test]
    async fn test_heartbeat_monitor_start_stop() {
        let config = HeartbeatConfig {
            interval_secs: 1,
            ..Default::default()
        };
        let monitor = Arc::new(HeartbeatMonitor::new(config));

        let monitor_clone = monitor.clone();
        let handle = tokio::spawn(async move {
            monitor_clone.start().await.unwrap();
        });

        sleep(Duration::from_millis(100)).await;
        assert!(monitor.is_running().await);

        monitor.stop().await.unwrap();
        sleep(Duration::from_millis(100)).await;

        // 等待任务完成
        let _ = tokio::time::timeout(Duration::from_secs(3), handle).await;
    }
}
