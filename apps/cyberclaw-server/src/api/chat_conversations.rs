//! Chat conversation store — Sprint 14 Story S14-3.
//!
//! Replaces the admin-console `localStorage` blob (`cyberclaw.admin.chat.
//! conversations`) with a server-backed CRUD surface so chat history is
//! visible to the governance / audit lane.
//!
//! # Routes (all require JWT; write routes additionally require admin)
//!
//! | Method | Path                                                  |
//! |--------|-------------------------------------------------------|
//! | GET    | `/api/v1/chat/conversations`                          |
//! | POST   | `/api/v1/chat/conversations`                          |
//! | GET    | `/api/v1/chat/conversations/:id`                      |
//! | PATCH  | `/api/v1/chat/conversations/:id`                      |
//! | DELETE | `/api/v1/chat/conversations/:id`                      |
//! | POST   | `/api/v1/chat/conversations/:id/messages`             |
//!
//! # Storage
//!
//! In-memory `HashMap<ConvId, Conversation>` wrapped in a `RwLock`; each
//! write persists the snapshot to `$HOME/.cyberclaw/conversations.json`
//! (override with `CYBERCLAW_CONVERSATIONS_PATH`). A real DB-backed store
//! is deferred.
//!
//! # RBAC
//!
//! - Viewer: only sees / mutates conversations where
//!   `owner_user_id == caller.user_id`.
//! - Admin: sees all conversations by default; mutation on somebody
//!   else's conversation requires the caller to pass `?force_admin=true`
//!   and is logged under a distinct audit action (`*.admin_override`).
//!
//! # Audit
//!
//! `ChatConversationCreated` / `…Renamed` / `…Deleted` actions are
//! emitted through the existing `AuditSink`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use axum::{
    extract::{Extension, Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;

use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::{require_admin, Claims};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A single chat message as recorded in a conversation.
///
/// The field set is intentionally loose so the admin UI can round-trip
/// its richer envelope (tool calls, attachments, timestamps) without
/// this module caring. `role` and `content` are the only required keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub ts: Option<i64>,
}

impl ChatMessage {
    /// Build a `role="handoff"` message for a resolved (Authorized/Accepted)
    /// [`HandoffRequest`]. Used by `execution_service::complete_handoff` to
    /// append a card into the conversation so the user sees the transfer.
    ///
    /// The `content` carries a short human-readable summary (used as fallback
    /// when JSX is unavailable); the `metadata` carries the structured payload
    /// the front-end `<HandoffCard>` JSX consumes.
    ///
    /// # Frontend contract
    /// - `role == "handoff"` → `pages_chat.jsx` routes to `<HandoffCard>` (S21 T11-T12)
    /// - `metadata.handoff_id / from_agent_id / to_agent_id / reason /
    ///    briefing_preview / decided_at` — card render fields
    /// - `content` — plain-text fallback for non-JSX consumers (e.g. log export)
    pub fn handoff_card(req: &cyberclaw_core::handoff::HandoffRequest) -> Self {
        // Preview = first 200 chars of briefing (full text lives in HandoffQueue;
        // chat message keeps payload small so listing 100 convs stays snappy)
        let briefing_preview: String = req.briefing_text.chars().take(200).collect();
        let briefing_truncated = req.briefing_text.chars().count() > 200;

        let content = format!(
            "🔀 Handoff: {} → {}\nReason: {}",
            req.from_agent_id, req.to_agent_id, req.reason
        );

        let metadata = serde_json::json!({
            "handoff_id": req.handoff_id.to_string(),
            "from_agent_id": req.from_agent_id.to_string(),
            "to_agent_id": req.to_agent_id.to_string(),
            "reason": req.reason,
            "briefing_preview": briefing_preview,
            "briefing_truncated": briefing_truncated,
            "initiated_at": req.initiated_at,
            "decided_at": req.decided_at,
        });

        Self {
            role: "handoff".to_string(),
            content,
            metadata: Some(metadata),
            ts: Some(req.initiated_at.timestamp()),
        }
    }
}

/// A chat conversation owned by a single operator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub owner_user_id: String,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<DateTime<Utc>>,
    /// Timestamp of the last auto- or manual-compress. Used by the auto-compress
    /// cooldown check to avoid re-compressing within a short window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_compressed_at: Option<DateTime<Utc>>,
    /// Currently active agent for this conversation. `None` = use default
    /// agent routing. Set by the HandoffRequest accept flow (S21 T6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_agent_id: Option<cyberclaw_core::ids::AgentId>,
}

/// In-memory store for conversations with JSON-file persistence.
#[derive(Debug, Default)]
pub struct ConversationStore {
    inner: RwLock<HashMap<String, Conversation>>,
    persist_path: Option<PathBuf>,
}

impl ConversationStore {
    /// Build a store persisting to the default on-disk location.
    pub fn new_default() -> Self {
        let path = default_persist_path();
        let mut map = HashMap::new();
        if let Some(p) = path.as_ref() {
            if let Ok(bytes) = std::fs::read(p) {
                if let Ok(list) = serde_json::from_slice::<Vec<Conversation>>(&bytes) {
                    for conv in list {
                        map.insert(conv.id.clone(), conv);
                    }
                    info!(path = %p.display(), count = map.len(), "chat_conversations: loaded from disk");
                }
            }
        }
        Self {
            inner: RwLock::new(map),
            persist_path: path,
        }
    }

