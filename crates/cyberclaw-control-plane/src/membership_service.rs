use chrono::{DateTime, Duration, Utc};
use cyberclaw_core::cluster::{ClusterMembership, MembershipState, NodeRecord};
use cyberclaw_core::ids::NodeId;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::RwLock;

/// Configuration for membership service
#[derive(Debug, Clone)]
pub struct MembershipConfig {
    /// Heartbeat timeout in seconds (default: 30s)
    pub heartbeat_timeout_secs: i64,
    /// Suspect timeout in seconds (default: 60s)
    pub suspect_timeout_secs: i64,
}

impl Default for MembershipConfig {
    fn default() -> Self {
        Self {
            heartbeat_timeout_secs: 30,
            suspect_timeout_secs: 60,
        }
    }
}

impl MembershipConfig {
    /// Minimum heartbeat timeout (prevents overly aggressive checking)
    const MIN_HEARTBEAT_TIMEOUT_SECS: i64 = 5;
    /// Maximum heartbeat timeout (prevents nodes staying in limbo too long)
    const MAX_HEARTBEAT_TIMEOUT_SECS: i64 = 300;
    /// Minimum suspect timeout
    const MIN_SUSPECT_TIMEOUT_SECS: i64 = 10;
    /// Maximum suspect timeout
    const MAX_SUSPECT_TIMEOUT_SECS: i64 = 600;

    /// Validate configuration values
    ///
    /// # Security
    /// Validates timeout values to prevent:
    /// - DoS via overly aggressive heartbeat checking
    /// - Cluster instability via too-short timeouts
    /// - Logical inconsistencies (suspect < heartbeat)
    pub fn validate(&self) -> anyhow::Result<()> {
        // Validate heartbeat timeout range
        if self.heartbeat_timeout_secs < Self::MIN_HEARTBEAT_TIMEOUT_SECS {
            anyhow::bail!(
                "heartbeat_timeout_secs too low: {} (min {})",
                self.heartbeat_timeout_secs,
                Self::MIN_HEARTBEAT_TIMEOUT_SECS
            );
        }
        if self.heartbeat_timeout_secs > Self::MAX_HEARTBEAT_TIMEOUT_SECS {
            anyhow::bail!(
                "heartbeat_timeout_secs too high: {} (max {})",
                self.heartbeat_timeout_secs,
                Self::MAX_HEARTBEAT_TIMEOUT_SECS
            );
        }

        // Validate suspect timeout range
        if self.suspect_timeout_secs < Self::MIN_SUSPECT_TIMEOUT_SECS {
            anyhow::bail!(
                "suspect_timeout_secs too low: {} (min {})",
                self.suspect_timeout_secs,
                Self::MIN_SUSPECT_TIMEOUT_SECS
            );
        }
        if self.suspect_timeout_secs > Self::MAX_SUSPECT_TIMEOUT_SECS {
            anyhow::bail!(
                "suspect_timeout_secs too high: {} (max {})",
                self.suspect_timeout_secs,
                Self::MAX_SUSPECT_TIMEOUT_SECS
            );
        }

        // Validate logical relationship: suspect_timeout must be > heartbeat_timeout
        if self.suspect_timeout_secs <= self.heartbeat_timeout_secs {
            anyhow::bail!(
                "suspect_timeout_secs ({}) must be greater than heartbeat_timeout_secs ({})",
                self.suspect_timeout_secs,
                self.heartbeat_timeout_secs
            );
        }

        Ok(())
    }
}

/// Membership service trait for cluster node management (Milestone C)
pub trait MembershipService: Send + Sync {
    /// Add a node to the cluster
    fn join(&self, node: NodeRecord) -> anyhow::Result<()>;

    /// Record heartbeat from a node
    fn heartbeat(&self, node_id: &NodeId) -> anyhow::Result<()>;

    /// Mark a node as draining (no new work, finishing existing work)
    fn mark_draining(&self, node_id: &NodeId) -> anyhow::Result<()>;

    /// Evict nodes that have timed out
    fn evict_timeout_nodes(&self) -> anyhow::Result<Vec<NodeId>>;

    /// List all active nodes (Active membership state, not Suspect/Draining/Left)
    fn list_active_nodes(&self) -> anyhow::Result<Vec<NodeRecord>>;

