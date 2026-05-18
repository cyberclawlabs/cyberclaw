//! Internal cluster APIs for cross-instance assignment delivery.
//!
//! These endpoints are intentionally separated from user-facing APIs.

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    routing::post,
    Json, Router,
};
use cyberclaw_control_plane::execution_service::ExecutionService;
use cyberclaw_core::execution::ExecutionStatus;
use cyberclaw_core::ids::{ExecutionId, NodeId};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

use crate::cluster_assignments::{
    validate_cluster_token, AssignmentAckResponse, AssignmentId, ClaimAssignmentRequest,
    ClaimAssignmentResponse, CompleteAssignmentRequest, PullAssignmentsRequest,
    PullAssignmentsResponse, ReleaseAssignmentRequest, RenewAssignmentLeaseRequest,
    RenewAssignmentLeaseResponse, ReportExecutionStatusRequest, ReportExecutionStatusResponse,
    DEFAULT_ASSIGNMENT_LEASE_TTL_SECS,
};
use crate::error::ApiError;
use crate::state::AppState;

pub fn create_internal_cluster_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/internal/cluster/assignments/pull", post(pull_assignments))
        .route(
            "/internal/cluster/executions/report",
            post(report_execution_status),
        )
        // Sprint 10 W2 L4 — worker-pull lease protocol.
        .route(
            "/internal/cluster/assignments/claim",
            post(claim_assignment),
        )
        .route(
            "/internal/cluster/assignments/:id/complete",
            post(complete_assignment),
        )
        .route(
            "/internal/cluster/assignments/:id/release",
            post(release_assignment),
        )
        .route(
            "/internal/cluster/assignments/:id/lease/renew",
            post(renew_assignment_lease),
        )
}

async fn pull_assignments(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<PullAssignmentsRequest>,
) -> Result<Json<PullAssignmentsResponse>, ApiError> {
    validate_cluster_token(&headers)
        .map_err(|e| ApiError::Unauthorized(format!("cluster token validation failed: {}", e)))?;

    let node_id = NodeId::from_string(req.node_id.clone())
        .map_err(|e| ApiError::InvalidInput(format!("invalid node_id: {}", e)))?;
    let limit = req.limit.unwrap_or(1).clamp(1, 32);
    let assignments = state.assignment_queue.dequeue_batch(&node_id, limit).await;

    info!(
        node_id = %node_id,
        pulled = assignments.len(),
        "internal cluster pull assignments"
    );

    Ok(Json(PullAssignmentsResponse { assignments }))
}

async fn report_execution_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ReportExecutionStatusRequest>,
) -> Result<Json<ReportExecutionStatusResponse>, ApiError> {
    validate_cluster_token(&headers)
        .map_err(|e| ApiError::Unauthorized(format!("cluster token validation failed: {}", e)))?;

    let execution_id = ExecutionId::from_string(req.execution_id.clone())
        .map_err(|e| ApiError::InvalidInput(format!("invalid execution_id: {}", e)))?;
    let status = parse_execution_status(&req.status)
        .map_err(|e| ApiError::InvalidInput(format!("invalid status: {}", e)))?;

    if let Some(err) = &req.error {
        warn!(
            execution_id = %execution_id,
            remote_status = %req.status,
            remote_error = %err,
            "remote worker reported execution failure"
        );
    }

    state
        .execution_service
        .update_status(&execution_id, status)
        .await
        .map_err(|e| {
            ApiError::InternalError(format!(
                "failed to update execution status from report: {}",
                e
            ))
        })?;

    Ok(Json(ReportExecutionStatusResponse { ok: true }))
}

// ---------------------------------------------------------------------------
// Worker-pull lease protocol handlers (Sprint 10 W2 L4)
// ---------------------------------------------------------------------------

fn resolve_lease_ttl(secs: Option<u64>) -> Duration {
    Duration::from_secs(secs.unwrap_or(DEFAULT_ASSIGNMENT_LEASE_TTL_SECS))
}

