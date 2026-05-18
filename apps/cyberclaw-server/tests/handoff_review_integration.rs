//! Sprint 24 T4 — End-to-end integration test for the auto-review path.
//!
//! Scope: `HandoffConnector::execute` (with `DefaultPolicyEngine`) automatically
//! enqueues both a `HandoffRequest` and a `ReviewRequest::for_handoff` when the
//! policy decision is `ReviewRequired`. The test then HTTP-approves the review
//! through the full axum Router and verifies `HandoffCompletionSink::finalize_accept`
//! is called.
//!
//! # What changed from S22 T10
//!
//! S22 T10 manually created `ReviewRequest::for_handoff(...)` and called
//! `ReviewQueue::enqueue` directly. S24 proves the **connector** creates the
//! review automatically when `PolicyEngine` returns `ReviewRequired`.
//!
//! # Why we call the connector directly (not `/_dev/trigger_handoff`)
//!
//! `/_dev/trigger_handoff` directly enqueues to `handoff_queue`, bypassing
//! `HandoffConnector` and `PolicyEngine`. To exercise the auto-review path we
//! must call `HandoffConnector::execute()` directly — the same path the real
//! gateway takes.
//!
//! # Self-approval guard
//!
//! The `HandoffConnector` sets `requested_by` to the `actor` from the
//! `CapabilityExecutionRequest`. We use a different actor id than the JWT
//! subject to satisfy the orchestrator's self-approval guard (H-3 security fix).

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use cyberclaw_connectors::{
    handoff::{HandoffConnector, HandoffSink, ReviewQueueSink},
    registry::ConnectorRegistry,
    types::{CapabilityExecutionRequest, ExecutionStatus},
    Connector,
};
use cyberclaw_control_plane::{
    event_bus::InMemoryEventBus,
    execution_service::InMemoryExecutionService,
    gateway_router::InMemoryGatewayRouter,
    handoff_completion_sink::HandoffCompletionSink,
    handoff_queue::{HandoffQueue, InMemoryHandoffQueue},
    lease_manager::InMemoryLeaseManager,
    membership_service::InMemoryMembershipService,
    orchestrator::ControlPlaneOrchestrator,
    placement_engine::InMemoryPlacementEngine,
    registry::InMemoryRegistry,
    resolver::InMemoryResolver,
    review_queue::{InMemoryReviewQueue, ReviewQueue},
    task_manager::InMemoryTaskManager,
};
use cyberclaw_core::{
    handoff::{HandoffError, HandoffRequest},
    identity::{ActorRef, ActorType},
    ids::{
        ActorId, CapabilityId, ConnectorId, ExecutionId, HandoffId, TraceId, UserId, WorkspaceId,
    },
    review::ReviewTarget,
    users::{OperatorRecord, UsersConfig},
    workspace::WorkspaceMode,
};
use cyberclaw_governance::{
    engine::DefaultPolicyEngine, tool_output_sanitizer::ToolOutputSanitizer,
};
use cyberclaw_llm::prelude::GenericOpenAiClient;
use cyberclaw_llm::LlmClient;
use cyberclaw_observability::{
    events::InMemoryEventRecorder, security_event_store::InMemorySecurityEventStore,
};
use cyberclaw_server::{create_router_with_config, AppState, ControlPlaneComponents, ServerConfig};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serial_test::serial;
use std::sync::{Arc, Mutex};
use tower::ServiceExt; // for `oneshot`

// ---------------------------------------------------------------------------
// MockHandoffCompletionSink
// ---------------------------------------------------------------------------

/// Records every `finalize_accept` call for assertion in tests.
struct MockHandoffCompletionSink {
    captures: Mutex<Vec<HandoffId>>,
}

impl MockHandoffCompletionSink {
    fn new() -> Self {
        Self {
            captures: Mutex::new(Vec::new()),
        }
    }

    fn len(&self) -> usize {
        self.captures.lock().unwrap().len()
    }

    fn first(&self) -> Option<HandoffId> {
        self.captures.lock().unwrap().first().cloned()
    }
}

impl std::fmt::Debug for MockHandoffCompletionSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockHandoffCompletionSink").finish()
    }
}