    /// Get membership record for a specific node
    fn get_membership(&self, node_id: &NodeId) -> anyhow::Result<Option<ClusterMembership>>;

    /// List all memberships (for debugging/monitoring)
    fn list_all_memberships(&self) -> anyhow::Result<Vec<ClusterMembership>>;
}

/// In-memory implementation of MembershipService
#[derive(Clone)]
pub struct InMemoryMembershipService {
    entries: Arc<RwLock<BTreeMap<NodeId, MembershipEntry>>>,
    config: MembershipConfig,
}

#[derive(Clone)]
struct MembershipEntry {
    node: NodeRecord,
    joined_at: DateTime<Utc>,
    draining_since: Option<DateTime<Utc>>,
    left_at: Option<DateTime<Utc>>,
}

impl InMemoryMembershipService {
    pub fn new(config: MembershipConfig) -> Self {
        Self {
            entries: Arc::new(RwLock::new(BTreeMap::new())),
            config,
        }
    }

    /// Check if a node has timed out based on heartbeat
    fn is_heartbeat_timeout(&self, last_heartbeat: DateTime<Utc>) -> bool {
        let now = Utc::now();
        let timeout = Duration::seconds(self.config.heartbeat_timeout_secs);
        now - last_heartbeat > timeout
    }

    /// Check if a suspect node has exceeded suspect timeout
    fn is_suspect_timeout(&self, last_heartbeat: DateTime<Utc>) -> bool {
        let now = Utc::now();
        let timeout = Duration::seconds(self.config.suspect_timeout_secs);
        now - last_heartbeat > timeout
    }

    fn to_membership(node_id: &NodeId, entry: &MembershipEntry) -> ClusterMembership {
        ClusterMembership {
            node_id: node_id.clone(),
            membership_state: entry.node.membership_state,
            joined_at: entry.joined_at,
            last_heartbeat_at: entry.node.last_heartbeat_at,
            draining_since: entry.draining_since,
            left_at: entry.left_at,
        }
    }
}

impl Default for InMemoryMembershipService {
    fn default() -> Self {
        Self::new(MembershipConfig::default())
    }
}

impl MembershipService for InMemoryMembershipService {
    fn join(&self, mut node: NodeRecord) -> anyhow::Result<()> {
        let mut entries = self
            .entries
            .write()
            .map_err(|_| anyhow::anyhow!("failed to acquire write lock on membership entries"))?;

        if entries.contains_key(&node.id) {
            anyhow::bail!("node {} already exists in cluster", node.id);
        }

        let now = Utc::now();
        node.membership_state = MembershipState::Joining;
        node.last_heartbeat_at = now;

        let entry = MembershipEntry {
            node,
            joined_at: now,
            draining_since: None,
            left_at: None,
        };

        entries.insert(entry.node.id.clone(), entry);

        Ok(())
    }

    fn heartbeat(&self, node_id: &NodeId) -> anyhow::Result<()> {
        let mut entries = self
            .entries
            .write()
            .map_err(|_| anyhow::anyhow!("failed to acquire write lock on membership entries"))?;

        let entry = entries
            .get_mut(node_id)
            .ok_or_else(|| anyhow::anyhow!("node {} not found", node_id))?;

        let now = Utc::now();
        entry.node.last_heartbeat_at = now;

        // Promote Joining -> Active on first heartbeat
        if entry.node.membership_state == MembershipState::Joining {
            entry.node.membership_state = MembershipState::Active;
        }

        // Recover from Suspect -> Active if heartbeat received
        if entry.node.membership_state == MembershipState::Suspect {
            entry.node.membership_state = MembershipState::Active;
            entry.left_at = None;
        }

        Ok(())
    }

    fn mark_draining(&self, node_id: &NodeId) -> anyhow::Result<()> {
        let mut entries = self
            .entries
            .write()
            .map_err(|_| anyhow::anyhow!("failed to acquire write lock on membership entries"))?;

        let entry = entries
            .get_mut(node_id)
            .ok_or_else(|| anyhow::anyhow!("node {} not found", node_id))?;

        if entry.node.membership_state == MembershipState::Left {
            anyhow::bail!("cannot mark left node as draining");
        }

        let now = Utc::now();
        entry.node.membership_state = MembershipState::Draining;
        entry.draining_since = Some(now);

        Ok(())
    }