    /// Build an in-memory-only store (test use).
    pub fn new_in_memory() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            persist_path: None,
        }
    }

    /// Look up a conversation by id. Returns `None` when not found or soft-deleted.
    pub async fn get(&self, id: &str) -> Option<Conversation> {
        let map = self.inner.read().await;
        map.get(id).filter(|c| c.deleted_at.is_none()).cloned()
    }

    /// Append a message to a conversation without going through the HTTP layer.
    ///
    /// Intended for internal callers (e.g. `AppState::ask_user_clarify`,
    /// `submit_clarify_respond_handler`) that need to persist clarify lifecycle
    /// messages to conversation history. Returns `Ok(())` when the conversation
    /// exists and is not deleted; returns an error string otherwise.
    ///
    /// Callers should use `.ok()` to swallow errors so persistence failures
    /// never block the primary clarify flow.
    pub async fn append_message_internal(
        &self,
        conv_id: &str,
        msg: ChatMessage,
    ) -> Result<(), String> {
        let mut guard = self.inner.write().await;
        let conv = guard
            .get_mut(conv_id)
            .ok_or_else(|| format!("conversation {} not found", conv_id))?;
        if conv.deleted_at.is_some() {
            return Err(format!("conversation {} is deleted", conv_id));
        }
        let mut stored = msg;
        if stored.ts.is_none() {
            stored.ts = Some(chrono::Utc::now().timestamp_millis());
        }
        conv.messages.push(stored);
        conv.updated_at = chrono::Utc::now();
        drop(guard);
        self.persist_snapshot().await;
        Ok(())
    }

    /// Replace the entire message list of a conversation.
    ///
    /// Loads the conversation, swaps its `messages` field with `new_messages`,
    /// updates `updated_at`, persists the snapshot, and returns the updated
    /// conversation. Returns an error string if the conversation is not found
    /// or has been soft-deleted.
    ///
    /// This is the write-back path used by the compress handler; it is
    /// only called after successful compression so transactionality is
    /// preserved at the caller level.
    pub async fn replace_messages(
        &self,
        conversation_id: &str,
        new_messages: Vec<ChatMessage>,
    ) -> Result<Conversation, String> {
        let out;
        {
            let mut guard = self.inner.write().await;
            let conv = guard
                .get_mut(conversation_id)
                .ok_or_else(|| format!("conversation {} not found", conversation_id))?;
            if conv.deleted_at.is_some() {
                return Err(format!("conversation {} is deleted", conversation_id));
            }
            conv.messages = new_messages;
            conv.updated_at = chrono::Utc::now();
            out = conv.clone();
        }
        self.persist_snapshot().await;
        Ok(out)
    }

    /// Mark a conversation as having been compressed at `now`.
    ///
    /// Called by `compress_conversation_internal` after a successful write-back
    /// so the auto-compress cooldown check can see the timestamp.
    pub async fn touch_last_compressed_at(&self, conversation_id: &str) {
        let mut guard = self.inner.write().await;
        if let Some(conv) = guard.get_mut(conversation_id) {
            conv.last_compressed_at = Some(chrono::Utc::now());
        }
        drop(guard);
        self.persist_snapshot().await;
    }

    /// Set (or replace) the active agent for a conversation.
    ///
    /// Called by the handoff accept flow (S21 T6) after admin approval so
    /// the chat dispatcher knows which agent to route subsequent completions
    /// to. Returns an error string if the conversation is not found or is
    /// soft-deleted.
    pub async fn set_active_agent(
        &self,
        conv_id: &str,
        agent_id: cyberclaw_core::ids::AgentId,
    ) -> Result<(), String> {
        {
            let mut guard = self.inner.write().await;
            let conv = guard
                .get_mut(conv_id)
                .ok_or_else(|| "conversation not found".to_string())?;
            if conv.deleted_at.is_some() {
                return Err("conversation deleted".to_string());
            }
            conv.active_agent_id = Some(agent_id);
            conv.updated_at = chrono::Utc::now();
        }
        self.persist_snapshot().await;
        Ok(())
    }

    /// Return a snapshot of all non-deleted conversations, newest-first.
    ///
    /// Used by the sessions aggregate endpoint (`GET /api/v1/sessions`).
    pub async fn list_all(&self) -> Vec<Conversation> {
        let map = self.inner.read().await;
        let mut out: Vec<Conversation> = map
            .values()
            .filter(|c| c.deleted_at.is_none())
            .cloned()
            .collect();
        out.sort_by_key(|b| std::cmp::Reverse(b.updated_at));
        out
    }

    /// Insert a minimal test conversation. Only compiled in test builds.
    #[cfg(test)]
    pub async fn create_for_test(&self, id: &str, owner_user_id: &str) {
        let now = Utc::now();
        let conv = Conversation {
            id: id.to_string(),
            title: format!("Test conversation {}", id),
            owner_user_id: owner_user_id.to_string(),
            messages: vec![],
            created_at: now,
            updated_at: now,
            deleted_at: None,
            last_compressed_at: None,
            active_agent_id: None,
        };
        let mut map = self.inner.write().await;
        map.insert(id.to_string(), conv);
    }

    async fn persist_snapshot(&self) {
        let Some(path) = self.persist_path.as_ref() else {
            return;
        };
        let map = self.inner.read().await;
        let list: Vec<&Conversation> = map.values().collect();
        let bytes = match serde_json::to_vec_pretty(&list) {
            Ok(b) => b,
            Err(err) => {
                warn!(%err, "chat_conversations: serialize snapshot failed");
                return;
            }
        };
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                if let Err(err) = std::fs::create_dir_all(parent) {
                    warn!(path = %parent.display(), %err, "chat_conversations: mkdir failed");
                    return;
                }
            }
        }
        if let Err(err) = std::fs::write(path, bytes) {
            warn!(path = %path.display(), %err, "chat_conversations: write snapshot failed");
            return;
        }
        // chmod 600 — conversations.json contains chat history
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
}

fn default_persist_path() -> Option<PathBuf> {
    if let Ok(v) = std::env::var("CYBERCLAW_CONVERSATIONS_PATH") {
        return Some(PathBuf::from(v));
    }
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".cyberclaw")
            .join("conversations.json"),
    )
}

/// Process-wide singleton store. Lazily constructed so tests that mutate
/// `$HOME` / `CYBERCLAW_CONVERSATIONS_PATH` before first use still pick
/// up their tempdir path.
///
/// Tests that need isolation should call [`reset_store_for_tests`].
static STORE: OnceLock<Arc<ConversationStore>> = OnceLock::new();

/// Handle to the in-process conversation store.
pub fn store() -> Arc<ConversationStore> {
    STORE
        .get_or_init(|| Arc::new(ConversationStore::new_default()))
        .clone()
}

/// Test helper: replace the store contents with an empty in-memory map
/// and skip disk persistence for the remainder of the process. Idempotent.
#[cfg(test)]
pub async fn reset_store_for_tests() {
    let s = store();
    let mut guard = s.inner.write().await;
    guard.clear();
}

// ---------------------------------------------------------------------------
// Request / response DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct CreateConversationRequest {
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateConversationResponse {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct RenameConversationRequest {
    pub title: String,
}

#[derive(Debug, Serialize)]
pub struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub owner_user_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub message_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<DateTime<Utc>>,
}