#[async_trait]
impl HandoffCompletionSink for MockHandoffCompletionSink {
    async fn finalize_accept(&self, handoff_id: &HandoffId) -> anyhow::Result<()> {
        self.captures.lock().unwrap().push(handoff_id.clone());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ReviewQueueSinkAdapter — same logic as state.rs, local copy for test
// ---------------------------------------------------------------------------

struct ReviewQueueSinkAdapter(Arc<InMemoryReviewQueue>);

impl std::fmt::Debug for ReviewQueueSinkAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReviewQueueSinkAdapter")
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl ReviewQueueSink for ReviewQueueSinkAdapter {
    async fn enqueue_review_for_handoff(
        &self,
        review_id: cyberclaw_core::ids::ReviewId,
        handoff_id: HandoffId,
        title: String,
        summary: String,
        requested_by: ActorRef,
    ) -> anyhow::Result<()> {
        let req = cyberclaw_core::review::ReviewRequest::for_handoff(
            review_id,
            handoff_id,
            None,
            title,
            summary,
            requested_by,
            cyberclaw_core::review::ReviewKind::HumanReview,
            TraceId::new(),
            chrono::Utc::now(),
        );
        self.0
            .enqueue(req)
            .await
            .map_err(|e| anyhow::anyhow!("review enqueue failed: {}", e))
    }
}

// ---------------------------------------------------------------------------
// QueueSink — bridge HandoffSink → HandoffQueue (local copy for test)
// ---------------------------------------------------------------------------

struct QueueSink(Arc<dyn HandoffQueue>);

impl std::fmt::Debug for QueueSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueueSink").finish_non_exhaustive()
    }
}

#[async_trait]
impl HandoffSink for QueueSink {
    async fn enqueue(&self, req: HandoffRequest) -> Result<(), HandoffError> {
        self.0.enqueue(req).await
    }
}

// ---------------------------------------------------------------------------
// JWT helpers
// ---------------------------------------------------------------------------

const TEST_JWT_SECRET: &str = "test-review-jwt-secret";

#[derive(Serialize, Deserialize)]
struct TestClaims {
    sub: String,
    iat: i64,
    exp: i64,
}

fn jwt_for_user(user_id: &str) -> String {
    let now = chrono::Utc::now().timestamp();
    let claims = TestClaims {
        sub: user_id.to_string(),
        iat: now,
        exp: now + 3600,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(TEST_JWT_SECRET.as_bytes()),
    )
    .expect("jwt encode")
}

// ---------------------------------------------------------------------------
// RAII HOME guard (same pattern as handoff_http_integration.rs)
// ---------------------------------------------------------------------------

struct RestoreHome {
    original: Option<std::ffi::OsString>,
}

impl RestoreHome {
    fn set(new_home: &std::path::Path) -> Self {
        let original = std::env::var_os("HOME");
        // SAFETY: tests tagged #[serial] serialize access to $HOME.
        unsafe {
            std::env::set_var("HOME", new_home);
        }
        Self { original }
    }
}

impl Drop for RestoreHome {
    fn drop(&mut self) {
        // SAFETY: see set().
        unsafe {
            match self.original.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}

/// Seed a temp users.toml with one admin operator and redirect $HOME to it.
fn seed_admin(user_id: &str) -> (tempfile::TempDir, RestoreHome) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let restore = RestoreHome::set(tmp.path());
    let uid = UserId::from_string(user_id.to_string()).expect("valid user id");
    let record = OperatorRecord {
        user_id: uid,
        display_name: format!("Admin {}", user_id),
        created_at: "2026-04-24T00:00:00Z".to_string(),
        last_login: None,
        role: "admin".to_string(),
        onboarded_at: None,
        intent_auto_route: false,
    };
    let mut cfg = UsersConfig::default();
    cfg.upsert(record);
    let path = UsersConfig::default_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    cfg.save_to_path(&path).expect("save users.toml");
    (tmp, restore)
}

// ---------------------------------------------------------------------------
// AppState builder with MockHandoffCompletionSink
// ---------------------------------------------------------------------------

/// Build AppState wired with the given `mock_sink`, `handoff_queue`, and `review_queue`.
///
/// The orchestrator is constructed here (not via `AppState::new`) so that
/// `.with_handoff_completion_sink()` and `.with_handoff_queue()` can be
/// applied before wrapping in `Arc`.
///
/// `review_queue` is shared so the test can inspect auto-created review entries.
fn build_state_with_sink(
    mock_sink: Arc<MockHandoffCompletionSink>,
    handoff_queue: Arc<InMemoryHandoffQueue>,
    review_queue: Arc<InMemoryReviewQueue>,
) -> Arc<AppState> {
    let llm: Arc<dyn LlmClient> =
        Arc::new(GenericOpenAiClient::new(None, "http://localhost:8080").expect("llm client"));
    let connector_registry = Arc::new(ConnectorRegistry::new());
    let policy_engine = Arc::new(DefaultPolicyEngine::default());
    let event_recorder = Arc::new(InMemoryEventRecorder::new());
    let task_manager = Arc::new(InMemoryTaskManager::new());
    let execution_service = Arc::new(InMemoryExecutionService::with_event_recorder(
        event_recorder.clone(),
    ));
    let registry = Arc::new(InMemoryRegistry::new());
    let resolver = Arc::new(InMemoryResolver::new(registry.clone()));
    let gateway = Arc::new(InMemoryGatewayRouter::new());
    let placement_engine = Arc::new(InMemoryPlacementEngine);
    let lease_manager = Arc::new(InMemoryLeaseManager::default());
    let membership_service = Arc::new(InMemoryMembershipService::default());
    let event_bus = Arc::new(InMemoryEventBus::default());
    let security_event_store = Arc::new(InMemorySecurityEventStore::new());
    let plugin_path = std::env::temp_dir().join(format!(
        "cyberclaw-t24-plugins-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let plugin_registry = Arc::new(cyberclaw_plugin_runtime::PluginRegistry::new(plugin_path));

    let sink_arc: Arc<dyn HandoffCompletionSink> = mock_sink;
    let hq_arc: Arc<dyn HandoffQueue> = handoff_queue;

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
            event_bus,
            policy_engine.clone(),
            plugin_registry,
            security_event_store,
        )
        .with_event_recorder(event_recorder.clone())
        .with_handoff_completion_sink(sink_arc)
        .with_handoff_queue(hq_arc),
    );

    let cp = ControlPlaneComponents {
        control_plane,
        task_manager,
        execution_service,
        review_queue,
    };
    let mut state = AppState::new(
        llm,
        connector_registry,
        policy_engine,
        event_recorder,
        TEST_JWT_SECRET.to_string(),
        cp,
    );
    state.handoff_feature_enabled = true;
    Arc::new(state)
}

// ---------------------------------------------------------------------------
// HTTP helper
// ---------------------------------------------------------------------------

async fn post_json(
    router: &axum::Router,
    uri: &str,
    token: &str,
    body: &str,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    let value: Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, value)
}

