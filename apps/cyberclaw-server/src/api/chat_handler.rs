//! # Chat Handler — S1 End-to-End Integration
//!
//! Integrates all S1 components into a unified chat completions handler:
//! - **S1-T1**: OrchestratorGateway — bridges agent-runtime to control-plane capabilities
//! - **S1-T2**: AgenticLoop — drives LLM -> tool-call -> observe cycles
//! - **S1-T3**: LoopDelegate — customizable decision hooks
//! - **S1-T4**: ProviderChain — retry, failover, circuit breaker for LLM calls
//! - **S1-T5**: SkillBinder — injects skill prompts and tools into the loop
//! - **S1-T6**: MemoryIntegration — loads/persists working memory context
//! - **S1-T7**: MiddlewarePipeline — policy, audit, tracing middleware
//!
//! The handler exposes `POST /v1/agent/chat/completions` which creates a
//! `DefaultAgenticLoop`, runs it to completion, and returns an OpenAI-compatible
//! `ChatCompletionResponse`.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::State,
    http::{header::HeaderName, HeaderMap, HeaderValue},
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::post,
    Extension, Json, Router,
};
use futures::stream::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use cyberclaw_agent_runtime::agentic_loop::{
    AgenticLoop, DefaultAgenticLoop, IterationResult, LoopConfig,
};
use cyberclaw_agent_runtime::builtin_tools::{BuiltinToolRegistry, ToolsetConfig};
use cyberclaw_agent_runtime::memory_integration::MemoryIntegration;
use cyberclaw_agent_runtime::tool_description::CapabilityFacade;
use cyberclaw_agent_runtime::{
    AgenticLoopGovernor, GovernorConfig, JsonStructureVerifier, LoopProfile, RegexAssertVerifier,
    VerifierChain,
};
use cyberclaw_core::execution::ExecutionMode;
use cyberclaw_core::gateway::{CapabilityRequest, OrchestratorGateway};
use cyberclaw_core::ids::{ExecutionId, SessionId};
use cyberclaw_core::memory::{InMemoryWorkingMemory, WorkingMemoryConfig};
use cyberclaw_llm::types::Message;
use cyberclaw_llm::types::{FunctionDefinition, ToolDefinition};

use crate::error::ApiError;
use crate::middleware::auth::Claims;
use crate::state::AppState;

// P0-1 fix: use the production ControlPlaneGateway from control-plane
// which integrates PolicyEngine (deny-by-default) instead of dispatch_auto()
use cyberclaw_control_plane::gateway_impl::ControlPlaneGateway as GoverningGateway;

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

/// Chat completion request for the agentic loop endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatRequest {
    /// Conversation messages.
    pub messages: Vec<ChatMessage>,
    /// LLM model name override. Defaults to "gpt-4" if absent.
    #[serde(default)]
    pub model: Option<String>,
    /// Whether to stream the response (reserved for future SSE support).
    #[serde(default)]
    pub stream: Option<bool>,
    /// Tool definitions to expose to the LLM (JSON Schema format).
    #[serde(default)]
    pub tools: Option<Vec<serde_json::Value>>,
    /// Optional agent identifier for agent-specific configuration.
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Skill IDs to bind into the agentic loop.
    #[serde(default)]
    pub skill_ids: Option<Vec<String>>,
    /// System prompt override.
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Maximum iterations for the agentic loop.
    #[serde(default)]
    pub max_iterations: Option<u32>,
    /// Sprint D3/D5 — execution-mode hint that decides which downstream
    /// runtime owns this request. `None` (back-compat with pre-D5
    /// clients) and `Some(Normal)` both fall through to the existing
    /// agentic-loop path. `Some(Persistent)` routes the request to
    /// `ExecutionService::execute()`'s D1 `ExecutionMode::Persistent`
    /// branch (commit 6447fca), which dispatches via the
    /// `PersistentLoop` wired on `AppState::execution_service`.
    /// `Some(Autopilot)` is preserved for symmetry; today it behaves
    /// the same as `Normal` on this endpoint (autopilot has its own
    /// dedicated handler, see `autopilot_handler`).
    ///
    /// `#[serde(default)]` keeps legacy request bodies that omit
    /// the field deserialising cleanly.
    #[serde(default)]
    pub execution_mode: Option<ExecutionMode>,
    /// Sprint D5 — test-only seed that bypasses `PersistentStoryPlanner` so
    /// e2e specs can inject a deterministic Story DAG without a live LLM.
    ///
    /// When present and non-null the `persistent_chat_dispatch` path skips
    /// the planner and constructs the `PersistentExecutionPlan` directly from
    /// this value.  The field is silently ignored for non-persistent requests.
    ///
    /// Expected shape (mirrors the TypeScript e2e helper):
    /// ```json
    /// { "stories": [{ "id": "...", "depends_on": [...], "acceptance": [...] }] }
    /// ```
    #[serde(default)]
    pub _persistent_test_seed: Option<serde_json::Value>,
}

/// A single chat message in the request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Role: "system", "user", "assistant", or "tool".
    pub role: String,
    /// Message content.
    pub content: String,
}

impl ChatMessage {
    /// Convert to the LLM SDK's `Message` type.
    // CONSTITUTION-BYPASS-OK: pure DTO-to-typed-message role converter. The
    // constitution is injected separately by the handler via `loop_config.
    // system_prompt = cyberclaw_constitution_text(...)` (see chat() fn,
    // ~line 412). This method just translates the role string and is not a
    // chat handler entry point.
    #[allow(dead_code)]
    fn to_llm_message(&self) -> Message {
        match self.role.as_str() {
            "system" => Message::system(&self.content),
            "assistant" => Message::assistant(&self.content),
            _ => Message::user(&self.content),
        }
    }
}

/// Chat completion response (OpenAI-compatible).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatResponse {
    /// Unique response identifier.
    pub id: String,
    /// Object type — always "chat.completion".
    pub object: String,
    /// Unix timestamp of creation.
    pub created: u64,
    /// Model used.
    pub model: String,
    /// Response choices.
    pub choices: Vec<AgentChoice>,
    /// Token usage statistics.
    pub usage: AgentUsage,
    /// Sprint D3 — Story DAG plan produced by PersistentStoryPlanner.
    ///
    /// Present only when `execution_mode = Persistent`. Omitted from JSON
    /// when `None` so the response stays OpenAI-compatible for non-persistent
    /// callers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<PersistentPlanSummary>,
}

/// Sprint D3 — summary of the Story DAG generated by [`PersistentStoryPlanner`].
///
/// Serialised into `AgentChatResponse.plan` so e2e tests and operator dashboards
/// can confirm real planning happened.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentPlanSummary {
    /// Goal text the planner received (mirrors `ExecutionPlan.goal`).
    pub goal: String,
    /// Ordered list of stories in the plan.
    pub stories: Vec<StorySummary>,
}

/// Sprint D3 — one-story summary for [`PersistentPlanSummary`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorySummary {
    /// Story identifier (e.g. "S1").
    pub id: String,
    /// Human-readable description.
    pub description: String,
    /// Capability bound to this story, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_id: Option<String>,
    /// Acceptance criteria for this story. Populated from the Story's criteria
    /// so e2e specs can inspect `met` flags and verify verifier results.
    /// Omitted when empty to keep the response compact for normal callers.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub acceptance: Vec<serde_json::Value>,
}

/// A single response choice.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChoice {
    /// Choice index.
    pub index: u32,
    /// The generated message.
    pub message: ChatMessage,
    /// Finish reason: "stop", "budget_exhausted", "stuck", etc.
    pub finish_reason: String,
}

/// Token usage statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentUsage {
    /// Prompt tokens consumed.
    pub prompt_tokens: u64,
    /// Completion tokens consumed.
    pub completion_tokens: u64,
    /// Total tokens consumed.
    pub total_tokens: u64,
    /// Number of agentic loop iterations.
    pub iterations: u32,
}

/// Stream event for future SSE support.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Incremental text content from the LLM.
    Delta {
        /// The content fragment.
        content: String,
    },
    /// A tool call has started.
    ToolCallStart {
        /// Tool call identifier.
        id: String,
        /// Tool/capability name.
        name: String,
    },
    /// A tool call has completed.
    ToolCallResult {
        /// Tool call identifier.
        id: String,
        /// Execution result payload.
        result: serde_json::Value,
    },
    /// The agentic loop has completed.
    Done,
    /// An error occurred.
    Error {
        /// Error message.
        message: String,
    },
}

/// 2026-05-19 — internal event the agentic-loop dispatch path emits when the
/// caller wants SSE streaming. Serialised into wire frames by
/// [`stream_frame_to_sse_event`].
///
/// Frame shapes the CLI's `parse_sse_data` (apps/cyberclaw-cli/src/commands/
/// chat.rs:294-332) accepts today:
///
/// * `Token` → `{"choices":[{"delta":{"content":"…"}}]}` (rendered)
/// * `ToolStart` → `{"type":"tool_start","tool":"…","args":{…}}` (Unknown→skip,
///   forward-compatible: a future client revision can render progress without
///   a server change)
/// * `ToolComplete` → `{"type":"tool_complete","tool":"…","ok":bool,
///   "preview":"…"}` (Unknown→skip, same forward-compatibility)
/// * `ErrorMsg` → `{"error":{"message":"…","type":"…"}}` (Unknown→skip on the
///   current CLI; surfaced for explicit observability in future revisions)
/// * `Done` → literal `[DONE]` sentinel (matches client `SseFrame::Done`)
#[derive(Debug, Clone)]
pub(crate) enum StreamFrame {
    /// A text fragment from the final assistant message. Each delta is
    /// re-emitted as an OpenAI-shaped `choices[].delta.content` chunk so
    /// chat-tui can keep its existing 4-frame parser unmodified.
    Token(String),
    /// A tool call dispatched through the gateway.
    ToolStart {
        tool: String,
        args: serde_json::Value,
    },
    /// A tool call returned (success or failure). `ok=false` carries the
    /// error message in `preview`; success carries a truncated payload
    /// preview (≤240 chars) so SSE never leaks full secrets via the
    /// streaming channel.
    ToolComplete {
        tool: String,
        ok: bool,
        preview: String,
        duration_ms: u64,
    },
    /// Terminal error. Loop aborted; the next frame after this is `Done`.
    ErrorMsg { message: String, kind: String },
    /// Rate-limit snapshot from the last LLM call. Emitted once just before
    /// `Done` when the provider returned `x-ratelimit-*` response headers.
    /// CLI clients that do not recognise this frame type skip it (Unknown).
    RateLimit {
        provider: String,
        requests_limit: Option<u64>,
        requests_remaining: Option<u64>,
        tokens_limit: Option<u64>,
        tokens_remaining: Option<u64>,
        requests_reset_secs: Option<f64>,
        tokens_reset_secs: Option<f64>,
    },
    /// Token usage snapshot emitted once just before `Done`.
    /// CLI clients use this to compute per-session cost estimates.
    /// Clients that do not recognise this frame type skip it (Unknown).
    Usage {
        /// LLM model name used for this session.
        model: String,
        /// Input tokens consumed (excludes cache tokens).
        input_tokens: u64,
        /// Output tokens generated.
        output_tokens: u64,
        /// Cache read tokens (Anthropic-style).
        cache_read_tokens: u64,
        /// Cache write tokens (Anthropic-style).
        cache_write_tokens: u64,
    },
    /// A capability dispatch entered the approval queue (governance ask/pending).
    /// Emitted once per (tool_name, reason) pair so the TUI can overlay a notice
    /// without waiting 60–90 s for the approval timeout.
    /// CLI clients that do not recognise this frame type skip it (Unknown).
    ApprovalPending {
        /// LLM-visible tool name that triggered the review.
        tool: String,
        /// Human-readable reason why approval is required (may be None when
        /// governance returns no message).
        reason: Option<String>,
    },
    /// Stream terminator. Maps to the literal `data: [DONE]\n\n` frame.
    Done,
}

/// Serialise a [`StreamFrame`] into an axum SSE [`SseEvent`].
///
/// The wire format mirrors the OpenAI-compatible token frame so the existing
/// CLI parser (`SseFrame::Token` branch) keeps working without modification.
/// `Done` becomes the literal `[DONE]` sentinel.
fn stream_frame_to_sse_event(frame: &StreamFrame) -> SseEvent {
    match frame {
        StreamFrame::Token(content) => {
            // OpenAI-shaped delta — `{"choices":[{"delta":{"content":"…"}}]}`.
            let payload = serde_json::json!({
                "choices": [{"delta": {"content": content}}],
            });
            SseEvent::default().data(payload.to_string())
        }
        StreamFrame::ToolStart { tool, args } => {
            let payload = serde_json::json!({
                "type": "tool_start",
                "tool": tool,
                "args": args,
            });
            SseEvent::default().data(payload.to_string())
        }
        StreamFrame::ToolComplete {
            tool,
            ok,
            preview,
            duration_ms,
        } => {
            let payload = serde_json::json!({
                "type": "tool_complete",
                "tool": tool,
                "ok": ok,
                "preview": preview,
                "duration_ms": duration_ms,
            });
            SseEvent::default().data(payload.to_string())
        }
        StreamFrame::ErrorMsg { message, kind } => {
            let payload = serde_json::json!({
                "error": {"message": message, "type": kind},
            });
            SseEvent::default().data(payload.to_string())
        }
        StreamFrame::RateLimit {
            provider,
            requests_limit,
            requests_remaining,
            tokens_limit,
            tokens_remaining,
            requests_reset_secs,
            tokens_reset_secs,
        } => {
            let payload = serde_json::json!({
                "type": "rate_limit",
                "rate_limit": {
                    "provider": provider,
                    "requests_limit": requests_limit,
                    "requests_remaining": requests_remaining,
                    "tokens_limit": tokens_limit,
                    "tokens_remaining": tokens_remaining,
                    "requests_reset_secs": requests_reset_secs,
                    "tokens_reset_secs": tokens_reset_secs,
                }
            });
            SseEvent::default().data(payload.to_string())
        }
        StreamFrame::Usage {
            model,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
        } => {
            let payload = serde_json::json!({
                "type": "usage",
                "usage": {
                    "model": model,
                    "input_tokens": input_tokens,
                    "output_tokens": output_tokens,
                    "cache_read_tokens": cache_read_tokens,
                    "cache_write_tokens": cache_write_tokens,
                }
            });
            SseEvent::default().data(payload.to_string())
        }
        StreamFrame::ApprovalPending { tool, reason } => {
            let payload = serde_json::json!({
                "type": "approval_pending",
                "approval_pending": {
                    "tool": tool,
                    "reason": reason,
                }
            });
            SseEvent::default().data(payload.to_string())
        }
        StreamFrame::Done => SseEvent::default().data("[DONE]"),
    }
}

// ---------------------------------------------------------------------------
// OrchestratorGateway bridge
// ---------------------------------------------------------------------------

// P0-1 FIX: The original ControlPlaneGateway here used dispatch_auto() which
// bypassed PolicyEngine entirely. Now we use gateway_impl::ControlPlaneGateway
// (aliased as GoverningGateway above) which integrates PolicyEngine with
// deny-by-default semantics. See gateway_impl.rs for the reference implementation.

/// Build the governing gateway that routes through PolicyEngine before dispatch.
pub fn build_governing_gateway(state: &Arc<AppState>) -> Arc<dyn OrchestratorGateway> {
    use cyberclaw_core::ids::WorkspaceId;
    use cyberclaw_core::workspace::{WorkspaceMode, WorkspaceRef};

    // BUG-CB-19 Fix 1: resolve workspace root as an absolute path so the
    // agent always knows where it can write. The old default "." is CWD-
    // relative and opaque to the LLM; agents defaulted to /tmp which the
    // connector boundary rejects.
    //
    // Priority:
    //   1. CYBERCLAW_AGENT_WORKSPACE_ROOT env var (explicit absolute override)
    //   2. CYBERCLAW_WORKSPACE_WRITABLE_ROOTS env var (comma-separated list,
    //      first entry used as root; historical Sprint 18 W3 mechanism)
    //   3. std::env::current_dir() — absolute CWD at server start
    //   4. "." — last-resort fallback (preserves pre-CB-19 behaviour if
    //      current_dir() fails, e.g. dir deleted under the process)
    let workspace_root_default: String = std::env::var("CYBERCLAW_AGENT_WORKSPACE_ROOT")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned()))
        .unwrap_or_else(|| ".".to_string());

    // Sprint 18 W3 — workspace writable roots are env-driven so staging
    // demos can exercise `/tmp` while production stays scoped. Comma-
    // separated absolute paths in `CYBERCLAW_WORKSPACE_WRITABLE_ROOTS`.
    let writable_roots: Vec<String> = std::env::var("CYBERCLAW_WORKSPACE_WRITABLE_ROOTS")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
        .unwrap_or_else(|| vec![workspace_root_default.clone()]);
    let default_workspace = WorkspaceRef {
        id: WorkspaceId::new(),
        mode: WorkspaceMode::Ephemeral,
        materialization_mode: None,
        home_node_id: None,
        backing_store: None,
        root: writable_roots
            .first()
            .cloned()
            .unwrap_or(workspace_root_default),
        writable_roots,
    };

    Arc::new(GoverningGateway::new(
        state.capability_dispatcher.clone(),
        state.connector_registry.clone(),
        Some(state.policy_engine.clone() as Arc<dyn cyberclaw_governance::engine::PolicyEngine>),
        default_workspace,
    ))
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// Create the agentic chat router.
///
/// Registers `POST /v1/agent/chat/completions` behind JWT auth.
pub fn create_agent_chat_router() -> Router<Arc<AppState>> {
    Router::new().route("/v1/agent/chat/completions", post(agent_chat_completions))
}

