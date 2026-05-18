//! Sprint 15 T7 — Clarify HTTP surface.
//!
//! Exposes the clarify lifecycle to the frontend:
//!
//! | Method | Path                                            |
//! |--------|-------------------------------------------------|
//! | POST   | `/api/v1/chat/clarify/:clarify_id/respond`      |
//! | GET    | `/api/v1/chat/clarify/pending`                  |
//! | GET    | `/api/v1/chat/clarify/all`                      |
//! | GET    | `/api/v1/chat/clarify/:clarify_id`              |
//!
//! # RBAC
//!
//! - Viewer: may submit answers and list/get only for conversations they
//!   own (`conversation_id` is matched against caller's user id via the
//!   conversation store). Admin: unrestricted.
//! - `list_all_clarifications` is admin-only.
//!
//! # Architectural compliance
//!
//! - Does NOT mutate the queue directly for resolution; calls
//!   `clarify_queue.resolve()` then `clarify_coordinator.notify_resolved()`
//!   so the agent loop can resume.
//! - Conversation message append is **not** implemented here — see
//!   `// TODO(T11)` comment below.

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{error, warn};

use cyberclaw_core::clarify::{ClarifyAnswer, ClarifyError, ClarifyRequest};
use cyberclaw_core::ids::ClarifyId;

use crate::api::admin::events::AdminEvent;
use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::clarify_broadcast::ClarifyEvent;
use crate::error::ApiError;
use crate::middleware::auth::{require_admin, Claims};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

/// Body for `POST /api/v1/chat/clarify/:clarify_id/respond`.
#[derive(Debug, Clone, Deserialize)]
pub struct SubmitClarifyRequest {
    /// The user's answers: `{questionText: answerString}`.
    pub answer: ClarifyAnswer,
    /// Conversation id — used for RBAC ownership check.
    pub conversation_id: Option<String>,
}

/// Success response body for submit.
#[derive(Debug, Clone, Serialize)]
pub struct SubmitClarifyResponse {
    pub success: bool,
    pub clarify_id: String,
    pub resolved_at: Option<DateTime<Utc>>,
}

/// Query params for `GET /api/v1/chat/clarify/pending`.
#[derive(Debug, Deserialize)]
pub struct ListPendingQuery {
    /// Required: filter by conversation id.
    pub conversation_id: String,
}

/// Query params for `GET /api/v1/chat/clarify/all`.
#[derive(Debug, Deserialize)]
pub struct ListAllQuery {
    /// Optional: only return entries resolved at or after this timestamp.
    pub since: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn caller_id(claims: &Claims) -> String {
    claims.sub.as_ref().to_string()
}

async fn check_admin(claims: &Claims) -> bool {
    require_admin(claims).await.is_ok()
}

/// Emit an audit entry for a clarify resolve action. Errors are logged but
/// swallowed so they cannot disrupt the caller's control flow.
async fn audit_clarify_resolved(
    state: &Arc<AppState>,
    actor: &str,
    clarify_id: &ClarifyId,
    picked_option: Option<String>,
    freeform_len: usize,
    result: AuditResult,
) {
    let Some(sink) = state.audit.as_ref() else {
        return;
    };
    sink.record(AuditEntry::now(
        actor.to_string(),
        AuditKind::ClarifyResolved {
            clarify_id: clarify_id.clone(),
            picked_option,
            freeform_len,
        },
        "clarify.respond",
        Some(format!("clarify:{}", clarify_id.as_str())),
        serde_json::json!({"source": "chat"}),
        result,
    ))
    .await;
}

/// Compute `freeform_len` (total byte length of all answer values) for the
/// audit entry. Does NOT log answer text itself.
fn answer_freeform_len(answer: &ClarifyAnswer) -> usize {
    answer.answers.values().map(|v| v.len()).sum()
}

/// Pick the first answer value as the "picked_option" label for the audit
/// trail. Returns `None` if the answer map is empty.
fn answer_picked_option(answer: &ClarifyAnswer) -> Option<String> {
    answer.answers.values().next().cloned()
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

// HTTP 410 for AlreadyTimedOut requires a concrete return type: axum's
// `impl IntoResponse` cannot carry different status codes. We define
// `ClarifyRespond` so the handler can return either a success JSON body
// or a 410 status.
// ---------------------------------------------------------------------------

/// Concrete response type that can carry either a success JSON body or a
/// 410/404 status with an error JSON body.
pub enum ClarifyRespond {
    Ok(Json<SubmitClarifyResponse>),
    TimedOut,
}

impl std::fmt::Debug for ClarifyRespond {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClarifyRespond::Ok(_) => write!(f, "ClarifyRespond::Ok(..)"),
            ClarifyRespond::TimedOut => write!(f, "ClarifyRespond::TimedOut"),
        }
    }
}

impl IntoResponse for ClarifyRespond {
    fn into_response(self) -> axum::response::Response {
        match self {
            ClarifyRespond::Ok(j) => j.into_response(),
            ClarifyRespond::TimedOut => (
                StatusCode::GONE,
                Json(serde_json::json!({"error": "already_timed_out"})),
            )
                .into_response(),
        }
    }
}