// ---------------------------------------------------------------------------
// T4 — Auto-review path: connector auto-creates review via PolicyEngine
// ---------------------------------------------------------------------------

/// Sprint 24 T4: `HandoffConnector` with `DefaultPolicyEngine` automatically
/// creates a `ReviewRequest::for_handoff` when executing `agent.handoff`
/// (High risk → ReviewRequired). The review is then HTTP-approved through
/// the axum Router and `HandoffCompletionSink::finalize_accept` is invoked.
///
/// # Flow
///
/// 1. Build shared queues + MockHandoffCompletionSink.
/// 2. Build a `HandoffConnector` wired with `DefaultPolicyEngine` (returns
///    `ReviewRequired` for High-risk capabilities) and `ReviewQueueSinkAdapter`
///    pointing at the shared review queue.
/// 3. Call `connector.execute(...)` directly — mirrors what the real gateway does.
/// 4. Assert result: `status = "pending_review"`, `review_id` present, `handoff_id` present.
/// 5. Assert `review_queue.list_pending()` contains exactly 1 entry with
///    `target: ReviewTarget::Handoff { handoff_id }`.
/// 6. HTTP POST `/api/v1/reviews/:review_id/approve` as admin.
/// 7. Assert `MockHandoffCompletionSink::finalize_accept` was called once with correct id.
///
/// # Why the connector is called directly
///
/// `/_dev/trigger_handoff` directly enqueues to `handoff_queue`, bypassing
/// `HandoffConnector` and `PolicyEngine`. To prove the auto-review path we
/// must call the connector — which is the production dispatch path.
///
/// # Self-approval guard
///
/// The connector's `actor.id` ("connector-actor-t24") differs from the JWT
/// subject ("t24-admin") to satisfy the orchestrator's self-approval guard.
#[tokio::test]
#[serial]
async fn handoff_dispatch_with_review_required_policy_creates_review_automatically() {
    // 1. Seed admin user and redirect $HOME so `require_admin` passes.
    let (_tmp, _restore) = seed_admin("t24-admin");

    // 2. Shared queues.
    let handoff_queue = Arc::new(InMemoryHandoffQueue::new());
    let review_queue = Arc::new(InMemoryReviewQueue::new(None));

    // 3. MockHandoffCompletionSink.
    let mock_sink = Arc::new(MockHandoffCompletionSink::new());
    let mock_sink_ref = mock_sink.clone();

    // 4. Build AppState + axum Router (orchestrator wired with mock_sink).
    let state = build_state_with_sink(mock_sink, handoff_queue.clone(), review_queue.clone());
    let router = create_router_with_config(state.clone(), ServerConfig::default());
    let token = jwt_for_user("t24-admin");

    // 5. Build a HandoffConnector with DefaultPolicyEngine (returns ReviewRequired
    //    for High-risk agent.handoff) and ReviewQueueSinkAdapter pointing at the
    //    shared review_queue.
    let connector_id = ConnectorId::from_string("handoff".to_string()).unwrap();
    let handoff_sink: Arc<dyn HandoffSink> = Arc::new(QueueSink(handoff_queue.clone()));
    let review_sink: Arc<dyn ReviewQueueSink> =
        Arc::new(ReviewQueueSinkAdapter(review_queue.clone()));

    let connector = HandoffConnector::new(
        connector_id,
        handoff_sink,
        Arc::new(ToolOutputSanitizer::new()),
        Arc::new(DefaultPolicyEngine::default()), // High risk → ReviewRequired
        review_sink,
    );

    // 6. Execute agent.handoff through the connector.
    //    actor.id ("connector-actor-t24") differs from JWT subject ("t24-admin")
    //    so the self-approval guard in the orchestrator passes.
    let exec_request = CapabilityExecutionRequest {
        execution_id: ExecutionId::from_string("exec_t24_auto_review".to_string()).unwrap(),
        trace_id: "trace_t24_auto_review".to_string(),
        actor: ActorRef {
            id: ActorId::from_string("connector-actor-t24".to_string()).unwrap(),
            actor_type: ActorType::Agent,
            tenant_id: None,
            home_node_id: None,
            display_name: "Connector Actor T24".to_string(),
        },
        workspace: cyberclaw_core::prelude::WorkspaceRef {
            id: WorkspaceId::from_string("ws_t24".to_string()).unwrap(),
            mode: WorkspaceMode::Ephemeral,
            materialization_mode: None,
            home_node_id: None,
            backing_store: None,
            root: "/tmp/t24-test".to_string(),
            writable_roots: vec![],
        },
        connector_id: ConnectorId::from_string("handoff".to_string()).unwrap(),
        capability_id: CapabilityId::from_string("agent.handoff".to_string()).unwrap(),
        input: serde_json::json!({
            "from_agent_id": "agent-sender-t24",
            "to_agent_id":   "agent-receiver-t24",
            "conversation_id": "conv-t24-auto-review",
            "reason": "T24 auto-review integration test",
            "briefing_text": "Passing the baton for automated review test.",
            "context_artifacts": []
        }),
    };

    let result = connector
        .execute(exec_request)
        .await
        .expect("execute should succeed");

    // 7. Assert connector output: status="pending_review", review_id present.
    assert_eq!(
        result.status,
        ExecutionStatus::Success,
        "connector execute should succeed; error: {:?}",
        result.error
    );
    assert_eq!(
        result.output["status"].as_str().unwrap_or(""),
        "pending_review",
        "DefaultPolicyEngine should return ReviewRequired for High-risk handoff; got: {}",
        result.output
    );
    let review_id_str = result.output["review_id"]
        .as_str()
        .expect("review_id field must be present in pending_review response");
    assert!(!review_id_str.is_empty(), "review_id must be non-empty");
    let handoff_id_str = result.output["handoff_id"]
        .as_str()
        .expect("handoff_id field must be present");
    assert!(
        handoff_id_str.starts_with("ho_"),
        "handoff_id should start with 'ho_', got: {handoff_id_str}"
    );

    // 8. Assert ReviewQueue contains exactly 1 entry with correct target.
    let pending = review_queue
        .list_pending()
        .await
        .expect("list_pending should succeed");
    assert_eq!(
        pending.len(),
        1,
        "review_queue should contain exactly 1 pending review after connector execute"
    );
    let review = &pending[0];
    assert_eq!(
        review.id.to_string(),
        review_id_str,
        "review.id must match output review_id"
    );
    assert!(
        matches!(review.target, ReviewTarget::Handoff { .. }),
        "review target must be ReviewTarget::Handoff, got: {:?}",
        review.target
    );
    let target_handoff_id = review.target.handoff_id().expect("target has handoff_id");
    assert_eq!(
        target_handoff_id.to_string(),
        handoff_id_str,
        "review target handoff_id must match connector output handoff_id"
    );

    // 9. HTTP POST /api/v1/reviews/:id/approve as admin.
    let approve_uri = format!("/api/v1/reviews/{}/approve", review_id_str);
    let (status, body) = post_json(&router, &approve_uri, &token, r#"{"reason":""}"#).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "approve should return 200, got {}: {}",
        status,
        body
    );
    assert_eq!(
        body["status"].as_str().unwrap_or(""),
        "handoff_approved",
        "response status must be 'handoff_approved', got: {}",
        body
    );
    assert_eq!(
        body["target"]["type"].as_str().unwrap_or(""),
        "handoff",
        "target.type must be 'handoff', got: {}",
        body
    );
    assert_eq!(
        body["target"]["handoff_id"].as_str().unwrap_or(""),
        handoff_id_str,
        "target.handoff_id must match, got: {}",
        body
    );

    // 10. Assert MockHandoffCompletionSink was called exactly once with the correct id.
    assert_eq!(
        mock_sink_ref.len(),
        1,
        "finalize_accept should be called exactly once"
    );
    assert_eq!(
        mock_sink_ref.first().as_ref().map(|id| id.to_string()),
        Some(handoff_id_str.to_string()),
        "finalize_accept must be called with the connector-generated handoff_id"
    );
}