/// Agentic chat completions handler.
///
/// Creates a `DefaultAgenticLoop`, optionally binds skills and memory context,
/// runs the loop until completion, and returns an OpenAI-compatible response.
pub async fn agent_chat_completions(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<AgentChatRequest>,
) -> Result<Response, ApiError> {
    let request_id = Uuid::new_v4().to_string();
    // Default model precedence: explicit request → CYBERCLAW_DEFAULT_MODEL env
    // (cyberclaw-specific override) → LLM_DEFAULT_MODEL env (provider-side
    // default, e.g. MiniMax-M2.7-HighSpeed) → gpt-4 (last-resort fallback for
    // openai-compat env). Mirrors chat_conversations.rs default model logic
    // (commit 787a566) — dogfood 2026-05-12 found this entrypoint missed.
    let model = req
        .model
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("CYBERCLAW_DEFAULT_MODEL").ok())
        .or_else(|| std::env::var("LLM_DEFAULT_MODEL").ok())
        .unwrap_or_else(|| "gpt-4".to_string());

    // Sprint D3/D5 — resolve execution_mode for observability + routing.
    // Default-on-omit keeps legacy clients on the agentic-loop path.
    let resolved_execution_mode = req.execution_mode.unwrap_or_default();

    info!(
        request_id = %request_id,
        model = %model,
        messages = req.messages.len(),
        agent_id = ?req.agent_id,
        execution_mode = ?resolved_execution_mode,
        caller = %claims.sub,
        "Received agentic chat completion request"
    );

    // Sprint 21 — record an audit row for every agentic invocation
    // so the SHA-256 hash chain has tool-using agent activity, not just
    // governance events. Uses AuditKind::Mutation since the call may
    // mutate state via the bound tool palette (file_write, bash, etc.).
    // The detail field captures the request shape (model, message
    // count, skill_ids, agent_id) but NOT message content — that
    // could leak user data into the audit chain.
    if let Some(audit) = state.audit.as_ref() {
        audit
            .record(crate::audit::AuditEntry::now(
                claims.sub.as_str().to_string(),
                crate::audit::AuditKind::Mutation,
                "agent.invoke",
                Some(format!("request:{}", request_id)),
                serde_json::json!({
                    "model": model,
                    "messages": req.messages.len(),
                    "skill_ids": req.skill_ids,
                    "agent_id": req.agent_id,
                    "stream": req.stream.unwrap_or(false),
                    "execution_mode": format!("{:?}", resolved_execution_mode),
                }),
                crate::audit::AuditResult::Success,
            ))
            .await;
    }

    // Sprint D3/D5 — Persistent route. When the caller asks for the
    // PersistentLoop runtime, build a minimal `ExecutionRequest` whose
    // `execution_mode = Some(Persistent)` and hand it to
    // `state.execution_service`. The D1 dispatch branch in
    // `InMemoryExecutionService::execute()` (commit 6447fca) handles
    // routing into the wired `PersistentLoop`. The chat-handler keeps
    // running on the agentic-loop path for `Normal`/`Autopilot`/`None`.
    //
    // Today we don't synthesize a rich Story plan from the chat
    // messages — that's tracked as a follow-up. The route exists so
    // (a) the field reaches `ExecutionService` end-to-end, and
    // (b) operators can verify the `PersistentLoop not wired` failure
    // mode is loud rather than silent.
    if matches!(resolved_execution_mode, ExecutionMode::Persistent) {
        return persistent_chat_dispatch(state.clone(), &req, &model, &claims, &request_id).await;
    }

    // 2026-05-19 — SSE streaming branch. Previously rejected with HTTP 400;
    // now produces a real `text/event-stream` response in the 4-frame format
    // the cyberclaw CLI chat-tui parser expects (token / tool_start /
    // tool_complete / [DONE]). Non-streaming path below is unchanged and
    // remains bit-for-bit identical to the legacy behaviour.
    if req.stream.unwrap_or(false) {
        return agent_chat_completions_streaming(
            state.clone(),
            claims.clone(),
            req,
            request_id,
            model,
        )
        .await;
    }

    // --- Build OrchestratorGateway (P0-1 fix: routes through PolicyEngine) ---
    let gateway: Arc<dyn OrchestratorGateway> = build_governing_gateway(&state);

    // --- Build LoopConfig ---
    // Sprint 21 — universal-resilience auto-binding. The
    // sk_universal-resilience skill (ecosystem/skills/universal-
    // resilience/SKILL.md) defines tool-failure recovery reflexes
    // that should apply to EVERY agentic call, not just opt-in via
    // skill_ids. When the operator hasn't supplied a custom
    // system_prompt, prepend the resilience methodology so every
    // agent inherits the fallback reflex by default. The cost is
    // ~80 lines of system prompt; the benefit is consistent
    // graceful-failure behavior across every workflow.
    let resilience_body = {
        let hub = state.skill_hub.read().await;
        let path = hub
            .base_dir()
            .join("installed")
            .join("universal-resilience")
            .join("SKILL.md");
        std::fs::read_to_string(&path).ok()
    };
    // v1.0-rc4: unified constitutional prompt — single source of truth in
    // cyberclaw_agent_runtime::constitution. See that module for the 10-section
    // Anthropic XML Schema (Role/Why/Success/Constraints/Protocol/Tools/Output/
    // FailureModes/Examples/Checklist) and the 6 Iron Laws.
    let core_prompt = cyberclaw_agent_runtime::constitution::cyberclaw_constitution_text(
        cyberclaw_agent_runtime::constitution::ConstitutionProfile::SkillFirst,
    );

    let default_system_prompt = match resilience_body.as_ref() {
        Some(body) => format!(
            "{core_prompt}\n\n<default_skill name=\"universal-resilience\">\n{body}\n</default_skill>"
        ),
        None => core_prompt,
    };
    let system_prompt = req.system_prompt.clone().unwrap_or(default_system_prompt);
    let max_iterations = req.max_iterations.unwrap_or(90);

    // Sprint 18 W3 + F1 fix (2026-05-12) — build the tool palette so the
    // LLM sees a tools list and can emit `tool_calls` instead of just
    // narrating in plain text.
    //
    // Previously this only pulled `BuiltinToolRegistry::with_defaults()`,
    // which after F12 Phase C contains exactly 3 chat-intercepted facades
    // (skill_create / skill_search / delegate_to_sub_agent). The remaining
    // ~38 connector-owned facades (file_*, bash, web_*, browser_*,
    // mcp_call, memory_*, lsp.*, todo_*, verify_numeric, …) are seeded
    // into `state.deferred_tool_registry` at startup (state.rs:849) but
    // never reached the LLM tool palette, so the model had to guess
    // tool names instead of being shown real schemas.
    //
    // Fix: merge the 3 default builtins with every Active facade from
    // `deferred_tool_registry`, deduping by tool name (builtin wins on
    // clash since it owns the chat_handler intercept path).
    //
    // Boundary: Deferred tools are intentionally NOT exposed here — they
    // remain name-only / retrievable via ToolSearch by design (F7 token
    // budget). Governance Gate is unchanged — these tools still flow
    // through capability_dispatcher → PolicyEngine on execute.
    //
    // When the request explicitly opts out by passing `tools: Some(vec![])`
    // we honour that; otherwise expose the merged set.
    let tools: Vec<ToolDefinition> = if matches!(req.tools.as_deref(), Some([])) {
        Vec::new()
    } else {
        let builtin_facades: Vec<CapabilityFacade> =
            BuiltinToolRegistry::with_defaults().get_facades(&ToolsetConfig::default_config());
        let mut seen_names: std::collections::HashSet<String> =
            builtin_facades.iter().map(|f| f.name.clone()).collect();

        let mut merged: Vec<CapabilityFacade> = builtin_facades;
        {
            let registry = state.deferred_tool_registry.read().await;
            for facade in registry.active_facades() {
                if seen_names.insert(facade.name.clone()) {
                    merged.push(facade.clone());
                }
            }
        }

        info!(
            request_id = %request_id,
            tool_count = merged.len(),
            "Exposing merged tool palette to LLM (builtin + deferred-active)"
        );
        debug!(
            request_id = %request_id,
            tool_names = ?merged.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            "Tool palette names"
        );

        // P1.1 — prompt cache 优化。env var `CYBERCLAW_PROMPT_CACHE`
        // 默认开启；接受 "0" / "false" / "no" 关闭。当启用时，
        // 给最后一个 tool 打上 ephemeral cache_control 标记，缓存
        // 整个 tools schema 前缀（Anthropic 原生支持；OpenAI
        // 自动缓存 ≥1024 token，自然忽略本字段；MiniMax/Ark/
        // Generic 通过 skip_if_none 序列化时自然缺省）。
        let cache_enabled = std::env::var("CYBERCLAW_PROMPT_CACHE")
            .map(|v| {
                let lv = v.to_lowercase();
                lv != "0" && lv != "false" && lv != "no"
            })
            .unwrap_or(true);

        let mut tool_defs: Vec<ToolDefinition> = merged
            .iter()
            .map(|f: &CapabilityFacade| ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: f.name.clone(),
                    description: f.description.clone(),
                    parameters: f
                        .input_schema
                        .clone()
                        .unwrap_or_else(|| serde_json::json!({"type":"object","properties":{}})),
                },
                cache_control: None,
            })
            .collect();

        if cache_enabled {
            if let Some(last) = tool_defs.last_mut() {
                last.cache_control = Some(cyberclaw_llm::types::CacheControl::ephemeral());
            }
        }

        tool_defs
    };

    // P1.1 — prompt cache 开关：env `CYBERCLAW_PROMPT_CACHE`
    // 默认 true，接受 "0"/"false"/"no" 关闭。LoopConfig 把它
    // 传到 agentic_loop，让 init() 决定是否给 system 消息打上
    // ephemeral cache_control。
    let cache_system_enabled = std::env::var("CYBERCLAW_PROMPT_CACHE")
        .map(|v| {
            let lv = v.to_lowercase();
            lv != "0" && lv != "false" && lv != "no"
        })
        .unwrap_or(true);

    let mut loop_config = LoopConfig {
        system_prompt,
        model: model.clone(),
        budget: cyberclaw_agent_runtime::agentic_loop::IterationBudget {
            max_iterations,
            max_tokens: 0,
            timeout: Duration::from_secs(600),
        },
        stuck_threshold: 3,
        tools,
        cache_system_prompt: cache_system_enabled,
    };

    // --- MemoryIntegration (S1-T6) ---
    let session_id = SessionId::from_string(format!("chat-{}", request_id))
        .map_err(|e| ApiError::InternalError(format!("Failed to create session ID: {}", e)))?;

    let working_memory = Arc::new(InMemoryWorkingMemory::new(WorkingMemoryConfig {
        capacity: 100,
        ttl_seconds: None,
    }));

    let mut memory_integration =
        MemoryIntegration::with_defaults(working_memory.clone(), session_id.clone());

    // Load existing memory context and inject into system prompt.
    let memory_snapshot = memory_integration.load_context().map_err(|e| {
        warn!(request_id = %request_id, "Failed to load memory context: {}", e);
        ApiError::InternalError(format!("Memory context load failed: {}", e))
    })?;

    if !memory_snapshot.formatted_context.is_empty() {
        loop_config.system_prompt = format!(
            "{}\n\n{}",
            loop_config.system_prompt, memory_snapshot.formatted_context
        );
    }

    // --- SkillBinding (Sprint 44+1 — Agent runtime owns binding state) ---
    //
    // The chat handler used to short-circuit the binding flow by reading
    // `req.skill_ids` once and threading the resolved ecosystem list down
    // into dispatch as a side parameter. The brief moved that
    // responsibility onto the AgenticLoop itself: bindings are now state
    // on the loop instance, resolved from agent defaults +
    // ExecutionContext runtime overrides, and the dispatch path consults
    // the loop's `active_skill_bindings()` to derive ecosystems.
    //
    // Today the chat handler hasn't fully wired Agent registry lookup
    // (that remains a follow-up sprint, see "Report" notes), so the
    // agent default skill list is empty here. The runtime override path
    // — `req.skill_ids -> ExecutionContext.runtime_skill_bindings` —
    // continues to function, plus when the registry lands, the agent's
    // `default_skills` will also flow in via the same resolve call.
    //
    // For each successfully-bound skill, this block still reads SKILL.md
    // and appends its body to the system prompt (skills are methodology,
    // not execution — execution flows through Connector → Capability).
    let runtime_skill_overrides: Option<Vec<cyberclaw_core::ids::SkillId>> =
        req.skill_ids.as_ref().map(|raw_ids| {
            raw_ids
                .iter()
                .filter_map(|raw_id| {
                    cyberclaw_core::ids::SkillId::from_string(raw_id.clone())
                        .map_err(|e| {
                            warn!(
                                request_id = %request_id,
                                skill_id = %raw_id,
                                error = %e,
                                "Skill bind: invalid skill id, skipping"
                            );
                        })
                        .ok()
                })
                .collect()
        });
    let execution_context = match runtime_skill_overrides {
        Some(list) => {
            cyberclaw_core::execution::ExecutionContext::new().with_runtime_skill_bindings(list)
        }
        None => cyberclaw_core::execution::ExecutionContext::new(),
    };

    // Sprint 44+1 — agent defaults placeholder. When the agent registry
    // lookup lands, replace this with `agent_resolver.lookup(req.agent_id)
    // .default_skills`. For now an empty default keeps current behaviour
    // (req.skill_ids alone drives bindings).
    let agent_default_skills: Vec<cyberclaw_core::ids::SkillId> = Vec::new();

    // --- Create AgenticLoop (S1-T2) ---
    // P1.2/P1.3 — clone gateway so the verify-by-execution path in
    // `run_agentic_loop` can still dispatch `fs.stat` after the loop owns
    // its own Arc.
    // --- Select LoopProfile based on message heuristic ---
    // L3 when multi-turn (≥4 messages) OR any single message > 500 chars (long/complex).
    // L2 when any message > 100 chars (medium).
    // L1 otherwise (short single-turn).
    let profile = select_loop_profile(&req.messages);

    let governor = AgenticLoopGovernor::new(GovernorConfig::from_profile(profile));

    // CodeBlockVerifier omitted — requires exec runtime wiring (sprint 3c).
    // JsonStructureVerifier + RegexAssertVerifier are pure-local and safe to enable now.
    let verifier_chain = Arc::new(
        VerifierChain::new()
            .add(Box::new(JsonStructureVerifier::new()))
            .add(Box::new(RegexAssertVerifier::new())),
    );

    let mut agentic_loop = DefaultAgenticLoop::new(state.llm_client.clone(), gateway.clone())
        .with_governor(governor)
        .with_verifier_chain(verifier_chain);

    // Sprint 44+1 — install resolved bindings on the loop. Precedence:
    // ctx.runtime_skill_bindings (from req.skill_ids) overrides the
    // agent's default_skills. The dispatch path reads
    // `agentic_loop.active_skill_bindings()` (no longer threads a
    // separate `bound_ecosystems` parameter — that's derived from
    // SKILL.md frontmatter below).
    agentic_loop.resolve_skill_bindings(&agent_default_skills, Some(&execution_context));

    // Sprint 44 — read SKILL.md for each active binding and inject into
    // system prompt + collect SourceEcosystem so dispatch can translate
    // ecosystem-specific tool names. Source of truth: the loop's
    // active bindings. This guarantees the prompt-injection set and the
    // translator's ecosystem hint can never drift.
    let mut bound_ecosystems: Vec<cyberclaw_skill_runtime::compat::SourceEcosystem> = Vec::new();
    let resolved_bindings: Vec<cyberclaw_core::ids::SkillId> =
        agentic_loop.active_skill_bindings().to_vec();
    if !resolved_bindings.is_empty() {
        let hub = state.skill_hub.read().await;
        let installed_dir = hub.base_dir().join("installed");
        let mut bound = Vec::new();
        for sk in &resolved_bindings {
            let raw_id = sk.as_str();
            let name = raw_id.strip_prefix("sk_").unwrap_or(raw_id);
            let skill_md = installed_dir.join(name).join("SKILL.md");
            match std::fs::read_to_string(&skill_md) {
                Ok(body) => {
                    let frontmatter = parse_skill_frontmatter(&body);
                    let eco =
                        cyberclaw_skill_runtime::compat::detect_source_ecosystem(&frontmatter);
                    if !bound_ecosystems.contains(&eco) {
                        bound_ecosystems.push(eco);
                    }
                    loop_config.system_prompt = format!(
                        "{}\n\n## Skill: {name}\n\n{body}",
                        loop_config.system_prompt
                    );
                    bound.push(name.to_string());
                }
                Err(err) => {
                    warn!(
                        request_id = %request_id,
                        skill_id = %raw_id,
                        path = %skill_md.display(),
                        %err,
                        "Skill bind: SKILL.md not readable, skipping"
                    );
                }
            }
        }
        info!(
            request_id = %request_id,
            requested = resolved_bindings.len(),
            bound = bound.len(),
            names = ?bound,
            ecosystems = ?bound_ecosystems
                .iter()
                .map(|e| e.as_str())
                .collect::<Vec<_>>(),
            "Skill binding complete"
        );
    }

    // Sprint v1.x R1 — keyword-driven auto-bind of domain-expert skills.
    // Runs after explicit binding so caller-supplied `skill_ids` always win;
    // any non-conflicting matched expert skill is appended to the system
    // prompt under an `Auto-bound skill` heading. See
    // docs/architecture/skills/auto-bind.md for the matching semantics.
    {
        let already_bound: Vec<String> = agentic_loop
            .active_skill_bindings()
            .iter()
            .map(|s| {
                s.as_str()
                    .strip_prefix("sk_")
                    .unwrap_or(s.as_str())
                    .to_string()
            })
            .collect();
        let last_user_msg = req
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.as_str())
            .unwrap_or("");
        if !last_user_msg.is_empty() {
            let installed_dir = {
                let hub = state.skill_hub.read().await;
                hub.base_dir().join("installed")
            };
            let extras = auto_bind_extra_skills(&installed_dir, last_user_msg, &already_bound);
            if !extras.is_empty() {
                let extra_names: Vec<&str> = extras.iter().map(|(n, _)| n.as_str()).collect();
                info!(
                    request_id = %request_id,
                    auto_bound = ?extra_names,
                    "Auto-bind: domain expert skill(s) injected"
                );
                for (name, body) in extras {
                    loop_config.system_prompt = format!(
                        "{}\n\n## Auto-bound skill: {name}\n\n{body}",
                        loop_config.system_prompt
                    );
                }
            }
        }
    }

    // BUG-CB-19 Fix 2: inject workspace root into system prompt so the agent
    // knows the absolute path where file writes are permitted. Without this
    // hint, agents default to /tmp which the connector boundary rejects,
    // triggering unnecessary tool failures and system_hint injections
    // (CB-17) that cascade into wall-clock budget exhaustion (CB-18).
    {
        let workspace_root_hint: String = std::env::var("CYBERCLAW_AGENT_WORKSPACE_ROOT")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned()))
            .unwrap_or_else(|| ".".to_string());
        loop_config.system_prompt = format!(
            "{}\n\nYour workspace root is `{workspace_root_hint}`. \
             All file writes must use paths inside this directory; \
             absolute paths outside the workspace (e.g. `/tmp/...`) \
             will be rejected by the connector boundary.",
            loop_config.system_prompt
        );
    }

    // Initialize the loop with config.
    agentic_loop.init(loop_config).await.map_err(|e| {
        error!(request_id = %request_id, "Failed to initialize agentic loop: {}", e);
        ApiError::InternalError(format!("Loop initialization failed: {}", e))
    })?;

    // Inject user messages from the request.
    for msg in &req.messages {
        // BUG-R7-01: do NOT inject assistant/tool role messages here.
        // Prior tool_calls + tool_call_id metadata is lost in ChatMessage
        // serialization, so reconstructing them as `[assistant]: text` user
        // messages produces malformed conversation history → MiniMax 400 (2013).
        // The agentic loop's session state already carries authoritative
        // assistant + tool history; client-side re-injection is redundant
        // and actively breaks the structure.
        if msg.role == "user" {
            agentic_loop.add_user_message(&msg.content);
        }
    }

    // P0-1 fix: construct caller identity from JWT claims for PolicyEngine
    let caller_actor = cyberclaw_core::identity::ActorRef {
        id: cyberclaw_core::ids::ActorId::from_string(claims.sub.as_str().to_string())
            .unwrap_or_else(|_| {
                cyberclaw_core::ids::ActorId::from_string("unknown-caller".to_string())
                    .expect("fallback actor id")
            }),
        actor_type: cyberclaw_core::identity::ActorType::Human,
        tenant_id: claims.tenant.clone(),
        home_node_id: None,
        display_name: claims.sub.as_str().to_string(),
    };

    // --- Run the agentic loop ---
    let (final_text, finish_reason) = run_agentic_loop(
        &state,
        &mut agentic_loop,
        &mut memory_integration,
        &request_id,
        &caller_actor,
        state.tool_mapper.as_ref(),
        &bound_ecosystems,
        gateway.clone(),
        req.agent_id.as_deref(),
        None, // no streaming sink — non-streaming path
    )
    .await?;

    // --- Finalize ---
    let summary = agentic_loop.finalize().await.map_err(|e| {
        error!(request_id = %request_id, "Failed to finalize agentic loop: {}", e);
        ApiError::InternalError(format!("Loop finalization failed: {}", e))
    })?;

    // Flush memory integration.
    memory_integration.flush().map_err(|e| {
        warn!(request_id = %request_id, "Memory flush failed: {}", e);
        ApiError::InternalError(format!("Memory flush failed: {}", e))
    })?;

    info!(
        request_id = %request_id,
        iterations = summary.iterations,
        tokens_used = summary.tokens_used,
        finish_reason = %finish_reason,
        "Agentic loop completed"
    );

    // --- Build response ---
    // v1.2 P4 (2026-05-23): emit a fallback message when the loop produced
    // no user-visible text. Causes observed:
    //   - LLM content-filter returned empty content (model-side refusal)
    //   - DangerousCapabilityFilter blocked the only tool call; model
    //     produced thinking but emitted no text
    //   - Network timeout produced finish_reason=stop with no content
    // Returning "" to the client looks like a crash. The fallback below
    // gives the user actionable signal (finish_reason already carries
    // the machine-readable cause).
    let mut response_text =
        final_text.unwrap_or_else(|| summary.final_output.clone().unwrap_or_default());
    if response_text.trim().is_empty() {
        tracing::warn!(
            request_id = %request_id,
            finish_reason = %finish_reason,
            iterations = summary.iterations,
            tokens_used = summary.tokens_used,
            "Agentic loop completed with empty user-facing text — emitting fallback"
        );
        response_text = format!(
            "I was unable to produce a response for this request (finish_reason={}, \
             iterations={}, tokens={}). This typically means the request was refused at \
             the model layer (content policy) or the only available tool was blocked by \
             governance. Please rephrase the request or check the audit log for details.",
            finish_reason, summary.iterations, summary.tokens_used
        );
    }

    let now_ts = chrono::Utc::now().timestamp() as u64;
    let response = AgentChatResponse {
        id: format!("chatcmpl-{}", request_id),
        object: "chat.completion".to_string(),
        created: now_ts,
        model: model.clone(),
        choices: vec![AgentChoice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: response_text,
            },
            finish_reason: finish_reason.clone(),
        }],
        usage: AgentUsage {
            prompt_tokens: summary.tokens_used / 2,
            completion_tokens: summary.tokens_used / 2,
            total_tokens: summary.tokens_used,
            iterations: summary.iterations,
        },
        plan: None,
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("x-cyberclaw-request-id"),
        HeaderValue::from_str(&request_id)
            .map_err(|e| ApiError::InternalError(format!("Invalid header value: {}", e)))?,
    );
    headers.insert(
        HeaderName::from_static("x-cyberclaw-iterations"),
        HeaderValue::from_str(&summary.iterations.to_string())
            .map_err(|e| ApiError::InternalError(format!("Invalid header value: {}", e)))?,
    );

    // Sprint 21 — outcome audit row paired with the agent.invoke
    // entry recorded at function entry. Captures iteration count and
    // token usage so an auditor can later distinguish a 1-iteration
    // simple call from a 15-iteration deep agentic flow. Same
    // request_id ties the two rows together via SQL JOIN.
    if let Some(audit) = state.audit.as_ref() {
        audit
            .record(crate::audit::AuditEntry::now(
                claims.sub.as_str().to_string(),
                crate::audit::AuditKind::Mutation,
                "agent.invoke.complete",
                Some(format!("request:{}", request_id)),
                serde_json::json!({
                    "model": model,
                    "iterations": summary.iterations,
                    "tokens": summary.tokens_used,
                    "finish_reason": &response.choices.first().map(|c| c.finish_reason.clone()),
                }),
                crate::audit::AuditResult::Success,
            ))
            .await;
    }

    Ok((headers, Json(response)).into_response())
}

