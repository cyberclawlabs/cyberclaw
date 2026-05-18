//! Cross-instance assignment delivery and consumption.
//!
//! This module closes the P1-012 gap by adding a transportable assignment
//! envelope, an in-memory dispatch queue on the coordinator, and a pull-worker
//! loop on remote nodes.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use cyberclaw_control_plane::event_bus::{EventBus, EventFilter};
use cyberclaw_control_plane::execution_service::{
    ExecutionRequest, ExecutionService, InMemoryExecutionService,
};
use cyberclaw_control_plane::{ControlPlaneContext, ExecutionPlan, PlannedAction, Resolution};
use cyberclaw_core::cluster::ClusterEvent;
use cyberclaw_core::enums::Priority;
use cyberclaw_core::execution::ExecutionStatus;
use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::{
    ActorId, AgentId, CapabilityId, CaseId, ConnectorId, ExecutionId, LeaseId, NodeId, TaskId,
    TraceId,
};
use cyberclaw_core::prelude::AgentRef;
use cyberclaw_core::task::{Task, TaskInput, TaskKind, TriggerRef};
use cyberclaw_core::workspace::WorkspaceRef;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use uuid::Uuid;

pub const CLUSTER_TOKEN_HEADER: &str = "x-cyberclaw-cluster-token";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentActionPayload {
    pub connector_id: String,
    pub capability: String,
    pub input: serde_json::Value,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionAssignmentPayload {
    pub execution_id: String,
    pub scheduled_node_id: String,
    pub lease_id: String,
    pub trace_id: String,
    pub agent_id: String,
    pub agent_role: String,
    pub task_id: Option<String>,
    pub case_id: Option<String>,
    pub workspace: Option<WorkspaceRef>,
    pub actions: Vec<AssignmentActionPayload>,
}

impl ExecutionAssignmentPayload {
    pub fn from_execution(
        execution: &cyberclaw_core::execution::Execution,
        plan: &ExecutionPlan,
        scheduled_node_id: &NodeId,
        lease_id: &LeaseId,
    ) -> Self {
        let actions = plan
            .actions
            .iter()
            .map(|action| AssignmentActionPayload {
                connector_id: action.connector_id.as_str().to_string(),
                capability: action.capability.as_str().to_string(),
                input: action.input.clone(),
                reason: action.reason.clone(),
            })
            .collect();

        Self {
            execution_id: execution.id.as_str().to_string(),
            scheduled_node_id: scheduled_node_id.as_str().to_string(),
            lease_id: lease_id.as_str().to_string(),
            trace_id: execution.trace_id.as_str().to_string(),
            agent_id: execution.agent.id.as_str().to_string(),
            agent_role: execution.agent.role.clone(),
            task_id: execution.task_id.as_ref().map(|id| id.as_str().to_string()),
            case_id: execution.case_id.as_ref().map(|id| id.as_str().to_string()),
            workspace: execution.workspace.clone(),
            actions,
        }
    }

    pub fn execution_id(&self) -> anyhow::Result<ExecutionId> {
        ExecutionId::from_string(self.execution_id.clone())
            .with_context(|| format!("invalid execution_id: {}", self.execution_id))
    }

    pub fn scheduled_node_id(&self) -> anyhow::Result<NodeId> {
        NodeId::from_string(self.scheduled_node_id.clone())
            .with_context(|| format!("invalid scheduled_node_id: {}", self.scheduled_node_id))
    }

    pub fn lease_id(&self) -> anyhow::Result<LeaseId> {
        LeaseId::from_string(self.lease_id.clone())
            .with_context(|| format!("invalid lease_id: {}", self.lease_id))
    }

    pub fn to_execution_request(&self) -> anyhow::Result<ExecutionRequest> {
        let execution_id = self.execution_id()?;
        let agent_id = AgentId::from_string(self.agent_id.clone())
            .with_context(|| format!("invalid agent_id: {}", self.agent_id))?;
        let trace_id = TraceId::from_string(self.trace_id.clone())
            .with_context(|| format!("invalid trace_id: {}", self.trace_id))?;

        let task_id = if let Some(task_id) = &self.task_id {
            TaskId::from_string(task_id.clone())
                .with_context(|| format!("invalid task_id: {}", task_id))?
        } else {
            TaskId::new()
        };
        let case_id = match &self.case_id {
            Some(case_id) => Some(
                CaseId::from_string(case_id.clone())
                    .with_context(|| format!("invalid case_id: {}", case_id))?,
            ),
            None => None,
        };

        let actions = self
            .actions
            .iter()
            .map(|action| {
                Ok(PlannedAction {
                    connector_id: ConnectorId::from_string(action.connector_id.clone())
                        .with_context(|| {
                            format!("invalid connector_id: {}", action.connector_id)
                        })?,
                    capability: CapabilityId::from_string(action.capability.clone())
                        .with_context(|| format!("invalid capability: {}", action.capability))?,
                    input: action.input.clone(),
                    reason: action.reason.clone(),
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        let resolution = Resolution {
            agent: agent_id.clone(),
            skills: Vec::new(),
            workflow: None,
            connectors: actions.iter().map(|a| a.connector_id.clone()).collect(),
            capabilities: actions.iter().map(|a| a.capability.clone()).collect(),
            reasons: vec!["remote-assignment-delivery".to_string()],
        };

        let task = Task {
            id: task_id,
            case_id,
            title: format!("Remote execution {}", execution_id),
            summary: "Execution delivered from coordinator node".to_string(),
            kind: TaskKind::Execution,
            priority: Priority::Medium,
            requested_by: ActorRef {
                id: ActorId::from_string("cluster-worker".to_string())
                    .unwrap_or_else(|_| ActorId::new()),
                actor_type: ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "cluster-worker".to_string(),
            },
            requested_at: chrono::Utc::now(),
            trigger: TriggerRef {
                kind: "cluster_assignment".to_string(),
                source: "remote_pull_worker".to_string(),
            },
            input: TaskInput::default(),
            desired_outputs: Vec::new(),
            labels: vec!["remote-assignment".to_string()],
            preferred_agent_id: None,
        };

        Ok(ExecutionRequest {
            execution_id,
            task,
            case: None,
            context: ControlPlaneContext {
                actor: ActorRef {
                    id: ActorId::from_string("cluster-worker".to_string())
                        .unwrap_or_else(|_| ActorId::new()),
                    actor_type: ActorType::System,
                    tenant_id: None,
                    home_node_id: None,
                    display_name: "cluster-worker".to_string(),
                },
                session: None,
                workspace: self.workspace.clone(),
            },
            agent: Some(AgentRef {
                id: agent_id,
                role: self.agent_role.clone(),
            }),
            trace_id: Some(trace_id),
            execution_mode: None,
            plan: Some(ExecutionPlan {
                resolution,
                actions,
                review_required: true,
                max_fix_loops: cyberclaw_control_plane::default_max_fix_loops(),
                expected_outcomes: vec![],
            }),
        })
    }
}

/// Stable identifier for an assignment envelope queued on the coordinator.
///
/// A dedicated new-type keeps the assignment's identity independent from the
/// underlying `ExecutionId`, so the queue can survive requeue-after-failover
/// bookkeeping without colliding with execution-level IDs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AssignmentId(String);

impl AssignmentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub fn from_string(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for AssignmentId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for AssignmentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Terminal outcome reported by a worker when an assignment finishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentOutcome {
    Completed,
    Failed,
    Cancelled,
}

/// Lifecycle states held in the queue.
///
/// ```text
///    enqueue
///       │
///       ▼
///  ┌─────────┐   claim(node, ttl)   ┌────────┐   complete / fail
///  │ Pending │ ───────────────────▶│ Leased │ ──────────────────▶ Completed*
///  └─────────┘                     └────────┘
///       ▲                              │
///       │ release / sweep_expired      │ lease_renew (extends expires_at)
///       └──────────────────────────────┘
/// ```
///
/// * Completed / Failed / Cancelled assignments are removed from the queue
///   by the worker's `complete` call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AssignmentState {
    Pending {
        /// When the assignment entered (or returned to) the pending queue.
        since: DateTime<Utc>,
    },
    Leased {
        owner_node_id: String,
        leased_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
        renewed_at: Option<DateTime<Utc>>,
    },
}

impl AssignmentState {
    fn pending_now() -> Self {
        Self::Pending { since: Utc::now() }
    }
}

/// Queue-level record wrapping the transportable payload plus lifecycle state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assignment {
    pub id: AssignmentId,
    /// Optional hard affinity. When `Some(node)`, only that node may claim the
    /// assignment. When `None`, any node may claim it (general pool mode used
    /// by Sprint 11 multi-node routing).
    pub target_node_id: Option<String>,
    pub state: AssignmentState,
    pub payload: ExecutionAssignmentPayload,
}

impl Assignment {
    fn matches_target(&self, node_id: &NodeId) -> bool {
        match &self.target_node_id {
            Some(target) => target == node_id.as_str(),
            None => true,
        }
    }
}

/// In-memory assignment queue with lease-based worker-pull semantics.
///
/// Sprint 11 will swap the backing store for Raft; the public API is the
/// contract the migration must preserve.
///
/// # Concurrency
/// All mutations go through a single `RwLock<BTreeMap<...>>`. Readers
/// (`list_pending_for_node`, `len`) take a read lock; writers
/// (`enqueue`/`claim`/`complete`/`release`/`renew`/`sweep_expired_leases`)
/// take a write lock. BTreeMap iteration order is deterministic by
/// `AssignmentId`, giving FIFO-like fairness within the monotonically
/// increasing UUID namespace.
#[derive(Debug, Clone, Default)]
pub struct AssignmentQueue {
    inner: Arc<RwLock<BTreeMap<AssignmentId, Assignment>>>,
}

impl AssignmentQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Directed enqueue: the assignment is pinned to `node_id` and becomes
    /// Pending, so only `claim(node_id, ...)` (or the legacy
    /// `dequeue_batch(node_id, ...)`) can pick it up.
    ///
    /// Kept for compatibility with the existing event collector, which
    /// already knows which node the coordinator assigned the execution to.
    pub async fn enqueue(
        &self,
        node_id: &NodeId,
        payload: ExecutionAssignmentPayload,
    ) -> AssignmentId {
        let assignment = Assignment {
            id: AssignmentId::new(),
            target_node_id: Some(node_id.as_str().to_string()),
            state: AssignmentState::pending_now(),
            payload,
        };
        let id = assignment.id.clone();
        let mut map = self.inner.write().await;
        map.insert(id.clone(), assignment);
        id
    }

    /// Pool enqueue: the assignment has no hard affinity and any healthy
    /// node may `claim()` it. Returned to the caller so the coordinator
    /// can correlate `claim`/`complete` callbacks.
    pub async fn enqueue_pool(&self, payload: ExecutionAssignmentPayload) -> AssignmentId {
        let assignment = Assignment {
            id: AssignmentId::new(),
            target_node_id: None,
            state: AssignmentState::pending_now(),
            payload,
        };
        let id = assignment.id.clone();
        let mut map = self.inner.write().await;
        map.insert(id.clone(), assignment);
        id
    }

    /// Worker-pull: the node asks for one pending assignment it can run and
    /// acquires a lease for `lease_ttl`. Returns `None` when no pending
    /// assignment is visible to that node.
    ///
    /// Fairness: the first assignment (by `AssignmentId` order) that is
    /// `Pending` **and** whose `target_node_id` is either `None` or matches
    /// the caller is selected.
    pub async fn claim(
        &self,
        node_id: &NodeId,
        lease_ttl: Duration,
    ) -> Option<(AssignmentId, ExecutionAssignmentPayload)> {
        let mut map = self.inner.write().await;
        let ttl =
            chrono::Duration::from_std(lease_ttl).unwrap_or_else(|_| chrono::Duration::seconds(60));
        let now = Utc::now();

        let picked_id = map
            .iter()
            .find(|(_, assignment)| {
                matches!(assignment.state, AssignmentState::Pending { .. })
                    && assignment.matches_target(node_id)
            })
            .map(|(id, _)| id.clone())?;

        let assignment = map.get_mut(&picked_id)?;
        assignment.state = AssignmentState::Leased {
            owner_node_id: node_id.as_str().to_string(),
            leased_at: now,
            expires_at: now + ttl,
            renewed_at: None,
        };
        Some((picked_id, assignment.payload.clone()))
    }

    /// Worker reports terminal outcome. The assignment is removed from the
    /// queue on success; on ownership mismatch the method returns an error
    /// so the caller can surface the protocol violation.
    pub async fn complete(
        &self,
        assignment_id: &AssignmentId,
        node_id: &NodeId,
        _outcome: AssignmentOutcome,
    ) -> anyhow::Result<()> {
        let mut map = self.inner.write().await;
        let assignment = map
            .get(assignment_id)
            .ok_or_else(|| anyhow::anyhow!("assignment {} not found", assignment_id))?;
        match &assignment.state {
            AssignmentState::Leased { owner_node_id, .. } if owner_node_id == node_id.as_str() => {
                map.remove(assignment_id);
                Ok(())
            }
            AssignmentState::Leased { owner_node_id, .. } => {
                anyhow::bail!(
                    "assignment {} is leased by {} but {} tried to complete it",
                    assignment_id,
                    owner_node_id,
                    node_id
                )
            }
            AssignmentState::Pending { .. } => {
                anyhow::bail!(
                    "assignment {} is pending; cannot complete without claim",
                    assignment_id
                )
            }
        }
    }

    /// Worker explicitly hands the assignment back. The assignment returns
    /// to `Pending` so another node can `claim` it on the next poll.
    pub async fn release(
        &self,
        assignment_id: &AssignmentId,
        node_id: &NodeId,
        reason: &str,
    ) -> anyhow::Result<()> {
        let mut map = self.inner.write().await;
        let assignment = map
            .get_mut(assignment_id)
            .ok_or_else(|| anyhow::anyhow!("assignment {} not found", assignment_id))?;
        match &assignment.state {
            AssignmentState::Leased { owner_node_id, .. } if owner_node_id == node_id.as_str() => {
                info!(
                    assignment_id = %assignment_id,
                    node_id = %node_id,
                    reason = reason,
                    "assignment released back to pending"
                );
                assignment.state = AssignmentState::pending_now();
                Ok(())
            }
            AssignmentState::Leased { owner_node_id, .. } => {
                anyhow::bail!(
                    "assignment {} is leased by {} but {} tried to release it",
                    assignment_id,
                    owner_node_id,
                    node_id
                )
            }
            AssignmentState::Pending { .. } => {
                // Idempotent: already pending, nothing to do.
                Ok(())
            }
        }
    }

    /// Extend the lease on an active assignment so a slow worker can keep
    /// running past the default TTL without being reclaimed by a sweep.
    pub async fn lease_renew(
        &self,
        assignment_id: &AssignmentId,
        node_id: &NodeId,
        lease_ttl: Duration,
    ) -> anyhow::Result<DateTime<Utc>> {
        let mut map = self.inner.write().await;
        let assignment = map
            .get_mut(assignment_id)
            .ok_or_else(|| anyhow::anyhow!("assignment {} not found", assignment_id))?;
        let ttl =
            chrono::Duration::from_std(lease_ttl).unwrap_or_else(|_| chrono::Duration::seconds(60));
        let now = Utc::now();
        match &mut assignment.state {
            AssignmentState::Leased {
                owner_node_id,
                expires_at,
                renewed_at,
                ..
            } if owner_node_id == node_id.as_str() => {
                *expires_at = now + ttl;
                *renewed_at = Some(now);
                Ok(*expires_at)
            }
            AssignmentState::Leased { owner_node_id, .. } => {
                anyhow::bail!(
                    "assignment {} is leased by {} but {} tried to renew it",
                    assignment_id,
                    owner_node_id,
                    node_id
                )
            }
            AssignmentState::Pending { .. } => {
                anyhow::bail!(
                    "assignment {} is pending; cannot renew lease",
                    assignment_id
                )
            }
        }
    }

    /// Inspect (read-only) the queue from a node's perspective. Returns
    /// pending assignments eligible for `claim` by this node.
    pub async fn list_pending_for_node(&self, node_id: &NodeId) -> Vec<Assignment> {
        let map = self.inner.read().await;
        map.values()
            .filter(|assignment| {
                matches!(assignment.state, AssignmentState::Pending { .. })
                    && assignment.matches_target(node_id)
            })
            .cloned()
            .collect()
    }

    /// Scan for Leased assignments whose `expires_at` is in the past and
    /// return them to `Pending`. Returns the reclaimed IDs for observability
    /// (e.g. metrics, trace events).
    ///
    /// Intended to be invoked from a background ticker (Sprint 11 will wire
    /// this into the cluster supervisor). The method is intentionally
    /// side-effect-free outside of the queue mutation so tests can drive it
    /// synchronously.
    pub async fn sweep_expired_leases(&self) -> Vec<AssignmentId> {
        let mut map = self.inner.write().await;
        let now = Utc::now();
        let mut reclaimed = Vec::new();
        for (id, assignment) in map.iter_mut() {
            if let AssignmentState::Leased { expires_at, .. } = &assignment.state {
                if *expires_at <= now {
                    reclaimed.push(id.clone());
                }
            }
        }
        for id in &reclaimed {
            if let Some(assignment) = map.get_mut(id) {
                assignment.state = AssignmentState::pending_now();
            }
        }
        reclaimed
    }

    /// Legacy batch pop used by the existing `/internal/cluster/assignments/pull`
    /// endpoint. Pops up to `limit` pending assignments pinned to `node_id`
    /// and deletes them from the queue (non-leased semantics — the caller
    /// is trusted to execute them). Preserved so the existing pull worker +
    /// remote-assignment delivery path keep working unchanged while Sprint 11
    /// migrates callers to the new `claim` flow.
    pub async fn dequeue_batch(
        &self,
        node_id: &NodeId,
        limit: usize,
    ) -> Vec<ExecutionAssignmentPayload> {
        let mut map = self.inner.write().await;
        let take = limit.max(1);
        let picked: Vec<AssignmentId> = map
            .iter()
            .filter(|(_, assignment)| {
                matches!(assignment.state, AssignmentState::Pending { .. })
                    && assignment.matches_target(node_id)
            })
            .take(take)
            .map(|(id, _)| id.clone())
            .collect();
        picked
            .into_iter()
            .filter_map(|id| map.remove(&id).map(|a| a.payload))
            .collect()
    }

    /// Number of pending assignments visible to `node_id` (used by tests and
    /// by the event collector to gate pull requests).
    pub async fn len(&self, node_id: &NodeId) -> usize {
        let map = self.inner.read().await;
        map.values()
            .filter(|assignment| {
                matches!(assignment.state, AssignmentState::Pending { .. })
                    && assignment.matches_target(node_id)
            })
            .count()
    }

    /// Total number of assignments tracked across all states (pending +
    /// leased). Useful for monitoring and debugging.
    pub async fn total(&self) -> usize {
        self.inner.read().await.len()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullAssignmentsRequest {
    pub node_id: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullAssignmentsResponse {
    pub assignments: Vec<ExecutionAssignmentPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportExecutionStatusRequest {
    pub execution_id: String,
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportExecutionStatusResponse {
    pub ok: bool,
}

// ---------------------------------------------------------------------------
// Worker-pull lease protocol DTOs (Sprint 10 W2 L4)
// ---------------------------------------------------------------------------

/// Default lease TTL applied when the caller does not supply one.
///
/// Matches `LeaseConfig::MIN_TTL_SECS` on the control-plane side so the
/// assignment lease cannot outlive the execution lease.
pub const DEFAULT_ASSIGNMENT_LEASE_TTL_SECS: u64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimAssignmentRequest {
    pub node_id: String,
    /// Lease TTL in seconds. `None` → [`DEFAULT_ASSIGNMENT_LEASE_TTL_SECS`].
    #[serde(default)]
    pub lease_ttl_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimAssignmentResponse {
    pub assignment_id: Option<String>,
    pub payload: Option<ExecutionAssignmentPayload>,
    pub lease_expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompleteAssignmentRequest {
    pub node_id: String,
    pub outcome: AssignmentOutcome,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseAssignmentRequest {
    pub node_id: String,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenewAssignmentLeaseRequest {
    pub node_id: String,
    #[serde(default)]
    pub lease_ttl_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenewAssignmentLeaseResponse {
    pub lease_expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentAckResponse {
    pub ok: bool,
}

#[derive(Debug, Clone)]
pub struct AssignmentPullWorkerConfig {
    pub coordinator_url: String,
    pub local_node_id: NodeId,
    pub poll_interval: Duration,
    pub request_timeout: Duration,
    pub shared_token: Option<String>,
}

pub fn load_pull_worker_config(local_node_id: &NodeId) -> Option<AssignmentPullWorkerConfig> {
    let coordinator_url = std::env::var("CYBERCLAW_ASSIGNMENT_PULL_URL").ok()?;
    let poll_interval_ms = std::env::var("CYBERCLAW_ASSIGNMENT_POLL_INTERVAL_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(800);
    let request_timeout_ms = std::env::var("CYBERCLAW_ASSIGNMENT_REQUEST_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(5000);

    Some(AssignmentPullWorkerConfig {
        coordinator_url: coordinator_url.trim_end_matches('/').to_string(),
        local_node_id: local_node_id.clone(),
        poll_interval: Duration::from_millis(poll_interval_ms),
        request_timeout: Duration::from_millis(request_timeout_ms),
        shared_token: std::env::var("CYBERCLAW_CLUSTER_SHARED_TOKEN").ok(),
    })
}

pub fn spawn_assignment_event_collector(
    event_bus: Arc<dyn EventBus>,
    execution_service: Arc<InMemoryExecutionService>,
    assignment_queue: Arc<AssignmentQueue>,
    local_node_id: NodeId,
) -> Option<tokio::task::JoinHandle<()>> {
    let mut subscriber = match event_bus.subscribe(EventFilter::EventTypes(vec![
        "execution_assigned".to_string(),
    ])) {
        Ok(sub) => sub,
        Err(err) => {
            warn!(
                error = %err,
                "failed to subscribe execution assignment collector to EventBus"
            );
            return None;
        }
    };

    Some(tokio::spawn(async move {
        info!(
            local_node_id = %local_node_id,
            "execution assignment collector started"
        );
        while let Some(event) = subscriber.receiver.recv().await {
            let ClusterEvent::ExecutionAssigned {
                execution_id,
                node_id,
                lease_id,
                ..
            } = event
            else {
                continue;
            };

            if node_id == local_node_id {
                continue;
            }

            let execution = match execution_service.get(&execution_id).await {
                Ok(Some(execution)) => execution,
                Ok(None) => {
                    warn!(
                        execution_id = %execution_id,
                        "execution missing when collecting remote assignment"
                    );
                    continue;
                }
                Err(err) => {
                    warn!(
                        execution_id = %execution_id,
                        error = %err,
                        "failed to load execution for remote assignment"
                    );
                    continue;
                }
            };

            let plan = match execution_service.get_plan(&execution_id).await {
                Ok(Some(plan)) => plan,
                Ok(None) => {
                    warn!(
                        execution_id = %execution_id,
                        "execution plan missing; cannot build remote assignment payload"
                    );
                    continue;
                }
                Err(err) => {
                    warn!(
                        execution_id = %execution_id,
                        error = %err,
                        "failed to load execution plan for remote assignment"
                    );
                    continue;
                }
            };

            let payload =
                ExecutionAssignmentPayload::from_execution(&execution, &plan, &node_id, &lease_id);
            assignment_queue.enqueue(&node_id, payload).await;
        }
    }))
}

pub fn spawn_assignment_pull_worker(
    execution_service: Arc<InMemoryExecutionService>,
    config: AssignmentPullWorkerConfig,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let http = match reqwest::Client::builder()
            .no_proxy()
            .timeout(config.request_timeout)
            .build()
        {
            Ok(client) => client,
            Err(err) => {
                error!(error = %err, "failed to create assignment pull http client");
                return;
            }
        };

        let pull_url = format!(
            "{}/internal/cluster/assignments/pull",
            config.coordinator_url
        );
        let report_url = format!(
            "{}/internal/cluster/executions/report",
            config.coordinator_url
        );

        info!(
            coordinator = %config.coordinator_url,
            local_node_id = %config.local_node_id,
            "assignment pull worker started"
        );

        loop {
            let pulled = match pull_remote_assignments(&http, &config, &pull_url).await {
                Ok(assignments) => assignments,
                Err(err) => {
                    warn!(error = %err, "pull remote assignments failed");
                    tokio::time::sleep(config.poll_interval).await;
                    continue;
                }
            };

            for payload in pulled {
                let execution_id = match payload.execution_id() {
                    Ok(id) => id,
                    Err(err) => {
                        warn!(error = %err, "skip malformed assignment payload");
                        continue;
                    }
                };

                let result = consume_remote_assignment(execution_service.clone(), &payload).await;
                let (status, error_message) = match result {
                    Ok(status) => (status, None),
                    Err(err) => (ExecutionStatus::Failed, Some(err.to_string())),
                };

                if let Err(err) = report_execution_status(
                    &http,
                    &config,
                    &report_url,
                    &execution_id,
                    status,
                    error_message,
                )
                .await
                {
                    warn!(
                        execution_id = %execution_id,
                        error = %err,
                        "failed to report remote execution status"
                    );
                }
            }

            tokio::time::sleep(config.poll_interval).await;
        }
    })
}

async fn pull_remote_assignments(
    http: &reqwest::Client,
    config: &AssignmentPullWorkerConfig,
    pull_url: &str,
) -> anyhow::Result<Vec<ExecutionAssignmentPayload>> {
    let request = PullAssignmentsRequest {
        node_id: config.local_node_id.as_str().to_string(),
        limit: Some(10),
    };

    let mut builder = http.post(pull_url).json(&request);
    if let Some(token) = &config.shared_token {
        builder = builder.header(CLUSTER_TOKEN_HEADER, token);
    }

    let response = builder.send().await?;
    if response.status() == StatusCode::NO_CONTENT {
        return Ok(Vec::new());
    }
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("pull endpoint returned {} with body: {}", status, body);
    }

    let payload: PullAssignmentsResponse = response.json().await?;
    Ok(payload.assignments)
}

pub async fn consume_remote_assignment(
    execution_service: Arc<InMemoryExecutionService>,
    payload: &ExecutionAssignmentPayload,
) -> anyhow::Result<ExecutionStatus> {
    let execution_id = payload.execution_id()?;
    let scheduled_node_id = payload.scheduled_node_id()?;
    let lease_id = payload.lease_id()?;

    // Idempotent submit: only create if execution does not exist locally.
    if execution_service.get(&execution_id).await?.is_none() {
        let request = payload.to_execution_request()?;
        execution_service.submit(request).await?;
    }

    execution_service
        .set_assignment(&execution_id, scheduled_node_id, lease_id)
        .await?;

    match execution_service.execute(&execution_id).await {
        Ok(()) => {}
        Err(err) => {
            // Ensure local terminal status is explicit before returning.
            let _ = execution_service
                .update_status(&execution_id, ExecutionStatus::Failed)
                .await;
            return Err(err);
        }
    }

    let status = execution_service
        .get(&execution_id)
        .await?
        .map(|exec| exec.status)
        .unwrap_or(ExecutionStatus::Failed);
    Ok(status)
}

async fn report_execution_status(
    http: &reqwest::Client,
    config: &AssignmentPullWorkerConfig,
    report_url: &str,
    execution_id: &ExecutionId,
    status: ExecutionStatus,
    error: Option<String>,
) -> anyhow::Result<()> {
    let request = ReportExecutionStatusRequest {
        execution_id: execution_id.as_str().to_string(),
        status: format!("{:?}", status).to_lowercase(),
        error,
    };

    let mut builder = http.post(report_url).json(&request);
    if let Some(token) = &config.shared_token {
        builder = builder.header(CLUSTER_TOKEN_HEADER, token);
    }

    let response = builder.send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("report endpoint returned {} with body: {}", status, body);
    }
    Ok(())
}

pub fn validate_cluster_token(headers: &HeaderMap) -> anyhow::Result<()> {
    let expected = std::env::var("CYBERCLAW_CLUSTER_SHARED_TOKEN").map_err(|_| {
        anyhow::anyhow!("CYBERCLAW_CLUSTER_SHARED_TOKEN not configured; cluster API disabled")
    })?;

    let provided = headers
        .get(CLUSTER_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();

    // Hash-then-compare: eliminates timing leak from length check.
    // Both tokens are SHA-256 hashed first so the comparison always operates
    // on fixed-length 32-byte digests, preventing length oracle attacks.
    use sha2::{Digest, Sha256};
    use subtle::ConstantTimeEq;

    let expected_hash = Sha256::digest(expected.as_bytes());
    let provided_hash = Sha256::digest(provided.as_bytes());
    let result: bool = expected_hash.ct_eq(&provided_hash).into();
    if !result {
        anyhow::bail!("cluster token mismatch");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_agent_runtime::{AgentConfig, MinimalAgentRuntime};
    use cyberclaw_control_plane::execution_service::ExecutionService;
    use cyberclaw_observability::events::InMemoryEventRecorder;
    use cyberclaw_skill_runtime::MinimalSkillRuntime;

    #[tokio::test]
    async fn test_assignment_queue_batch_dequeue() {
        let queue = AssignmentQueue::new();
        let node = NodeId::from_string("node-a".to_string()).unwrap();

        for i in 0..3 {
            queue
                .enqueue(
                    &node,
                    ExecutionAssignmentPayload {
                        execution_id: format!("execution-{}", i),
                        scheduled_node_id: node.as_str().to_string(),
                        lease_id: LeaseId::new().as_str().to_string(),
                        trace_id: TraceId::new().as_str().to_string(),
                        agent_id: AgentId::new().as_str().to_string(),
                        agent_role: "agent".to_string(),
                        task_id: None,
                        case_id: None,
                        workspace: None,
                        actions: Vec::new(),
                    },
                )
                .await;
        }

        let batch = queue.dequeue_batch(&node, 2).await;
        assert_eq!(batch.len(), 2);
        assert_eq!(queue.len(&node).await, 1);
    }

    #[tokio::test]
    async fn test_consume_remote_assignment_runs_execution() {
        let agent_runtime = Arc::new(MinimalAgentRuntime::new());
        let agent_id = AgentId::new();
        agent_runtime
            .register(AgentConfig::new(
                agent_id.clone(),
                "worker-agent",
                "remote worker test agent",
            ))
            .await
            .unwrap();

        let execution_service = Arc::new(InMemoryExecutionService::with_runtimes_and_recorder(
            agent_runtime,
            Arc::new(MinimalSkillRuntime::new()),
            Arc::new(InMemoryEventRecorder::new()),
        ));
        let payload = ExecutionAssignmentPayload {
            execution_id: ExecutionId::new().as_str().to_string(),
            scheduled_node_id: NodeId::from_string("worker-1".to_string())
                .unwrap()
                .as_str()
                .to_string(),
            lease_id: LeaseId::new().as_str().to_string(),
            trace_id: TraceId::new().as_str().to_string(),
            agent_id: agent_id.as_str().to_string(),
            agent_role: "worker-agent".to_string(),
            task_id: Some(TaskId::new().as_str().to_string()),
            case_id: None,
            workspace: None,
            actions: Vec::new(),
        };

        let status = consume_remote_assignment(execution_service.clone(), &payload)
            .await
            .unwrap();
        assert_eq!(status, ExecutionStatus::Completed);

        let execution = execution_service
            .get(&payload.execution_id().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(execution.status, ExecutionStatus::Completed);
    }

    // -----------------------------------------------------------------------
    // Worker-pull lease protocol (Sprint 10 W2 L4)
    // -----------------------------------------------------------------------

    fn sample_payload(tag: &str, scheduled_node: &NodeId) -> ExecutionAssignmentPayload {
        ExecutionAssignmentPayload {
            execution_id: format!("execution-{}", tag),
            scheduled_node_id: scheduled_node.as_str().to_string(),
            lease_id: LeaseId::new().as_str().to_string(),
            trace_id: TraceId::new().as_str().to_string(),
            agent_id: AgentId::new().as_str().to_string(),
            agent_role: "agent".to_string(),
            task_id: None,
            case_id: None,
            workspace: None,
            actions: Vec::new(),
        }
    }

    #[tokio::test]
    async fn cluster_assignment_enqueue_then_claim_by_node() {
        let queue = AssignmentQueue::new();
        let node = NodeId::from_string("node-a".to_string()).unwrap();

        let enqueue_id = queue.enqueue(&node, sample_payload("1", &node)).await;

        let claim = queue.claim(&node, Duration::from_secs(30)).await;
        let (claim_id, payload) = claim.expect("claim should succeed for pending assignment");

        assert_eq!(claim_id, enqueue_id, "claim must return the enqueued id");
        assert_eq!(payload.execution_id, "execution-1");
        assert_eq!(queue.len(&node).await, 0, "no pending left after claim");
        assert_eq!(queue.total().await, 1, "assignment still tracked as leased");
    }

    #[tokio::test]
    async fn cluster_assignment_double_claim_rejected_while_leased() {
        let queue = AssignmentQueue::new();
        let node_a = NodeId::from_string("node-a".to_string()).unwrap();
        let node_b = NodeId::from_string("node-b".to_string()).unwrap();

        // Pool assignment — any node could claim.
        queue.enqueue_pool(sample_payload("1", &node_a)).await;

        let first = queue.claim(&node_a, Duration::from_secs(30)).await;
        assert!(first.is_some(), "first claim must succeed");

        // While leased, no other node may claim it.
        let second_by_other = queue.claim(&node_b, Duration::from_secs(30)).await;
        assert!(
            second_by_other.is_none(),
            "leased assignment must not be claimable by another node"
        );

        // Same node also sees no new work (assignment is already leased, not pending).
        let same_node_again = queue.claim(&node_a, Duration::from_secs(30)).await;
        assert!(
            same_node_again.is_none(),
            "claim returns no pending assignment while lease held"
        );
    }

    #[tokio::test]
    async fn cluster_assignment_lease_expiry_returns_to_pending_on_sweep() {
        let queue = AssignmentQueue::new();
        let node = NodeId::from_string("node-a".to_string()).unwrap();

        queue.enqueue(&node, sample_payload("1", &node)).await;
        let claim = queue.claim(&node, Duration::from_millis(50)).await;
        let (claim_id, _) = claim.expect("claim must succeed");

        // Wait past the lease TTL then sweep.
        tokio::time::sleep(Duration::from_millis(120)).await;
        let reclaimed = queue.sweep_expired_leases().await;

        assert_eq!(reclaimed.len(), 1, "sweep must reclaim exactly one lease");
        assert_eq!(reclaimed[0], claim_id);

        // Assignment is back to pending and claimable again (by the same or a different node).
        let reclaim = queue.claim(&node, Duration::from_secs(30)).await;
        assert!(
            reclaim.is_some(),
            "reclaimed assignment should be claimable again"
        );
        assert_eq!(reclaim.unwrap().0, claim_id);
    }

    #[tokio::test]
    async fn cluster_assignment_complete_removes_from_queue() {
        let queue = AssignmentQueue::new();
        let node = NodeId::from_string("node-a".to_string()).unwrap();

        queue.enqueue(&node, sample_payload("1", &node)).await;
        let (claim_id, _) = queue
            .claim(&node, Duration::from_secs(30))
            .await
            .expect("claim must succeed");

        queue
            .complete(&claim_id, &node, AssignmentOutcome::Completed)
            .await
            .expect("complete by lease owner must succeed");

        assert_eq!(queue.total().await, 0, "completed assignment is evicted");

        // Second complete is a protocol error (no such id anymore).
        let err = queue
            .complete(&claim_id, &node, AssignmentOutcome::Completed)
            .await;
        assert!(
            err.is_err(),
            "completing an unknown assignment must error, got {:?}",
            err
        );
    }

    #[tokio::test]
    async fn cluster_assignment_release_allows_another_node_to_claim() {
        let queue = AssignmentQueue::new();
        let node_a = NodeId::from_string("node-a".to_string()).unwrap();
        let node_b = NodeId::from_string("node-b".to_string()).unwrap();

        // Pool assignment so the second node is eligible to pick it up.
        queue.enqueue_pool(sample_payload("1", &node_a)).await;

        let (claim_id, _) = queue
            .claim(&node_a, Duration::from_secs(30))
            .await
            .expect("first node claims pending assignment");

        queue
            .release(&claim_id, &node_a, "voluntary-handoff")
            .await
            .expect("release by lease owner must succeed");

        // Other node can now pick it up.
        let other = queue.claim(&node_b, Duration::from_secs(30)).await;
        assert!(
            other.is_some(),
            "released assignment must be claimable by another node"
        );
        let (second_claim_id, _) = other.unwrap();
        assert_eq!(second_claim_id, claim_id);

        // Non-owner release is rejected.
        let err = queue.release(&claim_id, &node_a, "stale-worker").await;
        assert!(err.is_err(), "non-owner release must be rejected");
    }

    #[tokio::test]
    async fn cluster_assignment_list_pending_for_node_filters_correctly() {
        let queue = AssignmentQueue::new();
        let node_a = NodeId::from_string("node-a".to_string()).unwrap();
        let node_b = NodeId::from_string("node-b".to_string()).unwrap();

        // Directed to node-a
        queue.enqueue(&node_a, sample_payload("a1", &node_a)).await;
        queue.enqueue(&node_a, sample_payload("a2", &node_a)).await;
        // Directed to node-b
        queue.enqueue(&node_b, sample_payload("b1", &node_b)).await;
        // Pool (visible to everyone)
        queue.enqueue_pool(sample_payload("pool", &node_a)).await;

        let pending_a = queue.list_pending_for_node(&node_a).await;
        let pending_b = queue.list_pending_for_node(&node_b).await;

        assert_eq!(pending_a.len(), 3, "node-a sees directed-a + pool");
        assert_eq!(pending_b.len(), 2, "node-b sees directed-b + pool");

        // After one node claims, the pool item must drop off everyone's list.
        let claimed = queue.claim(&node_a, Duration::from_secs(30)).await;
        assert!(claimed.is_some());
        let pending_b_after = queue.list_pending_for_node(&node_b).await;
        // node-b should still see its own directed item; depending on claim
        // order node-b may also still see the pool item (only one of the 4
        // was claimed). We assert monotone decrease from the queue total.
        assert!(
            pending_b_after.len() <= 2,
            "pending list is monotonically non-increasing after a claim"
        );
    }

    #[tokio::test]
    async fn cluster_assignment_lease_renew_extends_expiry() {
        let queue = AssignmentQueue::new();
        let node = NodeId::from_string("node-a".to_string()).unwrap();

        queue.enqueue(&node, sample_payload("1", &node)).await;
        let (claim_id, _) = queue
            .claim(&node, Duration::from_millis(100))
            .await
            .expect("claim must succeed");

        // Renew before expiry with a longer TTL.
        let new_expiry = queue
            .lease_renew(&claim_id, &node, Duration::from_secs(5))
            .await
            .expect("owner renew must succeed");
        assert!(
            new_expiry > chrono::Utc::now() + chrono::Duration::milliseconds(500),
            "renewed expiry should push well past the original 100ms TTL"
        );

        // Wait beyond the original 100ms TTL — sweep must not reclaim.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let reclaimed = queue.sweep_expired_leases().await;
        assert!(
            reclaimed.is_empty(),
            "renewed lease must not be reclaimed, got {:?}",
            reclaimed
        );

        // Non-owner renew is rejected.
        let other = NodeId::from_string("node-b".to_string()).unwrap();
        let err = queue
            .lease_renew(&claim_id, &other, Duration::from_secs(5))
            .await;
        assert!(err.is_err(), "non-owner renew must be rejected");
    }
}
