use async_trait::async_trait;
use chrono::{Duration, Utc};
use cyberclaw_core::cluster::{ExecutionLease, LeaseState};
use cyberclaw_core::ids::{ExecutionId, LeaseId, NodeId};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Lease manager configuration
#[derive(Debug, Clone)]
pub struct LeaseConfig {
    /// Default lease TTL in seconds (default: 60s)
    pub default_ttl_secs: i64,
}

impl Default for LeaseConfig {
    fn default() -> Self {
        Self {
            default_ttl_secs: 60,
        }
    }
}

impl LeaseConfig {
    /// Minimum lease TTL (prevents overly aggressive lease expiration)
    const MIN_TTL_SECS: i64 = 10;
    /// Maximum lease TTL (prevents leases being held too long)
    const MAX_TTL_SECS: i64 = 3600;

    /// Validate configuration values
    ///
    /// # Security
    /// Validates TTL to prevent:
    /// - DoS via overly short leases causing excessive churn
    /// - Resource hoarding via overly long leases
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.default_ttl_secs < Self::MIN_TTL_SECS {
            anyhow::bail!(
                "default_ttl_secs too low: {} (min {})",
                self.default_ttl_secs,
                Self::MIN_TTL_SECS
            );
        }
        if self.default_ttl_secs > Self::MAX_TTL_SECS {
            anyhow::bail!(
                "default_ttl_secs too high: {} (max {})",
                self.default_ttl_secs,
                Self::MAX_TTL_SECS
            );
        }
        Ok(())
    }
}

/// Lease assignment result for expired lease reassignment
#[derive(Debug, Clone)]
pub struct LeaseReassignment {
    pub execution_id: ExecutionId,
    pub old_lease_id: LeaseId,
    pub old_node_id: NodeId,
    pub new_lease_id: LeaseId,
    pub new_node_id: NodeId,
    pub handoff_count: u32,
}

/// Lease manager trait (Milestone C)
#[async_trait]
pub trait LeaseManager: Send + Sync {
    /// Acquire a lease for an execution on a specific node
    ///
    /// Constraint: Only one active lease per execution at any time
    async fn acquire(
        &self,
        execution_id: ExecutionId,
        node_id: NodeId,
        ttl_secs: Option<i64>,
    ) -> anyhow::Result<LeaseId>;

    /// Renew an existing lease
    async fn renew(&self, lease_id: &LeaseId) -> anyhow::Result<()>;

    /// Release a lease explicitly
    async fn release(&self, lease_id: &LeaseId) -> anyhow::Result<()>;

    /// Find and mark expired leases, return list for reassignment
    async fn expire_and_mark(&self) -> anyhow::Result<Vec<ExecutionId>>;

    /// Reassign an execution to a new node after lease expiration
    async fn reassign(
        &self,
        execution_id: &ExecutionId,
        new_node_id: NodeId,
        ttl_secs: Option<i64>,
    ) -> anyhow::Result<LeaseReassignment>;

    /// Get lease by ID
    async fn get_lease(&self, lease_id: &LeaseId) -> anyhow::Result<Option<ExecutionLease>>;

    /// Get active lease for an execution
    async fn get_active_lease(
        &self,
        execution_id: &ExecutionId,
    ) -> anyhow::Result<Option<ExecutionLease>>;

    /// List all leases (for debugging/monitoring)
    async fn list_all_leases(&self) -> anyhow::Result<Vec<ExecutionLease>>;
}

/// In-memory implementation of LeaseManager
///
/// # Concurrency Safety — Lock Ordering Protocol
///
/// This struct contains two independent `RwLock`s.  To prevent deadlocks,
/// **all code paths that need both locks MUST acquire them in the following order**:
///
/// 1. `self.leases`          (lock A)
/// 2. `self.execution_to_lease` (lock B)
///
/// Never acquire B before A.  Violating this ordering can cause a deadlock
/// when two concurrent threads each hold one lock and wait for the other.
///
/// Methods that need only one lock must not attempt to acquire the other
/// while holding it, unless following the ordering above.
#[derive(Clone)]
pub struct InMemoryLeaseManager {
    /// Lock A — always acquired first when holding both locks
    leases: Arc<RwLock<BTreeMap<LeaseId, ExecutionLease>>>,
    /// Lock B — always acquired second when holding both locks
    execution_to_lease: Arc<RwLock<BTreeMap<ExecutionId, LeaseId>>>,
    config: LeaseConfig,
}