/// 2026-05-19 — SSE-streaming variant of [`agent_chat_completions`].
///
/// Mirrors the non-streaming handler's setup (gateway, system prompt with
/// universal-resilience + constitution, memory integration, skill bindings,
/// tool palette, agent default merge) so governance / audit / hallucination
/// detection / silent-abandon enforcement remain identical. The only
/// behavioural delta is the response shape: instead of a single JSON body,
/// we return `text/event-stream` and:
///
/// 1. Emit `tool_start` / `tool_complete` SSE frames as each capability
///    dispatches through [`run_agentic_loop`]'s sink hook (the loop logic
///    itself is unchanged — the sink is `Option<&Sender>`, `None` for the
///    legacy path).
/// 2. Once the loop terminates, emit the final assistant text in ~24-char
///    chunks as OpenAI-shaped delta frames, pacing 12 ms apart so the
///    client gets a progressive-render feel.
/// 3. Terminate with the literal `data: [DONE]\n\n` sentinel that
///    `cyberclaw-cli`'s `SseFrame::Done` matches on.
///
/// Errors mid-loop (LLM 5xx, governance reject, etc.) are surfaced as a
/// single `data: {"error":{...}}` frame followed by `[DONE]` — the SSE
/// connection is never silently closed without a terminator.
///
/// The agentic loop is driven on a separate `tokio::spawn` task so we can
/// hand the SSE response off to axum immediately (first-byte latency
/// independent of how long the loop takes to assemble its first chunk).
async fn agent_chat_completions_streaming(
    state: Arc<AppState>,
    claims: Claims,
    req: AgentChatRequest,
    request_id: String,
    model: String,
) -> Result<Response, ApiError> {
    // --- Build OrchestratorGateway (P0-1 fix: routes through PolicyEngine) ---
    let gateway: Arc<dyn OrchestratorGateway> = build_governing_gateway(&state);

    // --- Universal-resilience + constitution + system_prompt override ---
    // Mirrors agent_chat_completions: read SKILL.md, prepend the
    // SkillFirst constitution, append memory snapshot + skill bodies later.
    let resilience_body = {
        let hub = state.skill_hub.read().await;
        let path = hub
            .base_dir()
            .join("installed")
            .join("universal-resilience")
            .join("SKILL.md");
        std::fs::read_to_string(&path).ok()
    };
    let core_prompt = cyberclaw_agent_runtime::constitution::cyberclaw_constitution_text(
        cyberclaw_agent_runtime::constitution::ConstitutionProfile::SkillFirst,
    );
    let default_system_prompt = match resilience_body.as_ref() {
        Some(body) => format!(
            "{core_prompt}\n\n<default_skill name=\"universal-resilience\">\n{body}\n</default_skill>"
        ),
        None => core_prompt,
    };
    let system_prompt = req.system_prompt.clone().unwrap_or(default_system_prompt);
    let max_iterations = req.max_iterations.unwrap_or(90);

    // --- Tool palette (builtin + active deferred, dedup, with cache_control) ---
    let tools: Vec<ToolDefinition> = if matches!(req.tools.as_deref(), Some([])) {
        Vec::new()
    } else {
        let builtin_facades: Vec<CapabilityFacade> =
            BuiltinToolRegistry::with_defaults().get_facades(&ToolsetConfig::default_config());
        let mut seen_names: std::collections::HashSet<String> =
            builtin_facades.iter().map(|f| f.name.clone()).collect();
        let mut merged: Vec<CapabilityFacade> = builtin_facades;
        {
            let registry = state.deferred_tool_registry.read().await;
            for facade in registry.active_facades() {
                if seen_names.insert(facade.name.clone()) {
                    merged.push(facade.clone());
                }
            }
        }

        let cache_enabled = std::env::var("CYBERCLAW_PROMPT_CACHE")
            .map(|v| {
                let lv = v.to_lowercase();
                lv != "0" && lv != "false" && lv != "no"
            })
            .unwrap_or(true);
        let mut tool_defs: Vec<ToolDefinition> = merged
            .iter()
            .map(|f: &CapabilityFacade| ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: f.name.clone(),
                    description: f.description.clone(),
                    parameters: f
                        .input_schema
                        .clone()
                        .unwrap_or_else(|| serde_json::json!({"type":"object","properties":{}})),
                },
                cache_control: None,
            })
            .collect();
        if cache_enabled {
            if let Some(last) = tool_defs.last_mut() {
                last.cache_control = Some(cyberclaw_llm::types::CacheControl::ephemeral());
            }
        }
        tool_defs
    };

    let cache_system_enabled = std::env::var("CYBERCLAW_PROMPT_CACHE")
        .map(|v| {
            let lv = v.to_lowercase();
            lv != "0" && lv != "false" && lv != "no"
        })
        .unwrap_or(true);

    let mut loop_config = LoopConfig {
        system_prompt,
        model: model.clone(),
        budget: cyberclaw_agent_runtime::agentic_loop::IterationBudget {
            max_iterations,
            max_tokens: 0,
            timeout: Duration::from_secs(600),
        },
        stuck_threshold: 3,
        tools,
        cache_system_prompt: cache_system_enabled,
    };

    // --- MemoryIntegration ---
    let session_id = SessionId::from_string(format!("chat-{}", request_id))
        .map_err(|e| ApiError::InternalError(format!("Failed to create session ID: {}", e)))?;
    let working_memory = Arc::new(InMemoryWorkingMemory::new(WorkingMemoryConfig {
        capacity: 100,
        ttl_seconds: None,
    }));
    let mut memory_integration =
        MemoryIntegration::with_defaults(working_memory.clone(), session_id.clone());
    let memory_snapshot = memory_integration.load_context().map_err(|e| {
        warn!(request_id = %request_id, "Failed to load memory context: {}", e);
        ApiError::InternalError(format!("Memory context load failed: {}", e))
    })?;
    if !memory_snapshot.formatted_context.is_empty() {
        loop_config.system_prompt = format!(
            "{}\n\n{}",
            loop_config.system_prompt, memory_snapshot.formatted_context
        );
    }

    // --- SkillBinding ---
    let runtime_skill_overrides: Option<Vec<cyberclaw_core::ids::SkillId>> =
        req.skill_ids.as_ref().map(|raw_ids| {
            raw_ids
                .iter()
                .filter_map(|raw_id| {
                    cyberclaw_core::ids::SkillId::from_string(raw_id.clone())
                        .map_err(|e| {
                            warn!(
                                request_id = %request_id,
                                skill_id = %raw_id,
                                error = %e,
                                "Skill bind: invalid skill id, skipping"
                            );
                        })
                        .ok()
                })
                .collect()
        });
    let execution_context = match runtime_skill_overrides {
        Some(list) => {
            cyberclaw_core::execution::ExecutionContext::new().with_runtime_skill_bindings(list)
        }
        None => cyberclaw_core::execution::ExecutionContext::new(),
    };
    let agent_default_skills: Vec<cyberclaw_core::ids::SkillId> = Vec::new();

    // BUG-CB-01 (2026-05-23): select profile + capture wall-clock budget so the
    // spawned task can apply a hard outer timeout. Mirrors the non-streaming path.
    let profile = select_loop_profile(&req.messages);
    let wall_clock_secs = profile.default_wall_clock().as_secs();
    let governor = AgenticLoopGovernor::new(GovernorConfig::from_profile(profile));

    let mut agentic_loop = DefaultAgenticLoop::new(state.llm_client.clone(), gateway.clone())
        .with_governor(governor);
    agentic_loop.resolve_skill_bindings(&agent_default_skills, Some(&execution_context));

    let mut bound_ecosystems: Vec<cyberclaw_skill_runtime::compat::SourceEcosystem> = Vec::new();
    let resolved_bindings: Vec<cyberclaw_core::ids::SkillId> =
        agentic_loop.active_skill_bindings().to_vec();
    if !resolved_bindings.is_empty() {
        let hub = state.skill_hub.read().await;
        let installed_dir = hub.base_dir().join("installed");
        for sk in &resolved_bindings {
            let raw_id = sk.as_str();
            let name = raw_id.strip_prefix("sk_").unwrap_or(raw_id);
            let skill_md = installed_dir.join(name).join("SKILL.md");
            if let Ok(body) = std::fs::read_to_string(&skill_md) {
                let frontmatter = parse_skill_frontmatter(&body);
                let eco = cyberclaw_skill_runtime::compat::detect_source_ecosystem(&frontmatter);
                if !bound_ecosystems.contains(&eco) {
                    bound_ecosystems.push(eco);
                }
                loop_config.system_prompt = format!(
                    "{}\n\n## Skill: {name}\n\n{body}",
                    loop_config.system_prompt
                );
            }
        }
    }

    // Sprint v1.x R1 — same keyword-driven auto-bind hook as the non-streaming
    // path. Kept symmetric to avoid SSE responses missing domain expertise.
    {
        let already_bound: Vec<String> = agentic_loop
            .active_skill_bindings()
            .iter()
            .map(|s| {
                s.as_str()
                    .strip_prefix("sk_")
                    .unwrap_or(s.as_str())
                    .to_string()
            })
            .collect();
        let last_user_msg = req
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.as_str())
            .unwrap_or("");
        if !last_user_msg.is_empty() {
            let installed_dir = {
                let hub = state.skill_hub.read().await;
                hub.base_dir().join("installed")
            };
            let extras = auto_bind_extra_skills(&installed_dir, last_user_msg, &already_bound);
            if !extras.is_empty() {
                let extra_names: Vec<&str> = extras.iter().map(|(n, _)| n.as_str()).collect();
                info!(
                    request_id = %request_id,
                    auto_bound = ?extra_names,
                    "Auto-bind (streaming): domain expert skill(s) injected"
                );
                for (name, body) in extras {
                    loop_config.system_prompt = format!(
                        "{}\n\n## Auto-bound skill: {name}\n\n{body}",
                        loop_config.system_prompt
                    );
                }
            }
        }
    }

    // BUG-CB-19 Fix 2 (streaming path): same workspace hint injection as
    // the non-streaming handler so agents on both paths know the allowed root.
    {
        let workspace_root_hint: String = std::env::var("CYBERCLAW_AGENT_WORKSPACE_ROOT")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned()))
            .unwrap_or_else(|| ".".to_string());
        loop_config.system_prompt = format!(
            "{}\n\nYour workspace root is `{workspace_root_hint}`. \
             All file writes must use paths inside this directory; \
             absolute paths outside the workspace (e.g. `/tmp/...`) \
             will be rejected by the connector boundary.",
            loop_config.system_prompt
        );
    }

    agentic_loop.init(loop_config).await.map_err(|e| {
        error!(request_id = %request_id, "Failed to initialize agentic loop (streaming): {}", e);
        ApiError::InternalError(format!("Loop initialization failed: {}", e))
    })?;

    for msg in &req.messages {
        // BUG-R7-01: do NOT inject assistant/tool role messages here.
        // Prior tool_calls + tool_call_id metadata is lost in ChatMessage
        // serialization, so reconstructing them as `[assistant]: text` user
        // messages produces malformed conversation history → MiniMax 400 (2013).
        // The agentic loop's session state already carries authoritative
        // assistant + tool history; client-side re-injection is redundant
        // and actively breaks the structure.
        if msg.role == "user" {
            agentic_loop.add_user_message(&msg.content);
        }
    }

    let caller_actor = cyberclaw_core::identity::ActorRef {
        id: cyberclaw_core::ids::ActorId::from_string(claims.sub.as_str().to_string())
            .unwrap_or_else(|_| {
                cyberclaw_core::ids::ActorId::from_string("unknown-caller".to_string())
                    .expect("fallback actor id")
            }),
        actor_type: cyberclaw_core::identity::ActorType::Human,
        tenant_id: claims.tenant.clone(),
        home_node_id: None,
        display_name: claims.sub.as_str().to_string(),
    };

    // --- Spawn the agentic loop on a task; pipe events through mpsc ---
    let (tx, rx) = mpsc::unbounded_channel::<StreamFrame>();

    let state_for_task = state.clone();
    let claims_for_task = claims.clone();
    let request_id_for_task = request_id.clone();
    let model_for_task = model.clone();
    let tx_for_task = tx.clone();
    let bound_for_task = bound_ecosystems.clone();
    let agent_id_for_task = req.agent_id.clone();
    let gateway_for_task = gateway.clone();

    tokio::spawn(async move {
        // BUG-CB-01: wrap with a hard wall-clock timeout derived from the loop
        // profile (L1=60s / L2=180s / L3=240s). Without this the spawn can hang
        // indefinitely when the LLM stalls, a memory-save blocks, or a mutex is
        // contended — leaving the TUI spinner running forever.
        let wall_clock_timeout = Duration::from_secs(wall_clock_secs);
        let loop_future = run_agentic_loop(
            &state_for_task,
            &mut agentic_loop,
            &mut memory_integration,
            &request_id_for_task,
            &caller_actor,
            state_for_task.tool_mapper.as_ref(),
            &bound_for_task,
            gateway_for_task.clone(),
            agent_id_for_task.as_deref(),
            Some(&tx_for_task),
        );
        let result = match tokio::time::timeout(wall_clock_timeout, loop_future).await {
            Ok(inner) => inner,
            Err(_elapsed) => {
                warn!(
                    request_id = %request_id_for_task,
                    wall_clock_secs = wall_clock_secs,
                    "Agentic loop exceeded wall-clock budget — aborting streaming task"
                );
                let _ = tx_for_task.send(StreamFrame::ErrorMsg {
                    message: format!(
                        "agentic loop exceeded {}s wall-clock budget; \
                         the request may have been too complex for the selected profile. \
                         Try a shorter query or contact your administrator.",
                        wall_clock_secs
                    ),
                    kind: "timeout".to_string(),
                });
                let _ = tx_for_task.send(StreamFrame::Done);
                return;
            }
        };

        match result {
            Ok((final_text, finish_reason)) => {
                // Finalize the loop so summary + memory flush match the
                // non-streaming path (audit chain stays consistent).
                let summary = match agentic_loop.finalize().await {
                    Ok(s) => s,
                    Err(e) => {
                        error!(request_id = %request_id_for_task, "finalize failed: {}", e);
                        let _ = tx_for_task.send(StreamFrame::ErrorMsg {
                            message: format!("loop finalize failed: {e}"),
                            kind: "internal".to_string(),
                        });
                        let _ = tx_for_task.send(StreamFrame::Done);
                        return;
                    }
                };
                if let Err(e) = memory_integration.flush() {
                    warn!(request_id = %request_id_for_task, "memory flush failed: {}", e);
                }

                info!(
                    request_id = %request_id_for_task,
                    iterations = summary.iterations,
                    tokens_used = summary.tokens_used,
                    finish_reason = %finish_reason,
                    "Agentic loop completed (streaming)"
                );

                // Pair the agent.invoke.complete audit row that the
                // non-streaming path emits at the end of agent_chat_completions.
                if let Some(audit) = state_for_task.audit.as_ref() {
                    audit
                        .record(crate::audit::AuditEntry::now(
                            claims_for_task.sub.as_str().to_string(),
                            crate::audit::AuditKind::Mutation,
                            "agent.invoke.complete",
                            Some(format!("request:{}", request_id_for_task)),
                            serde_json::json!({
                                "model": model_for_task,
                                "iterations": summary.iterations,
                                "tokens": summary.tokens_used,
                                "finish_reason": &finish_reason,
                                "stream": true,
                            }),
                            crate::audit::AuditResult::Success,
                        ))
                        .await;
                }

                // Emit the final assistant body as token deltas so chat-tui
                // can render it. We chunk on character boundaries (~24 chars)
                // and pace 12 ms apart. Skipping the pace on chunk 0 keeps
                // first-token latency under control.
                let body =
                    final_text.unwrap_or_else(|| summary.final_output.clone().unwrap_or_default());
                if !body.is_empty() {
                    let mut buf = String::with_capacity(32);
                    let mut idx = 0usize;
                    for ch in body.chars() {
                        buf.push(ch);
                        if buf.chars().count() >= 24 {
                            if idx > 0 {
                                tokio::time::sleep(Duration::from_millis(12)).await;
                            }
                            let chunk = std::mem::take(&mut buf);
                            if tx_for_task.send(StreamFrame::Token(chunk)).is_err() {
                                // client gone
                                return;
                            }
                            idx += 1;
                        }
                    }
                    if !buf.is_empty() {
                        if idx > 0 {
                            tokio::time::sleep(Duration::from_millis(12)).await;
                        }
                        let _ = tx_for_task.send(StreamFrame::Token(buf));
                    }
                }

                // BUG-CB-02 (2026-05-23): mirror the non-streaming empty-response
                // fallback. When governance silently denies the only tool call (or
                // the model returns no text for any other reason) the streaming path
                // previously emitted zero Token frames and then [DONE], leaving the
                // TUI with a blank response and no signal. Emit one diagnostic Token
                // frame so the user sees actionable text rather than a spinner freeze.
                if body.is_empty() {
                    let fallback = format!(
                        "I was unable to produce a response for this request \
                         (finish_reason={}, iterations={}, tokens={}). This typically \
                         means the request was refused at the model layer (content \
                         policy) or the only available tool was blocked by governance. \
                         Please rephrase the request or check the audit log for details.",
                        finish_reason, summary.iterations, summary.tokens_used
                    );
                    tracing::warn!(
                        request_id = %request_id_for_task,
                        finish_reason = %finish_reason,
                        iterations = summary.iterations,
                        tokens_used = summary.tokens_used,
                        "Streaming loop completed with empty body — emitting fallback token"
                    );
                    let _ = tx_for_task.send(StreamFrame::Token(fallback));
                }

                // Emit rate-limit snapshot from the last LLM call (if any).
                if let Some(rl) = summary.last_rate_limit {
                    let _ = tx_for_task.send(StreamFrame::RateLimit {
                        provider: rl.provider,
                        requests_limit: rl.requests_limit,
                        requests_remaining: rl.requests_remaining,
                        tokens_limit: rl.tokens_limit,
                        tokens_remaining: rl.tokens_remaining,
                        requests_reset_secs: rl.requests_reset_secs,
                        tokens_reset_secs: rl.tokens_reset_secs,
                    });
                }

                // Emit token usage snapshot for client-side cost estimation.
                // LoopSummary.tokens_used is the aggregate across all iterations
                // (prompt + completion combined). We emit it as input_tokens
                // since we cannot split prompt vs completion at this level;
                // clients use this for rough cost estimation only.
                let _ = tx_for_task.send(StreamFrame::Usage {
                    model: model_for_task.clone(),
                    input_tokens: summary.tokens_used,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                });

                let _ = tx_for_task.send(StreamFrame::Done);
            }
            Err(api_err) => {
                error!(
                    request_id = %request_id_for_task,
                    "Agentic loop failed (streaming): {:?}",
                    api_err
                );
                let (message, kind) = match &api_err {
                    ApiError::InvalidRequest(m) => (m.clone(), "invalid_request".to_string()),
                    ApiError::LlmError(m) => (m.clone(), "llm_error".to_string()),
                    ApiError::InternalError(m) => (m.clone(), "internal".to_string()),
                    other => (other.to_string(), "error".to_string()),
                };
                let _ = tx_for_task.send(StreamFrame::ErrorMsg { message, kind });
                let _ = tx_for_task.send(StreamFrame::Done);
            }
        }
    });

    // --- Convert mpsc → SSE stream ---
    // `UnboundedReceiverStream` adapts the receiver into a Stream; each
    // `StreamFrame` is serialised by `stream_frame_to_sse_event`. We wrap
    // with `Sse::new` + `KeepAlive` so proxies / load balancers (15 s default
    // is conservative) don't reap the connection mid-loop.
    let sse_stream = UnboundedReceiverStream::new(rx).map(|frame: StreamFrame| {
        Ok::<SseEvent, std::convert::Infallible>(stream_frame_to_sse_event(&frame))
    });

    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("x-cyberclaw-request-id"),
        HeaderValue::from_str(&request_id)
            .map_err(|e| ApiError::InternalError(format!("Invalid header value: {}", e)))?,
    );
    headers.insert(
        HeaderName::from_static("x-cyberclaw-stream"),
        HeaderValue::from_static("sse"),
    );

    let sse = Sse::new(sse_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    );

    let mut response = sse.into_response();
    response.headers_mut().extend(headers);
    Ok(response)
}

