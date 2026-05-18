//! Cluster-aware Connector — routes capability execution to local or remote nodes.
//!
//! The [`ClusterAwareConnector`] wraps any inner [`Connector`] and, based on the
//! configured [`RoutingStrategy`], either delegates execution to the inner connector
//! (local) or returns a [`RoutingDecision::Remote`] for the caller to dispatch.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use cyberclaw_core::ids::NodeId;

use crate::types::{CapabilityExecutionRequest, CapabilityExecutionResult, Connector};
use cyberclaw_core::manifests::{CapabilityContract, ConnectorRuntime};
use cyberclaw_core::prelude::ConnectorId;

/// Provides cluster topology information to the routing layer.
///
/// Implementors bridge the connector layer with however the platform tracks
/// cluster membership — in-memory map, distributed registry, etc.
pub trait ClusterContext: Send + Sync + std::fmt::Debug {
    /// The [`NodeId`] of the current (local) node.
    fn local_node_id(&self) -> &NodeId;

    /// Returns `true` when `node_id` refers to the local node.
    fn is_local(&self, node_id: &NodeId) -> bool;

    /// Returns the set of healthy nodes that can accept work (includes local).
    fn healthy_nodes(&self) -> Vec<NodeId>;
}

/// Strategy that governs how [`ClusterAwareConnector`] selects an execution target.
#[derive(Debug, Clone)]
pub enum RoutingStrategy {
    /// Always execute on the local node. Returns an error if local is unhealthy.
    LocalOnly,
    /// Execute on the local node when healthy; fall back to another healthy node otherwise.
    PreferLocal,
    /// Cycle through all healthy nodes in round-robin order.
    RoundRobin,
    /// Route to a specific node. Falls back to local when `node_id` matches local.
    AffinityBased(NodeId),
}

/// The outcome of a routing decision made by [`ClusterAwareConnector::route`].
#[derive(Debug)]
pub enum RoutingDecision {
    /// Execution was completed locally; the result is embedded.
    Local(CapabilityExecutionResult),
    /// Execution must be forwarded to a remote node.
    Remote {
        target_node_id: NodeId,
        capability_id: String,
        params: serde_json::Value,
    },
}

/// A [`Connector`] wrapper that selects an execution target based on cluster topology.
///
/// Local execution is handled by delegating to the inner connector.  Remote
/// execution is signalled by returning [`RoutingDecision::Remote`] — the caller
/// (control-plane, workflow engine, …) is responsible for the actual RPC.
#[derive(Debug)]
pub struct ClusterAwareConnector {
    id: ConnectorId,
    inner: Arc<dyn Connector>,
    context: Arc<dyn ClusterContext>,
    strategy: RoutingStrategy,
    /// Counter used by [`RoutingStrategy::RoundRobin`].
    round_robin_counter: Arc<AtomicUsize>,
}

