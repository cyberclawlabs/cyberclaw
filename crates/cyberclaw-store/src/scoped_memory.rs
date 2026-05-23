//! # ScopedMemory — sliding-window full retention + compressed tail
//!
//! Sprint R4 (v1.x optimization). Closes the multi-turn context-loss gap
//! discovered in baseline H-L2/L3 (cb 58–62 % accuracy vs hm 73–75 %): the
//! 4-stage `ContextCompressor` was too aggressive on short 5-turn
//! sequences, replacing early turns with the synthetic string
//! "[Context summary of N earlier messages]\n…" which dropped key facts
//! the user had provided in turn 1.
//!
//! `ScopedMemory` separates retention into two tiers:
//!
//! 1. **Raw turns** — the most recent `K` turns are kept verbatim. The LLM
//!    sees the exact user/assistant message pairs without summarization.
//! 2. **Compressed tail** — turns older than the K-window are folded into
//!    a deterministic compact representation (`CompressedTurn`) that
//!    preserves role + a truncated content snippet, so the older context
//!    is still discoverable but cheap.
//!
//! ## The K parameter
//!
//! - General-purpose conversations: `K = 3` (matches typical chat
//!   "remember last few turns" expectation).
//! - H-class (high-recall) conversations: `K = 10`, configurable per
//!   conversation by the caller.
//!
//! Conversations with fewer than `K` turns hold everything as raw, so this
//! module degrades gracefully to "full retention".
//!
//! ## Module-level invariants
//!
//! - `raw_turns.len() <= K` always.
//! - Total turns in `compressed_window + raw_turns` equals every turn ever
//!   pushed (no turn is dropped silently).
//! - `render_context()` outputs in chronological order:
//!   `[compressed_tail block?, raw_turn_0, raw_turn_1, ...]`.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Turn / Role
// ---------------------------------------------------------------------------

/// Conversational role of a turn. Mirrors `cyberclaw_llm::types::Role` but
/// kept local so this crate does not depend on `cyberclaw-llm`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnRole {
    /// System / instruction message.
    System,
    /// End-user message.
    User,
    /// Assistant / model reply.
    Assistant,
    /// Tool call result.
    Tool,
}

/// A single conversation turn. Content is held verbatim while in the
/// raw window; on eviction it becomes a [`CompressedTurn`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    /// Role of the speaker.
    pub role: TurnRole,
    /// Verbatim text content of the turn.
    pub content: String,
}

impl Turn {
    /// Create a new turn.
    pub fn new(role: TurnRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }

    /// Convenience constructor for a user turn.
    pub fn user(content: impl Into<String>) -> Self {
        Self::new(TurnRole::User, content)
    }

    /// Convenience constructor for an assistant turn.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new(TurnRole::Assistant, content)
    }

    /// Convenience constructor for a system turn.
    pub fn system(content: impl Into<String>) -> Self {
        Self::new(TurnRole::System, content)
    }

    /// Convenience constructor for a tool turn.
    pub fn tool(content: impl Into<String>) -> Self {
        Self::new(TurnRole::Tool, content)
    }
}

// ---------------------------------------------------------------------------
// CompressedTurn
// ---------------------------------------------------------------------------

/// Compact representation of a turn that has been evicted from the raw
/// window. Used by [`ScopedMemory::render_context`] to build the older-
/// context block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompressedTurn {
    /// Role of the original turn.
    pub role: TurnRole,
    /// Truncated content snippet (preserves first
    /// [`ScopedMemory::compressed_content_chars`] characters).
    pub content_snippet: String,
    /// Original full length in characters — useful for telemetry.
    pub original_chars: usize,
}

impl CompressedTurn {
    /// Build a compressed turn from a raw turn by truncating to `chars`.
    pub fn from_turn(turn: &Turn, chars: usize) -> Self {
        let original_chars = turn.content.chars().count();
        let content_snippet: String = if original_chars <= chars {
            turn.content.clone()
        } else {
            turn.content.chars().take(chars).collect()
        };
        Self {
            role: turn.role,
            content_snippet,
            original_chars,
        }
    }
}

// ---------------------------------------------------------------------------
// MessageBlock
// ---------------------------------------------------------------------------

