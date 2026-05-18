//! Cluster Brain API — F4 multi-node coordination endpoints.
//!
//! # Routes
//!
//! | Method | Route | Purpose |
//! |--------|-------|---------|
//! | POST | `/api/v1/cluster/brain/register` | Register a remote brain + seed heartbeat |
//! | POST | `/api/v1/cluster/heartbeat/{brain_id}` | Update heartbeat + load for a brain |
//! | POST | `/api/v1/cluster/sessions/assign` | Assign a session to the least-loaded brain |
//! | GET  | `/api/v1/cluster/state` | Snapshot of all brains + sessions |
//!
//! All four routes sit inside the JWT-authenticated `protected_routes` lane,
//! matching the admin-console security model used by the existing cluster-nodes
//! endpoints.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use cyberclaw_control_plane::cluster::node::{NodeHealthStatus, NodeLoad};

use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::Claims;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Request / response shapes
// ---------------------------------------------------------------------------

/// Body for `POST /api/v1/cluster/brain/register`.
#[derive(Debug, Deserialize)]
pub struct RegisterBrainRequest {
    pub brain_id: String,
    pub address: String,
    pub port: u16,
    pub max_concurrent: usize,
}

/// Response for `POST /api/v1/cluster/brain/register`.
#[derive(Debug, Serialize)]
pub struct RegisterBrainResponse {
    pub ok: bool,
    pub brain_id: String,
    pub registered_at: String,
}

/// Body for `POST /api/v1/cluster/heartbeat/{brain_id}`.
#[derive(Debug, Deserialize)]
pub struct HeartbeatRequest {
    pub load: NodeLoad,
}

/// Response for `POST /api/v1/cluster/heartbeat/{brain_id}`.
#[derive(Debug, Serialize)]
pub struct HeartbeatResponse {
    pub ok: bool,
    pub last_seen: String,
}

/// Body for `POST /api/v1/cluster/sessions/assign`.
#[derive(Debug, Deserialize)]
pub struct AssignSessionRequest {
    pub session_id: String,
}

/// Response for `POST /api/v1/cluster/sessions/assign`.
#[derive(Debug, Serialize)]
pub struct AssignSessionResponse {
    pub session_id: String,
    pub assigned_brain: String,
}

/// One brain's view in `GET /api/v1/cluster/state`.
#[derive(Debug, Serialize)]
pub struct BrainStateView {
    pub id: String,
    pub status: String,
    pub last_seen: Option<String>,
    pub load: Option<NodeLoad>,
}

/// One session's view in `GET /api/v1/cluster/state`.
#[derive(Debug, Serialize)]
pub struct SessionStateView {
    pub id: String,
    pub brain: Option<String>,
    pub last_touched: String,
}

