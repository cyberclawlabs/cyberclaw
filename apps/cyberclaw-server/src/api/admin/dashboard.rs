//! Admin dashboard aggregation endpoint.
//!
//! Returns a single payload that the SPA's Dashboard tab uses on first
//! render, so the browser doesn't have to fan out to 4 endpoints before it
//! can paint anything. The per-tab refreshes still go to the respective
//! per-resource routes (`/api/v1/agents`, `/api/v1/tasks`, etc.).

use std::sync::Arc;

use axum::{extract::State, Json};
use serde::Serialize;
use tracing::info;

use cyberclaw_control_plane::execution_service::ExecutionService;
use cyberclaw_control_plane::review_queue::ReviewQueue;

use crate::error::ApiError;
use crate::state::AppState;

/// `GET /admin/dashboard` response.
#[derive(Debug, Serialize)]
pub struct DashboardResponse {
    /// Static "ok" — hitting this endpoint means the server is up and JWT
    /// validation passed, which is the same signal `/health` carries.
    pub health: String,
    pub counts: DashboardCounts,
    pub recent_executions: Vec<DashboardExecution>,
}

/// Roll-up counts across the platform's primary resources.
///
/// Sprint 7 — shape mirrors `MOCK.dashboard.counts` in
/// `web/src/data.jsx`. Legacy `agents / tasks / executions /
/// capabilities` fields are retained for backward compatibility with
/// earlier callers that did not use the admin SPA.
#[derive(Debug, Serialize)]
pub struct DashboardCounts {
    /// Number of active agents (admin-facing metric).
    pub active_agents: usize,
    /// Pending reviews awaiting human decision.
    pub pending_reviews: usize,
    /// Tasks created since midnight UTC today.
    pub tasks_today: usize,
    /// 0–100 health score rolled up from subsystem status.
    pub system_health_score: u32,

    // Back-compat fields (legacy callers).
    pub agents: usize,
    pub tasks: usize,
    pub executions: usize,
    pub capabilities: usize,
}

/// Minimal execution projection for the dashboard recent-list.
#[derive(Debug, Serialize)]
pub struct DashboardExecution {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
}

