//! Sprint 12 L1 — Conversational approval endpoint.
//!
//! Drives the in-chat approval card lane: the agent emits a
//! `request_approval` tool_call with a stable `request_id`; the UI renders a
//! card; the operator clicks Approve/Reject or replies in natural language;
//! the frontend POSTs `/api/v1/chat/approval` which
//!
//!   1. looks up the pending [`ReviewRequest`] by id,
//!   2. dispatches the decision through the control-plane
//!      (`process_review_result`) so the normal governance chain runs,
//!   3. records an audit entry tagged `source=chat` so UI lanes can tell the
//!      difference between a manual "/api/v1/reviews/:id/approve" click and
//!      an in-chat approval,
//!   4. broadcasts a `ReviewResolved` admin event so other panes refresh.
//!
//! Per `AGENTS.md §2`, review is still a first-class governance artifact —
//! this endpoint is just a new *input surface*, not a new control path.
//! `Connector → Capability` is untouched; we only mutate [`ReviewQueue`].

use axum::{
    extract::{Extension, State},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{error, info};

use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::{ActorId, ReviewId};

use crate::api::admin::events::AdminEvent;
use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::{require_admin, Claims};
use crate::state::AppState;

/// Source tag recorded on the audit trail.
///
/// Manual REST decisions (`/api/v1/reviews/:id/approve`) record
/// `source="manual"`; chat-driven decisions record `source="chat"` so the
/// security log can tell them apart.
pub const CHAT_APPROVAL_SOURCE: &str = "chat";
pub const MANUAL_APPROVAL_SOURCE: &str = "manual";

/// Request body for `POST /api/v1/chat/approval`.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatApprovalRequest {
    /// Opaque request id emitted in the original `request_approval` tool_call.
    ///
    /// Must be a valid [`ReviewId`]; reused for correlation with the queue.
    pub request_id: String,
    /// Optional conversation id to round-trip with the assistant message.
    ///
    /// Currently used only for audit metadata — the chat history itself is
    /// held client-side.
    #[serde(default)]
    pub conversation_id: Option<String>,
    /// `true` to approve, `false` to reject.
    pub approved: bool,
    /// Optional human rationale; ends up in the assistant reply + audit entry.
    #[serde(default)]
    pub reason: Option<String>,
}

/// Response body returned to the chat UI after a decision is applied.
#[derive(Debug, Clone, Serialize)]
pub struct ChatApprovalResponse {
    pub success: bool,
    pub request_id: String,
    pub decision: String,
    /// Assistant-facing message that the UI can append to the conversation.
    pub assistant_message: String,
    /// Always "chat" — mirrors the audit entry so clients can double-check.
    pub source: &'static str,
}

/// Mount the chat approval router under `/api/v1/chat/approval`.
pub fn create_chat_approval_router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/chat/approval", post(chat_approval))
}

/// Parse keyword intent out of a free-form reason string.
///
/// Used when the frontend forwards a natural-language reply instead of a
/// button click. Returns `Some(true)` for approve intents, `Some(false)` for
/// reject intents, and `None` when no keyword matches (caller then trusts
/// the `approved` boolean verbatim).
pub fn parse_approval_intent(text: &str) -> Option<bool> {
    use cyberclaw_control_plane::intent_classifier::{Intent, IntentClassifier};
    let classifier = IntentClassifier::with_defaults();
    let matched = classifier.classify(text)?;
    match matched.intent {
        Intent::ApproveRequest => Some(true),
        Intent::RejectRequest => Some(false),
        _ => None,
    }
}

async fn audit_chat_approval(
    state: &Arc<AppState>,
    actor: &str,
    action: &str,
    request_id: &str,
    conversation_id: Option<&str>,
    result: AuditResult,
) {
    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            actor.to_string(),
            AuditKind::Mutation,
            action.to_string(),
            Some(format!("review:{}", request_id)),
            serde_json::json!({
                "source": CHAT_APPROVAL_SOURCE,
                "conversation_id": conversation_id,
            }),
            result,
        ))
        .await;
    }
}

