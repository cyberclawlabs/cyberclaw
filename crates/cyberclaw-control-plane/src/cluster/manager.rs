use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::dispatcher::{DispatchStrategy, TaskDispatcher};
use super::error::ClusterError;
use super::health::HealthChecker;
use super::node::{ClusterNode, NodeInfo, NodeStatus};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClusterCommand {
    JoinNode(NodeInfo),
    LeaveNode(Uuid),
    UpdateNodeCapacity { node_id: Uuid, current_tasks: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterState {
    pub total_nodes: usize,
    pub online_nodes: usize,
    pub offline_nodes: usize,
    pub total_capacity: usize,
    pub used_capacity: usize,
    pub available_capacity: usize,
}

impl ClusterState {
    pub fn utilization(&self) -> f32 {
        if self.total_capacity == 0 {
            return 0.0;
        }
        (self.used_capacity as f32) / (self.total_capacity as f32)
    }
}

pub struct ClusterManager {
    dispatcher: Arc<TaskDispatcher>,
    health_checker: Arc<HealthChecker>,
    nodes: Arc<RwLock<HashMap<Uuid, Arc<RwLock<ClusterNode>>>>>,
}

impl ClusterManager {
    pub fn new(strategy: DispatchStrategy) -> Self {
        let dispatcher = Arc::new(TaskDispatcher::new(strategy));
        let health_checker = Arc::new(HealthChecker::new(
            Duration::from_secs(5),
            Duration::from_secs(30),
        ));

        Self {
            dispatcher,
            health_checker,
            nodes: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn with_health_check_interval(
        strategy: DispatchStrategy,
        check_interval: Duration,
        timeout: Duration,
    ) -> Self {
        let dispatcher = Arc::new(TaskDispatcher::new(strategy));
        let health_checker = Arc::new(HealthChecker::new(check_interval, timeout));

        Self {
            dispatcher,
            health_checker,
            nodes: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn start(&self) -> Result<(), ClusterError> {
        self.health_checker.start().await;
        Ok(())
    }

    pub async fn stop(&self) -> Result<(), ClusterError> {
        self.health_checker.stop().await;
        Ok(())
    }

    pub async fn join_node(&self, info: NodeInfo) -> Result<(), ClusterError> {
        let mut node = ClusterNode::new(info.clone());
        node.connect().await?;

        let node_id = node.info.id;
        let node_arc = Arc::new(RwLock::new(node));

        {
            let mut nodes = self.nodes.write().await;
            nodes.insert(node_id, node_arc.clone());
        }

        // Share the same node reference with dispatcher and health checker
        {
            let mut dispatcher_nodes = self.dispatcher.nodes.write().await;
            dispatcher_nodes.insert(node_id, node_arc.clone());
        }

        {
            let mut health_nodes = self.health_checker.nodes.write().await;
            health_nodes.insert(node_id, node_arc.clone());
        }

        tracing::info!("Node {} joined cluster", node_id);

        Ok(())
    }

    pub async fn leave_node(&self, node_id: Uuid) -> Result<(), ClusterError> {
        {
            let mut nodes = self.nodes.write().await;
            nodes.remove(&node_id);
        }

        self.dispatcher.remove_node(node_id).await;
        self.health_checker.remove_node(node_id).await;

        tracing::info!("Node {} left cluster", node_id);

        Ok(())
    }

    pub async fn dispatch_task(&self, task_id: Uuid) -> Result<Uuid, ClusterError> {
        let node_id = self.dispatcher.dispatch_task(task_id).await?;

        // Increment task count
        let nodes = self.nodes.read().await;
        if let Some(node_lock) = nodes.get(&node_id) {
            let mut node = node_lock.write().await;
            node.info.capacity.increment_tasks();
        }

        Ok(node_id)
    }

    pub async fn complete_task(&self, node_id: Uuid) -> Result<(), ClusterError> {
        let nodes = self.nodes.read().await;
        let node_lock = nodes
            .get(&node_id)
            .ok_or(ClusterError::NodeNotFound(node_id))?;

        let mut node = node_lock.write().await;
        node.info.capacity.decrement_tasks();

        Ok(())
    }

    pub async fn update_heartbeat(&self, node_id: Uuid) -> Result<(), ClusterError> {
        self.health_checker.update_node_heartbeat(node_id).await
    }

    pub async fn get_cluster_state(&self) -> ClusterState {
        let nodes = self.nodes.read().await;

        let mut total_nodes = 0;
        let mut online_nodes = 0;
        let mut offline_nodes = 0;
        let mut total_capacity = 0;
        let mut used_capacity = 0;

        for node_lock in nodes.values() {
            let node = node_lock.read().await;
            total_nodes += 1;

            match node.info.status {
                NodeStatus::Online => online_nodes += 1,
                NodeStatus::Offline | NodeStatus::Failed => offline_nodes += 1,
                NodeStatus::Draining => {}
            }

            total_capacity += node.info.capacity.max_concurrent_tasks;
            used_capacity += node.info.capacity.current_tasks;
        }

        ClusterState {
            total_nodes,
            online_nodes,
            offline_nodes,
            total_capacity,
            used_capacity,
            available_capacity: total_capacity.saturating_sub(used_capacity),
        }
    }

    pub async fn get_node_info(&self, node_id: Uuid) -> Result<NodeInfo, ClusterError> {
        let nodes = self.nodes.read().await;
        let node_lock = nodes
            .get(&node_id)
            .ok_or(ClusterError::NodeNotFound(node_id))?;

        let node = node_lock.read().await;
        Ok(node.info.clone())
    }

    pub async fn list_nodes(&self) -> Vec<NodeInfo> {
        let nodes = self.nodes.read().await;
        let mut result = Vec::new();
        for node_lock in nodes.values() {
            let node = node_lock.read().await;
            result.push(node.info.clone());
        }
        result
    }

    pub async fn node_count(&self) -> usize {
        let nodes = self.nodes.read().await;
        nodes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_node_info(address: &str, port: u16) -> NodeInfo {
        NodeInfo::new(address.to_string(), port, 10)
    }

    #[tokio::test]
    async fn test_join_and_leave_node() {
        let manager = ClusterManager::new(DispatchStrategy::RoundRobin);

        let info = create_test_node_info("127.0.0.1", 8081);
        let node_id = info.id;

        manager.join_node(info).await.unwrap();
        assert_eq!(manager.node_count().await, 1);

        manager.leave_node(node_id).await.unwrap();
        assert_eq!(manager.node_count().await, 0);
    }

    #[tokio::test]
    async fn test_dispatch_task() {
        let manager = ClusterManager::new(DispatchStrategy::RoundRobin);

        let info = create_test_node_info("127.0.0.1", 8081);
        let node_id = info.id;

        manager.join_node(info).await.unwrap();

        let task_id = Uuid::new_v4();
        let assigned_node = manager.dispatch_task(task_id).await.unwrap();

        assert_eq!(assigned_node, node_id);

        // Check task count incremented
        let node_info = manager.get_node_info(node_id).await.unwrap();
        assert_eq!(node_info.capacity.current_tasks, 1);
    }

    #[tokio::test]
    async fn test_complete_task() {
        let manager = ClusterManager::new(DispatchStrategy::RoundRobin);

        let info = create_test_node_info("127.0.0.1", 8081);
        let node_id = info.id;

        manager.join_node(info).await.unwrap();

        let task_id = Uuid::new_v4();
        manager.dispatch_task(task_id).await.unwrap();

        manager.complete_task(node_id).await.unwrap();

        let node_info = manager.get_node_info(node_id).await.unwrap();
        assert_eq!(node_info.capacity.current_tasks, 0);
    }

    #[tokio::test]
    async fn test_cluster_state() {
        let manager = ClusterManager::new(DispatchStrategy::RoundRobin);

        let info1 = create_test_node_info("127.0.0.1", 8081);
        let info2 = create_test_node_info("127.0.0.1", 8082);

        manager.join_node(info1).await.unwrap();
        manager.join_node(info2).await.unwrap();

        let state = manager.get_cluster_state().await;

        assert_eq!(state.total_nodes, 2);
        assert_eq!(state.online_nodes, 2);
        assert_eq!(state.total_capacity, 20);
        assert_eq!(state.used_capacity, 0);
        assert_eq!(state.available_capacity, 20);
    }

    #[tokio::test]
    async fn test_multiple_task_dispatch() {
        let manager = ClusterManager::new(DispatchStrategy::RoundRobin);

        let info1 = create_test_node_info("127.0.0.1", 8081);
        let info2 = create_test_node_info("127.0.0.1", 8082);

        manager.join_node(info1).await.unwrap();
        manager.join_node(info2).await.unwrap();

        for _ in 0..5 {
            let task_id = Uuid::new_v4();
            manager.dispatch_task(task_id).await.unwrap();
        }

        let state = manager.get_cluster_state().await;
        assert_eq!(state.used_capacity, 5);
    }

    #[tokio::test]
    async fn test_list_nodes() {
        let manager = ClusterManager::new(DispatchStrategy::RoundRobin);

        let info1 = create_test_node_info("127.0.0.1", 8081);
        let info2 = create_test_node_info("127.0.0.1", 8082);

        manager.join_node(info1.clone()).await.unwrap();
        manager.join_node(info2.clone()).await.unwrap();

        let nodes = manager.list_nodes().await;
        assert_eq!(nodes.len(), 2);

        let ids: Vec<_> = nodes.iter().map(|n| n.id).collect();
        assert!(ids.contains(&info1.id));
        assert!(ids.contains(&info2.id));
    }
}
