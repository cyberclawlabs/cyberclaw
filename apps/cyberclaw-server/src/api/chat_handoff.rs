//! Sprint 21 T10 — Handoff HTTP surface.
//!
//! Exposes the handoff lifecycle to the admin console:
//!
//! | Method | Path                                          | Auth  |
//! |--------|-----------------------------------------------|-------|
//! | GET    | `/api/v1/chat/handoff?limit=50`               | admin |
//! | GET    | `/api/v1/chat/handoff/:id`                    | admin |
//! | POST   | `/api/v1/chat/handoff/:id/accept`             | admin |
//! | POST   | `/api/v1/chat/handoff/:id/reject`             | admin |
//! | POST   | `/_dev/trigger_handoff` (debug_assertions only)| admin |
//!
//! # RBAC
//!
//! All endpoints require `require_admin()`. Handoff decisions affect active
//! sessions and must not be accessible by viewers.
//!
//! # Architectural compliance
//!
//! - Does NOT modify the queue directly for status transitions; calls
//!   `queue.update_status()` through the `AppState.handoff_queue` field.
//! - The executor hook (`execution_service.complete_handoff`, T4) is called
//!   after `accept` transitions to `Authorized` — wired in a follow-up task.

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::warn;

use cyberclaw_core::handoff::{HandoffError, HandoffRequest, HandoffStatus};
use cyberclaw_core::ids::HandoffId;
use cyberclaw_observability::events::{EventRecorder as _, ObservabilityEvent};

use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::{require_admin, Claims};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

/// Query params for `GET /api/v1/chat/handoff?limit=N`.
#[derive(Debug, Deserialize)]
pub struct ListHandoffsQuery {
    /// Maximum number of records to return (default 50, max capped at 200).
    pub limit: Option<usize>,
}

/// Response body for `GET /api/v1/chat/handoff`.
#[derive(Debug, Serialize)]
pub struct ListHandoffsResponse {
    pub handoffs: Vec<HandoffRequest>,
}

/// Body for `POST /api/v1/chat/handoff/:id/reject`.
#[derive(Debug, Deserialize)]
pub struct RejectHandoffBody {
    pub reason: String,
}