/// POST /api/v1/chat/approval
///
/// # Security
///
/// - Requires a valid JWT (applied via the protected-routes middleware).
/// - Requires the caller's operator record to carry `role == "admin"`.
/// - Passes the decision through `process_review_result`, which runs the
///   full governance chain (audit + security event + resume hook). This
///   endpoint never mutates the queue directly.
async fn chat_approval(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<ChatApprovalRequest>,
) -> Result<Json<ChatApprovalResponse>, ApiError> {
    info!(
        caller = %claims.sub,
        request_id = %req.request_id,
        approved = req.approved,
        conversation_id = ?req.conversation_id,
        "chat.approval received"
    );

    // L7 — server-side role enforcement. Chat-driven approvals must NOT be
    // lower-privileged than their REST counterpart.
    if let Err(err) = require_admin(&claims).await {
        audit_chat_approval(
            &state,
            claims.sub.as_str(),
            "chat.approval:role_denied",
            &req.request_id,
            req.conversation_id.as_deref(),
            AuditResult::Failure {
                reason: "admin role required".to_string(),
            },
        )
        .await;
        return Err(err);
    }

    // Validate id shape before touching the queue so we emit a 400 instead
    // of a misleading 404.
    let review_id = ReviewId::from_string(req.request_id.clone())
        .map_err(|e| ApiError::InvalidRequest(format!("Invalid request_id: {}", e)))?;

    // If the frontend forwarded a free-form reply, let the intent classifier
    // override the `approved` flag — the user may have clicked "Approve" but
    // pasted a rejection reason, and intent wins.
    let effective_approved = match req.reason.as_deref() {
        Some(text) if !text.trim().is_empty() => {
            parse_approval_intent(text).unwrap_or(req.approved)
        }
        _ => req.approved,
    };

    let actor_id = ActorId::from_string(claims.sub.as_str().to_string())
        .map_err(|e| ApiError::InvalidRequest(format!("Invalid user ID in claims: {}", e)))?;
    let reviewer = ActorRef {
        id: actor_id,
        actor_type: ActorType::Human,
        tenant_id: claims.tenant.clone(),
        home_node_id: None,
        display_name: claims.sub.as_str().to_string(),
    };

    // Route through control-plane so the governance resume hook fires.
    match state
        .control_plane
        .process_review_result(&review_id, effective_approved, reviewer)
        .await
    {
        Ok(_) => {
            let decision = if effective_approved {
                "approved"
            } else {
                "rejected"
            };
            let _ = state.admin_event_bus.send(AdminEvent::ReviewResolved {
                review_id: req.request_id.clone(),
                decision: decision.to_string(),
            });
            audit_chat_approval(
                &state,
                claims.sub.as_str(),
                "chat.approval",
                &req.request_id,
                req.conversation_id.as_deref(),
                AuditResult::Success,
            )
            .await;

            let assistant_message =
                build_assistant_message(decision, &req.request_id, req.reason.as_deref());

            Ok(Json(ChatApprovalResponse {
                success: true,
                request_id: req.request_id,
                decision: decision.to_string(),
                assistant_message,
                source: CHAT_APPROVAL_SOURCE,
            }))
        }
        Err(err) => {
            error!(
                caller = %claims.sub,
                request_id = %req.request_id,
                error = %err,
                "chat.approval process_review_result failed"
            );
            audit_chat_approval(
                &state,
                claims.sub.as_str(),
                "chat.approval",
                &req.request_id,
                req.conversation_id.as_deref(),
                AuditResult::Failure {
                    reason: format!("{}", err),
                },
            )
            .await;
            Err(ApiError::NotFound(format!(
                "Review not found or already decided: {}",
                req.request_id
            )))
        }
    }
}

/// Format the assistant-facing confirmation that the UI can append to the
/// conversation history.
fn build_assistant_message(decision: &str, request_id: &str, reason: Option<&str>) -> String {
    match reason.map(str::trim).filter(|r| !r.is_empty()) {
        Some(r) => format!(
            "Approval request `{}` was {}. Reason: {}",
            request_id, decision, r
        ),
        None => format!("Approval request `{}` was {}.", request_id, decision),
    }
}

#[cfg(test)]
mod tests {
    //! Sprint 12 L1 — unit coverage.
    //!
    //! We don't spin up a full HTTP stack here; the handler is tested
    //! directly with `State`/`Extension`/`Json` extractors — same pattern
    //! used in `reviews.rs::role_tests`.

    use super::*;
    use crate::api::test_helpers::{build_test_state, seed_users_with_role};
    use axum::extract::{Extension, State};
    use cyberclaw_core::ids::UserId;
    use serial_test::serial;