async fn claim_assignment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ClaimAssignmentRequest>,
) -> Result<Json<ClaimAssignmentResponse>, ApiError> {
    validate_cluster_token(&headers)
        .map_err(|e| ApiError::Unauthorized(format!("cluster token validation failed: {}", e)))?;

    let node_id = NodeId::from_string(req.node_id.clone())
        .map_err(|e| ApiError::InvalidInput(format!("invalid node_id: {}", e)))?;
    let ttl = resolve_lease_ttl(req.lease_ttl_secs);

    let claim = state.assignment_queue.claim(&node_id, ttl).await;

    match claim {
        Some((assignment_id, payload)) => {
            info!(
                node_id = %node_id,
                assignment_id = %assignment_id,
                ttl_secs = ttl.as_secs(),
                "internal cluster claim assignment"
            );
            let lease_expires_at = chrono::Utc::now()
                + chrono::Duration::from_std(ttl).unwrap_or_else(|_| chrono::Duration::seconds(60));
            Ok(Json(ClaimAssignmentResponse {
                assignment_id: Some(assignment_id.as_str().to_string()),
                payload: Some(payload),
                lease_expires_at: Some(lease_expires_at),
            }))
        }
        None => Ok(Json(ClaimAssignmentResponse {
            assignment_id: None,
            payload: None,
            lease_expires_at: None,
        })),
    }
}

async fn complete_assignment(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<CompleteAssignmentRequest>,
) -> Result<Json<AssignmentAckResponse>, ApiError> {
    validate_cluster_token(&headers)
        .map_err(|e| ApiError::Unauthorized(format!("cluster token validation failed: {}", e)))?;

    let node_id = NodeId::from_string(req.node_id.clone())
        .map_err(|e| ApiError::InvalidInput(format!("invalid node_id: {}", e)))?;
    let assignment_id = AssignmentId::from_string(id);

    if let Some(err) = &req.error {
        warn!(
            assignment_id = %assignment_id,
            node_id = %node_id,
            outcome = ?req.outcome,
            remote_error = %err,
            "worker reported non-success assignment outcome"
        );
    }

    state
        .assignment_queue
        .complete(&assignment_id, &node_id, req.outcome)
        .await
        .map_err(|e| ApiError::InvalidInput(format!("complete rejected: {}", e)))?;

    info!(
        assignment_id = %assignment_id,
        node_id = %node_id,
        outcome = ?req.outcome,
        "assignment completed"
    );
    Ok(Json(AssignmentAckResponse { ok: true }))
}

async fn release_assignment(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<ReleaseAssignmentRequest>,
) -> Result<Json<AssignmentAckResponse>, ApiError> {
    validate_cluster_token(&headers)
        .map_err(|e| ApiError::Unauthorized(format!("cluster token validation failed: {}", e)))?;

    let node_id = NodeId::from_string(req.node_id.clone())
        .map_err(|e| ApiError::InvalidInput(format!("invalid node_id: {}", e)))?;
    let assignment_id = AssignmentId::from_string(id);
    let reason = req.reason.as_deref().unwrap_or("worker-release");

    state
        .assignment_queue
        .release(&assignment_id, &node_id, reason)
        .await
        .map_err(|e| ApiError::InvalidInput(format!("release rejected: {}", e)))?;

    Ok(Json(AssignmentAckResponse { ok: true }))
}

async fn renew_assignment_lease(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<RenewAssignmentLeaseRequest>,
) -> Result<Json<RenewAssignmentLeaseResponse>, ApiError> {
    validate_cluster_token(&headers)
        .map_err(|e| ApiError::Unauthorized(format!("cluster token validation failed: {}", e)))?;

    let node_id = NodeId::from_string(req.node_id.clone())
        .map_err(|e| ApiError::InvalidInput(format!("invalid node_id: {}", e)))?;
    let assignment_id = AssignmentId::from_string(id);
    let ttl = resolve_lease_ttl(req.lease_ttl_secs);

    let lease_expires_at = state
        .assignment_queue
        .lease_renew(&assignment_id, &node_id, ttl)
        .await
        .map_err(|e| ApiError::InvalidInput(format!("renew rejected: {}", e)))?;

    Ok(Json(RenewAssignmentLeaseResponse { lease_expires_at }))
}

fn parse_execution_status(value: &str) -> anyhow::Result<ExecutionStatus> {
    match value.to_ascii_lowercase().as_str() {
        "pending" => Ok(ExecutionStatus::Pending),
        "running" => Ok(ExecutionStatus::Running),
        "waiting_review" | "waitingreview" => Ok(ExecutionStatus::WaitingReview),
        "waiting_approval" | "waitingapproval" => Ok(ExecutionStatus::WaitingApproval),
        "completed" => Ok(ExecutionStatus::Completed),
        "failed" => Ok(ExecutionStatus::Failed),
        "cancelled" => Ok(ExecutionStatus::Cancelled),
        "timed_out" | "timedout" => Ok(ExecutionStatus::TimedOut),
        other => anyhow::bail!("unknown execution status '{}'", other),
    }
}