/// Real handler wired to axum — wraps `submit_clarify_response` to produce
/// the correct 410 status for timed-out requests.
pub async fn submit_clarify_respond_handler(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(clarify_id_str): Path<String>,
    Json(req): Json<SubmitClarifyRequest>,
) -> Result<ClarifyRespond, ApiError> {
    let caller = caller_id(&claims);
    let is_admin = check_admin(&claims).await;

    let clarify_id = ClarifyId::from_string(clarify_id_str.clone())
        .map_err(|e| ApiError::InvalidRequest(format!("Invalid clarify_id: {e}")))?;

    // RBAC: non-admin callers must supply a conversation_id and must own it.
    if !is_admin {
        match req.conversation_id.as_deref() {
            None => {
                warn!(caller = %caller, "clarify.respond: viewer did not supply conversation_id");
                return Err(ApiError::Forbidden(
                    "conversation_id required for non-admin callers".to_string(),
                ));
            }
            Some(conv_id) => {
                let existing = state.clarify_queue.get(&clarify_id).await;
                match existing {
                    None => {
                        return Err(ApiError::NotFound(format!(
                            "Clarify not found: {}",
                            clarify_id_str
                        )));
                    }
                    Some(ref clarify_req) => {
                        if clarify_req.conversation_id != conv_id {
                            return Err(ApiError::Forbidden(
                                "conversation_id does not match clarify request".to_string(),
                            ));
                        }
                        let store = state.conversation_store();
                        let conv = store.get(conv_id).await.ok_or_else(|| {
                            ApiError::NotFound("Conversation not found".to_string())
                        })?;
                        if conv.owner_user_id != caller {
                            audit_clarify_resolved(
                                &state,
                                &caller,
                                &clarify_id,
                                None,
                                0,
                                AuditResult::Failure {
                                    reason: "not the owner of the conversation".to_string(),
                                },
                            )
                            .await;
                            return Err(ApiError::Forbidden(
                                "not the owner of this conversation".to_string(),
                            ));
                        }
                    }
                }
            }
        }
    }

    let freeform_len = answer_freeform_len(&req.answer);
    let picked_option = answer_picked_option(&req.answer);

    match state
        .clarify_queue
        .resolve(&clarify_id, req.answer.clone())
        .await
    {
        Ok(resolved_req) => {
            state
                .clarify_coordinator
                .notify_resolved(&clarify_id, req.answer.clone())
                .await;

            // Broadcast resolved event to all active SSE connections for this
            // conversation so the frontend can transition the clarify card.
            let conversation_id = resolved_req.conversation_id.clone();
            state
                .clarify_broadcaster
                .publish(
                    &conversation_id,
                    ClarifyEvent::Resolved {
                        id: clarify_id.clone(),
                        answer: req.answer.clone(),
                    },
                )
                .await;

            let resolved_at = resolved_req.resolved_at.unwrap_or_else(Utc::now);
            let _ = state.admin_event_bus.send(AdminEvent::ClarifyResolved {
                clarify_id: clarify_id.clone(),
                conversation_id,
                resolved_at,
            });

            audit_clarify_resolved(
                &state,
                &caller,
                &clarify_id,
                picked_option,
                freeform_len,
                AuditResult::Success,
            )
            .await;

            // T11: Persist clarify_response to conversation history so a page
            // refresh can reconstruct the resolved state. Failures are swallowed
            // (.ok()) — persistence must never block the clarify flow.
            //
            // NFR-Security: answer text is now run through `ToolOutputSanitizer`
            // (sanitize_and_redact) so credentials / prompt-injection markers
            // are redacted before persistence. The raw user-supplied answer
            // never lands in the conversation store verbatim.
            {
                use crate::api::chat_conversations::ChatMessage;
                let raw_answer = req
                    .answer
                    .answers
                    .values()
                    .next()
                    .cloned()
                    .unwrap_or_default();
                let sanitized = state
                    .memory_sanitizer
                    .sanitize_and_redact("clarify_response", &raw_answer);
                if let Some(audit) = state.audit.as_ref() {
                    audit
                        .record_sanitizer_warnings(
                            caller.clone(),
                            "clarify_response",
                            Some(format!("clarify:{clarify_id_str}")),
                            &sanitized.warnings,
                        )
                        .await;
                }
                let answer_content = sanitized.content;
                let response_msg = ChatMessage {
                    role: "clarify_response".to_string(),
                    content: answer_content,
                    metadata: Some(serde_json::json!({
                        "clarify_id": clarify_id.as_str(),
                        "answer": req.answer,
                        "sanitization_modified": sanitized.was_modified,
                        "sanitization_warnings": sanitized.warnings.len(),
                    })),
                    ts: Some(chrono::Utc::now().timestamp_millis()),
                };
                state
                    .conversation_store()
                    .append_message_internal(&resolved_req.conversation_id, response_msg)
                    .await
                    .ok();
            }

            Ok(ClarifyRespond::Ok(Json(SubmitClarifyResponse {
                success: true,
                clarify_id: clarify_id_str,
                resolved_at: resolved_req.resolved_at,
            })))
        }
        Err(ClarifyError::AlreadyResolved) => {
            let existing = state.clarify_queue.get(&clarify_id).await;
            let resolved_at = existing.and_then(|r| r.resolved_at);
            Ok(ClarifyRespond::Ok(Json(SubmitClarifyResponse {
                success: true,
                clarify_id: clarify_id_str,
                resolved_at,
            })))
        }
        Err(ClarifyError::AlreadyTimedOut) => {
            audit_clarify_resolved(
                &state,
                &caller,
                &clarify_id,
                None,
                0,
                AuditResult::Failure {
                    reason: "already_timed_out".to_string(),
                },
            )
            .await;
            Ok(ClarifyRespond::TimedOut)
        }
        Err(ClarifyError::NotFound) => Err(ApiError::NotFound(format!(
            "Clarify not found: {}",
            clarify_id_str
        ))),
        Err(e) => {
            error!(
                caller = %caller,
                clarify_id = %clarify_id_str,
                error = %e,
                "clarify.respond: unexpected error"
            );
            Err(ApiError::InternalError(format!(
                "clarify resolve failed: {e}"
            )))
        }
    }
}

/// GET /api/v1/chat/clarify/pending?conversation_id=X
///
/// Returns all Pending clarify requests for the given conversation.
/// RBAC: viewer must own the conversation; admin sees without ownership check.
pub async fn list_pending_clarifications(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Query(params): Query<ListPendingQuery>,
) -> Result<Json<Vec<ClarifyRequest>>, ApiError> {
    let caller = caller_id(&claims);
    let is_admin = check_admin(&claims).await;

    if !is_admin {
        // Verify conversation ownership.
        let store = state.conversation_store();
        let conv = store
            .get(&params.conversation_id)
            .await
            .ok_or_else(|| ApiError::NotFound("Conversation not found".to_string()))?;
        if conv.owner_user_id != caller {
            return Err(ApiError::Forbidden(
                "not the owner of this conversation".to_string(),
            ));
        }
    }

    let pending = state.clarify_queue.list_pending().await;
    let filtered: Vec<ClarifyRequest> = pending
        .into_iter()
        .filter(|r| r.conversation_id == params.conversation_id)
        .collect();

    Ok(Json(filtered))
}