    fn evict_timeout_nodes(&self) -> anyhow::Result<Vec<NodeId>> {
        let mut entries = self
            .entries
            .write()
            .map_err(|_| anyhow::anyhow!("failed to acquire write lock on membership entries"))?;

        let mut evicted = Vec::new();

        for (node_id, entry) in entries.iter_mut() {
            match entry.node.membership_state {
                MembershipState::Active | MembershipState::Joining => {
                    // Check if heartbeat timeout -> mark as Suspect
                    if self.is_heartbeat_timeout(entry.node.last_heartbeat_at) {
                        entry.node.membership_state = MembershipState::Suspect;
                    }
                }
                MembershipState::Suspect => {
                    // Check if suspect timeout -> evict (Left)
                    if self.is_suspect_timeout(entry.node.last_heartbeat_at) {
                        let now = Utc::now();
                        entry.node.membership_state = MembershipState::Left;
                        entry.left_at = Some(now);
                        evicted.push(node_id.clone());
                    }
                }
                MembershipState::Draining | MembershipState::Left => {
                    // No automatic eviction for these states
                }
            }
        }

        Ok(evicted)
    }

    fn list_active_nodes(&self) -> anyhow::Result<Vec<NodeRecord>> {
        let entries = self
            .entries
            .read()
            .map_err(|_| anyhow::anyhow!("failed to acquire read lock on membership entries"))?;

        let active_nodes: Vec<NodeRecord> = entries
            .values()
            .filter(|entry| entry.node.membership_state == MembershipState::Active)
            .map(|entry| entry.node.clone())
            .collect();

        Ok(active_nodes)
    }

    fn get_membership(&self, node_id: &NodeId) -> anyhow::Result<Option<ClusterMembership>> {
        let entries = self
            .entries
            .read()
            .map_err(|_| anyhow::anyhow!("failed to acquire read lock on membership entries"))?;

        Ok(entries
            .get(node_id)
            .map(|entry| Self::to_membership(node_id, entry)))
    }

    fn list_all_memberships(&self) -> anyhow::Result<Vec<ClusterMembership>> {
        let entries = self
            .entries
            .read()
            .map_err(|_| anyhow::anyhow!("failed to acquire read lock on membership entries"))?;

        Ok(entries
            .iter()
            .map(|(node_id, entry)| Self::to_membership(node_id, entry))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::cluster::{NodeCapacity, NodeHealth, NodeRole};

    fn create_test_node(id: &str) -> NodeRecord {
        NodeRecord {
            id: NodeId::from_string(id.to_string()).unwrap(),
            role: NodeRole::Worker,
            labels: vec!["test".to_string()],
            region: Some("us-east-1".to_string()),
            zone: Some("us-east-1a".to_string()),
            health: NodeHealth::Healthy,
            membership_state: MembershipState::Joining,
            capacity: NodeCapacity {
                max_executions: Some(10),
                max_cpu_millis: None,
                max_memory_mb: None,
            },
            current_executions: 0,
            last_heartbeat_at: Utc::now(),
        }
    }

    #[test]
    fn test_join_node() {
        let service = InMemoryMembershipService::default();
        let node = create_test_node("node-1");

        let result = service.join(node);
        assert!(result.is_ok());

        // Verify membership created
        let membership = service
            .get_membership(&NodeId::from_string("node-1".to_string()).unwrap())
            .unwrap();
        assert!(membership.is_some());
        let membership = membership.unwrap();
        assert_eq!(membership.membership_state, MembershipState::Joining);
    }

    #[test]
    fn test_heartbeat_promotes_to_active() {
        let service = InMemoryMembershipService::default();
        let node = create_test_node("node-1");
        let node_id = node.id.clone();

        service.join(node).unwrap();

        // Send heartbeat
        service.heartbeat(&node_id).unwrap();

        // Verify promoted to Active
        let membership = service.get_membership(&node_id).unwrap().unwrap();
        assert_eq!(membership.membership_state, MembershipState::Active);
    }

    #[test]
    fn test_mark_draining() {
        let service = InMemoryMembershipService::default();
        let node = create_test_node("node-1");
        let node_id = node.id.clone();

        service.join(node).unwrap();
        service.heartbeat(&node_id).unwrap(); // Promote to Active

        // Mark as draining
        service.mark_draining(&node_id).unwrap();

        // Verify state
        let membership = service.get_membership(&node_id).unwrap().unwrap();
        assert_eq!(membership.membership_state, MembershipState::Draining);
        assert!(membership.draining_since.is_some());
    }

    #[test]
    fn test_list_active_nodes() {
        let service = InMemoryMembershipService::default();

        let node1 = create_test_node("node-1");
        let node2 = create_test_node("node-2");
        let node3 = create_test_node("node-3");

        service.join(node1.clone()).unwrap();
        service.join(node2.clone()).unwrap();
        service.join(node3.clone()).unwrap();

        // Promote node1 and node2 to Active
        service.heartbeat(&node1.id).unwrap();
        service.heartbeat(&node2.id).unwrap();

        // Mark node2 as draining
        service.mark_draining(&node2.id).unwrap();

        // List active nodes (should only include node1)
        let active = service.list_active_nodes().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, node1.id);
    }

    #[test]
    fn test_active_node_record_membership_state_is_consistent() {
        let service = InMemoryMembershipService::default();
        let node = create_test_node("node-state");
        let node_id = node.id.clone();

        service.join(node).unwrap();
        service.heartbeat(&node_id).unwrap();

        let active = service.list_active_nodes().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, node_id);
        assert_eq!(
            active[0].membership_state,
            MembershipState::Active,
            "list_active_nodes should return node records with Active membership_state"
        );
    }

