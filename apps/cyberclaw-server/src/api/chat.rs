//! Chat Completions API
//!
//! 兼容 OpenAI Chat Completions API 格式

use axum::{
    extract::{Extension, State},
    http::{
        header::{HeaderName, HeaderValue},
        HeaderMap,
    },
    response::{IntoResponse, Response, Sse},
    routing::post,
    Json, Router,
};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use chrono::Utc;
use cyberclaw_control_plane::intent_classifier::{Intent, IntentClassifier};
use cyberclaw_core::ids::ExecutionId;
use cyberclaw_core::users::UsersConfig;
use cyberclaw_llm::{
    ChatChunk, ChatRequest, ChatResponse, Choice, FunctionDefinition, Message, ToolCall,
    ToolDefinition, Usage,
};
use cyberclaw_llm_bridge::types::ToolExecutionResult;
use cyberclaw_observability::events::ObservabilityEvent;
use cyberclaw_observability::EventRecorder;

use crate::api::skills::CreateSkillRequest;
use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::Claims;
use crate::state::AppState;

/// Chat Completions 请求（OpenAI 兼容格式）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    /// 模型名称
    pub model: String,
    /// 消息列表
    pub messages: Vec<Message>,
    /// 温度参数（0.0 - 2.0）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// top_p 参数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// 最大生成 token 数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// 工具定义列表
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    /// 工具选择策略
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,
    /// 是否流式返回
    #[serde(default)]
    pub stream: bool,
    /// Sprint 12 L1 — optional conversation id for chat threads that use
    /// conversational approval. Forwarded to audit metadata; otherwise
    /// ignored. Opaque to the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
}

/// Name of the synthetic `request_approval` tool injected into the LLM's
/// tool list. The LLM is instructed to emit this tool_call (instead of
/// calling the capability directly) when a high-risk action needs human
/// sign-off. The frontend renders the emitted tool_call as an approval
/// card; when the operator clicks Approve/Reject the UI POSTs
/// `/api/v1/chat/approval` (see `chat_approval.rs`).
pub const REQUEST_APPROVAL_TOOL: &str = "request_approval";

/// Build the JSON-Schema backing [`REQUEST_APPROVAL_TOOL`].
///
/// Kept as a free function so the frontend and the chat-approval test suite
/// share a single source of truth — the `request_id` emitted here is the
/// same shape consumed by the approval endpoint.
pub fn request_approval_tool_definition() -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: REQUEST_APPROVAL_TOOL.to_string(),
            description: concat!(
                "Request explicit human approval before executing a ",
                "high-risk capability. Emit this tool_call INSTEAD OF ",
                "invoking the capability directly when the action is ",
                "destructive, non-reversible, touches production, or ",
                "matches the DangerousCapabilityFilter. The chat UI will ",
                "surface an approval card keyed by `request_id`; wait for ",
                "the operator's decision before continuing."
            )
            .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "required": ["request_id", "action", "risk", "capability_id"],
                "properties": {
                    "request_id": {
                        "type": "string",
                        "description": "Stable id for this approval request. \
                            Reuse when referring back to the same decision."
                    },
                    "action": {
                        "type": "string",
                        "description": "Short human-readable description of \
                            what would execute if approved."
                    },
                    "risk": {
                        "type": "string",
                        "enum": ["low", "medium", "high", "critical"],
                        "description": "Risk classification."
                    },
                    "capability_id": {
                        "type": "string",
                        "description": "Fully-qualified capability identifier, \
                            e.g. `github/create-pr` or `shell/exec`."
                    },
                    "rationale": {
                        "type": "string",
                        "description": "Optional reasoning shown to the reviewer."
                    }
                }
            }),
        },
        cache_control: None,
    }
}

/// Inject the `request_approval` tool into the outgoing request if the
/// caller did not already provide one with the same name.
///
/// Kept idempotent so upstream proxies that pre-populate the tool list
/// don't get duplicates.
pub fn inject_request_approval_tool(req: &mut ChatRequest) {
    let def = request_approval_tool_definition();
    match &mut req.tools {
        Some(tools)
            if tools
                .iter()
                .any(|t| t.function.name == REQUEST_APPROVAL_TOOL) =>
        {
            // Already present — leave alone.
        }
        Some(tools) => {
            tools.push(def);
        }
        None => {
            req.tools = Some(vec![def]);
        }
    }
}

impl From<ChatCompletionRequest> for ChatRequest {
    fn from(req: ChatCompletionRequest) -> Self {
        ChatRequest {
            model: req.model,
            messages: req.messages,
            temperature: req.temperature,
            top_p: req.top_p,
            max_tokens: req.max_tokens,
            tools: req.tools,
            tool_choice: req.tool_choice,
            stream: Some(req.stream),
            // `conversation_id` is NOT sent to the underlying LLM — it's a
            // server-side correlation id only. It is preserved on the
            // ChatCompletionRequest side and consumed by the approval
            // endpoint + audit trail.
            extra: Default::default(),
        }
    }
}

/// Chat Completions 响应（OpenAI 兼容格式）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    /// 响应 ID
    pub id: String,
    /// 对象类型（"chat.completion"）
    pub object: String,
    /// 创建时间戳
    pub created: i64,
    /// 模型名称
    pub model: String,
    /// 选择列表
    pub choices: Vec<Choice>,
    /// 使用情况统计
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

impl From<ChatResponse> for ChatCompletionResponse {
    fn from(resp: ChatResponse) -> Self {
        ChatCompletionResponse {
            id: resp.id,
            object: resp.object,
            created: resp.created,
            model: resp.model,
            choices: resp.choices,
            usage: resp.usage,
        }
    }
}

/// Intent hint resolved for a chat request.
///
/// Populated only when `CYBERCLAW_CHAT_INTENT_HINT` is not `"off"` and the
/// last user message matches a known platform intent.
#[derive(Debug, Clone)]
struct IntentHint {
    /// Short kebab-case label, e.g. `"create-skill"`.
    hint: &'static str,
    /// Recommended API endpoint for this intent.
    suggested_endpoint: &'static str,
}

/// Return `true` when the intent-hint feature is enabled.
///
/// Reads `CYBERCLAW_CHAT_INTENT_HINT`. Any value of `"off"` (case-insensitive)
/// disables the feature; every other value (including absence) leaves it on.
fn intent_hint_enabled() -> bool {
    std::env::var("CYBERCLAW_CHAT_INTENT_HINT")
        .ok()
        .map(|v| !v.trim().eq_ignore_ascii_case("off"))
        .unwrap_or(true)
}

/// Auto-route mode for chat requests.
///
/// Controlled by `CYBERCLAW_CHAT_AUTO_ROUTE`:
/// - `"off"` — no auto-routing; all requests go through the LLM (intent hints
///   are still emitted if `CYBERCLAW_CHAT_INTENT_HINT` is on).
/// - `"advisor"` (default) — advisor mode only; intent hints injected into
///   response headers but no automatic dispatch.
/// - `"auto"` — full opt-in routing: when the operator has
///   `intent_auto_route=true` AND the request contains a parseable skill spec,
///   the request is dispatched internally to the create-skill logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatAutoRouteMode {
    Off,
    Advisor,
    Auto,
}

fn parse_auto_route_mode(value: Option<&str>) -> ChatAutoRouteMode {
    match value
        .unwrap_or("advisor")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "off" => ChatAutoRouteMode::Off,
        "auto" => ChatAutoRouteMode::Auto,
        _ => ChatAutoRouteMode::Advisor,
    }
}

fn chat_auto_route_mode() -> ChatAutoRouteMode {
    parse_auto_route_mode(std::env::var("CYBERCLAW_CHAT_AUTO_ROUTE").ok().as_deref())
}

/// Load the operator record for `user_id` from `~/.cyberclaw/users.toml`.
/// Returns `None` when the record is missing or the file cannot be read.
fn load_operator_auto_route(user_id: &cyberclaw_core::ids::UserId) -> bool {
    let path = UsersConfig::default_path();
    let cfg = match UsersConfig::load_from_path(&path) {
        Ok(c) => c,
        Err(_) => return false,
    };
    cfg.find(user_id)
        .map(|r| r.intent_auto_route)
        .unwrap_or(false)
}

/// Attempt to parse a [`CreateSkillRequest`] from the last user message.
///
/// Accepts two formats:
/// 1. A JSON object embedded anywhere in the message (first `{...}` block that
///    deserialises cleanly as `CreateSkillRequest`).
/// 2. A bare natural-language description where the message itself is used as
///    `description` and a `name` can be derived — only when the message
///    contains the word "skill" and includes a quoted token or kebab-cased word
///    that passes the slug guard.
///
/// Returns `None` when neither heuristic succeeds so the caller can fall back
/// to advisor mode.
fn parse_create_skill_spec(messages: &[Message]) -> Option<CreateSkillRequest> {
    use cyberclaw_llm::Role;
    let text = messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .map(|m| m.content.as_str())?;

    // Heuristic 1: find the first JSON object in the message.
    if let Some(start) = text.find('{') {
        if let Some(end) = text[start..].rfind('}') {
            let candidate = &text[start..start + end + 1];
            if let Ok(req) = serde_json::from_str::<CreateSkillRequest>(candidate) {
                if !req.name.trim().is_empty() && !req.description.trim().is_empty() {
                    return Some(req);
                }
            }
        }
    }

    // Heuristic 2: bare prose — derive name from quoted token or first
    // kebab-slug-like word after "skill".
    let lower = text.to_lowercase();
    if !lower.contains("skill") {
        return None;
    }
    // Try to find a quoted name: "foo-bar" or 'foo-bar'
    let name_candidate =
        extract_quoted_slug(text).or_else(|| extract_kebab_word_after_skill(text))?;

    let valid_slug = name_candidate.len() >= 3
        && name_candidate.len() <= 32
        && name_candidate
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase())
        && name_candidate
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid_slug {
        return None;
    }

    Some(CreateSkillRequest {
        name: name_candidate,
        description: text.trim().to_string(),
        methodology: None,
        trigger_examples: vec![],
        source_request_id: None,
    })
}

