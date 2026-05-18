//! Audit API — read-only surface over the append-only [`AuditSink`].
//!
//! Sprint 8 Lane 1. Routes:
//!
//! | Method | Path                                | Purpose                                           |
//! |--------|-------------------------------------|---------------------------------------------------|
//! | GET    | `/api/v1/audit/logs`                | Tail N entries, with optional filters             |
//! | GET    | `/api/v1/audit/logs/stream`         | SSE live tail — new entries pushed as they arrive |
//! | GET    | `/api/v1/audit/logs/export`         | Stream the entire JSONL dump for archival         |
//! | GET    | `/api/v1/audit/verify`              | Re-hash every row, report first mismatch          |
//!
//! # Append-only
//!
//! There is **no `PUT` / `DELETE` / `PATCH` route**. The axum router will
//! return `405 Method Not Allowed` to any such request. This keeps the
//! "cannot be cleared via UI" invariant enforced at the HTTP layer.
//!
//! # Auth
//!
//! Every route on this module expects JWT and is mounted behind the
//! `protected_routes` middleware lane in [`crate::create_router_with_config`].
//! The SSE `/logs/stream` endpoint additionally accepts `?token=<jwt>` because
//! browsers' `EventSource` cannot set the `Authorization` header.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Extension, Query, State},
    http::HeaderMap,
    http::{header, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::get,
    Json, Router,
};
use futures::{stream, StreamExt as FuturesStreamExt};
use serde::{Deserialize, Serialize};
use tokio::time::interval;
use tokio_stream::wrappers::IntervalStream;
use tokio_stream::StreamExt as TokioStreamExt;
use tracing::warn;

use crate::audit::{AuditEntry, AuditKind, AuditQuery, VerifyReport};
use crate::error::ApiError;
use crate::middleware::{auth::Claims, verify_jwt};
use crate::state::AppState;

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1000;
/// Poll interval for the SSE live-tail stream.
const STREAM_POLL_INTERVAL: Duration = Duration::from_millis(1500);

/// `GET /api/v1/audit/logs` query params.
#[derive(Debug, Deserialize, Default)]
pub struct LogsQuery {
    pub limit: Option<usize>,
    pub actor: Option<String>,
    pub kind: Option<String>,
    pub action_prefix: Option<String>,
    pub since: Option<String>,
}

/// `GET /api/v1/audit/logs/stream` query params.
/// `token` is the JWT for `EventSource` callers that cannot set headers.
#[derive(Debug, Deserialize, Default)]
pub struct StreamQuery {
    pub kind: Option<String>,
    pub actor: Option<String>,
    pub token: Option<String>,
}

/// `GET /api/v1/audit/logs` response body.
#[derive(Debug, Serialize)]
pub struct LogsResponse {
    pub entries: Vec<AuditEntry>,
    pub total_returned: usize,
}

/// Build the `/api/v1/audit/*` router. All routes require JWT — the
/// caller must mount this inside the protected-routes lane.
/// Note: `/logs/stream` handles its own JWT validation (via `?token=` or
/// `Authorization` header) because `EventSource` cannot set headers.
pub fn create_audit_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/audit/logs", get(get_logs))
        .route("/api/v1/audit/logs/stream", get(stream_logs))
        .route("/api/v1/audit/logs/export", get(export_logs))
        .route("/api/v1/audit/verify", get(verify_chain))
}

/// `GET /api/v1/audit/logs` — tail the audit log.
async fn get_logs(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<LogsQuery>,
) -> Result<Json<LogsResponse>, ApiError> {
    let Some(sink) = state.audit.as_ref() else {
        warn!("audit: /logs requested but no sink configured");
        return Ok(Json(LogsResponse {
            entries: Vec::new(),
            total_returned: 0,
        }));
    };

    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    let kind = q.kind.as_deref().and_then(parse_kind);
    let since = q
        .since
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc));

    // Tenant isolation: non-admin tenanted callers see only their own actor entries
    // unless they explicitly requested a specific actor filter.
    let actor = if q.actor.is_none()
        && claims.tenant.is_some()
        && crate::middleware::auth::require_admin(&claims)
            .await
            .is_err()
    {
        Some(claims.sub.as_str().to_string())
    } else {
        q.actor
    };

    let filters = AuditQuery {
        actor,
        kind,
        action_prefix: q.action_prefix,
        since,
    };

    let entries = sink
        .tail(limit, &filters)
        .await
        .map_err(|e| ApiError::InternalError(format!("audit tail failed: {e}")))?;
    let total_returned = entries.len();
    Ok(Json(LogsResponse {
        entries,
        total_returned,
    }))
}

