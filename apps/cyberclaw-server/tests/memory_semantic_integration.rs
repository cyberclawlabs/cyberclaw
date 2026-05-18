//! Sprint 25 T6 — End-to-end integration test for memory semantic search.
//!
//! Exercises the full HTTP path:
//!
//! `DeterministicEmbedClient` injected into `AppState`
//!   → seed `LeveledMemoryRecord`s with distinct embedding vectors
//!   → `GET /api/v1/memory/search?mode=semantic` via axum Router oneshot
//!   → assert results ordered by cosine similarity to query vector
//!
//! # Why we inject at AppState level (not via env var)
//!
//! `AppState::new` constructs `embed_client` from env vars. Tests that need a
//! deterministic embed client mutate the freshly-built state struct before
//! wrapping it in `Arc` — the same pattern used by the `search_with_semantic_mode_returns_ranked`
//! unit test inside `api/memory.rs`.
//!
//! # Cosine geometry used in this file
//!
//! dim=4 vectors, orthogonal basis:
//!   record_a "rust async"       → [1.0, 0.0, 0.0, 0.0]
//!   record_b "python web"       → [0.0, 1.0, 0.0, 0.0]
//!   record_c "no embed"         → None (embedding absent)
//!   query    "rust performance" → [0.9, 0.0, 0.1, 0.0]  (nearly parallel to record_a)
//!
//! Expected cosine ranks: record_a (≈0.994) > record_b (0.0) > record_c (skipped).

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware as axum_middleware, Router,
};
use chrono::Utc;
use cyberclaw_llm::{EmbedClient, LlmResult, NoopEmbedClient};
use cyberclaw_store::{LeveledMemoryRecord, LeveledMemoryStore, MemoryLevel};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serial_test::serial;
use std::{collections::HashMap, sync::Arc};
use tower::ServiceExt;

// Re-export the shared session constant used by all memory handlers.
use cyberclaw_server::api::memory::GLOBAL_DEMO_SESSION;

// ---------------------------------------------------------------------------
// DeterministicEmbedClient — maps known strings → fixed vectors; default → zeros
// ---------------------------------------------------------------------------

/// Returns pre-configured vectors for known inputs; all others get a zero vector.
///
/// Used to make cosine ranking deterministic without a real embedding provider.
#[derive(Debug)]
struct DeterministicEmbedClient {
    responses: HashMap<String, Vec<f32>>,
    dim: usize,
}

impl DeterministicEmbedClient {
    fn new(responses: HashMap<String, Vec<f32>>, dim: usize) -> Self {
        Self { responses, dim }
    }
}

#[async_trait]
impl EmbedClient for DeterministicEmbedClient {
    async fn embed(&self, input: &str) -> LlmResult<Vec<f32>> {
        Ok(self
            .responses
            .get(input)
            .cloned()
            .unwrap_or_else(|| vec![0.0_f32; self.dim]))
    }

    fn dimension(&self) -> usize {
        self.dim
    }
}

// ---------------------------------------------------------------------------
// JWT helpers
// ---------------------------------------------------------------------------

const TEST_JWT_SECRET: &str = "test-s25-integration-secret";

#[derive(Debug, Serialize, Deserialize)]
struct TestClaims {
    sub: String,
    iat: i64,
    exp: i64,
}