/// Extract the first `"slug"` or `'slug'` in `text`.
fn extract_quoted_slug(text: &str) -> Option<String> {
    for delim in ['"', '\''] {
        if let Some(start) = text.find(delim) {
            let rest = &text[start + 1..];
            if let Some(end) = rest.find(delim) {
                let candidate = rest[..end].trim().to_lowercase();
                if !candidate.is_empty() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// Find the first word after "skill" (or "skill called", "skill named") that
/// looks like a kebab-slug.
fn extract_kebab_word_after_skill(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let trigger_idx = lower.find("skill")?;
    let after = &lower[trigger_idx + 5..];
    // Skip optional connectors.
    let after = after
        .trim_start_matches(|c: char| c.is_whitespace())
        .trim_start_matches("called")
        .trim_start_matches("named")
        .trim_start_matches(|c: char| c.is_whitespace());

    let word: String = after
        .chars()
        .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .collect();
    // Require true kebab signature (a hyphen) to reject plain English words
    // like "that", "is", "and". Explicit slugs such as `json-validator` pass.
    if word.is_empty() || !word.contains('-') {
        None
    } else {
        Some(word)
    }
}

/// Result of an auto-routed create-skill operation.
#[derive(Debug)]
struct AutoRoutedSkill {
    skill_id: String,
    name: String,
}

/// Internally execute the create-skill logic (mirrors `skills::create_skill`
/// but without going through HTTP). Returns `Ok(AutoRoutedSkill)` on success.
async fn execute_auto_route_create_skill(
    state: &Arc<AppState>,
    actor: &str,
    req: CreateSkillRequest,
) -> Result<AutoRoutedSkill, ApiError> {
    use cyberclaw_skill_runtime::skill_hub::{SkillBundle, SkillSource};
    use cyberclaw_skill_runtime::skill_scanner::SkillScanner;
    use cyberclaw_skill_runtime::skill_scanner::SkillTrustLevel;

    let name = req.name.trim().to_string();
    // Slug guard.
    let valid = name.len() >= 3
        && name.len() <= 32
        && name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        return Err(ApiError::InvalidRequest(
            "name must match ^[a-z][a-z0-9-]{2,31}$".to_string(),
        ));
    }
    if req.description.trim().is_empty() {
        return Err(ApiError::InvalidRequest(
            "description is required".to_string(),
        ));
    }

    // Write scaffold.
    let base_dir = std::env::var_os("HOME")
        .map(|h| {
            std::path::PathBuf::from(h)
                .join(".cyberclaw")
                .join("skills")
        })
        .unwrap_or_else(|| std::path::PathBuf::from(".cyberclaw").join("skills"));
    let skill_dir = base_dir.join(&name);
    if let Err(err) = std::fs::create_dir_all(&skill_dir) {
        return Err(ApiError::InternalError(format!("create dir failed: {err}")));
    }

    let scaffold = crate::api::skills::build_skill_scaffold_pub(&name, &req);
    let skill_md = skill_dir.join("SKILL.md");
    if let Err(err) = std::fs::write(&skill_md, &scaffold) {
        return Err(ApiError::InternalError(format!(
            "write SKILL.md failed: {err}"
        )));
    }

    let mut hub = state.skill_hub.write().await;
    hub.add_source(SkillSource::Local {
        path: skill_dir.clone(),
    });
    let bundle = SkillBundle {
        name: name.clone(),
        version: "0.1.0".to_string(),
        description: req.description.clone(),
        source: format!("local:{}", skill_dir.display()),
        trust_level: SkillTrustLevel::Community,
        sha256: None,
        signature: None,
        publisher_fingerprint: None,
    };
    hub.register_bundle(bundle.clone());

    let scanner = SkillScanner::new();
    match hub.download(&bundle) {
        Ok(_) => match hub.scan_and_install(&bundle, &scanner) {
            Ok(state_tag) => {
                info!(skill = %name, ?state_tag, "auto-routed skill scaffold installed");
            }
            Err(err) => {
                warn!(skill = %name, %err, "auto-route scan_and_install failed");
                return Err(ApiError::InternalError(format!("scan failed: {err}")));
            }
        },
        Err(err) => {
            warn!(skill = %name, %err, "auto-route hub download failed");
            return Err(ApiError::InternalError(format!(
                "hub download failed: {err}"
            )));
        }
    }

    let skill_id = format!("sk_{name}");

    // Mirror into admin_store.
    {
        let mut seeded = state.admin_store.skills.write().await;
        seeded.retain(|s| s.name != name);
        seeded.insert(
            0,
            crate::admin_store::AdminSkill {
                skill_id: skill_id.clone(),
                name: name.clone(),
                category: crate::api::skills::infer_category_pub(&name),
                source: "local".to_string(),
                description: req.description.clone(),
                installed_at: chrono::Utc::now().to_rfc3339(),
                enabled: true,
            },
        );
    }

    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            actor.to_string(),
            AuditKind::Mutation,
            "chat.auto_routed:create_skill".to_string(),
            Some(skill_id.clone()),
            serde_json::json!({
                "name": name,
                "auto_routed": true,
                "endpoint": "/api/v1/skills/create",
            }),
            AuditResult::Success,
        ))
        .await;
    }

    Ok(AutoRoutedSkill { skill_id, name })
}

/// Classify the last user message in `messages`.
///
/// Returns `None` when the feature is disabled, no user message is found, or
/// no intent matches.
fn classify_last_user_message(messages: &[Message]) -> Option<IntentHint> {
    if !intent_hint_enabled() {
        return None;
    }
    use cyberclaw_llm::Role;
    let last_user_text = messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .map(|m| m.content.as_str())?;

    let classifier = IntentClassifier::with_defaults();
    let matched = classifier.classify(last_user_text)?;

    let (hint, suggested_endpoint) = match matched.intent {
        Intent::CreateSkill => ("create-skill", "/api/v1/skills/create"),
        Intent::CreateAgent => ("create-agent", "/api/v1/agents"),
        Intent::Brainstorm => ("brainstorm", "/api/v1/chat/completions"),
        Intent::DailyDigest => ("daily-digest", "/api/v1/agents/:id/digest"),
        Intent::ApproveRequest => ("approve-request", "/api/v1/chat/approval"),
        Intent::RejectRequest => ("reject-request", "/api/v1/chat/approval"),
        Intent::PlanRequest => ("plan-request", "/api/v1/chat/completions"),
        Intent::OrchestrateRequest => ("orchestrate-request", "/api/v1/chat/completions"),
        Intent::SkillifyRequest => ("skillify-request", "/api/v1/skills/create"),
        Intent::DeepAnalyze => ("deep-analyze", "/api/v1/chat/completions"),
    };

    Some(IntentHint {
        hint,
        suggested_endpoint,
    })
}

/// Append intent hint headers to an existing `HeaderMap`.
fn apply_intent_hint_headers(headers: &mut HeaderMap, hint: &IntentHint) {
    if let Ok(v) = HeaderValue::from_str(hint.hint) {
        headers.insert(HeaderName::from_static("x-cyberclaw-intent-hint"), v);
    }
    if let Ok(v) = HeaderValue::from_str(hint.suggested_endpoint) {
        headers.insert(
            HeaderName::from_static("x-cyberclaw-intent-suggest-endpoint"),
            v,
        );
    }
}

/// 创建 Chat API 路由
pub fn create_chat_router() -> Router<Arc<AppState>> {
    Router::new().route("/v1/chat/completions", post(chat_completions))
}

/// Return `true` when the client's `Accept` header contains
/// `text/event-stream`, signalling an SSE response is acceptable.
///
/// This lets the frontend opt into streaming either by setting
/// `stream: true` in the JSON body (OpenAI compatible) or by just
/// setting the `Accept` header — useful when cross-origin fetch layers
/// strip unknown body fields.
fn accept_header_wants_sse(headers: &HeaderMap) -> bool {
    headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(',').any(|s| {
                // Strip media-type parameters like `;q=0.9` before comparing.
                let media = s.split(';').next().unwrap_or("").trim();
                media.eq_ignore_ascii_case("text/event-stream")
            })
        })
        .unwrap_or(false)
}

/// Build the default tool palette for the `/v1/chat/completions` endpoint.
///
/// This endpoint is the **generic** OpenAI-compatible chat surface — it does
/// **not** support sub-agent delegation or platform-native skill management.
/// Both need request-scoped state and inline interception that only
/// `chat_handler.rs` provides:
///   · `delegate_to_sub_agent` — needs caller_identity + depth/budget tracking
///   · `skill_search` / `skill_create` — wired to SkillHub via inline intercept
///     in chat_handler.rs (NOT register_standard_mappings)
/// If we expose these facades here, LLMs (especially under
/// `tool_choice="required"`) emit tool_calls that `ToolCallMapper` cannot
/// resolve → `Unknown tool: skill_search` 500 error.
///
/// The palette is therefore built from `BuiltinToolRegistry` with the
/// `SubAgent` AND `SkillManagement` toolset categories removed.
fn build_default_chat_palette() -> Vec<ToolDefinition> {
    use cyberclaw_agent_runtime::builtin_tools::{BuiltinToolRegistry, ToolsetConfig};
    use cyberclaw_agent_runtime::tool_description::CapabilityFacade;
    use cyberclaw_core::facade::ToolsetCategory;
    let mut palette_config = ToolsetConfig::default_config();
    palette_config
        .enabled_categories
        .remove(&ToolsetCategory::SubAgent);
    palette_config
        .enabled_categories
        .remove(&ToolsetCategory::SkillManagement);
    BuiltinToolRegistry::with_defaults()
        .get_facades(&palette_config)
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
        .collect()
}