    #[test]
    fn test_evict_timeout_nodes() {
        let config = MembershipConfig {
            heartbeat_timeout_secs: 1, // 1 second for testing
            suspect_timeout_secs: 2,   // 2 seconds for testing
        };
        let service = InMemoryMembershipService::new(config);
        let node = create_test_node("node-1");
        let node_id = node.id.clone();

        service.join(node).unwrap();
        service.heartbeat(&node_id).unwrap(); // Promote to Active

        // Wait for heartbeat timeout
        std::thread::sleep(std::time::Duration::from_secs(2));

        // First eviction should mark as Suspect
        let evicted = service.evict_timeout_nodes().unwrap();
        assert_eq!(evicted.len(), 0); // Not evicted yet, just marked Suspect

        let membership = service.get_membership(&node_id).unwrap().unwrap();
        assert_eq!(membership.membership_state, MembershipState::Suspect);

        // Wait for suspect timeout
        std::thread::sleep(std::time::Duration::from_secs(2));

        // Second eviction should mark as Left
        let evicted = service.evict_timeout_nodes().unwrap();
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0], node_id);

        let membership = service.get_membership(&node_id).unwrap().unwrap();
        assert_eq!(membership.membership_state, MembershipState::Left);
    }

    // Configuration Validation Tests

    #[test]
    fn test_config_default_is_valid() {
        let config = MembershipConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_heartbeat_too_low() {
        let config = MembershipConfig {
            heartbeat_timeout_secs: 4, // Below MIN (5)
            suspect_timeout_secs: 60,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("heartbeat_timeout_secs too low"));
    }

    #[test]
    fn test_config_heartbeat_too_high() {
        let config = MembershipConfig {
            heartbeat_timeout_secs: 301, // Above MAX (300)
            suspect_timeout_secs: 400,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("heartbeat_timeout_secs too high"));
    }

    #[test]
    fn test_config_suspect_too_low() {
        let config = MembershipConfig {
            heartbeat_timeout_secs: 30,
            suspect_timeout_secs: 9, // Below MIN (10)
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("suspect_timeout_secs too low"));
    }

    #[test]
    fn test_config_suspect_too_high() {
        let config = MembershipConfig {
            heartbeat_timeout_secs: 30,
            suspect_timeout_secs: 601, // Above MAX (600)
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("suspect_timeout_secs too high"));
    }

    #[test]
    fn test_config_suspect_not_greater_than_heartbeat() {
        let config = MembershipConfig {
            heartbeat_timeout_secs: 60,
            suspect_timeout_secs: 60, // Must be > heartbeat
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must be greater than"));
    }

    #[test]
    fn test_config_valid_edge_cases() {
        // Min values
        let config = MembershipConfig {
            heartbeat_timeout_secs: 5,
            suspect_timeout_secs: 10,
        };
        assert!(config.validate().is_ok());

        // Max values
        let config = MembershipConfig {
            heartbeat_timeout_secs: 300,
            suspect_timeout_secs: 600,
        };
        assert!(config.validate().is_ok());
    }
}