/// Response body for accept / reject operations.
#[derive(Debug, Serialize)]
pub struct HandoffActionResponse {
    pub handoff_id: String,
    pub status: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map a [`HandoffError`] to the appropriate [`ApiError`] variant.
fn handoff_err_to_api(e: HandoffError) -> ApiError {
    match e {
        HandoffError::NotFound => ApiError::NotFound("Handoff not found".to_string()),
        HandoffError::SelfHandoff => {
            ApiError::InvalidRequest("self-handoff is not allowed".to_string())
        }
        HandoffError::Validation(msg) => ApiError::InvalidRequest(msg),
        HandoffError::Denied(msg) => ApiError::Forbidden(msg),
        HandoffError::TargetNotFound(msg) => {
            ApiError::InvalidRequest(format!("target agent not found: {}", msg))
        }
        HandoffError::Expired => ApiError::InvalidRequest("handoff has expired".to_string()),
        HandoffError::Internal(msg) => ApiError::InternalError(msg),
    }
}

/// Parse a `HandoffId` from a path string, returning `ApiError::InvalidRequest` on failure.
fn parse_handoff_id(s: &str) -> Result<HandoffId, ApiError> {
    HandoffId::from_string(s.to_string())
        .map_err(|e| ApiError::InvalidRequest(format!("invalid handoff_id: {e}")))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/v1/chat/handoff?limit=50
///
/// Admin-only. Returns recent handoff requests ordered by `initiated_at` desc.
pub async fn list_handoffs(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Query(params): Query<ListHandoffsQuery>,
) -> Result<Json<ListHandoffsResponse>, ApiError> {
    require_admin(&claims).await?;

    let limit = params.limit.unwrap_or(50).min(200);
    let handoffs = state.handoff_queue.list_recent(limit).await;

    Ok(Json(ListHandoffsResponse { handoffs }))
}

/// GET /api/v1/chat/handoff/:id
///
/// Admin-only. Returns the full [`HandoffRequest`] for a given id.
pub async fn get_handoff(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(id_str): Path<String>,
) -> Result<Json<HandoffRequest>, ApiError> {
    require_admin(&claims).await?;

    let id = parse_handoff_id(&id_str)?;
    let req = state
        .handoff_queue
        .get(&id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Handoff not found: {}", id_str)))?;

    Ok(Json(req))
}

/// POST /api/v1/chat/handoff/:id/accept
///
/// Admin-only. Accept flow (S21 T6). Transitions to `Authorized`,
/// sets `conversation.active_agent_id`, appends a handoff card, transitions to
/// `Accepted`, and returns HTTP 200 `{status: "accepted"}`.
pub async fn accept_handoff(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(id_str): Path<String>,
) -> Result<(StatusCode, Json<HandoffActionResponse>), ApiError> {
    require_admin(&claims).await?;

    // T20 — feature flag guard
    if !state.handoff_feature_enabled {
        return Err(ApiError::InvalidRequest(
            "handoff feature disabled".to_string(),
        ));
    }

    let id = parse_handoff_id(&id_str)?;

    // Read current state.
    let existing = state
        .handoff_queue
        .get(&id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Handoff not found: {}", id_str)))?;

    // Guard: only Initiated or Authorized may proceed; anything else is an error.
    match existing.status {
        HandoffStatus::Initiated => {
            // Advance to Authorized first.
            state
                .handoff_queue
                .update_status(&id, HandoffStatus::Authorized)
                .await
                .map_err(handoff_err_to_api)?;

            // A1.1 — emit HandoffAuthorized (Initiated → Authorized transition, best-effort).
            let _ = state
                .event_recorder
                .record_event(ObservabilityEvent::HandoffAuthorized {
                    handoff_id: id.clone(),
                    timestamp: Utc::now(),
                })
                .await;
        }
        HandoffStatus::Authorized => {
            // Already authorized — proceed below.
        }
        HandoffStatus::Accepted => {
            // Already fully accepted — idempotent success.
            return Ok((
                StatusCode::OK,
                Json(HandoffActionResponse {
                    handoff_id: id_str,
                    status: "accepted".to_string(),
                }),
            ));
        }
        other => {
            return Err(ApiError::InvalidRequest(format!(
                "cannot accept handoff in state {:?}",
                other
            )));
        }
    }

    finalize_handoff_accept(&state, &id, &id_str, &claims).await?;

    Ok((
        StatusCode::OK,
        Json(HandoffActionResponse {
            handoff_id: id_str,
            status: "accepted".to_string(),
        }),
    ))
}

/// Shared finalization logic for `accept_handoff`.
///
/// Precondition: the handoff is in `Authorized` state.
/// Postcondition: active_agent set, handoff card appended, status = `Accepted`,
/// audit entry emitted.
async fn finalize_handoff_accept(
    state: &AppState,
    id: &HandoffId,
    id_str: &str,
    claims: &Claims,
) -> Result<(), ApiError> {
    // Re-read to get the freshest snapshot (decided_at may have been set).
    let req = state.handoff_queue.get(id).await.ok_or_else(|| {
        ApiError::InternalError("handoff disappeared after status update".to_string())
    })?;

    // Set the active agent on the conversation.
    let conv_store = state.conversation_store();
    conv_store
        .set_active_agent(&req.conversation_id, req.to_agent_id.clone())
        .await
        .map_err(|e| ApiError::InternalError(format!("set_active_agent: {e}")))?;

    // Append handoff card to the conversation.
    let card = crate::api::chat_conversations::ChatMessage::handoff_card(&req);
    conv_store
        .append_message_internal(&req.conversation_id, card)
        .await
        .map_err(|e| ApiError::InternalError(format!("append handoff card: {e}")))?;

    // Transition queue entry to Accepted.
    state
        .handoff_queue
        .update_status(id, HandoffStatus::Accepted)
        .await
        .map_err(handoff_err_to_api)?;

    // T17 — emit audit entry for accept mutation.
    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            claims.sub.to_string(),
            AuditKind::Mutation,
            "handoff.accept",
            Some(format!("handoff:{}", id_str)),
            serde_json::json!({
                "handoff_id": req.handoff_id,
                "from": req.from_agent_id,
                "to": req.to_agent_id,
                "reason": req.reason,
            }),
            AuditResult::Success,
        ))
        .await;
    }

    Ok(())
}

/// POST /api/v1/chat/handoff/:id/reject
///
/// Admin-only. Transitions the handoff status to `Declined`.
/// Body: `{reason: String}` — propagated to audit log.
pub async fn reject_handoff(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(id_str): Path<String>,
    Json(body): Json<RejectHandoffBody>,
) -> Result<Json<HandoffActionResponse>, ApiError> {
    require_admin(&claims).await?;

    // T20 — feature flag guard
    if !state.handoff_feature_enabled {
        return Err(ApiError::InvalidRequest(
            "handoff feature disabled".to_string(),
        ));
    }

    let id = parse_handoff_id(&id_str)?;

    // Verify existence.
    let _ = state
        .handoff_queue
        .get(&id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Handoff not found: {}", id_str)))?;

    state
        .handoff_queue
        .update_status(&id, HandoffStatus::Declined)
        .await
        .map_err(handoff_err_to_api)?;

    // Sanitize reason text before logging / persisting in audit detail.
    // The reason is operator-supplied free text and could carry a
    // credential or injection payload that we don't want to ship into
    // the audit log verbatim.
    let sanitized_reason = state
        .memory_sanitizer
        .sanitize_and_redact("handoff.reject_reason", &body.reason);
    if !sanitized_reason.warnings.is_empty() {
        if let Some(sink) = state.audit.as_ref() {
            sink.record_sanitizer_warnings(
                claims.sub.to_string(),
                "handoff.reject_reason",
                Some(format!("handoff:{}", id_str)),
                &sanitized_reason.warnings,
            )
            .await;
        }
    }
    let safe_reason = sanitized_reason.content;

    warn!(
        handoff_id = %id_str,
        reason = %safe_reason,
        admin = %claims.sub,
        "handoff rejected by admin"
    );

    // T17 — emit audit entry for reject mutation.
    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            claims.sub.to_string(),
            AuditKind::Mutation,
            "handoff.reject",
            Some(format!("handoff:{}", id_str)),
            serde_json::json!({
                "handoff_id": id_str,
                "reason_input": safe_reason,
            }),
            AuditResult::Success,
        ))
        .await;
    }

    Ok(Json(HandoffActionResponse {
        handoff_id: id_str,
        status: "declined".to_string(),
    }))
}

