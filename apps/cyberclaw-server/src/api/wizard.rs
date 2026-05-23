//! Onboarding Web UI — first-party server module.
//!
//! This module exposes the `/wizard/*` HTTP surface that drives the same
//! [`WizardEngine`](cyberclaw_control_plane::wizard_engine::WizardEngine)
//! state machine used by `cyberclaw onboard` (TUI). The engine, plan, and
//! translation helpers all live in `cyberclaw-control-plane`; this module
//! is a thin HTTP adapter that implements a [`WizardStepHandler`] backed
//! by tokio channels, so the browser can supply answers over REST while
//! the engine runs in a background task.
//!
//! # Routes
//!
//! All routes live under `/wizard`:
//!
//! | Route | Body/Query | Response |
//! |---|---|---|
//! | `POST /wizard/start` | `{plan_id?}` | `{session_id}` |
//! | `POST /wizard/next` | `{session_id, answer?}` | `{step_index, step, done}` |
//! | `POST /wizard/cancel` | `{session_id}` | `{ok:true}` |
//! | `GET  /wizard/status?session_id=X` | — | `{session_id, state, current_step_idx, total_steps}` |
//! | `GET  /wizard/events?session_id=X` | — | SSE stream of wizard events |
//! | `GET  /wizard` | — | `text/html` static page |
//!
//! # Architectural compliance
//!
//! - **Idiom §1 (Injected trait dispatcher)** — [`WebWizardStepHandler`] is
//!   a `WizardStepHandler` implementation passed to the engine; the engine
//!   itself is untouched.
//! - **Idiom §2 (Event sink trait)** — every session gets a
//!   [`VecWizardEventSink`] which the SSE endpoint polls for new events.
//! - **Idiom §3 (Plan-in + events-out)** — the plan is built via
//!   [`build_onboard_plan`] (shared with the TUI) and flows as pure data.
//! - **Idiom §4 (Facades from owner crate)** — N/A, no new capability.
//! - **Idiom §5 (No covert sixth object)** — this is a plain first-party
//!   server module. It is NOT a Platform Plugin: plugin runtime has no
//!   HTTP contribution surface yet (that arrives in Sprint 7+).
//! - **Idiom §6 (Skill is methodology)** — N/A, no Skill bundle.
//!
//! # Bootstrap token (deferred to Sprint 6)
//!
//! The wizard routes are **unauthenticated** in this MVP because the
//! operator configures the server via the wizard (chicken-and-egg). A
//! production deployment MUST either:
//!
//! 1. Bind the server to `127.0.0.1` / a Unix socket, or
//! 2. Add a `CYBERCLAW_BOOTSTRAP_TOKEN` middleware (Sprint 6 work).
//!
//! This module does NOT implement (2). A banner is rendered on the
//! static HTML page stating the MVP restriction.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::{
    extract::{Query, Request, State},
    http::{header, StatusCode},
    middleware::{from_fn_with_state, Next},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use futures::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{info, warn};
use uuid::Uuid;

use cyberclaw_control_plane::wizard_engine::{
    build_onboard_plan, session_to_operator_record, HandlerError, VecWizardEventSink, WizardAnswer,
    WizardEngine, WizardEvent, WizardSession, WizardState, WizardStep, WizardStepHandler,
};
use cyberclaw_core::users::UsersConfig;

use crate::error::ApiError;
use crate::middleware::verify_jwt;
use crate::state::AppState;

/// HTTP header name for the first-run wizard bootstrap token.
pub const BOOTSTRAP_TOKEN_HEADER: &str = "X-Wizard-Bootstrap-Token";

// ============================================================================
// Registry types (stored in AppState)
// ============================================================================

/// Per-session handle owned by [`WizardSessionRegistry`].
///
/// A session is "live" while the background task is running. The HTTP
/// layer interacts with it via two channels:
///
/// - `step_rx` — background task emits the next step to render.
/// - `answer_tx` — HTTP `/wizard/next` pushes the answer back.
pub struct WizardSessionHandle {
    /// Receives the next `WizardStep` the engine wants handled.
    /// `None` signals the engine has finished (session terminal).
    pub step_rx: Mutex<mpsc::Receiver<Option<WizardStep>>>,
    /// Sends an answer (or cancellation) back to the handler.
    pub answer_tx: mpsc::Sender<HandlerResponse>,
    /// Shared event sink so `/wizard/events` can stream to the browser.
    pub event_sink: Arc<VecWizardEventSink>,
    /// Handle to the driving task. Held so we can detect completion.
    pub task_handle: Mutex<Option<JoinHandle<WizardSession>>>,
    /// Index of the current step awaiting an answer, tracked server-side
    /// so `/wizard/status` is cheap.
    pub current_step_idx: Mutex<usize>,
    /// Cached total step count for `/wizard/status`.
    pub total_steps: usize,
    /// Terminal state (populated once the task completes).
    pub terminal_state: Mutex<Option<WizardState>>,
    /// Signals the background task to abort on cancellation.
    pub cancel_tx: Mutex<Option<oneshot::Sender<()>>>,
}

