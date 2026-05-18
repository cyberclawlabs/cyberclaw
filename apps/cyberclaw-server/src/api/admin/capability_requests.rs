//! Admin Capability Gap Queue endpoints — A-4 (2026-05-05).
//!
//! Surfaces the `capability_requests` SQLite table written by
//! [`crate::audit::AuditSink`] when an LLM tool call cannot be dispatched
//! (capability not found, governance denial, or execution error).
//!
//! | Method | Path                                                | Purpose                                  |
//! |--------|-----------------------------------------------------|------------------------------------------|
//! | GET    | `/api/v1/admin/capability-requests`                 | List gap entries (filter by `status`)    |
//! | PUT    | `/api/v1/admin/capability-requests/:id/status`      | Mark `implemented` / `rejected` / etc.   |
//!
//! No DELETE: the queue is the operator's audit trail of what agents
//! tried to do but couldn't — silently dropping rows would defeat the
//! purpose. Operators close gaps by transitioning status, not by
//! removing rows.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::audit::{CapabilityRequestRow, CapabilityRequestStatus};
use crate::error::ApiError;
use crate::feedback_loop::{run_feedback_once, FeedbackLoopConfig};
use crate::state::AppState;

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1000;

#[derive(Debug, Deserialize, Default)]
pub struct ListQuery {
    /// Filter to a specific status (`pending` / `implemented` / `rejected`).
    /// When omitted, returns all rows.
    pub status: Option<String>,
    /// Cap on the response size. Defaults to 100, max 1000.
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub entries: Vec<CapabilityRequestRow>,
    pub total_returned: usize,
}

#[derive(Debug, Deserialize)]
pub struct UpdateStatusBody {
    /// `pending` | `implemented` | `rejected`.
    pub status: String,
    /// Optional operator note appended to the row.
    pub notes: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UpdateStatusResponse {
    pub id: String,
    pub status: String,
    pub updated: bool,
}

/// `GET /api/v1/admin/capability-requests`
pub async fn list_requests(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListQuery>,
) -> Result<Json<ListResponse>, ApiError> {
    let Some(sink) = state.audit.as_ref() else {
        // No audit sink configured (test mode or local dev without sink) — surface
        // an empty list so the SPA renders cleanly instead of 500-ing.
        return Ok(Json(ListResponse {
            entries: Vec::new(),
            total_returned: 0,
        }));
    };

    let status = match q.status.as_deref() {
        None | Some("") | Some("all") => None,
        Some(s) => match CapabilityRequestStatus::parse(s) {
            Some(parsed) => Some(parsed),
            None => {
                return Err(ApiError::InvalidInput(format!(
                    "unknown status '{}': expected pending|implemented|rejected|all",
                    s
                )))
            }
        },
    };

    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    let entries = sink
        .list_capability_requests(status, limit)
        .await
        .map_err(|e| ApiError::InternalError(format!("list capability_requests failed: {e}")))?;
    let total_returned = entries.len();
    Ok(Json(ListResponse {
        entries,
        total_returned,
    }))
}

/// `PUT /api/v1/admin/capability-requests/:id/status`
pub async fn update_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateStatusBody>,
) -> Result<(StatusCode, Json<UpdateStatusResponse>), ApiError> {
    let Some(sink) = state.audit.as_ref() else {
        return Err(ApiError::InternalError(
            "audit sink not configured — capability gap updates require persistence".to_string(),
        ));
    };

    let status = CapabilityRequestStatus::parse(&body.status).ok_or_else(|| {
        ApiError::InvalidInput(format!(
            "unknown status '{}': expected pending|implemented|rejected",
            body.status
        ))
    })?;

    let updated = sink
        .update_capability_request_status(&id, status, body.notes.as_deref())
        .await
        .map_err(|e| ApiError::InternalError(format!("update capability_request failed: {e}")))?;

    if !updated {
        return Err(ApiError::NotFound(format!(
            "capability_request id '{}' not found",
            id
        )));
    }

    Ok((
        StatusCode::OK,
        Json(UpdateStatusResponse {
            id,
            status: status.as_str().to_string(),
            updated,
        }),
    ))
}

/// D-4 (2026-05-05) — response from `POST .../run-feedback`.
#[derive(Debug, Serialize)]
pub struct RunFeedbackResponse {
    pub aggregated: u32,
    pub written_to_queue: u32,
}

/// `POST /api/v1/admin/capability-requests/run-feedback`
///
/// Triggers an immediate feedback cycle: scan recent failed
/// `capability.complete` rows, fold clusters back into
/// `capability_requests`. Reuses the same window/min_count tunables the
/// background loop reads from env, so manual + scheduled cycles agree.
///
/// Returns `{ aggregated, written_to_queue }`. Used by operators to
/// validate a fresh deploy ("did D-4 wire up?") and by CI to assert the
/// route is mounted.
pub async fn run_feedback(
    State(state): State<Arc<AppState>>,
) -> Result<Json<RunFeedbackResponse>, ApiError> {
    let Some(sink) = state.audit.as_ref() else {
        return Err(ApiError::InternalError(
            "audit sink not configured — feedback loop requires persistence".to_string(),
        ));
    };
    let cfg = FeedbackLoopConfig::from_env();
    let report = run_feedback_once(sink, cfg.window, cfg.min_count)
        .await
        .map_err(|e| ApiError::InternalError(format!("run_feedback_once failed: {e}")))?;
    Ok(Json(RunFeedbackResponse {
        aggregated: report.aggregated,
        written_to_queue: report.written_to_queue,
    }))
}