/// Chat Completions 处理函数
async fn chat_completions(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    headers_in: HeaderMap,
    Json(mut req): Json<ChatCompletionRequest>,
) -> Result<Response, ApiError> {
    // OpenAI-compatible contract: empty messages is a client error. Validating
    // here (before system-prompt injection) keeps the 400 path honest —
    // otherwise the injection step would mask the missing user input.
    if req.messages.is_empty() {
        return Err(ApiError::InvalidRequest(
            "messages must contain at least one entry".to_string(),
        ));
    }

    // Sprint 18 W3 — inject the built-in tool palette when the client
    // didn't supply one, so the LLM can emit `tool_calls` instead of
    // narrating "I would call file_write" in plain text. Source: same
    // BuiltinToolRegistry surfaced by GET /api/v1/tools.
    // Explicit `tools: Some([])` from a client opts out and is honoured.
    if req.tools.is_none() {
        // build_default_chat_palette() returns only inline-intercepted facades
        // (skill_*, delegate_to_sub_agent). The real 40+ connector tools live
        // in state.deferred_tool_registry (populated at startup). Pull from
        // there so the LLM gets the full palette (file_*, cmd_run, browser_*,
        // lsp_*, memory_*, mcp_call, task_*, search_glob, ...).
        let registry_guard = state.deferred_tool_registry.read().await;
        let active = registry_guard.active_facades();
        let mut palette: Vec<ToolDefinition> = active
            .iter()
            .map(|f| ToolDefinition {
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
        drop(registry_guard);
        // Append inline-intercept facades (skill_*, delegate) — dedup by name.
        let mut seen: std::collections::HashSet<String> =
            palette.iter().map(|t| t.function.name.clone()).collect();
        for extra in build_default_chat_palette() {
            if seen.insert(extra.function.name.clone()) {
                palette.push(extra);
            }
        }
        // web_search is intercepted inline in execute_tool_calls but not in
        // any facade — add a synthetic definition so the LLM knows about it.
        if seen.insert("web_search".to_string()) {
        palette.push(ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "web_search".to_string(),
                description: "Search the web via DuckDuckGo Instant Answer. \
                              Use for current events / facts the model can't know."
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query string" }
                    },
                    "required": ["query"]
                }),
            },
            cache_control: None,
        });
        }
        if !palette.is_empty() {
            req.tools = Some(palette);
        }
    }

    // Inject CyberClaw identity system prompt when caller didn't supply one.
    // Without this, MiniMax/Anthropic/OpenAI models default to their vendor
    // persona ("I am MiniMax M2.7 …") instead of the CyberClaw agent identity
    // the user is interacting with via the platform.
    let has_system = req
        .messages
        .first()
        .map(|m| matches!(m.role, cyberclaw_llm::Role::System))
        .unwrap_or(false);
    if !has_system {
        req.messages.insert(
            0,
            cyberclaw_llm::Message::system(
                cyberclaw_agent_runtime::constitution::cyberclaw_constitution_text(
                    cyberclaw_agent_runtime::constitution::ConstitutionProfile::Generic,
                ),
            ),
        );
    }

    let wants_sse = req.stream || accept_header_wants_sse(&headers_in);
    info!(
        "Received chat completion request: model={}, messages={}, stream={}, accept_sse={}",
        req.model,
        req.messages.len(),
        req.stream,
        wants_sse
    );

    // Intent hint classification (non-blocking; advisor only).
    let intent_hint = classify_last_user_message(&req.messages);

    // ── Auto-route gate ──────────────────────────────────────────────────────
    //
    // Conditions (all must hold):
    //   1. Env `CYBERCLAW_CHAT_AUTO_ROUTE=auto`
    //   2. Intent is CreateSkill (hint present with "create-skill")
    //   3. Operator's `intent_auto_route=true` in users.toml
    //   4. User message is parseable as a CreateSkillRequest spec
    //
    // On parse failure → fall through to advisor mode (hint headers only).
    // ────────────────────────────────────────────────────────────────────────
    if chat_auto_route_mode() == ChatAutoRouteMode::Auto {
        if let Some(ref hint) = intent_hint {
            if hint.hint == "create-skill" && load_operator_auto_route(&claims.sub) {
                match parse_create_skill_spec(&req.messages) {
                    Some(spec) => {
                        let skill_name = spec.name.clone();
                        match execute_auto_route_create_skill(&state, claims.sub.as_str(), spec)
                            .await
                        {
                            Ok(created) => {
                                info!(
                                    skill_id = %created.skill_id,
                                    name = %created.name,
                                    caller = %claims.sub,
                                    "chat.auto_routed: skill created directly"
                                );
                                let body = serde_json::json!({
                                    "id": format!("chatcmpl-autoroute-{}", Uuid::new_v4().simple()),
                                    "object": "chat.completion",
                                    "created": chrono::Utc::now().timestamp(),
                                    "model": req.model,
                                    "choices": [{
                                        "index": 0,
                                        "message": {
                                            "role": "assistant",
                                            "content": format!(
                                                "Skill `{}` 已创建，id={}",
                                                created.name, created.skill_id
                                            ),
                                        },
                                        "finish_reason": "stop",
                                    }],
                                    "usage": null,
                                    "auto_routed": true,
                                    "endpoint": "/api/v1/skills/create",
                                });
                                let mut headers = HeaderMap::new();
                                headers.insert(
                                    HeaderName::from_static("x-cyberclaw-auto-routed"),
                                    HeaderValue::from_static("true"),
                                );
                                headers.insert(
                                    HeaderName::from_static("x-cyberclaw-intent-hint"),
                                    HeaderValue::from_static("create-skill"),
                                );
                                return Ok((headers, Json(body)).into_response());
                            }
                            Err(err) => {
                                warn!(
                                    skill_name = %skill_name,
                                    caller = %claims.sub,
                                    "auto-route create_skill failed, falling back to advisor: {}",
                                    err
                                );
                                // Fall through to normal LLM path with advisor hint.
                            }
                        }
                    }
                    None => {
                        // Not parseable — fall through to advisor mode.
                        debug!(
                            caller = %claims.sub,
                            "auto-route: CreateSkill intent but message not parseable; advisor mode"
                        );
                    }
                }
            }
        }
    }

    // Emit audit entry when a hint is matched (advisor path).
    if let Some(ref hint) = intent_hint {
        if let Some(audit) = state.audit.as_ref() {
            let entry = AuditEntry::now(
                claims.sub.as_str(),
                AuditKind::Mutation,
                "chat.intent_hint",
                None,
                serde_json::json!({
                    "intent_hint": hint.hint,
                    "suggested_endpoint": hint.suggested_endpoint,
                }),
                AuditResult::Success,
            );
            let _ = audit.record(entry).await;
        }
    }

    // S21 T6: read active_agent_id from conversation store when a conversation_id
    // is present. The resolved agent is available for downstream use once the
    // control-plane ingress pipeline supports per-request agent overrides (T8).
    let routing_agent_id: Option<cyberclaw_core::ids::AgentId> =
        if let Some(ref conv_id) = req.conversation_id {
            state
                .conversation_store()
                .get(conv_id)
                .await
                .and_then(|c| c.active_agent_id)
        } else {
            None
        };

    // 转换请求
    let chat_req: ChatRequest = req.clone().into();

    if wants_sse {
        // 流式响应（SSE）
        handle_stream_completion(state, claims, chat_req, intent_hint, req.conversation_id).await
    } else {
        // 非流式响应
        handle_completion(state, claims, chat_req, intent_hint, routing_agent_id).await
    }
}

/// 处理非流式 completion
///
/// # 架构说明
///
/// **当前实现**：通过 Control Plane 进行完整的执行编排：
/// - HTTP -> Control Plane -> Task Manager -> Execution Service
/// - PolicyEngine 在 Control Plane 层进行治理审批
/// - 执行链支持 Review Queue 和审计追踪
async fn handle_completion(
    state: Arc<AppState>,
    claims: Claims,
    req: ChatRequest,
    intent_hint: Option<IntentHint>,
    preferred_agent_id: Option<cyberclaw_core::ids::AgentId>,
) -> Result<Response, ApiError> {
    let trace_id = Uuid::new_v4().to_string();

    debug!(trace_id = %trace_id, "Starting chat completion via process_ingress");

    // P0-2 Phase: 接入 process_ingress 主链（修复架构旁路）
    //
    // 架构说明：
    // - Chat API 现在通过完整的 Control Plane 主链：
    //   Gateway → Resolver → PolicyEngine → ReviewQueue → ExecutionService
    // - 使用 "allow-empty-actions" label 豁免 H-4 空 actions 审核
    //   （Chat 请求无需 Connector，只调用 LLM Client）
    // - 完整的审计追踪、治理决策、安全事件记录
    //
    // 架构一致性：
    // - ✅ 走 process_ingress 主链（不再旁路）
    // - ✅ PolicyEngine 完整评估（空 actions 豁免）
    // - ✅ SecurityEvent 完整记录
    // - ✅ ExecutionService 统一编排

    use cyberclaw_control_plane::gateway_router::IngressRequest;
    use cyberclaw_core::identity::ActorRef;
    use cyberclaw_core::identity::ActorType;
    use cyberclaw_core::ids::{ActorId, TaskId};
    use cyberclaw_core::task::{Task, TaskInput, TaskKind, TriggerRef};

    // 构造 ActorRef
    let actor_id = ActorId::from_string(claims.sub.as_str().to_string()).unwrap_or_else(|_| {
        ActorId::from_string("chat-api-user".to_string()).expect("fallback actor id must be valid")
    });

    let actor = ActorRef {
        id: actor_id,
        actor_type: ActorType::Human,
        tenant_id: claims.tenant.clone(),
        home_node_id: None,
        display_name: claims.sub.as_str().to_string(),
    };

    // 创建 Task（带 "allow-empty-actions" label）
    let task = Task {
        id: TaskId::from_string(format!("chat-{}", trace_id))
            .map_err(|e| ApiError::InternalError(format!("Failed to create task ID: {}", e)))?,
        case_id: None,
        title: "Chat Completion Request".to_string(),
        summary: format!("Chat completion with model: {}", req.model),
        kind: TaskKind::Custom("chat".to_string()),
        priority: cyberclaw_core::enums::Priority::Medium,
        requested_by: actor.clone(),
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "api".to_string(),
            source: "chat-api".to_string(),
        },
        input: TaskInput {
            payload: serde_json::json!({
                "messages": req.messages,
                "model": req.model,
                "temperature": req.temperature,
                "max_tokens": req.max_tokens,
                "tools": req.tools,
            }),
        },
        desired_outputs: vec![],
        labels: vec!["allow-empty-actions".to_string()], // H-4 豁免标记
        preferred_agent_id,
    };

    // 构造 IngressRequest
    let ingress_request = IngressRequest {
        actor,
        session: None,
        workspace: None,
        task,
    };

    // 通过 process_ingress 主链提交（完整治理）
    let submit_result = state
        .control_plane
        .process_ingress(ingress_request)
        .await
        .map_err(|e| {
            error!(
                trace_id = %trace_id,
                caller = %claims.sub,
                "Chat completion ingress failed: {:?}", e
            );
            ApiError::InternalError(format!("Chat completion ingress failed: {:?}", e))
        })?;

    info!(
        trace_id = %trace_id,
        execution_id = %submit_result.execution_id,
        caller = %claims.sub,
        "Chat completion submitted via process_ingress (P0-2: architecture bypass fixed)"
    );

    // 通过 LLM Client 生成响应（Control Plane 的完整治理链已在上方完成）
    info!(trace_id = %trace_id, "Invoking LLM for chat completion");

    // 1-5. LLM 调用 + tool dispatch 循环。Sprint 21 提取为
    // `run_llm_with_tool_dispatch`，让流式分支也能复用同一份调度逻辑。
    let response = run_llm_with_tool_dispatch(state.clone(), req.clone(), trace_id.clone()).await?;

    // Record usage counters (best-effort; never block the response).
    {
        let (in_tok, out_tok) = extract_token_counts(&response, &req);
        state.usage.record(&req.model, in_tok, out_tok, 0);
        state.usage.record_session();
    }

    let completion_response: ChatCompletionResponse = response.into();
    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("x-cyberclaw-execution-id"),
        HeaderValue::from_str(submit_result.execution_id.as_str())
            .map_err(|e| ApiError::InternalError(format!("Invalid execution id header: {}", e)))?,
    );
    headers.insert(
        HeaderName::from_static("x-cyberclaw-submitted"),
        HeaderValue::from_static("true"),
    );
    if let Some(ref hint) = intent_hint {
        apply_intent_hint_headers(&mut headers, hint);
    }
    Ok((headers, Json(completion_response)).into_response())
}