/// What the HTTP layer returns to the handler for a given step.
#[derive(Debug)]
pub enum HandlerResponse {
    /// An answer was supplied.
    Answer(WizardAnswer),
    /// The user cancelled the session.
    Cancel,
}

/// Registry of live wizard sessions. Stored on [`AppState`].
#[derive(Default)]
pub struct WizardSessionRegistry {
    inner: RwLock<HashMap<String, Arc<WizardSessionHandle>>>,
}

impl WizardSessionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, id: String, handle: Arc<WizardSessionHandle>) {
        self.inner.write().await.insert(id, handle);
    }

    pub async fn get(&self, id: &str) -> Option<Arc<WizardSessionHandle>> {
        self.inner.read().await.get(id).cloned()
    }

    pub async fn remove(&self, id: &str) -> Option<Arc<WizardSessionHandle>> {
        self.inner.write().await.remove(id)
    }
}

// ============================================================================
// Web handler — implements WizardStepHandler
// ============================================================================

/// [`WizardStepHandler`] backed by tokio channels. Each `handle()` call
/// pushes the step to the HTTP layer, then awaits the answer.
pub struct WebWizardStepHandler {
    step_tx: mpsc::Sender<Option<WizardStep>>,
    answer_rx: Mutex<mpsc::Receiver<HandlerResponse>>,
}

impl WebWizardStepHandler {
    pub fn new(
        step_tx: mpsc::Sender<Option<WizardStep>>,
        answer_rx: mpsc::Receiver<HandlerResponse>,
    ) -> Self {
        Self {
            step_tx,
            answer_rx: Mutex::new(answer_rx),
        }
    }
}

#[async_trait]
impl WizardStepHandler for WebWizardStepHandler {
    async fn handle(&self, step: &WizardStep) -> Result<WizardAnswer, HandlerError> {
        // Push the step to the HTTP layer. If the receiver is gone, the
        // HTTP session was dropped — treat as cancellation.
        if self.step_tx.send(Some(step.clone())).await.is_err() {
            return Err(HandlerError::Cancelled);
        }

        // Await the answer.
        let mut rx = self.answer_rx.lock().await;
        match rx.recv().await {
            Some(HandlerResponse::Answer(a)) => Ok(a),
            Some(HandlerResponse::Cancel) => Err(HandlerError::Cancelled),
            None => Err(HandlerError::Cancelled),
        }
    }
}

// ============================================================================
// Request / response DTOs
// ============================================================================