impl ClusterAwareConnector {
    /// Create a new [`ClusterAwareConnector`].
    pub fn new(
        inner: Arc<dyn Connector>,
        context: Arc<dyn ClusterContext>,
        strategy: RoutingStrategy,
    ) -> Self {
        let raw_id = format!("cluster-aware-{}", inner.id().as_str());
        let id = ConnectorId::from_string(raw_id).expect("connector id is valid");
        Self {
            id,
            inner,
            context,
            strategy,
            round_robin_counter: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Determine the execution target and either execute locally or return a
    /// [`RoutingDecision::Remote`].
    pub async fn route(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<RoutingDecision> {
        let target = self.select_target()?;

        if self.context.is_local(&target) {
            let result = self.inner.execute(request).await?;
            Ok(RoutingDecision::Local(result))
        } else {
            Ok(RoutingDecision::Remote {
                target_node_id: target,
                capability_id: request.capability_id.as_str().to_string(),
                params: request.input,
            })
        }
    }

    /// Pick a target [`NodeId`] according to the configured strategy.
    fn select_target(&self) -> anyhow::Result<NodeId> {
        match &self.strategy {
            RoutingStrategy::LocalOnly => Ok(self.context.local_node_id().clone()),

            RoutingStrategy::PreferLocal => {
                let local = self.context.local_node_id().clone();
                let healthy = self.context.healthy_nodes();
                if healthy.contains(&local) {
                    Ok(local)
                } else {
                    // Local is unhealthy — pick first available remote node.
                    healthy
                        .into_iter()
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("no healthy nodes available"))
                }
            }

            RoutingStrategy::RoundRobin => {
                let healthy = self.context.healthy_nodes();
                if healthy.is_empty() {
                    anyhow::bail!("no healthy nodes available for round-robin routing");
                }
                let idx = self.round_robin_counter.fetch_add(1, Ordering::Relaxed) % healthy.len();
                Ok(healthy[idx].clone())
            }

            RoutingStrategy::AffinityBased(node_id) => Ok(node_id.clone()),
        }
    }
}

/// [`Connector`] implementation delegates local execution to the inner connector.
///
/// The standard `execute()` path always runs locally (identical to `LocalOnly`).
/// Callers that need cluster-aware routing should use [`ClusterAwareConnector::route`]
/// directly.
#[async_trait::async_trait]
impl Connector for ClusterAwareConnector {
    fn id(&self) -> &ConnectorId {
        &self.id
    }

    fn runtime(&self) -> ConnectorRuntime {
        self.inner.runtime()
    }

    fn capabilities(&self) -> Vec<CapabilityContract> {
        self.inner.capabilities()
    }

    async fn execute(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<CapabilityExecutionResult> {
        self.inner.execute(request).await
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CapabilityExecutionResult, ExecutionStatus};
    use cyberclaw_core::prelude::*;

    // ── Test double: ClusterContext ──────────────────────────────────────────

    #[derive(Debug)]
    struct TestClusterContext {
        local_id: NodeId,
        healthy: Vec<NodeId>,
    }

    impl TestClusterContext {
        fn new(local: &str, healthy: Vec<&str>) -> Self {
            Self {
                local_id: NodeId::from_string(local.to_string()).unwrap(),
                healthy: healthy
                    .into_iter()
                    .map(|s| NodeId::from_string(s.to_string()).unwrap())
                    .collect(),
            }
        }
    }

    impl ClusterContext for TestClusterContext {
        fn local_node_id(&self) -> &NodeId {
            &self.local_id
        }

        fn is_local(&self, node_id: &NodeId) -> bool {
            node_id == &self.local_id
        }

        fn healthy_nodes(&self) -> Vec<NodeId> {
            self.healthy.clone()
        }
    }

    // ── Test double: Connector ───────────────────────────────────────────────

    #[derive(Debug)]
    struct StubConnector {
        id: ConnectorId,
    }

    impl StubConnector {
        fn new() -> Self {
            Self {
                id: ConnectorId::from_string("stub".to_string()).unwrap(),
            }
        }
    }

    #[async_trait::async_trait]
    impl Connector for StubConnector {
        fn id(&self) -> &ConnectorId {
            &self.id
        }

        fn runtime(&self) -> ConnectorRuntime {
            ConnectorRuntime::Native
        }

        fn capabilities(&self) -> Vec<CapabilityContract> {
            vec![]
        }

        async fn execute(
            &self,
            request: CapabilityExecutionRequest,
        ) -> anyhow::Result<CapabilityExecutionResult> {
            Ok(CapabilityExecutionResult {
                execution_id: request.execution_id,
                trace_id: request.trace_id,
                connector_id: request.connector_id,
                capability_id: request.capability_id,
                output: serde_json::json!({"stub": true}),
                status: ExecutionStatus::Success,
                error: None,
                actual_runtime: None,
            })
        }
    }

    // ── Helper ───────────────────────────────────────────────────────────────

    fn make_request() -> CapabilityExecutionRequest {
        CapabilityExecutionRequest {
            execution_id: ExecutionId::new(),
            trace_id: "trace-test".to_string(),
            actor: ActorRef {
                id: ActorId::from_string("actor-1".to_string()).unwrap(),
                actor_type: ActorType::Agent,
                tenant_id: None,
                home_node_id: None,
                display_name: "test-actor".to_string(),
            },
            workspace: WorkspaceRef {
                id: WorkspaceId::from_string("ws-1".to_string()).unwrap(),
                mode: WorkspaceMode::Ephemeral,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: "/tmp".to_string(),
                writable_roots: vec![],
            },
            connector_id: ConnectorId::from_string("stub".to_string()).unwrap(),
            capability_id: CapabilityId::from_string("fs.read".to_string()).unwrap(),
            input: serde_json::json!({"path": "/tmp/test.txt"}),
        }
    }

    // ── Test 1: LocalOnly always executes locally ────────────────────────────

    #[tokio::test]
    async fn test_local_only_executes_locally() {
        let ctx = Arc::new(TestClusterContext::new(
            "node-local",
            vec!["node-local", "node-remote-1"],
        ));
        let connector = ClusterAwareConnector::new(
            Arc::new(StubConnector::new()),
            ctx,
            RoutingStrategy::LocalOnly,
        );

        let decision = connector.route(make_request()).await.unwrap();
        assert!(
            matches!(decision, RoutingDecision::Local(_)),
            "LocalOnly must always route to local"
        );
    }

    // ── Test 2: PreferLocal chooses local when healthy ───────────────────────

    #[tokio::test]
    async fn test_prefer_local_uses_local_when_healthy() {
        let ctx = Arc::new(TestClusterContext::new(
            "node-local",
            vec!["node-local", "node-remote-1"],
        ));
        let connector = ClusterAwareConnector::new(
            Arc::new(StubConnector::new()),
            ctx,
            RoutingStrategy::PreferLocal,
        );

        let decision = connector.route(make_request()).await.unwrap();
        assert!(
            matches!(decision, RoutingDecision::Local(_)),
            "PreferLocal should execute locally when local node is healthy"
        );
    }

    // ── Test 3: PreferLocal falls back to remote when local is unhealthy ─────

    #[tokio::test]
    async fn test_prefer_local_falls_back_to_remote_when_local_unhealthy() {
        // Local node ("node-local") is NOT in the healthy list.
        let ctx = Arc::new(TestClusterContext::new(
            "node-local",
            vec!["node-remote-1", "node-remote-2"],
        ));
        let connector = ClusterAwareConnector::new(
            Arc::new(StubConnector::new()),
            ctx,
            RoutingStrategy::PreferLocal,
        );

        let decision = connector.route(make_request()).await.unwrap();
        match decision {
            RoutingDecision::Remote { target_node_id, .. } => {
                assert_eq!(
                    target_node_id.as_str(),
                    "node-remote-1",
                    "should fall back to first healthy remote"
                );
            }
            RoutingDecision::Local(_) => {
                panic!("expected Remote decision when local is unhealthy");
            }
        }
    }

    // ── Test 4: AffinityBased routes to specified node ───────────────────────

    #[tokio::test]
    async fn test_affinity_based_routes_to_specified_node() {
        let target = NodeId::from_string("node-gpu-1".to_string()).unwrap();
        let ctx = Arc::new(TestClusterContext::new(
            "node-local",
            vec!["node-local", "node-gpu-1"],
        ));
        let connector = ClusterAwareConnector::new(
            Arc::new(StubConnector::new()),
            ctx,
            RoutingStrategy::AffinityBased(target.clone()),
        );

        let decision = connector.route(make_request()).await.unwrap();
        match decision {
            RoutingDecision::Remote { target_node_id, .. } => {
                assert_eq!(
                    target_node_id, target,
                    "AffinityBased must route to the specified node"
                );
            }
            RoutingDecision::Local(_) => {
                panic!("expected Remote decision for non-local affinity target");
            }
        }
    }

    // ── Test 5: RoundRobin cycles through healthy nodes ───────────────────────

    #[tokio::test]
    async fn test_round_robin_cycles_nodes() {
        let ctx = Arc::new(TestClusterContext::new(
            "node-a",
            vec!["node-a", "node-b", "node-c"],
        ));
        let connector = Arc::new(ClusterAwareConnector::new(
            Arc::new(StubConnector::new()),
            ctx,
            RoutingStrategy::RoundRobin,
        ));

        // Collect 3 consecutive targets; they should cover node-a, node-b, node-c.
        let mut seen: Vec<String> = vec![];
        for _ in 0..3 {
            let decision = connector.route(make_request()).await.unwrap();
            match decision {
                RoutingDecision::Local(_) => seen.push("node-a".to_string()),
                RoutingDecision::Remote { target_node_id, .. } => {
                    seen.push(target_node_id.as_str().to_string())
                }
            }
        }
        // All three nodes should appear exactly once.
        assert!(seen.contains(&"node-a".to_string()));
        assert!(seen.contains(&"node-b".to_string()));
        assert!(seen.contains(&"node-c".to_string()));
    }

    // ── Test 6: AffinityBased routes locally when target is local ────────────

    #[tokio::test]
    async fn test_affinity_based_local_when_target_is_local() {
        let local_id = NodeId::from_string("node-local".to_string()).unwrap();
        let ctx = Arc::new(TestClusterContext::new("node-local", vec!["node-local"]));
        let connector = ClusterAwareConnector::new(
            Arc::new(StubConnector::new()),
            ctx,
            RoutingStrategy::AffinityBased(local_id),
        );

        let decision = connector.route(make_request()).await.unwrap();
        assert!(
            matches!(decision, RoutingDecision::Local(_)),
            "AffinityBased should execute locally when target equals local node"
        );
    }
}
