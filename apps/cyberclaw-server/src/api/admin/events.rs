//! Admin console live event stream — `/admin/events` SSE.
//!
//! Sprint 7. Exposes a Server-Sent Events stream that the admin SPA can
//! consume via `EventSource`. Events are broadcast through a
//! `tokio::sync::broadcast::Sender<AdminEvent>` held on [`AppState`];
//! any component in the server process can publish to the same bus
//! without threading additional wires through constructors.
//!
//! # Auth
//!
//! This endpoint accepts JWT via **either**:
//! - `Authorization: Bearer <jwt>` header (preferred), OR
//! - `?token=<jwt>` query parameter — required because `EventSource` in
//!   browsers cannot set arbitrary request headers.
//!
//! The handler validates the token against `AppState::jwt_secret` using the
//! same [`crate::middleware::verify_jwt`] helper every other protected
//! route uses. When neither mechanism yields a valid token the response
//! is `401 Unauthorized`.
//!
//! # Architectural compliance (EVOLUTION_IDIOMS §2)
//!
//! [`AdminEvent`] is the outward event contract; the bus producer side
//! mirrors the `XxxEventSink` pattern (§2). A no-op drop default is
//! automatic: publishers call `state.admin_event_bus.send(event)` and
//! tolerate `Err(SendError)` when no subscribers exist — the bus quietly
//! discards events in that case, matching the `NoopEventSink` semantics.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Query, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::get,
    Router,
};
use futures::stream::StreamExt;
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::BroadcastStream;
use tracing::{debug, warn};

use chrono::{DateTime, Utc};
use cyberclaw_core::ids::ClarifyId;

use crate::middleware::verify_jwt;
use crate::state::AppState;

/// Capacity for the in-process admin event bus.
///
/// Subscribers that lag by more than this many events will see `Err(Lagged)`
/// on their stream — surfaced as an SSE comment so the SPA can re-sync.
pub const ADMIN_EVENT_BUS_CAPACITY: usize = 256;

/// Events published on the admin bus. Keep the enum small and the field
/// set stable — every variant is wire-visible to the SPA.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AdminEvent {
    /// Heartbeat tick. Emitted every ~5s so the SPA can confirm the
    /// stream is live even when no real events have fired.
    Heartbeat { at: String },
    /// An execution transitioned into Running.
    ExecutionStarted {
        execution_id: String,
        agent: String,
        capability: String,
    },
    /// An execution reached a terminal state (Completed/Failed/Cancelled).
    ExecutionCompleted {
        execution_id: String,
        status: String,
    },
    /// A review request was created.
    ReviewCreated {
        review_id: String,
        execution_id: String,
        risk_level: String,
    },
    /// A review was approved or rejected.
    ReviewResolved { review_id: String, decision: String },
    /// A task's status changed.
    TaskStatusChanged { task_id: String, status: String },
    /// A clarify request was raised and is waiting for user input.
    /// Does not carry question text — the SPA should fetch the full
    /// clarify payload via `GET /api/v1/clarify/{clarify_id}`.
    ClarifyRequested {
        clarify_id: ClarifyId,
        conversation_id: String,
        created_at: DateTime<Utc>,
    },
    /// A clarify request was resolved (answered or dismissed).
    ClarifyResolved {
        clarify_id: ClarifyId,
        conversation_id: String,
        resolved_at: DateTime<Utc>,
    },
    /// A conversation's message history was compressed.
    ConversationCompressed {
        conversation_id: String,
        compressed_at: DateTime<Utc>,
    },
}

/// Query params on `/admin/events`. `token` is the JWT when the caller
/// cannot set an Authorization header (browsers' `EventSource`).
#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    #[serde(default)]
    pub token: Option<String>,
}

/// Build the admin events router. Not JWT-gated at the middleware layer
/// — the handler itself validates `?token=` because `EventSource` cannot
/// set headers.
pub fn create_admin_events_router() -> Router<Arc<AppState>> {
    Router::new().route("/admin/events", get(admin_events))
}

/// `GET /admin/events` — SSE stream.
///
/// Returns an indefinite stream of [`AdminEvent`] JSON payloads plus a
/// periodic keep-alive comment. The stream terminates only when the
/// client disconnects.
async fn admin_events(
    State(state): State<Arc<AppState>>,
    Query(query): Query<EventsQuery>,
    headers: HeaderMap,
) -> Response {
    // Resolve JWT from either the Authorization header or ?token=.
    let token_candidate = headers
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(|t| t.to_string())
        .or(query.token);

    let Some(token) = token_candidate else {
        return (StatusCode::UNAUTHORIZED, "missing token").into_response();
    };

    if let Err(err) = verify_jwt(&token, state.jwt_secret.as_bytes()) {
        warn!(%err, "admin events rejected: invalid jwt");
        return (StatusCode::UNAUTHORIZED, "invalid token").into_response();
    }

    let rx = state.admin_event_bus.subscribe();
    debug!("admin events subscriber attached");

    // Fold the broadcast stream into SSE events. Broadcast lag becomes
    // a comment so the SPA sees the gap but stays connected.
    let stream = BroadcastStream::new(rx).map(to_sse_event);

    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
}

/// Convert one broadcast item into a `Result<Event, Infallible>`.
///
/// Using `Infallible` means the axum SSE driver never terminates the
/// stream on an internal error; lag is encoded as a comment on a
/// data-free event.
fn to_sse_event(
    item: Result<AdminEvent, tokio_stream::wrappers::errors::BroadcastStreamRecvError>,
) -> Result<Event, Infallible> {
    match item {
        Ok(event) => {
            let payload = serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string());
            Ok(Event::default().event("message").data(payload))
        }
        Err(err) => {
            // Lagged — encode as a comment so the SPA logs a gap.
            Ok(Event::default().comment(format!("lagged: {err}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_helpers::{build_test_state, jwt_for};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn events_app(state: Arc<AppState>) -> Router {
        create_admin_events_router().with_state(state)
    }

    #[tokio::test]
    async fn rejects_without_token() {
        let state = build_test_state();
        let app = events_app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn rejects_invalid_token() {
        let state = build_test_state();
        let app = events_app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/events?token=not-a-jwt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn accepts_query_param_token_and_emits_heartbeat() {
        let state = build_test_state();
        let token = jwt_for(&state, "op-events");

        // Publish an event so the subscriber's stream has something to
        // drain without waiting on the 15s keep-alive.
        let bus = state.admin_event_bus.clone();
        let app = events_app(state);
        let req = Request::builder()
            .method("GET")
            .uri(format!("/admin/events?token={}", token))
            .body(Body::empty())
            .unwrap();
        let resp_fut = app.oneshot(req);

        // Give the server a tick to subscribe, then publish.
        let publish_fut = async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = bus.send(AdminEvent::Heartbeat {
                at: "2026-04-18T00:00:00Z".to_string(),
            });
        };

        let (resp, _) = tokio::join!(resp_fut, publish_fut);
        let resp = resp.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap().starts_with("text/event-stream"))
            .unwrap_or(false));
    }

    #[test]
    fn admin_event_serializes_with_kind_tag() {
        let event = AdminEvent::ExecutionStarted {
            execution_id: "ex-1".to_string(),
            agent: "research-analyst".to_string(),
            capability: "web.search".to_string(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["kind"], "execution_started");
        assert_eq!(json["execution_id"], "ex-1");
    }
}