// ---------------------------------------------------------------------------
// Router factory
// ---------------------------------------------------------------------------

/// Build the handoff router.
///
/// The `/_dev/trigger_handoff` route is only compiled and registered when
/// `debug_assertions` are enabled (i.e. `cargo test` and debug builds).
pub fn create_handoff_router() -> Router<Arc<AppState>> {
    // `mut` is only used inside the `#[cfg(debug_assertions)]` block below.
    // In release builds the `r =` reassignment is compiled out, so clippy
    // would warn `unused_mut`. The cfg_attr keeps the binding mut in debug
    // (where the dev route is appended) and silences the warning in release.
    #[cfg_attr(not(debug_assertions), allow(unused_mut))]
    let mut r = Router::new()
        .route("/api/v1/chat/handoff", get(list_handoffs))
        .route("/api/v1/chat/handoff/:id", get(get_handoff))
        .route("/api/v1/chat/handoff/:id/accept", post(accept_handoff))
        .route("/api/v1/chat/handoff/:id/reject", post(reject_handoff));
    #[cfg(debug_assertions)]
    {
        r = r.route("/_dev/trigger_handoff", post(dev::trigger_handoff_dev));
    }
    r
}

// ---------------------------------------------------------------------------
// Dev-only trigger endpoint
// ---------------------------------------------------------------------------
//
// `POST /_dev/trigger_handoff` is only compiled when `debug_assertions` are
// enabled. It constructs a HandoffRequest from the request body, validates it,
// and enqueues it — making integration tests possible without a real agent loop.

#[cfg(any(debug_assertions, test))]
pub mod dev {
    use super::*;
    use cyberclaw_core::artifact::ArtifactRef;
    use cyberclaw_core::ids::AgentId;

    /// Request body for the dev trigger endpoint.
    #[derive(Debug, Deserialize)]
    pub struct TriggerHandoffBody {
        pub from_agent_id: String,
        pub to_agent_id: String,
        pub conversation_id: String,
        pub reason: String,
        pub briefing_text: String,
        pub briefing_text_optional: Option<String>,
        pub context_artifacts: Option<Vec<ArtifactRef>>,
    }

    /// Response body from the dev trigger endpoint.
    #[derive(Debug, Serialize)]
    pub struct TriggerHandoffResponse {
        pub handoff_id: String,
        pub status: String,
    }