#[derive(Debug, Deserialize, Default)]
pub struct StartRequest {
    #[serde(default)]
    pub plan_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StartResponse {
    pub session_id: String,
}

#[derive(Debug, Deserialize)]
pub struct NextRequest {
    pub session_id: String,
    #[serde(default)]
    pub answer: Option<WizardAnswer>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NextResponse {
    pub step_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<WizardStep>,
    pub done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_state: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CancelRequest {
    pub session_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CancelResponse {
    pub ok: bool,
}

#[derive(Debug, Deserialize)]
pub struct SessionQuery {
    pub session_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StatusResponse {
    pub session_id: String,
    pub state: String,
    pub current_step_idx: usize,
    pub total_steps: usize,
}

// ============================================================================
// Router
// ============================================================================

/// Build the `/wizard/*` router.
///
/// Mounted on the PUBLIC (no-JWT) layer because onboarding must be reachable
/// before any operator exists. Access is gated by [`bootstrap_auth`] which
/// accepts EITHER a valid `X-Wizard-Bootstrap-Token` header (first-run) OR a
/// valid JWT Bearer token (existing operators re-running the wizard).
pub fn create_wizard_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/wizard", get(wizard_static))
        .route("/wizard/start", post(wizard_start))
        .route("/wizard/next", post(wizard_next))
        .route("/wizard/cancel", post(wizard_cancel))
        .route("/wizard/status", get(wizard_status))
        .route("/wizard/events", get(wizard_events))
}

/// Build the `/wizard/*` router pre-wrapped with the [`bootstrap_auth`]
/// middleware against the supplied [`AppState`].
///
/// This is the form `cyberclaw-server/src/lib.rs` composes into the public
/// routes. Tests can still use [`create_wizard_router`] without the layer
/// by calling `.with_state(state)` directly.
pub fn create_wizard_router_with_auth(state: Arc<AppState>) -> Router<Arc<AppState>> {
    create_wizard_router().layer(from_fn_with_state(state, bootstrap_auth))
}

/// Auth middleware for `/wizard/*`.
///
/// Accepts the request when either:
/// - The `X-Wizard-Bootstrap-Token` header matches the currently-stored
///   one-shot token (first-run flow), OR
/// - The `Authorization: Bearer <jwt>` header contains a valid JWT signed
///   with the server's secret (existing operator re-running the wizard).
///
/// Rejects with `401 Unauthorized` when no bootstrap token is set AND no
/// valid JWT is present. The static HTML shell at `GET /wizard` is NOT
/// exempt: the page includes a form to enter the token which is then sent
/// with subsequent API requests, so the shell is also gated.
pub async fn bootstrap_auth(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    // 1) Header-based bootstrap token (preferred during first-run).
    let header_token = req
        .headers()
        .get(BOOTSTRAP_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    if let Some(stored) = state.wizard_bootstrap_token.read().await.as_ref().cloned() {
        if let Some(got) = header_token.as_deref() {
            // Constant-time compare to avoid timing leaks.
            if got.as_bytes().ct_eq(stored.as_bytes()).into() {
                return Ok(next.run(req).await);
            }
        }
    }

    // 2) JWT fallback for already-registered operators.
    if let Some(auth_header) = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
    {
        if let Some(token) = auth_header.strip_prefix("Bearer ") {
            if verify_jwt(token, state.jwt_secret.as_bytes()).is_ok() {
                return Ok(next.run(req).await);
            }
        }
    }

    Err(ApiError::Unauthorized(
        "wizard requires a valid bootstrap token or JWT".to_string(),
    ))
}

/// Static HTML shell served at `GET /wizard`.
const WIZARD_HTML: &str = include_str!("wizard.html");

async fn wizard_static() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        WIZARD_HTML,
    )
        .into_response()
}

// ============================================================================
// POST /wizard/start
// ============================================================================

async fn wizard_start(
    State(state): State<Arc<AppState>>,
    Json(_req): Json<StartRequest>,
) -> Result<Json<StartResponse>, ApiError> {
    let session_id = format!("wiz-{}", Uuid::new_v4());

    // Build the onboarding plan. Skill discovery reads from the default
    // ecosystem path; the web UI is not configurable on this axis yet.
    let skills_dir = PathBuf::from("ecosystem/skills");
    let plan = build_onboard_plan(&skills_dir);
    let total_steps = plan.steps.len();

    // Step + answer channels wired into WebWizardStepHandler.
    let (step_tx, step_rx) = mpsc::channel::<Option<WizardStep>>(8);
    let (answer_tx, answer_rx) = mpsc::channel::<HandlerResponse>(4);
    let (cancel_tx, _cancel_rx) = oneshot::channel::<()>();

    let event_sink = Arc::new(VecWizardEventSink::new());
    let session = WizardSession::new(session_id.clone(), &plan);

    // Clone what we need to hand to the background task.
    let sink_for_task = event_sink.clone();
    let step_tx_for_task = step_tx.clone();
    let session_id_for_task = session_id.clone();
    let bootstrap_slot = state.wizard_bootstrap_token.clone();

    // Spawn the engine driver.
    let task_handle: JoinHandle<WizardSession> = tokio::spawn(async move {
        let handler = WebWizardStepHandler::new(step_tx_for_task.clone(), answer_rx);
        let engine = WizardEngine::new().with_event_sink(sink_for_task);
        let finished = engine.run(&plan, &handler, session).await;

        // Sprint 5: on a clean completion, upsert the operator record into
        // `~/.cyberclaw/users.toml` and discard the bootstrap token. On any
        // non-Done terminal state we leave the token alone so the operator
        // can retry.
        if matches!(finished.state, WizardState::Done) {
            persist_operator_and_clear_bootstrap(&finished, &bootstrap_slot).await;
        }

        // Signal completion to any `/wizard/next` caller blocked on step_rx.
        let _ = step_tx_for_task.send(None).await;
        info!(
            session_id = %session_id_for_task,
            state = ?finished.state,
            "wizard session finished"
        );
        finished
    });

    let handle = Arc::new(WizardSessionHandle {
        step_rx: Mutex::new(step_rx),
        answer_tx,
        event_sink,
        task_handle: Mutex::new(Some(task_handle)),
        current_step_idx: Mutex::new(0),
        total_steps,
        terminal_state: Mutex::new(None),
        cancel_tx: Mutex::new(Some(cancel_tx)),
    });

    state
        .wizard_registry
        .insert(session_id.clone(), handle)
        .await;

    Ok(Json(StartResponse { session_id }))
}

// ============================================================================
// POST /wizard/next
// ============================================================================

async fn wizard_next(
    State(state): State<Arc<AppState>>,
    Json(req): Json<NextRequest>,
) -> Result<Json<NextResponse>, ApiError> {
    let handle = state
        .wizard_registry
        .get(&req.session_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("unknown session: {}", req.session_id)))?;

    // If an answer was provided, deliver it to the handler first.
    if let Some(answer) = req.answer {
        // Sensitive-text answers are masked by the engine before event
        // emission; logging happens only via masked().
        let masked = answer.masked();
        info!(session_id = %req.session_id, ?masked, "wizard answer submitted");

        if handle
            .answer_tx
            .send(HandlerResponse::Answer(answer))
            .await
            .is_err()
        {
            // Background task already terminated.
            return finalize_response(&handle).await;
        }
    }

    // Await the next step (or termination signal).
    let next_step = {
        let mut rx = handle.step_rx.lock().await;
        // Use a short timeout so we don't hang forever if the task dies
        // without emitting the final `None`.
        tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .unwrap_or(Some(None))
    };

    match next_step {
        Some(Some(step)) => {
            let idx = {
                let mut guard = handle.current_step_idx.lock().await;
                let current = *guard;
                // The step we just received is the one at `current`. Next
                // /next call (after user submits) advances to current+1.
                *guard = current.saturating_add(1);
                current
            };
            Ok(Json(NextResponse {
                step_index: idx,
                step: Some(step),
                done: false,
                terminal_state: None,
            }))
        }
        _ => finalize_response(&handle).await,
    }
}

async fn finalize_response(
    handle: &Arc<WizardSessionHandle>,
) -> Result<Json<NextResponse>, ApiError> {
    // Join the task if still running, record terminal state.
    let mut terminal_guard = handle.terminal_state.lock().await;
    if terminal_guard.is_none() {
        let mut task_slot = handle.task_handle.lock().await;
        if let Some(task) = task_slot.take() {
            match task.await {
                Ok(session) => {
                    *terminal_guard = Some(session.state);
                }
                Err(e) => {
                    warn!(error = %e, "wizard task join failed");
                    *terminal_guard = Some(WizardState::Error(format!("task join: {}", e)));
                }
            }
        }
    }
    let state_str = match &*terminal_guard {
        Some(WizardState::Done) => "done",
        Some(WizardState::Cancelled) => "cancelled",
        Some(WizardState::Error(_)) => "error",
        Some(WizardState::Running) | None => "unknown",
    };
    let idx = *handle.current_step_idx.lock().await;
    Ok(Json(NextResponse {
        step_index: idx,
        step: None,
        done: true,
        terminal_state: Some(state_str.to_string()),
    }))
}

// ============================================================================
// POST /wizard/cancel
// ============================================================================

async fn wizard_cancel(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CancelRequest>,
) -> Result<Json<CancelResponse>, ApiError> {
    let handle = state
        .wizard_registry
        .get(&req.session_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("unknown session: {}", req.session_id)))?;