/// Sprint D5 — construct a [`PersistentExecutionPlan`] from a
/// `_persistent_test_seed` JSON value so e2e specs can inject a
/// deterministic Story DAG without requiring a live LLM.
///
/// Expected seed shape:
/// ```json
/// { "stories": [{ "id": "...", "depends_on": [...], "acceptance": [{ "description": "..." }] }] }
/// ```
/// Unknown fields are silently ignored. Stories with missing `id` are skipped.
fn build_plan_from_seed(
    goal: &str,
    seed: &serde_json::Value,
) -> cyberclaw_control_plane::persistent_execution::PersistentExecutionPlan {
    use cyberclaw_control_plane::persistent_execution::{
        AcceptanceCriterion, CapabilitySource, ExecutionPlan, Story, VerifierKind,
    };
    use cyberclaw_core::ids::{CapabilityId, ConnectorId};

    let mut plan = ExecutionPlan::new(goal);

    if let Some(stories_arr) = seed.get("stories").and_then(|v| v.as_array()) {
        for s in stories_arr {
            let id = match s.get("id").and_then(|v| v.as_str()) {
                Some(id) if !id.is_empty() => id.to_string(),
                _ => continue,
            };
            let depends_on: Vec<String> = s
                .get("depends_on")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            // 2026-05-06 — pick up the per-criterion verifier so seeded
            // plans can exercise the real DefaultVerifierExecutor branches.
            // Without this every criterion was treated as no-verifier ⇒
            // auto-pass, masking real dispatch failures.
            let criteria: Vec<AcceptanceCriterion> = s
                .get("acceptance")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .map(|entry| {
                            let desc = entry
                                .get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("acceptance criterion")
                                .to_string();
                            let mut crit = AcceptanceCriterion::new(desc);
                            if let Some(verifier_val) = entry.get("verifier") {
                                if let Ok(kind) =
                                    serde_json::from_value::<VerifierKind>(verifier_val.clone())
                                {
                                    crit = crit.with_verifier(kind);
                                }
                            }
                            crit
                        })
                        .collect()
                })
                .unwrap_or_default();
            let mut story = Story::new(id.clone(), id.clone(), criteria);
            story.depends_on = depends_on;

            // 2026-05-06 — also pick up capability_id + capability_input
            // so seeded persistent runs actually dispatch to a connector
            // with real input (e.g. seed plan asking slides.render with
            // markdown body). Connector defaults to "local"; future
            // enhancement: let seed specify connector_id explicitly.
            if let Some(cap_str) = s.get("capability_id").and_then(|v| v.as_str()) {
                if let Ok(cap_id) = CapabilityId::from_string(cap_str.to_string()) {
                    if let Ok(connector) = ConnectorId::from_string("local".to_string()) {
                        story = story
                            .with_capability_id(cap_id)
                            .with_source(CapabilitySource::Native { connector });
                    }
                }
            }
            if let Some(input_val) = s.get("capability_input").cloned() {
                if !input_val.is_null() {
                    story = story.with_capability_input(input_val);
                }
            }

            plan.add_story(story);
        }
    }

    // Ensure at least one story so the plan is non-empty.
    if plan.stories.is_empty() {
        plan.add_story(Story::new(
            "seed_fallback",
            "seed had no valid stories",
            vec![],
        ));
    }

    plan
}

/// Sprint D3/D5 — dispatch a chat request whose
/// `execution_mode == Some(Persistent)` through
/// `state.execution_service`. The control-plane's D1 dispatch branch
/// (`InMemoryExecutionService::execute`) routes Persistent executions
/// to the wired `PersistentLoop`. When no `PersistentLoop` is wired
/// the underlying service fails loudly with `PersistentLoop not
/// wired`, surfaced here as a 500.
///
/// # F1+F2 fix (commit after audit 4fe0613)
///
/// Earlier the singleton `PersistentLoop` in `AppState` was constructed with
/// an empty `"chat-default-persistent"` plan + `NoopVerifierExecutor`, and
/// `ExecutionRequest.plan` (Autopilot shape) couldn't carry the Story DAG
/// shape used by `PersistentLoop`. The Story DAG plan was only surfaced in
/// the HTTP response — never executed. `acceptance.met` stayed false, no
/// capabilities dispatched.
///
/// Now: each persistent-mode chat builds a **per-request** `PersistentLoop`
/// with the planner-generated plan, `DefaultVerifierExecutor` (was Noop),
/// and `LiveCapabilityDispatchSink` wrapping the production
/// `CapabilityDispatcher`. `acceptance.met` flags are flipped from
/// `result.stories_completed`/`stories_failed`; `usage.iterations` reflects
/// the real story count.
///
/// Sprint D3: replaces the placeholder `ExecutionPlan::new("chat-default-persistent")`
/// with a planner-generated plan. On LLM failure the planner returns a single-story
/// placeholder plan internally, so this path always has a runnable plan.
///
/// Wraps the production `CapabilityDispatcher` so `PersistentLoop` can
/// invoke real connectors. F1 fix: previous singleton path used
/// `NoopCapabilityDispatchSink` which dropped every dispatch on the floor.
struct LiveCapabilityDispatchSink {
    dispatcher: Arc<cyberclaw_connectors::CapabilityDispatcher>,
    actor: cyberclaw_core::identity::ActorRef,
    workspace: cyberclaw_core::workspace::WorkspaceRef,
    execution_id: ExecutionId,
}

impl std::fmt::Debug for LiveCapabilityDispatchSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveCapabilityDispatchSink")
            .field("execution_id", &self.execution_id)
            .field("actor.id", &self.actor.id)
            .field("workspace.root", &self.workspace.root)
            .finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
impl cyberclaw_control_plane::persistent_execution::CapabilityDispatchSink
    for LiveCapabilityDispatchSink
{
    async fn dispatch(
        &self,
        connector_id: &cyberclaw_core::ids::ConnectorId,
        capability_id: &cyberclaw_core::ids::CapabilityId,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let request = cyberclaw_connectors::CapabilityExecutionRequest {
            execution_id: self.execution_id.clone(),
            trace_id: format!("persistent:{}", self.execution_id.as_str()),
            actor: self.actor.clone(),
            workspace: self.workspace.clone(),
            connector_id: connector_id.clone(),
            capability_id: capability_id.clone(),
            input,
        };
        match self.dispatcher.dispatch(request).await {
            Ok(result) => Ok(result.output),
            Err(e) => Err(format!("dispatcher failed: {}", e)),
        }
    }
}

async fn persistent_chat_dispatch(
    state: Arc<AppState>,
    req: &AgentChatRequest,
    model: &str,
    claims: &Claims,
    request_id: &str,
) -> Result<Response, ApiError> {
    use cyberclaw_control_plane::execution_service::{ExecutionRequest, ExecutionService as _};
    use cyberclaw_control_plane::types::ControlPlaneContext;
    use cyberclaw_core::enums::Priority;
    use cyberclaw_core::identity::{ActorRef, ActorType};
    use cyberclaw_core::ids::{ActorId, AgentId, TaskId};
    use cyberclaw_core::task::{Task, TaskInput, TaskKind, TriggerRef};

    info!(
        request_id = %request_id,
        "Routing chat request to PersistentLoop via ExecutionService"
    );

    let caller_actor = ActorRef {
        id: ActorId::from_string(claims.sub.as_str().to_string()).unwrap_or_else(|_| {
            ActorId::from_string("unknown-caller".to_string()).expect("fallback actor id")
        }),
        actor_type: ActorType::Human,
        tenant_id: claims.tenant.clone(),
        home_node_id: None,
        display_name: claims.sub.as_str().to_string(),
    };

    // Compose a Task summary from the chat messages so audit / journal
    // rows have something searchable. Keep within Task::validate's
    // 2000-char summary cap by truncating the joined body.
    let summary_body: String = req
        .messages
        .iter()
        .map(|m| format!("[{}] {}", m.role, m.content))
        .collect::<Vec<_>>()
        .join("\n");
    let summary_capped = if summary_body.len() > 1900 {
        format!("{}…", &summary_body[..1900])
    } else {
        summary_body
    };

    let task = Task {
        id: TaskId::new(),
        case_id: None,
        title: format!("chat-persistent:{}", request_id),
        summary: summary_capped,
        kind: TaskKind::Analysis,
        priority: Priority::Low,
        requested_by: caller_actor.clone(),
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "agent.chat".to_string(),
            source: format!("request:{}", request_id),
        },
        input: TaskInput::default(),
        desired_outputs: vec![],
        labels: vec!["chat".to_string(), "persistent".to_string()],
        preferred_agent_id: None,
    };
    task.validate().map_err(|e| {
        ApiError::InvalidRequest(format!("derived persistent task failed validation: {}", e))
    })?;

    // Sprint D3 — build a real Story DAG via PersistentStoryPlanner.
    // Extract the last user message as the goal; fall back to a generic label
    // when the conversation has no user turns.
    let goal_text = req
        .messages
        .iter()
        .rfind(|m| m.role == "user")
        .map(|m| m.content.clone())
        .unwrap_or_else(|| "execute persistent task".to_string());

    // Sprint D5 — if the request carries a `_persistent_test_seed` value,
    // bypass the LLM-backed PersistentStoryPlanner and build the plan
    // directly from the seed so e2e specs get deterministic story IDs
    // without requiring a live model call.
    let persistent_plan = if let Some(seed) = req._persistent_test_seed.as_ref() {
        build_plan_from_seed(&goal_text, seed)
    } else {
        state.persistent_story_planner.plan(&goal_text).await
    };
    info!(
        request_id = %request_id,
        goal = %goal_text,
        stories = persistent_plan.stories.len(),
        plan_goal = %persistent_plan.goal,
        seeded = req._persistent_test_seed.is_some(),
        "PersistentStoryPlanner produced plan"
    );

    // Build the response-level plan summary; we'll flip the `met` flags after
    // the PersistentLoop runs.
    let mut plan_summary = PersistentPlanSummary {
        goal: persistent_plan.goal.clone(),
        stories: persistent_plan
            .stories
            .iter()
            .map(|s| StorySummary {
                id: s.id.clone(),
                description: s.description.clone(),
                capability_id: s.capability_id.as_ref().map(|c| c.as_str().to_string()),
                acceptance: s
                    .criteria
                    .iter()
                    .map(|c| {
                        serde_json::json!({
                            "description": c.description,
                            "met": c.met,
                        })
                    })
                    .collect(),
            })
            .collect(),
    };

    let execution_id = ExecutionId::new();
    // Submit a placeholder ExecutionRequest so /api/v1/audit and
    // /api/v1/executions list this run (audit visibility). The real
    // dispatch happens on a per-request PersistentLoop below — F1+F2 fix.
    // We keep `plan: None` because ExecutionRequest.plan is the Autopilot
    // shape and cannot carry a Story DAG.
    let exec_request = ExecutionRequest {
        execution_id: execution_id.clone(),
        task,
        case: None,
        context: ControlPlaneContext {
            actor: caller_actor.clone(),
            session: None,
            workspace: None,
        },
        agent: req.agent_id.as_ref().and_then(|raw| {
            AgentId::from_string(raw.clone())
                .ok()
                .map(|id| cyberclaw_core::execution::AgentRef {
                    id,
                    role: "chat".to_string(),
                })
        }),
        trace_id: None,
        execution_mode: Some(ExecutionMode::Persistent),
        plan: None,
    };
    if let Err(e) = state.execution_service.submit(exec_request).await {
        warn!(request_id = %request_id, error = %e, "Persistent submit failed (audit visibility lost; continuing)");
    }

    // F1+F2 — Build a per-request PersistentLoop with:
    //   * the planner-generated Story DAG (was: empty default plan)
    //   * DefaultVerifierExecutor (was: NoopVerifierExecutor → always Fail)
    //   * LiveCapabilityDispatchSink wrapping the real CapabilityDispatcher
    //     (was: NoopCapabilityDispatchSink → no-op)
    //
    // This replaces `state.execution_service.execute(execution_id)` (which
    // would have run the singleton PersistentLoop's empty default plan).
    let live_sink: Arc<dyn cyberclaw_control_plane::persistent_execution::CapabilityDispatchSink> =
        Arc::new(LiveCapabilityDispatchSink {
            dispatcher: state.capability_dispatcher.clone(),
            actor: caller_actor.clone(),
            workspace: cyberclaw_core::workspace::WorkspaceRef {
                id: cyberclaw_core::ids::WorkspaceId::new(),
                mode: cyberclaw_core::workspace::WorkspaceMode::Ephemeral,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: ".".to_string(),
                writable_roots: vec![".".to_string(), "/tmp".to_string()],
            },
            execution_id: execution_id.clone(),
        });
    let verifier: Arc<dyn cyberclaw_control_plane::persistent_execution::VerifierExecutor> =
        Arc::new(cyberclaw_control_plane::verifier_impl::DefaultVerifierExecutor::new());
    let ploop = cyberclaw_control_plane::persistent_execution::PersistentLoop::new(
        persistent_plan.clone(),
        cyberclaw_control_plane::persistent_execution::LoopConfig::default(),
    )
    .with_capability_dispatcher(live_sink)
    .with_verifier_executor(verifier);

    let mut exec_ctx = cyberclaw_core::execution::ExecutionContext::default();
    let loop_outcome = ploop.execute(&persistent_plan, &mut exec_ctx).await;

    // Flip plan_summary `met` flags + attach evidence based on the loop result.
    let mut completed_stories: usize = 0;
    let mut failed_stories: usize = 0;
    let finish_reason = match &loop_outcome {
        Ok(result) => {
            completed_stories = result.stories_completed.len();
            failed_stories = result.stories_failed.len();
            let completed_set: std::collections::HashSet<&String> =
                result.stories_completed.iter().collect();
            for story in plan_summary.stories.iter_mut() {
                let met = completed_set.contains(&story.id);
                for crit in story.acceptance.iter_mut() {
                    if let Some(obj) = crit.as_object_mut() {
                        obj.insert("met".to_string(), serde_json::json!(met));
                        if let Some(evidence) = result.verification_evidence.get(&story.id) {
                            obj.insert("evidence".to_string(), serde_json::json!(evidence));
                        }
                    }
                }
            }
            info!(
                request_id = %request_id,
                completed = completed_stories,
                failed = failed_stories,
                "PersistentLoop executed Story DAG"
            );
            if failed_stories == 0 && completed_stories > 0 {
                "persistent_completed".to_string()
            } else if failed_stories > 0 && completed_stories > 0 {
                "persistent_partial".to_string()
            } else if failed_stories > 0 {
                "persistent_failed".to_string()
            } else {
                "persistent_pending".to_string()
            }
        }
        Err(e) => {
            warn!(request_id = %request_id, error = %e, "PersistentLoop execute returned error");
            format!("persistent_error: {e}")
        }
    };

    let now_ts = chrono::Utc::now().timestamp() as u64;
    let response = AgentChatResponse {
        id: format!("chatcmpl-{}", request_id),
        object: "chat.completion".to_string(),
        created: now_ts,
        model: model.to_string(),
        choices: vec![AgentChoice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: format!(
                    "[persistent dispatch — execution_id={}; status={}]",
                    execution_id.as_str(),
                    finish_reason
                ),
            },
            finish_reason,
        }],
        usage: AgentUsage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            // F1+F2 fix: report real story count instead of hardcoded 0,
            // so clients/tests can detect that PersistentLoop actually ran.
            iterations: (completed_stories + failed_stories) as u32,
        },
        plan: Some(plan_summary),
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("x-cyberclaw-request-id"),
        HeaderValue::from_str(request_id)
            .map_err(|e| ApiError::InternalError(format!("Invalid header value: {}", e)))?,
    );
    headers.insert(
        HeaderName::from_static("x-cyberclaw-execution-id"),
        HeaderValue::from_str(execution_id.as_str())
            .map_err(|e| ApiError::InternalError(format!("Invalid header value: {}", e)))?,
    );
    headers.insert(
        HeaderName::from_static("x-cyberclaw-execution-mode"),
        HeaderValue::from_static("persistent"),
    );

    Ok((headers, Json(response)).into_response())
}