/// Response for `GET /api/v1/cluster/state`.
#[derive(Debug, Serialize)]
pub struct ClusterStateResponse {
    pub brains: Vec<BrainStateView>,
    pub sessions: Vec<SessionStateView>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/v1/cluster/brain/register
///
/// Seeds the heartbeat monitor with an initial zero-load record so the brain
/// is immediately trackable. The actual `StatelessBrain` lives on the remote
/// node; this node only tracks its registration + heartbeat metadata.
async fn register_brain(
    State(state): State<Arc<AppState>>,
    claims: Option<axum::Extension<Claims>>,
    Json(req): Json<RegisterBrainRequest>,
) -> Result<Json<RegisterBrainResponse>, ApiError> {
    // Seed a zero-load heartbeat so the node is Healthy right after register.
    let initial_load = NodeLoad {
        cpu_percent: 0.0,
        memory_percent: 0.0,
        active_sessions: 0,
        capacity: req.max_concurrent as u32,
    };
    state
        .heartbeat_monitor
        .record_heartbeat(&req.brain_id, initial_load);

    let registered_at = Utc::now().to_rfc3339();
    tracing::info!(
        brain_id = %req.brain_id,
        address = %req.address,
        port = req.port,
        max_concurrent = req.max_concurrent,
        "F4: brain registered"
    );

    if let Some(sink) = state.audit.as_ref() {
        let actor = claims
            .as_ref()
            .map(|c| c.sub.to_string())
            .unwrap_or_else(|| "system".to_string());
        sink.record(AuditEntry::now(
            actor,
            AuditKind::Mutation,
            "cluster.brain.register".to_string(),
            Some(format!("brain:{}", req.brain_id)),
            serde_json::json!({
                "brain_id": req.brain_id,
                "address": req.address,
                "port": req.port,
                "max_concurrent": req.max_concurrent,
            }),
            AuditResult::Success,
        ))
        .await;
    }

    Ok(Json(RegisterBrainResponse {
        ok: true,
        brain_id: req.brain_id,
        registered_at,
    }))
}

/// POST /api/v1/cluster/heartbeat/{brain_id}
async fn record_heartbeat(
    State(state): State<Arc<AppState>>,
    Path(brain_id): Path<String>,
    Json(req): Json<HeartbeatRequest>,
) -> Result<Json<HeartbeatResponse>, ApiError> {
    state
        .heartbeat_monitor
        .record_heartbeat(&brain_id, req.load);

    let last_seen = Utc::now().to_rfc3339();
    tracing::debug!(brain_id = %brain_id, "F4: heartbeat recorded");

    Ok(Json(HeartbeatResponse {
        ok: true,
        last_seen,
    }))
}

/// POST /api/v1/cluster/sessions/assign
async fn assign_session(
    State(state): State<Arc<AppState>>,
    claims: Option<axum::Extension<Claims>>,
    Json(req): Json<AssignSessionRequest>,
) -> Result<Json<AssignSessionResponse>, ApiError> {
    let assigned_brain = state
        .brain_coordinator
        .assign_session(&req.session_id)
        .await
        .map_err(|e| ApiError::InvalidRequest(format!("session assign failed: {e}")))?;

    tracing::info!(
        session_id = %req.session_id,
        brain = %assigned_brain,
        "F4: session assigned"
    );

    if let Some(sink) = state.audit.as_ref() {
        let actor = claims
            .as_ref()
            .map(|c| c.sub.to_string())
            .unwrap_or_else(|| "system".to_string());
        sink.record(AuditEntry::now(
            actor,
            AuditKind::Mutation,
            "cluster.session.assign".to_string(),
            Some(format!("session:{}", req.session_id)),
            serde_json::json!({
                "session_id": req.session_id,
                "assigned_brain": assigned_brain,
            }),
            AuditResult::Success,
        ))
        .await;
    }

    Ok(Json(AssignSessionResponse {
        session_id: req.session_id,
        assigned_brain,
    }))
}

/// GET /api/v1/cluster/state
async fn get_cluster_state(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ClusterStateResponse>, ApiError> {
    // Collect brain views from heartbeat monitor.
    let node_ids = state.heartbeat_monitor.tracked_nodes();
    let brains: Vec<BrainStateView> = node_ids
        .iter()
        .map(|id| {
            let health = state.heartbeat_monitor.check_health(id);
            let load = state.heartbeat_monitor.get_load(id);
            let status = match health {
                NodeHealthStatus::Healthy => "healthy",
                NodeHealthStatus::Degraded => "degraded",
                NodeHealthStatus::Dead => "dead",
                NodeHealthStatus::Unknown => "unknown",
            };
            BrainStateView {
                id: id.clone(),
                status: status.to_string(),
                // HeartbeatMonitor stores Instant internally; we approximate
                // last_seen as now for Healthy nodes (exact time not exposed).
                last_seen: if health == NodeHealthStatus::Healthy {
                    Some(Utc::now().to_rfc3339())
                } else {
                    None
                },
                load,
            }
        })
        .collect();

    // Collect session views from session store.
    // We gather sessions for every known brain plus unassigned ones.
    let mut sessions: Vec<SessionStateView> = Vec::new();
    for brain_id in &node_ids {
        match state.session_store.list_sessions_for_node(brain_id).await {
            Ok(brain_sessions) => {
                for s in brain_sessions {
                    sessions.push(SessionStateView {
                        id: s.session_id,
                        brain: s.assigned_brain,
                        last_touched: s.last_active.to_rfc3339(),
                    });
                }
            }
            Err(e) => {
                tracing::warn!(brain_id = %brain_id, error = %e, "F4: failed to list sessions");
            }
        }
    }

    Ok(Json(ClusterStateResponse { brains, sessions }))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the `/api/v1/cluster/brain/*` and `/api/v1/cluster/sessions/*` router.
pub fn create_cluster_brain_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/cluster/brain/register", post(register_brain))
        .route(
            "/api/v1/cluster/heartbeat/:brain_id",
            post(record_heartbeat),
        )
        .route("/api/v1/cluster/sessions/assign", post(assign_session))
        .route("/api/v1/cluster/state", get(get_cluster_state))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    use crate::api::test_helpers::build_test_state;

    /// Verify AppState constructs with non-trivially-initialized brain_coordinator.
    #[tokio::test]
    async fn test_appstate_has_brain_coordinator() {
        let state = build_test_state(); // returns Arc<AppState>
                                        // tracked_nodes() returns empty slice before any register call — that's fine.
        let nodes = state.heartbeat_monitor.tracked_nodes();
        assert!(nodes.is_empty(), "fresh state should have no tracked nodes");
        // brain_coordinator is Arc — just verify it's there.
        let _ = state.brain_coordinator.clone();
    }

    /// Round-trip: POST /register → GET /state sees the brain.
    #[tokio::test]
    async fn test_register_and_get_state() {
        let state = build_test_state();
        let app = create_cluster_brain_router().with_state(state);

        // Register a brain.
        let register_body = serde_json::json!({
            "brain_id": "brain-test-1",
            "address": "127.0.0.1",
            "port": 9100,
            "max_concurrent": 8,
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/cluster/brain/register")
            .header("content-type", "application/json")
            .body(Body::from(register_body.to_string()))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["brain_id"], "brain-test-1");

        // GET /state should include the registered brain.
        let req = Request::builder()
            .method("GET")
            .uri("/api/v1/cluster/state")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let brains = json["brains"].as_array().unwrap();
        assert!(
            brains.iter().any(|b| b["id"] == "brain-test-1"),
            "registered brain should appear in /cluster/state"
        );
    }
}