/// `GET /api/v1/audit/logs/stream` — SSE live tail of the audit log.
///
/// Polls the audit sink every 1.5 s and pushes any entries newer than the
/// last emitted timestamp as individual SSE `message` events. Each event
/// data is a JSON-serialised [`AuditEntry`].
///
/// Auth: accepts `Authorization: Bearer <jwt>` OR `?token=<jwt>` (required
/// for browsers' `EventSource` which cannot set custom headers).
async fn stream_logs(
    State(state): State<Arc<AppState>>,
    Query(q): Query<StreamQuery>,
    headers: HeaderMap,
) -> Response {
    // Resolve JWT from Authorization header or ?token= query param.
    let token_candidate = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(|t| t.to_string())
        .or(q.token);

    let Some(token) = token_candidate else {
        return (StatusCode::UNAUTHORIZED, "missing token").into_response();
    };

    if let Err(err) = verify_jwt(&token, state.jwt_secret.as_bytes()) {
        warn!(%err, "audit stream rejected: invalid jwt");
        return (StatusCode::UNAUTHORIZED, "invalid token").into_response();
    }

    let Some(sink) = state.audit.clone() else {
        // No audit sink configured — return an empty stream with keep-alive.
        let empty = stream::empty::<Result<Event, Infallible>>();
        return Sse::new(empty)
            .keep_alive(
                KeepAlive::new()
                    .interval(Duration::from_secs(15))
                    .text("keep-alive"),
            )
            .into_response();
    };

    let kind_filter = q.kind.as_deref().and_then(parse_kind);
    let actor_filter = q.actor.clone();

    // High-water mark: start from now so only entries arriving after this
    // connection is established are pushed.
    let last_ts = Arc::new(tokio::sync::Mutex::new(chrono::Utc::now()));

    let sse_stream = TokioStreamExt::then(IntervalStream::new(interval(STREAM_POLL_INTERVAL)), {
        let last_ts = Arc::clone(&last_ts);
        move |_| {
            let sink = sink.clone();
            let actor = actor_filter.clone();
            let kind = kind_filter.clone();
            let last_ts = Arc::clone(&last_ts);
            async move {
                let current_last = *last_ts.lock().await;
                let filters = AuditQuery {
                    actor,
                    kind,
                    action_prefix: None,
                    since: Some(current_last),
                };
                match sink.tail(1000, &filters).await {
                    Ok(mut new_entries) => {
                        // tail() returns newest-first; reverse to emit oldest-first.
                        new_entries.reverse();
                        let fresh: Vec<AuditEntry> = new_entries
                            .into_iter()
                            .filter(|e| e.ts > current_last)
                            .collect();
                        if let Some(newest) = fresh.iter().map(|e| e.ts).max() {
                            *last_ts.lock().await = newest;
                        }
                        fresh
                            .into_iter()
                            .map(|e| {
                                let data =
                                    serde_json::to_string(&e).unwrap_or_else(|_| "{}".to_string());
                                Ok::<Event, Infallible>(
                                    Event::default().event("message").data(data),
                                )
                            })
                            .collect::<Vec<_>>()
                    }
                    Err(err) => {
                        warn!(%err, "audit stream: tail poll failed");
                        vec![]
                    }
                }
            }
        }
    });

    let flat = FuturesStreamExt::flat_map(sse_stream, stream::iter);

    Sse::new(flat)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
}