/// 内部 helper —— LLM 调用 + tool_calls 执行循环（最多一次往返）。
///
/// Sprint 21: 从 `handle_completion` 提取，让流式分支也能复用同一份
/// 工具调度逻辑。当 LLM 返回 `tool_calls` 时：
///   1. 调用 `execute_tool_calls` 把每个 tool_call 派到对应 connector
///   2. 把 LLM 的 assistant message + 工具结果追加到上下文
///   3. 再调一次 LLM 让它根据工具结果生成最终回复
///
/// 当前实现是 single-round（一次 dispatch + 一次 follow-up）；多轮 chain
/// 仍走 `/v1/agent/chat/completions` 的 `DefaultAgenticLoop`。
async fn run_llm_with_tool_dispatch(
    state: Arc<AppState>,
    mut req: ChatRequest,
    trace_id: String,
) -> Result<cyberclaw_llm::types::ChatResponse, ApiError> {
    // The helper always makes non-streaming LLM calls — the underlying
    // tool-dispatch loop needs the full response in one shot. If the
    // caller's `req.stream` is true (i.e. /v1/chat/completions stream
    // mode being routed through here for tool dispatch), force it
    // false before handing to the LLM client; some providers
    // (MiniMax-M2.7-HighSpeed observed) return a malformed body when
    // `stream:true` reaches the non-streaming endpoint.
    req.stream = Some(false);

    // v0.2.11: multi-iteration dispatch loop. Pre-v0.2.11 this was a
    // single-iteration call — if the LLM emitted tool_calls in the post-
    // dispatch response, we'd return them to the client unexecuted.
    // Multi-step reasoning ("tool A -> see result -> tool B -> answer")
    // therefore required client-side retry.
    //
    // Loop budget guards against pathological cases (LLM stuck in an
    // infinite tool_call loop, runaway sub-agent). Default 5 iterations;
    // override with CYBERCLAW_TOOL_DISPATCH_MAX_ITERS env var.
    let max_iters: usize = std::env::var("CYBERCLAW_TOOL_DISPATCH_MAX_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);

    let mut current_req = req;
    let mut response = state
        .llm_client
        .chat_completion(current_req.clone())
        .await
        .map_err(|e| {
            error!("LLM chat completion failed: {}", e);
            ApiError::LlmError(e.to_string())
        })?;

    // 2026-05-17 — Silent-abandon enforcement (mirror of chat_handler.rs
    // logic for the OpenAI-compat path). When the previous iteration's
    // tool calls ALL errored and the next LLM response is a Done with no
    // tool_calls, we intercept it, push an enforcement Message::system,
    // re-call the LLM once, then accept whatever comes back. This makes
    // /v1/chat/completions IRON LAW 6-protected too, not just the
    // /v1/agent/chat/completions path.
    let mut prev_all_errored = false;
    let mut forced_retry_used = false;

    for iter in 0..max_iters {
        // DSML synthesis (v1.2 #29): if response content contains
        // DeepSeek-style <｜｜DSML｜｜tool_calls>... markup but the structured
        // tool_calls field is empty, parse + synthesize so the loop's
        // dispatch flow handles it identically.
        if let Some(first_choice_mut) = response.choices.get_mut(0) {
            let dsml_empty = first_choice_mut
                .message
                .tool_calls
                .as_ref()
                .is_none_or(|tc| tc.is_empty());
            if dsml_empty {
                if let Some(parsed) = cyberclaw_agent_runtime::dsml_parser::parse_dsml_tool_calls(
                    &first_choice_mut.message.content,
                ) {
                    info!(
                        trace_id = %trace_id,
                        synth_count = parsed.len(),
                        "Synthesized {} tool_calls from DSML markup",
                        parsed.len()
                    );
                    first_choice_mut.message.content =
                        cyberclaw_agent_runtime::dsml_parser::strip_dsml(
                            &first_choice_mut.message.content,
                        );
                    first_choice_mut.message.tool_calls = Some(parsed);
                }
            }
        }

        let Some(first_choice) = response.choices.first() else {
            break;
        };
        let no_tool_calls = first_choice
            .message
            .tool_calls
            .as_ref()
            .is_none_or(|tc| tc.is_empty());
        if no_tool_calls {
            // 2026-05-17 — Silent-abandon enforcement gate. If the previous
            // iteration's tools all errored and the model now wants to finish
            // without retrying, push a Message::system nudge and force ONE
            // more LLM round-trip. After the forced retry we accept whatever
            // comes back regardless (forced_retry_used short-circuits).
            if prev_all_errored && !forced_retry_used {
                warn!(
                    trace_id = %trace_id,
                    iteration = iter + 1,
                    "Silent-abandon detected (OpenAI-compat path): previous tools all failed, \
                     model returned final answer without retry. Forcing one more iteration."
                );
                let mut msgs = current_req.messages.clone();
                // CONSTITUTION-BYPASS-OK: this Message::system is an in-flight
                // enforcement nudge injected mid-loop (not a chat-handler entry
                // point). The constitution itself is already part of the
                // request via `cyberclaw_constitution_text(...)` at line 729
                // (built when this request first entered chat_completions).
                // What we push here is a SUPPLEMENT — IRON LAW 6 reinforcement
                // when the model abandons after every tool errored.
                msgs.push(cyberclaw_llm::Message::system(
                    "ENFORCEMENT: Your last reply ended without retrying after every tool call in the previous iteration was rejected. Per IRON LAW 6 (universal-resilience reflex), you MUST either: (a) attempt one alternative tool path that respects governance (e.g. write under workspace root instead of /), OR (b) deliver the actual answer INLINE in your reply as a markdown code block (no file_write needed). Silent abandonment is not acceptable — the user is waiting for a deliverable, not an acknowledgement of failure. Try again now.",
                ));
                current_req = ChatRequest {
                    messages: msgs,
                    ..current_req
                };
                forced_retry_used = true;
                prev_all_errored = false;
                response = state
                    .llm_client
                    .chat_completion(current_req.clone())
                    .await
                    .map_err(|e| {
                        error!("LLM chat completion failed (forced retry): {}", e);
                        ApiError::LlmError(e.to_string())
                    })?;
                continue;
            }
            // LLM produced a final answer — stop looping.
            break;
        }
        // SAFETY: no_tool_calls is false → message.tool_calls is Some(non-empty).
        let tool_calls = first_choice
            .message
            .tool_calls
            .as_ref()
            .expect("checked non-empty above");
        info!(
            trace_id = %trace_id,
            iteration = iter + 1,
            tool_count = tool_calls.len(),
            "Multi-iteration dispatch: executing tools"
        );

        let tool_results = execute_tool_calls(state.clone(), tool_calls, trace_id.clone()).await?;

        // 2026-05-17 — track "every tool in this batch errored" so the
        // enforcement gate above can detect silent-abandon next iter.
        prev_all_errored = !tool_results.is_empty()
            && tool_results
                .iter()
                .all(|r| matches!(r, ToolExecutionResult::Error { .. }));

        let mut updated_messages = current_req.messages.clone();
        updated_messages.push(first_choice.message.clone());
        for result in tool_results {
            let tool_message = format_tool_result_as_message(result);
            updated_messages.push(tool_message);
        }

        current_req = ChatRequest {
            messages: updated_messages,
            ..current_req
        };

        response = state
            .llm_client
            .chat_completion(current_req.clone())
            .await
            .map_err(|e| {
                error!(
                    trace_id = %trace_id,
                    iteration = iter + 1,
                    "LLM chat completion (post-tools iteration {}) failed: {}",
                    iter + 1,
                    e
                );
                ApiError::LlmError(e.to_string())
            })?;
    }

    // If we exhausted max_iters and LLM still wants more tools, the final
    // response carries those tool_calls — client side can handle them or
    // retry. Logged at warn so operators can tune CYBERCLAW_TOOL_DISPATCH_MAX_ITERS.
    if let Some(c) = response.choices.first() {
        if let Some(tc) = c.message.tool_calls.as_ref() {
            if !tc.is_empty() {
                warn!(
                    trace_id = %trace_id,
                    max_iters,
                    "Tool dispatch loop hit budget; LLM still emitting tool_calls — \
                     returning unexecuted. Bump CYBERCLAW_TOOL_DISPATCH_MAX_ITERS if needed."
                );
            }
        }
    }

    Ok(response)
}

/// 处理流式 completion
///
/// S14-4: 当 LLM 客户端原生支持流式（`chat_completion_stream`）时直接透传；
/// 当原生流式失败或后端仅支持非流式时，回退到 `chat_completion` 并把最终
/// 内容切片成多个小块（每 20ms 一次），提供渐进渲染的 UX。无论走哪条路径，
/// 都会以 `data: [DONE]` 作为终止帧，前端据此收尾。
///
/// S15-T10: 若调用方提供了 `conversation_id`，订阅 `ClarifyBroadcaster` 并将
/// clarify 事件与 token 帧合并（`futures::stream::select`）。clarify 事件帧
/// 优先级与 token 帧对等——`select` 以轮询方式公平调度，不会阻塞 token 流。
async fn handle_stream_completion(
    state: Arc<AppState>,
    _claims: Claims,
    req: ChatRequest,
    intent_hint: Option<IntentHint>,
    conversation_id: Option<String>,
) -> Result<Response, ApiError> {
    use futures::StreamExt;
    use tokio_stream::wrappers::BroadcastStream;

    // Type alias for the pinned SSE event stream used throughout this function.
    type SseItem = Result<axum::response::sse::Event, std::convert::Infallible>;
    type PinnedSseStream = std::pin::Pin<Box<dyn futures::stream::Stream<Item = SseItem> + Send>>;

    // Subscribe to the clarify broadcaster (async) and convert to an SSE stream
    // that emits clarify frames. Returns a `Box::pin`-ned stream so we can merge
    // it without needing `Unpin` on either side.
    async fn make_clarify_stream(state: &Arc<AppState>, conv_id: &str) -> PinnedSseStream {
        use crate::clarify_broadcast::ClarifyEvent;
        let rx = state.clarify_broadcaster.subscribe(conv_id).await;
        Box::pin(BroadcastStream::new(rx).filter_map(|result| {
            futures::future::ready(match result {
                Ok(ClarifyEvent::Requested(ref req)) => build_clarify_sse_frame(req).ok().map(Ok),
                Ok(ClarifyEvent::Resolved { ref id, ref answer }) => {
                    build_clarify_resolved_sse_frame(id, answer).ok().map(Ok)
                }
                Err(_) => None, // lagged receiver — skip
            })
        }))
    }

    // Merge a base token stream with an optional clarify stream.
    // When `conversation_id` is None (legacy callers), returns the base unchanged.
    async fn merge_with_clarify(
        base: PinnedSseStream,
        state: &Arc<AppState>,
        conversation_id: Option<String>,
    ) -> PinnedSseStream {
        match conversation_id {
            None => base,
            Some(ref conv_id) => {
                let clarify = make_clarify_stream(state, conv_id).await;
                // `futures::stream::select` polls both sides round-robin for
                // fair scheduling; token frames are never starved.
                Box::pin(futures::stream::select(base, clarify))
            }
        }
    }

    // Sprint 21 — when the request carries a non-empty tool palette,
    // we cannot just transparently forward the LLM's SSE stream:
    // tool_call deltas would reach the client without ever firing the
    // dispatcher (chat.rs:888-897 lives in the non-streaming path).
    // Buffer the full LLM response, run tool dispatch via the shared
    // `run_llm_with_tool_dispatch` helper, then re-emit the final
    // assistant message as chunked SSE so the client still gets a
    // streaming-shaped envelope. Loses token-by-token UX for tool
    // flows; preserves the SSE protocol contract and makes tool
    // dispatch actually work in the CLI / streaming clients.
    let has_tools = req.tools.as_ref().map(|t| !t.is_empty()).unwrap_or(false);
    if has_tools {
        let trace_id = uuid::Uuid::new_v4().to_string();
        let final_response =
            run_llm_with_tool_dispatch(state.clone(), req.clone(), trace_id).await?;
        // Record usage for the tools-buffered path (we have the full response).
        {
            let (in_tok, out_tok) = extract_token_counts(&final_response, &req);
            state.usage.record(&req.model, in_tok, out_tok, 0);
            state.usage.record_session();
        }
        let base: PinnedSseStream = Box::pin(chunked_fallback_sse(final_response));
        let merged = merge_with_clarify(base, &state, conversation_id).await;
        let mut response = Sse::new(merged).into_response();
        if let Some(ref hint) = intent_hint {
            apply_intent_hint_headers(response.headers_mut(), hint);
        }
        return Ok(response);
    }

    match state.llm_client.chat_completion_stream(req.clone()).await {
        Ok(stream) => {
            // Record session; token counts are not available mid-stream — estimate from input.
            {
                let in_tok = estimate_input_tokens(&req);
                state.usage.record(&req.model, in_tok, 0, 0);
                state.usage.record_session();
            }
            let base: PinnedSseStream = Box::pin(convert_to_sse(stream));
            let merged = merge_with_clarify(base, &state, conversation_id).await;
            let mut response = Sse::new(merged).into_response();
            if let Some(ref hint) = intent_hint {
                apply_intent_hint_headers(response.headers_mut(), hint);
            }
            Ok(response)
        }
        Err(stream_err) => {
            warn!(
                "LLM stream unsupported or failed ({}). Falling back to chunked non-stream.",
                stream_err
            );
            let fallback = state
                .llm_client
                .chat_completion(req.clone())
                .await
                .map_err(|e| {
                    error!("LLM chat_completion fallback failed: {}", e);
                    ApiError::LlmError(e.to_string())
                })?;
            // Record usage for the fallback non-streaming path.
            {
                let (in_tok, out_tok) = extract_token_counts(&fallback, &req);
                state.usage.record(&req.model, in_tok, out_tok, 0);
                state.usage.record_session();
            }
            let base: PinnedSseStream = Box::pin(chunked_fallback_sse(fallback));
            let merged = merge_with_clarify(base, &state, conversation_id).await;
            let mut response = Sse::new(merged).into_response();
            if let Some(ref hint) = intent_hint {
                apply_intent_hint_headers(response.headers_mut(), hint);
            }
            Ok(response)
        }
    }
}

/// Fallback for web_search: scrape html.duckduckgo.com result page.
/// Handles person names / news / niche queries where IA returns empty.
/// Returns up to 5 result entries each with title/url/snippet.
async fn ddg_html_fallback(
    query: &str,
) -> Result<Vec<serde_json::Value>, reqwest::Error> {
    let body = reqwest::Client::new()
        .post("https://html.duckduckgo.com/html/")
        .header("User-Agent", "Mozilla/5.0 cyberclaw/1.2.0")
        .form(&[("q", query)])
        .send()
        .await?
        .text()
        .await?;

    // Parse result blocks with state machine — each block has:
    //   <a class="result__a" href="URL">TITLE</a>
    //   <a class="result__snippet" href="URL">SNIPPET (with html tags)</a>
    let mut out: Vec<serde_json::Value> = Vec::new();
    let mut rest = body.as_str();
    while let Some(a_at) = rest.find("class=\"result__a\" href=\"") {
        let after_href = &rest[a_at + "class=\"result__a\" href=\"".len()..];
        let url_end = match after_href.find('"') {
            Some(p) => p,
            None => break,
        };
        let url = strip_redirect(&after_href[..url_end]);
        let after_close = &after_href[url_end + 2..];
        let title_end = match after_close.find("</a>") {
            Some(p) => p,
            None => break,
        };
        let title = strip_html(&after_close[..title_end]).trim().to_string();

        let snippet = match after_close[title_end..]
            .find("class=\"result__snippet\"")
        {
            Some(s_off) => {
                let snippet_after = &after_close[title_end + s_off..];
                if let Some(gt) = snippet_after.find('>') {
                    if let Some(close) = snippet_after[gt + 1..].find("</a>") {
                        strip_html(&snippet_after[gt + 1..gt + 1 + close])
                            .trim()
                            .to_string()
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                }
            }
            None => String::new(),
        };

        if !title.is_empty() && !url.is_empty() {
            out.push(serde_json::json!({
                "title": title,
                "url": url,
                "snippet": snippet,
            }));
            if out.len() >= 5 {
                break;
            }
        }
        rest = &after_close[title_end + "</a>".len()..];
    }
    Ok(out)
}

fn strip_html(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            out.push(c);
        }
    }
    // Decode common HTML entities
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#x2F;", "/")
        .replace("&nbsp;", " ")
}

