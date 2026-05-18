#![allow(clippy::duplicate_mod)]

#[path = "common/mod.rs"]
mod common;

use anyhow::Result;
use axum::Router;
use cyberclaw_connectors::{registry::ConnectorRegistry, CapabilityDispatcher, LocalConnector};
use cyberclaw_control_plane::{
    event_bus::{EventBus, InMemoryEventBus},
    execution_service::{ExecutionRequest, ExecutionService, InMemoryExecutionService},
    gateway_router::InMemoryGatewayRouter,
    lease_manager::InMemoryLeaseManager,
    membership_service::{InMemoryMembershipService, MembershipService},
    orchestrator::ControlPlaneOrchestrator,
    placement_engine::InMemoryPlacementEngine,
    registry::InMemoryRegistry,
    resolver::InMemoryResolver,
    review_queue::InMemoryReviewQueue,
    task_manager::InMemoryTaskManager,
    types::{ControlPlaneContext, ExecutionPlan, PlannedAction, Resolution},
};
use cyberclaw_core::{
    cluster::ClusterEvent,
    cluster::{MembershipState, NodeCapacity, NodeHealth, NodeRecord, NodeRole},
    execution::ExecutionStatus,
    identity::{ActorRef, ActorType},
    ids::{
        ActorId, AgentId, ConnectorId, ExecutionId, LeaseId, NodeId, TaskId, TraceId, WorkspaceId,
    },
    task::{Task, TaskInput, TaskKind, TriggerRef},
    workspace::{WorkspaceMode, WorkspaceRef},
};
use cyberclaw_governance::engine::DefaultPolicyEngine;
use cyberclaw_observability::{
    events::InMemoryEventRecorder, security_event_store::InMemorySecurityEventStore,
};
use cyberclaw_plugin_runtime::PluginRegistry;
use cyberclaw_server::cluster_assignments::{
    spawn_assignment_event_collector, spawn_assignment_pull_worker, AssignmentPullWorkerConfig,
    PullAssignmentsRequest, PullAssignmentsResponse, CLUSTER_TOKEN_HEADER,
};
use cyberclaw_server::{create_router_with_config, AppState, ControlPlaneComponents, ServerConfig};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tempfile::TempDir;
use tokio::{net::TcpListener, task::JoinHandle};

struct RunningNode {
    node_id: NodeId,
    execution_service: Arc<InMemoryExecutionService>,
    event_bus: Arc<InMemoryEventBus>,
    state: Arc<AppState>,
    app: Router,
    workspace: TempDir,
}

struct StartedServer {
    base_url: String,
    _task: JoinHandle<()>,
}

struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let original = std::env::var(key).ok();
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        unsafe {
            if let Some(value) = &self.original {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}

fn make_workspace(root: &Path) -> WorkspaceRef {
    WorkspaceRef {
        id: WorkspaceId::from_string(format!("ws-{}", uuid::Uuid::new_v4())).unwrap(),
        mode: WorkspaceMode::Isolated,
        materialization_mode: None,
        home_node_id: None,
        backing_store: None,
        root: root.to_string_lossy().to_string(),
        writable_roots: vec![root.to_string_lossy().to_string()],
    }
}

fn make_task() -> Task {
    Task {
        id: TaskId::new(),
        case_id: None,
        title: "remote assignment delivery test".to_string(),
        summary: "verify node-to-node assignment consumption".to_string(),
        kind: TaskKind::Automation,
        input: TaskInput {
            payload: serde_json::json!({"kind": "cluster-assignment-test"}),
        },
        requested_by: ActorRef {
            id: ActorId::from_string("test-user".to_string()).unwrap(),
            actor_type: ActorType::Human,
            tenant_id: None,
            home_node_id: None,
            display_name: "Test User".to_string(),
        },
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "cluster-assignment-test".to_string(),
        },
        desired_outputs: vec![],
        labels: vec!["test".to_string()],
        priority: cyberclaw_core::enums::Priority::Medium,
        preferred_agent_id: None,
    }
}

fn make_plan() -> ExecutionPlan {
    ExecutionPlan {
        resolution: Resolution {
            agent: AgentId::from_string("cyberclaw/test-agent".to_string()).unwrap(),
            skills: vec![],
            workflow: None,
            connectors: vec![ConnectorId::from_string("local".to_string()).unwrap()],
            capabilities: vec![cyberclaw_core::ids::CapabilityId::from_string(
                "host.todo.write".to_string(),
            )
            .unwrap()],
            reasons: vec!["cross-node assignment delivery test".to_string()],
        },
        actions: vec![PlannedAction {
            connector_id: ConnectorId::from_string("local".to_string()).unwrap(),
            capability: cyberclaw_core::ids::CapabilityId::from_string(
                "host.todo.write".to_string(),
            )
            .unwrap(),
            input: serde_json::json!({
                "content": "delivered from node-a to node-b"
            }),
            reason: "exercise connector/capability execution on remote node".to_string(),
        }],
        review_required: false,
        max_fix_loops: cyberclaw_control_plane::default_max_fix_loops(),
        expected_outcomes: vec![],
    }
}

