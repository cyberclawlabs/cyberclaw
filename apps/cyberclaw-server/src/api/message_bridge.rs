//! Sprint 16 Task #1 — Message bridge.
//!
//! Bidirectional conversion between the server's [`ChatMessage`] type and the
//! LLM crate's [`cyberclaw_llm::types::Message`]. The two types evolved
//! independently:
//!
//! - `ChatMessage` (chat_conversations) uses a `role: String` and carries an
//!   optional `ts: i64` timestamp and a freeform `metadata: Option<Value>`.
//! - `cyberclaw_llm::types::Message` uses a typed `Role` enum and is the input
//!   to the context compressor.
//!
//! # Role mapping
//!
//! | chat_conversations role | LLM Role             |
//! |-------------------------|----------------------|
//! | "user"                  | Role::User           |
//! | "assistant"             | Role::Assistant      |
//! | "system"                | Role::System         |
//! | "clarify"               | Role::User (wrapped) |
//! | "clarify_response"      | Role::User           |
//! | "summary"               | *skip* (None)        |
//! | "tool_result"           | Role::Tool           |
//! | (unknown)               | Role::User (default) |

use cyberclaw_llm::types::{Message as LlmMessage, Role};

use crate::api::chat_conversations::ChatMessage;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Convert a single [`ChatMessage`] to an [`LlmMessage`].
///
/// Returns `None` when the message should be skipped entirely (e.g. a
/// `role="summary"` message that was already produced by a previous
/// compression pass and must not be fed back into the compressor).
pub fn chat_to_llm(msg: &ChatMessage) -> Option<LlmMessage> {
    let role = match msg.role.as_str() {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        "system" => Role::System,
        // clarify questions are surfaced as user turns in the LLM context.
        "clarify" | "clarify_response" => Role::User,
        // summary messages are the output of a previous compression pass;
        // skip them so the compressor does not re-compress already-compressed
        // content.
        "summary" => return None,
        "tool_result" => Role::Tool,
        // unknown roles: conservative default to user so we don't drop content.
        _ => Role::User,
    };

    Some(LlmMessage {
        role,
        content: msg.content.clone(),
        tool_calls: None,
        tool_call_id: None,
        name: None,
        cache_control: None,
    })
}

/// Convert an [`LlmMessage`] back to a [`ChatMessage`].
///
/// The `new_id` string is stored as `name` in `metadata` to allow callers to
/// track the chat-layer identifier. The `ts` field is populated with the
/// current time.
pub fn llm_to_chat(msg: &LlmMessage, new_id: String) -> ChatMessage {
    let role = match msg.role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
        Role::Tool => "tool_result",
    }
    .to_string();

    ChatMessage {
        role,
        content: msg.content.clone(),
        metadata: Some(serde_json::json!({ "bridge_id": new_id })),
        ts: Some(chrono::Utc::now().timestamp_millis()),
    }
}

/// Batch-convert a slice of [`ChatMessage`]s to [`LlmMessage`]s, skipping
/// any messages that convert to `None` (e.g. `role="summary"`).
pub fn chat_vec_to_llm(msgs: &[ChatMessage]) -> Vec<LlmMessage> {
    msgs.iter().filter_map(chat_to_llm).collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn chat_msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.to_string(),
            content: content.to_string(),
            metadata: None,
            ts: None,
        }
    }

    #[test]
    fn test_chat_user_to_llm_user_roundtrip() {
        let chat = chat_msg("user", "Hello world");
        let llm = chat_to_llm(&chat).expect("user must convert");
        assert_eq!(llm.role, Role::User);
        assert_eq!(llm.content, "Hello world");

        let back = llm_to_chat(&llm, "id-1".to_string());
        assert_eq!(back.role, "user");
        assert_eq!(back.content, "Hello world");
    }

    #[test]
    fn test_chat_assistant_to_llm_assistant_roundtrip() {
        let chat = chat_msg("assistant", "I can help you.");
        let llm = chat_to_llm(&chat).expect("assistant must convert");
        assert_eq!(llm.role, Role::Assistant);

        let back = llm_to_chat(&llm, "id-2".to_string());
        assert_eq!(back.role, "assistant");
    }

    #[test]
    fn test_chat_system_to_llm_system_roundtrip() {
        let chat = chat_msg("system", "You are an agent.");
        let llm = chat_to_llm(&chat).expect("system must convert");
        assert_eq!(llm.role, Role::System);

        let back = llm_to_chat(&llm, "id-3".to_string());
        assert_eq!(back.role, "system");
    }

    #[test]
    fn test_chat_clarify_wraps_as_user() {
        let chat = chat_msg("clarify", "What do you mean?");
        let llm = chat_to_llm(&chat).expect("clarify must convert");
        assert_eq!(llm.role, Role::User, "clarify maps to User");
        assert_eq!(llm.content, "What do you mean?");

        let cr = chat_msg("clarify_response", "The second one.");
        let llm2 = chat_to_llm(&cr).expect("clarify_response must convert");
        assert_eq!(llm2.role, Role::User);
    }

    #[test]
    fn test_chat_summary_returns_none() {
        let chat = chat_msg("summary", "[Context summary of 10 earlier messages]");
        let result = chat_to_llm(&chat);
        assert!(result.is_none(), "summary role must be skipped");
    }

    #[test]
    fn test_chat_tool_result_to_llm_tool() {
        let chat = chat_msg("tool_result", "output: 42");
        let llm = chat_to_llm(&chat).expect("tool_result must convert");
        assert_eq!(llm.role, Role::Tool);
        assert_eq!(llm.content, "output: 42");
    }

    #[test]
    fn test_chat_unknown_role_defaults_to_user() {
        let chat = chat_msg("someunknownrole", "content");
        let llm = chat_to_llm(&chat).expect("unknown role must convert as user");
        assert_eq!(llm.role, Role::User);
    }

    #[test]
    fn test_chat_vec_to_llm_skips_summary() {
        let msgs = vec![
            chat_msg("user", "Hello"),
            chat_msg("summary", "[Context summary of 5 earlier messages]"),
            chat_msg("assistant", "Hi there"),
        ];
        let llm_msgs = chat_vec_to_llm(&msgs);
        assert_eq!(llm_msgs.len(), 2);
        assert_eq!(llm_msgs[0].role, Role::User);
        assert_eq!(llm_msgs[1].role, Role::Assistant);
    }

    #[test]
    fn test_llm_to_chat_sets_ts() {
        let llm = LlmMessage::user("test");
        let chat = llm_to_chat(&llm, "bridge-id".to_string());
        assert!(chat.ts.is_some(), "ts must be set by llm_to_chat");
    }
}