fn make_jwt(user_id: &str) -> String {
    let now = Utc::now().timestamp();
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
// RAII $HOME guard — mirrors handoff_review_integration.rs pattern
// ---------------------------------------------------------------------------

struct RestoreHome {
    original: Option<std::ffi::OsString>,
}

impl RestoreHome {
    fn set(new_home: &std::path::Path) -> Self {
        let original = std::env::var_os("HOME");
        // SAFETY: tests are serialised with #[serial].
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

/// Seed a temp `users.toml` with one admin operator and redirect `$HOME`.
///
/// Returns the `TempDir` (must stay alive for the duration of the test) and
/// the `RestoreHome` RAII guard.
fn seed_admin(user_id: &str) -> (tempfile::TempDir, RestoreHome) {
    use cyberclaw_core::ids::UserId;
    use cyberclaw_core::users::{OperatorRecord, UsersConfig};

    let tmp = tempfile::tempdir().expect("tempdir");
    let restore = RestoreHome::set(tmp.path());

    let uid = UserId::from_string(user_id.to_string()).expect("valid user id");
    let record = OperatorRecord {
        user_id: uid,
        display_name: format!("Admin {user_id}"),
        created_at: "2026-04-26T00:00:00Z".to_string(),
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

/// Build a minimal `AppState` with the given `embed_client` injected.
///
/// Uses the same construction path as `build_test_state` in `api/test_helpers`
/// but replaces `embed_client` before the state is frozen in `Arc`.
fn build_state_with_embed(embed_client: Arc<dyn EmbedClient>) -> Arc<cyberclaw_server::AppState> {
    use cyberclaw_connectors::registry::ConnectorRegistry;
    use cyberclaw_control_plane::{
        event_bus::InMemoryEventBus, execution_service::InMemoryExecutionService,
        gateway_router::InMemoryGatewayRouter, lease_manager::InMemoryLeaseManager,
        membership_service::InMemoryMembershipService, orchestrator::ControlPlaneOrchestrator,
        placement_engine::InMemoryPlacementEngine, registry::InMemoryRegistry,
        resolver::InMemoryResolver, review_queue::InMemoryReviewQueue,
        task_manager::InMemoryTaskManager,
    };
    use cyberclaw_governance::engine::DefaultPolicyEngine;
    use cyberclaw_llm::prelude::GenericOpenAiClient;
    use cyberclaw_llm::LlmClient;
    use cyberclaw_observability::{
        events::InMemoryEventRecorder, security_event_store::InMemorySecurityEventStore,
    };
    use cyberclaw_server::ControlPlaneComponents;

    let llm: Arc<dyn LlmClient> =
        Arc::new(GenericOpenAiClient::new(None, "http://localhost:18080").expect("llm client"));

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

    let plugin_registry = Arc::new(cyberclaw_plugin_runtime::PluginRegistry::new(
        std::env::temp_dir().join(format!(
            "cyberclaw-s25-plugins-{}",
            uuid::Uuid::new_v4().simple()
        )),
    ));

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

    // Build default AppState (embed_client will be NoopEmbedClient from env).
    let mut state = cyberclaw_server::AppState::new(
        llm,
        connector_registry,
        policy_engine,
        event_recorder,
        TEST_JWT_SECRET.to_string(),
        cp,
    );

    // Inject the deterministic embed client, overriding the Noop default.
    state.embed_client = embed_client;

    Arc::new(state)
}

/// Wrap `memory_router` with JWT auth middleware — same as the unit tests in memory.rs.
fn memory_app(state: Arc<cyberclaw_server::AppState>) -> Router {
    cyberclaw_server::api::memory::create_memory_router()
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            cyberclaw_server::middleware::jwt_auth,
        ))
        .with_state(state)
}

/// Seed a `LeveledMemoryRecord` with a given embedding into the store.
async fn seed_record(
    store: &dyn LeveledMemoryStore,
    id: &str,
    content: &str,
    embedding: Option<Vec<f32>>,
) {
    let now = Utc::now();
    let record = LeveledMemoryRecord {
        id: id.to_string(),
        session_id: GLOBAL_DEMO_SESSION.to_string(),
        agent_id: "agent-s25-test".to_string(),
        level: MemoryLevel::L1Summary,
        key: id.to_string(),
        content: serde_json::json!(content),
        created_at: now,
        updated_at: now,
        ttl_seconds: MemoryLevel::L1Summary.default_ttl_seconds(),
        source_execution_id: None,
        embedding,
        tags: Vec::new(),
    };
    store.store_leveled(record).await.expect("seed record");
}

// ---------------------------------------------------------------------------
// Test 1 — semantic_search_returns_cosine_ranked_results
// ---------------------------------------------------------------------------

/// Sprint 25 T6: semantic search returns results ranked by cosine similarity.
///
/// Setup:
///   - `DeterministicEmbedClient` with dim=4
///   - query "rust performance" → [0.9, 0.0, 0.1, 0.0]
///   - record_a "rust async" embedding [1.0, 0.0, 0.0, 0.0]  → cosine ≈ 0.994
///   - record_b "python web" embedding [0.0, 1.0, 0.0, 0.0]  → cosine = 0.0
///   - record_c "no embed"   embedding None                   → skipped
///
/// Asserts:
///   - HTTP 200
///   - `mode` field = "semantic"
///   - results[0].memory.id = "s25-record-a" (highest cosine)
///   - results[1].memory.id = "s25-record-b"
///   - record_c absent from results (no embedding)
///   - scores in descending order
#[tokio::test]
#[serial]
async fn semantic_search_returns_cosine_ranked_results() {
    // Seed admin user so require_admin passes.
    let (_tmp, _restore) = seed_admin("s25-admin-ranked");

    // Build deterministic embed client: "rust performance" → [0.9, 0.0, 0.1, 0.0].
    let mut responses = HashMap::new();
    responses.insert("rust performance".to_string(), vec![0.9_f32, 0.0, 0.1, 0.0]);
    let embed_client: Arc<dyn EmbedClient> = Arc::new(DeterministicEmbedClient::new(responses, 4));

    let state = build_state_with_embed(embed_client);

    // Seed 3 records: two with embeddings, one without.
    seed_record(
        state.memory_store.as_ref(),
        "s25-record-a",
        "rust async",
        Some(vec![1.0, 0.0, 0.0, 0.0]),
    )
    .await;
    seed_record(
        state.memory_store.as_ref(),
        "s25-record-b",
        "python web",
        Some(vec![0.0, 1.0, 0.0, 0.0]),
    )
    .await;
    seed_record(
        state.memory_store.as_ref(),
        "s25-record-c",
        "no embed",
        None, // no embedding — must be skipped by semantic search
    )
    .await;

    let token = make_jwt("s25-admin-ranked");
    let app = memory_app(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/memory/search?q=rust+performance&mode=semantic&limit=10")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot");

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "semantic search must return 200"
    );

    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body bytes");
    let body: Value = serde_json::from_slice(&bytes).expect("parse json");

    // mode field must be "semantic".
    assert_eq!(
        body["mode"].as_str().unwrap_or(""),
        "semantic",
        "response must carry mode=semantic"
    );

    let results = body["results"].as_array().expect("results array");

    // record_c has no embedding and must be absent.
    let ids: Vec<&str> = results
        .iter()
        .map(|r| r["memory"]["id"].as_str().unwrap_or(""))
        .collect();
    assert!(
        !ids.contains(&"s25-record-c"),
        "record with None embedding must be excluded; got ids: {ids:?}"
    );

    // record_a must be first (cosine ≈ 0.994 > 0.0 for record_b).
    assert!(
        results.len() >= 2,
        "expected at least 2 results (a and b); got {results:?}"
    );
    assert_eq!(
        results[0]["memory"]["id"].as_str().unwrap_or(""),
        "s25-record-a",
        "record_a ([1,0,0,0]) must rank first against query [0.9,0,0.1,0]; got: {results:?}"
    );
    assert_eq!(
        results[1]["memory"]["id"].as_str().unwrap_or(""),
        "s25-record-b",
        "record_b ([0,1,0,0]) must rank second; got: {results:?}"
    );

    // Scores must be in descending order.
    let scores: Vec<f64> = results
        .iter()
        .map(|r| r["score"].as_f64().unwrap_or(0.0))
        .collect();
    for window in scores.windows(2) {
        assert!(
            window[0] >= window[1],
            "scores must be non-increasing; got {scores:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 3 — hybrid_search_combines_bm25_recall_with_semantic_rerank
// ---------------------------------------------------------------------------

/// Sprint 26 T3: hybrid search combines BM25 recall with cosine reranking.
///
/// Setup (dim=4, query "rust" → [1.0, 0.0, 0.0, 0.0]):
///   - record_alpha "rust async helper"      embedding [1.0, 0.0, 0.0, 0.0]
///       BM25 > 0 (contains "rust") + cosine = 1.0 → hybrid ≈ 1.0  (top)
///   - record_beta  "rust performance guide" embedding [0.0, 1.0, 0.0, 0.0]
///       BM25 > 0 (contains "rust") + cosine = 0.0 → hybrid ≈ 0.5  (second)
///   - record_gamma "systems programming"    embedding None
///       BM25 = 0 (no "rust") → filtered out before cosine step → absent
///
/// Asserts:
///   - HTTP 200, mode == "hybrid"
///   - results[0].memory.id == "s26-alpha" (highest hybrid score)
///   - results length == 2 (gamma absent)
#[tokio::test]
#[serial]
async fn hybrid_search_combines_bm25_recall_with_semantic_rerank() {
    let (_tmp, _restore) = seed_admin("s26-admin-hybrid");

    // "rust" → [1.0, 0.0, 0.0, 0.0]; everything else → zeros (not called for records).
    let mut responses = HashMap::new();
    responses.insert("rust".to_string(), vec![1.0_f32, 0.0, 0.0, 0.0]);
    let embed_client: Arc<dyn EmbedClient> = Arc::new(DeterministicEmbedClient::new(responses, 4));

    let state = build_state_with_embed(embed_client);

    // record_alpha: BM25 high (rust token) + cosine 1.0 → hybrid top.
    seed_record(
        state.memory_store.as_ref(),
        "s26-alpha",
        "rust async helper",
        Some(vec![1.0, 0.0, 0.0, 0.0]),
    )
    .await;

    // record_beta: BM25 high (rust token) + cosine 0.0 → hybrid second.
    seed_record(
        state.memory_store.as_ref(),
        "s26-beta",
        "rust performance guide",
        Some(vec![0.0, 1.0, 0.0, 0.0]),
    )
    .await;

    // record_gamma: BM25 = 0 (no "rust" token) → filtered before cosine.
    seed_record(
        state.memory_store.as_ref(),
        "s26-gamma",
        "systems programming",
        None,
    )
    .await;

    let token = make_jwt("s26-admin-hybrid");
    let app = memory_app(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/memory/search?q=rust&mode=hybrid&limit=10")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot");

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "hybrid search must return 200"
    );

    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body bytes");
    let body: Value = serde_json::from_slice(&bytes).expect("parse json");

    // mode must be "hybrid" (not fallback).
    assert_eq!(
        body["mode"].as_str().unwrap_or(""),
        "hybrid",
        "response mode must be 'hybrid'; got: {body}"
    );

    let results = body["results"].as_array().expect("results array");

    // gamma has BM25=0 → must be absent.
    let ids: Vec<&str> = results
        .iter()
        .map(|r| r["memory"]["id"].as_str().unwrap_or(""))
        .collect();
    assert!(
        !ids.contains(&"s26-gamma"),
        "record_gamma (BM25=0) must be excluded; got ids: {ids:?}"
    );

    // Exactly alpha and beta should appear.
    assert_eq!(
        results.len(),
        2,
        "expected exactly 2 results (alpha + beta); got: {ids:?}"
    );

    // alpha must rank first: hybrid ≈ 0.5*1.0 + 0.5*1.0 = 1.0.
    assert_eq!(
        results[0]["memory"]["id"].as_str().unwrap_or(""),
        "s26-alpha",
        "record_alpha must be top-ranked (highest hybrid score); got: {ids:?}"
    );

    // alpha score must exceed beta score.
    let alpha_score = results[0]["score"].as_f64().unwrap_or(0.0);
    let beta_score = results[1]["score"].as_f64().unwrap_or(0.0);
    assert!(
        alpha_score > beta_score,
        "alpha hybrid score ({alpha_score}) must exceed beta ({beta_score})"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — semantic_search_returns_400_when_embed_disabled
// ---------------------------------------------------------------------------

/// Sprint 25 T6: when `embed_client.dimension() == 0` (NoopEmbedClient, the
/// default), requesting `mode=semantic` must return HTTP 400 with an
/// explanatory message pointing to `CYBERCLAW_EMBED_ENABLED`.
///
/// This validates the guard in `api/memory.rs::semantic_search` before any
/// embedding call is attempted.
#[tokio::test]
#[serial]
async fn semantic_search_returns_400_when_embed_disabled() {
    let (_tmp, _restore) = seed_admin("s25-admin-disabled");

    // NoopEmbedClient has dimension=0 — semantic search must reject immediately.
    let noop: Arc<dyn EmbedClient> = Arc::new(NoopEmbedClient);
    let state = build_state_with_embed(noop);

    let token = make_jwt("s25-admin-disabled");
    let app = memory_app(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/memory/search?q=anything&mode=semantic")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot");

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "mode=semantic with NoopEmbedClient (dim=0) must return 400"
    );

    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body bytes");
    let body_str = String::from_utf8_lossy(&bytes);
    assert!(
        body_str.contains("CYBERCLAW_EMBED_ENABLED"),
        "400 body must reference CYBERCLAW_EMBED_ENABLED; got: {body_str}"
    );
}
