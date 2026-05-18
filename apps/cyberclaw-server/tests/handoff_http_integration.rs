//! Sprint 21 T18 — Integration tests exercising the full axum Router.
//!
//! Unlike the `#[cfg(test)]` tests in `api::chat_handoff::tests`, these
//! go through `create_router_with_config()` + JWT middleware + request
//! deserialization, verifying the router is actually wired and reachable.
//!
//! Scenarios:
//! 1. Happy path — trigger, accept, verify Accepted + card appended
//! 2. Reject — trigger, reject with reason, verify Declined
//! 3. Two-hop handoff chain (A→B→C)

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use cyberclaw_connectors::registry::ConnectorRegistry;
use cyberclaw_control_plane::{
    event_bus::InMemoryEventBus, execution_service::InMemoryExecutionService,
    gateway_router::InMemoryGatewayRouter, lease_manager::InMemoryLeaseManager,
    membership_service::InMemoryMembershipService, orchestrator::ControlPlaneOrchestrator,
    placement_engine::InMemoryPlacementEngine, registry::InMemoryRegistry,
    resolver::InMemoryResolver, review_queue::InMemoryReviewQueue,
    task_manager::InMemoryTaskManager,
};
use cyberclaw_core::{
    ids::UserId,
    users::{OperatorRecord, UsersConfig},
};
use cyberclaw_governance::engine::DefaultPolicyEngine;
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
use std::sync::Arc;
use tower::ServiceExt; // for `oneshot`

// ---------------------------------------------------------------------------
// JWT helpers
// ---------------------------------------------------------------------------

const TEST_JWT_SECRET: &str = "test-api-jwt-secret";

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
// RAII HOME guard
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
/// Returns (TempDir, RestoreHome) — caller must keep both alive.
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
// AppState builder
// ---------------------------------------------------------------------------

