//! Stub endpoints — empty-list responses for v2 frontend pages.
//!
//! Each handler returns a sensible default (empty array / disabled flag) so
//! the frontend's amber "endpoint not yet implemented" banners become proper
//! empty-state tables instead of HTTP 404 errors.
//!
//! **These are stubs.** No storage layer is wired. When real data is ready,
//! replace the handler body and keep the route registration in `lib.rs`.
//!
//! All routes require JWT and are mounted inside the `protected_routes` lane.

use axum::{extract::State, routing::get, Json, Router};
use serde::Serialize;
use std::sync::Arc;

use crate::audit::{AuditKind, AuditQuery};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// GET /api/v1/clarifications
// ---------------------------------------------------------------------------

/// Stub returning empty list; frontend ClarificationsPage renders skeleton.
#[derive(Serialize)]
struct ClarificationsResponse {
    clarifications: Vec<serde_json::Value>,
}

async fn list_clarifications() -> Json<ClarificationsResponse> {
    Json(ClarificationsResponse {
        clarifications: vec![],
    })
}

// ---------------------------------------------------------------------------
// GET /api/v1/handoffs
// ---------------------------------------------------------------------------

/// Stub returning empty list; frontend HandoffsPage renders skeleton.
/// (The full handoff lifecycle lives at /api/v1/chat/handoff.)
#[derive(Serialize)]
struct HandoffsResponse {
    handoffs: Vec<serde_json::Value>,
}

async fn list_handoffs_stub() -> Json<HandoffsResponse> {
    Json(HandoffsResponse { handoffs: vec![] })
}

// ---------------------------------------------------------------------------
// GET /api/v1/learning/sessions
// ---------------------------------------------------------------------------

/// Stub returning empty list; frontend LearningPage renders skeleton.
#[derive(Serialize)]
struct LearningSessionsResponse {
    sessions: Vec<serde_json::Value>,
}

async fn list_learning_sessions() -> Json<LearningSessionsResponse> {
    Json(LearningSessionsResponse { sessions: vec![] })
}

// ---------------------------------------------------------------------------
// GET /api/v1/curator/audits
// ---------------------------------------------------------------------------

/// Stub returning empty list; frontend CuratorPage renders skeleton.
#[derive(Serialize)]
struct CuratorAuditsResponse {
    audits: Vec<serde_json::Value>,
}

async fn list_curator_audits(State(state): State<Arc<AppState>>) -> Json<CuratorAuditsResponse> {
    // No longer a stub — query the audit sink for curator.* events.
    // Falls back to empty list if no audit sink is wired (test path).
    let Some(sink) = state.audit.as_ref() else {
        return Json(CuratorAuditsResponse { audits: vec![] });
    };
    let filters = AuditQuery {
        kind: Some(AuditKind::Mutation),
        action_prefix: Some("curator.".to_string()),
        ..Default::default()
    };
    let rows = sink.tail(200, &filters).await.unwrap_or_default();
    let audits: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "ts": r.ts,
                "actor": r.actor,
                "kind": format!("{:?}", r.kind),
                "action": r.action,
                "target": r.target,
                "detail": r.detail,
                "result": format!("{:?}", r.result),
            })
        })
        .collect();
    Json(CuratorAuditsResponse { audits })
}

// ---------------------------------------------------------------------------
// GET /api/v1/capability-monitor
// ---------------------------------------------------------------------------

/// Stub returning empty verdicts list; frontend CapabilityMonitorPage renders skeleton.
#[derive(Serialize)]
struct CapabilityMonitorResponse {
    verdicts: Vec<serde_json::Value>,
}

async fn list_capability_verdicts() -> Json<CapabilityMonitorResponse> {
    Json(CapabilityMonitorResponse { verdicts: vec![] })
}

// ---------------------------------------------------------------------------
// GET /api/v1/cluster
// ---------------------------------------------------------------------------

/// Stub returning empty cluster state; frontend ClusterPage renders skeleton.
/// Shape matches ClusterState: { brains, coordinator_id, election_term, total_active_loops }.
#[derive(Serialize)]
struct ClusterStateResponse {
    brains: Vec<serde_json::Value>,
    coordinator_id: Option<String>,
    election_term: u32,
    total_active_loops: u32,
}

async fn get_cluster_state() -> Json<ClusterStateResponse> {
    Json(ClusterStateResponse {
        brains: vec![],
        coordinator_id: None,
        election_term: 0,
        total_active_loops: 0,
    })
}

// ---------------------------------------------------------------------------
// GET /api/v1/multimodal
// ---------------------------------------------------------------------------

/// Stub returning disabled + empty capabilities; frontend MultimodalPage amber
/// banner references this path.
#[derive(Serialize)]
struct MultimodalResponse {
    enabled: bool,
    capabilities: Vec<serde_json::Value>,
}

async fn get_multimodal_status() -> Json<MultimodalResponse> {
    Json(MultimodalResponse {
        enabled: false,
        capabilities: vec![],
    })
}

// ---------------------------------------------------------------------------
// Router factory
// ---------------------------------------------------------------------------

/// Build the stub router. Mount inside `protected_routes` (JWT required).
///
/// NOTE: /api/v1/cron is intentionally NOT here — it is served by the real
/// CronStore-backed router in `api::cron` (merged separately in lib.rs).
pub fn create_stubs_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/clarifications", get(list_clarifications))
        .route("/api/v1/handoffs", get(list_handoffs_stub))
        .route("/api/v1/learning/sessions", get(list_learning_sessions))
        .route("/api/v1/curator/audits", get(list_curator_audits))
        .route("/api/v1/capability-monitor", get(list_capability_verdicts))
        .route("/api/v1/cluster", get(get_cluster_state))
        .route("/api/v1/multimodal", get(get_multimodal_status))
}
