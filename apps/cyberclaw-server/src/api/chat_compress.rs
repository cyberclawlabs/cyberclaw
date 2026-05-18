//! Sprint 16 Task #2+#3 — Compress HTTP surface + Summary message injection.
//!
//! Exposes a single endpoint:
//!
//! | Method | Path                                                              |
//! |--------|-------------------------------------------------------------------|
//! | POST   | `/api/v1/chat/conversations/:conversation_id/compress`            |
//!
//! # What it does
//!
//! 1. RBAC: only the conversation owner or an admin may compress.
//! 2. Guard: at least 5 messages (else `400 conversation_too_short`).
//! 3. Skip: if every message already has `role="summary"` return
//!    `400 already_compressed`.
//! 4. Convert chat messages to LLM messages via [`message_bridge`].
//! 5. Run [`ContextCompressor::compress_all`] (fresh instance per request
//!    to avoid cross-request circuit-breaker pollution).
//! 6. Extract the synthetic summary produced by the `SummarizeEarly` stage
//!    (prefix `[Context summary`) and inject it as a `role="summary"` chat
//!    message at the head of the resulting list.
//! 7. Atomically replace the conversation's message list.
//! 8. Emit `AuditKind::Mutation` + `AdminEvent::ConversationCompressed`.
//!
//! # Transactionality
//!
//! If the compressor returns an error (circuit breaker tripped / empty output)
//! the conversation is **never mutated** — the handler returns early before
//! writing back to the store.

use std::sync::Arc;