impl From<&Conversation> for ConversationSummary {
    fn from(c: &Conversation) -> Self {
        Self {
            id: c.id.clone(),
            title: c.title.clone(),
            owner_user_id: c.owner_user_id.clone(),
            created_at: c.created_at,
            updated_at: c.updated_at,
            message_count: c.messages.len(),
            deleted_at: c.deleted_at,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ListConversationsResponse {
    pub conversations: Vec<ConversationSummary>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ListQuery {
    #[serde(default)]
    pub include_deleted: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct AdminOverrideQuery {
    #[serde(default)]
    pub force_admin: Option<bool>,
}

// ---------------------------------------------------------------------------
// POST /api/v1/chat/message — flat stateless chat endpoint for admin SPA
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SendChatMessageRequest {
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Optional profile id whose `system_prompt` is prepended to the message
    /// list and whose `default_model` overrides the request model when the
    /// caller did not specify one. Profile lookup is owner-scoped: a profile
    /// owned by another user is ignored. See `profiles_store::ProfileStore`.
    #[serde(default)]
    pub profile_id: Option<String>,
    pub messages: Vec<SendChatMessageInput>,
}

#[derive(Debug, Deserialize)]
pub struct SendChatMessageInput {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct SendChatMessageResponse {
    pub message: SendChatMessageOutput,
}

#[derive(Debug, Serialize)]
pub struct SendChatMessageOutput {
    pub role: String,
    pub content: String,
    /// v1.1: heuristic confidence score on the response. Range [0.0, 1.0].
    /// Computed in [`compute_confidence_score`] — higher when the LLM finished
    /// cleanly (stop), used tools, and produced non-trivial content; lower
    /// when truncated, refused, or empty. Surfaced for operators / clients
    /// that want to gate on response quality.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence_score: Option<f32>,
}

/// v1.1 — Heuristic confidence score for a chat response. Range [0.0, 1.0].
///
/// Inputs:
/// - `content`: the assistant message content
/// - `finish_reason`: LLM stop reason (None / Some("stop") / "length" / …)
/// - `tool_calls_used`: did the LLM emit/use any tool_calls?
///
/// Heuristic v1 — calibration TBD with real user feedback in v1.2:
/// - base 0.50
/// - +0.20 if finish_reason == "stop"
/// - -0.20 if finish_reason == "length" (truncated)
/// - +0.15 if tool_calls_used (LLM acted, didn't just narrate)
/// - +0.10 if content has ≥80 chars of substance (post think-strip)
/// - -0.30 if content is empty AND no tool_calls (silent failure)
fn compute_confidence_score(
    content: &str,
    finish_reason: Option<&str>,
    tool_calls_used: bool,
) -> f32 {
    // Strip <think>...</think> blocks for substance check. Simple regex-free
    // walk that's UTF-8 safe (the previous byte-slicing version panicked on
    // Chinese punctuation 。 / multi-byte chars).
    let visible: String = {
        let mut out = String::with_capacity(content.len());
        let mut rest = content;
        while !rest.is_empty() {
            match rest.find("<think>") {
                Some(start) => {
                    out.push_str(&rest[..start]);
                    let after_open = &rest[start + "<think>".len()..];
                    match after_open.find("</think>") {
                        Some(end) => {
                            rest = &after_open[end + "</think>".len()..];
                        }
                        None => {
                            // Unclosed think block — bail to avoid infinite loop.
                            break;
                        }
                    }
                }
                None => {
                    out.push_str(rest);
                    break;
                }
            }
        }
        out.trim().to_string()
    };

    let mut score: f32 = 0.50;
    match finish_reason {
        Some("stop") => score += 0.20,
        Some("length") => score -= 0.20,
        _ => {}
    }
    if tool_calls_used {
        score += 0.15;
    }
    if visible.chars().count() >= 80 {
        score += 0.10;
    }
    if visible.is_empty() && !tool_calls_used {
        score -= 0.30;
    }
    score.clamp(0.0, 1.0)
}

#[cfg(test)]
mod confidence_tests {
    use super::compute_confidence_score;

    #[test]
    fn high_when_stop_and_substantial() {
        let c = compute_confidence_score(&"A".repeat(200), Some("stop"), false);
        assert!(c > 0.7, "expected high confidence, got {c}");
    }

    #[test]
    fn low_when_truncated() {
        let c = compute_confidence_score("half answer cut off", Some("length"), false);
        assert!(c < 0.5, "truncated must be lower-than-base, got {c}");
    }

    #[test]
    fn very_low_when_silent_failure() {
        let c = compute_confidence_score("", None, false);
        assert!(c < 0.30, "silent failure must be very low, got {c}");
    }

    #[test]
    fn tool_use_boosts_score() {
        let with_tool = compute_confidence_score("ok", Some("stop"), true);
        let no_tool = compute_confidence_score("ok", Some("stop"), false);
        assert!(with_tool > no_tool);
    }

    #[test]
    fn no_panic_on_chinese_punctuation() {
        // Regression: previous byte-slicing impl panicked on `。` (3-byte
        // UTF-8) when buf.len() landed mid-char.
        let content = "**无法完成此任务。**\n\n经过搜索，未匹配到相关 Skill。";
        let _c = compute_confidence_score(content, Some("stop"), false);
    }

    #[test]
    fn think_block_stripped_for_substance() {
        let with_think = format!("<think>{}</think>short", "x".repeat(500));
        let c = compute_confidence_score(&with_think, Some("stop"), false);
        // 'short' is <80 chars after strip → no +0.10 boost from substance
        let pure = compute_confidence_score("short", Some("stop"), false);
        assert!((c - pure).abs() < 0.01);
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the `/api/v1/chat/conversations/*` router. Mount inside the
/// protected-routes lane so every handler sees a `Claims` extension.
pub fn create_chat_conversations_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/chat/conversations",
            get(list_conversations).post(create_conversation),
        )
        .route(
            "/api/v1/chat/conversations/:id",
            get(get_conversation)
                .patch(rename_conversation)
                .delete(delete_conversation),
        )
        .route(
            "/api/v1/chat/conversations/:id/messages",
            post(append_message),
        )
        .route("/api/v1/chat/message", post(send_chat_message))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn caller_id(claims: &Claims) -> String {
    claims.sub.as_ref().to_string()
}

async fn is_admin(claims: &Claims) -> bool {
    require_admin(claims).await.is_ok()
}

async fn record_audit(
    state: &AppState,
    actor: String,
    action: &str,
    target: Option<String>,
    detail: serde_json::Value,
    result: AuditResult,
) {
    let Some(sink) = state.audit.as_ref() else {
        return;
    };
    sink.record(AuditEntry::now(
        actor,
        AuditKind::Mutation,
        action.to_string(),
        target,
        detail,
        result,
    ))
    .await;
}

fn gen_id() -> String {
    format!("conv_{}", Uuid::new_v4().simple())
}

fn clamp_title(raw: Option<String>) -> String {
    let t = raw.unwrap_or_else(|| "New conversation".to_string());
    let trimmed = t.trim();
    if trimmed.is_empty() {
        "New conversation".to_string()
    } else {
        trimmed.chars().take(200).collect()
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn list_conversations(
    State(_state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<ListQuery>,
) -> Result<Json<ListConversationsResponse>, ApiError> {
    let store = store();
    let guard = store.inner.read().await;
    let caller = caller_id(&claims);
    let admin = is_admin(&claims).await;
    let include_deleted = q.include_deleted.unwrap_or(false);

    let mut out: Vec<ConversationSummary> = guard
        .values()
        .filter(|c| admin || c.owner_user_id == caller)
        .filter(|c| include_deleted || c.deleted_at.is_none())
        .map(ConversationSummary::from)
        .collect();
    // Stable newest-first ordering.
    out.sort_by_key(|b| std::cmp::Reverse(b.updated_at));
    Ok(Json(ListConversationsResponse { conversations: out }))
}

async fn create_conversation(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<CreateConversationRequest>,
) -> Result<Json<CreateConversationResponse>, ApiError> {
    let now = Utc::now();
    let id = gen_id();
    let title = clamp_title(req.title);
    let caller = caller_id(&claims);

    let conv = Conversation {
        id: id.clone(),
        title: title.clone(),
        owner_user_id: caller.clone(),
        messages: Vec::new(),
        created_at: now,
        updated_at: now,
        deleted_at: None,
        last_compressed_at: None,
        active_agent_id: None,
    };

    let store = store();
    {
        let mut guard = store.inner.write().await;
        guard.insert(id.clone(), conv);
    }
    store.persist_snapshot().await;

    record_audit(
        state.as_ref(),
        caller,
        "chat.conversation.created",
        Some(format!("conversation:{}", id)),
        serde_json::json!({ "title": title }),
        AuditResult::Success,
    )
    .await;

    Ok(Json(CreateConversationResponse {
        id,
        title,
        created_at: now,
    }))
}

async fn get_conversation(
    State(_state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<String>,
) -> Result<Json<Conversation>, ApiError> {
    let store = store();
    let guard = store.inner.read().await;
    let conv = guard
        .get(&id)
        .ok_or_else(|| ApiError::NotFound(format!("conversation {} not found", id)))?;
    let caller = caller_id(&claims);
    if conv.owner_user_id != caller && !is_admin(&claims).await {
        return Err(ApiError::Forbidden(
            "not the owner of this conversation".to_string(),
        ));
    }
    Ok(Json(conv.clone()))
}

async fn rename_conversation(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<String>,
    Query(q): Query<AdminOverrideQuery>,
    Json(req): Json<RenameConversationRequest>,
) -> Result<Json<ConversationSummary>, ApiError> {
    let new_title = clamp_title(Some(req.title));
    let caller = caller_id(&claims);
    let admin = is_admin(&claims).await;
    let force_admin = q.force_admin.unwrap_or(false);

    let summary;
    let is_override;
    {
        let store = store();
        let mut guard = store.inner.write().await;
        let conv = guard
            .get_mut(&id)
            .ok_or_else(|| ApiError::NotFound(format!("conversation {} not found", id)))?;
        is_override = conv.owner_user_id != caller;
        if is_override && !(admin && force_admin) {
            return Err(ApiError::Forbidden(
                "not the owner; pass ?force_admin=true as admin to override".to_string(),
            ));
        }
        conv.title = new_title.clone();
        conv.updated_at = Utc::now();
        summary = ConversationSummary::from(&*conv);
    }
    store().persist_snapshot().await;

    let action = if is_override {
        "chat.conversation.renamed.admin_override"
    } else {
        "chat.conversation.renamed"
    };
    record_audit(
        state.as_ref(),
        caller,
        action,
        Some(format!("conversation:{}", id)),
        serde_json::json!({ "title": new_title, "admin_override": is_override }),
        AuditResult::Success,
    )
    .await;

    Ok(Json(summary))
}

async fn delete_conversation(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<String>,
    Query(q): Query<AdminOverrideQuery>,
) -> Result<Json<ConversationSummary>, ApiError> {
    let caller = caller_id(&claims);
    let admin = is_admin(&claims).await;
    let force_admin = q.force_admin.unwrap_or(false);

    let summary;
    let is_override;
    {
        let store = store();
        let mut guard = store.inner.write().await;
        let conv = guard
            .get_mut(&id)
            .ok_or_else(|| ApiError::NotFound(format!("conversation {} not found", id)))?;
        is_override = conv.owner_user_id != caller;
        if is_override && !(admin && force_admin) {
            return Err(ApiError::Forbidden(
                "not the owner; pass ?force_admin=true as admin to override".to_string(),
            ));
        }
        conv.deleted_at = Some(Utc::now());
        conv.updated_at = Utc::now();
        summary = ConversationSummary::from(&*conv);
    }
    store().persist_snapshot().await;

    let action = if is_override {
        "chat.conversation.deleted.admin_override"
    } else {
        "chat.conversation.deleted"
    };
    record_audit(
        state.as_ref(),
        caller,
        action,
        Some(format!("conversation:{}", id)),
        serde_json::json!({ "admin_override": is_override }),
        AuditResult::Success,
    )
    .await;

    Ok(Json(summary))
}

async fn append_message(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<String>,
    Query(q): Query<AdminOverrideQuery>,
    Json(msg): Json<ChatMessage>,
) -> Result<Json<Conversation>, ApiError> {
    if msg.role.trim().is_empty() {
        return Err(ApiError::InvalidRequest("role is required".to_string()));
    }

    let caller = caller_id(&claims);
    let admin = is_admin(&claims).await;
    let force_admin = q.force_admin.unwrap_or(false);

    let out;
    {
        let store = store();
        let mut guard = store.inner.write().await;
        let conv = guard
            .get_mut(&id)
            .ok_or_else(|| ApiError::NotFound(format!("conversation {} not found", id)))?;
        let is_override = conv.owner_user_id != caller;
        if is_override && !(admin && force_admin) {
            return Err(ApiError::Forbidden(
                "not the owner; pass ?force_admin=true as admin to override".to_string(),
            ));
        }
        if conv.deleted_at.is_some() {
            return Err(ApiError::InvalidRequest(
                "conversation is deleted".to_string(),
            ));
        }
        let mut stored = msg;
        if stored.ts.is_none() {
            stored.ts = Some(Utc::now().timestamp_millis());
        }
        conv.messages.push(stored);
        conv.updated_at = Utc::now();
        out = conv.clone();
    }
    store().persist_snapshot().await;

    // Auto-compress: fire-and-forget background task when threshold exceeded.
    if crate::api::chat_compress::should_auto_compress(&out) {
        let state_clone = state.clone();
        let conv_id_clone = id.clone();
        let caller_clone = caller.clone();
        let msg_count = out.messages.len();
        tokio::spawn(async move {
            tracing::info!(
                conv_id = %conv_id_clone,
                msg_count,
                "auto-compress triggered"
            );
            if let Err(e) = crate::api::chat_compress::compress_conversation_internal(
                &state_clone,
                &conv_id_clone,
                &caller_clone,
            )
            .await
            {
                tracing::warn!(conv_id = %conv_id_clone, error = %e, "auto-compress failed");
            }
        });
    }

    Ok(Json(out))
}

// ---------------------------------------------------------------------------
// POST /api/v1/chat/message  —  stateless LLM round-trip
// ---------------------------------------------------------------------------

async fn send_chat_message(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<SendChatMessageRequest>,
) -> Result<Json<SendChatMessageResponse>, ApiError> {
    use cyberclaw_llm::types::{ChatRequest, Message, Role};

    if req.messages.is_empty() {
        return Err(ApiError::InvalidRequest(
            "messages must not be empty".to_string(),
        ));
    }

    let caller = caller_id(&claims);

    // ----- Profile injection (B5) -------------------------------------------
    // When the caller passes `profile_id`, load the profile and:
    //   1. Prepend its `system_prompt` as a `system` role message (head of
    //      the message list). If the caller already supplied a `system`
    //      message at index 0, the profile prompt is inserted *before* it
    //      so profile identity wins.
    //   2. Override the chat model with `profile.default_model` when the
    //      caller did not explicitly pass `model`.
    // Profile lookup is owner-scoped: a profile owned by another user is
    // silently ignored (acts like no profile_id was provided) to avoid
    // cross-tenant leakage. Unknown profile_id is also silently ignored
    // (404-style) — we don't fail the request on a stale id.
    let mut profile_system_prompt: Option<String> = None;
    let mut profile_default_model: Option<String> = None;
    if let Some(pid) = req.profile_id.as_ref().filter(|s| !s.is_empty()) {
        if let Some(profile) = state.profile_store.get(pid) {
            if profile.owner_user_id == caller {
                if !profile.system_prompt.trim().is_empty() {
                    profile_system_prompt = Some(profile.system_prompt.clone());
                }
                profile_default_model = profile.default_model.clone();
            } else {
                warn!(
                    profile_id = %pid,
                    caller = %caller,
                    "send_chat_message: profile_id owned by another user, ignoring"
                );
            }
        } else {
            warn!(
                profile_id = %pid,
                caller = %caller,
                "send_chat_message: profile_id not found, ignoring"
            );
        }
    }

    // v1.1: chat_conversations now exposes `skill_search` + inline-intercepts
    // it (see dispatch loop below), so we can safely use the SkillFirst
    // profile — LLM can act on Iron Law 1 (search before refusing).
    let cyberclaw_core_prompt = cyberclaw_agent_runtime::constitution::cyberclaw_constitution_text(
        cyberclaw_agent_runtime::constitution::ConstitutionProfile::SkillFirst,
    );

    let mut messages: Vec<Message> = Vec::with_capacity(req.messages.len() + 2);
    messages.push(Message {
        role: Role::System,
        content: cyberclaw_core_prompt,
        tool_calls: None,
        tool_call_id: None,
        name: None,
        cache_control: None,
    });
    if let Some(sp) = profile_system_prompt.as_ref() {
        messages.push(Message {
            role: Role::System,
            content: sp.clone(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            cache_control: None,
        });
    }
    for m in req.messages.into_iter() {
        let role = match m.role.to_lowercase().as_str() {
            "system" => Role::System,
            "assistant" => Role::Assistant,
            "tool" => Role::Tool,
            _ => Role::User,
        };
        messages.push(Message {
            role,
            content: m.content,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            cache_control: None,
        });
    }

    // Default model precedence: explicit request → profile.default_model →
    // CYBERCLAW_DEFAULT_MODEL env (cyberclaw-specific override) →
    // LLM_DEFAULT_MODEL env (provider-side default, e.g. MiniMax-M2.7-HighSpeed) →
    // gpt-4o-mini (last-resort fallback for openai-compat env).
    let model = req
        .model
        .filter(|s| !s.is_empty())
        .or(profile_default_model)
        .or_else(|| std::env::var("CYBERCLAW_DEFAULT_MODEL").ok())
        .or_else(|| std::env::var("LLM_DEFAULT_MODEL").ok())
        .unwrap_or_else(|| "gpt-4o-mini".to_string());

    // ----- Gateway routing flag ---------------------------------------------
    // CYBERCLAW_CHAT_VIA_GATEWAY=true switches this handler to materialize
    // the governing gateway (which carries PolicyEngine) *before* dispatching
    // the LLM call. The chat round-trip itself doesn't issue Connector→
    // Capability calls, so the gateway materialization is currently a
    // governance touch-point + audit-visibility upgrade, not a routing
    // change. Default (flag absent) keeps the legacy direct-LLM path so
    // we can ship the fix in two stages.
    let via_gateway = std::env::var("CYBERCLAW_CHAT_VIA_GATEWAY")
        .ok()
        .filter(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .is_some();

    if via_gateway {
        // Materialize the governing gateway. This binds PolicyEngine into
        // the call chain; even though the LLM round-trip itself doesn't
        // invoke a capability, this gives the governance lane visibility
        // and prepares for follow-up work that will route tool calls
        // through the gateway. The handle is intentionally unused for the
        // pure LLM round-trip — the side effect is the audit trail below.
        let _gateway = crate::api::chat_handler::build_governing_gateway(&state);
    }

    // v1.1-rc7 Option B: webui chat now FULLY delegates to chat_handler's
    // agentic loop (41 tools + ToolCallMapper + IterationBudget + skill_search
    // intercept + skill scripts/ executable). This unifies all 3 chat paths
    // on one agentic core — chat_conversations is the user-friendly wrapper,
    // agent_chat_completions is the engine. Before this commit chat_conversations
    // had a residual single-tool dispatch loop (skill_search only) that left
    // LLM unable to execute discovered skills (PPT user case).
    let agent_messages: Vec<crate::api::chat_handler::ChatMessage> = messages
        .into_iter()
        .map(|m| crate::api::chat_handler::ChatMessage {
            role: match m.role {
                Role::System => "system".to_string(),
                Role::User => "user".to_string(),
                Role::Assistant => "assistant".to_string(),
                Role::Tool => "tool".to_string(),
            },
            content: m.content,
        })
        .collect();

    // Pre-call audit so governance sees the chat dispatch.
    record_audit(
        state.as_ref(),
        caller.clone(),
        if via_gateway {
            "chat.message.sent.via_gateway"
        } else {
            "chat.message.sent"
        },
        req.conversation_id
            .as_ref()
            .map(|id| format!("conversation:{}", id)),
        serde_json::json!({
            "model": model,
            "profile_id": req.profile_id,
            "message_count": agent_messages.len(),
            "via_gateway": via_gateway,
            "delegate": "agent_chat_completions",
        }),
        AuditResult::Success,
    )
    .await;

    // Build AgentChatRequest via JSON (struct has no Default derive). Only
    // populating the 3 fields chat_conversations actually needs; the rest
    // (agent_id, skill_ids, execution_mode, ...) take serde defaults.
    let agent_req: crate::api::chat_handler::AgentChatRequest =
        serde_json::from_value(serde_json::json!({
            "messages": agent_messages,
            "model": model.clone(),
            "stream": false,
        }))
        .map_err(|e| ApiError::InternalError(format!("build agent_req: {e}")))?;

    // Reuse the caller's Claims for the delegated handler — both routes are
    // mounted under the same JWT-protected lane, so this preserves identity.
    let delegated_claims = claims.clone();

    use axum::body::to_bytes;
    let inner_resp = crate::api::chat_handler::agent_chat_completions(
        axum::extract::State(state.clone()),
        axum::Extension(delegated_claims),
        axum::Json(agent_req),
    )
    .await?;

    let (parts, body) = inner_resp.into_parts();
    if !parts.status.is_success() {
        return Err(ApiError::InternalError(format!(
            "agent_chat delegate returned status {}",
            parts.status
        )));
    }
    let body_bytes = to_bytes(body, 32 * 1024 * 1024)
        .await
        .map_err(|e| ApiError::InternalError(format!("body read: {e}")))?;
    let chat_resp: serde_json::Value = serde_json::from_slice(&body_bytes)
        .map_err(|e| ApiError::InternalError(format!("parse: {e}")))?;

    let content = chat_resp
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|c0| c0.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    let finish_reason = chat_resp
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|c0| c0.get("finish_reason"))
        .and_then(|f| f.as_str())
        .map(|s| s.to_string());
    let tool_calls_used = chat_resp
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|c0| c0.get("message"))
        .and_then(|m| m.get("tool_calls"))
        .and_then(|tc| tc.as_array())
        .map(|arr| !arr.is_empty())
        .unwrap_or(false);

    let confidence = compute_confidence_score(&content, finish_reason.as_deref(), tool_calls_used);

    return Ok(Json(SendChatMessageResponse {
        message: SendChatMessageOutput {
            role: "assistant".to_string(),
            content,
            confidence_score: Some(confidence),
        },
    }));
    #[allow(unreachable_code)]
    {
        // Legacy single-tool dispatch loop preserved below as #[allow(dead_code)]
        // for rollback safety. Will be removed after rc7 bakes in production.
        let max_iters: u32 = 5;
        let mut working_req = ChatRequest::default();
        let mut any_tool_dispatched = false;
        let mut final_choice: Option<cyberclaw_llm::types::Choice> = None;
        for _iter in 0..max_iters {
            let mut iter_req = working_req.clone();
            iter_req.stream = Some(false);
            let resp = state
                .llm_client
                .chat_completion(iter_req)
                .await
                .map_err(|e| ApiError::LlmError(e.to_string()))?;
            let choice = resp
                .choices
                .into_iter()
                .next()
                .ok_or_else(|| ApiError::LlmError("LLM returned no choices".to_string()))?;

            let tcs = choice.message.tool_calls.clone().unwrap_or_default();
            let has_skill_search = tcs.iter().any(|t| t.function.name == "skill_search");

            if !has_skill_search {
                final_choice = Some(choice);
                break;
            }

            // Append the assistant message (with tool_calls intent) so the
            // provider sees the round-trip continuity.
            working_req.messages.push(Message {
                role: Role::Assistant,
                content: choice.message.content.clone(),
                tool_calls: Some(tcs.clone()),
                tool_call_id: None,
                name: None,
                cache_control: None,
            });

            // Intercept each skill_search call. Note: skill_search hits the
            // SkillIndex read-only — no governance gate needed beyond the
            // skill.search audit emit.
            for tc in &tcs {
                if tc.function.name != "skill_search" {
                    // Non-skill_search tool — surface a clear "tool not exposed"
                    // message so LLM doesn't loop trying it.
                    working_req.messages.push(Message::tool(
                        tc.id.clone(),
                        format!(
                            "{{\"error\":\"tool '{}' not exposed on this chat path; \
                         only skill_search is dispatchable here. Reply with a \
                         direct answer or use the matched skill's instructions.\"}}",
                            tc.function.name
                        ),
                    ));
                    continue;
                }
                any_tool_dispatched = true;
                let args: serde_json::Value = serde_json::from_str(&tc.function.arguments)
                    .unwrap_or_else(|_| serde_json::json!({"q": "", "limit": 20}));
                let q = args
                    .get("q")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as u32;

                let result_str = match state.skill_index.as_ref() {
                    Some(idx) => match idx.search(q.clone(), limit).await {
                        Ok(rows) => serde_json::to_string(&serde_json::json!({"results": rows}))
                            .unwrap_or_else(|_| "{\"results\":[]}".to_string()),
                        Err(e) => format!("{{\"error\":\"{}\"}}", e),
                    },
                    None => "{\"error\":\"skill index not initialized\"}".to_string(),
                };

                // Audit emit so skill.search is visible in the governance chain.
                record_audit(
                    state.as_ref(),
                    caller.clone(),
                    "skill.search",
                    Some(format!("query:{}", q.chars().take(80).collect::<String>())),
                    serde_json::json!({"q": q, "limit": limit}),
                    AuditResult::Success,
                )
                .await;

                working_req
                    .messages
                    .push(Message::tool(tc.id.clone(), result_str));
            }
        }

        let choice = final_choice.ok_or_else(|| {
            ApiError::LlmError(format!(
                "LLM dispatch loop exhausted after {} iterations without converging",
                max_iters
            ))
        })?;

        let finish_reason = choice.finish_reason.clone();
        let tool_calls_used = any_tool_dispatched
            || choice
                .message
                .tool_calls
                .as_ref()
                .map(|t| !t.is_empty())
                .unwrap_or(false);
        let content = choice.message.content;
        let confidence =
            compute_confidence_score(&content, finish_reason.as_deref(), tool_calls_used);

        Ok(Json(SendChatMessageResponse {
            message: SendChatMessageOutput {
                role: "assistant".to_string(),
                content,
                confidence_score: Some(confidence),
            },
        }))
    } // end #[allow(unreachable_code)] block
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_helpers::build_test_state;
    use cyberclaw_core::ids::UserId;
    use serial_test::serial;

    fn claims_for(user: &str) -> Claims {
        let uid = UserId::from_string(user.to_string()).expect("valid user id");
        let now = Utc::now().timestamp();
        Claims {
            sub: uid,
            tenant: None,
            iat: now,
            exp: now + 3600,
        }
    }

    async fn clean_store() {
        reset_store_for_tests().await;
    }

    #[tokio::test]
    #[serial]
    async fn list_empty_returns_no_rows() {
        clean_store().await;
        let state = build_test_state();
        let resp = list_conversations(
            State(state),
            Extension(claims_for("alice")),
            Query(ListQuery::default()),
        )
        .await
        .expect("list ok");
        assert_eq!(resp.0.conversations.len(), 0);
    }

    #[tokio::test]
    #[serial]
    async fn create_and_list_roundtrip() {
        clean_store().await;
        let state = build_test_state();
        let created = create_conversation(
            State(state.clone()),
            Extension(claims_for("alice")),
            Json(CreateConversationRequest {
                title: Some("First".to_string()),
            }),
        )
        .await
        .expect("create ok");
        assert_eq!(created.0.title, "First");
        assert!(created.0.id.starts_with("conv_"));

        let listed = list_conversations(
            State(state),
            Extension(claims_for("alice")),
            Query(ListQuery::default()),
        )
        .await
        .expect("list ok");
        assert_eq!(listed.0.conversations.len(), 1);
        assert_eq!(listed.0.conversations[0].id, created.0.id);
    }

    #[tokio::test]
    #[serial]
    async fn list_rbac_viewer_sees_only_own() {
        clean_store().await;
        let state = build_test_state();
        let _a = create_conversation(
            State(state.clone()),
            Extension(claims_for("alice")),
            Json(CreateConversationRequest {
                title: Some("A".to_string()),
            }),
        )
        .await
        .unwrap();
        let _b = create_conversation(
            State(state.clone()),
            Extension(claims_for("bob")),
            Json(CreateConversationRequest {
                title: Some("B".to_string()),
            }),
        )
        .await
        .unwrap();

        let alice_view = list_conversations(
            State(state),
            Extension(claims_for("alice")),
            Query(ListQuery::default()),
        )
        .await
        .unwrap();
        assert_eq!(alice_view.0.conversations.len(), 1);
        assert_eq!(alice_view.0.conversations[0].title, "A");
    }

    #[tokio::test]
    #[serial]
    async fn rename_happy_path() {
        clean_store().await;
        let state = build_test_state();
        let created = create_conversation(
            State(state.clone()),
            Extension(claims_for("alice")),
            Json(CreateConversationRequest {
                title: Some("Old".to_string()),
            }),
        )
        .await
        .unwrap();

        let renamed = rename_conversation(
            State(state),
            Extension(claims_for("alice")),
            Path(created.0.id.clone()),
            Query(AdminOverrideQuery::default()),
            Json(RenameConversationRequest {
                title: "New Title".to_string(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(renamed.0.title, "New Title");
        assert_eq!(renamed.0.id, created.0.id);
    }

    #[tokio::test]
    #[serial]
    async fn rename_rejected_for_non_owner_viewer() {
        clean_store().await;
        let state = build_test_state();
        let created = create_conversation(
            State(state.clone()),
            Extension(claims_for("alice")),
            Json(CreateConversationRequest {
                title: Some("Old".to_string()),
            }),
        )
        .await
        .unwrap();

        let err = rename_conversation(
            State(state),
            Extension(claims_for("mallory")),
            Path(created.0.id),
            Query(AdminOverrideQuery::default()),
            Json(RenameConversationRequest {
                title: "Hijack".to_string(),
            }),
        )
        .await
        .expect_err("non-owner must be rejected");
        assert!(matches!(err, ApiError::Forbidden(_)), "got {:?}", err);
    }

    #[tokio::test]
    #[serial]
    async fn soft_delete_hides_from_default_listing() {
        clean_store().await;
        let state = build_test_state();
        let created = create_conversation(
            State(state.clone()),
            Extension(claims_for("alice")),
            Json(CreateConversationRequest {
                title: Some("Doomed".to_string()),
            }),
        )
        .await
        .unwrap();

        let deleted = delete_conversation(
            State(state.clone()),
            Extension(claims_for("alice")),
            Path(created.0.id.clone()),
            Query(AdminOverrideQuery::default()),
        )
        .await
        .unwrap();
        assert!(deleted.0.deleted_at.is_some());

        let listed = list_conversations(
            State(state.clone()),
            Extension(claims_for("alice")),
            Query(ListQuery::default()),
        )
        .await
        .unwrap();
        assert_eq!(listed.0.conversations.len(), 0);

        // But include_deleted=true surfaces it.
        let listed_all = list_conversations(
            State(state),
            Extension(claims_for("alice")),
            Query(ListQuery {
                include_deleted: Some(true),
            }),
        )
        .await
        .unwrap();
        assert_eq!(listed_all.0.conversations.len(), 1);
    }

    #[tokio::test]
    #[serial]
    async fn append_message_updates_conversation() {
        clean_store().await;
        let state = build_test_state();
        let created = create_conversation(
            State(state.clone()),
            Extension(claims_for("alice")),
            Json(CreateConversationRequest {
                title: Some("Chat".to_string()),
            }),
        )
        .await
        .unwrap();

        let appended = append_message(
            State(state),
            Extension(claims_for("alice")),
            Path(created.0.id.clone()),
            Query(AdminOverrideQuery::default()),
            Json(ChatMessage {
                role: "user".to_string(),
                content: "hi".to_string(),
                metadata: None,
                ts: None,
            }),
        )
        .await
        .unwrap();
        assert_eq!(appended.0.messages.len(), 1);
        assert_eq!(appended.0.messages[0].role, "user");
        assert_eq!(appended.0.messages[0].content, "hi");
        assert!(appended.0.messages[0].ts.is_some());
    }

    #[tokio::test]
    #[serial]
    async fn audit_event_emitted_on_create() {
        use crate::audit::AuditSink;
        use std::sync::Arc as StdArc;

        clean_store().await;
        let tmp = tempfile::TempDir::new().unwrap();
        let sink = StdArc::new(AuditSink::new(tmp.path().join("audit.db")).await.unwrap());
        let mut state = build_test_state();
        StdArc::get_mut(&mut state).expect("unique").audit = Some(sink.clone());

        let _ = create_conversation(
            State(state),
            Extension(claims_for("alice")),
            Json(CreateConversationRequest {
                title: Some("Audited".to_string()),
            }),
        )
        .await
        .unwrap();

        let entries = sink
            .tail(10, &crate::audit::AuditQuery::default())
            .await
            .unwrap();
        assert!(entries
            .iter()
            .any(|e| e.action == "chat.conversation.created"));
    }

    #[tokio::test]
    #[serial]
    async fn conversation_serializes_roundtrip() {
        let c = Conversation {
            id: "conv_abc".to_string(),
            title: "Title".to_string(),
            owner_user_id: "alice".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: "hi".to_string(),
                metadata: None,
                ts: Some(1700000000),
            }],
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            last_compressed_at: None,
            active_agent_id: None,
        };
        let encoded = serde_json::to_string(&c).unwrap();
        let decoded: Conversation = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.id, c.id);
        assert_eq!(decoded.messages.len(), 1);
        assert_eq!(decoded.messages[0].content, "hi");
    }

    // -----------------------------------------------------------------------
    // Sprint 21 T2 — role="handoff" message construction + roundtrip
    // -----------------------------------------------------------------------

    fn sample_handoff_request() -> cyberclaw_core::handoff::HandoffRequest {
        cyberclaw_core::handoff::HandoffRequest::new(
            cyberclaw_core::ids::HandoffId::from_string("ho_01".to_string()).unwrap(),
            cyberclaw_core::ids::AgentId::from_string("agent_a".to_string()).unwrap(),
            cyberclaw_core::ids::AgentId::from_string("agent_b".to_string()).unwrap(),
            "conv_handoff_42".to_string(),
            "B is better at backend".to_string(),
            "Schema review needed; PRD at /tmp/prd.md; key concern: latency".to_string(),
            vec![],
            None,
            Utc::now(),
        )
    }

    #[test]
    fn handoff_card_has_correct_role_and_payload() {
        let req = sample_handoff_request();
        let msg = ChatMessage::handoff_card(&req);

        assert_eq!(msg.role, "handoff");
        assert!(msg.content.contains("🔀"));
        assert!(msg.content.contains("agent_a"));
        assert!(msg.content.contains("agent_b"));
        assert!(msg.content.contains("B is better at backend"));
        assert!(msg.ts.is_some());

        let meta = msg.metadata.expect("metadata must be present");
        assert_eq!(meta["handoff_id"], "ho_01");
        assert_eq!(meta["from_agent_id"], "agent_a");
        assert_eq!(meta["to_agent_id"], "agent_b");
        assert_eq!(meta["reason"], "B is better at backend");
        assert!(meta["briefing_preview"]
            .as_str()
            .unwrap()
            .contains("Schema review"));
        assert_eq!(meta["briefing_truncated"], false);
    }

    #[test]
    fn handoff_card_truncates_long_briefing() {
        let mut req = sample_handoff_request();
        req.briefing_text = "x".repeat(500);
        let msg = ChatMessage::handoff_card(&req);

        let meta = msg.metadata.expect("metadata must be present");
        let preview = meta["briefing_preview"].as_str().unwrap();
        assert_eq!(preview.chars().count(), 200);
        assert_eq!(meta["briefing_truncated"], true);
    }

    #[tokio::test]
    async fn handoff_card_roundtrips_through_store() {
        let store = ConversationStore::new_in_memory();
        {
            let mut guard = store.inner.write().await;
            guard.insert(
                "conv_handoff_42".to_string(),
                Conversation {
                    id: "conv_handoff_42".to_string(),
                    title: "Test".to_string(),
                    owner_user_id: "alice".to_string(),
                    messages: vec![],
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    deleted_at: None,
                    last_compressed_at: None,
                    active_agent_id: None,
                },
            );
        }

        let req = sample_handoff_request();
        let msg = ChatMessage::handoff_card(&req);
        store
            .append_message_internal("conv_handoff_42", msg)
            .await
            .expect("append should succeed");

        let conv = store.get("conv_handoff_42").await.expect("exists");
        assert_eq!(conv.messages.len(), 1);
        assert_eq!(conv.messages[0].role, "handoff");

        // Serde roundtrip (simulates disk persistence path)
        let bytes = serde_json::to_vec(&conv).unwrap();
        let decoded: Conversation = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded.messages[0].role, "handoff");
        let meta = decoded.messages[0]
            .metadata
            .as_ref()
            .expect("metadata preserved");
        assert_eq!(meta["handoff_id"], "ho_01");
    }

    // -----------------------------------------------------------------------
    // Sprint 21 T6 — active_agent_id field + set_active_agent
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn conversation_serializes_roundtrip_with_active_agent() {
        let agent_id =
            cyberclaw_core::ids::AgentId::from_string("agent_target".to_string()).unwrap();
        let mut c = Conversation {
            id: "conv_t6".to_string(),
            title: "T6 Test".to_string(),
            owner_user_id: "alice".to_string(),
            messages: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            last_compressed_at: None,
            active_agent_id: None,
        };

        // Before set: field absent from serialized JSON (skip_serializing_if)
        let json_before = serde_json::to_string(&c).unwrap();
        assert!(
            !json_before.contains("active_agent_id"),
            "active_agent_id should be omitted when None"
        );

        // Set the agent id
        c.active_agent_id = Some(agent_id.clone());

        // Roundtrip through JSON
        let encoded = serde_json::to_string(&c).unwrap();
        let decoded: Conversation = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.id, c.id);
        assert_eq!(
            decoded.active_agent_id.as_ref().map(|a| a.to_string()),
            Some("agent_target".to_string()),
            "active_agent_id must survive serde roundtrip"
        );
    }

    #[tokio::test]
    async fn set_active_agent_updates_conversation() {
        let store = ConversationStore::new_in_memory();
        let now = Utc::now();
        {
            let mut guard = store.inner.write().await;
            guard.insert(
                "conv_set_agent".to_string(),
                Conversation {
                    id: "conv_set_agent".to_string(),
                    title: "Test".to_string(),
                    owner_user_id: "alice".to_string(),
                    messages: vec![],
                    created_at: now,
                    updated_at: now,
                    deleted_at: None,
                    last_compressed_at: None,
                    active_agent_id: None,
                },
            );
        }
        let agent_id = cyberclaw_core::ids::AgentId::from_string("agent_new".to_string()).unwrap();
        store
            .set_active_agent("conv_set_agent", agent_id)
            .await
            .expect("set_active_agent should succeed");

        let conv = store.get("conv_set_agent").await.expect("exists");
        assert_eq!(
            conv.active_agent_id.as_ref().map(|a| a.to_string()),
            Some("agent_new".to_string())
        );
    }

    #[tokio::test]
    async fn set_active_agent_returns_err_for_missing_conv() {
        let store = ConversationStore::new_in_memory();
        let agent_id = cyberclaw_core::ids::AgentId::from_string("agent_x".to_string()).unwrap();
        let result = store.set_active_agent("nonexistent", agent_id).await;
        assert!(result.is_err(), "should error on missing conversation");
    }

    #[test]
    fn unknown_role_deserializes_without_error() {
        // Backward-compat invariant: future roles added by peers must not
        // break ChatMessage deserialization. role is String (not enum) so this
        // is guaranteed by construction, but lock the property with a test.
        let json = r#"{"role":"future_role_xyz","content":"hello"}"#;
        let msg: ChatMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.role, "future_role_xyz");
        assert_eq!(msg.content, "hello");
    }

    #[tokio::test]
    #[serial]
    async fn owner_check_blocks_rename_without_force_admin() {
        clean_store().await;
        let state = build_test_state();
        // alice creates
        let created = create_conversation(
            State(state.clone()),
            Extension(claims_for("alice")),
            Json(CreateConversationRequest {
                title: Some("Original".to_string()),
            }),
        )
        .await
        .unwrap();
        // bob tries to rename — should fail (bob is not admin anyway; even
        // without users.toml seed, require_admin returns Err).
        let err = rename_conversation(
            State(state),
            Extension(claims_for("bob")),
            Path(created.0.id.clone()),
            Query(AdminOverrideQuery {
                force_admin: Some(true),
            }),
            Json(RenameConversationRequest {
                title: "Pwn".to_string(),
            }),
        )
        .await
        .expect_err("non-admin must be denied even with force_admin=true");
        assert!(matches!(err, ApiError::Forbidden(_)), "got {:?}", err);
    }
}