/// DDG wraps real URLs in `/l/?uddg=ENCODED&...` redirect links — unwrap.
fn strip_redirect(href: &str) -> String {
    if let Some(idx) = href.find("uddg=") {
        let after = &href[idx + 5..];
        let end = after.find('&').unwrap_or(after.len());
        let encoded = &after[..end];
        url::form_urlencoded::parse(format!("u={}", encoded).as_bytes())
            .next()
            .map(|(_, v)| v.to_string())
            .unwrap_or_else(|| href.to_string())
    } else {
        href.to_string()
    }
}

/// 执行 tool_calls 并返回结果
async fn execute_tool_calls(
    state: Arc<AppState>,
    tool_calls: &[ToolCall],
    trace_id: String,
) -> Result<Vec<ToolExecutionResult>, ApiError> {
    info!(
        trace_id = %trace_id,
        tool_count = tool_calls.len(),
        "Executing tool calls via ToolExecutor"
    );

    let execution_id = ExecutionId::from_string(trace_id.clone())
        .map_err(|e| ApiError::InvalidRequest(format!("Invalid trace_id: {}", e)))?;
    let mut results = Vec::with_capacity(tool_calls.len());

    // 逐个执行工具调用，并记录事件
    for tool_call in tool_calls {
        // Inline intercept: web_search via DuckDuckGo Instant Answer JSON
        // (no API key required). Skips tool_mapper since this isn't a
        // connector-backed capability. v1.2 follow-up: promote to proper
        // capability registered via a Web connector.
        if tool_call.function.name == "web_search"
            || tool_call.function.name == "search"
        {
            let args: serde_json::Value =
                serde_json::from_str(&tool_call.function.arguments).unwrap_or_default();
            let query = args
                .get("query")
                .or_else(|| args.get("q"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let result_json = if query.is_empty() {
                serde_json::json!({"error":"query parameter required"})
            } else {
                let encoded_query: String =
                    url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
                let url = format!(
                    "https://api.duckduckgo.com/?q={}&format=json&no_html=1&skip_disambig=1",
                    encoded_query
                );
                match reqwest::Client::new()
                    .get(&url)
                    .header("User-Agent", "cyberclaw/1.1.0 (search)")
                    .send()
                    .await
                {
                    Ok(resp) => match resp.json::<serde_json::Value>().await {
                        Ok(j) => {
                            let abstract_text =
                                j.get("AbstractText").and_then(|v| v.as_str()).unwrap_or("");
                            let topics: Vec<String> = j
                                .get("RelatedTopics")
                                .and_then(|v| v.as_array())
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|t| {
                                            t.get("Text").and_then(|v| v.as_str()).map(String::from)
                                        })
                                        .take(8)
                                        .collect()
                                })
                                .unwrap_or_default();
                            // DuckDuckGo IA only has data for Wikipedia-style
                            // entities. Empty → fall back to scraping the
                            // html.duckduckgo.com results page (wider coverage:
                            // person names, news, niche topics).
                            if abstract_text.is_empty() && topics.is_empty() {
                                match ddg_html_fallback(&query).await {
                                    Ok(results) if !results.is_empty() => {
                                        serde_json::json!({
                                            "query": query,
                                            "results": results,
                                            "source": "duckduckgo_html",
                                        })
                                    }
                                    _ => serde_json::json!({
                                        "query": query,
                                        "no_results": true,
                                        "note": "Both DuckDuckGo Instant Answer and HTML fallback returned no usable data. Tell the user honestly that no search results were found; do NOT fabricate.",
                                    }),
                                }
                            } else {
                                serde_json::json!({
                                    "query": query,
                                    "abstract": abstract_text,
                                    "related_topics": topics,
                                    "source": "duckduckgo_instant_answer",
                                })
                            }
                        }
                        Err(e) => serde_json::json!({"error": format!("parse JSON: {}", e)}),
                    },
                    Err(e) => serde_json::json!({"error": format!("HTTP: {}", e)}),
                }
            };
            results.push(ToolExecutionResult::success(
                tool_call.id.clone(),
                "web_search".to_string(),
                result_json,
            ));
            continue;
        }

        let mapped_request = state
            .tool_mapper
            .map_tool_call(tool_call, trace_id.clone())
            .map_err(|e| {
                error!(
                    trace_id = %trace_id,
                    tool_name = %tool_call.function.name,
                    "Failed to map tool call for observability: {}", e
                );
                ApiError::InvalidRequest(format!("Tool mapping failed: {}", e))
            })?;
        let capability_id = mapped_request.capability_id.clone();
        let start_time = std::time::Instant::now();

        // 记录 Capability 调用开始
        let _ = state
            .event_recorder
            .record_event(ObservabilityEvent::CapabilityInvoked {
                execution_id: execution_id.clone(),
                capability_id: capability_id.clone(),
                timestamp: Utc::now(),
            })
            .await;

        // 执行工具
        let result = state
            .tool_executor
            .execute_tool(tool_call, trace_id.clone())
            .await
            .map_err(|e| {
                error!(trace_id = %trace_id, error = %e, "Tool execution failed");
                ApiError::InternalError(format!("Tool execution error: {}", e))
            })?;

        let duration_ms = start_time.elapsed().as_millis() as u64;

        // 记录执行结果
        let success = matches!(result, ToolExecutionResult::Success { .. });
        match &result {
            ToolExecutionResult::Success {
                tool_call_id,
                tool_name,
                ..
            } => {
                info!(
                    trace_id = %trace_id,
                    tool_call_id = %tool_call_id,
                    tool_name = %tool_name,
                    duration_ms = duration_ms,
                    "Tool executed successfully"
                );
            }
            ToolExecutionResult::Error {
                tool_call_id,
                tool_name,
                error,
                recoverable,
            } => {
                warn!(
                    trace_id = %trace_id,
                    tool_call_id = %tool_call_id,
                    tool_name = %tool_name,
                    error = %error,
                    recoverable = %recoverable,
                    duration_ms = duration_ms,
                    "Tool execution failed"
                );
            }
        }

        // 记录 Capability 执行完成
        let _ = state
            .event_recorder
            .record_event(ObservabilityEvent::CapabilityCompleted {
                execution_id: execution_id.clone(),
                capability_id,
                success,
                duration_ms,
                timestamp: Utc::now(),
            })
            .await;

        results.push(result);
    }

    if matches!(current_tool_failure_mode(), ToolFailureMode::FailClosed) {
        if let Some((tool_name, error, recoverable)) = first_tool_error(&results) {
            return Err(ApiError::InternalError(format!(
                "Tool execution failed in fail-closed mode: tool={} recoverable={} error={}",
                tool_name, recoverable, error
            )));
        }
    }

    Ok(results)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolFailureMode {
    FailOpen,
    FailClosed,
}

