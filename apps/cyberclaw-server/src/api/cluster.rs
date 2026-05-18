//! Cluster API — minimal surface for the admin's Nodes page.
//!
//! Sprint 7 MVP: a single-node server returns itself as the sole entry.
//! Wiring to the real membership service lives in
//! [`cyberclaw_control_plane::membership_service::InMemoryMembershipService`]
//! and an in-tree follow-up sprint will swap this adapter to read from
//! there. For now, we report what the process knows about itself so the
//! frontend renders a correct row and doesn't have to fall back to MOCK.
//!
//! # Routes
//!
//! | Route | Purpose |
//! |---|---|
//! | `GET /api/v1/cluster/nodes` | List known nodes (self only in MVP) |
//! | `GET /api/v1/cluster/nodes/:id` | Details for one node |

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use serde::Serialize;

use cyberclaw_control_plane::execution_service::ExecutionService;

use crate::error::ApiError;
use crate::state::AppState;

/// Admin-visible node row. Mirrors `MOCK.nodes` in `web/src/data.jsx`.
#[derive(Debug, Serialize)]
pub struct NodeView {
    pub node_id: String,
    pub role: String,
    pub status: String,
    pub last_heartbeat: String,
    pub version: String,
    pub cpu: u32,
    pub mem: u32,
    pub active_execs: u32,
    pub region: String,
}

/// Build the `/api/v1/cluster/*` router.
pub fn create_cluster_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/cluster/nodes", get(list_nodes))
        .route("/api/v1/cluster/nodes/:id", get(get_node))
}

/// Build the self-node projection — the MVP answer for a single-process
/// deployment. Active executions are counted from the execution service.
async fn build_self_view(state: &Arc<AppState>) -> NodeView {
    let active = state
        .execution_service
        .list_all(Some(cyberclaw_core::execution::ExecutionStatus::Running))
        .await
        .map(|v| v.len() as u32)
        .unwrap_or(0);

    NodeView {
        node_id: state.node_id.clone(),
        role: "controller".to_string(),
        status: "leader".to_string(),
        last_heartbeat: chrono::Utc::now().to_rfc3339(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        cpu: 0,
        mem: 0,
        active_execs: active,
        region: std::env::var("CYBERCLAW_REGION").unwrap_or_else(|_| "local".to_string()),
    }
}

/// Project an `AdminNode` seed row into a `NodeView`.
fn node_view_from_admin(n: &crate::admin_store::AdminNode) -> NodeView {
    NodeView {
        node_id: n.node_id.clone(),
        role: n.role.clone(),
        status: n.status.clone(),
        last_heartbeat: n.last_heartbeat.clone(),
        version: n.version.clone(),
        cpu: n.cpu,
        mem: n.mem,
        active_execs: n.active_execs,
        region: n.region.clone().unwrap_or_else(|| "local".to_string()),
    }
}

/// `GET /api/v1/cluster/nodes` — list known nodes.
///
/// Sprint 8 Phase A: prefers the admin_store seed list if populated
/// (demo mode); falls back to the single-node self view otherwise.
async fn list_nodes(State(state): State<Arc<AppState>>) -> Json<Vec<NodeView>> {
    let seeded = state.admin_store.nodes.read().await;
    if !seeded.is_empty() {
        return Json(seeded.iter().map(node_view_from_admin).collect());
    }
    drop(seeded);
    let self_view = build_self_view(&state).await;
    Json(vec![self_view])
}

/// `GET /api/v1/cluster/nodes/:id` — details for one node.
async fn get_node(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<NodeView>, ApiError> {
    if id == state.node_id {
        return Ok(Json(build_self_view(&state).await));
    }
    let seeded = state.admin_store.nodes.read().await;
    match seeded.iter().find(|n| n.node_id == id) {
        Some(n) => Ok(Json(node_view_from_admin(n))),
        None => Err(ApiError::NotFound(format!(
            "node {id} is not known to this controller"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_helpers::build_test_state;

    #[tokio::test]
    async fn build_self_view_uses_state_node_id() {
        let state = build_test_state();
        let view = build_self_view(&state).await;
        assert_eq!(view.node_id, state.node_id);
        assert_eq!(view.role, "controller");
        assert_eq!(view.active_execs, 0);
        assert!(!view.version.is_empty());
    }

    #[tokio::test]
    async fn get_node_unknown_returns_not_found() {
        let state = build_test_state();
        let result = get_node(
            State(state.clone()),
            Path("node-does-not-exist".to_string()),
        )
        .await;
        assert!(result.is_err());
        match result {
            Err(ApiError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_nodes_returns_self() {
        let state = build_test_state();
        let axum::Json(nodes) = list_nodes(State(state.clone())).await;
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_id, state.node_id);
    }
}