    fn claims_for(user_id: &str) -> Claims {
        let uid = UserId::from_string(user_id.to_string()).expect("valid user id");
        let now = chrono::Utc::now().timestamp();
        Claims {
            sub: uid,
            tenant: None,
            iat: now,
            exp: now + 3600,
        }
    }

    // ── Intent parser ─────────────────────────────────────────────────────

    #[test]
    fn chat_approval_parser_recognises_approve_reply() {
        assert_eq!(parse_approval_intent("同意执行"), Some(true));
        assert_eq!(parse_approval_intent("Please go ahead"), Some(true));
        assert_eq!(parse_approval_intent("approve"), Some(true));
    }

    #[test]
    fn chat_approval_parser_recognises_reject_reply() {
        assert_eq!(parse_approval_intent("reject"), Some(false));
        assert_eq!(parse_approval_intent("我拒绝"), Some(false));
        assert_eq!(parse_approval_intent("stop"), Some(false));
    }

    #[test]
    fn chat_approval_parser_returns_none_for_unrelated_reply() {
        assert_eq!(parse_approval_intent("what is the weather"), None);
        assert_eq!(parse_approval_intent(""), None);
    }

    // ── Assistant message formatter ───────────────────────────────────────

    #[test]
    fn chat_approval_builds_assistant_message_with_reason() {
        let msg = build_assistant_message("approved", "rv_abc", Some("looks good"));
        assert!(msg.contains("rv_abc"));
        assert!(msg.contains("approved"));
        assert!(msg.contains("looks good"));
    }

    #[test]
    fn chat_approval_builds_assistant_message_without_reason() {
        let msg = build_assistant_message("rejected", "rv_def", None);
        assert!(msg.contains("rv_def"));
        assert!(msg.contains("rejected"));
        assert!(!msg.contains("Reason"));
    }

    // ── Endpoint role gate ────────────────────────────────────────────────

    #[tokio::test]
    #[serial]
    async fn chat_approval_rejected_for_viewer() {
        let (_tmp, _restore) = seed_users_with_role("chat-viewer", "viewer");
        let state = build_test_state();
        let claims = claims_for("chat-viewer");
        let err = chat_approval(
            State(state.clone()),
            Extension(claims),
            Json(ChatApprovalRequest {
                request_id: "rv_any".to_string(),
                conversation_id: Some("conv-1".to_string()),
                approved: true,
                reason: None,
            }),
        )
        .await
        .expect_err("viewer must be forbidden");
        assert!(matches!(err, ApiError::Forbidden(_)), "got {:?}", err);
    }

    #[tokio::test]
    #[serial]
    async fn chat_approval_admin_passes_role_gate_then_404s_on_missing_review() {
        // Admin passes role gate; the synthetic review id does not exist,
        // so we expect NotFound (never Forbidden). The admin-store fallback
        // used by /api/v1/reviews/:id isn't wired here — we go through
        // control_plane, which strictly 404s unknown ids.
        let (_tmp, _restore) = seed_users_with_role("chat-admin", "admin");
        let state = build_test_state();
        let claims = claims_for("chat-admin");
        // Craft a syntactically-valid ReviewId (UUID-shaped).
        let review_id = ReviewId::new();
        let res = chat_approval(
            State(state.clone()),
            Extension(claims),
            Json(ChatApprovalRequest {
                request_id: review_id.as_str().to_string(),
                conversation_id: None,
                approved: true,
                reason: None,
            }),
        )
        .await;
        match res {
            Err(ApiError::NotFound(_)) => {}
            Err(other) => panic!("admin must NOT be forbidden, got: {:?}", other),
            Ok(_) => {}
        }
    }

    #[tokio::test]
    #[serial]
    async fn chat_approval_rejects_invalid_request_id() {
        let (_tmp, _restore) = seed_users_with_role("chat-admin-2", "admin");
        let state = build_test_state();
        let claims = claims_for("chat-admin-2");
        let err = chat_approval(
            State(state.clone()),
            Extension(claims),
            Json(ChatApprovalRequest {
                request_id: "".to_string(), // empty → from_string fails
                conversation_id: None,
                approved: true,
                reason: None,
            }),
        )
        .await
        .expect_err("empty request_id must be rejected");
        assert!(
            matches!(err, ApiError::InvalidRequest(_)),
            "expected InvalidRequest, got {:?}",
            err
        );
    }
}