    /// POST /_dev/trigger_handoff
    ///
    /// Admin-only. Constructs a HandoffRequest server-side, validates it, and
    /// enqueues it. Used by integration tests to seed the queue without a
    /// real agent loop.
    pub async fn trigger_handoff_dev(
        State(state): State<Arc<AppState>>,
        Extension(claims): Extension<Claims>,
        Json(body): Json<TriggerHandoffBody>,
    ) -> Result<Json<TriggerHandoffResponse>, ApiError> {
        require_admin(&claims).await?;

        // T20 — feature flag guard
        if !state.handoff_feature_enabled {
            return Err(ApiError::InvalidRequest(
                "handoff feature disabled".to_string(),
            ));
        }

        // Snapshot string fields up-front for the audit emission below — the
        // HandoffRequest constructor consumes them by value.
        let audit_from = body.from_agent_id.clone();
        let audit_to = body.to_agent_id.clone();
        let audit_reason = body.reason.clone();

        let from_agent_id = AgentId::from_string(body.from_agent_id)
            .map_err(|e| ApiError::InvalidRequest(format!("invalid from_agent_id: {e}")))?;
        let to_agent_id = AgentId::from_string(body.to_agent_id)
            .map_err(|e| ApiError::InvalidRequest(format!("invalid to_agent_id: {e}")))?;

        // Use optional override briefing text if provided.
        let raw_briefing = body.briefing_text_optional.unwrap_or(body.briefing_text);

        let handoff_id =
            HandoffId::from_string(format!("ho_dev_{}", uuid::Uuid::new_v4().simple()))
                .map_err(|e| ApiError::InternalError(format!("id gen failed: {e}")))?;

        // Sanitize briefing + reason before constructing the
        // HandoffRequest. The briefing text is replayed to the
        // receiving agent — exactly the surface a malicious source
        // would target with a "ignore previous instructions" payload
        // or a leaked credential. Use sanitize_and_redact so the
        // payload is neutered (not blocked) and warnings hit the
        // audit log under sanitizer.<category>.
        let sanitized_briefing = state
            .memory_sanitizer
            .sanitize_and_redact("handoff.briefing", &raw_briefing);
        let sanitized_reason = state
            .memory_sanitizer
            .sanitize_and_redact("handoff.reason", &audit_reason);
        if let Some(sink) = state.audit.as_ref() {
            if !sanitized_briefing.warnings.is_empty() {
                sink.record_sanitizer_warnings(
                    claims.sub.to_string(),
                    "handoff.briefing",
                    Some(format!("handoff:{}", handoff_id.as_str())),
                    &sanitized_briefing.warnings,
                )
                .await;
            }
            if !sanitized_reason.warnings.is_empty() {
                sink.record_sanitizer_warnings(
                    claims.sub.to_string(),
                    "handoff.reason",
                    Some(format!("handoff:{}", handoff_id.as_str())),
                    &sanitized_reason.warnings,
                )
                .await;
            }
        }

        let req = HandoffRequest::new(
            handoff_id.clone(),
            from_agent_id,
            to_agent_id,
            body.conversation_id,
            sanitized_reason.content,
            sanitized_briefing.content,
            body.context_artifacts.unwrap_or_default(),
            None,
            Utc::now(),
        );

        req.validate()
            .map_err(|e| ApiError::InvalidRequest(format!("invalid handoff request: {e}")))?;

        state
            .handoff_queue
            .enqueue(req)
            .await
            .map_err(handoff_err_to_api)?;

        // A1.1 — emit HandoffInitiated observability event (best-effort).
        let _ = state
            .event_recorder
            .record_event(ObservabilityEvent::HandoffInitiated {
                handoff_id: handoff_id.clone(),
                from_agent_id: audit_from.clone(),
                to_agent_id: audit_to.clone(),
                timestamp: Utc::now(),
            })
            .await;

        // T17 — emit audit entry for trigger mutation.
        if let Some(sink) = state.audit.as_ref() {
            sink.record(super::AuditEntry::now(
                claims.sub.to_string(),
                super::AuditKind::Mutation,
                "handoff.trigger",
                Some(format!("handoff:{}", handoff_id)),
                serde_json::json!({
                    "handoff_id": handoff_id,
                    "from": audit_from,
                    "to": audit_to,
                    "reason": audit_reason,
                }),
                super::AuditResult::Success,
            ))
            .await;
        }

        Ok(Json(TriggerHandoffResponse {
            handoff_id: handoff_id.to_string(),
            status: "initiated".to_string(),
        }))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_helpers::{build_test_state, seed_users_with_role};
    use axum::extract::{Extension, Path, Query, State};
    use cyberclaw_core::ids::{AgentId, HandoffId};
    use serial_test::serial;

    fn claims_for(user_id: &str) -> Claims {
        use cyberclaw_core::ids::UserId;
        let uid = UserId::from_string(user_id.to_string()).expect("valid user id");
        let now = chrono::Utc::now().timestamp();
        Claims {
            sub: uid,
            tenant: None,
            iat: now,
            exp: now + 3600,
        }
    }

    fn make_handoff_request(from: &str, to: &str, conv: &str) -> HandoffRequest {
        HandoffRequest::new(
            HandoffId::from_string(format!("ho_{}", uuid::Uuid::new_v4().simple())).unwrap(),
            AgentId::from_string(from.to_string()).unwrap(),
            AgentId::from_string(to.to_string()).unwrap(),
            conv.to_string(),
            "needs backend expertise".to_string(),
            "brief context summary".to_string(),
            vec![],
            None,
            Utc::now(),
        )
    }

    // ── Test 1: list admin-only — viewer gets 403 ────────────────────────────

    #[tokio::test]
    #[serial]
    async fn list_admin_only_rejects_viewer() {
        let (_tmp, _restore) = seed_users_with_role("viewer-handoff-list", "viewer");
        let state = build_test_state();

        let claims = claims_for("viewer-handoff-list");
        let err = list_handoffs(
            State(state),
            Extension(claims),
            Query(ListHandoffsQuery { limit: None }),
        )
        .await
        .expect_err("should be forbidden");

        assert!(matches!(err, ApiError::Forbidden(_)), "got {:?}", err);
    }

    // ── Test 2: list happy path (empty queue) ────────────────────────────────

    #[tokio::test]
    #[serial]
    async fn list_happy_path_empty() {
        let (_tmp, _restore) = seed_users_with_role("admin-handoff-list-empty", "admin");
        let state = build_test_state();

        let claims = claims_for("admin-handoff-list-empty");
        let result = list_handoffs(
            State(state),
            Extension(claims),
            Query(ListHandoffsQuery { limit: Some(50) }),
        )
        .await
        .unwrap();

        assert!(
            result.0.handoffs.is_empty(),
            "empty queue should return empty list"
        );
    }

    // ── Test 3: list happy path after trigger (2 items) ─────────────────────

    #[tokio::test]
    #[serial]
    async fn list_happy_path_after_trigger() {
        let (_tmp, _restore) = seed_users_with_role("admin-handoff-list-filled", "admin");
        let state = build_test_state();

        // Seed two handoffs directly into the queue.
        let r1 = make_handoff_request("agent_a", "agent_b", "conv_1");
        let r2 = make_handoff_request("agent_c", "agent_d", "conv_2");
        state.handoff_queue.enqueue(r1).await.unwrap();
        state.handoff_queue.enqueue(r2).await.unwrap();

        let claims = claims_for("admin-handoff-list-filled");
        let result = list_handoffs(
            State(state),
            Extension(claims),
            Query(ListHandoffsQuery { limit: Some(50) }),
        )
        .await
        .unwrap();

        assert_eq!(result.0.handoffs.len(), 2, "should have 2 handoffs");
    }

    // ── Test 4: get — 404 for unknown id ────────────────────────────────────

    #[tokio::test]
    #[serial]
    async fn get_404_for_unknown() {
        let (_tmp, _restore) = seed_users_with_role("admin-handoff-get-404", "admin");
        let state = build_test_state();

        let claims = claims_for("admin-handoff-get-404");
        let err = get_handoff(
            State(state),
            Extension(claims),
            Path("ho_does_not_exist".to_string()),
        )
        .await
        .expect_err("should be not found");

        assert!(matches!(err, ApiError::NotFound(_)), "got {:?}", err);
    }

    // ── Test 5: get admin-only — viewer gets 403 ────────────────────────────

    #[tokio::test]
    #[serial]
    async fn get_admin_only_rejects_viewer() {
        let (_tmp, _restore) = seed_users_with_role("viewer-handoff-get", "viewer");
        let state = build_test_state();

        let claims = claims_for("viewer-handoff-get");
        let err = get_handoff(State(state), Extension(claims), Path("ho_any".to_string()))
            .await
            .expect_err("should be forbidden");

        assert!(matches!(err, ApiError::Forbidden(_)), "got {:?}", err);
    }

    // ── Test 6: accept happy path → status Authorized ────────────────────────

    #[tokio::test]
    #[serial]
    async fn accept_happy_path() {
        let (_tmp, _restore) = seed_users_with_role("admin-handoff-accept", "admin");
        crate::api::chat_conversations::reset_store_for_tests().await;
        let state = build_test_state();

        // Seed the conversation so set_active_agent succeeds.
        crate::api::chat_conversations::store()
            .create_for_test("conv_accept", "alice")
            .await;

        let req = make_handoff_request("agent_a", "agent_b", "conv_accept");
        let id_str = req.handoff_id.to_string();
        state.handoff_queue.enqueue(req).await.unwrap();

        let claims = claims_for("admin-handoff-accept");
        let result = accept_handoff(
            State(state.clone()),
            Extension(claims),
            Path(id_str.clone()),
        )
        .await
        .unwrap();

        // accept_handoff now completes the full flow → Accepted.
        assert_eq!(result.1 .0.status, "accepted");
        assert_eq!(result.1 .0.handoff_id, id_str);

        // Verify in queue.
        let id = HandoffId::from_string(id_str).unwrap();
        let stored = state.handoff_queue.get(&id).await.unwrap();
        assert_eq!(stored.status, HandoffStatus::Accepted);
    }

    // ── Test 7: reject happy path → status Declined ──────────────────────────

    #[tokio::test]
    #[serial]
    async fn reject_happy_path() {
        let (_tmp, _restore) = seed_users_with_role("admin-handoff-reject", "admin");
        let state = build_test_state();

        let req = make_handoff_request("agent_a", "agent_b", "conv_reject");
        let id_str = req.handoff_id.to_string();
        state.handoff_queue.enqueue(req).await.unwrap();

        let claims = claims_for("admin-handoff-reject");
        let result = reject_handoff(
            State(state.clone()),
            Extension(claims),
            Path(id_str.clone()),
            Json(RejectHandoffBody {
                reason: "not appropriate for this agent".to_string(),
            }),
        )
        .await
        .unwrap();

        assert_eq!(result.0.status, "declined");
        assert_eq!(result.0.handoff_id, id_str);

        // Verify in queue.
        let id = HandoffId::from_string(id_str).unwrap();
        let stored = state.handoff_queue.get(&id).await.unwrap();
        assert_eq!(stored.status, HandoffStatus::Declined);
    }

    // ── Test 8: trigger dev — validation fails on self-handoff → 400 ─────────

    #[tokio::test]
    #[serial]
    async fn trigger_handoff_validation_fails_on_self_handoff() {
        let (_tmp, _restore) = seed_users_with_role("admin-handoff-trigger-self", "admin");
        let state = build_test_state();

        let claims = claims_for("admin-handoff-trigger-self");
        let err = dev::trigger_handoff_dev(
            State(state),
            Extension(claims),
            Json(dev::TriggerHandoffBody {
                from_agent_id: "agent_same".to_string(),
                to_agent_id: "agent_same".to_string(), // self-handoff
                conversation_id: "conv_self".to_string(),
                reason: "testing self handoff rejection".to_string(),
                briefing_text: "some briefing".to_string(),
                briefing_text_optional: None,
                context_artifacts: None,
            }),
        )
        .await
        .expect_err("should fail validation");

        assert!(
            matches!(err, ApiError::InvalidRequest(_)),
            "expected InvalidRequest, got {:?}",
            err
        );
    }

    // ── Sprint 21 T17 + T20 tests ───────────────────────────────────────────

    /// Helper: build a state with the feature flag enabled.
    fn build_state_with_flag(enabled: bool) -> Arc<AppState> {
        let mut s = (*build_test_state()).clone();
        s.handoff_feature_enabled = enabled;
        Arc::new(s)
    }

    /// T20: feature flag disabled → trigger_handoff_dev returns InvalidRequest.
    #[tokio::test]
    #[serial]
    async fn feature_flag_disabled_blocks_trigger() {
        let (_tmp, _restore) = seed_users_with_role("admin-ff-trigger", "admin");
        let state = build_state_with_flag(false);
        let claims = claims_for("admin-ff-trigger");

        let err = dev::trigger_handoff_dev(
            State(state),
            Extension(claims),
            Json(dev::TriggerHandoffBody {
                from_agent_id: "agent_a".to_string(),
                to_agent_id: "agent_b".to_string(),
                conversation_id: "conv_ff".to_string(),
                reason: "test flag".to_string(),
                briefing_text: "brief".to_string(),
                briefing_text_optional: None,
                context_artifacts: None,
            }),
        )
        .await
        .expect_err("should be blocked by feature flag");

        assert!(
            matches!(err, ApiError::InvalidRequest(_)),
            "expected InvalidRequest, got {:?}",
            err
        );
    }

    /// T20: feature flag disabled → accept_handoff returns InvalidRequest.
    #[tokio::test]
    #[serial]
    async fn feature_flag_disabled_blocks_accept() {
        let (_tmp, _restore) = seed_users_with_role("admin-ff-accept", "admin");
        let state = build_state_with_flag(false);

        // Seed a handoff directly bypassing the flag (queue access is direct).
        let req = make_handoff_request("agent_a", "agent_b", "conv_ff_accept");
        let id_str = req.handoff_id.to_string();
        state.handoff_queue.enqueue(req).await.unwrap();

        let claims = claims_for("admin-ff-accept");
        let err = accept_handoff(State(state), Extension(claims), Path(id_str))
            .await
            .expect_err("should be blocked by feature flag");

        assert!(
            matches!(err, ApiError::InvalidRequest(_)),
            "expected InvalidRequest, got {:?}",
            err
        );
    }

    /// T20: feature flag enabled → accept still works (sanity).
    #[tokio::test]
    #[serial]
    async fn feature_flag_enabled_allows_accept() {
        let (_tmp, _restore) = seed_users_with_role("admin-ff-ok", "admin");
        crate::api::chat_conversations::reset_store_for_tests().await;
        let state = build_state_with_flag(true);

        let conv_id = "conv_ff_ok";
        crate::api::chat_conversations::store()
            .create_for_test(conv_id, "alice")
            .await;

        let req = make_handoff_request("agent_from", "agent_to", conv_id);
        let id_str = req.handoff_id.to_string();
        state.handoff_queue.enqueue(req).await.unwrap();

        let claims = claims_for("admin-ff-ok");
        let result = accept_handoff(State(state), Extension(claims), Path(id_str))
            .await
            .unwrap();

        assert_eq!(result.1 .0.status, "accepted");
    }

    /// T17: audit entry emitted on accept (with audit sink wired).
    #[tokio::test]
    #[serial]
    async fn audit_entry_emitted_on_accept() {
        let (_tmp, _restore) = seed_users_with_role("admin-audit-accept", "admin");
        crate::api::chat_conversations::reset_store_for_tests().await;

        // Build state with feature enabled + a real audit sink.
        let audit_tmp = tempfile::tempdir().expect("audit tempdir");
        let sink = Arc::new(
            crate::audit::AuditSink::new(audit_tmp.path().join("audit.db"))
                .await
                .expect("audit sink"),
        );
        let state = {
            let mut s = (*build_test_state()).clone();
            s.handoff_feature_enabled = true;
            s.audit = Some(sink.clone());
            Arc::new(s)
        };

        let conv_id = "conv_audit_accept";
        crate::api::chat_conversations::store()
            .create_for_test(conv_id, "alice")
            .await;

        let req = make_handoff_request("agent_from_a", "agent_to_a", conv_id);
        let id_str = req.handoff_id.to_string();
        state.handoff_queue.enqueue(req).await.unwrap();

        let claims = claims_for("admin-audit-accept");
        let _ = accept_handoff(State(state), Extension(claims), Path(id_str.clone()))
            .await
            .unwrap();

        // Inspect audit log — should contain a "handoff.accept" entry.
        let rows = sink
            .tail(50, &crate::audit::AuditQuery::default())
            .await
            .expect("audit tail");
        let handoff_entries: Vec<_> = rows
            .iter()
            .filter(|r| r.action.contains("handoff"))
            .collect();
        assert!(
            !handoff_entries.is_empty(),
            "expected at least one handoff audit entry, got: {:?}",
            rows.iter().map(|r| &r.action).collect::<Vec<_>>()
        );
        assert_eq!(handoff_entries[0].action, "handoff.accept");
    }

    // ── Sprint 21 T6 tests ───────────────────────────────────────────────────

    /// Helper: seed a conversation into the process-wide store.
    async fn seed_conversation(conv_id: &str) {
        let s = crate::api::chat_conversations::store();
        s.create_for_test(conv_id, "alice").await;
    }

    // ── T6 Test 1: full accept flow ──────────────────────────────────────────

    #[tokio::test]
    #[serial]
    async fn accept_handoff_full_flow_sets_active_agent_and_appends_card() {
        let (_tmp, _restore) = seed_users_with_role("admin-t6-full", "admin");
        crate::api::chat_conversations::reset_store_for_tests().await;
        let state = build_test_state();

        let conv_id = "conv_t6_full";
        seed_conversation(conv_id).await;

        let req = make_handoff_request("agent_from", "agent_to", conv_id);
        let id_str = req.handoff_id.to_string();
        state.handoff_queue.enqueue(req).await.unwrap();

        let claims = claims_for("admin-t6-full");
        let result = accept_handoff(
            State(state.clone()),
            Extension(claims),
            Path(id_str.clone()),
        )
        .await
        .unwrap();

        assert_eq!(result.1 .0.status, "accepted");

        // Queue entry must be Accepted.
        let hid = HandoffId::from_string(id_str.clone()).unwrap();
        let stored = state.handoff_queue.get(&hid).await.unwrap();
        assert_eq!(stored.status, HandoffStatus::Accepted);

        // Conversation must have active_agent_id set.
        let conv_store = state.conversation_store();
        let conv = conv_store.get(conv_id).await.expect("conversation exists");
        assert_eq!(
            conv.active_agent_id.as_ref().map(|a| a.to_string()),
            Some("agent_to".to_string()),
            "active_agent_id must be set to to_agent_id"
        );

        // Conversation must have exactly 1 handoff card message.
        assert_eq!(conv.messages.len(), 1, "handoff card must be appended");
        assert_eq!(
            conv.messages[0].role, "handoff",
            "appended message must have role=handoff"
        );
    }

    // ── T6 Test 2: second accept call returns 400 ────────────────────────────

    #[tokio::test]
    #[serial]
    async fn accept_handoff_idempotent_on_accepted_state() {
        let (_tmp, _restore) = seed_users_with_role("admin-t6-idem", "admin");
        crate::api::chat_conversations::reset_store_for_tests().await;
        let state = build_test_state();

        let conv_id = "conv_t6_idem";
        seed_conversation(conv_id).await;

        let req = make_handoff_request("agent_from2", "agent_to2", conv_id);
        let id_str = req.handoff_id.to_string();
        state.handoff_queue.enqueue(req).await.unwrap();

        let claims_first = claims_for("admin-t6-idem");
        // First accept succeeds.
        let _ = accept_handoff(
            State(state.clone()),
            Extension(claims_first),
            Path(id_str.clone()),
        )
        .await
        .unwrap();

        // Second accept returns "accepted" idempotently (Accepted → Accepted branch).
        let claims_second = claims_for("admin-t6-idem");
        let result = accept_handoff(
            State(state.clone()),
            Extension(claims_second),
            Path(id_str.clone()),
        )
        .await
        .unwrap();
        assert_eq!(
            result.1 .0.status, "accepted",
            "second call on already-Accepted must succeed idempotently"
        );
    }

    // ── T6 Test 3: accept from Expired state → 400 ───────────────────────────

    #[tokio::test]
    #[serial]
    async fn accept_handoff_from_expired_state_rejected() {
        let (_tmp, _restore) = seed_users_with_role("admin-t6-expired", "admin");
        let state = build_test_state();

        let req = make_handoff_request("agent_a", "agent_b", "conv_t6_expired");
        let id_str = req.handoff_id.to_string();
        let hid = req.handoff_id.clone();
        state.handoff_queue.enqueue(req).await.unwrap();

        // Force to Expired state.
        state
            .handoff_queue
            .update_status(&hid, HandoffStatus::Expired)
            .await
            .unwrap();

        let claims = claims_for("admin-t6-expired");
        let err = accept_handoff(
            State(state.clone()),
            Extension(claims),
            Path(id_str.clone()),
        )
        .await
        .expect_err("Expired handoff must be rejected");

        assert!(
            matches!(err, ApiError::InvalidRequest(_)),
            "expected InvalidRequest for Expired state, got {:?}",
            err
        );
    }

    // ── A1.1 tests — HandoffInitiated / HandoffAuthorized emit sites ────────────

    /// A1.1 Test 1: trigger_handoff_dev emits HandoffInitiated into event_recorder.
    #[tokio::test]
    #[serial]
    async fn trigger_handoff_emits_handoff_initiated() {
        use cyberclaw_observability::events::EventRecorder as _;

        let (_tmp, _restore) = seed_users_with_role("admin-a11-initiated", "admin");
        let state = build_test_state();

        let claims = claims_for("admin-a11-initiated");
        let _ = dev::trigger_handoff_dev(
            State(state.clone()),
            Extension(claims),
            Json(dev::TriggerHandoffBody {
                from_agent_id: "agent_from_a11".to_string(),
                to_agent_id: "agent_to_a11".to_string(),
                conversation_id: "conv_a11_initiated".to_string(),
                reason: "a11 observability test".to_string(),
                briefing_text: "brief".to_string(),
                briefing_text_optional: None,
                context_artifacts: None,
            }),
        )
        .await
        .unwrap();

        let events = state.event_recorder.get_events().await.unwrap_or_default();
        let has_initiated = events.iter().any(|e| {
            matches!(e, ObservabilityEvent::HandoffInitiated {
                from_agent_id,
                to_agent_id,
                ..
            } if from_agent_id == "agent_from_a11" && to_agent_id == "agent_to_a11")
        });
        assert!(
            has_initiated,
            "HandoffInitiated event must be emitted after trigger; got: {:?}",
            events
                .iter()
                .map(std::mem::discriminant)
                .collect::<Vec<_>>()
        );
    }

    /// A1.1 Test 2: accept_handoff (Initiated → Authorized transition) emits HandoffAuthorized.
    #[tokio::test]
    #[serial]
    async fn accept_handoff_emits_handoff_authorized() {
        use cyberclaw_observability::events::EventRecorder as _;

        let (_tmp, _restore) = seed_users_with_role("admin-a11-authorized", "admin");
        crate::api::chat_conversations::reset_store_for_tests().await;
        let state = build_test_state();

        let conv_id = "conv_a11_authorized";
        crate::api::chat_conversations::store()
            .create_for_test(conv_id, "alice")
            .await;

        let req = make_handoff_request("agent_from_auth", "agent_to_auth", conv_id);
        let id_str = req.handoff_id.to_string();
        state.handoff_queue.enqueue(req).await.unwrap();

        let claims = claims_for("admin-a11-authorized");
        let _ = accept_handoff(
            State(state.clone()),
            Extension(claims),
            Path(id_str.clone()),
        )
        .await
        .unwrap();

        let events = state.event_recorder.get_events().await.unwrap_or_default();
        let has_authorized = events
            .iter()
            .any(|e| matches!(e, ObservabilityEvent::HandoffAuthorized { .. }));
        assert!(
            has_authorized,
            "HandoffAuthorized event must be emitted on Initiated→Authorized transition; got: {:?}",
            events
                .iter()
                .map(std::mem::discriminant)
                .collect::<Vec<_>>()
        );
    }
}