fn build_node(node_name: &str, jwt_secret: &str) -> Result<RunningNode> {
    let connector_registry = Arc::new(ConnectorRegistry::new());
    let workspace = tempfile::tempdir()?;
    let workspace_root = workspace.path().to_path_buf();
    connector_registry.register(Arc::new(LocalConnector::new(workspace_root.clone())))?;

    let dispatcher = Arc::new(CapabilityDispatcher::new(connector_registry.clone()));
    let event_recorder = Arc::new(InMemoryEventRecorder::new());
    let execution_service = Arc::new(
        InMemoryExecutionService::with_event_recorder(event_recorder.clone())
            .with_capability_dispatcher(dispatcher),
    );

    let policy_engine = Arc::new(DefaultPolicyEngine::default());
    let task_manager = Arc::new(InMemoryTaskManager::new());
    let review_queue = Arc::new(InMemoryReviewQueue::new(None));
    let registry = Arc::new(InMemoryRegistry::new());
    let resolver = Arc::new(InMemoryResolver::new(registry.clone()));
    let gateway = Arc::new(InMemoryGatewayRouter::new());
    let placement_engine = Arc::new(InMemoryPlacementEngine);
    let lease_manager = Arc::new(InMemoryLeaseManager::default());
    let membership_service = Arc::new(InMemoryMembershipService::default());
    let event_bus = Arc::new(InMemoryEventBus::default());
    let security_event_store = Arc::new(InMemorySecurityEventStore::new());
    let plugin_registry = Arc::new(PluginRegistry::new(std::path::PathBuf::from(
        "/tmp/test-plugins",
    )));
    let node_id = NodeId::from_string(node_name.to_string()).unwrap();

    membership_service.join(NodeRecord {
        id: node_id.clone(),
        role: NodeRole::Hybrid,
        labels: vec!["worker".to_string()],
        region: None,
        zone: None,
        health: NodeHealth::Healthy,
        membership_state: MembershipState::Active,
        capacity: NodeCapacity::default(),
        current_executions: 0,
        last_heartbeat_at: chrono::Utc::now(),
    })?;
    membership_service.heartbeat(&node_id)?;

    let control_plane = Arc::new(
        ControlPlaneOrchestrator::new(
            gateway,
            resolver,
            registry,
            review_queue.clone(),
            task_manager.clone(),
            execution_service.clone(),
            placement_engine,
            lease_manager,
            membership_service,
            event_bus.clone(),
            policy_engine.clone(),
            plugin_registry,
            security_event_store,
        )
        .with_local_node_id(node_id.clone())
        .with_event_recorder(event_recorder.clone()),
    );

    let state = Arc::new(AppState::new(
        Arc::new(common::MockLlmClient::new()),
        connector_registry,
        policy_engine,
        event_recorder,
        jwt_secret.to_string(),
        ControlPlaneComponents {
            control_plane,
            task_manager,
            execution_service: execution_service.clone(),
            review_queue,
        },
    ));

    Ok(RunningNode {
        node_id,
        execution_service,
        event_bus,
        state: state.clone(),
        app: create_router_with_config(state, ServerConfig::default()),
        workspace,
    })
}

async fn spawn_server(app: Router) -> Result<StartedServer> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("test server should run");
    });
    Ok(StartedServer {
        base_url: format!("http://{}", addr),
        _task: task,
    })
}

