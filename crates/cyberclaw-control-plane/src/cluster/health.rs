use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;
use uuid::Uuid;

use super::error::ClusterError;
use super::node::{ClusterNode, NodeStatus};

pub struct HealthChecker {
    pub(super) nodes: Arc<RwLock<HashMap<Uuid, Arc<RwLock<ClusterNode>>>>>,
    check_interval: Duration,
    timeout: Duration,
    running: Arc<RwLock<bool>>,
}

impl HealthChecker {
    pub fn new(check_interval: Duration, timeout: Duration) -> Self {
        Self {
            nodes: Arc::new(RwLock::new(HashMap::new())),
            check_interval,
            timeout,
            running: Arc::new(RwLock::new(false)),
        }
    }

    pub async fn add_node(&self, node: ClusterNode) {
        let node_id = node.info.id;
        let mut nodes = self.nodes.write().await;
        nodes.insert(node_id, Arc::new(RwLock::new(node)));
    }

    pub async fn remove_node(&self, node_id: Uuid) {
        let mut nodes = self.nodes.write().await;
        nodes.remove(&node_id);
    }

    pub async fn start(&self) {
        let mut running = self.running.write().await;
        if *running {
            return;
        }
        *running = true;
        drop(running);

        let nodes = self.nodes.clone();
        let check_interval = self.check_interval;
        let timeout = self.timeout;
        let running_flag = self.running.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(check_interval);

            loop {
                interval.tick().await;

                {
                    let should_run = running_flag.read().await;
                    if !*should_run {
                        break;
                    }
                }

                let nodes = nodes.read().await;
                for node_lock in nodes.values() {
                    let mut node = node_lock.write().await;
                    match Self::check_node_health(&mut node, timeout).await {
                        Ok(true) => {
                            if node.info.status != NodeStatus::Online {
                                node.mark_online();
                                tracing::info!("Node {} recovered", node.info.id);
                            }
                        }
                        Ok(false) | Err(_) => {
                            if node.info.status == NodeStatus::Online {
                                node.mark_offline();
                                tracing::warn!("Node {} marked offline", node.info.id);
                            }
                        }
                    }
                }
            }

            tracing::info!("Health checker stopped");
        });
    }

    pub async fn stop(&self) {
        let mut running = self.running.write().await;
        *running = false;
    }

    async fn check_node_health(
        node: &mut ClusterNode,
        timeout: Duration,
    ) -> Result<bool, ClusterError> {
        let now = SystemTime::now();
        let elapsed = now
            .duration_since(node.info.last_heartbeat)
            .map_err(|e| ClusterError::TimeError(e.to_string()))?;

        Ok(elapsed < timeout)
    }

    pub async fn update_node_heartbeat(&self, node_id: Uuid) -> Result<(), ClusterError> {
        let nodes = self.nodes.read().await;
        let node_lock = nodes
            .get(&node_id)
            .ok_or(ClusterError::NodeNotFound(node_id))?;

        let mut node = node_lock.write().await;
        node.update_heartbeat();
        Ok(())
    }

    pub async fn get_node_status(&self, node_id: Uuid) -> Result<NodeStatus, ClusterError> {
        let nodes = self.nodes.read().await;
        let node_lock = nodes
            .get(&node_id)
            .ok_or(ClusterError::NodeNotFound(node_id))?;

        let node = node_lock.read().await;
        Ok(node.info.status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::node::NodeInfo;

    fn create_test_node(address: &str, port: u16) -> ClusterNode {
        ClusterNode::new(NodeInfo::new(address.to_string(), port, 10))
    }

    #[tokio::test]
    async fn test_health_check_timeout() {
        let mut node = create_test_node("127.0.0.1", 8081);

        // Set heartbeat to past
        node.info.last_heartbeat = SystemTime::now() - Duration::from_secs(60);

        let timeout = Duration::from_secs(30);
        let result = HealthChecker::check_node_health(&mut node, timeout)
            .await
            .unwrap();

        assert!(!result);
    }

    #[tokio::test]
    async fn test_health_check_success() {
        let mut node = create_test_node("127.0.0.1", 8081);

        // Set heartbeat to now
        node.info.last_heartbeat = SystemTime::now();

        let timeout = Duration::from_secs(30);
        let result = HealthChecker::check_node_health(&mut node, timeout)
            .await
            .unwrap();

        assert!(result);
    }

    #[tokio::test]
    async fn test_update_heartbeat() {
        let checker = HealthChecker::new(Duration::from_secs(5), Duration::from_secs(30));

        let node = create_test_node("127.0.0.1", 8081);
        let node_id = node.info.id;

        checker.add_node(node).await;

        // Sleep to ensure time difference
        tokio::time::sleep(Duration::from_millis(100)).await;

        let old_heartbeat = {
            let nodes = checker.nodes.read().await;
            let node_lock = nodes.get(&node_id).unwrap();
            let node = node_lock.read().await;
            node.info.last_heartbeat
        };

        checker.update_node_heartbeat(node_id).await.unwrap();

        let new_heartbeat = {
            let nodes = checker.nodes.read().await;
            let node_lock = nodes.get(&node_id).unwrap();
            let node = node_lock.read().await;
            node.info.last_heartbeat
        };

        assert!(new_heartbeat > old_heartbeat);
    }

    #[tokio::test]
    async fn test_start_stop_health_checker() {
        let checker = HealthChecker::new(Duration::from_millis(100), Duration::from_secs(30));

        let node = create_test_node("127.0.0.1", 8081);
        checker.add_node(node).await;

        checker.start().await;

        {
            let running = checker.running.read().await;
            assert!(*running);
        }

        checker.stop().await;

        {
            let running = checker.running.read().await;
            assert!(!*running);
        }
    }

    #[tokio::test]
    async fn test_health_checker_marks_offline() {
        let checker = HealthChecker::new(Duration::from_millis(100), Duration::from_millis(50));

        let mut node = create_test_node("127.0.0.1", 8081);
        let node_id = node.info.id;

        // Set old heartbeat
        node.info.last_heartbeat = SystemTime::now() - Duration::from_secs(10);

        checker.add_node(node).await;
        checker.start().await;

        // Wait for health check cycle
        tokio::time::sleep(Duration::from_millis(200)).await;

        let status = checker.get_node_status(node_id).await.unwrap();
        assert_eq!(status, NodeStatus::Offline);

        checker.stop().await;
    }
}