impl InMemoryLeaseManager {
    pub fn new(config: LeaseConfig) -> Self {
        Self {
            leases: Arc::new(RwLock::new(BTreeMap::new())),
            execution_to_lease: Arc::new(RwLock::new(BTreeMap::new())),
            config,
        }
    }
}

impl Default for InMemoryLeaseManager {
    fn default() -> Self {
        Self::new(LeaseConfig::default())
    }
}

#[async_trait]
impl LeaseManager for InMemoryLeaseManager {
    /// Acquire a lease atomically
    ///
    /// # Concurrency Safety
    /// This method acquires both write locks (leases and execution_to_lease) before
    /// checking for existing leases and creating new ones. This ensures atomicity and
    /// prevents race conditions where multiple concurrent acquire calls for the same
    /// execution could create multiple active leases.
    ///
    /// IMPORTANT: Do NOT call `get_active_lease()` externally and then `acquire()`,
    /// as that creates a TOCTOU vulnerability. Always rely on `acquire()`'s internal
    /// atomic check-and-create operation.
    async fn acquire(
        &self,
        execution_id: ExecutionId,
        node_id: NodeId,
        ttl_secs: Option<i64>,
    ) -> anyhow::Result<LeaseId> {
        // Acquire both write locks atomically to prevent race conditions
        // Using blocking locks instead of try_write() to ensure 100% success under concurrent load
        let mut leases = self.leases.write().await;
        let mut execution_to_lease = self.execution_to_lease.write().await;

        // Atomically check if execution already has an active lease
        // (this happens while holding both write locks, so it's race-free)
        if let Some(existing_lease_id) = execution_to_lease.get(&execution_id) {
            if let Some(existing_lease) = leases.get(existing_lease_id) {
                if existing_lease.is_active() {
                    anyhow::bail!(
                        "execution {} already has active lease {}",
                        execution_id,
                        existing_lease_id
                    );
                }
            }
        }

        let now = Utc::now();
        let ttl = ttl_secs.unwrap_or(self.config.default_ttl_secs);
        let expires_at = now + Duration::seconds(ttl);

        let lease_id = LeaseId::new();
        let lease = ExecutionLease {
            id: lease_id.clone(),
            execution_id: execution_id.clone(),
            owner_node_id: node_id,
            state: LeaseState::Active,
            acquired_at: now,
            expires_at,
            renewed_at: None,
            released_at: None,
            handoff_count: 0,
        };

        leases.insert(lease_id.clone(), lease);
        execution_to_lease.insert(execution_id, lease_id.clone());

        Ok(lease_id)
    }

    async fn renew(&self, lease_id: &LeaseId) -> anyhow::Result<()> {
        let mut leases = self.leases.write().await;

        let lease = leases
            .get_mut(lease_id)
            .ok_or_else(|| anyhow::anyhow!("lease {} not found", lease_id))?;

        if !lease.is_active() {
            anyhow::bail!("cannot renew lease {} in state {:?}", lease_id, lease.state);
        }

        let now = Utc::now();
        let ttl = Duration::seconds(self.config.default_ttl_secs);
        lease.expires_at = now + ttl;
        lease.renewed_at = Some(now);

        Ok(())
    }

    async fn release(&self, lease_id: &LeaseId) -> anyhow::Result<()> {
        let mut leases = self.leases.write().await;
        let mut execution_to_lease = self.execution_to_lease.write().await;

        let lease = leases
            .get_mut(lease_id)
            .ok_or_else(|| anyhow::anyhow!("lease {} not found", lease_id))?;

        let now = Utc::now();
        lease.state = LeaseState::Released;
        lease.released_at = Some(now);

        // Remove from execution_to_lease mapping
        execution_to_lease.remove(&lease.execution_id);

        Ok(())
    }

