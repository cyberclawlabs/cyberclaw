use async_trait::async_trait;
use reqwest::header::HeaderValue;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};
use uuid::Uuid;

use super::error::ClusterError;

/// HTTP header used to carry the shared cluster token.
/// Must match the server-side constant in `cluster_assignments.rs`.
const CLUSTER_TOKEN_HEADER: &str = "x-cyberclaw-cluster-token";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: Uuid,
    pub address: String,
    pub port: u16,
    pub status: NodeStatus,
    pub capacity: NodeCapacity,
    pub last_heartbeat: SystemTime,
}

impl NodeInfo {
    pub fn new(address: String, port: u16, max_concurrent_tasks: usize) -> Self {
        Self {
            id: Uuid::new_v4(),
            address,
            port,
            status: NodeStatus::Online,
            capacity: NodeCapacity::new(max_concurrent_tasks),
            last_heartbeat: SystemTime::now(),
        }
    }

    /// Returns the base HTTP URL for this node's internal cluster API.
    pub fn base_url(&self) -> String {
        format!("http://{}:{}", self.address, self.port)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeStatus {
    Online,
    Offline,
    Draining,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeCapacity {
    pub max_concurrent_tasks: usize,
    pub current_tasks: usize,
    pub cpu_usage: f32,
    pub memory_usage: f32,
}

impl NodeCapacity {
    pub fn new(max_concurrent_tasks: usize) -> Self {
        Self {
            max_concurrent_tasks,
            current_tasks: 0,
            cpu_usage: 0.0,
            memory_usage: 0.0,
        }
    }

    pub fn available_slots(&self) -> usize {
        self.max_concurrent_tasks.saturating_sub(self.current_tasks)
    }

    pub fn load_factor(&self) -> f32 {
        if self.max_concurrent_tasks == 0 {
            return 1.0;
        }
        (self.current_tasks as f32) / (self.max_concurrent_tasks as f32)
    }

    pub fn increment_tasks(&mut self) {
        self.current_tasks = self.current_tasks.saturating_add(1);
    }

    pub fn decrement_tasks(&mut self) {
        self.current_tasks = self.current_tasks.saturating_sub(1);
    }
}

pub struct ClusterNode {
    pub info: NodeInfo,
    client: Option<reqwest::Client>,
}

impl ClusterNode {
    pub fn new(info: NodeInfo) -> Self {
        Self { info, client: None }
    }

    /// Establish an HTTP connection to this node.
    ///
    /// Builds a `reqwest::Client` with the shared cluster token pre-configured
    /// as a default header (`x-cyberclaw-cluster-token`).  When
    /// `CYBERCLAW_CLUSTER_SHARED_TOKEN` is not set, the client is still created
    /// but no auth header is attached — the server will reject protected routes.
    pub async fn connect(&mut self) -> Result<(), ClusterError> {
        let mut headers = reqwest::header::HeaderMap::new();

        if let Ok(token) = std::env::var("CYBERCLAW_CLUSTER_SHARED_TOKEN") {
            let header_value = HeaderValue::from_str(&token).map_err(|e| {
                ClusterError::ConnectionError(format!("invalid cluster token header value: {e}"))
            })?;
            headers.insert(CLUSTER_TOKEN_HEADER, header_value);
        }

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(|e| ClusterError::ConnectionError(e.to_string()))?;

        self.client = Some(client);

        tracing::debug!(
            node_id = %self.info.id,
            address = %self.info.address,
            port = self.info.port,
            "cluster node HTTP client initialised",
        );

        Ok(())
    }

    /// Returns a reference to the HTTP client, if connected.
    pub fn http_client(&self) -> Option<&reqwest::Client> {
        self.client.as_ref()
    }

    pub fn is_available(&self) -> bool {
        self.info.status == NodeStatus::Online && self.info.capacity.available_slots() > 0
    }

    pub fn update_heartbeat(&mut self) {
        self.info.last_heartbeat = SystemTime::now();
    }

    pub fn mark_offline(&mut self) {
        self.info.status = NodeStatus::Offline;
    }

    pub fn mark_online(&mut self) {
        self.info.status = NodeStatus::Online;
    }
}

impl Clone for ClusterNode {
    fn clone(&self) -> Self {
        // The HTTP client is not cloned; call connect() on the new instance.
        Self {
            info: self.info.clone(),
            client: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Session Assignment Protocol
// ---------------------------------------------------------------------------

/// Messages exchanged between cluster nodes for session management and health.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClusterMessage {
    /// Coordinator assigns a session to a specific brain node.
    SessionAssign {
        session_id: String,
        brain_id: String,
    },
    /// Coordinator releases a session from its assigned brain node.
    SessionRelease { session_id: String },
    /// Periodic heartbeat from a node reporting its current load.
    Heartbeat {
        node_id: String,
        timestamp: u64,
        load: NodeLoad,
    },
    /// Acknowledgement of a received heartbeat.
    HeartbeatAck { node_id: String },
    /// Status update for a specific node.
    NodeStatus {
        node_id: String,
        status: NodeHealthStatus,
    },
}

/// Resource utilisation snapshot for a single node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeLoad {
    pub cpu_percent: f32,
    pub memory_percent: f32,
    pub active_sessions: u32,
    pub capacity: u32,
}

/// Health status derived from heartbeat monitoring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeHealthStatus {
    Healthy,
    Degraded,
    Dead,
    Unknown,
}

// ---------------------------------------------------------------------------
// Heartbeat Monitor
// ---------------------------------------------------------------------------

/// Internal record kept per monitored node.
struct HeartbeatRecord {
    last_seen: Instant,
    load: NodeLoad,
}

/// Tracks heartbeats from cluster nodes and determines their health.
pub struct HeartbeatMonitor {
    timeout: Duration,
    records: Mutex<HashMap<String, HeartbeatRecord>>,
}

impl HeartbeatMonitor {
    /// Create a new monitor with the given timeout duration.
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            records: Mutex::new(HashMap::new()),
        }
    }

    /// Create a monitor with the default 30-second timeout.
    pub fn default_timeout() -> Self {
        Self::new(Duration::from_secs(30))
    }

    /// Record a heartbeat from a node with its current load.
    pub fn record_heartbeat(&self, node_id: &str, load: NodeLoad) {
        let mut records = self.records.lock().expect("heartbeat lock poisoned");
        records.insert(
            node_id.to_string(),
            HeartbeatRecord {
                last_seen: Instant::now(),
                load,
            },
        );
    }

    /// Check the health status of a node based on its last heartbeat.
    pub fn check_health(&self, node_id: &str) -> NodeHealthStatus {
        let records = self.records.lock().expect("heartbeat lock poisoned");
        match records.get(node_id) {
            None => NodeHealthStatus::Unknown,
            Some(record) => {
                if record.last_seen.elapsed() > self.timeout {
                    NodeHealthStatus::Dead
                } else {
                    NodeHealthStatus::Healthy
                }
            }
        }
    }

    /// Return the most recently recorded load for a node, if any.
    pub fn get_load(&self, node_id: &str) -> Option<NodeLoad> {
        let records = self.records.lock().expect("heartbeat lock poisoned");
        records.get(node_id).map(|r| r.load.clone())
    }

    /// List all tracked node IDs.
    pub fn tracked_nodes(&self) -> Vec<String> {
        let records = self.records.lock().expect("heartbeat lock poisoned");
        records.keys().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// Session Assigner
// ---------------------------------------------------------------------------

/// Trait for session-to-brain assignment strategies.
#[async_trait]
pub trait SessionAssigner: Send + Sync {
    /// Assign a session to a brain node, returning the brain_id.
    async fn assign_session(&self, session_id: &str) -> Result<String, ClusterError>;
    /// Release a previously assigned session.
    async fn release_session(&self, session_id: &str) -> Result<(), ClusterError>;
    /// Return a snapshot of all current session -> brain_id assignments.
    fn active_assignments(&self) -> HashMap<String, String>;
}

/// Assigns sessions to the brain node with the fewest active sessions.
pub struct LeastLoadedAssigner {
    monitor: std::sync::Arc<HeartbeatMonitor>,
    assignments: Mutex<HashMap<String, String>>,
}

impl LeastLoadedAssigner {
    pub fn new(monitor: std::sync::Arc<HeartbeatMonitor>) -> Self {
        Self {
            monitor,
            assignments: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl SessionAssigner for LeastLoadedAssigner {
    async fn assign_session(&self, session_id: &str) -> Result<String, ClusterError> {
        let nodes = self.monitor.tracked_nodes();
        if nodes.is_empty() {
            return Err(ClusterError::NoAvailableNodes);
        }

        // Pick the node with the lowest active_sessions among healthy nodes.
        let mut best: Option<(String, u32)> = None;
        for node_id in &nodes {
            if self.monitor.check_health(node_id) == NodeHealthStatus::Dead {
                continue;
            }
            if let Some(load) = self.monitor.get_load(node_id) {
                match &best {
                    None => best = Some((node_id.clone(), load.active_sessions)),
                    Some((_, best_sessions)) => {
                        if load.active_sessions < *best_sessions {
                            best = Some((node_id.clone(), load.active_sessions));
                        }
                    }
                }
            }
        }

        let (brain_id, _) = best.ok_or(ClusterError::NoAvailableNodes)?;

        let mut assignments = self.assignments.lock().expect("assignments lock poisoned");
        assignments.insert(session_id.to_string(), brain_id.clone());

        Ok(brain_id)
    }

    async fn release_session(&self, session_id: &str) -> Result<(), ClusterError> {
        let mut assignments = self.assignments.lock().expect("assignments lock poisoned");
        assignments.remove(session_id);
        Ok(())
    }

    fn active_assignments(&self) -> HashMap<String, String> {
        let assignments = self.assignments.lock().expect("assignments lock poisoned");
        assignments.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_capacity_calculations() {
        let mut capacity = NodeCapacity::new(10);
        assert_eq!(capacity.available_slots(), 10);
        assert_eq!(capacity.load_factor(), 0.0);

        capacity.current_tasks = 5;
        assert_eq!(capacity.available_slots(), 5);
        assert_eq!(capacity.load_factor(), 0.5);

        capacity.current_tasks = 10;
        assert_eq!(capacity.available_slots(), 0);
        assert_eq!(capacity.load_factor(), 1.0);
    }

    #[test]
    fn test_node_availability() {
        let info = NodeInfo::new("127.0.0.1".to_string(), 8080, 10);
        let node = ClusterNode::new(info);
        assert!(node.is_available());

        let mut node_full = node.clone();
        node_full.info.capacity.current_tasks = 10;
        assert!(!node_full.is_available());

        let mut node_offline = node.clone();
        node_offline.info.status = NodeStatus::Offline;
        assert!(!node_offline.is_available());
    }

    #[test]
    fn test_capacity_increment_decrement() {
        let mut capacity = NodeCapacity::new(5);

        capacity.increment_tasks();
        assert_eq!(capacity.current_tasks, 1);

        capacity.increment_tasks();
        capacity.increment_tasks();
        assert_eq!(capacity.current_tasks, 3);

        capacity.decrement_tasks();
        assert_eq!(capacity.current_tasks, 2);

        capacity.decrement_tasks();
        capacity.decrement_tasks();
        assert_eq!(capacity.current_tasks, 0);

        // Test underflow protection
        capacity.decrement_tasks();
        assert_eq!(capacity.current_tasks, 0);
    }

    #[tokio::test]
    async fn test_connect_without_token() {
        // No CYBERCLAW_CLUSTER_SHARED_TOKEN in env → connect() still succeeds.
        std::env::remove_var("CYBERCLAW_CLUSTER_SHARED_TOKEN");
        let info = NodeInfo::new("127.0.0.1".to_string(), 8080, 4);
        let mut node = ClusterNode::new(info);
        node.connect()
            .await
            .expect("connect without token should succeed");
        assert!(node.http_client().is_some());
    }

    #[tokio::test]
    async fn test_connect_with_token() {
        std::env::set_var("CYBERCLAW_CLUSTER_SHARED_TOKEN", "test-secret-token");
        let info = NodeInfo::new("127.0.0.1".to_string(), 8080, 4);
        let mut node = ClusterNode::new(info);
        node.connect()
            .await
            .expect("connect with token should succeed");
        assert!(node.http_client().is_some());
        std::env::remove_var("CYBERCLAW_CLUSTER_SHARED_TOKEN");
    }

    #[test]
    fn test_base_url() {
        let info = NodeInfo::new("10.0.0.1".to_string(), 9000, 2);
        assert_eq!(info.base_url(), "http://10.0.0.1:9000");
    }

    #[test]
    fn test_clone_has_no_client() {
        let info = NodeInfo::new("127.0.0.1".to_string(), 8080, 4);
        let node = ClusterNode::new(info);
        let cloned = node.clone();
        assert!(cloned.http_client().is_none());
    }

    // -----------------------------------------------------------------------
    // Heartbeat & Session Protocol Tests
    // -----------------------------------------------------------------------

    fn sample_load(active: u32) -> NodeLoad {
        NodeLoad {
            cpu_percent: 25.0,
            memory_percent: 40.0,
            active_sessions: active,
            capacity: 100,
        }
    }

    #[test]
    fn test_heartbeat_record_and_health_check() {
        let monitor = HeartbeatMonitor::default_timeout();
        monitor.record_heartbeat("node-1", sample_load(5));
        assert_eq!(monitor.check_health("node-1"), NodeHealthStatus::Healthy);
    }

    #[test]
    fn test_heartbeat_unknown_node() {
        let monitor = HeartbeatMonitor::default_timeout();
        assert_eq!(
            monitor.check_health("no-such-node"),
            NodeHealthStatus::Unknown
        );
    }

    #[test]
    fn test_heartbeat_dead_after_timeout() {
        // Use a zero-duration timeout so the node is immediately dead.
        let monitor = HeartbeatMonitor::new(Duration::from_millis(0));
        monitor.record_heartbeat("node-1", sample_load(1));
        // Even a tiny elapsed time exceeds 0ms timeout.
        std::thread::sleep(Duration::from_millis(1));
        assert_eq!(monitor.check_health("node-1"), NodeHealthStatus::Dead);
    }

    #[test]
    fn test_heartbeat_load_tracking() {
        let monitor = HeartbeatMonitor::default_timeout();
        monitor.record_heartbeat("node-1", sample_load(7));
        let load = monitor.get_load("node-1").expect("load should exist");
        assert_eq!(load.active_sessions, 7);
        assert_eq!(load.capacity, 100);
    }

    #[test]
    fn test_heartbeat_tracked_nodes() {
        let monitor = HeartbeatMonitor::default_timeout();
        monitor.record_heartbeat("a", sample_load(1));
        monitor.record_heartbeat("b", sample_load(2));
        monitor.record_heartbeat("c", sample_load(3));
        let mut nodes = monitor.tracked_nodes();
        nodes.sort();
        assert_eq!(nodes, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_heartbeat_overwrite() {
        let monitor = HeartbeatMonitor::default_timeout();
        monitor.record_heartbeat("node-1", sample_load(3));
        monitor.record_heartbeat("node-1", sample_load(10));
        let load = monitor.get_load("node-1").unwrap();
        assert_eq!(load.active_sessions, 10);
    }

    #[tokio::test]
    async fn test_least_loaded_assign_session() {
        let monitor = std::sync::Arc::new(HeartbeatMonitor::default_timeout());
        monitor.record_heartbeat("brain-a", sample_load(10));
        monitor.record_heartbeat("brain-b", sample_load(2));
        monitor.record_heartbeat("brain-c", sample_load(5));

        let assigner = LeastLoadedAssigner::new(monitor);
        let brain = assigner.assign_session("session-1").await.unwrap();
        assert_eq!(brain, "brain-b");
    }

    #[tokio::test]
    async fn test_least_loaded_release_session() {
        let monitor = std::sync::Arc::new(HeartbeatMonitor::default_timeout());
        monitor.record_heartbeat("brain-a", sample_load(1));

        let assigner = LeastLoadedAssigner::new(monitor);
        assigner.assign_session("s1").await.unwrap();
        assert_eq!(assigner.active_assignments().len(), 1);

        assigner.release_session("s1").await.unwrap();
        assert!(assigner.active_assignments().is_empty());
    }

    #[tokio::test]
    async fn test_least_loaded_no_nodes_error() {
        let monitor = std::sync::Arc::new(HeartbeatMonitor::default_timeout());
        let assigner = LeastLoadedAssigner::new(monitor);
        let result = assigner.assign_session("s1").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_least_loaded_skips_dead_nodes() {
        // brain-a is dead (0ms timeout), brain-b is healthy (30s timeout).
        let monitor = std::sync::Arc::new(HeartbeatMonitor::new(Duration::from_millis(0)));
        monitor.record_heartbeat("brain-a", sample_load(1));
        // Sleep to ensure brain-a is dead.
        std::thread::sleep(Duration::from_millis(1));
        // Re-record brain-b with a fresh heartbeat — but the monitor timeout is 0ms,
        // so we need a different approach: use a longer-timeout monitor.
        drop(monitor);

        let monitor = std::sync::Arc::new(HeartbeatMonitor::default_timeout());
        // brain-a has fewer sessions but will be dead.
        monitor.record_heartbeat("brain-a", sample_load(1));
        monitor.record_heartbeat("brain-b", sample_load(5));

        // Create a custom assigner that treats brain-a as dead.
        // We simulate this by using a 0ms timeout monitor, recording brain-a first,
        // waiting, then recording brain-b.
        let short_monitor = std::sync::Arc::new(HeartbeatMonitor::new(Duration::from_millis(0)));
        short_monitor.record_heartbeat("brain-a", sample_load(1));
        std::thread::sleep(Duration::from_millis(2));
        // brain-b heartbeat is fresh (but will also expire immediately with 0ms timeout).
        // Instead, let's use a 50ms timeout and only let brain-a expire.
        let monitor = std::sync::Arc::new(HeartbeatMonitor::new(Duration::from_millis(50)));
        monitor.record_heartbeat("brain-a", sample_load(1));
        std::thread::sleep(Duration::from_millis(60));
        monitor.record_heartbeat("brain-b", sample_load(5));

        let assigner = LeastLoadedAssigner::new(monitor);
        let brain = assigner.assign_session("s1").await.unwrap();
        assert_eq!(brain, "brain-b");
    }

    #[tokio::test]
    async fn test_least_loaded_multiple_assignments() {
        let monitor = std::sync::Arc::new(HeartbeatMonitor::default_timeout());
        monitor.record_heartbeat("brain-a", sample_load(3));
        monitor.record_heartbeat("brain-b", sample_load(3));

        let assigner = LeastLoadedAssigner::new(monitor);
        assigner.assign_session("s1").await.unwrap();
        assigner.assign_session("s2").await.unwrap();
        assert_eq!(assigner.active_assignments().len(), 2);
    }

    #[test]
    fn test_cluster_message_serialization() {
        let msg = ClusterMessage::Heartbeat {
            node_id: "n1".to_string(),
            timestamp: 1234567890,
            load: sample_load(5),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: ClusterMessage = serde_json::from_str(&json).unwrap();
        match deserialized {
            ClusterMessage::Heartbeat {
                node_id,
                timestamp,
                load,
            } => {
                assert_eq!(node_id, "n1");
                assert_eq!(timestamp, 1234567890);
                assert_eq!(load.active_sessions, 5);
            }
            _ => panic!("expected Heartbeat variant"),
        }
    }
}