pub fn create_admin_capability_requests_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/admin/capability-requests", get(list_requests))
        .route(
            "/api/v1/admin/capability-requests/:id/status",
            put(update_status),
        )
        // D-4 — manual trigger for the feedback loop.
        .route(
            "/api/v1/admin/capability-requests/run-feedback",
            post(run_feedback),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_helpers::build_test_state;
    use crate::audit::{AuditSink, CapabilityRequestReason};
    use tempfile::TempDir;

    async fn state_with_audit() -> (Arc<AppState>, TempDir) {
        let tmp = TempDir::new().unwrap();
        let sink = Arc::new(
            AuditSink::new(tmp.path().join("audit.db"))
                .await
                .expect("audit sink"),
        );
        let mut state = build_test_state();
        let state_mut = Arc::get_mut(&mut state).expect("unique Arc in test");
        state_mut.audit = Some(sink);
        (state, tmp)
    }

    #[tokio::test]
    async fn list_returns_empty_when_no_sink() {
        let state = build_test_state();
        let resp = list_requests(State(state), Query(ListQuery::default()))
            .await
            .unwrap();
        assert_eq!(resp.0.total_returned, 0);
    }

    #[tokio::test]
    async fn list_returns_recorded_gap() {
        let (state, _tmp) = state_with_audit().await;
        state
            .audit
            .as_ref()
            .unwrap()
            .record_capability_request(
                "verify_numeric",
                None,
                None,
                CapabilityRequestReason::NotFound,
                Some("agent_alpha"),
                None,
            )
            .await;
        let resp = list_requests(State(state), Query(ListQuery::default()))
            .await
            .unwrap();
        assert_eq!(resp.0.total_returned, 1);
        assert_eq!(resp.0.entries[0].tool_name, "verify_numeric");
    }

    #[tokio::test]
    async fn list_rejects_unknown_status() {
        let (state, _tmp) = state_with_audit().await;
        let res = list_requests(
            State(state),
            Query(ListQuery {
                status: Some("garbled".into()),
                limit: None,
            }),
        )
        .await;
        assert!(matches!(res, Err(ApiError::InvalidInput(_))));
    }

    #[tokio::test]
    async fn update_status_marks_row_implemented() {
        let (state, _tmp) = state_with_audit().await;
        state
            .audit
            .as_ref()
            .unwrap()
            .record_capability_request(
                "verify_numeric",
                None,
                None,
                CapabilityRequestReason::NotFound,
                None,
                None,
            )
            .await;
        let listed = state
            .audit
            .as_ref()
            .unwrap()
            .list_capability_requests(None, 10)
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);
        let id = listed[0].id.clone();

        let resp = update_status(
            State(state.clone()),
            Path(id.clone()),
            Json(UpdateStatusBody {
                status: "implemented".to_string(),
                notes: Some("S37 ships verify.numeric_aggregate".to_string()),
            }),
        )
        .await
        .unwrap();
        assert_eq!(resp.0, StatusCode::OK);
        assert!(resp.1.updated);
        assert_eq!(resp.1.status, "implemented");

        // Confirm the row moved status.
        let pending = state
            .audit
            .as_ref()
            .unwrap()
            .list_capability_requests(Some(CapabilityRequestStatus::Pending), 10)
            .await
            .unwrap();
        assert_eq!(pending.len(), 0);
    }

    #[tokio::test]
    async fn update_status_returns_404_for_unknown_id() {
        let (state, _tmp) = state_with_audit().await;
        let res = update_status(
            State(state),
            Path("no_such_id".into()),
            Json(UpdateStatusBody {
                status: "rejected".into(),
                notes: None,
            }),
        )
        .await;
        assert!(matches!(res, Err(ApiError::NotFound(_))));
    }

    #[tokio::test]
    async fn run_feedback_aggregates_and_writes() {
        let (state, _tmp) = state_with_audit().await;
        // Seed 4 failed capability.complete rows for the same tool.
        let sink = state.audit.as_ref().unwrap().clone();
        for _ in 0..4 {
            sink.record_capability_complete(
                "agent_alpha",
                "exec_dx",
                "verify.numeric",
                "local",
                "verify_numeric",
                10,
                0,
                Some("boom"),
            )
            .await;
        }
        let resp = run_feedback(State(state.clone())).await.unwrap();
        assert_eq!(resp.0.aggregated, 1);
        assert_eq!(resp.0.written_to_queue, 1);

        // The aggregate must surface in the queue.
        let listed = sink.list_capability_requests(None, 100).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].tool_name, "verify_numeric");
    }

    #[tokio::test]
    async fn run_feedback_returns_zero_when_no_failures() {
        let (state, _tmp) = state_with_audit().await;
        let resp = run_feedback(State(state)).await.unwrap();
        assert_eq!(resp.0.aggregated, 0);
        assert_eq!(resp.0.written_to_queue, 0);
    }

    #[tokio::test]
    async fn run_feedback_errors_when_no_audit_sink() {
        let state = build_test_state();
        let res = run_feedback(State(state)).await;
        assert!(matches!(res, Err(ApiError::InternalError(_))));
    }

    #[tokio::test]
    async fn update_status_rejects_unknown_status() {
        let (state, _tmp) = state_with_audit().await;
        let res = update_status(
            State(state),
            Path("anything".into()),
            Json(UpdateStatusBody {
                status: "garbled".into(),
                notes: None,
            }),
        )
        .await;
        assert!(matches!(res, Err(ApiError::InvalidInput(_))));
    }
}