async fn seed_remote_assignment(
    execution_service: Arc<InMemoryExecutionService>,
    workspace_root: PathBuf,
    target_node_id: NodeId,
) -> Result<(ExecutionId, LeaseId)> {
    let execution_id = ExecutionId::new();
    let lease_id = LeaseId::new();
    let task = make_task();
    let plan = make_plan();
    execution_service
        .submit(ExecutionRequest {
            execution_id: execution_id.clone(),
            task,
            case: None,
            context: ControlPlaneContext {
                actor: ActorRef {
                    id: ActorId::from_string("system".to_string()).unwrap(),
                    actor_type: ActorType::System,
                    tenant_id: None,
                    home_node_id: None,
                    display_name: "System".to_string(),
                },
                session: None,
                workspace: Some(make_workspace(&workspace_root)),
            },
            agent: Some(cyberclaw_core::execution::AgentRef {
                id: AgentId::from_string("cyberclaw/test-agent".to_string()).unwrap(),
                role: "test-agent".to_string(),
            }),
            trace_id: Some(TraceId::new()),
            execution_mode: None,
            plan: Some(plan),
        })
        .await?;
    execution_service
        .set_assignment(&execution_id, target_node_id, lease_id.clone())
        .await?;
    Ok((execution_id, lease_id))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_remote_assignment_delivery_consumption_and_status_transition() -> Result<()> {
    let jwt_secret = "cluster-assignment-test-secret";
    let cluster_token = "cluster-assignment-shared-token";
    let _cluster_token_guard = EnvVarGuard::set("CYBERCLAW_CLUSTER_SHARED_TOKEN", cluster_token);
    let node_a = build_node("node-a", jwt_secret)?;
    let node_b = build_node("node-b", jwt_secret)?;

    let server_a = spawn_server(node_a.app.clone()).await?;

    let collector_handle = spawn_assignment_event_collector(
        node_a.event_bus.clone(),
        node_a.execution_service.clone(),
        node_a.state.assignment_queue.clone(),
        node_a.node_id.clone(),
    )
    .expect("assignment collector should subscribe");
    let (execution_id, lease_id) = seed_remote_assignment(
        node_a.execution_service.clone(),
        node_a.workspace.path().to_path_buf(),
        node_b.node_id.clone(),
    )
    .await?;

    node_a.event_bus.publish(ClusterEvent::ExecutionAssigned {
        execution_id: execution_id.clone(),
        node_id: node_b.node_id.clone(),
        lease_id,
        timestamp: chrono::Utc::now(),
    })?;

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if node_a.state.assignment_queue.len(&node_b.node_id).await > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("assignment collector did not enqueue remote assignment"))?;

    let probe_client = reqwest::Client::builder().no_proxy().build()?;
    let probe_response = probe_client
        .post(format!(
            "{}/internal/cluster/assignments/pull",
            server_a.base_url
        ))
        .header(CLUSTER_TOKEN_HEADER, cluster_token)
        .json(&PullAssignmentsRequest {
            node_id: node_b.node_id.as_str().to_string(),
            limit: Some(1),
        })
        .send()
        .await?;
    let probe_status = probe_response.status();
    let probe_body = probe_response.text().await?;
    if !probe_status.is_success() {
        return Err(anyhow::anyhow!(
            "probe pull failed with status {} and body {}",
            probe_status,
            probe_body
        ));
    }
    let probe_payload = serde_json::from_str::<PullAssignmentsResponse>(&probe_body)?;
    assert_eq!(
        probe_payload.assignments.len(),
        1,
        "probe pull should return queued assignment"
    );
    for assignment in probe_payload.assignments {
        let scheduled_node_id = assignment.scheduled_node_id()?;
        node_a
            .state
            .assignment_queue
            .enqueue(&scheduled_node_id, assignment)
            .await;
    }

    let pull_worker_handle = spawn_assignment_pull_worker(
        node_b.execution_service.clone(),
        AssignmentPullWorkerConfig {
            coordinator_url: server_a.base_url.clone(),
            local_node_id: node_b.node_id.clone(),
            poll_interval: Duration::from_millis(50),
            request_timeout: Duration::from_secs(2),
            shared_token: Some(cluster_token.to_string()),
        },
    );

    let completion_result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let source = node_a
                .execution_service
                .get(&execution_id)
                .await
                .expect("source execution lookup")
                .expect("source execution should exist");
            let remote = node_b
                .execution_service
                .get(&execution_id)
                .await
                .expect("remote execution lookup");

            if source.status == ExecutionStatus::Completed
                && remote
                    .as_ref()
                    .map(|execution| execution.status == ExecutionStatus::Completed)
                    .unwrap_or(false)
            {
                break;
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;

    if completion_result.is_err() {
        let source_status = node_a
            .execution_service
            .get(&execution_id)
            .await?
            .map(|execution| format!("{:?}", execution.status))
            .unwrap_or_else(|| "missing".to_string());
        let remote_status = node_b
            .execution_service
            .get(&execution_id)
            .await?
            .map(|execution| format!("{:?}", execution.status))
            .unwrap_or_else(|| "missing".to_string());
        let queue_len = node_a.state.assignment_queue.len(&node_b.node_id).await;
        return Err(anyhow::anyhow!(
            "remote assignment did not reach Completed on both nodes (source_status={}, remote_status={}, queue_len={})",
            source_status,
            remote_status,
            queue_len
        ));
    }

    let todo_file = node_b.workspace.path().join(".cyberclaw").join("TODO.md");
    let todo_contents = tokio::fs::read_to_string(&todo_file).await?;
    assert!(
        todo_contents.contains("delivered from node-a to node-b"),
        "remote node should execute connector/capability action"
    );

    pull_worker_handle.abort();
    collector_handle.abort();

    Ok(())
}