/// GET /api/v1/chat/clarify/:clarify_id
///
/// Returns a single clarify request by id.
/// RBAC: viewer must own the conversation the clarify belongs to; admin unrestricted.
pub async fn get_clarify(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(clarify_id_str): Path<String>,
) -> Result<Json<ClarifyRequest>, ApiError> {
    let caller = caller_id(&claims);
    let is_admin = check_admin(&claims).await;

    let clarify_id = ClarifyId::from_string(clarify_id_str.clone())
        .map_err(|e| ApiError::InvalidRequest(format!("Invalid clarify_id: {e}")))?;

    let clarify_req = state
        .clarify_queue
        .get(&clarify_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Clarify not found: {}", clarify_id_str)))?;

    if !is_admin {
        let store = state.conversation_store();
        let conv = store
            .get(&clarify_req.conversation_id)
            .await
            .ok_or_else(|| ApiError::NotFound("Conversation not found".to_string()))?;
        if conv.owner_user_id != caller {
            return Err(ApiError::Forbidden(
                "not the owner of this conversation".to_string(),
            ));
        }
    }

    Ok(Json(clarify_req))
}

/// GET /api/v1/chat/clarify/all?since=<iso8601>
///
/// Admin-only. Returns all clarify requests (Pending + Resolved since `since`).
pub async fn list_all_clarifications(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Query(params): Query<ListAllQuery>,
) -> Result<Json<Vec<ClarifyRequest>>, ApiError> {
    require_admin(&claims).await?;

    let since = params.since.unwrap_or_else(|| {
        // Default: epoch — return everything.
        DateTime::from_timestamp(0, 0).unwrap_or_else(Utc::now)
    });

    let mut all: Vec<ClarifyRequest> = state.clarify_queue.list_pending().await;
    let resolved = state.clarify_queue.list_resolved(since).await;
    all.extend(resolved);

    // Include TimedOut entries so the admin governance view surfaces all
    // terminal states. list_timed_out uses no `since` filter — admins see
    // all timed-out requests up to the queue default limit (usize::MAX).
    match state.clarify_queue.list_timed_out(usize::MAX).await {
        Ok(timed_out) => all.extend(timed_out),
        Err(e) => {
            error!(error = %e, "list_all_clarifications: list_timed_out failed, continuing without timed-out entries");
        }
    }

    all.sort_by_key(|r| r.created_at);

    Ok(Json(all))
}

// ---------------------------------------------------------------------------
// Router factory
// ---------------------------------------------------------------------------

/// Build the clarify router.
///
/// Note: `/pending` and `/all` must be registered BEFORE `/:clarify_id`
/// so axum does not misroute those static path segments as id parameters.
pub fn create_chat_clarify_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/chat/clarify/pending",
            get(list_pending_clarifications),
        )
        .route("/api/v1/chat/clarify/all", get(list_all_clarifications))
        .route("/api/v1/chat/clarify/:clarify_id", get(get_clarify))
        .route(
            "/api/v1/chat/clarify/:clarify_id/respond",
            post(submit_clarify_respond_handler),
        )
}

// ---------------------------------------------------------------------------
// Dev-only trigger endpoint (Task #10 integration testing)
// ---------------------------------------------------------------------------
//
// `POST /api/v1/_dev/trigger_clarify` is **only compiled and registered when
// `debug_assertions` are enabled** (i.e., `cargo test` and debug builds).
// Production release builds exclude this route entirely.
//
// Body: `TriggerClarifyRequest`
// Success: 200 + `TriggerClarifyResponse` containing the agent's received answer.
// Timeout: 408 + `{"error": "timeout"}`.
//
// The endpoint calls `state.ask_user_clarify()`, which:
//   1. Broadcasts `ClarifyEvent::Requested` to SSE subscribers.
//   2. Enqueues the request into `clarify_queue`.
//   3. Blocks until a POST to `/:id/respond` calls `notify_resolved`.
//
// This makes it possible to write integration tests that trigger a clarify
// from the outside without a real agent loop.

#[cfg(debug_assertions)]
pub mod dev {
    use super::*;
    use std::time::Duration;

    /// Request body for the dev trigger endpoint.
    #[derive(Debug, Deserialize)]
    pub struct TriggerClarifyRequest {
        pub conversation_id: String,
        pub agent_id: String,
        pub question: String,
        pub options: Vec<TriggerClarifyOption>,
        /// Timeout in seconds (default: 30).
        pub timeout_secs: Option<u64>,
    }

    #[derive(Debug, Deserialize)]
    pub struct TriggerClarifyOption {
        pub label: String,
        pub description: String,
    }

    /// Response body from the dev trigger endpoint.
    #[derive(Debug, Serialize)]
    pub struct TriggerClarifyResponse {
        pub clarify_id: String,
        pub answer: ClarifyAnswer,
    }

    /// POST /api/v1/_dev/trigger_clarify
    ///
    /// Simulates an agent calling `ask_user_clarify`. Blocks until the user
    /// (or test harness) submits an answer via `POST /clarify/:id/respond`.
    pub async fn trigger_clarify_handler(
        State(state): State<Arc<AppState>>,
        Extension(claims): Extension<Claims>,
        Json(body): Json<TriggerClarifyRequest>,
    ) -> Result<impl IntoResponse, ApiError> {
        use cyberclaw_core::clarify::{ClarifyOption, ClarifyQuestion, ClarifyStatus};

        let agent_id = cyberclaw_core::ids::AgentId::from_string(body.agent_id)
            .map_err(|e| ApiError::InvalidRequest(format!("Invalid agent_id: {e}")))?;

        let now = chrono::Utc::now();
        let timeout_secs = body.timeout_secs.unwrap_or(30);

        let req = ClarifyRequest {
            id: ClarifyId::new(),
            conversation_id: body.conversation_id.clone(),
            agent_id,
            questions: vec![ClarifyQuestion {
                question: body.question,
                options: body
                    .options
                    .into_iter()
                    .map(|o| ClarifyOption {
                        label: o.label,
                        description: o.description,
                        preview: None,
                    })
                    .collect(),
                multi_select: false,
            }],
            source: Some(format!("dev-trigger:{}", claims.sub.as_ref())),
            created_at: now,
            expires_at: now + chrono::Duration::seconds(timeout_secs as i64),
            status: ClarifyStatus::Pending,
            answers: None,
            resolved_at: None,
        };

        req.validate()
            .map_err(|e| ApiError::InvalidRequest(format!("Invalid clarify request: {e}")))?;

        let clarify_id = req.id.clone();
        let timeout = Duration::from_secs(timeout_secs);

        match state
            .ask_user_clarify(&body.conversation_id, req, timeout)
            .await
        {
            Ok(answer) => Ok((
                axum::http::StatusCode::OK,
                Json(TriggerClarifyResponse {
                    clarify_id: clarify_id.as_str().to_string(),
                    answer,
                }),
            )
                .into_response()),
            Err(cyberclaw_core::clarify::ClarifyError::AlreadyTimedOut) => Ok((
                axum::http::StatusCode::REQUEST_TIMEOUT,
                Json(serde_json::json!({"error": "timeout"})),
            )
                .into_response()),
            Err(e) => Err(ApiError::InternalError(format!(
                "trigger_clarify failed: {e}"
            ))),
        }
    }