/// Build a base AppState with handoff_feature_enabled = true.
/// Does not depend on #[cfg(test)]-gated api::test_helpers.
fn build_state() -> Arc<AppState> {
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
    let review_queue = Arc::new(InMemoryReviewQueue::new(None));
    let gateway = Arc::new(InMemoryGatewayRouter::new());
    let placement_engine = Arc::new(InMemoryPlacementEngine);
    let lease_manager = Arc::new(InMemoryLeaseManager::default());
    let membership_service = Arc::new(InMemoryMembershipService::default());
    let event_bus = Arc::new(InMemoryEventBus::default());
    let security_event_store = Arc::new(InMemorySecurityEventStore::new());
    let plugin_path = std::env::temp_dir().join(format!(
        "cyberclaw-t18-plugins-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let plugin_registry = Arc::new(cyberclaw_plugin_runtime::PluginRegistry::new(plugin_path));
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
        .with_event_recorder(event_recorder.clone()),
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

/// Build the full axum Router (JWT middleware included).
fn build_router(state: Arc<AppState>) -> axum::Router {
    create_router_with_config(state, ServerConfig::default())
}

// ---------------------------------------------------------------------------
// HTTP helpers
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

/// Create a conversation via `POST /api/v1/chat/conversations` and return its id.
async fn create_conversation(router: &axum::Router, token: &str, title: &str) -> String {
    let body = format!(r#"{{"title":"{}"}}"#, title);
    let (status, v) = post_json(router, "/api/v1/chat/conversations", token, &body).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "create_conversation failed: {}: {}",
        status,
        v
    );
    v["id"]
        .as_str()
        .expect("id in create_conversation response")
        .to_string()
}

/// Trigger a handoff via `POST /_dev/trigger_handoff` and return `handoff_id`.
async fn trigger_handoff(
    router: &axum::Router,
    token: &str,
    from: &str,
    to: &str,
    conv_id: &str,
) -> String {
    let body = format!(
        r#"{{"from_agent_id":"{}","to_agent_id":"{}","conversation_id":"{}","reason":"t18 integration test","briefing_text":"passing the baton"}}"#,
        from, to, conv_id
    );
    let (status, v) = post_json(router, "/_dev/trigger_handoff", token, &body).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "trigger_handoff failed: {}: {}",
        status,
        v
    );
    v["handoff_id"]
        .as_str()
        .expect("handoff_id in trigger response")
        .to_string()
}

// ---------------------------------------------------------------------------
// Test 1 — Happy path
// ---------------------------------------------------------------------------

/// T18 Scenario 1: trigger → accept → 200 Accepted, card appended.
///
/// What this tests that the pre-existing `#[cfg(test)] mod tests` DID NOT:
/// - The router is actually wired in `lib.rs` `protected_routes`.
/// - The JWT middleware layer (`jwt_auth`) correctly extracts claims from the
///   `Authorization: Bearer` header before dispatch.
/// - Axum correctly deserializes path params (`:id/accept`) through the router.
/// - The `/_dev/trigger_handoff` route is registered (debug_assertions).
#[tokio::test]
#[serial]
async fn handoff_happy_path_through_router() {
    let (_tmp, _restore) = seed_admin("t18-admin-happy");

    let state = build_state();
    let router = build_router(state.clone());
    let token = jwt_for_user("t18-admin-happy");

    // Create a real conversation through the router.
    let conv_id = create_conversation(&router, &token, "T18 Happy Path Conv").await;

    // Step 1: trigger handoff.
    let handoff_id = trigger_handoff(&router, &token, "agent-alpha", "agent-beta", &conv_id).await;
    assert!(
        handoff_id.starts_with("ho_"),
        "handoff_id should start with 'ho_', got: {}",
        handoff_id
    );

    // Step 2: accept handoff through the router.
    let accept_uri = format!("/api/v1/chat/handoff/{}/accept", handoff_id);
    let (status, v) = post_json(&router, &accept_uri, &token, "{}").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "accept should return 200, got {}: {}",
        status,
        v
    );
    assert_eq!(
        v["status"].as_str().unwrap_or(""),
        "accepted",
        "response status should be 'accepted'"
    );
    assert_eq!(
        v["handoff_id"].as_str().unwrap_or(""),
        handoff_id,
        "handoff_id echoed in response"
    );

    // Step 3: verify via queue that entry is Accepted.
    let hid = cyberclaw_core::ids::HandoffId::from_string(handoff_id.clone()).unwrap();
    let stored = state
        .handoff_queue
        .get(&hid)
        .await
        .expect("handoff still in queue");
    assert_eq!(
        stored.status,
        cyberclaw_core::handoff::HandoffStatus::Accepted,
        "queue entry must be Accepted"
    );
    assert_eq!(
        stored.to_agent_id.to_string(),
        "agent-beta",
        "to_agent_id preserved"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — Reject
// ---------------------------------------------------------------------------

/// T18 Scenario 3: trigger → reject with reason → 200 declined.
///
/// What this tests that the pre-existing unit tests DID NOT:
/// - JSON body deserialization of `RejectHandoffBody` through the full axum
///   extractor chain (Content-Type + JSON body → router → handler).
/// - The reject route (`/:id/reject`) resolves through the router + JWT layer.
#[tokio::test]
#[serial]
async fn handoff_reject_through_router() {
    let (_tmp, _restore) = seed_admin("t18-admin-reject");

    let state = build_state();
    let router = build_router(state.clone());
    let token = jwt_for_user("t18-admin-reject");

    // Trigger (no real conversation needed — reject doesn't touch conv store).
    let handoff_id = trigger_handoff(
        &router,
        &token,
        "agent-rej-from",
        "agent-rej-to",
        "conv-t18-reject-placeholder",
    )
    .await;

    // Reject with reason.
    let reject_uri = format!("/api/v1/chat/handoff/{}/reject", handoff_id);
    let reject_body = r#"{"reason":"not the right specialist"}"#;
    let (status, v) = post_json(&router, &reject_uri, &token, reject_body).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "reject should return 200, got {}: {}",
        status,
        v
    );
    assert_eq!(
        v["status"].as_str().unwrap_or(""),
        "declined",
        "response status should be 'declined'"
    );
    assert_eq!(
        v["handoff_id"].as_str().unwrap_or(""),
        handoff_id,
        "handoff_id echoed in response"
    );

    // Verify queue entry is Declined.
    let hid = cyberclaw_core::ids::HandoffId::from_string(handoff_id.clone()).unwrap();
    let stored = state
        .handoff_queue
        .get(&hid)
        .await
        .expect("handoff still in queue");
    assert_eq!(
        stored.status,
        cyberclaw_core::handoff::HandoffStatus::Declined,
        "queue entry must be Declined"
    );
}

// ---------------------------------------------------------------------------
// Test 4 — Two-hop handoff chain (A→B→C)
// ---------------------------------------------------------------------------

/// A4 chain test: trigger A→B, accept, verify active=B, then trigger B→C on the
/// same conversation, accept again, verify active=C and two handoff cards exist.
///
/// TODO(s22-v2): enforce max-chain-depth in production to prevent infinite loops.
///
/// Self-handoff-in-chain note: this test supplies explicit `from_agent_id` values
/// in each trigger body, so the `HandoffError::SelfHandoff` guard (which fires when
/// `from == to`) is only exercised if we accidentally set both to the same id. That
/// guard path is not tested here; see the unit test `enqueue_invalid_request_returns_
/// validation_error` in `handoff_queue.rs` for coverage of that case.
#[tokio::test]
#[serial]
async fn handoff_chain_two_hops_through_router() {
    let (_tmp, _restore) = seed_admin("t18-admin-chain");

    let state = build_state();
    let router = build_router(state.clone());
    let token = jwt_for_user("t18-admin-chain");

    // Create one conversation that both hops will share.
    // UUID-based IDs from create_conversation are non-colliding; no store reset needed.
    let conv_id = create_conversation(&router, &token, "T18 Chain Conv A-B-C").await;

    // ── Hop 1: A → B ─────────────────────────────────────────────────────────

    let hop1_id = trigger_handoff(&router, &token, "agent-alpha", "agent-beta", &conv_id).await;
    assert!(
        hop1_id.starts_with("ho_"),
        "hop1 handoff_id should start with 'ho_', got: {}",
        hop1_id
    );

    let accept1_uri = format!("/api/v1/chat/handoff/{}/accept", hop1_id);
    let (status1, v1) = post_json(&router, &accept1_uri, &token, "{}").await;
    assert_eq!(
        status1,
        StatusCode::OK,
        "hop1 accept should return 200, got {}: {}",
        status1,
        v1
    );
    assert_eq!(
        v1["status"].as_str().unwrap_or(""),
        "accepted",
        "hop1 response status should be 'accepted'"
    );

    // Verify active_agent_id == agent-beta after hop 1.
    let conv_after_hop1 = state
        .conversation_store()
        .get(&conv_id)
        .await
        .expect("conversation must exist after hop 1");
    assert_eq!(
        conv_after_hop1
            .active_agent_id
            .as_ref()
            .map(|a| a.to_string()),
        Some("agent-beta".to_string()),
        "active_agent_id must be agent-beta after hop 1"
    );

    // Exactly 1 handoff card so far.
    assert_eq!(
        conv_after_hop1.messages.len(),
        1,
        "exactly one handoff card after hop 1"
    );
    assert_eq!(
        conv_after_hop1.messages[0].role, "handoff",
        "message role must be 'handoff'"
    );

    // ── Hop 2: B → C ─────────────────────────────────────────────────────────

    let hop2_id = trigger_handoff(&router, &token, "agent-beta", "agent-gamma", &conv_id).await;
    assert!(
        hop2_id.starts_with("ho_"),
        "hop2 handoff_id should start with 'ho_', got: {}",
        hop2_id
    );
    assert_ne!(
        hop1_id, hop2_id,
        "each hop must receive a distinct handoff_id"
    );

    let accept2_uri = format!("/api/v1/chat/handoff/{}/accept", hop2_id);
    let (status2, v2) = post_json(&router, &accept2_uri, &token, "{}").await;
    assert_eq!(
        status2,
        StatusCode::OK,
        "hop2 accept should return 200, got {}: {}",
        status2,
        v2
    );
    assert_eq!(
        v2["status"].as_str().unwrap_or(""),
        "accepted",
        "hop2 response status should be 'accepted'"
    );

    // Verify active_agent_id == agent-gamma after hop 2.
    let conv_after_hop2 = state
        .conversation_store()
        .get(&conv_id)
        .await
        .expect("conversation must exist after hop 2");
    assert_eq!(
        conv_after_hop2
            .active_agent_id
            .as_ref()
            .map(|a| a.to_string()),
        Some("agent-gamma".to_string()),
        "active_agent_id must be agent-gamma after hop 2"
    );

    // Two handoff cards total — oldest is the A→B card, newest is the B→C card.
    assert_eq!(
        conv_after_hop2.messages.len(),
        2,
        "two handoff cards must exist after both hops"
    );
    assert_eq!(
        conv_after_hop2.messages[0].role, "handoff",
        "first card must have role='handoff'"
    );
    assert_eq!(
        conv_after_hop2.messages[1].role, "handoff",
        "second card must have role='handoff'"
    );

    // Both queue entries must be in Accepted state.
    let hid1 = cyberclaw_core::ids::HandoffId::from_string(hop1_id.clone()).unwrap();
    let hid2 = cyberclaw_core::ids::HandoffId::from_string(hop2_id.clone()).unwrap();
    let stored1 = state
        .handoff_queue
        .get(&hid1)
        .await
        .expect("hop1 handoff still in queue");
    let stored2 = state
        .handoff_queue
        .get(&hid2)
        .await
        .expect("hop2 handoff still in queue");
    assert_eq!(
        stored1.status,
        cyberclaw_core::handoff::HandoffStatus::Accepted,
        "hop1 queue entry must be Accepted"
    );
    assert_eq!(
        stored2.status,
        cyberclaw_core::handoff::HandoffStatus::Accepted,
        "hop2 queue entry must be Accepted"
    );

    // list_recent(10) must return at least both handoffs, both Accepted.
    let recent = state.handoff_queue.list_recent(10).await;
    let accepted_ids: Vec<String> = recent
        .iter()
        .filter(|r| r.status == cyberclaw_core::handoff::HandoffStatus::Accepted)
        .map(|r| r.handoff_id.to_string())
        .collect();
    assert!(
        accepted_ids.contains(&hop1_id),
        "list_recent must include hop1 in Accepted state; got: {:?}",
        accepted_ids
    );
    assert!(
        accepted_ids.contains(&hop2_id),
        "list_recent must include hop2 in Accepted state; got: {:?}",
        accepted_ids
    );
}