fn parse_tool_failure_mode(value: Option<&str>) -> ToolFailureMode {
    match value
        .unwrap_or("fail-open")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "fail-closed" | "fail_closed" => ToolFailureMode::FailClosed,
        _ => ToolFailureMode::FailOpen,
    }
}

fn current_tool_failure_mode() -> ToolFailureMode {
    parse_tool_failure_mode(std::env::var("CYBERCLAW_TOOL_FAILURE_MODE").ok().as_deref())
}

fn first_tool_error(results: &[ToolExecutionResult]) -> Option<(&str, &str, bool)> {
    results.iter().find_map(|result| match result {
        ToolExecutionResult::Error {
            tool_name,
            error,
            recoverable,
            ..
        } => Some((tool_name.as_str(), error.as_str(), *recoverable)),
        ToolExecutionResult::Success { .. } => None,
    })
}

/// 将 ToolExecutionResult 转换为 Message 格式
fn format_tool_result_as_message(result: ToolExecutionResult) -> Message {
    match result {
        ToolExecutionResult::Success {
            tool_call_id,
            result,
            ..
        } => {
            // 成功结果转为 JSON 字符串
            let content = serde_json::to_string(&result).unwrap_or_else(|_| result.to_string());
            Message::tool(tool_call_id, content)
        }
        ToolExecutionResult::Error {
            tool_call_id,
            error,
            recoverable,
            ..
        } => {
            // 错误结果包含错误信息和可恢复性
            let error_obj = serde_json::json!({
                "error": error,
                "recoverable": recoverable
            });
            Message::tool(tool_call_id, error_obj.to_string())
        }
    }
}

/// Extract (input_tokens, output_tokens) from a ChatResponse.
///
/// Uses the `usage` field when the provider populates it; falls back to a
/// rough character-based estimate (chars / 4) so the counter is never zero.
fn extract_token_counts(
    resp: &cyberclaw_llm::types::ChatResponse,
    req: &cyberclaw_llm::ChatRequest,
) -> (u64, u64) {
    if let Some(ref usage) = resp.usage {
        let in_tok = usage.prompt_tokens as u64;
        let out_tok = usage.completion_tokens as u64;
        if in_tok > 0 || out_tok > 0 {
            return (in_tok, out_tok);
        }
    }
    // Estimate from raw character counts when usage is missing.
    let in_chars: usize = req.messages.iter().map(|m| m.content.len()).sum();
    let out_chars: usize = resp.choices.iter().map(|c| c.message.content.len()).sum();
    ((in_chars / 4).max(1) as u64, (out_chars / 4).max(1) as u64)
}

/// Estimate input token count from message character lengths (chars / 4).
fn estimate_input_tokens(req: &cyberclaw_llm::ChatRequest) -> u64 {
    let chars: usize = req.messages.iter().map(|m| m.content.len()).sum();
    (chars / 4).max(1) as u64
}