/// `GET /api/v1/audit/logs/export` — stream the entire JSONL file.
///
/// Returns the raw bytes. Content-Type is `application/x-ndjson` so
/// downstream tooling (jq, fluent-bit, etc.) can parse line-by-line.
async fn export_logs(State(state): State<Arc<AppState>>) -> Result<Response, ApiError> {
    let Some(sink) = state.audit.as_ref() else {
        return Ok((StatusCode::OK, "").into_response());
    };
    let bytes = sink
        .export()
        .await
        .map_err(|e| ApiError::InternalError(format!("audit export failed: {e}")))?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-ndjson")],
        bytes,
    )
        .into_response())
}

/// `GET /api/v1/audit/verify` — re-hash every row and return the first mismatch.
///
/// When the log is intact the response has `corrupted_at = null` and
/// `ok_until = total`. Operators polling this endpoint can alert on any
/// non-null `corrupted_at`.
async fn verify_chain(State(state): State<Arc<AppState>>) -> Result<Json<VerifyReport>, ApiError> {
    let Some(sink) = state.audit.as_ref() else {
        warn!("audit: /verify requested but no sink configured");
        return Ok(Json(VerifyReport {
            total: 0,
            ok_until: 0,
            corrupted_at: None,
        }));
    };
    let report = sink
        .verify_chain()
        .await
        .map_err(|e| ApiError::InternalError(format!("audit verify failed: {e}")))?;
    Ok(Json(report))
}

fn parse_kind(s: &str) -> Option<AuditKind> {
    match s.to_ascii_lowercase().as_str() {
        "auth" => Some(AuditKind::Auth),
        "mutation" => Some(AuditKind::Mutation),
        "config" => Some(AuditKind::Config),
        "security" => Some(AuditKind::Security),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_helpers::build_test_state;
    use crate::audit::{AuditEntry, AuditKind, AuditResult};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_claims() -> Claims {
        use cyberclaw_core::ids::UserId;
        let now = chrono::Utc::now().timestamp();
        Claims {
            sub: UserId::from_string("test-user".to_string()).expect("valid user id"),
            tenant: None,
            iat: now,
            exp: now + 3600,
        }
    }

    fn app(state: Arc<AppState>) -> Router {
        create_audit_router()
            .with_state(state)
            .layer(axum::Extension(test_claims()))
    }

    #[tokio::test]
    async fn audit_no_delete_route() {
        let state = build_test_state();
        let app = app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/v1/audit/logs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn audit_no_update_route() {
        let state = build_test_state();
        let app = app(state);
        for method in ["PUT", "PATCH"] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri("/api/v1/audit/logs")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "method {method} must be 405"
            );
        }
    }

    #[tokio::test]
    async fn audit_export_streams_file() {
        use tempfile::TempDir;
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("audit.db");
        let sink = Arc::new(crate::audit::AuditSink::new(path).await.unwrap());
        sink.record(AuditEntry::now(
            "alice".to_string(),
            AuditKind::Auth,
            "login.success".to_string(),
            None,
            serde_json::json!({"ip": "127.0.0.1"}),
            AuditResult::Success,
        ))
        .await;

        let mut state = build_test_state();
        // Replace the audit sink on the AppState. `build_test_state`
        // builds without a sink by default; inject ours.
        let state_mut = Arc::get_mut(&mut state).expect("unique Arc in test");
        state_mut.audit = Some(sink);

        let app = app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/audit/logs/export")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let content_type = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(content_type.starts_with("application/x-ndjson"));
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.contains("login.success"));
        assert!(text.contains("alice"));
    }

    #[tokio::test]
    async fn audit_logs_tail_returns_entries() {
        use tempfile::TempDir;
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("audit.db");
        let sink = Arc::new(crate::audit::AuditSink::new(path).await.unwrap());
        sink.record(AuditEntry::now(
            "op".to_string(),
            AuditKind::Mutation,
            "task.create".to_string(),
            Some("task:t1".to_string()),
            serde_json::json!({}),
            AuditResult::Success,
        ))
        .await;

        let mut state = build_test_state();
        let state_mut = Arc::get_mut(&mut state).expect("unique Arc in test");
        state_mut.audit = Some(sink);

        let app = app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/audit/logs?limit=10")
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
        assert_eq!(json["total_returned"], 1);
        let entries = json["entries"].as_array().unwrap();
        assert_eq!(entries[0]["actor"], "op");
        assert_eq!(entries[0]["action"], "task.create");
    }
}