/// Drive the agentic loop to completion.
///
/// Handles each `IterationResult` variant: tool calls are executed via the
/// gateway (already wired into the loop), text responses and continuation
/// are handled internally, and terminal conditions produce the final output.
///
/// `state` is threaded so tool results can be passed through
/// `state.memory_sanitizer` before being fed back to the LLM — this is
/// the entry point most exposed to prompt-injection (a malicious tool
/// output can carry "ignore previous instructions" payloads). Sanitizer
/// hits are recorded in the audit log under `kind=Security,
/// action=sanitizer.<category>`.
#[allow(clippy::too_many_arguments)] // 9 args required: async loop state cannot be bundled into a builder without introducing a separate struct for a single call-site
async fn run_agentic_loop(
    state: &Arc<AppState>,
    agentic_loop: &mut DefaultAgenticLoop,
    memory_integration: &mut MemoryIntegration,
    request_id: &str,
    caller_actor: &cyberclaw_core::identity::ActorRef,
    tool_mapper: &cyberclaw_llm_bridge::ToolCallMapper,
    // Sprint 44 — ecosystem bindings detected from req.skill_ids; empty when
    // no skill is active or when source detection found Native/Unknown only.
    // Used to translate ecosystem-specific LLM tool names (e.g.
    // `browser_navigate`, `Bash`) into cyberclaw facade names before
    // dispatch via the SkillCompatRegistry on AppState.
    bound_ecosystems: &[cyberclaw_skill_runtime::compat::SourceEcosystem],
    // P1.2/P1.3 — gateway threaded in so the verify-by-execution path can
    // dispatch `fs.stat` for claimed file paths without re-building a
    // governing gateway on each iteration.
    gateway: Arc<dyn OrchestratorGateway>,
    // D2 — agent_id injected into todo_read / todo_write capability input so
    // the TodoConnector knows which agent's list to read/write. LLM emits
    // todo_write({content:"..."}) without agent_id; we supply it here from
    // the ChatRequest.agent_id field.
    agent_id: Option<&str>,
    // 2026-05-19 — SSE streaming sink. When `Some`, each capability dispatch
    // emits `tool_start` / `tool_complete` frames so the streaming client can
    // surface progress. `None` keeps the legacy non-streaming behaviour
    // (bit-for-bit identical: no events emitted, no extra allocations).
    stream_sink: Option<&mpsc::UnboundedSender<StreamFrame>>,
) -> Result<(Option<String>, String), ApiError> {
    let mut final_text: Option<String> = None;
    #[allow(unused_assignments)]
    let mut finish_reason = String::new();
    // F3 — tracks iteration number for hallucination audit detail, and whether
    // the most recent completed iteration dispatched any tool calls. A Done or
    // TextResponse that arrives without a preceding ToolCalls iteration in this
    // session is a candidate hallucination.
    let mut iteration_count: u32 = 0;
    // Total tool-call dispatches across all iterations in this session.
    // When 0 at Done/TextResponse time, no capability was ever invoked.
    let mut total_tool_calls_dispatched: u32 = 0;

    // 2026-05-17 — Silent-abandon enforcement (IRON LAW 6 server-side).
    // When the previous iteration dispatched tool calls that ALL errored,
    // a model that immediately returns Done with a "give up" reply is
    // abandoning the user. We intercept the first such Done, inject a
    // system-role nudge, and force one additional iteration. Cap at one
    // forced retry per session to avoid pathological loops (StuckDetector
    // catches the opposite failure mode — repeated identical tool calls).
    let mut last_iter_all_tools_errored: bool = false;
    let mut forced_retry_used: bool = false;

    // BUG-CB-18: track the last LLM API error seen in this session so that
    // a BudgetExhausted outcome can surface the real root cause.
    //
    // Architecture note: when an LLM error propagates through `next_iteration`
    // as Err(...), the loop exits immediately via `return Err(...)` below —
    // that path does NOT reach BudgetExhausted. The scenario where both occur
    // is: the LLM client's internal RetryProvider absorbs the error and retries
    // until wall-clock is exhausted, at which point the governor fires
    // BudgetExhausted on the *next* `pre_iteration` check. To capture that
    // error we would need to thread it out of `DefaultAgenticLoop::next_iteration`
    // (a larger change). Instead we use a simpler heuristic: if the governor's
    // BudgetExhausted reason contains "wall-clock" or "timeout", we annotate
    // the user-facing message to suggest checking server logs for LLM errors.
    let last_llm_error: Option<String> = None;

    loop {
        iteration_count += 1;
        let iteration_result = agentic_loop.next_iteration().await;
        let result = match iteration_result {
            Ok(r) => r,
            Err(e) => {
                error!(request_id = %request_id, "Agentic loop iteration failed: {}", e);
                // Map upstream LLM provider errors more precisely so the HTTP
                // status reflects whose fault it is:
                //   · LLM 4xx (unknown model, expired key, bad request shape)
                //     → InvalidRequest (400) — caller passed something the
                //       provider rejected
                //   · LLM 5xx (provider outage, rate limit at the gateway)
                //     → LlmError (502) — upstream is down, caller can retry
                //   · Other loop failures → InternalError (500)
                // Without this routing, a client typo (e.g. wrong model name)
                // would surface as 500 and look like a server bug.
                let msg = e.to_string();
                return Err(if msg.contains("API error: 4") {
                    ApiError::InvalidRequest(msg)
                } else if msg.contains("API error: 5") || msg.contains("LLM call failed") {
                    ApiError::LlmError(msg)
                } else {
                    ApiError::InternalError(format!("Loop iteration failed: {}", e))
                });
            }
        };

        match result {
            IterationResult::Done(text) => {
                debug!(request_id = %request_id, "Loop finished with Done");

                // 2026-05-17 — Silent-abandon enforcement (IRON LAW 6 server-side).
                // If the immediately preceding iteration's tool calls ALL errored,
                // and the model now wants to finish without trying again,
                // it's abandoning the user. Inject a system nudge and force
                // one more iteration. Done is accepted on the second pass
                // regardless (avoids infinite loop; StuckDetector catches
                // the inverse "repeated identical calls" pattern).
                if last_iter_all_tools_errored && !forced_retry_used {
                    warn!(
                        request_id = %request_id,
                        iteration = iteration_count,
                        "Silent-abandon detected: previous iteration's tools all failed, \
                         model returned Done without retry. Forcing one more iteration."
                    );
                    agentic_loop.add_system_hint(
                        "ENFORCEMENT: Your last reply ended without retrying after every tool call in the previous iteration was rejected. Per IRON LAW 6 (universal-resilience reflex), you MUST either: (a) attempt one alternative tool path that respects governance (e.g. write under workspace root instead of /), OR (b) deliver the actual answer INLINE in your reply as a markdown code block (no file_write needed). Silent abandonment is not acceptable — the user is waiting for a deliverable, not an acknowledgement of failure. Try again now.",
                    );
                    forced_retry_used = true;
                    // Reset the flag so a second consecutive all-errored
                    // iteration does NOT chain another forced retry.
                    last_iter_all_tools_errored = false;
                    continue;
                }

                // F3 + P1.2/P1.3 — Hallucination detection: LLM claimed completion but
                // invoked 0 capabilities across the entire session. This catches PPT-style
                // hallucinations where the LLM narrates file creation without ever
                // dispatching a tool call. `handle_hallucination_check` returns the
                // (possibly mutated) message text and may return Err(ApiError) in
                // `block` mode if verification confirms the hallucination.
                let processed = handle_hallucination_check(
                    state,
                    &gateway,
                    caller_actor,
                    request_id,
                    iteration_count,
                    total_tool_calls_dispatched,
                    text,
                )
                .await?;
                final_text = Some(processed);
                finish_reason = "stop".to_string();
                break;
            }
            IterationResult::BudgetExhausted(reason) => {
                warn!(request_id = %request_id, reason = %reason, "Loop budget exhausted");
                // BT-30: surface the specific budget reason to operators /
                // end users instead of a generic "budget_exhausted" string.
                //
                // BUG-CB-18: when the wall-clock budget fires, the most
                // likely cause is the LLM provider's internal retry logic
                // consuming all available time (e.g. MiniMax 400 retried
                // until wall-clock deadline). We annotate the message so
                // operators know to check server logs for upstream LLM
                // errors alongside the budget reason.
                // `last_llm_error` is populated when next_iteration() itself
                // returns Err (the direct-propagation path); for the
                // RetryProvider-absorb-then-wall-clock path it stays None
                // and we fall back to the heuristic annotation below.
                finish_reason = format!("budget_exhausted: {reason}");
                let reason_lower = reason.to_lowercase();
                final_text = Some(if let Some(ref api_err) = last_llm_error {
                    // Direct LLM error was captured before BudgetExhausted.
                    format!("[error — {api_err}; budget exhausted: {reason}]")
                } else if reason_lower.contains("wall-clock")
                    || reason_lower.contains("timeout")
                    || reason_lower.contains("wall_clock")
                {
                    // Wall-clock exhaustion often means LLM retries consumed
                    // all time. Direct operators to server logs for the real
                    // upstream error.
                    format!(
                        "[budget exhausted — {reason}] \
                         (wall-clock deadline reached; check server logs for upstream LLM errors)"
                    )
                } else {
                    format!("[budget exhausted — {reason}]")
                });
                break;
            }
            IterationResult::Stuck(reason) => {
                warn!(request_id = %request_id, reason = %reason, "Loop stuck");
                finish_reason = "stuck".to_string();
                break;
            }
            IterationResult::ToolCalls(tool_calls) => {
                // F3 — accumulate total dispatches so Done/TextResponse can detect 0-invoke sessions.
                total_tool_calls_dispatched += tool_calls.len() as u32;
                // 2026-05-17 — count errors in this batch so we can detect
                // "all tools failed → next Done is abandonment" (see Done
                // branch above for the silent-abandon enforcement).
                let mut errors_in_batch: u32 = 0;
                debug!(
                    request_id = %request_id,
                    tool_count = tool_calls.len(),
                    "Executing tool calls via gateway"
                );

                // Execute each tool call through the OrchestratorGateway.
                // Sprint 18 W3 — route through `state.tool_mapper` so the
                // LLM-emitted tool name (e.g. `file_write`, `bash`) is
                // translated into the canonical capability + connector
                // pair (e.g. `fs.write` on `local`). Without this the
                // dispatcher reported "Connector local does not support
                // capability file_write" and tool calls never executed.
                for tool_call in &tool_calls {
                    // Sprint 21 — agent-callable skill_create intercept.
                    // The skill registry lives in `state.skill_hub` (server
                    // process) and the `cyberclaw-connectors` crate
                    // intentionally doesn't depend on it. So skill_create
                    // is handled inline here, before the connector
                    // dispatcher sees the call. The current request_id is
                    // auto-attached as `source_request_id` so the
                    // resulting skill is provenance-traced back to this
                    // exact agentic invocation (per the provenance trail
                    // commit 015cdc4).
                    // Sprint 21 path #3 — skill_search intercept. Same
                    // pattern as skill_create below: SkillIndex lives in
                    // server state, connectors crate doesn't depend on
                    // it, so handle inline here. RiskLevel::Low so it
                    // dispatches without review.
                    // F5 closure (2026-05-06) — agentic_loop integration of
                    // SubAgentOrchestrator. The LLM emits a
                    // `delegate_to_sub_agent` tool call when it decides a
                    // subtask deserves a fresh context window. We construct
                    // a per-request orchestrator (caller_identity is
                    // request-scoped) and route through spawn_child +
                    // run_child. The child's final text is fed back to the
                    // parent loop as the tool result. SubAgentOrchestrator
                    // enforces depth limit 3 + max 5 children + budget
                    // fraction 0.5, all per SpawnPolicy::default().
                    if tool_call.function.name == "delegate_to_sub_agent" {
                        use cyberclaw_agent_runtime::agentic_loop::IterationBudget;
                        use cyberclaw_agent_runtime::sub_agent::{
                            SpawnPolicy, SubAgentOrchestrator,
                        };
                        use cyberclaw_core::ids::AgentId;
                        let args: serde_json::Value =
                            serde_json::from_str(&tool_call.function.arguments).unwrap_or_else(
                                |_| serde_json::json!({"raw": tool_call.function.arguments}),
                            );
                        let task_desc = args
                            .get("task_description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let max_iters = args
                            .get("max_iterations")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(8) as u32;
                        let result_str = if task_desc.trim().is_empty() {
                            r#"{"error":"task_description required"}"#.to_string()
                        } else {
                            let gateway = build_governing_gateway(state);
                            let mut orch = SubAgentOrchestrator::new(
                                SpawnPolicy::default(),
                                state.llm_client.clone(),
                                gateway,
                                0,
                                caller_actor.clone(),
                            );
                            let parent_id = AgentId::new();
                            let budget = IterationBudget {
                                max_iterations: max_iters,
                                max_tokens: 32_000,
                                timeout: std::time::Duration::from_secs(120),
                            };
                            match orch.spawn_child(parent_id, task_desc.clone(), &budget) {
                                Ok(child_id) => match orch.run_child(&child_id).await {
                                    Ok(out) => serde_json::json!({
                                        "child_agent_id": child_id.as_str(),
                                        "output": out,
                                        "success": true,
                                    })
                                    .to_string(),
                                    Err(e) => serde_json::json!({
                                        "child_agent_id": child_id.as_str(),
                                        "error": format!("{}", e),
                                        "success": false,
                                    })
                                    .to_string(),
                                },
                                Err(e) => serde_json::json!({
                                    "error": format!("spawn_child failed: {}", e),
                                    "success": false,
                                })
                                .to_string(),
                            }
                        };
                        // D5 — audit agent.delegate so governance chain is
                        // not bypassed for sub-agent spawns.
                        if let Some(audit) = state.audit.as_ref() {
                            audit
                                .record(crate::audit::AuditEntry::now(
                                    caller_actor.id.as_str().to_string(),
                                    crate::audit::AuditKind::Mutation,
                                    "agent.delegate",
                                    Some(format!("request:{}", request_id)),
                                    serde_json::json!({
                                        "sub_agent_task_preview": task_desc.chars().take(120).collect::<String>(),
                                        "max_iterations": max_iters,
                                    }),
                                    crate::audit::AuditResult::Success,
                                ))
                                .await;
                        }
                        agentic_loop.add_tool_result(tool_call.id.clone(), result_str);
                        continue;
                    }

                    if tool_call.function.name == "skill_search" {
                        let args: serde_json::Value =
                            serde_json::from_str(&tool_call.function.arguments).unwrap_or_else(
                                |_| serde_json::json!({"raw": tool_call.function.arguments}),
                            );
                        let q = args
                            .get("q")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as u32;
                        let result_str = match state.skill_index.as_ref() {
                            Some(idx) => match idx.search(q.clone(), limit).await {
                                Ok(rows) => {
                                    serde_json::to_string(&serde_json::json!({"results": rows}))
                                        .unwrap_or_else(|_| "{\"results\":[]}".to_string())
                                }
                                Err(e) => format!("{{\"error\":\"{}\"}}", e),
                            },
                            None => "{\"error\":\"skill index not initialized\"}".to_string(),
                        };
                        // D5 — audit skill.search so read-access is visible
                        // in the governance chain (was previously invisible).
                        if let Some(audit) = state.audit.as_ref() {
                            audit
                                .record(crate::audit::AuditEntry::now(
                                    caller_actor.id.as_str().to_string(),
                                    crate::audit::AuditKind::Config,
                                    "skill.search",
                                    Some(format!("request:{}", request_id)),
                                    serde_json::json!({
                                        "query": q,
                                        "limit": limit,
                                    }),
                                    crate::audit::AuditResult::Success,
                                ))
                                .await;
                        }
                        agentic_loop.add_tool_result(tool_call.id.clone(), result_str);
                        continue;
                    }

                    if tool_call.function.name == "skill_create" {
                        let args: serde_json::Value =
                            serde_json::from_str(&tool_call.function.arguments).unwrap_or_else(
                                |_| serde_json::json!({"raw": tool_call.function.arguments}),
                            );
                        let skill_name = args
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let req = crate::api::skills::CreateSkillRequest {
                            name: skill_name.clone(),
                            description: args
                                .get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            methodology: args
                                .get("methodology")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            trigger_examples: args
                                .get("trigger_examples")
                                .and_then(|v| v.as_array())
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|x| x.as_str().map(String::from))
                                        .collect()
                                })
                                .unwrap_or_default(),
                            source_request_id: Some(format!("request:{}", request_id)),
                        };
                        let actor_id = caller_actor.id.as_str().to_string();
                        let result_str = match crate::api::skills::create_skill_core(
                            state, req, &actor_id,
                        )
                        .await
                        {
                            Ok(view) => serde_json::to_string(&serde_json::json!({
                                "skill_id": view.skill_id,
                                "name": view.name,
                                "category": view.category,
                                "installed_at": view.installed_at,
                                "source_request_id": format!("request:{}", request_id),
                            }))
                            .unwrap_or_else(|_| "ok".to_string()),
                            Err(e) => format!("{{\"error\":\"{}\"}}", e),
                        };
                        // D5 — audit skill.create so LLM-triggered skill
                        // creation is visible in the governance chain.
                        if let Some(audit) = state.audit.as_ref() {
                            audit
                                .record(crate::audit::AuditEntry::now(
                                    caller_actor.id.as_str().to_string(),
                                    crate::audit::AuditKind::Mutation,
                                    "skill.create",
                                    Some(format!("request:{}", request_id)),
                                    serde_json::json!({
                                        "skill_name": skill_name,
                                    }),
                                    crate::audit::AuditResult::Success,
                                ))
                                .await;
                        }
                        agentic_loop.add_tool_result(tool_call.id.clone(), result_str);
                        continue;
                    }

                    let execution_id = ExecutionId::new();

                    // Map the LLM-side tool name to the canonical
                    // (connector_id, capability_id, transformed_input)
                    // tuple. Fall back to the raw name when the mapper
                    // doesn't know the tool — keeps existing behaviour
                    // for direct-named tools.
                    //
                    // Sprint 44 — when the request bound any external skill
                    // ecosystem (hermes/anthropic/etc.), build a translator
                    // closure that consults `state.skill_compat` for those
                    // ecosystems first. Translation is best-effort: if no
                    // alias matches, the raw name is used (back-compat).
                    let compat_registry = state.skill_compat.clone();
                    let bound = bound_ecosystems.to_vec();
                    let translator = move |name: &str| -> Option<String> {
                        if bound.is_empty() {
                            return None;
                        }
                        compat_registry.translate_for(&bound, name)
                    };
                    let mapped = if bound_ecosystems.is_empty() {
                        tool_mapper
                            .map_tool_call(tool_call, request_id.to_string())
                            .ok()
                    } else {
                        tool_mapper
                            .map_tool_call_with_compat(
                                tool_call,
                                request_id.to_string(),
                                Some(&translator),
                            )
                            .ok()
                    };
                    let cap_request = if let Some(m) = mapped {
                        CapabilityRequest {
                            execution_id: execution_id.clone(),
                            requested_by: caller_actor.clone(),
                            capability_id: m.capability_id,
                            connector_id: m.connector_id,
                            input: m.input,
                            reason: format!("Agentic loop tool call: {}", tool_call.function.name),
                        }
                    } else {
                        CapabilityRequest {
                            execution_id: execution_id.clone(),
                            requested_by: caller_actor.clone(),
                            capability_id: cyberclaw_core::ids::CapabilityId::from_string(
                                tool_call.function.name.clone(),
                            )
                            .unwrap_or_else(|_| {
                                cyberclaw_core::ids::CapabilityId::from_string(
                                    "unknown".to_string(),
                                )
                                .expect("fallback capability id")
                            }),
                            connector_id: cyberclaw_core::ids::ConnectorId::from_string(
                                "local".to_string(),
                            )
                            .expect("local connector id"),
                            input: serde_json::from_str(&tool_call.function.arguments)
                                .unwrap_or_else(
                                    |_| serde_json::json!({"raw": tool_call.function.arguments}),
                                ),
                            reason: format!("Agentic loop tool call: {}", tool_call.function.name),
                        }
                    };

                    // D2 — inject agent_id into todo_read / todo_write input.
                    // The LLM emits todo_write({content:"..."}) without agent_id;
                    // the TodoConnector requires it to scope the list to the
                    // correct agent. We supply it from the ChatRequest.agent_id
                    // that was threaded into run_agentic_loop.
                    let cap_request =
                        if matches!(tool_call.function.name.as_str(), "todo_read" | "todo_write") {
                            if let Some(aid) = agent_id {
                                let mut patched = cap_request;
                                if let serde_json::Value::Object(ref mut map) = patched.input {
                                    map.entry("agent_id").or_insert_with(|| {
                                        serde_json::Value::String(aid.to_string())
                                    });
                                }
                                patched
                            } else {
                                cap_request
                            }
                        } else {
                            cap_request
                        };

                    // R-3 (2026-05-05) — audit each tool call before + after.
                    // Pre-dispatch row gives the operator a record even when
                    // the connector hangs / times out; post-dispatch row
                    // captures latency and failure reason.
                    let cap_id_str = cap_request.capability_id.as_str().to_string();
                    let conn_id_str = cap_request.connector_id.as_str().to_string();
                    let exec_id_str = execution_id.as_str().to_string();
                    let tool_name_str = tool_call.function.name.clone();
                    if let Some(audit) = state.audit.as_ref() {
                        audit
                            .record_capability_invoke(
                                caller_actor.id.as_str().to_string(),
                                &exec_id_str,
                                &cap_id_str,
                                &conn_id_str,
                                &tool_name_str,
                                None,
                            )
                            .await;
                    }
                    // 2026-05-19 — emit tool_start SSE frame (streaming only).
                    // Carries the LLM-visible tool name + the parsed argument
                    // object so the client can render "running file.read(path=…)".
                    // Falls through silently when the sink is closed (client
                    // disconnect) — the loop continues so audit / governance
                    // chains stay complete.
                    if let Some(sink) = stream_sink {
                        let args_val: serde_json::Value =
                            serde_json::from_str(&tool_call.function.arguments).unwrap_or_else(
                                |_| serde_json::json!({"raw": tool_call.function.arguments}),
                            );
                        let _ = sink.send(StreamFrame::ToolStart {
                            tool: tool_call.function.name.clone(),
                            args: args_val,
                        });
                    }

                    let dispatch_started = std::time::Instant::now();
                    let tool_result = agentic_loop.gateway().execute_capability(cap_request).await;
                    let dispatch_latency_ms = dispatch_started.elapsed().as_millis() as u64;

                    // 2026-05-19 — emit tool_complete SSE frame (streaming only).
                    // Includes a short result preview (errors verbatim, success
                    // truncated to 240 chars) so the client can surface "ok" /
                    // "failed: …" without us shipping potentially-secret full
                    // payloads through SSE.
                    if let Some(sink) = stream_sink {
                        let (ok, preview) = match &tool_result {
                            Ok(r) => {
                                let body = serde_json::to_string(&r.output)
                                    .unwrap_or_else(|_| r.output.to_string());
                                let trimmed: String = body.chars().take(240).collect();
                                (true, trimmed)
                            }
                            Err(e) => (false, e.to_string()),
                        };
                        let _ = sink.send(StreamFrame::ToolComplete {
                            tool: tool_call.function.name.clone(),
                            ok,
                            preview,
                            duration_ms: dispatch_latency_ms,
                        });
                    }
                    if let Some(audit) = state.audit.as_ref() {
                        let (output_bytes, error_reason) = match &tool_result {
                            Ok(r) => (
                                serde_json::to_string(&r.output)
                                    .map(|s| s.len())
                                    .unwrap_or(0),
                                None,
                            ),
                            Err(e) => (0usize, Some(e.to_string())),
                        };
                        audit
                            .record_capability_complete(
                                caller_actor.id.as_str().to_string(),
                                &exec_id_str,
                                &cap_id_str,
                                &conn_id_str,
                                &tool_name_str,
                                dispatch_latency_ms,
                                output_bytes,
                                error_reason.as_deref(),
                            )
                            .await;

                        // A-4 (2026-05-05) — Capability Gap Queue.
                        // When a tool call fails to dispatch, classify the
                        // failure (NotFound vs GovernanceDenied vs ExecutionError)
                        // and record/aggregate it for operator triage via
                        // /api/v1/admin/capability-requests. Aggregated rows
                        // dedupe by (tool_name, failure_reason, status='pending').
                        if let Some(reason) = error_reason.as_deref() {
                            use crate::audit::CapabilityRequestReason;
                            let lower = reason.to_ascii_lowercase();
                            let classified = if lower.contains("not found")
                                || lower.contains("unknown capability")
                                || lower.contains("not registered")
                            {
                                CapabilityRequestReason::NotFound
                            } else if lower.contains("denied")
                                || lower.contains("policy")
                                || lower.contains("forbidden")
                                || lower.contains("governance")
                            {
                                CapabilityRequestReason::GovernanceDenied
                            } else {
                                CapabilityRequestReason::ExecutionError
                            };
                            // BUG-CB-03 (2026-05-23): emit ApprovalPending SSE frame so
                            // the TUI can overlay a notice instead of showing "Thinking…"
                            // for 60–90 s while the approval timeout elapses.
                            // Only emit for GovernanceDenied (the "check /approvals" path);
                            // NotFound and ExecutionError are surfaced via ToolComplete.ok=false.
                            if matches!(classified, CapabilityRequestReason::GovernanceDenied) {
                                if let Some(sink) = stream_sink {
                                    let _ = sink.send(StreamFrame::ApprovalPending {
                                        tool: tool_name_str.clone(),
                                        reason: Some(reason.chars().take(200).collect()),
                                    });
                                }
                            }
                            audit
                                .record_capability_request(
                                    &tool_name_str,
                                    Some(&cap_id_str),
                                    Some(&conn_id_str),
                                    classified,
                                    Some(caller_actor.id.as_str()),
                                    Some(exec_id_str.as_str()),
                                )
                                .await;
                        }
                    }

                    let raw_result_content = match tool_result {
                        Ok(result) => serde_json::to_string(&result.output)
                            .unwrap_or_else(|_| result.output.to_string()),
                        Err(e) => {
                            warn!(
                                request_id = %request_id,
                                tool = %tool_call.function.name,
                                error = %e,
                                "Tool call failed"
                            );
                            errors_in_batch += 1;
                            // 2026-05-17: WebUI verification of IRON LAW 2a showed the
                            // model would emit one tool call, get rejected by governance
                            // (e.g. fs.list_dir(/) → "Path outside workspace boundary"),
                            // then silently abandon — leaving the user with no answer.
                            // Inline `guidance` field nudges the model to follow IRON
                            // LAW 6 (universal-resilience reflex) directly: try a
                            // different path OR deliver the answer inline in chat. For
                            // code/text deliverables, a markdown code block IS the
                            // delivery — no file_write needed.
                            serde_json::json!({
                                "error": e.to_string(),
                                "guidance": "This tool call was rejected. Per IRON LAW 6 (universal-resilience reflex), before reporting failure to the user: (1) consider whether the request can be answered INLINE in your next reply — for code/text deliverables, a ```language ... ``` markdown code block IS the delivery, no file_write needed; (2) OR try ONE different tool/path that respects governance (e.g. write under workspace root instead of /). Do not silently abandon the user's request after a single rejection."
                            }).to_string()
                        }
                    };

                    // Sanitize the tool output before feeding it back to
                    // the LLM. A malicious tool result (e.g. file_read on
                    // an attacker-controlled file) is the easiest path
                    // for a prompt-injection or credential-leak payload
                    // to reach the model. Redacted output replaces the
                    // raw value; warnings are recorded as Security audit
                    // rows so /api/v1/security/injection/hits surfaces
                    // them in real time.
                    let sanitized = state
                        .memory_sanitizer
                        .sanitize_and_redact(&tool_call.function.name, &raw_result_content);
                    if !sanitized.warnings.is_empty() {
                        if let Some(audit) = state.audit.as_ref() {
                            audit
                                .record_sanitizer_warnings(
                                    caller_actor.id.as_str().to_string(),
                                    &format!("agent.tool_output:{}", tool_call.function.name),
                                    Some(format!("execution:{}", execution_id.as_str())),
                                    &sanitized.warnings,
                                )
                                .await;
                        }
                    }

                    agentic_loop.add_tool_result(tool_call.id.clone(), sanitized.content);
                }

                // 2026-05-17 — Silent-abandon enforcement. Flag this iteration
                // as all-errored if every tool call returned an error. The
                // Done branch above intercepts the next iteration's Done and
                // forces one extra round if this flag is set.
                last_iter_all_tools_errored =
                    !tool_calls.is_empty() && (errors_in_batch as usize) == tool_calls.len();

                // Write iteration summary to memory (debounced).
                let _ = memory_integration.write_entry(
                    "tool_calls",
                    serde_json::json!({
                        "count": tool_calls.len(),
                        "tools": tool_calls.iter().map(|tc| &tc.function.name).collect::<Vec<_>>()
                    }),
                );
            }
            IterationResult::TextResponse(text) => {
                debug!(
                    request_id = %request_id,
                    text_len = text.len(),
                    "Received intermediate text response"
                );

                // 2026-05-17 — Silent-abandon enforcement on TextResponse too.
                // Some models (e.g. DeepSeek when finish_reason isn't "stop")
                // return abandonment-style text via TextResponse instead of
                // Done. Without this check, the loop would continue naturally
                // but the model has no signal that abandonment is unacceptable.
                // The injected system_hint makes the model see the enforcement
                // before its next iteration starts.
                if last_iter_all_tools_errored && !forced_retry_used {
                    warn!(
                        request_id = %request_id,
                        iteration = iteration_count,
                        "Silent-abandon detected (TextResponse path): previous iteration's tools \
                         all failed, model returned abandonment text. Injecting enforcement nudge."
                    );
                    agentic_loop.add_system_hint(
                        "ENFORCEMENT: Your last reply ended without retrying after every tool call in the previous iteration was rejected. Per IRON LAW 6 (universal-resilience reflex), you MUST either: (a) attempt one alternative tool path that respects governance, OR (b) deliver the actual answer INLINE in your reply as a markdown code block. Silent abandonment is not acceptable.",
                    );
                    forced_retry_used = true;
                    last_iter_all_tools_errored = false;
                    continue;
                }

                // F3 + P1.2/P1.3 — Hallucination detection on intermediate text responses too.
                let processed = handle_hallucination_check(
                    state,
                    &gateway,
                    caller_actor,
                    request_id,
                    iteration_count,
                    total_tool_calls_dispatched,
                    text,
                )
                .await?;
                final_text = Some(processed);
                // Continue the loop for more iterations.
            }
            IterationResult::Continue => {
                debug!(request_id = %request_id, "Loop continuing");
            }
        }
    }

    Ok((final_text, finish_reason))
}