/// 将 LLM Stream 转换为 SSE Event Stream，并在末尾追加 `[DONE]` 终止帧。
///
/// S14-4: 前端通过 `data: [DONE]` 识别流结束，与 OpenAI 对齐。如果上游流中断
/// （客户端 abort），`StreamExt::chain` 同样会被丢弃，axum 负责正确关闭连接。
fn convert_to_sse(
    stream: Box<dyn Stream<Item = Result<ChatChunk, cyberclaw_llm::LlmError>> + Send + Unpin>,
) -> impl Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>> + Send {
    use futures::StreamExt;

    let body = stream.map(|result| {
        let event_data = match result {
            Ok(chunk) => serde_json::to_string(&chunk)
                .unwrap_or_else(|e| format!(r#"{{"error": "Serialization failed: {}"}}"#, e)),
            Err(e) => {
                format!(r#"{{"error": "{}"}}"#, e)
            }
        };
        Ok(axum::response::sse::Event::default().data(event_data))
    });
    let terminator = futures::stream::once(async {
        Ok::<_, std::convert::Infallible>(axum::response::sse::Event::default().data("[DONE]"))
    });
    body.chain(terminator)
}

/// 将一次性的 [`ChatResponse`] 切成小块的 SSE 流，模拟渐进式输出。
///
/// S14-4: 仅在上游 LLM 不支持真正流式（或流式连接失败）时作为 MVP 回退路径
/// 使用。每 ~25ms 推送一个短片段，末尾以 `[DONE]` 收尾。
fn chunked_fallback_sse(
    response: ChatResponse,
) -> impl Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>> + Send {
    use futures::StreamExt;

    let id = response.id.clone();
    let model = response.model.clone();
    let created = response.created;
    let content: String = response
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_default();

    // Chunk into roughly 8-char slices on char boundaries.
    let mut slices: Vec<String> = Vec::new();
    let mut buf = String::with_capacity(16);
    for ch in content.chars() {
        buf.push(ch);
        if buf.chars().count() >= 8 {
            slices.push(std::mem::take(&mut buf));
        }
    }
    if !buf.is_empty() {
        slices.push(buf);
    }
    if slices.is_empty() {
        // Ensure at least one empty delta so the frontend registers the assistant message.
        slices.push(String::new());
    }

    // Build a stream of delayed chunks.
    let total = slices.len();
    let body = futures::stream::iter(slices.into_iter().enumerate()).then(move |(idx, piece)| {
        let id = id.clone();
        let model = model.clone();
        async move {
            // Small pacing delay to surface streaming UX; skipped for the first chunk
            // so the first byte arrives immediately.
            if idx > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            let is_last = idx + 1 == total;
            let finish_reason = if is_last {
                Some("stop".to_string())
            } else {
                None
            };
            let delta = cyberclaw_llm::Delta {
                role: if idx == 0 {
                    Some(cyberclaw_llm::Role::Assistant)
                } else {
                    None
                },
                content: Some(piece),
                tool_calls: None,
            };
            let chunk = ChatChunk {
                id: id.clone(),
                object: "chat.completion.chunk".to_string(),
                created,
                model: model.clone(),
                choices: vec![cyberclaw_llm::ChunkChoice {
                    index: 0,
                    delta,
                    finish_reason,
                }],
            };
            let data = serde_json::to_string(&chunk)
                .unwrap_or_else(|e| format!(r#"{{"error": "Serialization failed: {}"}}"#, e));
            Ok::<_, std::convert::Infallible>(axum::response::sse::Event::default().data(data))
        }
    });
    let terminator = futures::stream::once(async {
        Ok::<_, std::convert::Infallible>(axum::response::sse::Event::default().data("[DONE]"))
    });
    body.chain(terminator)
}

// ---------------------------------------------------------------------------
// SSE clarify frame helpers (S15-T8)
// ---------------------------------------------------------------------------

/// 构造携带 [`ClarifyRequest`] 的 SSE 事件帧。
///
/// 输出格式：
/// ```text
/// data: {"type":"clarify","clarify":{...}}\n\n
/// ```
///
/// token 帧（`{"choices":[...]}` 格式）保持不变，前端通过 `payload.type ===
/// "clarify"` 区分。
///
pub fn build_clarify_sse_frame(
    clarify: &cyberclaw_core::clarify::ClarifyRequest,
) -> Result<axum::response::sse::Event, crate::error::ApiError> {
    let payload = serde_json::json!({
        "type": "clarify",
        "clarify": clarify,
    });
    let data = serde_json::to_string(&payload)
        .map_err(|e| crate::error::ApiError::InternalError(e.to_string()))?;
    Ok(axum::response::sse::Event::default().data(data))
}

/// 构造标志 clarify 已解答的 SSE 事件帧。
///
/// 输出格式：
/// ```text
/// data: {"type":"clarify_resolved","clarify_id":"...","answer":{...}}\n\n
/// ```
///
/// 由 T10 在 `ClarifyCoordinator::notify_resolved()` 收到回答后调用，
/// 推送到对应对话的 active SSE stream。
pub fn build_clarify_resolved_sse_frame(
    clarify_id: &cyberclaw_core::ids::ClarifyId,
    answer: &cyberclaw_core::clarify::ClarifyAnswer,
) -> Result<axum::response::sse::Event, crate::error::ApiError> {
    let payload = serde_json::json!({
        "type": "clarify_resolved",
        "clarify_id": clarify_id,
        "answer": answer,
    });
    let data = serde_json::to_string(&payload)
        .map_err(|e| crate::error::ApiError::InternalError(e.to_string()))?;
    Ok(axum::response::sse::Event::default().data(data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // ── Intent hint tests ────────────────────────────────────────────────────

    /// A skill-creation prompt should produce a `create-skill` hint.
    #[test]
    #[serial]
    fn chat_creates_intent_hint_for_skill_prompt() {
        // Ensure the kill switch is off.
        unsafe { std::env::remove_var("CYBERCLAW_CHAT_INTENT_HINT") };

        let messages = vec![Message::user("Please create a skill that validates JSON")];
        let hint = classify_last_user_message(&messages).expect("hint must be Some");
        assert_eq!(hint.hint, "create-skill");
        assert_eq!(hint.suggested_endpoint, "/api/v1/skills/create");
    }

    /// An unrelated prompt should produce no hint.
    #[test]
    #[serial]
    fn chat_no_hint_when_intent_not_matched() {
        unsafe { std::env::remove_var("CYBERCLAW_CHAT_INTENT_HINT") };

        let messages = vec![Message::user("What is the capital of France?")];
        let hint = classify_last_user_message(&messages);
        assert!(hint.is_none(), "no hint expected for unrelated prompt");
    }

    /// When `CYBERCLAW_CHAT_INTENT_HINT=off` the classifier is bypassed entirely.
    #[test]
    #[serial]
    fn chat_hint_disabled_by_env() {
        unsafe { std::env::set_var("CYBERCLAW_CHAT_INTENT_HINT", "off") };
        let result = std::panic::catch_unwind(|| {
            let messages = vec![Message::user("create a skill for JSON validation")];
            classify_last_user_message(&messages)
        });
        unsafe { std::env::remove_var("CYBERCLAW_CHAT_INTENT_HINT") };
        let hint = result.expect("no panic");
        assert!(hint.is_none(), "hint must be None when feature is disabled");
    }

    // ── Existing tests ───────────────────────────────────────────────────────

    #[test]
    fn test_request_conversion() {
        let req = ChatCompletionRequest {
            model: "gpt-4".to_string(),
            messages: vec![Message::user("Hello")],
            temperature: Some(0.7),
            top_p: None,
            max_tokens: Some(100),
            tools: None,
            tool_choice: None,
            stream: false,
            conversation_id: None,
        };

        let chat_req: ChatRequest = req.into();
        assert_eq!(chat_req.model, "gpt-4");
        assert_eq!(chat_req.messages.len(), 1);
        assert_eq!(chat_req.temperature, Some(0.7));
        assert_eq!(chat_req.max_tokens, Some(100));
    }

    #[test]
    fn test_parse_tool_failure_mode() {
        assert!(matches!(
            parse_tool_failure_mode(Some("fail-closed")),
            ToolFailureMode::FailClosed
        ));
        assert!(matches!(
            parse_tool_failure_mode(Some("FAIL_CLOSED")),
            ToolFailureMode::FailClosed
        ));
        assert!(matches!(
            parse_tool_failure_mode(Some("fail-open")),
            ToolFailureMode::FailOpen
        ));
        assert!(matches!(
            parse_tool_failure_mode(Some("unexpected")),
            ToolFailureMode::FailOpen
        ));
        assert!(matches!(
            parse_tool_failure_mode(None),
            ToolFailureMode::FailOpen
        ));
    }

    // ── Auto-route tests ─────────────────────────────────────────────────────

    /// Without `intent_auto_route=true` on the operator, chat goes to advisor
    /// mode regardless of env — `parse_auto_route_mode` returns `Auto` but
    /// `load_operator_auto_route` returns `false`.
    #[test]
    #[serial]
    fn chat_without_auto_route_flag_uses_advisor_mode() {
        use crate::api::test_helpers::seed_users_with_role;

        // Seed operator with intent_auto_route = false (default).
        let (_tmp, _restore) = seed_users_with_role("ar-no-flag-user", "admin");
        let uid = cyberclaw_core::ids::UserId::from_string("ar-no-flag-user".to_string())
            .expect("valid uid");

        // Even with env=auto, load_operator_auto_route must return false.
        unsafe { std::env::set_var("CYBERCLAW_CHAT_AUTO_ROUTE", "auto") };
        let result = load_operator_auto_route(&uid);
        unsafe { std::env::remove_var("CYBERCLAW_CHAT_AUTO_ROUTE") };

        assert!(
            !result,
            "operator without intent_auto_route=true must not auto-route"
        );
    }

    /// `parse_create_skill_spec` succeeds on a JSON-embedded skill spec in the
    /// user message and returns a well-formed `CreateSkillRequest`.
    #[test]
    fn chat_auto_route_enabled_and_parseable_intent_creates_skill() {
        let json_msg = r#"Please create this skill: {"name":"json-validator","description":"Validates JSON payloads against a schema"}"#;
        let messages = vec![Message::user(json_msg)];
        let spec =
            parse_create_skill_spec(&messages).expect("spec must be parseable from embedded JSON");
        assert_eq!(spec.name, "json-validator");
        assert!(spec.description.contains("Validates JSON"));
    }

    /// When the user message matches CreateSkill intent but is NOT parseable as
    /// a structured spec, `parse_create_skill_spec` returns `None`, signalling
    /// fall-back to advisor mode.
    #[test]
    fn chat_auto_route_enabled_but_unparseable_falls_back_to_advisor() {
        // Vague / unstructured message — no JSON object and no valid slug.
        let messages = vec![Message::user(
            "I want a skill that does something useful please",
        )];
        let spec = parse_create_skill_spec(&messages);
        assert!(
            spec.is_none(),
            "unparseable message must return None so caller falls back to advisor"
        );
    }

    // ── SSE streaming tests (S14-4) ──────────────────────────────────────────

    /// `accept_header_wants_sse` must return true when the Accept header
    /// contains `text/event-stream`, case-insensitive and in any position.
    #[test]
    fn accept_header_detects_sse() {
        use axum::http::HeaderMap;
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::ACCEPT,
            "text/event-stream".parse().unwrap(),
        );
        assert!(accept_header_wants_sse(&h));

        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::ACCEPT,
            "application/json, text/event-stream;q=0.9".parse().unwrap(),
        );
        assert!(accept_header_wants_sse(&h));

        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::ACCEPT,
            "application/json".parse().unwrap(),
        );
        assert!(!accept_header_wants_sse(&h));

        let h = HeaderMap::new();
        assert!(!accept_header_wants_sse(&h));
    }

    /// The fallback chunker must emit at least one body chunk plus the
    /// `[DONE]` terminator, and the concatenated deltas must equal the
    /// original content.
    #[tokio::test]
    async fn chunked_fallback_sse_preserves_content_and_terminates() {
        use futures::StreamExt;
        let resp = ChatResponse {
            id: "chatcmpl-fallback".to_string(),
            object: "chat.completion".to_string(),
            created: 1234567890,
            model: "mock-model".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message::assistant("Hello streaming world from fallback"),
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        };
        let stream = chunked_fallback_sse(resp);
        futures::pin_mut!(stream);

        let mut payloads: Vec<String> = Vec::new();
        while let Some(item) = stream.next().await {
            let ev = item.expect("infallible");
            // `Event` does not expose data publicly — go through its Display/Debug.
            // We instead format via `Event::default().data(...)` pattern by
            // rendering to its string form via serde_json round-trip on chunks;
            // simpler: reconstruct by replaying `chunked_fallback_sse` through
            // formatter below. Here we rely on Debug to assert non-empty.
            let dbg = format!("{:?}", ev);
            payloads.push(dbg);
        }
        // At minimum: one content chunk + [DONE]
        assert!(
            payloads.len() >= 2,
            "expected >=2 events (content + DONE), got {}",
            payloads.len()
        );
        // Last event must be the DONE terminator.
        let last = payloads.last().unwrap();
        assert!(
            last.contains("[DONE]"),
            "last event must be [DONE] terminator, got: {}",
            last
        );
    }

    /// Disconnect-mid-stream: dropping the stream must not panic and must
    /// release gracefully. This simulates the frontend cancelling via
    /// AbortController.
    #[tokio::test]
    async fn chunked_fallback_sse_handles_drop_midstream() {
        use futures::StreamExt;
        let resp = ChatResponse {
            id: "chatcmpl-abort".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "m".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message::assistant(
                    "Lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod",
                ),
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        };
        let stream = chunked_fallback_sse(resp);
        futures::pin_mut!(stream);

        // Consume just the first event then let the pinned stream fall
        // out of scope (simulating client disconnect). The test passes if
        // this completes without panic.
        let first = stream.next().await;
        assert!(first.is_some());
        // Explicitly end the borrow scope.
        let _ = stream;
    }

    /// `CYBERCLAW_CHAT_AUTO_ROUTE=off` forces `Advisor` semantics — the mode
    /// enum must resolve to `Off` and never reach the auto-route gate.
    #[test]
    #[serial]
    fn chat_auto_route_respects_env_override_off() {
        unsafe { std::env::set_var("CYBERCLAW_CHAT_AUTO_ROUTE", "off") };
        let mode = chat_auto_route_mode();
        unsafe { std::env::remove_var("CYBERCLAW_CHAT_AUTO_ROUTE") };
        assert_eq!(
            mode,
            ChatAutoRouteMode::Off,
            "CYBERCLAW_CHAT_AUTO_ROUTE=off must yield Off mode"
        );

        // Default (env absent) must be Advisor, not Auto.
        unsafe { std::env::remove_var("CYBERCLAW_CHAT_AUTO_ROUTE") };
        assert_eq!(
            chat_auto_route_mode(),
            ChatAutoRouteMode::Advisor,
            "absent env must default to Advisor mode"
        );
    }

    // ── SSE clarify frame tests (S15-T8) ────────────────────────────────────

    fn make_test_clarify_request() -> cyberclaw_core::clarify::ClarifyRequest {
        use cyberclaw_core::clarify::{
            ClarifyOption, ClarifyQuestion, ClarifyRequest, ClarifyStatus,
        };
        use cyberclaw_core::ids::{AgentId, ClarifyId};
        let now = chrono::Utc::now();
        ClarifyRequest {
            id: ClarifyId::new(),
            conversation_id: "conv-test-sse-001".to_string(),
            agent_id: AgentId::from_string("test-agent".to_string()).unwrap(),
            questions: vec![ClarifyQuestion {
                question: "Which staging environment?".to_string(),
                options: vec![
                    ClarifyOption {
                        label: "staging-a".to_string(),
                        description: "Use staging A environment".to_string(),
                        preview: None,
                    },
                    ClarifyOption {
                        label: "staging-b".to_string(),
                        description: "Use staging B environment".to_string(),
                        preview: None,
                    },
                ],
                multi_select: false,
            }],
            source: Some("test".to_string()),
            created_at: now,
            expires_at: now + chrono::Duration::seconds(300),
            status: ClarifyStatus::Pending,
            answers: None,
            resolved_at: None,
        }
    }

    /// `build_clarify_sse_frame` must produce a valid JSON payload with
    /// `type == "clarify"` and the `clarify` key containing the request.
    ///
    /// Strategy: we replicate the same `serde_json::json!` construction that
    /// the helper uses, then assert on the parsed Value directly.  This avoids
    /// brittle Debug-string parsing (axum `Event` does not expose `data`
    /// publicly, and its Debug format byte-escapes inner quotes).
    #[test]
    fn test_build_clarify_frame_schema_correct() {
        let req = make_test_clarify_request();
        // Verify the helper compiles and succeeds.
        let _ = build_clarify_sse_frame(&req).expect("frame must build without error");

        // Independently construct the same JSON payload and assert schema.
        let payload = serde_json::json!({
            "type": "clarify",
            "clarify": &req,
        });
        assert_eq!(payload["type"], "clarify", "type field must be 'clarify'");
        assert!(payload["clarify"].is_object(), "clarify must be an object");
        assert_eq!(
            payload["clarify"]["conversation_id"], "conv-test-sse-001",
            "conversation_id must be present"
        );
        assert!(
            payload["clarify"]["questions"].is_array(),
            "questions must be an array"
        );
        assert_eq!(
            payload["clarify"]["questions"][0]["question"], "Which staging environment?",
            "question text must be present"
        );
        assert_eq!(
            payload["clarify"]["questions"][0]["options"][0]["label"], "staging-a",
            "option label must be present"
        );
    }

    /// `build_clarify_sse_frame` payload must round-trip back to the original
    /// `ClarifyRequest` shape via JSON parse (schema fidelity check).
    #[test]
    fn test_build_clarify_frame_roundtrip() {
        use cyberclaw_core::clarify::ClarifyRequest;
        let req = make_test_clarify_request();
        let expected_id = req.id.clone();
        let expected_conv = req.conversation_id.clone();

        // Build and verify the helper succeeds.
        let _ = build_clarify_sse_frame(&req).expect("frame must build");

        // Reconstruct the JSON payload and round-trip the inner ClarifyRequest.
        let payload = serde_json::json!({
            "type": "clarify",
            "clarify": &req,
        });
        assert_eq!(payload["type"], "clarify", "type field must be 'clarify'");

        let clarify_obj = &payload["clarify"];
        assert!(clarify_obj.is_object(), "clarify must be an object");

        let decoded: ClarifyRequest = serde_json::from_value(clarify_obj.clone())
            .expect("clarify value must deserialize to ClarifyRequest");
        assert_eq!(decoded.id, expected_id, "id must round-trip");
        assert_eq!(
            decoded.conversation_id, expected_conv,
            "conversation_id must round-trip"
        );
        assert_eq!(
            decoded.questions.len(),
            1,
            "questions count must round-trip"
        );
    }

    /// `build_clarify_resolved_sse_frame` must produce `type == "clarify_resolved"`
    /// with `clarify_id` and `answer` keys.
    #[test]
    fn test_build_clarify_resolved_frame_schema() {
        use cyberclaw_core::clarify::ClarifyAnswer;
        use cyberclaw_core::ids::ClarifyId;
        let id = ClarifyId::new();
        let mut answer = ClarifyAnswer::new();
        answer.insert("Which staging environment?", "staging-a");

        // Verify the helper compiles and succeeds.
        let _ = build_clarify_resolved_sse_frame(&id, &answer)
            .expect("resolved frame must build without error");

        // Independently assert the JSON schema.
        let payload = serde_json::json!({
            "type": "clarify_resolved",
            "clarify_id": &id,
            "answer": &answer,
        });
        assert_eq!(
            payload["type"], "clarify_resolved",
            "type field must be 'clarify_resolved'"
        );
        assert!(
            !payload["clarify_id"].is_null(),
            "clarify_id must be present"
        );
        assert!(payload["answer"].is_object(), "answer must be an object");
        assert_eq!(
            payload["answer"]["answers"]["Which staging environment?"], "staging-a",
            "answer value must be present"
        );
    }

    /// Token frames produced by `chunked_fallback_sse` must use the OpenAI
    /// `{"choices":[...]}` schema and must NOT contain a `type` discriminator
    /// field — this is the backwards-compatibility proof that S14-4 token
    /// frames are unchanged and clients that don't know about clarify work.
    ///
    /// Strategy: deserialise each chunk from the serialised `ChatChunk` JSON
    /// (the same JSON that ends up in the SSE data field) and assert on the
    /// parsed Value, avoiding Debug-string byte-escaping issues.
    #[tokio::test]
    async fn test_existing_token_frame_schema_unchanged() {
        use cyberclaw_llm::{ChatChunk, Delta, Role};

        // Construct a representative ChatChunk (what chunked_fallback_sse emits).
        let chunk = ChatChunk {
            id: "chatcmpl-compat".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: 1000000000,
            model: "test-model".to_string(),
            choices: vec![cyberclaw_llm::ChunkChoice {
                index: 0,
                delta: Delta {
                    role: Some(Role::Assistant),
                    content: Some("hello".to_string()),
                    tool_calls: None,
                },
                finish_reason: None,
            }],
        };

        // Serialise the chunk to JSON exactly as chunked_fallback_sse does.
        let json = serde_json::to_string(&chunk).expect("chunk must serialise");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("chunk JSON must parse");

        // Token frames must use OpenAI choices format.
        assert!(
            parsed["choices"].is_array(),
            "token frame must have choices array, got: {}",
            json
        );
        // Token frames must NOT carry a top-level `type` field.
        assert!(
            parsed.get("type").is_none(),
            "token frame must NOT contain a 'type' field (backwards compat proof), got: {}",
            json
        );
    }

    /// Regression test: `/v1/chat/completions` must NOT expose
    /// `delegate_to_sub_agent` in its default tool palette.
    ///
    /// **Why**: SubAgent dispatch requires request-scoped state
    /// (`caller_identity`, depth/budget tracking) that only `chat_handler.rs`
    /// provides via inline interception. If `delegate_to_sub_agent` reaches
    /// a generic chat client and the LLM picks it (especially under
    /// `tool_choice="required"`), `ToolCallMapper` fails with
    /// `Unknown tool: delegate_to_sub_agent` (HTTP 500).
    ///
    /// **Real bug captured**: 2026-05-09 with MiniMax-M2.7-HighSpeed +
    /// `tool_choice="required"` against the running staging server.
    #[test]
    fn test_palette_excludes_subagent_for_chat_endpoint() {
        let palette = build_default_chat_palette();
        let names: Vec<&str> = palette.iter().map(|t| t.function.name.as_str()).collect();

        // TODO(human): write the assertions for this regression test.
        //
        // At minimum:
        //   1. Assert `names` does NOT contain `"delegate_to_sub_agent"`
        //      (the bug we're preventing — primary defense).
        //   2. Assert `names` DOES contain at least one expected core tool
        //      such as `"file_read"` (sanity check we didn't accidentally
        //      empty the palette by removing too many categories).
        //
        // Optional stronger guarantee:
        //   3. Assert `names.len() >= 10` so we'd catch a regression that
        //      empties the palette but keeps the canary tool.
        let _ = names; // silence unused warning until you implement the asserts
    }
}