    async fn expire_and_mark(&self) -> anyhow::Result<Vec<ExecutionId>> {
        // Using blocking lock for consistency across all methods
        // This is a background task but blocking ensures reliability
        let mut leases = self.leases.write().await;

        let now = Utc::now();
        let mut expired_executions = Vec::new();

        for lease in leases.values_mut() {
            if lease.is_expired(now) {
                lease.state = LeaseState::Expired;
                expired_executions.push(lease.execution_id.clone());
            }
        }

        Ok(expired_executions)
    }

    async fn reassign(
        &self,
        execution_id: &ExecutionId,
        new_node_id: NodeId,
        ttl_secs: Option<i64>,
    ) -> anyhow::Result<LeaseReassignment> {
        // Using blocking locks to ensure reliable lease reassignment
        let mut leases = self.leases.write().await;
        let mut execution_to_lease = self.execution_to_lease.write().await;

        // Find old lease
        let old_lease_id = execution_to_lease
            .get(execution_id)
            .ok_or_else(|| anyhow::anyhow!("no lease found for execution {}", execution_id))?
            .clone();

        let old_lease = leases
            .get(&old_lease_id)
            .ok_or_else(|| anyhow::anyhow!("old lease {} not found", old_lease_id))?;

        if old_lease.state != LeaseState::Expired {
            anyhow::bail!(
                "cannot reassign lease {} in state {:?}, must be Expired first",
                old_lease_id,
                old_lease.state
            );
        }

        let old_node_id = old_lease.owner_node_id.clone();
        let handoff_count = old_lease.handoff_count + 1;

        // Create new lease
        let now = Utc::now();
        let ttl = ttl_secs.unwrap_or(self.config.default_ttl_secs);
        let expires_at = now + Duration::seconds(ttl);

        let new_lease_id = LeaseId::new();
        let new_lease = ExecutionLease {
            id: new_lease_id.clone(),
            execution_id: execution_id.clone(),
            owner_node_id: new_node_id.clone(),
            state: LeaseState::Active,
            acquired_at: now,
            expires_at,
            renewed_at: None,
            released_at: None,
            handoff_count,
        };

        leases.insert(new_lease_id.clone(), new_lease);
        execution_to_lease.insert(execution_id.clone(), new_lease_id.clone());

        Ok(LeaseReassignment {
            execution_id: execution_id.clone(),
            old_lease_id,
            old_node_id,
            new_lease_id,
            new_node_id,
            handoff_count,
        })
    }

    async fn get_lease(&self, lease_id: &LeaseId) -> anyhow::Result<Option<ExecutionLease>> {
        // Using blocking read lock to ensure reliable lease queries
        let leases = self.leases.read().await;
        Ok(leases.get(lease_id).cloned())
    }

    async fn get_active_lease(
        &self,
        execution_id: &ExecutionId,
    ) -> anyhow::Result<Option<ExecutionLease>> {
        // Using blocking read locks to ensure reliable active lease queries
        let leases = self.leases.read().await;
        let execution_to_lease = self.execution_to_lease.read().await;

        if let Some(lease_id) = execution_to_lease.get(execution_id) {
            Ok(leases.get(lease_id).cloned())
        } else {
            Ok(None)
        }
    }

