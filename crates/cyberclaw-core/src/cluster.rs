use crate::ids::{ExecutionId, LeaseId, NodeId};
use crate::review::ReviewTarget;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Membership state for cluster nodes (Milestone C)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipState {
    /// Node is in the process of joining the cluster
    Joining,
    /// Node is active and can accept work
    Active,
    /// Node is draining (no new work, finishing existing work)
    Draining,
    /// Node is suspected to be down (missed heartbeats)
    Suspect,
    /// Node has left the cluster
    Left,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRole {
    ControlPlane,
    Worker,
    Hybrid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeHealth {
    Healthy,
    Degraded,
    Unreachable,
    Draining,
}

/// Lease state for execution ownership (Milestone C)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseState {
    /// Lease is active and valid
    Active,
    /// Lease has expired and can be reassigned
    Expired,
    /// Lease was explicitly released
    Released,
    /// Lease was revoked (e.g., node failure)
    Revoked,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NodeCapacity {
    pub max_executions: Option<u32>,
    pub max_cpu_millis: Option<u64>,
    pub max_memory_mb: Option<u64>,
}

/// Cluster membership record (Milestone C)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterMembership {
    pub node_id: NodeId,
    pub membership_state: MembershipState,
    pub joined_at: DateTime<Utc>,
    pub last_heartbeat_at: DateTime<Utc>,
    pub draining_since: Option<DateTime<Utc>>,
    pub left_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRecord {
    pub id: NodeId,
    pub role: NodeRole,
    pub labels: Vec<String>,
    pub region: Option<String>,
    pub zone: Option<String>,
    pub health: NodeHealth,
    pub membership_state: MembershipState,
    pub capacity: NodeCapacity,
    pub current_executions: u32,
    pub last_heartbeat_at: DateTime<Utc>,
}

/// Execution lease with full state tracking (Milestone C)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionLease {
    pub id: LeaseId,
    pub execution_id: ExecutionId,
    pub owner_node_id: NodeId,
    pub state: LeaseState,
    pub acquired_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub renewed_at: Option<DateTime<Utc>>,
    pub released_at: Option<DateTime<Utc>>,
    pub handoff_count: u32,
}

impl ExecutionLease {
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.state == LeaseState::Active && now > self.expires_at
    }

    pub fn is_active(&self) -> bool {
        self.state == LeaseState::Active
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CapabilityPlacement {
    pub allowed_node_labels: Vec<String>,
    pub required_runtime: Vec<String>,
    pub requires_local_secret: bool,
    pub network_zone: Option<String>,
}

/// Placement decision with reason (Milestone C)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacementDecision {
    pub execution_id: ExecutionId,
    pub scheduled_node_id: NodeId,
    pub reason: String,
    pub decided_at: DateTime<Utc>,
}

/// Cluster events for EventBus (Milestone C)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum ClusterEvent {
    /// Execution assigned to a worker node
    ExecutionAssigned {
        execution_id: ExecutionId,
        node_id: NodeId,
        lease_id: LeaseId,
        timestamp: DateTime<Utc>,
    },
    /// Execution lease expired
    ExecutionLeaseExpired {
        execution_id: ExecutionId,
        lease_id: LeaseId,
        expired_node_id: NodeId,
        timestamp: DateTime<Utc>,
    },
    /// Execution reassigned to different node
    ExecutionReassigned {
        execution_id: ExecutionId,
        old_node_id: NodeId,
        new_node_id: NodeId,
        new_lease_id: LeaseId,
        handoff_count: u32,
        timestamp: DateTime<Utc>,
    },
    /// Worker heartbeat missed
    WorkerHeartbeatMissed {
        node_id: NodeId,
        last_heartbeat_at: DateTime<Utc>,
        timestamp: DateTime<Utc>,
    },
    /// Node membership state changed
    NodeMembershipChanged {
        node_id: NodeId,
        old_state: MembershipState,
        new_state: MembershipState,
        timestamp: DateTime<Utc>,
    },
    /// Review created
    ReviewCreated {
        review_id: crate::ids::ReviewId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        execution_id: Option<ExecutionId>,
        /// S22: discriminated target; defaults to Execution for legacy events.
        #[serde(default)]
        target: ReviewTarget,
        timestamp: DateTime<Utc>,
    },
    /// Review approved
    ReviewApproved {
        review_id: crate::ids::ReviewId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        execution_id: Option<ExecutionId>,
        approved_by: crate::ids::ActorId,
        /// S22: discriminated target; defaults to Execution for legacy events.
        #[serde(default)]
        target: ReviewTarget,
        timestamp: DateTime<Utc>,
    },
    /// Review rejected
    ReviewRejected {
        review_id: crate::ids::ReviewId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        execution_id: Option<ExecutionId>,
        rejected_by: crate::ids::ActorId,
        reason: String,
        /// S22: discriminated target; defaults to Execution for legacy events.
        #[serde(default)]
        target: ReviewTarget,
        timestamp: DateTime<Utc>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{ExecutionId, ReviewId};

    #[test]
    fn review_created_event_with_target_serde_roundtrip() {
        let exec_id = ExecutionId::from_string("exec_1".to_string()).unwrap();
        let event = ClusterEvent::ReviewCreated {
            review_id: ReviewId::from_string("rev_1".to_string()).unwrap(),
            execution_id: Some(exec_id.clone()),
            target: ReviewTarget::Execution {
                execution_id: exec_id.clone(),
            },
            timestamp: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: ClusterEvent = serde_json::from_str(&json).unwrap();
        if let ClusterEvent::ReviewCreated { target, .. } = back {
            assert!(target.execution_id().is_some());
        } else {
            panic!("expected ReviewCreated");
        }
    }

    #[test]
    fn review_created_event_legacy_no_target_defaults_to_execution() {
        // JSON without target field must deserialize via default_target_execution.
        let json = r#"{"event_type":"review_created","review_id":"rev_legacy","execution_id":"exec_old","timestamp":"2026-01-01T00:00:00Z"}"#;
        let event: ClusterEvent = serde_json::from_str(json).unwrap();
        if let ClusterEvent::ReviewCreated { target, .. } = event {
            assert!(matches!(target, ReviewTarget::Execution { .. }));
        } else {
            panic!("expected ReviewCreated");
        }
    }
}