/// Sprint 44 — extract YAML frontmatter from a SKILL.md body and parse to JSON.
///
/// Returns `Value::Null` when the body has no frontmatter or YAML parsing
/// fails — `detect_source_ecosystem` handles `Null` cleanly (returns
/// `Unknown`, which the translator treats as passthrough).
///
/// Frontmatter is the convention `---\n<yaml>\n---\n<body>` used by every
/// supported skill ecosystem. We deliberately don't pull in `serde_yaml` as
/// a new workspace dep: the function uses a tiny manual extractor + falls
/// back to JSON parsing of the raw block when YAML isn't available. Most
/// skill frontmatter is JSON-compatible (key: value pairs with simple
/// scalars), which is enough for ecosystem detection.
/// Sprint v1.x R1 — Auto-bind domain-expert skills based on user-prompt
/// keyword match. Returns `(skill_name, body)` pairs for skills whose
/// `manifest.yaml::spec.auto_bind` rule matches the prompt and that aren't
/// already in `already_bound`.
///
/// This is a *pure additive* layer on top of explicit `skill_ids` binding;
/// explicit bindings always win, auto-bound skills are appended afterwards.
/// On any I/O error the function logs at `warn` and continues — auto-bind
/// must never break the request path.
fn auto_bind_extra_skills(
    installed_dir: &std::path::Path,
    prompt: &str,
    already_bound: &[String],
) -> Vec<(String, String)> {
    let mut binder = cyberclaw_skill_runtime::SkillBinder::new();
    if let Err(err) = binder.load_from_dir(installed_dir) {
        warn!(
            path = %installed_dir.display(),
            ?err,
            "auto-bind: scanning installed dir failed; skipping auto-bind"
        );
        return Vec::new();
    }
    if binder.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(String, String)> = Vec::new();
    for rule in binder.match_prompt(prompt) {
        if already_bound.iter().any(|n| n == &rule.skill_name) {
            continue;
        }
        if hits.iter().any(|(n, _)| n == &rule.skill_name) {
            continue;
        }
        let skill_md_path = rule.skill_dir.join("SKILL.md");
        match std::fs::read_to_string(&skill_md_path) {
            Ok(body) => {
                debug!(
                    skill = %rule.skill_name,
                    priority = rule.priority,
                    "auto-bind: matched skill"
                );
                hits.push((rule.skill_name.clone(), body));
            }
            Err(err) => {
                warn!(
                    skill = %rule.skill_name,
                    path = %skill_md_path.display(),
                    ?err,
                    "auto-bind: SKILL.md unreadable; skipping"
                );
            }
        }
    }
    hits
}

fn parse_skill_frontmatter(body: &str) -> serde_json::Value {
    let trimmed = body.trim_start();
    if !trimmed.starts_with("---") {
        return serde_json::Value::Null;
    }
    let after_open = &trimmed[3..];
    // Skip optional newline immediately after `---`.
    let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);
    // Find the closing `---` on its own line (or end of string).
    let close = match after_open.find("\n---") {
        Some(pos) => pos,
        None => return serde_json::Value::Null,
    };
    let yaml_block = &after_open[..close];

    // Extremely small parser: handle `key: value` and nested
    // `key:\n  subkey: value` structure that's enough for
    // detect_source_ecosystem (it only needs `metadata.{hermes,
    // anthropic, openclaw, superpowers, cyberclaw.source}` and `name`).
    let mut root = serde_json::Map::new();
    let mut current_section: Option<String> = None;
    let mut section_obj = serde_json::Map::new();
    for line in yaml_block.lines() {
        let raw = line;
        if raw.trim().is_empty() || raw.trim_start().starts_with('#') {
            continue;
        }
        // Top-level key (no leading whitespace).
        if !raw.starts_with(' ') && !raw.starts_with('\t') {
            // Flush previous section if any.
            if let Some(name) = current_section.take() {
                root.insert(name, serde_json::Value::Object(section_obj.clone()));
                section_obj.clear();
            }
            if let Some((k, v)) = raw.split_once(':') {
                let key = k.trim().to_string();
                let value = v.trim();
                if value.is_empty() {
                    // Section header — collect indented children.
                    current_section = Some(key);
                } else {
                    let cleaned = strip_yaml_quotes(value);
                    root.insert(key, serde_json::Value::String(cleaned.to_string()));
                }
            }
        } else if current_section.is_some() {
            // Indented child line.
            if let Some((k, v)) = raw.trim_start().split_once(':') {
                let key = k.trim().to_string();
                let value = v.trim();
                let cleaned = strip_yaml_quotes(value);
                section_obj.insert(key, serde_json::Value::String(cleaned.to_string()));
            }
        }
    }
    if let Some(name) = current_section.take() {
        root.insert(name, serde_json::Value::Object(section_obj));
    }
    serde_json::Value::Object(root)
}

/// Strip surrounding YAML quotes (single or double) from a value.
fn strip_yaml_quotes(s: &str) -> &str {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
        || (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2)
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

// ---------------------------------------------------------------------------
// F3 — Hallucination detector
// ---------------------------------------------------------------------------

/// Returns a short excerpt from `text` if the message appears to claim task
/// completion without any capability invocation having occurred in the same
/// iteration. Returns `None` when no completion claim is detected (safe path).
///
/// Detection strategy: case-insensitive substring match against a fixed list
/// of "completion claim" phrases common in LLM outputs. No regex, no LLM
/// judge — purely a fast string scan so latency impact is negligible.
///
/// False-positive risk: normal assistant messages that happen to contain
/// these phrases (e.g. "I have not finished yet" does NOT contain any of the
/// keywords, so it won't trigger). The phrases are chosen to be specific to
/// confident completion claims, not neutral language. The detector is
/// intentionally conservative: it only fires when the message both contains a
/// completion claim AND references a path or file, reducing false positives on
/// purely conversational Done messages (e.g. "Task complete. Anything else?").
fn hallucination_claimed_completion(text: &str) -> Option<String> {
    let lower = text.to_lowercase();

    // Completion claim phrases — LLM asserts it finished.
    let completion_phrases = [
        "已经完成",
        "已生成",
        "已创建",
        "已经生成",
        "已经创建",
        "文件已",
        "i have completed",
        "i have finished",
        "i have created",
        "i have generated",
        "i've completed",
        "i've finished",
        "i've created",
        "i've generated",
        "task complete",
        "task is complete",
        "successfully created",
        "successfully generated",
        "successfully completed",
        "has been created",
        "has been generated",
        "has been completed",
        "finished generating",
        "finished creating",
    ];

    // File/path reference phrases — message mentions a concrete artifact.
    let path_phrases = [
        "./",
        "/out/",
        ".pptx",
        ".pdf",
        ".docx",
        ".xlsx",
        ".png",
        ".jpg",
        ".json",
        ".csv",
        ".zip",
        "output file",
        "output path",
        "generated file",
    ];

    let has_completion_claim = completion_phrases.iter().any(|p| lower.contains(p));
    let has_path_reference = path_phrases.iter().any(|p| lower.contains(p));

    if has_completion_claim && has_path_reference {
        // Return a 120-char excerpt for the audit detail (no full message to
        // keep audit rows compact; full message is in the LLM conversation).
        let excerpt: String = text.chars().take(120).collect();
        return Some(excerpt);
    }

    None
}

// ---------------------------------------------------------------------------
// P1.2 / P1.3 — Verify-by-execution + block mode
// ---------------------------------------------------------------------------

/// Hallucination detection enforcement mode.
///
/// Selected by the `CYBERCLAW_HALLUCINATION_MODE` environment variable. The
/// three modes give operators a graduated response without changing client
/// contracts:
/// * `warn` (default) — preserves the legacy F3 behaviour: emit an audit row
///   when the string-matching heuristic fires; pass the message through to the
///   caller unchanged. No `fs.stat` dispatch is attempted.
/// * `verify` — in addition to the audit row, extract file paths from the
///   message and dispatch `fs.stat` for each. When any path is missing, append
///   a `[WARNING: claimed file <path> but file_stat returned 404]` line to the
///   response so the user is informed but the request still succeeds.
/// * `block` — same verification as `verify`; on missing paths the handler
///   returns `ApiError::InvalidRequest(...)` (HTTP 400) instead of the LLM
///   response, so a hallucinated completion never reaches the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HallucinationMode {
    Warn,
    Verify,
    Block,
}

impl HallucinationMode {
    /// Resolve the mode from the `CYBERCLAW_HALLUCINATION_MODE` env var.
    ///
    /// Unknown / unset values fall back to `Warn` so deployments that haven't
    /// opted in keep their pre-upgrade behaviour. Matching is case-insensitive.
    fn from_env() -> Self {
        match std::env::var("CYBERCLAW_HALLUCINATION_MODE")
            .ok()
            .map(|v| v.to_lowercase())
            .as_deref()
        {
            Some("verify") => Self::Verify,
            Some("block") => Self::Block,
            _ => Self::Warn,
        }
    }
}

/// Extract concrete file paths from an LLM message.
///
/// Scans whitespace-delimited tokens and keeps those that look like file
/// references — absolute paths (`/tmp/...`, `/Users/...`), relative paths
/// (`./out/foo`, `../bar`), or bare names ending in a known artifact
/// extension (`.pptx`, `.pdf`, etc.). Trailing punctuation (commas, periods,
/// closing brackets) is stripped so `at ./out/foo.pptx.` yields
/// `./out/foo.pptx`.
///
/// Intentionally conservative: only emits paths the LLM has plausibly
/// claimed to create, so the downstream `fs.stat` verification stays cheap
/// (<100ms typical for 0-3 paths via the local connector).
fn extract_paths_from_text(text: &str) -> Vec<String> {
    // Known artifact extensions worth verifying. Mirrors `path_phrases` in
    // `hallucination_claimed_completion` so the detector and verifier agree.
    const ARTIFACT_EXTS: &[&str] = &[
        ".pptx", ".pdf", ".docx", ".xlsx", ".png", ".jpg", ".jpeg", ".json", ".csv", ".zip",
        ".txt", ".md", ".html", ".py", ".rs", ".ts", ".js", ".yml", ".yaml", ".toml",
    ];

    let mut paths: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Whitespace tokenization is sufficient for typical LLM prose; we also
    // split on common message punctuation so "at ./out/foo.pptx," doesn't get
    // stuck to "at" or the trailing comma.
    let tokens = text.split(|c: char| {
        c.is_whitespace() || matches!(c, ',' | ';' | '"' | '\'' | '`' | '(' | ')' | '[' | ']')
    });

    for raw in tokens {
        // Only trim trailing sentence punctuation. Leading dots are part of
        // relative paths (`./out/...`, `../foo`) so we must preserve them.
        let token = raw.trim_end_matches(['.', ',', ';', ':', '!', '?', '>']);
        if token.is_empty() {
            continue;
        }

        // Skip obvious non-paths: URLs, raw markdown, very long blobs.
        if token.starts_with("http://") || token.starts_with("https://") {
            continue;
        }
        if token.len() > 512 {
            continue;
        }

        let looks_like_abs = token.starts_with('/') && token.len() > 1;
        let looks_like_rel = token.starts_with("./") || token.starts_with("../");
        // Re-check ext on the trimmed token (preserves the dot that's part of
        // the extension while stripping a trailing sentence period).
        let lower = token.to_lowercase();
        let has_artifact_ext = ARTIFACT_EXTS.iter().any(|ext| lower.ends_with(ext));

        // Require either a directory marker (abs/rel prefix) or a known
        // artifact extension. Bare words like "report" or "data" never make
        // it through, which keeps false-positive verification dispatches off.
        if !(looks_like_abs || looks_like_rel || has_artifact_ext) {
            continue;
        }

        // Reject anything that obviously isn't a filesystem path (contains a
        // colon outside the leading drive prefix is rare on Unix and tends
        // to indicate URLs we stripped above, but be defensive).
        if token.contains("://") {
            continue;
        }

        let owned = token.to_string();
        if seen.insert(owned.clone()) {
            paths.push(owned);
        }
    }

    paths
}

/// Dispatch `fs.stat` for each path through the supplied gateway and return
/// the subset that does NOT exist (the verification "miss list").
///
/// Each dispatch goes through the governing gateway (PolicyEngine →
/// CapabilityDispatcher → LocalConnector::stat), so this never bypasses
/// governance. `fs.stat` is `RiskLevel::Low` + `read_only: true`, so the
/// engine admits it without review.
///
/// Returns an empty vec when `paths` is empty or every path either exists
/// or could not be dispatched (treat dispatch errors as "could not verify"
/// → no false-positive hallucination flag).
async fn verify_paths_with_file_stat(
    gateway: &Arc<dyn OrchestratorGateway>,
    actor: &cyberclaw_core::identity::ActorRef,
    paths: &[String],
) -> Vec<String> {
    use cyberclaw_core::ids::{CapabilityId, ConnectorId};

    if paths.is_empty() {
        return Vec::new();
    }

    let connector_id = match ConnectorId::from_string("local".to_string()) {
        Ok(id) => id,
        Err(_) => return Vec::new(),
    };
    let capability_id = match CapabilityId::from_string("fs.stat".to_string()) {
        Ok(id) => id,
        Err(_) => return Vec::new(),
    };

    let mut missing: Vec<String> = Vec::new();
    for path in paths {
        let request = CapabilityRequest {
            execution_id: ExecutionId::new(),
            requested_by: actor.clone(),
            capability_id: capability_id.clone(),
            connector_id: connector_id.clone(),
            input: serde_json::json!({ "path": path }),
            reason: "hallucination_verification: fs.stat probe".to_string(),
        };

        match gateway.execute_capability(request).await {
            Ok(result) => {
                // `fs.stat` returns { exists: bool, ... }. A `false` here is
                // the canonical "claimed file is missing" signal. We do NOT
                // treat connector errors as missing because that would flag
                // governance-denied paths as hallucinations.
                let exists = result
                    .output
                    .get("exists")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                if !exists {
                    missing.push(path.clone());
                }
            }
            Err(e) => {
                // Logging only — verification dispatch failed (denied,
                // connector down, path outside workspace, etc.). The caller
                // already has the audit row from phase 1; don't escalate.
                debug!(path = %path, err = %e, "fs.stat verification probe failed");
            }
        }
    }

    missing
}