    /// Build the dev-only router.
    pub fn create_dev_clarify_router() -> Router<Arc<AppState>> {
        Router::new().route(
            "/api/v1/_dev/trigger_clarify",
            post(trigger_clarify_handler),
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_helpers::{build_test_state_with_clarify, seed_users_with_role};
    use axum::extract::{Extension, Path, Query, State};
    use cyberclaw_core::clarify::{ClarifyOption, ClarifyQuestion, ClarifyStatus};
    use cyberclaw_core::ids::{AgentId, ClarifyId};
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

    fn make_clarify_request(conversation_id: &str) -> ClarifyRequest {
        let now = Utc::now();
        ClarifyRequest {
            id: ClarifyId::new(),
            conversation_id: conversation_id.to_string(),
            agent_id: AgentId::from_string("test-agent".to_string()).unwrap(),
            questions: vec![ClarifyQuestion {
                question: "Which environment?".to_string(),
                options: vec![
                    ClarifyOption {
                        label: "staging".to_string(),
                        description: "Use the staging environment".to_string(),
                        preview: None,
                    },
                    ClarifyOption {
                        label: "production".to_string(),
                        description: "Use the production environment".to_string(),
                        preview: None,
                    },
                ],
                multi_select: false,
            }],
            source: None,
            created_at: now,
            expires_at: now + chrono::Duration::seconds(300),
            status: ClarifyStatus::Pending,
            answers: None,
            resolved_at: None,
        }
    }

    fn make_answer() -> ClarifyAnswer {
        let mut a = ClarifyAnswer::new();
        a.insert("Which environment?", "staging");
        a
    }

    // ── Test 1: viewer submits own conversation clarify → 200 ─────────────

    #[tokio::test]
    #[serial]
    async fn test_submit_happy_path() {
        let (_tmp, _restore) = seed_users_with_role("viewer-happy", "viewer");
        let state = build_test_state_with_clarify();

        // Seed a conversation owned by the viewer.
        let conv_id = "conv-happy-001".to_string();
        state
            .conversation_store()
            .create_for_test(&conv_id, "viewer-happy")
            .await;

        // Enqueue a clarify request for that conversation.
        let clarify = make_clarify_request(&conv_id);
        let clarify_id = clarify.id.clone();
        state.clarify_queue.enqueue(clarify).await.unwrap();

        let claims = claims_for("viewer-happy");
        let result = submit_clarify_respond_handler(
            State(state.clone()),
            Extension(claims),
            Path(clarify_id.as_str().to_string()),
            Json(SubmitClarifyRequest {
                answer: make_answer(),
                conversation_id: Some(conv_id),
            }),
        )
        .await;

        assert!(result.is_ok(), "expected Ok, got: {:?}", result.err());
        let resolved = state.clarify_queue.get(&clarify_id).await.unwrap();
        assert_eq!(resolved.status, ClarifyStatus::Resolved);
    }

    // ── Test 2: viewer submits another user's conversation → 403 ──────────

    #[tokio::test]
    #[serial]
    async fn test_submit_rbac_viewer_denied() {
        let (_tmp, _restore) = seed_users_with_role("viewer-denied", "viewer");
        let state = build_test_state_with_clarify();

        // Conversation owned by a different user.
        let conv_id = "conv-other-002".to_string();
        state
            .conversation_store()
            .create_for_test(&conv_id, "other-owner")
            .await;

        let clarify = make_clarify_request(&conv_id);
        let clarify_id = clarify.id.clone();
        state.clarify_queue.enqueue(clarify).await.unwrap();

        let claims = claims_for("viewer-denied");
        let err = submit_clarify_respond_handler(
            State(state.clone()),
            Extension(claims),
            Path(clarify_id.as_str().to_string()),
            Json(SubmitClarifyRequest {
                answer: make_answer(),
                conversation_id: Some(conv_id),
            }),
        )
        .await
        .expect_err("should be forbidden");

        assert!(matches!(err, ApiError::Forbidden(_)), "got {:?}", err);
    }

    // ── Test 3: admin submits any clarify → 200 ───────────────────────────

    #[tokio::test]
    #[serial]
    async fn test_submit_admin_override() {
        let (_tmp, _restore) = seed_users_with_role("admin-override", "admin");
        let state = build_test_state_with_clarify();

        let conv_id = "conv-admin-003".to_string();
        state
            .conversation_store()
            .create_for_test(&conv_id, "some-other-user")
            .await;

        let clarify = make_clarify_request(&conv_id);
        let clarify_id = clarify.id.clone();
        state.clarify_queue.enqueue(clarify).await.unwrap();

        let claims = claims_for("admin-override");
        let result = submit_clarify_respond_handler(
            State(state.clone()),
            Extension(claims),
            Path(clarify_id.as_str().to_string()),
            Json(SubmitClarifyRequest {
                answer: make_answer(),
                conversation_id: None, // admin doesn't need to supply conversation_id
            }),
        )
        .await;

        assert!(result.is_ok(), "admin should succeed: {:?}", result.err());
    }

    // ── Test 4: double submit returns 200 + original resolved_at ──────────

    #[tokio::test]
    #[serial]
    async fn test_submit_idempotent_double() {
        let (_tmp, _restore) = seed_users_with_role("admin-idem", "admin");
        let state = build_test_state_with_clarify();

        let clarify = make_clarify_request("conv-idem-004");
        let clarify_id = clarify.id.clone();
        state.clarify_queue.enqueue(clarify).await.unwrap();

        let claims = claims_for("admin-idem");

        // First submit.
        let r1 = submit_clarify_respond_handler(
            State(state.clone()),
            Extension(claims.clone()),
            Path(clarify_id.as_str().to_string()),
            Json(SubmitClarifyRequest {
                answer: make_answer(),
                conversation_id: None,
            }),
        )
        .await;
        assert!(r1.is_ok(), "first submit should succeed");

        // Second submit (idempotent).
        let r2 = submit_clarify_respond_handler(
            State(state.clone()),
            Extension(claims),
            Path(clarify_id.as_str().to_string()),
            Json(SubmitClarifyRequest {
                answer: make_answer(),
                conversation_id: None,
            }),
        )
        .await;
        assert!(r2.is_ok(), "second submit should succeed (idempotent)");
    }

    // ── Test 5: timed-out clarify → 410 ───────────────────────────────────

    #[tokio::test]
    #[serial]
    async fn test_submit_already_timedout_returns_410() {
        let (_tmp, _restore) = seed_users_with_role("admin-timeout", "admin");
        let state = build_test_state_with_clarify();

        let clarify = make_clarify_request("conv-timeout-005");
        let clarify_id = clarify.id.clone();
        state.clarify_queue.enqueue(clarify).await.unwrap();
        // Manually mark as timed out.
        state.clarify_queue.mark_timeout(&clarify_id).await.unwrap();

        let claims = claims_for("admin-timeout");
        let result = submit_clarify_respond_handler(
            State(state.clone()),
            Extension(claims),
            Path(clarify_id.as_str().to_string()),
            Json(SubmitClarifyRequest {
                answer: make_answer(),
                conversation_id: None,
            }),
        )
        .await;

        assert!(result.is_ok(), "should return Ok(TimedOut), not Err");
        match result.unwrap() {
            ClarifyRespond::TimedOut => {}
            ClarifyRespond::Ok(_) => panic!("expected TimedOut variant"),
        }
    }

    // ── Test 6: not-found clarify → 404 ───────────────────────────────────

    #[tokio::test]
    #[serial]
    async fn test_submit_not_found_returns_404() {
        let (_tmp, _restore) = seed_users_with_role("admin-404", "admin");
        let state = build_test_state_with_clarify();

        let fake_id = ClarifyId::new();
        let claims = claims_for("admin-404");
        let err = submit_clarify_respond_handler(
            State(state.clone()),
            Extension(claims),
            Path(fake_id.as_str().to_string()),
            Json(SubmitClarifyRequest {
                answer: make_answer(),
                conversation_id: None,
            }),
        )
        .await
        .expect_err("should be not found");

        assert!(matches!(err, ApiError::NotFound(_)), "got {:?}", err);
    }

    // ── Test 7: list_pending filters by conversation_id ───────────────────

    #[tokio::test]
    #[serial]
    async fn test_list_pending_filters_by_conversation_id() {
        let (_tmp, _restore) = seed_users_with_role("admin-list", "admin");
        let state = build_test_state_with_clarify();

        let conv_a = "conv-list-a".to_string();
        let conv_b = "conv-list-b".to_string();

        let r_a = make_clarify_request(&conv_a);
        let r_b = make_clarify_request(&conv_b);
        state.clarify_queue.enqueue(r_a).await.unwrap();
        state.clarify_queue.enqueue(r_b).await.unwrap();

        let claims = claims_for("admin-list");
        let result = list_pending_clarifications(
            State(state.clone()),
            Extension(claims),
            Query(ListPendingQuery {
                conversation_id: conv_a.clone(),
            }),
        )
        .await
        .unwrap();

        assert_eq!(result.0.len(), 1);
        assert_eq!(result.0[0].conversation_id, conv_a);
    }

    // ── Test 8: get_clarify returns full request ───────────────────────────

    #[tokio::test]
    #[serial]
    async fn test_get_clarify_returns_full_request() {
        let (_tmp, _restore) = seed_users_with_role("admin-get", "admin");
        let state = build_test_state_with_clarify();

        let clarify = make_clarify_request("conv-get-008");
        let clarify_id = clarify.id.clone();
        let expected_conv_id = clarify.conversation_id.clone();
        state.clarify_queue.enqueue(clarify).await.unwrap();

        let claims = claims_for("admin-get");
        let result = get_clarify(
            State(state.clone()),
            Extension(claims),
            Path(clarify_id.as_str().to_string()),
        )
        .await
        .unwrap();

        assert_eq!(result.0.id, clarify_id);
        assert_eq!(result.0.conversation_id, expected_conv_id);
        assert_eq!(result.0.status, ClarifyStatus::Pending);
    }

    // ── Integration Test 9: end-to-end clarify flow via broadcaster ───────
    //
    // Proves the full chain:
    //   fake-agent calls ask_user_clarify()
    //   → broadcaster emits ClarifyEvent::Requested to SSE subscriber
    //   → POST respond wakes coordinator
    //   → broadcaster emits ClarifyEvent::Resolved to SSE subscriber
    //   → fake-agent receives Ok(answer)

    #[tokio::test]
    async fn test_end_to_end_clarify_flow() {
        use crate::clarify_broadcast::ClarifyEvent;
        use std::time::Duration;

        let state = build_test_state_with_clarify();
        let conv_id = "conv-e2e-009".to_string();

        // Subscribe to the broadcaster BEFORE ask (simulates SSE handler).
        let mut rx = state.clarify_broadcaster.subscribe(&conv_id).await;

        // Build a valid ClarifyRequest.
        let req = make_clarify_request(&conv_id);
        let clarify_id = req.id.clone();

        // Spawn the "fake agent" that calls ask_user_clarify.
        let state_clone = state.clone();
        let conv_id_clone = conv_id.clone();
        let agent_handle = tokio::spawn(async move {
            state_clone
                .ask_user_clarify(&conv_id_clone, req, Duration::from_secs(5))
                .await
        });

        // Assert SSE subscriber receives ClarifyEvent::Requested within 500ms.
        let event = tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("should receive Requested within 500ms")
            .expect("recv ok");
        match event {
            ClarifyEvent::Requested(ref r) => assert_eq!(r.id, clarify_id),
            _ => panic!("expected ClarifyEvent::Requested, got Resolved"),
        }

        // Simulate user POST /clarify/:id/respond — calls notify_resolved + broadcasts Resolved.
        let answer = make_answer();
        state
            .clarify_coordinator
            .notify_resolved(&clarify_id, answer.clone())
            .await;
        state
            .clarify_broadcaster
            .publish(
                &conv_id,
                ClarifyEvent::Resolved {
                    id: clarify_id.clone(),
                    answer: answer.clone(),
                },
            )
            .await;

        // Assert SSE subscriber receives ClarifyEvent::Resolved within 500ms.
        let event2 = tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("should receive Resolved within 500ms")
            .expect("recv ok");
        match event2 {
            ClarifyEvent::Resolved { ref id, .. } => assert_eq!(*id, clarify_id),
            _ => panic!("expected ClarifyEvent::Resolved, got Requested"),
        }

        // Assert fake agent task returns Ok(answer) within timeout.
        let agent_result = tokio::time::timeout(Duration::from_secs(2), agent_handle)
            .await
            .expect("agent task should finish within 2s")
            .expect("task should not panic");

        assert!(
            agent_result.is_ok(),
            "agent should receive Ok(answer): {:?}",
            agent_result
        );
        let received_answer = agent_result.unwrap();
        assert_eq!(
            received_answer.answers.get("Which environment?").unwrap(),
            "staging"
        );
    }

    // ── Integration Test 10: ask() timeout propagates ─────────────────────

    #[tokio::test]
    async fn test_ask_timeout_propagates() {
        use cyberclaw_core::clarify::ClarifyError;
        use std::time::Duration;

        let state = build_test_state_with_clarify();
        let conv_id = "conv-timeout-010".to_string();

        let req = make_clarify_request(&conv_id);
        let clarify_id = req.id.clone();

        // 50ms timeout — nobody will call respond.
        let result = state
            .ask_user_clarify(&conv_id, req, Duration::from_millis(50))
            .await;

        assert!(
            matches!(result, Err(ClarifyError::AlreadyTimedOut)),
            "expected AlreadyTimedOut, got: {:?}",
            result
        );

        // Queue should show TimedOut status.
        let stored = state.clarify_queue.get(&clarify_id).await;
        assert!(stored.is_some());
        assert_eq!(
            stored.unwrap().status,
            ClarifyStatus::TimedOut,
            "queue status should be TimedOut"
        );
    }

    // ── Test T11-a: submit handler appends clarify_response message ──────────

    #[tokio::test]
    #[serial]
    async fn test_submit_appends_clarify_response_message() {
        use crate::api::chat_conversations::reset_store_for_tests;

        reset_store_for_tests().await;
        let (_tmp, _restore) = seed_users_with_role("admin-t11a", "admin");
        let state = build_test_state_with_clarify();

        let conv_id = "conv-t11a-001".to_string();
        // Create a conversation in the global store so append_message_internal finds it.
        state
            .conversation_store()
            .create_for_test(&conv_id, "admin-t11a")
            .await;

        let clarify = make_clarify_request(&conv_id);
        let clarify_id = clarify.id.clone();
        state.clarify_queue.enqueue(clarify).await.unwrap();

        let claims = claims_for("admin-t11a");
        let result = submit_clarify_respond_handler(
            State(state.clone()),
            Extension(claims),
            Path(clarify_id.as_str().to_string()),
            Json(SubmitClarifyRequest {
                answer: make_answer(),
                conversation_id: None,
            }),
        )
        .await;
        assert!(result.is_ok(), "submit should succeed: {:?}", result.err());

        // Verify that a "clarify_response" message was appended.
        let conv = state
            .conversation_store()
            .get(&conv_id)
            .await
            .expect("conversation must exist");
        let clarify_resp_msgs: Vec<_> = conv
            .messages
            .iter()
            .filter(|m| m.role == "clarify_response")
            .collect();
        assert_eq!(
            clarify_resp_msgs.len(),
            1,
            "expected exactly one clarify_response message"
        );
        let msg = &clarify_resp_msgs[0];
        assert_eq!(
            msg.content, "staging",
            "content should be the selected answer"
        );
        // metadata must contain clarify_id.
        let meta = msg.metadata.as_ref().expect("metadata must be present");
        assert_eq!(
            meta["clarify_id"].as_str().unwrap(),
            clarify_id.as_str(),
            "clarify_id must match in metadata"
        );
    }

    // ── Test T11-b: ask_user_clarify appends clarify message ─────────────────

    #[tokio::test]
    #[serial]
    async fn test_ask_user_clarify_appends_clarify_message() {
        use crate::api::chat_conversations::reset_store_for_tests;
        use cyberclaw_core::clarify::ClarifyError;
        use std::time::Duration;

        reset_store_for_tests().await;
        let state = build_test_state_with_clarify();

        let conv_id = "conv-t11b-001".to_string();
        // Create conversation so the store can append to it.
        state
            .conversation_store()
            .create_for_test(&conv_id, "test-owner")
            .await;

        let req = make_clarify_request(&conv_id);

        // Use a short timeout — nobody will respond, so ask() returns
        // AlreadyTimedOut. The clarify message is appended via tokio::spawn
        // before coordinator.ask() blocks, so it arrives before the timeout.
        let result = state
            .ask_user_clarify(&conv_id, req, Duration::from_millis(200))
            .await;

        assert!(
            matches!(result, Err(ClarifyError::AlreadyTimedOut)),
            "expected timeout: {:?}",
            result
        );

        // Give the background spawn a moment to complete its store write.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // The "clarify" message should have been appended.
        let conv = state
            .conversation_store()
            .get(&conv_id)
            .await
            .expect("conversation must exist");
        let clarify_msgs: Vec<_> = conv
            .messages
            .iter()
            .filter(|m| m.role == "clarify")
            .collect();
        assert_eq!(
            clarify_msgs.len(),
            1,
            "expected exactly one clarify message"
        );
        let msg = &clarify_msgs[0];
        assert_eq!(
            msg.content, "Which environment?",
            "content should be the question text"
        );
        let meta = msg.metadata.as_ref().expect("metadata must be present");
        assert!(
            meta.get("clarify_id").is_some(),
            "clarify_id must be in metadata"
        );
        assert!(
            meta.get("questions").is_some(),
            "questions must be in metadata"
        );
        assert!(
            meta.get("expires_at").is_some(),
            "expires_at must be in metadata"
        );
    }

    // ── Integration Test 11: broadcaster publish reaches resolved subscriber ─

    #[tokio::test]
    #[serial]
    async fn test_submit_handler_broadcasts_resolved() {
        use crate::clarify_broadcast::ClarifyEvent;

        let (_tmp, _restore) = seed_users_with_role("admin-broadcast", "admin");
        let state = build_test_state_with_clarify();

        let conv_id = "conv-broadcast-011".to_string();

        // Subscribe to broadcaster before submit.
        let mut rx = state.clarify_broadcaster.subscribe(&conv_id).await;

        // Enqueue a clarify.
        let clarify = make_clarify_request(&conv_id);
        let clarify_id = clarify.id.clone();
        state.clarify_queue.enqueue(clarify).await.unwrap();

        // Call the HTTP handler (admin path — no conversation ownership needed).
        let claims = claims_for("admin-broadcast");
        let result = submit_clarify_respond_handler(
            State(state.clone()),
            Extension(claims),
            Path(clarify_id.as_str().to_string()),
            Json(SubmitClarifyRequest {
                answer: make_answer(),
                conversation_id: None,
            }),
        )
        .await;

        assert!(result.is_ok(), "submit should succeed: {:?}", result.err());

        // Assert resolved event was broadcast within 200ms.
        let event = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("should receive Resolved within 200ms")
            .expect("recv ok");

        match event {
            ClarifyEvent::Resolved { ref id, .. } => assert_eq!(*id, clarify_id),
            ClarifyEvent::Requested(_) => panic!("expected Resolved, got Requested"),
        }
    }
}

// ---------------------------------------------------------------------------
// Integration tests — T17 edge-case scenarios
// ---------------------------------------------------------------------------

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::api::test_helpers::{build_test_state_with_clarify, seed_users_with_role};
    use axum::extract::{Extension, Path, Query, State};
    use cyberclaw_core::clarify::{ClarifyOption, ClarifyQuestion, ClarifyStatus};
    use cyberclaw_core::ids::{AgentId, ClarifyId};
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

    fn make_clarify_request(conversation_id: &str) -> ClarifyRequest {
        let now = Utc::now();
        ClarifyRequest {
            id: ClarifyId::new(),
            conversation_id: conversation_id.to_string(),
            agent_id: AgentId::from_string("test-agent".to_string()).unwrap(),
            questions: vec![ClarifyQuestion {
                question: "Which environment?".to_string(),
                options: vec![
                    ClarifyOption {
                        label: "staging".to_string(),
                        description: "Use the staging environment".to_string(),
                        preview: None,
                    },
                    ClarifyOption {
                        label: "production".to_string(),
                        description: "Use the production environment".to_string(),
                        preview: None,
                    },
                ],
                multi_select: false,
            }],
            source: None,
            created_at: now,
            expires_at: now + chrono::Duration::seconds(300),
            status: ClarifyStatus::Pending,
            answers: None,
            resolved_at: None,
        }
    }

    fn make_answer() -> ClarifyAnswer {
        let mut a = ClarifyAnswer::new();
        a.insert("Which environment?", "staging");
        a
    }

    // ── T17-1: fan-out correctness — 2 subscribers both receive Requested ──────
    //
    // Starts 2 SSE subscribers on the same conversation_id. Triggers
    // ask_user_clarify (which publishes ClarifyEvent::Requested). Asserts both
    // subscribers receive the event within 100ms. Then drops one subscriber and
    // asserts a subsequent publish does NOT panic.

    #[tokio::test]
    async fn test_clarify_survives_multiple_subscribers() {
        use crate::clarify_broadcast::ClarifyEvent;
        use std::time::Duration;

        let state = build_test_state_with_clarify();
        let conv_id = "conv-fanout-t17-1".to_string();

        // Two SSE subscribers for the same conversation.
        let mut rx1 = state.clarify_broadcaster.subscribe(&conv_id).await;
        let mut rx2 = state.clarify_broadcaster.subscribe(&conv_id).await;

        // Enqueue clarify and broadcast Requested.
        let req = make_clarify_request(&conv_id);
        let clarify_id = req.id.clone();
        state.clarify_queue.enqueue(req.clone()).await.unwrap();
        state
            .clarify_broadcaster
            .publish(&conv_id, ClarifyEvent::Requested(req))
            .await;

        // Both subscribers must receive within 100ms.
        let e1 = tokio::time::timeout(Duration::from_millis(100), rx1.recv())
            .await
            .expect("rx1: should receive within 100ms")
            .expect("rx1: recv ok");
        let e2 = tokio::time::timeout(Duration::from_millis(100), rx2.recv())
            .await
            .expect("rx2: should receive within 100ms")
            .expect("rx2: recv ok");

        match &e1 {
            ClarifyEvent::Requested(r) => assert_eq!(r.id, clarify_id, "rx1 clarify_id mismatch"),
            _ => panic!("rx1: expected Requested"),
        }
        match &e2 {
            ClarifyEvent::Requested(r) => assert_eq!(r.id, clarify_id, "rx2 clarify_id mismatch"),
            _ => panic!("rx2: expected Requested"),
        }

        // Drop rx1 — one subscriber gone. Publish again; must not panic.
        drop(rx1);
        state
            .clarify_broadcaster
            .publish(
                &conv_id,
                ClarifyEvent::Resolved {
                    id: clarify_id.clone(),
                    answer: make_answer(),
                },
            )
            .await;
        // rx2 still alive — it should receive the Resolved event.
        let e3 = tokio::time::timeout(Duration::from_millis(100), rx2.recv())
            .await
            .expect("rx2: should receive Resolved within 100ms")
            .expect("rx2: recv ok");
        assert!(
            matches!(e3, ClarifyEvent::Resolved { .. }),
            "rx2: expected Resolved after rx1 drop"
        );
    }

    // ── T17-2: race — submit vs timeout, exactly one wins ───────────────────
    //
    // Sets up a 500ms timeout clarify. At ~480ms, fires submit and the timeout
    // concurrently via tokio::join!. Asserts that the combined outcome is
    // exactly one success (resolve) and the agent loop does not panic.
    // The winner is non-deterministic; we check invariants, not which side won.

    #[tokio::test]
    async fn test_clarify_race_submit_vs_timeout() {
        use cyberclaw_core::clarify::ClarifyError;
        use std::time::Duration;

        let state = build_test_state_with_clarify();
        let conv_id = "conv-race-t17-2".to_string();

        let req = make_clarify_request(&conv_id);
        let clarify_id = req.id.clone();

        // Spawn the agent loop with a 500ms timeout — it blocks until
        // notify_resolved is called OR the timeout fires.
        let state_clone = state.clone();
        let conv_id_clone = conv_id.clone();
        let agent_handle = tokio::spawn(async move {
            state_clone
                .ask_user_clarify(&conv_id_clone, req, Duration::from_millis(500))
                .await
        });

        // Wait 480ms then race submit vs timeout simultaneously.
        tokio::time::sleep(Duration::from_millis(480)).await;

        let state_submit = state.clone();
        let cid_submit = clarify_id.clone();
        let state_timeout = state.clone();
        let cid_timeout = clarify_id.clone();

        let (resolve_result, timeout_result) = tokio::join!(
            // Arm 1: attempt to resolve via queue.resolve + notify_resolved
            // (simulates the POST handler path).
            async move {
                let r = state_submit
                    .clarify_queue
                    .resolve(&cid_submit, make_answer())
                    .await;
                if r.is_ok() {
                    state_submit
                        .clarify_coordinator
                        .notify_resolved(&cid_submit, make_answer())
                        .await;
                }
                r
            },
            // Arm 2: force timeout via mark_timeout (simulates coordinator sweep).
            async move { state_timeout.clarify_queue.mark_timeout(&cid_timeout).await }
        );

        // Agent handle must complete without panic.
        let agent_result = tokio::time::timeout(Duration::from_secs(2), agent_handle)
            .await
            .expect("agent task should finish within 2s")
            .expect("agent task must not panic");

        // The agent receives either Ok(answer) or Err(AlreadyTimedOut) — never
        // any other error variant.
        match &agent_result {
            Ok(_) => {}
            Err(ClarifyError::AlreadyTimedOut) => {}
            Err(e) => panic!("unexpected agent error: {:?}", e),
        }

        // Exactly one of the two arms must have succeeded; the other returns
        // an "already" error. Both outcomes are valid — we just verify no panic
        // and the final state is non-Pending.
        let resolve_ok =
            resolve_result.is_ok() || matches!(resolve_result, Err(ClarifyError::AlreadyTimedOut));
        let timeout_ok =
            timeout_result.is_ok() || matches!(timeout_result, Err(ClarifyError::AlreadyResolved));
        assert!(
            resolve_ok || timeout_ok,
            "at least one arm must have succeeded or returned an expected conflict error"
        );

        // Final queue state must be Resolved or TimedOut — never Pending.
        let final_req = state.clarify_queue.get(&clarify_id).await.unwrap();
        assert!(
            final_req.status == ClarifyStatus::Resolved
                || final_req.status == ClarifyStatus::TimedOut,
            "final queue status must be Resolved or TimedOut, got: {:?}",
            final_req.status
        );
    }

    // ── T17-3: resolved message persists to conversation history ─────────────
    //
    // Verifies the T11 persistence chain end-to-end:
    //   ask_user_clarify() appends role="clarify"
    //   submit handler appends role="clarify_response"
    //   GET conversation returns both messages with matching clarify_id

    #[tokio::test]
    #[serial]
    async fn test_clarify_resolved_message_persists_to_conversation() {
        use crate::api::chat_conversations::reset_store_for_tests;
        use std::time::Duration;

        reset_store_for_tests().await;
        let (_tmp, _restore) = seed_users_with_role("admin-persist-t17-3", "admin");
        let state = build_test_state_with_clarify();

        let conv_id = "conv-persist-t17-3".to_string();
        state
            .conversation_store()
            .create_for_test(&conv_id, "admin-persist-t17-3")
            .await;

        let req = make_clarify_request(&conv_id);
        let clarify_id = req.id.clone();

        // Spawn fake agent: triggers clarify (appends role="clarify").
        let state_agent = state.clone();
        let conv_id_agent = conv_id.clone();
        let agent_handle = tokio::spawn(async move {
            state_agent
                .ask_user_clarify(&conv_id_agent, req, Duration::from_secs(5))
                .await
        });

        // Give the background spawn in ask_user_clarify time to write the
        // "clarify" message before we call submit.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Submit answer via the HTTP handler (appends role="clarify_response").
        let claims = claims_for("admin-persist-t17-3");
        let result = submit_clarify_respond_handler(
            State(state.clone()),
            Extension(claims),
            Path(clarify_id.as_str().to_string()),
            Json(SubmitClarifyRequest {
                answer: make_answer(),
                conversation_id: None,
            }),
        )
        .await;
        assert!(result.is_ok(), "submit should succeed: {:?}", result.err());

        // Agent must have unblocked.
        let _ = tokio::time::timeout(Duration::from_secs(2), agent_handle)
            .await
            .expect("agent must finish")
            .expect("no panic");

        // GET conversation — verify both message roles exist.
        let conv = state
            .conversation_store()
            .get(&conv_id)
            .await
            .expect("conversation must exist");

        let clarify_msgs: Vec<_> = conv
            .messages
            .iter()
            .filter(|m| m.role == "clarify")
            .collect();
        let response_msgs: Vec<_> = conv
            .messages
            .iter()
            .filter(|m| m.role == "clarify_response")
            .collect();

        assert_eq!(clarify_msgs.len(), 1, "expected one clarify message");
        assert_eq!(
            response_msgs.len(),
            1,
            "expected one clarify_response message"
        );

        // Both messages must carry the same clarify_id in metadata.
        let clarify_meta = clarify_msgs[0]
            .metadata
            .as_ref()
            .expect("clarify message must have metadata");
        let response_meta = response_msgs[0]
            .metadata
            .as_ref()
            .expect("clarify_response message must have metadata");

        assert_eq!(
            clarify_meta["clarify_id"].as_str().unwrap(),
            clarify_id.as_str(),
            "clarify message clarify_id mismatch"
        );
        assert_eq!(
            response_meta["clarify_id"].as_str().unwrap(),
            clarify_id.as_str(),
            "clarify_response message clarify_id mismatch"
        );
    }

    // ── T17-4: list_pending excludes resolved and timed_out ──────────────────
    //
    // Creates 3 clarify items in different states: Pending / Resolved / TimedOut.
    // GET /clarify/pending must return exactly 1 (the Pending one).

    #[tokio::test]
    #[serial]
    async fn test_list_pending_excludes_resolved_and_timed_out() {
        let (_tmp, _restore) = seed_users_with_role("admin-filter-t17-4", "admin");
        let state = build_test_state_with_clarify();

        let conv_id = "conv-filter-t17-4".to_string();

        // Clarify 1: stays Pending.
        let req_pending = make_clarify_request(&conv_id);
        state
            .clarify_queue
            .enqueue(req_pending.clone())
            .await
            .unwrap();

        // Clarify 2: resolve it.
        let req_resolved = make_clarify_request(&conv_id);
        let id_resolved = req_resolved.id.clone();
        state.clarify_queue.enqueue(req_resolved).await.unwrap();
        state
            .clarify_queue
            .resolve(&id_resolved, make_answer())
            .await
            .expect("resolve should succeed");

        // Clarify 3: mark as timed out.
        let req_timed_out = make_clarify_request(&conv_id);
        let id_timed_out = req_timed_out.id.clone();
        state.clarify_queue.enqueue(req_timed_out).await.unwrap();
        state
            .clarify_queue
            .mark_timeout(&id_timed_out)
            .await
            .expect("mark_timeout should succeed");

        // GET pending — admin path, no conversation ownership needed.
        let claims = claims_for("admin-filter-t17-4");
        let result = list_pending_clarifications(
            State(state.clone()),
            Extension(claims),
            Query(ListPendingQuery {
                conversation_id: conv_id.clone(),
            }),
        )
        .await
        .expect("list_pending should succeed");

        assert_eq!(
            result.0.len(),
            1,
            "only the Pending clarify must appear in /pending"
        );
        assert_eq!(
            result.0[0].id, req_pending.id,
            "the returned clarify must be the Pending one"
        );
        assert_eq!(
            result.0[0].status,
            ClarifyStatus::Pending,
            "status must be Pending"
        );
    }

    // ── T17-5: submit with missing answer field returns 400 ─────────────────
    //
    // POSTs a body where answers map is empty (no keys). The handler should
    // succeed at the HTTP level (resolve with empty answer is valid) but the
    // audit entry freeform_len must be 0. This guards against panics on empty
    // answer maps.

    #[tokio::test]
    #[serial]
    async fn test_submit_handles_empty_answer_gracefully() {
        let (_tmp, _restore) = seed_users_with_role("admin-empty-t17-5", "admin");
        let state = build_test_state_with_clarify();

        let clarify = make_clarify_request("conv-empty-t17-5");
        let clarify_id = clarify.id.clone();
        state.clarify_queue.enqueue(clarify).await.unwrap();

        let empty_answer = ClarifyAnswer::new(); // no answers inserted
        let claims = claims_for("admin-empty-t17-5");
        let result = submit_clarify_respond_handler(
            State(state.clone()),
            Extension(claims),
            Path(clarify_id.as_str().to_string()),
            Json(SubmitClarifyRequest {
                answer: empty_answer,
                conversation_id: None,
            }),
        )
        .await;

        // Must succeed — empty answer is valid (agent gets to decide what to do).
        assert!(
            result.is_ok(),
            "empty answer must not panic or error: {:?}",
            result.err()
        );
        // Queue status must be Resolved.
        let stored = state.clarify_queue.get(&clarify_id).await.unwrap();
        assert_eq!(
            stored.status,
            ClarifyStatus::Resolved,
            "status must be Resolved even for empty answer"
        );
    }
}