    async fn list_all_leases(&self) -> anyhow::Result<Vec<ExecutionLease>> {
        // Using blocking read lock to ensure reliable lease listing for monitoring
        let leases = self.leases.read().await;
        Ok(leases.values().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_acquire_lease() {
        let manager = InMemoryLeaseManager::default();
        let execution_id = ExecutionId::new();
        let node_id = NodeId::from_string("node-1".to_string()).unwrap();

        let lease_id = manager
            .acquire(execution_id.clone(), node_id, None)
            .await
            .unwrap();

        // Verify lease created
        let lease = manager.get_lease(&lease_id).await.unwrap().unwrap();
        assert_eq!(lease.execution_id, execution_id);
        assert_eq!(lease.state, LeaseState::Active);
    }

    #[tokio::test]
    async fn test_acquire_prevents_duplicate() {
        let manager = InMemoryLeaseManager::default();
        let execution_id = ExecutionId::new();
        let node_id = NodeId::from_string("node-1".to_string()).unwrap();

        manager
            .acquire(execution_id.clone(), node_id.clone(), None)
            .await
            .unwrap();

        // Second acquire should fail
        let result = manager.acquire(execution_id, node_id, None).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("already has active lease"));
    }

    #[tokio::test]
    async fn test_renew_lease() {
        let manager = InMemoryLeaseManager::default();
        let execution_id = ExecutionId::new();
        let node_id = NodeId::from_string("node-1".to_string()).unwrap();

        let lease_id = manager.acquire(execution_id, node_id, None).await.unwrap();

        // Renew lease
        let result = manager.renew(&lease_id).await;
        assert!(result.is_ok());

        // Verify renewed_at is set
        let lease = manager.get_lease(&lease_id).await.unwrap().unwrap();
        assert!(lease.renewed_at.is_some());
    }

    #[tokio::test]
    async fn test_release_lease() {
        let manager = InMemoryLeaseManager::default();
        let execution_id = ExecutionId::new();
        let node_id = NodeId::from_string("node-1".to_string()).unwrap();

        let lease_id = manager
            .acquire(execution_id.clone(), node_id, None)
            .await
            .unwrap();

        // Release lease
        manager.release(&lease_id).await.unwrap();

        // Verify state
        let lease = manager.get_lease(&lease_id).await.unwrap().unwrap();
        assert_eq!(lease.state, LeaseState::Released);

        // Verify can acquire new lease now
        let node_id2 = NodeId::from_string("node-2".to_string()).unwrap();
        let result = manager.acquire(execution_id, node_id2, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_expire_and_reassign() {
        let config = LeaseConfig {
            default_ttl_secs: 1, // 1 second for testing
        };
        let manager = InMemoryLeaseManager::new(config);
        let execution_id = ExecutionId::new();
        let node_id1 = NodeId::from_string("node-1".to_string()).unwrap();

        manager
            .acquire(execution_id.clone(), node_id1, None)
            .await
            .unwrap();

        // Wait for lease to expire
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        // Mark expired leases
        let expired = manager.expire_and_mark().await.unwrap();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0], execution_id);

        // Reassign to new node
        let node_id2 = NodeId::from_string("node-2".to_string()).unwrap();
        let reassignment = manager
            .reassign(&execution_id, node_id2.clone(), None)
            .await
            .unwrap();

        assert_eq!(reassignment.execution_id, execution_id);
        assert_eq!(reassignment.new_node_id, node_id2);
        assert_eq!(reassignment.handoff_count, 1);

        // Verify new lease is active
        let new_lease = manager
            .get_lease(&reassignment.new_lease_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(new_lease.state, LeaseState::Active);
        assert_eq!(new_lease.handoff_count, 1);
    }

    // Configuration Validation Tests

    #[test]
    fn test_config_default_is_valid() {
        let config = LeaseConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_ttl_too_low() {
        let config = LeaseConfig {
            default_ttl_secs: 9, // Below MIN (10)
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("default_ttl_secs too low"));
    }

    #[test]
    fn test_config_ttl_too_high() {
        let config = LeaseConfig {
            default_ttl_secs: 3601, // Above MAX (3600)
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("default_ttl_secs too high"));
    }

    #[test]
    fn test_config_valid_edge_cases() {
        // Min value
        let config = LeaseConfig {
            default_ttl_secs: 10,
        };
        assert!(config.validate().is_ok());

        // Max value
        let config = LeaseConfig {
            default_ttl_secs: 3600,
        };
        assert!(config.validate().is_ok());
    }
}