use axum::{
    extract::{Extension, Path, State},
    routing::post,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use cyberclaw_agent_runtime::context_compressor::{
    CompressionConfig, CompressionStage, ContextCompressor,
};
use cyberclaw_llm::types::{Message as LlmMessage, Role};

use crate::api::admin::events::AdminEvent;
use crate::api::chat_conversations::{store, ChatMessage};
use crate::api::message_bridge::{chat_vec_to_llm, llm_to_chat};
use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::{require_admin, Claims};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Response type
// ---------------------------------------------------------------------------

/// Response returned by `POST …/compress`.
#[derive(Debug, Clone, Serialize)]
pub struct CompressResponse {
    pub success: bool,
    /// Number of messages before compression.
    pub original_count: usize,
    /// Number of messages after compression (including any injected summary).
    pub compressed_count: usize,
    /// Id of the injected `role="summary"` message, when one was created.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_message_id: Option<String>,
    /// Stages that actually changed the message list.
    pub stages_applied: Vec<CompressionStage>,
    pub compressed_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Mount the compress endpoint inside the protected-routes lane.
pub fn create_chat_compress_router() -> Router<Arc<AppState>> {
    Router::new().route(
        "/api/v1/chat/conversations/:conversation_id/compress",
        post(compress_handler),
    )
}

// ---------------------------------------------------------------------------
// Auto-compress configuration
// ---------------------------------------------------------------------------

/// Number of accumulated chars in a conversation that triggers auto-compress.
///
/// Env: `CYBERCLAW_AUTO_COMPRESS_THRESHOLD` (default 24000 ≈ 6000 tokens @ 4:1).
pub fn auto_compress_threshold() -> usize {
    std::env::var("CYBERCLAW_AUTO_COMPRESS_THRESHOLD")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(24_000)
}

/// Cooldown in seconds between successive auto-compresses of the same
/// conversation, to avoid thrashing.
///
/// Env: `CYBERCLAW_AUTO_COMPRESS_COOLDOWN_SECS` (default 600 = 10 min).
pub fn auto_compress_cooldown_secs() -> i64 {
    std::env::var("CYBERCLAW_AUTO_COMPRESS_COOLDOWN_SECS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(600)
}

/// Return `true` when `conv` should be auto-compressed: threshold exceeded and
/// cooldown elapsed.
pub fn should_auto_compress(conv: &crate::api::chat_conversations::Conversation) -> bool {
    let total_chars: usize = conv
        .messages
        .iter()
        .map(|m| m.content.chars().count())
        .sum();
    if total_chars < auto_compress_threshold() {
        return false;
    }
    // Cooldown check
    if let Some(last) = conv.last_compressed_at {
        let elapsed = Utc::now().signed_duration_since(last).num_seconds();
        if elapsed < auto_compress_cooldown_secs() {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Internal compress function (shared by HTTP handler + auto-compress path)
// ---------------------------------------------------------------------------

/// Core compress logic — no HTTP layer, no RBAC check.
///
/// Callers are responsible for RBAC. Returns `Ok(CompressResponse)` on
/// success. On validation failure (too short / already compressed) returns
/// `Err(ApiError)` without mutating the store. The auto-compress path calls
/// this from a `tokio::spawn` background task.
pub(crate) async fn compress_conversation_internal(
    state: &Arc<AppState>,
    conversation_id: &str,
    caller: &str,
) -> Result<CompressResponse, ApiError> {
    let conv_store = store();
    let conv = conv_store
        .get(conversation_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("conversation {} not found", conversation_id)))?;

    let messages = &conv.messages;
    let original_count = messages.len();

    // Guard: minimum length
    if original_count < 5 {
        return Err(ApiError::InvalidRequest(
            "conversation_too_short: need at least 5 messages to compress".to_string(),
        ));
    }

    // Guard: already fully compressed
    if messages.iter().all(|m| m.role == "summary") {
        return Err(ApiError::InvalidRequest(
            "already_compressed: all messages are already summaries".to_string(),
        ));
    }

    // Convert to LLM messages (summary messages are skipped by bridge)
    let llm_msgs = chat_vec_to_llm(messages);

    // Compress — fresh compressor per call (avoids cross-request circuit
    //    breaker state)
    let mut compressor = ContextCompressor::new(CompressionConfig::default());
    let result = compressor.compress_all(&llm_msgs);

    // Guard: circuit breaker tripped
    if result.stages_applied.is_empty()
        && result.compressed_count == result.original_count
        && result.original_count > 0
    {
        return Err(ApiError::InternalError(
            "compression_failed: circuit breaker tripped".to_string(),
        ));
    }

    // Extract summary and convert back to chat messages
    let stages_applied = result.stages_applied.clone();
    let (new_messages, summary_message_id) =
        extract_summary_and_convert(result.messages, original_count, &stages_applied);

    let compressed_count = new_messages.len();

    // Atomically replace messages
    conv_store
        .replace_messages(conversation_id, new_messages)
        .await
        .map_err(|e| ApiError::InternalError(format!("compression_failed: {}", e)))?;

    // Mark last_compressed_at (best-effort; failure is non-fatal)
    conv_store.touch_last_compressed_at(conversation_id).await;

    let compressed_at = Utc::now();

    // Audit entry
    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            caller.to_string(),
            AuditKind::Mutation,
            "chat.conversation.compressed",
            Some(format!("conversation:{}", conversation_id)),
            serde_json::json!({
                "conversation_id": conversation_id,
                "original_count": original_count,
                "compressed_count": compressed_count,
            }),
            AuditResult::Success,
        ))
        .await;
    }

    // Admin event
    let _ = state
        .admin_event_bus
        .send(AdminEvent::ConversationCompressed {
            conversation_id: conversation_id.to_string(),
            compressed_at,
        });

    Ok(CompressResponse {
        success: true,
        original_count,
        compressed_count,
        summary_message_id,
        stages_applied,
        compressed_at,
    })
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

pub async fn compress_handler(
    State(state): State<Arc<AppState>>,
    Path(conversation_id): Path<String>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<CompressResponse>, ApiError> {
    let caller = claims.sub.as_ref().to_string();
    let admin = require_admin(&claims).await.is_ok();

    // 1. Load conversation (for RBAC check)
    let conv_store = store();
    let conv = conv_store
        .get(&conversation_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("conversation {} not found", conversation_id)))?;

    // 2. RBAC: owner or admin
    if conv.owner_user_id != caller && !admin {
        return Err(ApiError::Forbidden(
            "not the owner of this conversation".to_string(),
        ));
    }

    let resp = compress_conversation_internal(&state, &conversation_id, &caller).await?;
    Ok(Json(resp))
}

// ---------------------------------------------------------------------------
// Task #3 — Summary injection
// ---------------------------------------------------------------------------

/// Scan `compressed` for synthetic summary messages produced by
/// [`CompressionStage::SummarizeEarly`] (identified by the `[Context summary`
/// prefix on a `Role::System` message), build a single `role="summary"` chat
/// message, and convert the remaining messages back to chat format.
///
/// Returns `(new_messages, summary_msg_id_option)`.
fn extract_summary_and_convert(
    compressed: Vec<LlmMessage>,
    original_count: usize,
    stages_applied: &[CompressionStage],
) -> (Vec<ChatMessage>, Option<String>) {
    const SUMMARY_PREFIX: &str = "[Context summary";

    let mut summary_parts: Vec<String> = Vec::new();
    let mut non_summary: Vec<LlmMessage> = Vec::new();

    for msg in compressed {
        if msg.role == Role::System && msg.content.starts_with(SUMMARY_PREFIX) {
            summary_parts.push(msg.content.clone());
        } else {
            non_summary.push(msg);
        }
    }

    let now = Utc::now();
    let mut new_messages: Vec<ChatMessage> = Vec::new();
    let mut summary_message_id: Option<String> = None;

    if !summary_parts.is_empty() {
        let summary_text = summary_parts.join("\n");
        let sid = format!("msg_{}", Uuid::new_v4().simple());

        let stages_json: Vec<String> = stages_applied.iter().map(|s| format!("{:?}", s)).collect();

        let summary_msg = ChatMessage {
            role: "summary".to_string(),
            content: summary_text,
            metadata: Some(serde_json::json!({
                "original_count": original_count,
                "compressed_at": now.to_rfc3339(),
                "stages_applied": stages_json,
                "summary_id": sid,
            })),
            ts: Some(now.timestamp_millis()),
        };
        summary_message_id = Some(sid);
        new_messages.push(summary_msg);
    }

    // Convert remaining LLM messages back to chat messages
    for llm_msg in non_summary {
        let id = format!("msg_{}", Uuid::new_v4().simple());
        new_messages.push(llm_to_chat(&llm_msg, id));
    }

    (new_messages, summary_message_id)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::chat_conversations::{reset_store_for_tests, store, ChatMessage};
    use crate::api::test_helpers::{build_test_state, seed_users_with_role};
    use crate::audit::AuditSink;
    use serial_test::serial;
    use std::sync::Arc as StdArc;

    fn make_messages(n: usize) -> Vec<ChatMessage> {
        (0..n)
            .map(|i| ChatMessage {
                role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
                content: format!("Message content number {}", i),
                metadata: None,
                ts: Some(i as i64 * 1000),
            })
            .collect()
    }

    async fn setup_conversation_with_messages(conv_id: &str, owner: &str, n: usize) {
        let s = store();
        s.create_for_test(conv_id, owner).await;
        let msgs = make_messages(n);
        s.replace_messages(conv_id, msgs).await.expect("replace ok");
    }

    fn claims_for(user: &str) -> Claims {
        use cyberclaw_core::ids::UserId;
        let uid = UserId::from_string(user.to_string()).expect("valid");
        let now = Utc::now().timestamp();
        Claims {
            sub: uid,
            tenant: None,
            iat: now,
            exp: now + 3600,
        }
    }

    // -----------------------------------------------------------------------
    // Test 1: happy path
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[serial]
    async fn test_compress_happy_path() {
        reset_store_for_tests().await;
        let state = build_test_state();
        setup_conversation_with_messages("conv_happy", "alice", 20).await;

        let resp = compress_handler(
            State(state),
            Path("conv_happy".to_string()),
            Extension(claims_for("alice")),
        )
        .await
        .expect("compress should succeed");

        assert!(resp.0.success);
        assert_eq!(resp.0.original_count, 20);
        assert!(resp.0.compressed_count < 20, "should compress");
        assert!(
            !resp.0.stages_applied.is_empty(),
            "at least one stage applied"
        );
    }

    // -----------------------------------------------------------------------
    // Test 2: too short
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[serial]
    async fn test_compress_too_short() {
        reset_store_for_tests().await;
        let state = build_test_state();
        setup_conversation_with_messages("conv_short", "alice", 3).await;

        let err = compress_handler(
            State(state),
            Path("conv_short".to_string()),
            Extension(claims_for("alice")),
        )
        .await
        .expect_err("must fail for < 5 messages");

        assert!(
            matches!(err, ApiError::InvalidRequest(ref s) if s.contains("conversation_too_short")),
            "got {:?}",
            err
        );
    }

    // -----------------------------------------------------------------------
    // Test 3: RBAC viewer denied
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[serial]
    async fn test_compress_rbac_viewer_denied() {
        reset_store_for_tests().await;
        let state = build_test_state();
        setup_conversation_with_messages("conv_rbac", "alice", 10).await;

        let err = compress_handler(
            State(state),
            Path("conv_rbac".to_string()),
            Extension(claims_for("mallory")),
        )
        .await
        .expect_err("non-owner must be denied");

        assert!(matches!(err, ApiError::Forbidden(_)), "got {:?}", err);
    }

    // -----------------------------------------------------------------------
    // Test 4: admin override
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[serial]
    async fn test_compress_admin_override() {
        reset_store_for_tests().await;
        let (tmp, _restore) = seed_users_with_role("admin-user", "admin");
        let state = build_test_state();
        setup_conversation_with_messages("conv_admin", "alice", 20).await;

        // Admin compresses a conversation they don't own
        let resp = compress_handler(
            State(state),
            Path("conv_admin".to_string()),
            Extension(claims_for("admin-user")),
        )
        .await
        .expect("admin should be able to compress");

        assert!(resp.0.success);
        drop(tmp);
    }

    // -----------------------------------------------------------------------
    // Test 5: already all summary
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[serial]
    async fn test_compress_already_all_summary() {
        reset_store_for_tests().await;
        let state = build_test_state();

        let s = store();
        s.create_for_test("conv_summary", "alice").await;
        let all_summaries: Vec<ChatMessage> = (0..5)
            .map(|i| ChatMessage {
                role: "summary".to_string(),
                content: format!("[Context summary of {} messages]", i * 5),
                metadata: None,
                ts: Some(i as i64 * 1000),
            })
            .collect();
        s.replace_messages("conv_summary", all_summaries)
            .await
            .expect("replace ok");

        let err = compress_handler(
            State(state),
            Path("conv_summary".to_string()),
            Extension(claims_for("alice")),
        )
        .await
        .expect_err("already compressed must fail");

        assert!(
            matches!(err, ApiError::InvalidRequest(ref s) if s.contains("already_compressed")),
            "got {:?}",
            err
        );
    }

    // -----------------------------------------------------------------------
    // Test 6: audit entry emitted
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[serial]
    async fn test_compress_emits_audit_entry() {
        reset_store_for_tests().await;
        let tmp = tempfile::TempDir::new().unwrap();
        let sink = StdArc::new(AuditSink::new(tmp.path().join("audit.db")).await.unwrap());

        let mut state = build_test_state();
        StdArc::get_mut(&mut state).expect("unique").audit = Some(sink.clone());

        setup_conversation_with_messages("conv_audit", "alice", 20).await;

        let _ = compress_handler(
            State(state),
            Path("conv_audit".to_string()),
            Extension(claims_for("alice")),
        )
        .await
        .expect("compress ok");

        let entries = sink
            .tail(10, &crate::audit::AuditQuery::default())
            .await
            .unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e.action == "chat.conversation.compressed"),
            "audit entry not found; entries={:?}",
            entries.iter().map(|e| &e.action).collect::<Vec<_>>()
        );
    }

    // -----------------------------------------------------------------------
    // Test 7: admin event emitted
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[serial]
    async fn test_compress_emits_admin_event() {
        reset_store_for_tests().await;
        let state = build_test_state();
        let mut rx = state.admin_event_bus.subscribe();

        setup_conversation_with_messages("conv_event", "alice", 20).await;

        let _ = compress_handler(
            State(state),
            Path("conv_event".to_string()),
            Extension(claims_for("alice")),
        )
        .await
        .expect("compress ok");

        let event = rx.try_recv().expect("admin event must be emitted");
        assert!(
            matches!(event, AdminEvent::ConversationCompressed { .. }),
            "expected ConversationCompressed, got {:?}",
            event
        );
    }

    // -----------------------------------------------------------------------
    // Test 8: conversation unchanged on error (txn safety)
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[serial]
    async fn test_compress_preserves_conversation_on_error() {
        reset_store_for_tests().await;
        let state = build_test_state();

        // 3 messages — will trigger the too_short guard before any write
        setup_conversation_with_messages("conv_txn", "alice", 3).await;

        let _ = compress_handler(
            State(state),
            Path("conv_txn".to_string()),
            Extension(claims_for("alice")),
        )
        .await
        .expect_err("should error");

        // Conversation must still have 3 messages
        let conv = store().get("conv_txn").await.expect("conv exists");
        assert_eq!(
            conv.messages.len(),
            3,
            "messages must be unchanged after failed compress"
        );
    }

    // -----------------------------------------------------------------------
    // Test 9: auto-compress triggers when threshold exceeded
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[serial]
    async fn test_auto_compress_triggers_on_threshold() {
        use crate::api::chat_conversations::{reset_store_for_tests, store, ChatMessage};

        reset_store_for_tests().await;

        // Build a conversation whose total char count exceeds the default
        // threshold (24 000). Each message is ~300 chars; 100 messages = 30 000.
        let s = store();
        s.create_for_test("conv_threshold", "alice").await;
        let msgs: Vec<ChatMessage> = (0..100)
            .map(|i| ChatMessage {
                role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
                // ~300 chars per message
                content: format!("Message {:04}: {}", i, "a".repeat(290)),
                metadata: None,
                ts: Some(i as i64 * 1000),
            })
            .collect();
        s.replace_messages("conv_threshold", msgs)
            .await
            .expect("replace ok");

        let conv = s.get("conv_threshold").await.expect("conv exists");
        assert!(
            should_auto_compress(&conv),
            "conversation exceeding threshold must trigger auto-compress"
        );
    }

    // -----------------------------------------------------------------------
    // Test 10: auto-compress respects cooldown
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[serial]
    async fn test_auto_compress_respects_cooldown() {
        use crate::api::chat_conversations::{reset_store_for_tests, store, ChatMessage};

        reset_store_for_tests().await;

        let s = store();
        s.create_for_test("conv_cooldown", "alice").await;
        // Same large messages as above
        let msgs: Vec<ChatMessage> = (0..100)
            .map(|i| ChatMessage {
                role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
                content: format!("Message {:04}: {}", i, "b".repeat(290)),
                metadata: None,
                ts: Some(i as i64 * 1000),
            })
            .collect();
        s.replace_messages("conv_cooldown", msgs)
            .await
            .expect("replace ok");

        // Simulate a very recent compress (now) — cooldown should suppress.
        s.touch_last_compressed_at("conv_cooldown").await;

        let conv = s.get("conv_cooldown").await.expect("conv exists");
        assert!(
            !should_auto_compress(&conv),
            "conversation compressed just now must not trigger auto-compress (cooldown)"
        );
    }
}