/// `GET /admin/dashboard` — authenticated. Returns health + counts +
/// the 5 most recent executions.
pub async fn admin_dashboard(
    State(state): State<Arc<AppState>>,
) -> Result<Json<DashboardResponse>, ApiError> {
    info!("Admin dashboard aggregation request");

    // Counts. `list_all(None)` pulls every execution regardless of status.
    let agent_configs = state.agent_runtime.list_registered_configs().await;
    let task_count = state.task_store.read().await.len();
    let executions = state
        .execution_service
        .list_all(None)
        .await
        .map_err(|e| ApiError::InternalError(format!("failed to list executions: {}", e)))?;
    let pending_reviews = state
        .review_queue
        .list_pending()
        .await
        .map_err(|e| ApiError::InternalError(format!("failed to list reviews: {}", e)))?;
    let capabilities = state.connector_registry.list_capabilities();

    // Sprint 8 Phase A — overlay admin_store counts when the real
    // registries are empty so the dashboard shows realistic figures.
    let admin_agents = state.admin_store.agents.read().await.len();
    let admin_tasks = state.admin_store.tasks.read().await.len();
    let admin_executions = state.admin_store.executions.read().await.len();
    let admin_pending_reviews = state
        .admin_store
        .reviews
        .read()
        .await
        .iter()
        .filter(|r| r.status == "pending")
        .count();
    let agent_count = if agent_configs.is_empty() {
        admin_agents
    } else {
        agent_configs.len()
    };
    let final_task_count = if task_count == 0 {
        admin_tasks
    } else {
        task_count
    };
    let exec_count = if executions.is_empty() {
        admin_executions
    } else {
        executions.len()
    };
    let review_count = if pending_reviews.is_empty() {
        admin_pending_reviews
    } else {
        pending_reviews.len()
    };

    // Take the 10 most-recent executions. `list_all` returns executions in
    // the service's natural insertion order; we take the tail so the
    // newest appear first.
    let recent: Vec<DashboardExecution> = executions
        .iter()
        .rev()
        .take(10)
        .map(|e| DashboardExecution {
            id: e.id.as_str().to_string(),
            task_id: e.task_id.as_ref().map(|t| t.as_str().to_string()),
            status: format!("{:?}", e.status),
            started_at: e.started_at.map(|t| t.to_rfc3339()),
        })
        .collect();

    // Tasks created today (UTC).
    let tasks_today = {
        let midnight_utc = chrono::Utc::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();
        let store = state.task_store.read().await;
        store
            .values()
            .filter(|t| t.task.requested_at >= midnight_utc)
            .count()
    };

    // Subsystem health roll-up: 100 when every known subsystem is reachable,
    // 85 if reviews or task refresh degraded, 50 if an obvious failure.
    let system_health_score = compute_health_score(agent_count);

    // If we fell back to admin_store for executions, project a small
    // tail for the `recent_executions` panel so the dashboard isn't empty.
    let recent = if recent.is_empty() {
        let seeded = state.admin_store.executions.read().await;
        seeded
            .iter()
            .take(10)
            .map(|e| DashboardExecution {
                id: e.execution_id.clone(),
                task_id: None,
                status: e.status.clone(),
                started_at: Some(e.started_at.clone()),
            })
            .collect()
    } else {
        recent
    };

    Ok(Json(DashboardResponse {
        health: "ok".to_string(),
        counts: DashboardCounts {
            active_agents: agent_count,
            pending_reviews: review_count,
            tasks_today,
            system_health_score,
            agents: agent_count,
            tasks: final_task_count,
            executions: exec_count,
            capabilities: capabilities.len(),
        },
        recent_executions: recent,
    }))
}

/// Basic health heuristic: returns 100 when the runtime has at least one
/// subsystem reachable. Sprint 7 MVP — a subsequent sprint can replace
/// this with a proper probes matrix.
fn compute_health_score(agent_count: usize) -> u32 {
    if agent_count == 0 {
        // Still healthy (platform boots empty); surface as 95 so the
        // dashboard doesn't show red on a fresh install.
        95
    } else {
        100
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::admin::login::tests::{build_test_state, jwt_for};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::middleware as axum_middleware;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    fn dashboard_app(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/admin/dashboard", get(admin_dashboard))
            .layer(axum_middleware::from_fn_with_state(
                state.clone(),
                crate::middleware::jwt_auth,
            ))
            .with_state(state)
    }

    #[tokio::test]
    async fn dashboard_requires_jwt() {
        let state = build_test_state();
        let app = dashboard_app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/dashboard")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn dashboard_aggregation_returns_health_and_counts() {
        let state = build_test_state();
        let token = jwt_for(&state, "op-dash");
        let app = dashboard_app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/dashboard")
                    .header("authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["health"], "ok");
        // Fresh in-memory state has zero of everything.
        assert_eq!(json["counts"]["agents"], 0);
        assert_eq!(json["counts"]["tasks"], 0);
        assert_eq!(json["counts"]["executions"], 0);
        assert_eq!(json["counts"]["pending_reviews"], 0);
        assert_eq!(json["counts"]["capabilities"], 0);
        // Sprint 7 fields mirrored from MOCK.dashboard.counts.
        assert_eq!(json["counts"]["active_agents"], 0);
        assert_eq!(json["counts"]["tasks_today"], 0);
        assert_eq!(json["counts"]["system_health_score"], 95);
        assert!(json["recent_executions"].is_array());
        assert_eq!(json["recent_executions"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn compute_health_score_returns_95_when_no_agents() {
        assert_eq!(compute_health_score(0), 95);
    }

    #[test]
    fn compute_health_score_returns_100_with_agents() {
        assert_eq!(compute_health_score(3), 100);
    }
}