/// Output type of [`ScopedMemory::render_context`]. Designed to be cheap
/// to convert into `cyberclaw_llm::types::Message` at the call site (this
/// crate intentionally avoids the dependency).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageBlock {
    /// Role of the rendered block.
    pub role: TurnRole,
    /// Content of the rendered block.
    pub content: String,
}

impl MessageBlock {
    /// Convenience constructor.
    pub fn new(role: TurnRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// ScopedMemory
// ---------------------------------------------------------------------------

/// Sliding-window full retention + compressed tail conversation memory.
///
/// See the module-level docs for the K semantics.
#[derive(Debug, Clone)]
pub struct ScopedMemory {
    /// How many most-recent turns to keep raw (full retention).
    full_retention_turns: usize,
    /// How many characters of each evicted turn to keep as a snippet.
    /// Default: `200`.
    compressed_content_chars: usize,
    /// Older-than-K turns folded into compact form.
    compressed_window: Vec<CompressedTurn>,
    /// Most-recent ≤ K turns held verbatim.
    raw_turns: VecDeque<Turn>,
}

impl ScopedMemory {
    /// Create a new memory with the given full-retention window size.
    ///
    /// `full_retention_turns = 0` would collapse every turn straight into
    /// the compressed tail; we coerce that to `1` to guarantee the LLM
    /// always sees at least the most recent turn raw.
    pub fn new(full_retention_turns: usize) -> Self {
        let k = full_retention_turns.max(1);
        Self {
            full_retention_turns: k,
            compressed_content_chars: 200,
            compressed_window: Vec::new(),
            raw_turns: VecDeque::with_capacity(k),
        }
    }

    /// Builder: override the per-evicted-turn snippet length. Larger
    /// values keep more older context at the cost of token spend.
    pub fn with_compressed_content_chars(mut self, chars: usize) -> Self {
        self.compressed_content_chars = chars.max(1);
        self
    }

    /// Current value of the K window.
    pub fn full_retention_turns(&self) -> usize {
        self.full_retention_turns
    }

    /// Number of compressed (evicted) turns currently retained.
    pub fn compressed_len(&self) -> usize {
        self.compressed_window.len()
    }

    /// Number of raw turns currently retained.
    pub fn raw_len(&self) -> usize {
        self.raw_turns.len()
    }

    /// Total turns ever pushed.
    pub fn total_turns(&self) -> usize {
        self.compressed_window.len() + self.raw_turns.len()
    }

    /// Read-only view of the compressed window. Older turns first.
    pub fn compressed_window(&self) -> &[CompressedTurn] {
        &self.compressed_window
    }

    /// Read-only iterator over the raw window, oldest first.
    pub fn raw_turns(&self) -> impl Iterator<Item = &Turn> {
        self.raw_turns.iter()
    }

    /// Push a new turn into the memory.
    ///
    /// If the raw window already holds `K` turns, the oldest is compressed
    /// and moved to the compressed tail.
    pub fn push(&mut self, turn: Turn) {
        if self.raw_turns.len() >= self.full_retention_turns {
            // Evict the oldest raw turn into the compressed tail.
            if let Some(evicted) = self.raw_turns.pop_front() {
                let compressed =
                    CompressedTurn::from_turn(&evicted, self.compressed_content_chars);
                self.compressed_window.push(compressed);
            }
        }
        self.raw_turns.push_back(turn);
    }

    /// Reset everything.
    pub fn clear(&mut self) {
        self.compressed_window.clear();
        self.raw_turns.clear();
    }

    /// Render the memory as an LLM-ready ordered sequence of message
    /// blocks.
    ///
    /// ## Algorithm
    ///
    /// 1. If `compressed_window` is non-empty, emit ONE synthetic system
    ///    block summarising it as `"[Earlier context, N turns]\n- [Role]
    ///    snippet…\n- [Role] snippet…"`. We use a single block (not one
    ///    per evicted turn) to minimize LLM prompt overhead.
    /// 2. Then emit each raw turn as its own block, in chronological
    ///    order. The LLM sees these as if they were the live tail of the
    ///    conversation.
    ///
    /// The synthetic block is omitted entirely when nothing has been
    /// evicted yet, so short conversations match what the LLM would have
    /// seen without `ScopedMemory` at all.
    pub fn render_context(&self) -> Vec<MessageBlock> {
        let mut out: Vec<MessageBlock> = Vec::with_capacity(self.raw_turns.len() + 1);

        if !self.compressed_window.is_empty() {
            let mut body = String::new();
            body.push_str(&format!(
                "[Earlier context, {} turns]",
                self.compressed_window.len()
            ));
            for ct in &self.compressed_window {
                body.push('\n');
                let role_label = match ct.role {
                    TurnRole::System => "System",
                    TurnRole::User => "User",
                    TurnRole::Assistant => "Assistant",
                    TurnRole::Tool => "Tool",
                };
                let truncated_marker = if ct.original_chars > ct.content_snippet.chars().count() {
                    "…"
                } else {
                    ""
                };
                body.push_str(&format!(
                    "- [{}] {}{}",
                    role_label, ct.content_snippet, truncated_marker
                ));
            }
            out.push(MessageBlock::new(TurnRole::System, body));
        }

        for turn in &self.raw_turns {
            out.push(MessageBlock::new(turn.role, turn.content.clone()));
        }

        out
    }
}

impl Default for ScopedMemory {
    /// Default K = 3 (general-purpose conversation).
    fn default() -> Self {
        Self::new(3)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_with_zero_k_coerces_to_one() {
        let m = ScopedMemory::new(0);
        assert_eq!(m.full_retention_turns(), 1);
    }

    #[test]
    fn default_uses_k_three() {
        let m = ScopedMemory::default();
        assert_eq!(m.full_retention_turns(), 3);
    }

    #[test]
    fn push_keeps_all_turns_when_below_k() {
        let mut m = ScopedMemory::new(5);
        m.push(Turn::user("turn 0"));
        m.push(Turn::assistant("turn 1"));
        m.push(Turn::user("turn 2"));

        assert_eq!(m.raw_len(), 3);
        assert_eq!(m.compressed_len(), 0);
        assert_eq!(m.total_turns(), 3);
    }

    #[test]
    fn push_evicts_oldest_when_above_k() {
        let mut m = ScopedMemory::new(3);
        m.push(Turn::user("u0"));
        m.push(Turn::assistant("a0"));
        m.push(Turn::user("u1"));
        // Above K: evict u0.
        m.push(Turn::assistant("a1"));

        assert_eq!(m.raw_len(), 3);
        assert_eq!(m.compressed_len(), 1);
        assert_eq!(m.compressed_window()[0].role, TurnRole::User);
        assert!(m.compressed_window()[0].content_snippet.starts_with("u0"));
    }

    #[test]
    fn push_k_plus_5_yields_k_raw_and_5_compressed() {
        // The acceptance criterion from the task: push K + 5 turns,
        // render_context() should return K full + 5 compressed (folded into
        // one synthetic block).
        let k = 5;
        let mut m = ScopedMemory::new(k);
        for i in 0..(k + 5) {
            m.push(Turn::user(format!("turn-{i}")));
        }
        assert_eq!(m.raw_len(), k);
        assert_eq!(m.compressed_len(), 5);
        assert_eq!(m.total_turns(), k + 5);

        let rendered = m.render_context();
        // 1 synthetic block + K raw turns.
        assert_eq!(rendered.len(), k + 1);
        // First block is the synthetic summary system message.
        assert_eq!(rendered[0].role, TurnRole::System);
        assert!(rendered[0].content.starts_with("[Earlier context, 5 turns]"));
        // The 5 earliest "turn-0..turn-4" should be in the synthetic body.
        for i in 0..5 {
            assert!(
                rendered[0].content.contains(&format!("turn-{i}")),
                "synthetic block must reference turn-{i}: {}",
                rendered[0].content
            );
        }
        // The K raw blocks must be the latest K turns in order.
        for i in 0..k {
            let expected = format!("turn-{}", 5 + i);
            assert_eq!(rendered[1 + i].content, expected);
            assert_eq!(rendered[1 + i].role, TurnRole::User);
        }
    }

    #[test]
    fn render_no_synthetic_block_when_below_k() {
        let mut m = ScopedMemory::new(3);
        m.push(Turn::user("hi"));
        let r = m.render_context();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].role, TurnRole::User);
        assert_eq!(r[0].content, "hi");
    }

    #[test]
    fn render_preserves_chronological_order() {
        let mut m = ScopedMemory::new(2);
        m.push(Turn::user("u0"));
        m.push(Turn::assistant("a0"));
        m.push(Turn::user("u1"));
        m.push(Turn::assistant("a1"));

        let r = m.render_context();
        // 1 synthetic + 2 raw.
        assert_eq!(r.len(), 3);
        // Raw window should be u1, a1 in that order.
        assert_eq!(r[1].content, "u1");
        assert_eq!(r[1].role, TurnRole::User);
        assert_eq!(r[2].content, "a1");
        assert_eq!(r[2].role, TurnRole::Assistant);
    }

    #[test]
    fn compressed_snippet_truncates_long_content() {
        let m = ScopedMemory::new(3).with_compressed_content_chars(10);
        let long = "a".repeat(100);
        let turn = Turn::user(long.clone());
        let ct = CompressedTurn::from_turn(&turn, 10);
        assert_eq!(ct.original_chars, 100);
        assert_eq!(ct.content_snippet.chars().count(), 10);

        // Sanity-check the builder threading too.
        let mut m2 = m.clone();
        m2.push(Turn::user(long.clone()));
        m2.push(Turn::assistant("a"));
        m2.push(Turn::user("u"));
        m2.push(Turn::assistant("a2")); // evicts the long turn
        let r = m2.render_context();
        assert!(r[0].content.contains("…"));
    }

    #[test]
    fn h_class_k10_keeps_ten_raw() {
        // H-class conversations should not lose context within first 10
        // turns even after many later turns.
        let mut m = ScopedMemory::new(10);
        for i in 0..15 {
            m.push(Turn::user(format!("h-turn-{i}")));
        }
        assert_eq!(m.raw_len(), 10);
        assert_eq!(m.compressed_len(), 5);

        let r = m.render_context();
        // K=10 raw turns + 1 synthetic.
        assert_eq!(r.len(), 11);
        // Most recent raw turn is the last block.
        assert_eq!(r.last().unwrap().content, "h-turn-14");
    }

    #[test]
    fn clear_resets_state() {
        let mut m = ScopedMemory::new(2);
        m.push(Turn::user("a"));
        m.push(Turn::user("b"));
        m.push(Turn::user("c"));
        assert_eq!(m.total_turns(), 3);

        m.clear();
        assert_eq!(m.total_turns(), 0);
        assert_eq!(m.raw_len(), 0);
        assert_eq!(m.compressed_len(), 0);
        assert!(m.render_context().is_empty());
    }

    #[test]
    fn role_round_trips_through_compression() {
        let mut m = ScopedMemory::new(1);
        m.push(Turn::system("sys"));
        m.push(Turn::user("u")); // evicts sys
        let r = m.render_context();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].role, TurnRole::System);
        assert!(r[0].content.contains("[System]"));
        assert_eq!(r[1].role, TurnRole::User);
        assert_eq!(r[1].content, "u");
    }

    #[test]
    fn iter_raw_turns_returns_chronological() {
        let mut m = ScopedMemory::new(3);
        m.push(Turn::user("0"));
        m.push(Turn::user("1"));
        m.push(Turn::user("2"));
        let v: Vec<&str> = m.raw_turns().map(|t| t.content.as_str()).collect();
        assert_eq!(v, vec!["0", "1", "2"]);
    }

    #[test]
    fn compressed_window_omits_when_empty() {
        let m = ScopedMemory::new(5);
        assert!(m.compressed_window().is_empty());
        assert!(m.render_context().is_empty());
    }

    #[test]
    fn snippet_marker_omitted_when_under_limit() {
        let mut m = ScopedMemory::new(1).with_compressed_content_chars(200);
        m.push(Turn::user("short"));
        m.push(Turn::user("next")); // evict "short"
        let r = m.render_context();
        // The full "short" content fits, so no trailing ellipsis.
        assert!(r[0].content.contains("- [User] short"));
        assert!(!r[0].content.contains("- [User] short…"));
    }
}