    // Fire the cancel token (not currently observed by the engine, but
    // kept for future abort-on-timeout work).
    {
        let mut slot = handle.cancel_tx.lock().await;
        if let Some(tx) = slot.take() {
            let _ = tx.send(());
        }
    }

    // Push a cancel response to the handler. If the handler is not
    // currently awaiting, the message goes into the buffer and is
    // consumed on the next step.
    let _ = handle.answer_tx.send(HandlerResponse::Cancel).await;

    Ok(Json(CancelResponse { ok: true }))
}

// ============================================================================
// GET /wizard/status
// ============================================================================

async fn wizard_status(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SessionQuery>,
) -> Result<Json<StatusResponse>, ApiError> {
    let handle = state
        .wizard_registry
        .get(&q.session_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("unknown session: {}", q.session_id)))?;

    let state_str = {
        let terminal = handle.terminal_state.lock().await;
        match &*terminal {
            Some(WizardState::Done) => "done",
            Some(WizardState::Cancelled) => "cancelled",
            Some(WizardState::Error(_)) => "error",
            Some(WizardState::Running) | None => "running",
        }
        .to_string()
    };
    let current_step_idx = *handle.current_step_idx.lock().await;

    Ok(Json(StatusResponse {
        session_id: q.session_id,
        state: state_str,
        current_step_idx,
        total_steps: handle.total_steps,
    }))
}

// ============================================================================
// GET /wizard/events (SSE)
// ============================================================================