/// Run the F3 hallucination detector and apply the configured enforcement mode.
///
/// Three phases:
/// 1. **Phase 1 — string match** (unchanged from legacy F3): if the message
///    contains both a completion-claim phrase and a path-reference phrase,
///    emit the `agent.hallucination_warning` audit row.
/// 2. **Phase 2 — verify-by-execution** (P1.2): when mode is `verify` or
///    `block`, extract the concrete paths and dispatch `fs.stat` for each.
///    Missing paths are appended to the audit row's `verified_missing_paths`
///    field.
/// 3. **Phase 3 — enforcement** (P1.3): `warn` returns the original text;
///    `verify` returns the original text with a `[WARNING: ...]` suffix when
///    any path is missing; `block` returns `Err(ApiError::InvalidRequest)`.
async fn handle_hallucination_check(
    state: &Arc<AppState>,
    gateway: &Arc<dyn OrchestratorGateway>,
    caller_actor: &cyberclaw_core::identity::ActorRef,
    request_id: &str,
    iteration_count: u32,
    total_tool_calls_dispatched: u32,
    text: String,
) -> Result<String, ApiError> {
    // Only check when no tool was invoked across the whole session — same
    // gating condition as the legacy F3 code path.
    if total_tool_calls_dispatched != 0 {
        return Ok(text);
    }

    let excerpt = match hallucination_claimed_completion(&text) {
        Some(e) => e,
        None => return Ok(text),
    };

    let mode = HallucinationMode::from_env();

    // Phase 1 — always emit the legacy audit row + warn log.
    warn!(
        request_id = %request_id,
        iteration = iteration_count,
        excerpt = %excerpt,
        mode = ?mode,
        "agent.hallucination_warning: LLM claimed completion with 0 tool calls"
    );

    // Phase 2 — verify file paths when the mode demands it.
    let (paths, missing) = match mode {
        HallucinationMode::Warn => (Vec::new(), Vec::new()),
        HallucinationMode::Verify | HallucinationMode::Block => {
            let paths = extract_paths_from_text(&text);
            let missing = verify_paths_with_file_stat(gateway, caller_actor, &paths).await;
            (paths, missing)
        }
    };

    // Emit a single audit row with the verification result (so the legacy
    // F3 audit chain still contains exactly one row per detected
    // hallucination, but now carries fs.stat evidence in verify/block modes).
    if let Some(audit) = state.audit.as_ref() {
        audit
            .record(crate::audit::AuditEntry::now(
                caller_actor.id.as_str().to_string(),
                crate::audit::AuditKind::Mutation,
                "agent.hallucination_warning",
                Some(format!("request:{}", request_id)),
                serde_json::json!({
                    "iteration": iteration_count,
                    "message_excerpt": excerpt,
                    "reason": "LLM claimed completion with 0 capability invocations",
                    "mode": format!("{:?}", mode).to_lowercase(),
                    "extracted_paths": paths,
                    "verified_missing_paths": missing,
                }),
                crate::audit::AuditResult::Success,
            ))
            .await;
    }

    // Phase 3 — enforce.
    match mode {
        HallucinationMode::Warn => Ok(text),
        HallucinationMode::Verify => {
            if missing.is_empty() {
                Ok(text)
            } else {
                // Append a single warning suffix listing the verified-missing
                // paths. Keep the original LLM body so the user can still see
                // what the agent claimed (transparency > hiding).
                let warning = format!(
                    "\n\n[WARNING: claimed file{} {} but file_stat returned 404]",
                    if missing.len() == 1 { "" } else { "s" },
                    missing.join(", ")
                );
                Ok(format!("{text}{warning}"))
            }
        }
        HallucinationMode::Block => {
            if missing.is_empty() {
                Ok(text)
            } else {
                // Fail loud: HTTP 400 with a precise reason. The audit row is
                // already recorded above, so operators can grep
                // `agent.hallucination_warning` rows to find blocked
                // requests.
                Err(ApiError::InvalidRequest(format!(
                    "hallucinated completion: agent claimed file{} {} but file_stat returned 404",
                    if missing.len() == 1 { "" } else { "s" },
                    missing.join(", ")
                )))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Select a [`LoopProfile`] from the request message list.
///
/// Heuristic:
/// - L3: conversation contains an assistant reply OR ≥4 messages OR any message > 500 chars
///   (multi-turn / long-context / tool-using agents in 2nd+ turn)
/// - L2: any message > 100 chars (medium single-turn)
/// - L1: everything else (short single-turn)
///
/// The `has_assistant` check is the key fix for BUG-CB-16: tool-using tasks accumulate
/// system prompt (~10.5k) + tools schema (~2k) + per-tool-call results (500-2000 tokens
/// each), easily exceeding the 32k L1/L2 budget after just 2-3 tool calls. Any
/// conversation that already has an assistant reply is in the 2nd+ turn and must use L3.
pub(crate) fn select_loop_profile(messages: &[ChatMessage]) -> LoopProfile {
    let has_assistant = messages.iter().any(|m| m.role == "assistant");

    // Heuristic: any prompt containing file paths, code-execution keywords,
    // or tool-invocation language likely needs an agentic loop with tools.
    // These need L3 (128k budget) from turn 1 — L1 (32k) gets eaten by
    // system prompt (~10.5k) + tools schema (~2k) + first tool result.
    let likely_agentic = messages.iter().any(|m| {
        let c = m.content.to_lowercase();
        c.contains('/')               // file path
        || c.contains("create")       // create file / project
        || c.contains("write")        // write file / code
        || c.contains("read")         // read file
        || c.contains("run")          // run command / script
        || c.contains("execute")
        || c.contains("install")      // pip install / npm install
        || c.contains("generate")     // generate code / pptx / report
        || c.contains("build")
        || c.contains(".pptx") || c.contains(".docx") || c.contains(".xlsx")
        || c.contains(".py") || c.contains(".rs") || c.contains(".ts")
        || c.contains(".js") || c.contains(".md") || c.contains(".txt")
        || c.contains("file") || c.contains("文件")
        || c.contains("脚本") || c.contains("代码")
        || c.contains("ppt") || c.contains("pdf")
    });

    if has_assistant || likely_agentic || messages.len() >= 4 || messages.iter().any(|m| m.content.len() > 500) {
        LoopProfile::L3
    } else if messages.iter().any(|m| m.content.len() > 100) {
        LoopProfile::L2
    } else {
        LoopProfile::L1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_chat_request_deserialization() {
        let json = serde_json::json!({
            "messages": [
                {"role": "user", "content": "Hello, world!"}
            ],
            "model": "gpt-4",
            "stream": false,
            "agent_id": "test-agent"
        });

        let req: AgentChatRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, "user");
        assert_eq!(req.messages[0].content, "Hello, world!");
        assert_eq!(req.model, Some("gpt-4".to_string()));
        assert_eq!(req.stream, Some(false));
        assert_eq!(req.agent_id, Some("test-agent".to_string()));
    }

    #[test]
    fn test_agent_chat_request_minimal() {
        let json = serde_json::json!({
            "messages": [
                {"role": "user", "content": "Hi"}
            ]
        });

        let req: AgentChatRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.messages.len(), 1);
        assert!(req.model.is_none());
        assert!(req.stream.is_none());
        assert!(req.tools.is_none());
        assert!(req.agent_id.is_none());
        assert!(req.skill_ids.is_none());
        assert!(req.system_prompt.is_none());
        assert!(req.max_iterations.is_none());
    }

    #[test]
    fn test_agent_chat_response_serialization() {
        let resp = AgentChatResponse {
            id: "chatcmpl-123".to_string(),
            object: "chat.completion".to_string(),
            created: 1700000000,
            model: "gpt-4".to_string(),
            choices: vec![AgentChoice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: "Hello!".to_string(),
                },
                finish_reason: "stop".to_string(),
            }],
            usage: AgentUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
                iterations: 1,
            },
            plan: None,
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["id"], "chatcmpl-123");
        assert_eq!(json["object"], "chat.completion");
        assert_eq!(json["choices"][0]["message"]["content"], "Hello!");
        assert_eq!(json["choices"][0]["finish_reason"], "stop");
        assert_eq!(json["usage"]["total_tokens"], 15);
        assert_eq!(json["usage"]["iterations"], 1);
    }

    #[test]
    fn test_chat_message_to_llm_message() {
        let user_msg = ChatMessage {
            role: "user".to_string(),
            content: "test".to_string(),
        };
        let llm_msg = user_msg.to_llm_message();
        assert_eq!(llm_msg.role, cyberclaw_llm::types::Role::User);
        assert_eq!(llm_msg.content, "test");

        let sys_msg = ChatMessage {
            role: "system".to_string(),
            content: "system prompt".to_string(),
        };
        let llm_msg = sys_msg.to_llm_message();
        assert_eq!(llm_msg.role, cyberclaw_llm::types::Role::System);

        let asst_msg = ChatMessage {
            role: "assistant".to_string(),
            content: "reply".to_string(),
        };
        let llm_msg = asst_msg.to_llm_message();
        assert_eq!(llm_msg.role, cyberclaw_llm::types::Role::Assistant);
    }

    #[test]
    fn test_stream_event_serialization() {
        let delta = StreamEvent::Delta {
            content: "hello".to_string(),
        };
        let json = serde_json::to_value(&delta).unwrap();
        assert_eq!(json["type"], "delta");
        assert_eq!(json["content"], "hello");

        let tool_start = StreamEvent::ToolCallStart {
            id: "call-1".to_string(),
            name: "file.read".to_string(),
        };
        let json = serde_json::to_value(&tool_start).unwrap();
        assert_eq!(json["type"], "tool_call_start");
        assert_eq!(json["id"], "call-1");
        assert_eq!(json["name"], "file.read");

        let tool_result = StreamEvent::ToolCallResult {
            id: "call-1".to_string(),
            result: serde_json::json!({"data": "content"}),
        };
        let json = serde_json::to_value(&tool_result).unwrap();
        assert_eq!(json["type"], "tool_call_result");

        let done = StreamEvent::Done;
        let json = serde_json::to_value(&done).unwrap();
        assert_eq!(json["type"], "done");

        let error = StreamEvent::Error {
            message: "something failed".to_string(),
        };
        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(json["type"], "error");
        assert_eq!(json["message"], "something failed");
    }

    #[test]
    fn test_agent_chat_request_with_skills() {
        let json = serde_json::json!({
            "messages": [{"role": "user", "content": "Hello"}],
            "skill_ids": ["skill-a", "skill-b"],
            "system_prompt": "You are a specialized agent.",
            "max_iterations": 10
        });

        let req: AgentChatRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.skill_ids.as_ref().unwrap().len(), 2);
        assert_eq!(
            req.system_prompt.as_ref().unwrap(),
            "You are a specialized agent."
        );
        assert_eq!(req.max_iterations, Some(10));
    }

    #[test]
    fn test_agent_chat_response_budget_exhausted() {
        let resp = AgentChatResponse {
            id: "chatcmpl-456".to_string(),
            object: "chat.completion".to_string(),
            created: 1700000000,
            model: "gpt-4".to_string(),
            choices: vec![AgentChoice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: String::new(),
                },
                finish_reason: "budget_exhausted".to_string(),
            }],
            usage: AgentUsage {
                prompt_tokens: 5000,
                completion_tokens: 5000,
                total_tokens: 10000,
                iterations: 90,
            },
            plan: None,
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["choices"][0]["finish_reason"], "budget_exhausted");
        assert_eq!(json["usage"]["iterations"], 90);
    }

    // ---- Sprint 44+1: chat-handler binding resolution ----
    //
    // These tests pin the precedence contract between
    // `req.skill_ids` (per-request runtime override) and the agent's
    // `default_skills` (manifest declaration) by exercising the same
    // resolve path the handler uses on the LLM dispatch hot path.
    // We reach into the underlying `DefaultAgenticLoop` instead of
    // standing up a full HTTP/JWT/AppState rig — the goal is to verify
    // the wiring contract, not the axum router.

    // `AgenticLoop` trait must be in scope for `active_skill_bindings()`
    // to resolve via dynamic dispatch — DefaultAgenticLoop's inherent
    // method has the same signature so the trait import isn't strictly
    // required, but keeping it documents intent and stays robust if
    // the inherent override is removed in a future refactor.
    #[allow(unused_imports)]
    use cyberclaw_agent_runtime::agentic_loop::{
        AgenticLoop as _, DefaultAgenticLoop as _DefaultAgenticLoop,
    };
    use cyberclaw_core::execution::ExecutionContext as _ExecutionContext;
    use cyberclaw_core::ids::SkillId as _SkillId;
    use cyberclaw_llm::client::LlmClient as _LlmClient;
    use cyberclaw_llm::error::LlmResult as _LlmResult;
    use cyberclaw_llm::prelude::Stream as _Stream;
    use cyberclaw_llm::types::{
        ChatChunk as _ChatChunk, ChatRequest as _ChatRequest, ChatResponse as _ChatResponse,
    };

    // Minimal mocks; never actually called because tests don't run the loop.
    struct _MockLlm;
    #[async_trait::async_trait]
    impl _LlmClient for _MockLlm {
        async fn chat_completion(&self, _r: _ChatRequest) -> _LlmResult<_ChatResponse> {
            unreachable!("loop not exercised in binding-resolution tests")
        }
        async fn chat_completion_stream(
            &self,
            _r: _ChatRequest,
        ) -> _LlmResult<Box<dyn _Stream<Item = _LlmResult<_ChatChunk>> + Send + Unpin>> {
            unreachable!("loop not exercised in binding-resolution tests")
        }
        fn provider(&self) -> &str {
            "mock"
        }
        async fn validate_connection(&self) -> _LlmResult<()> {
            Ok(())
        }
    }

    struct _MockGw;
    #[async_trait::async_trait]
    impl cyberclaw_core::gateway::OrchestratorGateway for _MockGw {
        async fn execute_capability(
            &self,
            request: cyberclaw_core::gateway::CapabilityRequest,
        ) -> Result<cyberclaw_core::gateway::CapabilityResult, cyberclaw_core::gateway::GatewayError>
        {
            Ok(cyberclaw_core::gateway::CapabilityResult {
                execution_id: request.execution_id,
                capability_id: request.capability_id,
                output: serde_json::json!({}),
            })
        }
        async fn list_capabilities(
            &self,
        ) -> Result<
            Vec<cyberclaw_core::gateway::CapabilityInfo>,
            cyberclaw_core::gateway::GatewayError,
        > {
            Ok(vec![])
        }
    }

    fn make_loop_for_resolution() -> _DefaultAgenticLoop {
        let llm = std::sync::Arc::new(_MockLlm);
        let gw = std::sync::Arc::new(_MockGw);
        _DefaultAgenticLoop::new(llm, gw)
    }

    fn parse_request_skill_ids(req: &AgentChatRequest) -> Option<Vec<_SkillId>> {
        req.skill_ids.as_ref().map(|raws| {
            raws.iter()
                .filter_map(|r| _SkillId::from_string(r.clone()).ok())
                .collect()
        })
    }

    #[test]
    fn binding_resolve_agent_default_used_when_request_omits_skill_ids() {
        // Agent default = ["powerpoint"], req.skill_ids = None
        // -> loop sees agent default.
        let req = AgentChatRequest {
            messages: vec![],
            model: None,
            stream: None,
            tools: None,
            agent_id: Some("agent-x".into()),
            skill_ids: None,
            system_prompt: None,
            max_iterations: None,
            execution_mode: None,
            _persistent_test_seed: None,
        };
        let agent_defaults = vec![_SkillId::from_string("powerpoint".into()).unwrap()];

        let runtime_overrides = parse_request_skill_ids(&req);
        let ctx = match runtime_overrides {
            Some(list) => _ExecutionContext::new().with_runtime_skill_bindings(list),
            None => _ExecutionContext::new(),
        };

        let mut l = make_loop_for_resolution();
        l.resolve_skill_bindings(&agent_defaults, Some(&ctx));
        assert_eq!(l.active_skill_bindings().len(), 1);
        assert_eq!(l.active_skill_bindings()[0].as_str(), "powerpoint");
    }

    #[test]
    fn binding_resolve_request_override_used_when_agent_default_empty() {
        // Agent default = [], req.skill_ids = ["powerpoint"]
        // -> loop sees the request override.
        let req = AgentChatRequest {
            messages: vec![],
            model: None,
            stream: None,
            tools: None,
            agent_id: Some("agent-x".into()),
            skill_ids: Some(vec!["powerpoint".into()]),
            system_prompt: None,
            max_iterations: None,
            execution_mode: None,
            _persistent_test_seed: None,
        };
        let agent_defaults: Vec<_SkillId> = vec![];

        let runtime_overrides = parse_request_skill_ids(&req);
        let ctx = match runtime_overrides {
            Some(list) => _ExecutionContext::new().with_runtime_skill_bindings(list),
            None => _ExecutionContext::new(),
        };

        let mut l = make_loop_for_resolution();
        l.resolve_skill_bindings(&agent_defaults, Some(&ctx));
        assert_eq!(l.active_skill_bindings().len(), 1);
        assert_eq!(l.active_skill_bindings()[0].as_str(), "powerpoint");
    }

    #[test]
    fn binding_resolve_request_override_wins_over_agent_default() {
        // Agent default = ["powerpoint"], req.skill_ids = ["excel"]
        // -> req.skill_ids wins.
        let req = AgentChatRequest {
            messages: vec![],
            model: None,
            stream: None,
            tools: None,
            agent_id: Some("agent-x".into()),
            skill_ids: Some(vec!["excel".into()]),
            system_prompt: None,
            max_iterations: None,
            execution_mode: None,
            _persistent_test_seed: None,
        };
        let agent_defaults = vec![_SkillId::from_string("powerpoint".into()).unwrap()];

        let runtime_overrides = parse_request_skill_ids(&req);
        let ctx = match runtime_overrides {
            Some(list) => _ExecutionContext::new().with_runtime_skill_bindings(list),
            None => _ExecutionContext::new(),
        };

        let mut l = make_loop_for_resolution();
        l.resolve_skill_bindings(&agent_defaults, Some(&ctx));
        assert_eq!(l.active_skill_bindings().len(), 1);
        assert_eq!(l.active_skill_bindings()[0].as_str(), "excel");
    }

    #[test]
    fn binding_resolve_request_empty_array_explicitly_clears_defaults() {
        // Agent default = ["powerpoint"], req.skill_ids = Some(vec![])
        // -> explicit clear; loop has no bindings.
        let req = AgentChatRequest {
            messages: vec![],
            model: None,
            stream: None,
            tools: None,
            agent_id: Some("agent-x".into()),
            skill_ids: Some(vec![]),
            system_prompt: None,
            max_iterations: None,
            execution_mode: None,
            _persistent_test_seed: None,
        };
        let agent_defaults = vec![_SkillId::from_string("powerpoint".into()).unwrap()];

        let runtime_overrides = parse_request_skill_ids(&req);
        let ctx = match runtime_overrides {
            Some(list) => _ExecutionContext::new().with_runtime_skill_bindings(list),
            None => _ExecutionContext::new(),
        };

        let mut l = make_loop_for_resolution();
        l.resolve_skill_bindings(&agent_defaults, Some(&ctx));
        assert!(l.active_skill_bindings().is_empty());
    }

    #[test]
    fn binding_resolve_invalid_skill_ids_skipped_silently() {
        // SkillId::from_string rejects control chars, "..", etc. Invalid
        // entries log a warning and drop out — they do not poison the
        // whole list. Same as connector's silent-skip on parse failure.
        let req = AgentChatRequest {
            messages: vec![],
            model: None,
            stream: None,
            tools: None,
            agent_id: Some("agent-x".into()),
            // "../foo" is rejected by SkillId validation; "valid" survives.
            skill_ids: Some(vec!["../foo".into(), "valid".into()]),
            system_prompt: None,
            max_iterations: None,
            execution_mode: None,
            _persistent_test_seed: None,
        };
        let agent_defaults: Vec<_SkillId> = vec![];

        let runtime_overrides = parse_request_skill_ids(&req);
        // Should retain the one valid id only.
        let parsed = runtime_overrides.expect("Some when req.skill_ids is Some");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].as_str(), "valid");

        let ctx = _ExecutionContext::new().with_runtime_skill_bindings(parsed);
        let mut l = make_loop_for_resolution();
        l.resolve_skill_bindings(&agent_defaults, Some(&ctx));
        assert_eq!(l.active_skill_bindings().len(), 1);
        assert_eq!(l.active_skill_bindings()[0].as_str(), "valid");
    }

    // ---- Sprint D3/D5: AgentChatRequest.execution_mode contract ----
    //
    // These tests pin the wire-level contract for the new
    // `execution_mode` field. The brief from the D5 e2e report:
    // serialization must be back-compatible with pre-D5 clients
    // (omitted field), and the field must round-trip through the
    // request type for `Normal`, `Autopilot`, `Persistent`. The
    // resolved-default rule (`req.execution_mode.unwrap_or_default()`)
    // is also checked because that's the value the handler logs and
    // routes on.

    #[test]
    fn execution_mode_back_compat_legacy_request_without_field_deserialises() {
        // A pre-D5 client that doesn't know about execution_mode must
        // continue to work — the field must default to None on the
        // wire and the handler must treat that as Normal.
        let json = serde_json::json!({
            "messages": [{"role": "user", "content": "hi"}]
        });
        let req: AgentChatRequest = serde_json::from_value(json).unwrap();
        assert!(
            req.execution_mode.is_none(),
            "omitted execution_mode must deserialise to None"
        );
        // Handler-side resolution: None -> Normal (back-compat default).
        let resolved = req.execution_mode.unwrap_or_default();
        assert!(
            matches!(resolved, cyberclaw_core::execution::ExecutionMode::Normal),
            "None must resolve to Normal, got {:?}",
            resolved
        );
    }

    #[test]
    fn execution_mode_persistent_round_trips_through_serde() {
        // Explicit Persistent must deserialise correctly so the chat
        // handler routes through the ExecutionService dispatch path.
        let json = serde_json::json!({
            "messages": [{"role": "user", "content": "go"}],
            "execution_mode": "persistent"
        });
        let req: AgentChatRequest = serde_json::from_value(json).unwrap();
        assert_eq!(
            req.execution_mode,
            Some(cyberclaw_core::execution::ExecutionMode::Persistent),
            "Persistent must round-trip"
        );
        // Re-serialise + deserialise to confirm symmetry on both legs.
        let back = serde_json::to_value(&req).unwrap();
        assert_eq!(back["execution_mode"], "persistent");
    }

    #[test]
    fn execution_mode_autopilot_round_trips_and_does_not_break_handler() {
        // Autopilot must not break the chat endpoint even though
        // autopilot has its own dedicated handler — symmetry only.
        let json = serde_json::json!({
            "messages": [{"role": "user", "content": "go"}],
            "execution_mode": "autopilot"
        });
        let req: AgentChatRequest = serde_json::from_value(json).unwrap();
        assert_eq!(
            req.execution_mode,
            Some(cyberclaw_core::execution::ExecutionMode::Autopilot),
            "Autopilot must round-trip"
        );
        // Resolved value drives the handler's `matches!(_, Persistent)`
        // branch — Autopilot must NOT match.
        let resolved = req.execution_mode.unwrap_or_default();
        assert!(
            !matches!(
                resolved,
                cyberclaw_core::execution::ExecutionMode::Persistent
            ),
            "Autopilot must not be confused with Persistent"
        );
    }

    #[test]
    fn execution_mode_normal_explicit_round_trips() {
        // Explicit Normal must round-trip the same as omission.
        let json = serde_json::json!({
            "messages": [{"role": "user", "content": "ping"}],
            "execution_mode": "normal"
        });
        let req: AgentChatRequest = serde_json::from_value(json).unwrap();
        assert_eq!(
            req.execution_mode,
            Some(cyberclaw_core::execution::ExecutionMode::Normal)
        );
        let resolved = req.execution_mode.unwrap_or_default();
        assert!(matches!(
            resolved,
            cyberclaw_core::execution::ExecutionMode::Normal
        ));
    }

    #[test]
    fn execution_mode_routing_predicate_only_fires_on_persistent() {
        // The handler decides on the dispatch path with
        // `matches!(resolved, ExecutionMode::Persistent)`. Pin that
        // predicate's truth table so future ExecutionMode variants
        // don't silently start routing through the persistent branch.
        let cases = [
            (None, false),
            (
                Some(cyberclaw_core::execution::ExecutionMode::Normal),
                false,
            ),
            (
                Some(cyberclaw_core::execution::ExecutionMode::Autopilot),
                false,
            ),
            (
                Some(cyberclaw_core::execution::ExecutionMode::Persistent),
                true,
            ),
        ];
        for (mode, expected_persistent) in cases {
            let req = AgentChatRequest {
                messages: vec![],
                model: None,
                stream: None,
                tools: None,
                agent_id: None,
                skill_ids: None,
                system_prompt: None,
                max_iterations: None,
                execution_mode: mode,
                _persistent_test_seed: None,
            };
            let resolved = req.execution_mode.unwrap_or_default();
            let routes_persistent = matches!(
                resolved,
                cyberclaw_core::execution::ExecutionMode::Persistent
            );
            assert_eq!(
                routes_persistent, expected_persistent,
                "mode {:?} predicate mismatch",
                mode
            );
        }
    }

    #[test]
    fn execution_mode_field_on_struct_ctor_back_compat() {
        // Direct struct construction (the path used internally and
        // by adjacent tests) must accept `execution_mode: None` so
        // existing callers don't have to rebuild every ctor — the
        // serde default is for the wire path; this is for code.
        let req = AgentChatRequest {
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: "test".to_string(),
            }],
            model: Some("gpt-4".to_string()),
            stream: Some(false),
            tools: None,
            agent_id: None,
            skill_ids: None,
            system_prompt: None,
            max_iterations: None,
            execution_mode: None,
            _persistent_test_seed: None,
        };
        assert!(req.execution_mode.is_none());
        // And the same struct with Persistent must construct without
        // touching the rest — serialise it to verify wire shape.
        let req_p = AgentChatRequest {
            execution_mode: Some(cyberclaw_core::execution::ExecutionMode::Persistent),
            ..req
        };
        let v = serde_json::to_value(&req_p).unwrap();
        assert_eq!(v["execution_mode"], "persistent");
    }

    // ---- Sprint D3: PersistentStoryPlanner integration tests ----

    /// Plan field absent from JSON when None (skip_serializing_if).
    #[test]
    fn plan_field_omitted_from_json_when_none() {
        let resp = AgentChatResponse {
            id: "chatcmpl-d3".to_string(),
            object: "chat.completion".to_string(),
            created: 1700000000,
            model: "gpt-4".to_string(),
            choices: vec![AgentChoice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: "ok".to_string(),
                },
                finish_reason: "stop".to_string(),
            }],
            usage: AgentUsage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
                iterations: 0,
            },
            plan: None,
        };
        let json = serde_json::to_value(&resp).unwrap();
        // plan: None must be omitted entirely (skip_serializing_if = "Option::is_none")
        assert!(
            json.get("plan").is_none(),
            "plan key must be absent when None, got: {:?}",
            json.get("plan")
        );
    }

    /// Plan field present and correctly structured when Some.
    #[test]
    fn plan_field_serializes_correctly_when_some() {
        let summary = PersistentPlanSummary {
            goal: "transcribe audio".to_string(),
            stories: vec![
                StorySummary {
                    id: "S1".to_string(),
                    description: "run transcription".to_string(),
                    capability_id: Some("voice.transcribe".to_string()),
                    acceptance: vec![],
                },
                StorySummary {
                    id: "S2".to_string(),
                    description: "verify output".to_string(),
                    capability_id: None,
                    acceptance: vec![],
                },
            ],
        };
        let resp = AgentChatResponse {
            id: "chatcmpl-d3b".to_string(),
            object: "chat.completion".to_string(),
            created: 1700000000,
            model: "gpt-4".to_string(),
            choices: vec![AgentChoice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: "[persistent dispatch]".to_string(),
                },
                finish_reason: "persistent_completed".to_string(),
            }],
            usage: AgentUsage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
                iterations: 0,
            },
            plan: Some(summary),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["plan"]["goal"], "transcribe audio");
        assert_eq!(json["plan"]["stories"].as_array().unwrap().len(), 2);
        assert_eq!(json["plan"]["stories"][0]["id"], "S1");
        assert_eq!(
            json["plan"]["stories"][0]["capability_id"],
            "voice.transcribe"
        );
        // capability_id: None must be omitted
        assert!(
            json["plan"]["stories"][1].get("capability_id").is_none(),
            "capability_id must be absent when None"
        );
    }

    /// PersistentStoryPlanner produces a plan that maps correctly to PersistentPlanSummary.
    #[tokio::test]
    async fn persistent_story_planner_produces_non_empty_plan_summary() {
        use async_trait::async_trait;
        use cyberclaw_control_plane::persistent_story_planner::PersistentStoryPlanner;
        use cyberclaw_llm::error::LlmResult;
        use cyberclaw_llm::types::{ChatChunk, ChatResponse, Choice, Message};
        use futures::stream::Stream;
        use std::sync::Arc;

        struct OkLlm;

        #[async_trait]
        impl cyberclaw_llm::LlmClient for OkLlm {
            async fn chat_completion(
                &self,
                _req: cyberclaw_llm::types::ChatRequest,
            ) -> LlmResult<ChatResponse> {
                let body = r#"{
                    "stories": [
                        {
                            "id": "S1",
                            "description": "fetch data",
                            "capability_id": null,
                            "depends_on": [],
                            "criteria": [
                                {"description": "data present",
                                 "verifier": {"type": "file_exists", "path": "/tmp/out"}}
                            ]
                        }
                    ]
                }"#;
                Ok(ChatResponse {
                    id: "mock".to_string(),
                    object: "chat.completion".to_string(),
                    created: 0,
                    model: "mock".to_string(),
                    choices: vec![Choice {
                        index: 0,
                        message: Message::assistant(body.to_string()),
                        finish_reason: Some("stop".to_string()),
                    }],
                    usage: None,
                    rate_limit: None,
                })
            }
            async fn chat_completion_stream(
                &self,
                _req: cyberclaw_llm::types::ChatRequest,
            ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>>
            {
                unreachable!()
            }
            fn provider(&self) -> &str {
                "mock"
            }
            async fn validate_connection(&self) -> LlmResult<()> {
                Ok(())
            }
        }

        let planner = PersistentStoryPlanner::new(Arc::new(OkLlm), "mock-model", vec![]);
        let plan = planner.plan("fetch and process data").await;

        // Map to PersistentPlanSummary exactly as persistent_chat_dispatch does.
        let summary = PersistentPlanSummary {
            goal: plan.goal.clone(),
            stories: plan
                .stories
                .iter()
                .map(|s| StorySummary {
                    id: s.id.clone(),
                    description: s.description.clone(),
                    capability_id: s.capability_id.as_ref().map(|c| c.as_str().to_string()),
                    acceptance: vec![],
                })
                .collect(),
        };

        assert!(
            !summary.stories.is_empty(),
            "plan must have at least one story"
        );
        assert_eq!(summary.stories[0].id, "S1");
        assert_eq!(summary.stories[0].description, "fetch data");
        assert!(summary.stories[0].capability_id.is_none());
    }

    /// LLM failure in planner yields fallback placeholder summary (non-empty stories).
    #[tokio::test]
    async fn persistent_story_planner_llm_failure_yields_fallback_summary() {
        use async_trait::async_trait;
        use cyberclaw_control_plane::persistent_story_planner::PersistentStoryPlanner;
        use cyberclaw_llm::error::{LlmError, LlmResult};
        use cyberclaw_llm::types::{ChatChunk, ChatRequest, ChatResponse};
        use futures::stream::Stream;
        use std::sync::Arc;

        struct ErrLlm;

        #[async_trait]
        impl cyberclaw_llm::LlmClient for ErrLlm {
            async fn chat_completion(&self, _req: ChatRequest) -> LlmResult<ChatResponse> {
                Err(LlmError::Internal("network down".to_string()))
            }
            async fn chat_completion_stream(
                &self,
                _req: ChatRequest,
            ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>>
            {
                unreachable!()
            }
            fn provider(&self) -> &str {
                "err"
            }
            async fn validate_connection(&self) -> LlmResult<()> {
                Ok(())
            }
        }

        let planner = PersistentStoryPlanner::new(Arc::new(ErrLlm), "mock-model", vec![]);
        let plan = planner.plan("anything").await;

        // Placeholder plan: goal contains "[placeholder]", one story.
        assert!(
            plan.goal.contains("[placeholder]"),
            "fallback goal must contain [placeholder], got: {}",
            plan.goal
        );
        let summary = PersistentPlanSummary {
            goal: plan.goal.clone(),
            stories: plan
                .stories
                .iter()
                .map(|s| StorySummary {
                    id: s.id.clone(),
                    description: s.description.clone(),
                    capability_id: s.capability_id.as_ref().map(|c| c.as_str().to_string()),
                    acceptance: vec![],
                })
                .collect(),
        };
        assert!(
            !summary.stories.is_empty(),
            "fallback must have at least one story"
        );
    }

    // ---------------------------------------------------------------------------
    // F3 — hallucination_claimed_completion unit tests
    // ---------------------------------------------------------------------------

    #[test]
    fn hallucination_detector_triggers_on_chinese_completion_with_path() {
        // Simulates the PPT scenario: LLM says "已生成" + mentions ./out/
        let text = "我已经完成了任务，文件已生成到 ./out/cyberclaw-intro.pptx，请查看。";
        assert!(
            hallucination_claimed_completion(text).is_some(),
            "should detect Chinese completion claim with file path"
        );
    }

    #[test]
    fn hallucination_detector_triggers_on_english_completion_with_path() {
        let text = "I have successfully created the presentation at ./out/slides.pptx.";
        assert!(
            hallucination_claimed_completion(text).is_some(),
            "should detect English completion claim with file path"
        );
    }

    #[test]
    fn hallucination_detector_no_false_positive_without_path() {
        // Completion phrase but no file path -> should NOT trigger.
        let text = "Task complete. Let me know if you need anything else.";
        assert!(
            hallucination_claimed_completion(text).is_none(),
            "should not trigger without a file path reference"
        );
    }

    #[test]
    fn hallucination_detector_no_false_positive_path_only() {
        // Has a path but no completion claim -> should NOT trigger.
        let text = "The file ./out/data.json contains the raw results from the API.";
        assert!(
            hallucination_claimed_completion(text).is_none(),
            "should not trigger when path present but no completion claim"
        );
    }

    #[test]
    fn hallucination_detector_returns_excerpt_up_to_120_chars() {
        let long_text = format!(
            "I have successfully generated the report at ./out/report.pdf. {}",
            "x".repeat(200)
        );
        let excerpt = hallucination_claimed_completion(&long_text).unwrap();
        assert!(
            excerpt.chars().count() <= 120,
            "excerpt should be at most 120 chars, got {}",
            excerpt.chars().count()
        );
    }

    #[test]
    fn hallucination_detector_case_insensitive() {
        let text = "SUCCESSFULLY CREATED the output at ./out/result.csv";
        assert!(
            hallucination_claimed_completion(text).is_some(),
            "detector should be case-insensitive"
        );
    }

    // ---------------------------------------------------------------------------
    // P1.2 / P1.3 — extract_paths_from_text + HallucinationMode unit tests
    // ---------------------------------------------------------------------------

    #[test]
    fn extract_paths_finds_relative_path_with_extension() {
        let text = "I have completed generation: file ./out/cyberclaw-intro.pptx is ready.";
        let paths = extract_paths_from_text(text);
        assert!(
            paths.iter().any(|p| p == "./out/cyberclaw-intro.pptx"),
            "relative path with .pptx must be extracted, got: {:?}",
            paths
        );
    }

    #[test]
    fn extract_paths_finds_absolute_unix_path() {
        let text = "Output saved to /tmp/foo/bar.json successfully.";
        let paths = extract_paths_from_text(text);
        assert!(
            paths.iter().any(|p| p == "/tmp/foo/bar.json"),
            "absolute /tmp path must be extracted, got: {:?}",
            paths
        );
    }

    #[test]
    fn extract_paths_strips_trailing_punctuation() {
        let text = "Wrote ./out/report.pdf, then exited.";
        let paths = extract_paths_from_text(text);
        assert!(
            paths.iter().any(|p| p == "./out/report.pdf"),
            "trailing comma should be stripped, got: {:?}",
            paths
        );
    }

    #[test]
    fn extract_paths_skips_urls() {
        let text = "See https://example.com/foo.json for details.";
        let paths = extract_paths_from_text(text);
        assert!(
            paths.is_empty() || !paths.iter().any(|p| p.contains("example.com")),
            "URLs must not be extracted as filesystem paths, got: {:?}",
            paths
        );
    }

    #[test]
    fn extract_paths_dedupes_same_path() {
        let text = "Created ./out/x.csv. Then ./out/x.csv was finalized.";
        let paths = extract_paths_from_text(text);
        let count = paths.iter().filter(|p| p == &"./out/x.csv").count();
        assert_eq!(
            count, 1,
            "duplicate paths should be deduped, got: {:?}",
            paths
        );
    }

    #[test]
    fn extract_paths_returns_empty_for_no_paths() {
        let text = "I have completed the task with no file references.";
        let paths = extract_paths_from_text(text);
        assert!(paths.is_empty(), "no paths expected, got: {:?}", paths);
    }

    // Env-var tests serialize through this mutex because Rust runs tests in
    // parallel by default and `std::env::set_var` mutates process-global
    // state. Holding the guard across each `from_env()` call keeps observed
    // values consistent.
    fn _env_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    #[test]
    fn hallucination_mode_defaults_to_warn() {
        let _g = _env_lock().lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("CYBERCLAW_HALLUCINATION_MODE");
        assert_eq!(HallucinationMode::from_env(), HallucinationMode::Warn);
    }

    #[test]
    fn hallucination_mode_parses_verify() {
        let _g = _env_lock().lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CYBERCLAW_HALLUCINATION_MODE", "verify");
        assert_eq!(HallucinationMode::from_env(), HallucinationMode::Verify);
        std::env::remove_var("CYBERCLAW_HALLUCINATION_MODE");
    }

    #[test]
    fn hallucination_mode_parses_block_case_insensitive() {
        let _g = _env_lock().lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CYBERCLAW_HALLUCINATION_MODE", "BLOCK");
        assert_eq!(HallucinationMode::from_env(), HallucinationMode::Block);
        std::env::remove_var("CYBERCLAW_HALLUCINATION_MODE");
    }

    #[test]
    fn hallucination_mode_unknown_falls_back_to_warn() {
        let _g = _env_lock().lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CYBERCLAW_HALLUCINATION_MODE", "panic");
        assert_eq!(HallucinationMode::from_env(), HallucinationMode::Warn);
        std::env::remove_var("CYBERCLAW_HALLUCINATION_MODE");
    }

    // ---------------------------------------------------------------------------
    // P1.2 — verify_paths_with_file_stat dispatch behaviour
    // ---------------------------------------------------------------------------

    /// Minimal in-memory gateway that returns `exists` based on a fixed
    /// allow-list. Used to exercise the verify path without standing up
    /// a real LocalConnector / governance pipeline.
    struct _ExistsGateway {
        existing: std::collections::HashSet<String>,
    }

    #[async_trait::async_trait]
    impl cyberclaw_core::gateway::OrchestratorGateway for _ExistsGateway {
        async fn execute_capability(
            &self,
            request: cyberclaw_core::gateway::CapabilityRequest,
        ) -> Result<cyberclaw_core::gateway::CapabilityResult, cyberclaw_core::gateway::GatewayError>
        {
            let path = request
                .input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let exists = self.existing.contains(path);
            Ok(cyberclaw_core::gateway::CapabilityResult {
                execution_id: request.execution_id,
                capability_id: request.capability_id,
                output: serde_json::json!({ "exists": exists }),
            })
        }
        async fn list_capabilities(
            &self,
        ) -> Result<
            Vec<cyberclaw_core::gateway::CapabilityInfo>,
            cyberclaw_core::gateway::GatewayError,
        > {
            Ok(vec![])
        }
    }

    fn _test_actor() -> cyberclaw_core::identity::ActorRef {
        cyberclaw_core::identity::ActorRef {
            id: cyberclaw_core::ids::ActorId::from_string("test-actor".to_string()).unwrap(),
            actor_type: cyberclaw_core::identity::ActorType::Human,
            tenant_id: None,
            home_node_id: None,
            display_name: "test".to_string(),
        }
    }

    #[tokio::test]
    async fn verify_paths_returns_missing_subset() {
        let mut existing = std::collections::HashSet::new();
        existing.insert("/tmp/real.json".to_string());
        let gw: std::sync::Arc<dyn cyberclaw_core::gateway::OrchestratorGateway> =
            std::sync::Arc::new(_ExistsGateway { existing });
        let actor = _test_actor();
        let paths = vec!["/tmp/real.json".to_string(), "/tmp/ghost.json".to_string()];
        let missing = verify_paths_with_file_stat(&gw, &actor, &paths).await;
        assert_eq!(missing, vec!["/tmp/ghost.json".to_string()]);
    }

    #[tokio::test]
    async fn verify_paths_empty_input_returns_empty() {
        let gw: std::sync::Arc<dyn cyberclaw_core::gateway::OrchestratorGateway> =
            std::sync::Arc::new(_ExistsGateway {
                existing: std::collections::HashSet::new(),
            });
        let actor = _test_actor();
        let missing = verify_paths_with_file_stat(&gw, &actor, &[]).await;
        assert!(missing.is_empty());
    }

    // ---- Test 1: select_loop_profile heuristic ----

    fn msg(content: &str) -> ChatMessage {
        ChatMessage {
            role: "user".to_string(),
            content: content.to_string(),
        }
    }

    #[test]
    fn loop_profile_zero_messages_is_l1() {
        assert_eq!(select_loop_profile(&[]), LoopProfile::L1);
    }

    #[test]
    fn loop_profile_one_short_message_is_l1() {
        // 10 chars < 100 → L1
        assert_eq!(select_loop_profile(&[msg("hello!")]), LoopProfile::L1);
    }

    #[test]
    fn loop_profile_one_medium_message_is_l2() {
        // 150 chars: > 100, ≤ 500 → L2
        let content = "a".repeat(150);
        assert_eq!(select_loop_profile(&[msg(&content)]), LoopProfile::L2);
    }

    #[test]
    fn loop_profile_one_long_message_is_l3() {
        // 600 chars: > 500 → L3
        let content = "a".repeat(600);
        assert_eq!(select_loop_profile(&[msg(&content)]), LoopProfile::L3);
    }

    #[test]
    fn loop_profile_four_or_more_messages_is_l3() {
        // 4 short messages → L3 (multi-turn)
        let msgs = vec![msg("hi"), msg("hi"), msg("hi"), msg("hi")];
        assert_eq!(select_loop_profile(&msgs), LoopProfile::L3);
    }

    #[test]
    fn loop_profile_three_short_messages_is_l1() {
        // 3 messages, all short → L1
        let msgs = vec![msg("hi"), msg("hi"), msg("hi")];
        assert_eq!(select_loop_profile(&msgs), LoopProfile::L1);
    }

    // BUG-CB-16: multi-turn agentic conversations must use L3 budget.
    #[test]
    fn test_select_loop_profile_assistant_history_picks_l3() {
        // 1 user + 1 assistant + 1 user — the assistant reply triggers L3
        // even though the messages are short and fewer than 4.
        let msgs = vec![
            ChatMessage {
                role: "user".to_string(),
                content: "what files exist?".to_string(),
            },
            ChatMessage {
                role: "assistant".to_string(),
                content: "I'll check.".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: "thanks".to_string(),
            },
        ];
        assert_eq!(select_loop_profile(&msgs), LoopProfile::L3);
    }

    #[test]
    fn test_select_loop_profile_single_short_user_picks_l1() {
        // Single short user message — first turn, no tool history → L1 (no regression).
        let msgs = vec![ChatMessage {
            role: "user".to_string(),
            content: "hi".to_string(),
        }];
        assert_eq!(select_loop_profile(&msgs), LoopProfile::L1);
    }

    // BUG-CB-20: turn-1 agentic prompts must use L3 budget. ----------------------

    #[test]
    fn test_select_loop_profile_file_path_picks_l3() {
        // A prompt containing a file path (contains '/') → likely_agentic → L3.
        let msgs = vec![msg("please read /Users/foo/bar.txt and summarize it")];
        assert_eq!(select_loop_profile(&msgs), LoopProfile::L3);
    }

    #[test]
    fn test_select_loop_profile_create_keyword_picks_l3() {
        // A prompt with "create" → likely_agentic → L3.
        let msgs = vec![msg("create a pptx slide deck about the project")];
        assert_eq!(select_loop_profile(&msgs), LoopProfile::L3);
    }

    #[test]
    fn test_select_loop_profile_chinese_keyword_picks_l3() {
        // A prompt with Chinese "文件" → likely_agentic → L3.
        let msgs = vec![msg("写一份文件，总结项目进度")];
        assert_eq!(select_loop_profile(&msgs), LoopProfile::L3);
    }

    #[test]
    fn test_select_loop_profile_pure_short_text_still_l1() {
        // A pure short math question has no agentic keywords → stays L1 (no regression).
        let msgs = vec![msg("what is 2+2?")];
        assert_eq!(select_loop_profile(&msgs), LoopProfile::L1);
    }

    // BUG-CB-19 -------------------------------------------------------------------

    #[test]
    fn test_workspace_root_defaults_to_absolute_cwd() {
        // When neither CYBERCLAW_AGENT_WORKSPACE_ROOT nor
        // CYBERCLAW_WORKSPACE_WRITABLE_ROOTS is set, the workspace root
        // derived by CB-19 logic must be an absolute path (not ".").
        //
        // We replicate the resolution logic from build_governing_gateway and
        // the workspace_root_hint block so the test stays co-located with the
        // code it validates.
        // Temporarily unset both env vars to exercise the current_dir() path.
        let prev_root = std::env::var("CYBERCLAW_AGENT_WORKSPACE_ROOT").ok();
        let prev_roots = std::env::var("CYBERCLAW_WORKSPACE_WRITABLE_ROOTS").ok();
        unsafe {
            std::env::remove_var("CYBERCLAW_AGENT_WORKSPACE_ROOT");
            std::env::remove_var("CYBERCLAW_WORKSPACE_WRITABLE_ROOTS");
        }

        let workspace_root: String = std::env::var("CYBERCLAW_AGENT_WORKSPACE_ROOT")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned()))
            .unwrap_or_else(|| ".".to_string());

        // Restore env vars.
        unsafe {
            if let Some(v) = prev_root {
                std::env::set_var("CYBERCLAW_AGENT_WORKSPACE_ROOT", v);
            }
            if let Some(v) = prev_roots {
                std::env::set_var("CYBERCLAW_WORKSPACE_WRITABLE_ROOTS", v);
            }
        }

        assert_ne!(
            workspace_root, ".",
            "workspace root must not be the opaque relative path '.'; got '{workspace_root}'"
        );
        assert!(
            std::path::Path::new(&workspace_root).is_absolute(),
            "workspace root must be absolute; got '{workspace_root}'"
        );
    }
}
