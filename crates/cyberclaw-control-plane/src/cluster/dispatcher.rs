use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::error::ClusterError;
use super::node::ClusterNode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchStrategy {
    RoundRobin,
    LeastLoaded,
    ConsistentHash,
}

pub struct TaskDispatcher {
    pub(super) nodes: Arc<RwLock<HashMap<Uuid, Arc<RwLock<ClusterNode>>>>>,
    strategy: DispatchStrategy,
    current_index: Arc<RwLock<usize>>,
}

impl TaskDispatcher {
    pub fn new(strategy: DispatchStrategy) -> Self {
        Self {
            nodes: Arc::new(RwLock::new(HashMap::new())),
            strategy,
            current_index: Arc::new(RwLock::new(0)),
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

    pub async fn get_node(&self, node_id: Uuid) -> Option<Arc<RwLock<ClusterNode>>> {
        let nodes = self.nodes.read().await;
        nodes.get(&node_id).cloned()
    }

    pub async fn dispatch_task(&self, task_id: Uuid) -> Result<Uuid, ClusterError> {
        match self.strategy {
            DispatchStrategy::RoundRobin => self.round_robin_dispatch().await,
            DispatchStrategy::LeastLoaded => self.least_loaded_dispatch().await,
            DispatchStrategy::ConsistentHash => self.consistent_hash_dispatch(task_id).await,
        }
    }

    async fn round_robin_dispatch(&self) -> Result<Uuid, ClusterError> {
        let nodes = self.nodes.read().await;
        if nodes.is_empty() {
            return Err(ClusterError::NoAvailableNodes);
        }

        let node_list: Vec<_> = nodes.values().cloned().collect();
        drop(nodes);

        let mut current_idx = self.current_index.write().await;
        let start_idx = *current_idx;

        loop {
            let node_lock = &node_list[*current_idx];
            let node = node_lock.read().await;

            if node.is_available() {
                let node_id = node.info.id;
                *current_idx = (*current_idx + 1) % node_list.len();
                return Ok(node_id);
            }

            *current_idx = (*current_idx + 1) % node_list.len();
            if *current_idx == start_idx {
                return Err(ClusterError::NoAvailableNodes);
            }
        }
    }

    async fn least_loaded_dispatch(&self) -> Result<Uuid, ClusterError> {
        let nodes = self.nodes.read().await;
        if nodes.is_empty() {
            return Err(ClusterError::NoAvailableNodes);
        }

        let mut best_node: Option<(Uuid, f32)> = None;

        for node_lock in nodes.values() {
            let node = node_lock.read().await;
            if node.is_available() {
                let load = node.info.capacity.load_factor();
                if best_node.is_none() || load < best_node.unwrap().1 {
                    best_node = Some((node.info.id, load));
                }
            }
        }

        best_node
            .map(|(id, _)| id)
            .ok_or(ClusterError::NoAvailableNodes)
    }

    async fn consistent_hash_dispatch(&self, task_id: Uuid) -> Result<Uuid, ClusterError> {
        let nodes = self.nodes.read().await;
        if nodes.is_empty() {
            return Err(ClusterError::NoAvailableNodes);
        }

        let node_list: Vec<_> = nodes.values().cloned().collect();
        drop(nodes);

        // Simple hash-based selection
        let hash = task_id.as_u128();
        let idx = (hash % node_list.len() as u128) as usize;

        let node = node_list[idx].read().await;
        if node.is_available() {
            Ok(node.info.id)
        } else {
            // Fallback to round-robin if hashed node unavailable
            drop(node);
            self.round_robin_dispatch().await
        }
    }

    pub async fn node_count(&self) -> usize {
        let nodes = self.nodes.read().await;
        nodes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::node::{NodeInfo, NodeStatus};

    fn create_test_node(address: &str, port: u16, max_tasks: usize) -> ClusterNode {
        ClusterNode::new(NodeInfo::new(address.to_string(), port, max_tasks))
    }

    #[tokio::test]
    async fn test_round_robin_dispatch() {
        let dispatcher = TaskDispatcher::new(DispatchStrategy::RoundRobin);

        let node1 = create_test_node("127.0.0.1", 8081, 10);
        let node2 = create_test_node("127.0.0.1", 8082, 10);
        let node3 = create_test_node("127.0.0.1", 8083, 10);

        let id1 = node1.info.id;
        let id2 = node2.info.id;
        let id3 = node3.info.id;

        dispatcher.add_node(node1).await;
        dispatcher.add_node(node2).await;
        dispatcher.add_node(node3).await;

        let task1 = Uuid::new_v4();
        let task2 = Uuid::new_v4();
        let task3 = Uuid::new_v4();
        let task4 = Uuid::new_v4();

        let result1 = dispatcher.dispatch_task(task1).await.unwrap();
        let result2 = dispatcher.dispatch_task(task2).await.unwrap();
        let result3 = dispatcher.dispatch_task(task3).await.unwrap();
        let result4 = dispatcher.dispatch_task(task4).await.unwrap();

        // Should cycle through nodes
        let ids = [id1, id2, id3];
        assert!(ids.contains(&result1));
        assert!(ids.contains(&result2));
        assert!(ids.contains(&result3));
        assert!(ids.contains(&result4));

        // Fourth should match first (full cycle)
        assert_eq!(result1, result4);
    }

    #[tokio::test]
    async fn test_least_loaded_dispatch() {
        let dispatcher = TaskDispatcher::new(DispatchStrategy::LeastLoaded);

        let mut node1 = create_test_node("127.0.0.1", 8081, 10);
        let mut node2 = create_test_node("127.0.0.1", 8082, 10);

        node1.info.capacity.current_tasks = 8; // 80% loaded
        node2.info.capacity.current_tasks = 3; // 30% loaded

        let id2 = node2.info.id;

        dispatcher.add_node(node1).await;
        dispatcher.add_node(node2).await;

        let task = Uuid::new_v4();
        let result = dispatcher.dispatch_task(task).await.unwrap();

        // Should select less loaded node
        assert_eq!(result, id2);
    }

    #[tokio::test]
    async fn test_consistent_hash_dispatch() {
        let dispatcher = TaskDispatcher::new(DispatchStrategy::ConsistentHash);

        let node1 = create_test_node("127.0.0.1", 8081, 10);
        let node2 = create_test_node("127.0.0.1", 8082, 10);

        dispatcher.add_node(node1).await;
        dispatcher.add_node(node2).await;

        let task = Uuid::new_v4();
        let result1 = dispatcher.dispatch_task(task).await.unwrap();
        let result2 = dispatcher.dispatch_task(task).await.unwrap();

        // Same task should go to same node
        assert_eq!(result1, result2);
    }

    #[tokio::test]
    async fn test_no_available_nodes() {
        let dispatcher = TaskDispatcher::new(DispatchStrategy::RoundRobin);

        let task = Uuid::new_v4();
        let result = dispatcher.dispatch_task(task).await;

        assert!(matches!(result, Err(ClusterError::NoAvailableNodes)));
    }

    #[tokio::test]
    async fn test_all_nodes_unavailable() {
        let dispatcher = TaskDispatcher::new(DispatchStrategy::RoundRobin);

        let mut node1 = create_test_node("127.0.0.1", 8081, 10);
        node1.info.status = NodeStatus::Offline;

        dispatcher.add_node(node1).await;

        let task = Uuid::new_v4();
        let result = dispatcher.dispatch_task(task).await;

        assert!(matches!(result, Err(ClusterError::NoAvailableNodes)));
    }
}