async fn wizard_events(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SessionQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, ApiError> {
    let handle = state
        .wizard_registry
        .get(&q.session_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("unknown session: {}", q.session_id)))?;

    let sink = handle.event_sink.clone();

    // State: (last_len, pending_batch). We poll the sink at a small
    // interval and emit any events added since the last snapshot. Using
    // `futures::stream::unfold` avoids pulling in a new crate for macros
    // (async-stream / tokio-stream).
    struct StreamState {
        sink: Arc<VecWizardEventSink>,
        last_len: usize,
        buffered: std::collections::VecDeque<String>,
        terminal_seen: bool,
    }

    let init = StreamState {
        sink,
        last_len: 0,
        buffered: std::collections::VecDeque::new(),
        terminal_seen: false,
    };

    let stream = stream::unfold(init, |mut st| async move {
        loop {
            // Drain any buffered serialized events first.
            if let Some(data) = st.buffered.pop_front() {
                return Some((
                    Ok::<Event, std::convert::Infallible>(Event::default().data(data)),
                    st,
                ));
            }
            if st.terminal_seen {
                return None;
            }
            let events = st.sink.snapshot();
            if events.len() > st.last_len {
                for ev in &events[st.last_len..] {
                    if let Ok(data) = serialize_event(ev) {
                        st.buffered.push_back(data);
                    }
                    if is_terminal_event(ev) {
                        st.terminal_seen = true;
                    }
                }
                st.last_len = events.len();
                continue; // fall through to drain
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// Serialize an event to a JSON string for SSE data lines. Events that
/// carry sensitive answers have already been masked by the engine.
fn serialize_event(ev: &WizardEvent) -> Result<String, serde_json::Error> {
    // `WizardEvent` is not `Serialize`, so project into a small DTO.
    #[derive(Serialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum EventDto<'a> {
        SessionStarted {
            session_id: &'a str,
            plan_id: &'a str,
        },
        StepEntered {
            step_idx: usize,
            step_id: &'a str,
        },
        AnswerCaptured {
            step_id: &'a str,
            answer: &'a WizardAnswer,
        },
        StepSkipped {
            step_idx: usize,
            reason: &'a str,
        },
        SessionCompleted,
        SessionCancelled {
            at_step: usize,
        },
        SessionErrored {
            error: &'a str,
        },
    }
    let dto = match ev {
        WizardEvent::SessionStarted {
            session_id,
            plan_id,
        } => EventDto::SessionStarted {
            session_id,
            plan_id,
        },
        WizardEvent::StepEntered { step_idx, step_id } => EventDto::StepEntered {
            step_idx: *step_idx,
            step_id,
        },
        WizardEvent::AnswerCaptured { step_id, answer } => {
            EventDto::AnswerCaptured { step_id, answer }
        }
        WizardEvent::StepSkipped { step_idx, reason } => EventDto::StepSkipped {
            step_idx: *step_idx,
            reason,
        },
        WizardEvent::SessionCompleted { .. } => EventDto::SessionCompleted,
        WizardEvent::SessionCancelled { at_step } => {
            EventDto::SessionCancelled { at_step: *at_step }
        }
        WizardEvent::SessionErrored { error } => EventDto::SessionErrored { error },
    };
    serde_json::to_string(&dto)
}

/// Persist an [`cyberclaw_core::users::OperatorRecord`] for a completed
/// wizard session, then discard the bootstrap token.
///
/// Failures are logged but do not panic — the wizard has already written the
/// config; losing identity persistence is recoverable by rerunning the
/// wizard with the still-valid bootstrap token.
async fn persist_operator_and_clear_bootstrap(
    session: &WizardSession,
    bootstrap_slot: &Arc<RwLock<Option<String>>>,
) {
    let record = session_to_operator_record(session);
    let path = UsersConfig::default_path();

    let mut cfg = match UsersConfig::load_from_path(&path) {
        Ok(c) => c,
        Err(err) => {
            warn!(
                path = %path.display(),
                %err,
                "failed to load existing users.toml; starting with empty config"
            );
            UsersConfig::default()
        }
    };
    cfg.upsert(record);

    match cfg.save_to_path(&path) {
        Ok(()) => {
            info!(path = %path.display(), "persisted operator record to users.toml");
            // Token is single-use; drop it now that a real operator exists.
            *bootstrap_slot.write().await = None;
        }
        Err(err) => {
            warn!(
                path = %path.display(),
                %err,
                "failed to persist operator record; bootstrap token left in place"
            );
        }
    }
}

fn is_terminal_event(ev: &WizardEvent) -> bool {
    matches!(
        ev,
        WizardEvent::SessionCompleted { .. }
            | WizardEvent::SessionCancelled { .. }
            | WizardEvent::SessionErrored { .. }
    )
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AppState, ControlPlaneComponents};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use cyberclaw_connectors::registry::ConnectorRegistry;
    use cyberclaw_control_plane::{
        event_bus::InMemoryEventBus, execution_service::InMemoryExecutionService,
        gateway_router::InMemoryGatewayRouter, lease_manager::InMemoryLeaseManager,
        membership_service::InMemoryMembershipService, orchestrator::ControlPlaneOrchestrator,
        placement_engine::InMemoryPlacementEngine, registry::InMemoryRegistry,
        resolver::InMemoryResolver, review_queue::InMemoryReviewQueue,
        task_manager::InMemoryTaskManager,
    };
    use cyberclaw_governance::engine::DefaultPolicyEngine;
    use cyberclaw_llm::prelude::GenericOpenAiClient;
    use cyberclaw_llm::LlmClient;
    use cyberclaw_observability::events::InMemoryEventRecorder;
    use cyberclaw_observability::security_event_store::InMemorySecurityEventStore;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn build_test_state() -> Arc<AppState> {
        let llm_client: Arc<dyn LlmClient> =
            Arc::new(GenericOpenAiClient::new(None, "http://localhost:8080").unwrap());
        let connector_registry = Arc::new(ConnectorRegistry::new());
        let policy_engine = Arc::new(DefaultPolicyEngine::default());
        let event_recorder = Arc::new(InMemoryEventRecorder::new());
        let task_manager = Arc::new(InMemoryTaskManager::new());
        let execution_service = Arc::new(InMemoryExecutionService::with_event_recorder(
            event_recorder.clone(),
        ));
        let registry = Arc::new(InMemoryRegistry::new());
        let resolver = Arc::new(InMemoryResolver::new(registry.clone()));
        let review_queue = Arc::new(InMemoryReviewQueue::new(None));
        let gateway = Arc::new(InMemoryGatewayRouter::new());
        let placement_engine = Arc::new(InMemoryPlacementEngine);
        let lease_manager = Arc::new(InMemoryLeaseManager::default());
        let membership_service = Arc::new(InMemoryMembershipService::default());
        let event_bus = Arc::new(InMemoryEventBus::default());
        let security_event_store = Arc::new(InMemorySecurityEventStore::new());
        let plugin_registry = Arc::new(cyberclaw_plugin_runtime::PluginRegistry::new(
            std::path::PathBuf::from("/tmp/test_plugins_wizard"),
        ));
        let control_plane = Arc::new(
            ControlPlaneOrchestrator::new(
                gateway,
                resolver,
                registry.clone(),
                review_queue.clone(),
                task_manager.clone(),
                execution_service.clone(),
                placement_engine,
                lease_manager,
                membership_service,
                event_bus,
                policy_engine.clone(),
                plugin_registry,
                security_event_store,
            )
            .with_event_recorder(event_recorder.clone()),
        );
        let cp_components = ControlPlaneComponents {
            control_plane,
            task_manager,
            execution_service,
            review_queue,
            package_registry: registry,
        };
        let state = Arc::new(AppState::new(
            llm_client,
            connector_registry,
            policy_engine,
            event_recorder,
            "test-jwt-secret".to_string(),
            cp_components,
        ));
        // Tests exercise the inner router directly, bypassing the
        // `bootstrap_auth` middleware. Clear the token so whatever the host
        // filesystem had in `~/.cyberclaw/users.toml` at test time does not
        // leak into observable state. `try_write` works here because this
        // runs during test setup with no contention; falling back to
        // leaving the token populated would be harmless for the inner-route
        // tests anyway.
        if let Ok(mut slot) = state.wizard_bootstrap_token.try_write() {
            *slot = None;
        }
        state
    }

    fn wizard_app(state: Arc<AppState>) -> Router {
        create_wizard_router().with_state(state)
    }

    async fn post_json<T: serde::de::DeserializeOwned>(
        app: &Router,
        path: &str,
        body: serde_json::Value,
    ) -> (StatusCode, T) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let parsed: T = serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "decode {} body failed: {} — raw: {}",
                path,
                e,
                String::from_utf8_lossy(&bytes)
            )
        });
        (status, parsed)
    }

    #[tokio::test]
    async fn start_creates_session_id() {
        let state = build_test_state();
        let app = wizard_app(state.clone());
        let (status, body): (_, StartResponse) =
            post_json(&app, "/wizard/start", serde_json::json!({})).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.session_id.starts_with("wiz-"));
        // Registry should contain it.
        assert!(state.wizard_registry.get(&body.session_id).await.is_some());
    }

    #[tokio::test]
    async fn start_initial_state_is_running() {
        let state = build_test_state();
        let app = wizard_app(state.clone());
        let (_, start): (_, StartResponse) =
            post_json(&app, "/wizard/start", serde_json::json!({})).await;

        // Query status — should report running with 8 total steps (Sprint 5
        // added the `display_name` step between `welcome` and
        // `connection_mode`).
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/wizard/status?session_id={}", start.session_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let status_body: StatusResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status_body.state, "running");
        // v2 plan (Sprint 5 wave 2026-04-26+): 14 steps across all surfaces.
        // See WizardEngine::build_v2 in cyberclaw-control-plane/wizard_engine.rs.
        assert_eq!(status_body.total_steps, 14);
    }

    #[tokio::test]
    async fn next_advances_with_valid_answer() {
        let state = build_test_state();
        let app = wizard_app(state.clone());
        let (_, start): (_, StartResponse) =
            post_json(&app, "/wizard/start", serde_json::json!({})).await;

        // First call fetches the first step (welcome note).
        let (status, first): (_, NextResponse) = post_json(
            &app,
            "/wizard/next",
            serde_json::json!({ "session_id": start.session_id }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!first.done);
        assert_eq!(first.step_index, 0);
        assert!(first.step.is_some());

        // Submit ActionDone for the note; second call fetches step 1
        // (`display_name`, Text).
        let (_, second): (_, NextResponse) = post_json(
            &app,
            "/wizard/next",
            serde_json::json!({
                "session_id": start.session_id,
                "answer": { "kind": "action_done" }
            }),
        )
        .await;
        assert_eq!(second.step_index, 1);
        assert!(second.step.is_some());
    }

    #[tokio::test]
    async fn next_rejects_unknown_session_id() {
        let state = build_test_state();
        let app = wizard_app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/wizard/next")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "session_id": "does-not-exist"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn cancel_transitions_to_cancelled() {
        let state = build_test_state();
        let app = wizard_app(state.clone());
        let (_, start): (_, StartResponse) =
            post_json(&app, "/wizard/start", serde_json::json!({})).await;

        // Fetch the first step so the handler is actually blocked on an answer.
        let (_, _first): (_, NextResponse) = post_json(
            &app,
            "/wizard/next",
            serde_json::json!({ "session_id": start.session_id }),
        )
        .await;

        let (status, body): (_, CancelResponse) = post_json(
            &app,
            "/wizard/cancel",
            serde_json::json!({ "session_id": start.session_id }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.ok);

        // A follow-up /next should now report `done = true`, state = cancelled.
        let (_, fin): (_, NextResponse) = post_json(
            &app,
            "/wizard/next",
            serde_json::json!({ "session_id": start.session_id }),
        )
        .await;
        assert!(fin.done);
        assert_eq!(fin.terminal_state.as_deref(), Some("cancelled"));
    }

    #[tokio::test]
    async fn status_returns_current_step_index() {
        let state = build_test_state();
        let app = wizard_app(state.clone());
        let (_, start): (_, StartResponse) =
            post_json(&app, "/wizard/start", serde_json::json!({})).await;

        // Advance one step.
        let (_, _): (_, NextResponse) = post_json(
            &app,
            "/wizard/next",
            serde_json::json!({ "session_id": start.session_id }),
        )
        .await;

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/wizard/status?session_id={}", start.session_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: StatusResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.current_step_idx, 1);
        assert_eq!(body.state, "running");
    }

    #[tokio::test]
    async fn events_sse_emits_on_advance() {
        // Instead of consuming the SSE stream (which streams indefinitely),
        // directly assert that the registry's event sink accumulates
        // events as the session advances. The SSE endpoint is a thin
        // wrapper over `VecWizardEventSink::snapshot()`; exercising the
        // sink is the tractable unit-test surface.
        let state = build_test_state();
        let app = wizard_app(state.clone());
        let (_, start): (_, StartResponse) =
            post_json(&app, "/wizard/start", serde_json::json!({})).await;

        // Advance one step to produce StepEntered + AnswerCaptured.
        let (_, _): (_, NextResponse) = post_json(
            &app,
            "/wizard/next",
            serde_json::json!({ "session_id": start.session_id }),
        )
        .await;
        let (_, _): (_, NextResponse) = post_json(
            &app,
            "/wizard/next",
            serde_json::json!({
                "session_id": start.session_id,
                "answer": { "kind": "action_done" }
            }),
        )
        .await;

        // Give the background task a tick to emit events.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let handle = state
            .wizard_registry
            .get(&start.session_id)
            .await
            .expect("session present");
        let events = handle.event_sink.snapshot();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, WizardEvent::SessionStarted { .. })),
            "expected SessionStarted, got {:?}",
            events
        );
        assert!(events
            .iter()
            .any(|e| matches!(e, WizardEvent::StepEntered { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, WizardEvent::AnswerCaptured { .. })));
    }

    #[tokio::test]
    async fn static_html_returned_on_get_wizard() {
        let state = build_test_state();
        let app = wizard_app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/wizard")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(ct.starts_with("text/html"), "content-type was {:?}", ct);
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let html = std::str::from_utf8(&bytes).unwrap();
        assert!(html.contains("CyberClaw Onboarding"));
        assert!(html.contains("/wizard/start"));
    }

    #[tokio::test]
    async fn sensitive_answer_masked_in_events() {
        let state = build_test_state();
        let app = wizard_app(state.clone());
        let (_, start): (_, StartResponse) =
            post_json(&app, "/wizard/start", serde_json::json!({})).await;

        // v2 plan step layout (cyberclaw-control-plane wizard_engine::build_v2):
        //   0: welcome         (Note)
        //   1: display_name    (Text, non-sensitive)
        //   2: connection_mode (Select)
        //   3: llm_provider    (Select)
        //   4: llm_api_key     (Text, sensitive)   <-- masking checked here

        // Step 0 fetch + ack.
        let _: (_, NextResponse) = post_json(
            &app,
            "/wizard/next",
            serde_json::json!({ "session_id": start.session_id }),
        )
        .await;
        let _: (_, NextResponse) = post_json(
            &app,
            "/wizard/next",
            serde_json::json!({
                "session_id": start.session_id,
                "answer": { "kind": "action_done" }
            }),
        )
        .await;
        // Step 1 ack — display_name.
        let _: (_, NextResponse) = post_json(
            &app,
            "/wizard/next",
            serde_json::json!({
                "session_id": start.session_id,
                "answer": { "kind": "text", "value": "Alice" }
            }),
        )
        .await;
        // Step 2 ack — connection_mode.
        let _: (_, NextResponse) = post_json(
            &app,
            "/wizard/next",
            serde_json::json!({
                "session_id": start.session_id,
                "answer": { "kind": "selected_idx", "value": 0 }
            }),
        )
        .await;
        // Step 3 ack — llm_provider (v2 inserted this before api_key).
        let _: (_, NextResponse) = post_json(
            &app,
            "/wizard/next",
            serde_json::json!({
                "session_id": start.session_id,
                "answer": { "kind": "selected_idx", "value": 0 }
            }),
        )
        .await;
        // Step 4 ack — supply the secret text for llm_api_key.
        let _: (_, NextResponse) = post_json(
            &app,
            "/wizard/next",
            serde_json::json!({
                "session_id": start.session_id,
                "answer": { "kind": "text", "value": "super-secret-key" }
            }),
        )
        .await;

        tokio::time::sleep(Duration::from_millis(100)).await;

        let handle = state
            .wizard_registry
            .get(&start.session_id)
            .await
            .expect("session present");
        let events = handle.event_sink.snapshot();
        let captured = events
            .iter()
            .find_map(|e| match e {
                WizardEvent::AnswerCaptured { step_id, answer } if step_id == "llm_api_key" => {
                    Some(answer.clone())
                }
                _ => None,
            })
            .expect("llm_api_key AnswerCaptured event");
        match captured {
            WizardAnswer::Text(s) => assert_eq!(s, "***", "sensitive answer must be masked"),
            other => panic!("expected masked Text, got {:?}", other),
        }
    }

    // ------------------------------------------------------------------------
    // Sprint 5: bootstrap-token middleware tests
    // ------------------------------------------------------------------------

    /// Build a router that DOES include the `bootstrap_auth` middleware.
    fn wizard_app_with_auth(state: Arc<AppState>) -> Router {
        create_wizard_router_with_auth(state.clone()).with_state(state)
    }

    async fn set_bootstrap_token(state: &Arc<AppState>, token: Option<String>) {
        *state.wizard_bootstrap_token.write().await = token;
    }

    fn jwt_for(state: &Arc<AppState>, user: &str) -> String {
        use crate::middleware::auth::generate_jwt;
        use cyberclaw_core::ids::UserId;
        let uid = UserId::from_string(user.to_string()).expect("valid user id");
        generate_jwt(&uid, state.jwt_secret.as_bytes(), 3600).expect("jwt")
    }

    #[tokio::test]
    async fn wizard_bootstrap_token_required_when_users_empty() {
        let state = build_test_state();
        set_bootstrap_token(&state, Some("the-token".into())).await;
        let app = wizard_app_with_auth(state);

        // Missing header — should be 401.
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/wizard/start")
                    .header("content-type", "application/json")
                    .body(Body::from("{}".as_bytes().to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn wizard_accepts_valid_bootstrap_token() {
        let state = build_test_state();
        set_bootstrap_token(&state, Some("the-token".into())).await;
        let app = wizard_app_with_auth(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/wizard/start")
                    .header("content-type", "application/json")
                    .header(BOOTSTRAP_TOKEN_HEADER, "the-token")
                    .body(Body::from("{}".as_bytes().to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn wizard_rejects_wrong_bootstrap_token() {
        let state = build_test_state();
        set_bootstrap_token(&state, Some("the-token".into())).await;
        let app = wizard_app_with_auth(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/wizard/start")
                    .header("content-type", "application/json")
                    .header(BOOTSTRAP_TOKEN_HEADER, "not-the-token")
                    .body(Body::from("{}".as_bytes().to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn wizard_accepts_valid_jwt_when_users_exist() {
        let state = build_test_state();
        // Simulate post-first-run: no bootstrap token, but JWT works.
        set_bootstrap_token(&state, None).await;
        let token = jwt_for(&state, "existing-operator");
        let app = wizard_app_with_auth(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/wizard/start")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {}", token))
                    .body(Body::from("{}".as_bytes().to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn wizard_completion_discards_bootstrap_token() {
        // Exercise `persist_operator_and_clear_bootstrap` directly to avoid
        // touching `~/.cyberclaw/users.toml` on the developer machine. We
        // point `HOME` at a tempdir for the duration of the call.
        //
        // Sprint 6: `#[serial]` serializes this with admin tests which
        // also mutate `$HOME`; serial_test uses one process-wide lane so
        // any test using `#[serial]` across the crate will not race.
        let state = build_test_state();
        set_bootstrap_token(&state, Some("the-token".into())).await;

        let tmp = tempfile::tempdir().expect("tempdir");
        let original = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", tmp.path());
        }

        // Build a completed session with a display_name answer so the
        // generated OperatorRecord uses it.
        let skills_dir = tempfile::tempdir().unwrap();
        let plan = cyberclaw_control_plane::wizard_engine::build_onboard_plan(skills_dir.path());
        let mut session = WizardSession::new("wiz-done", &plan);
        session
            .answers
            .insert("display_name".into(), WizardAnswer::Text("Alice".into()));
        session.state = WizardState::Done;

        persist_operator_and_clear_bootstrap(&session, &state.wizard_bootstrap_token).await;

        // Restore HOME before any assertion to avoid leaking into siblings.
        unsafe {
            match original {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }

        // Token should now be cleared.
        let slot = state.wizard_bootstrap_token.read().await;
        assert!(slot.is_none(), "bootstrap token must be cleared on Done");

        // users.toml should now contain the persisted operator.
        let users_path = tmp.path().join(".cyberclaw").join("users.toml");
        assert!(users_path.exists(), "users.toml must be written on Done");
        let cfg = UsersConfig::load_from_path(&users_path).expect("load ok");
        assert_eq!(cfg.users.len(), 1);
        assert_eq!(cfg.users[0].display_name, "Alice");
    }
}
