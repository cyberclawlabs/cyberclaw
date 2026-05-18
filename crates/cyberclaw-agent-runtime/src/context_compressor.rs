//! # Context Compressor
//!
//! Four-stage context compression for long-running agentic loops.
//!
//! When token usage exceeds a configurable threshold, the compressor trims
//! accumulated messages through four ordered stages:
//!
//! 1. **PruneToolResults** - Remove old tool call results, keeping the last N.
//! 2. **SummarizeEarly** - Replace early conversation turns with an LLM summary.
//! 3. **HideSystemDetails** - Strip verbose details from system prompts.
//! 4. **SlidingWindow** - Hard-truncate to the last N messages.
//!
//! A circuit breaker stops compression attempts after consecutive failures.

use serde::{Deserialize, Serialize};

use crate::agentic_loop::{IterationBudget, LoopState};
use cyberclaw_llm::types::{Message, Role};

// ---------------------------------------------------------------------------
// CompressionStage
// ---------------------------------------------------------------------------

/// The four compression stages, applied in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CompressionStage {
    /// Stage 1: Remove old tool call result messages, keeping the last N.
    PruneToolResults,
    /// Stage 2: Replace early conversation turns with an LLM-generated summary.
    SummarizeEarly,
    /// Stage 3: Strip verbose details from system prompt messages.
    HideSystemDetails,
    /// Stage 4: Hard-truncate to the last N messages (sliding window).
    SlidingWindow,
}

impl CompressionStage {
    /// Return all stages in their canonical execution order.
    pub fn all_ordered() -> &'static [CompressionStage] {
        &[
            CompressionStage::PruneToolResults,
            CompressionStage::SummarizeEarly,
            CompressionStage::HideSystemDetails,
            CompressionStage::SlidingWindow,
        ]
    }
}

// ---------------------------------------------------------------------------
// MemoryLevel
// ---------------------------------------------------------------------------

/// Memory tiering levels for context management.
///
/// - `L0` — Full conversation context (current session).
/// - `L1` — Key-information summary produced by an LLM pass.
/// - `L2` — Structured metadata extracted as JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum MemoryLevel {
    /// Full context (current conversation).
    L0 = 0,
    /// Key info summary (LLM-generated).
    L1 = 1,
    /// Structured metadata (JSON extraction).
    L2 = 2,
}

// ---------------------------------------------------------------------------
// CompressionConfig
// ---------------------------------------------------------------------------

/// Configuration for the context compressor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionConfig {
    /// Token usage ratio (consumed / budget) that triggers compression.
    /// Default: `0.8` (80%).
    pub trigger_threshold: f64,
    /// Number of recent tool-result messages to keep during pruning.
    /// Default: `10`.
    pub tool_result_keep_count: usize,
    /// Maximum tokens allowed for the LLM-generated summary (stage 2).
    /// Default: `500`.
    pub summary_max_tokens: u32,
    /// Number of most-recent messages to keep in the sliding window.
    /// Default: `50`.
    pub sliding_window_size: usize,
    /// After this many consecutive compression failures, stop trying.
    /// Default: `3`.
    pub max_consecutive_failures: u32,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            trigger_threshold: 0.8,
            tool_result_keep_count: 10,
            summary_max_tokens: 500,
            sliding_window_size: 50,
            max_consecutive_failures: 3,
        }
    }
}

// ---------------------------------------------------------------------------
// CompressedResult
// ---------------------------------------------------------------------------

/// Result of a compression pass.
#[derive(Debug, Clone)]
pub struct CompressedResult {
    /// The compressed message list.
    pub messages: Vec<Message>,
    /// Number of messages before compression.
    pub original_count: usize,
    /// Number of messages after compression.
    pub compressed_count: usize,
    /// Which stages were applied.
    pub stages_applied: Vec<CompressionStage>,
    /// Estimated tokens saved (character-based heuristic: chars / 4).
    pub tokens_saved_estimate: u64,
}

// ---------------------------------------------------------------------------
// ContextCompressor
// ---------------------------------------------------------------------------

/// Four-stage context compressor with circuit breaker.
#[derive(Debug)]
pub struct ContextCompressor {
    config: CompressionConfig,
    consecutive_failures: u32,
}

impl ContextCompressor {
    /// Create a new compressor with the given configuration.
    pub fn new(config: CompressionConfig) -> Self {
        Self {
            config,
            consecutive_failures: 0,
        }
    }

    /// Check whether compression should be triggered based on current loop
    /// state and iteration budget.
    ///
    /// Returns `true` when the token usage ratio exceeds the configured
    /// threshold and the circuit breaker has not tripped.
    pub fn should_compress(&self, state: &LoopState, budget: &IterationBudget) -> bool {
        // Circuit breaker: stop if we've failed too many times in a row.
        if self.consecutive_failures >= self.config.max_consecutive_failures {
            return false;
        }

        // If max_tokens is 0 (unlimited), we cannot compute a ratio.
        if budget.max_tokens == 0 {
            return false;
        }

        let ratio = state.tokens_consumed as f64 / budget.max_tokens as f64;
        ratio >= self.config.trigger_threshold
    }

    /// Execute a single compression stage on the provided messages.
    pub fn compress(&mut self, messages: &[Message], stage: CompressionStage) -> CompressedResult {
        let original_count = messages.len();
        let original_chars: usize = messages.iter().map(|m| m.content.len()).sum();

        let compressed = match stage {
            CompressionStage::PruneToolResults => {
                self.prune_tool_results(messages, self.config.tool_result_keep_count)
            }
            CompressionStage::SummarizeEarly => {
                self.summarize_early(messages, self.config.summary_max_tokens)
            }
            CompressionStage::HideSystemDetails => self.hide_system_details(messages),
            CompressionStage::SlidingWindow => {
                self.sliding_window(messages, self.config.sliding_window_size)
            }
        };

        let compressed_chars: usize = compressed.iter().map(|m| m.content.len()).sum();
        let chars_saved = original_chars.saturating_sub(compressed_chars);
        // Rough token estimate: ~4 characters per token.
        let tokens_saved_estimate = (chars_saved / 4) as u64;

        let compressed_count = compressed.len();

        if compressed_count <= original_count {
            self.consecutive_failures = 0;
        } else {
            // Should not happen, but treat as a failure.
            self.consecutive_failures += 1;
        }

        CompressedResult {
            messages: compressed,
            original_count,
            compressed_count,
            stages_applied: vec![stage],
            tokens_saved_estimate,
        }
    }

    /// Run all four compression stages in order.
    ///
    /// If the circuit breaker has tripped, returns the original messages
    /// unchanged.
    pub fn compress_all(&mut self, messages: &[Message]) -> CompressedResult {
        let original_count = messages.len();
        let original_chars: usize = messages.iter().map(|m| m.content.len()).sum();

        if self.consecutive_failures >= self.config.max_consecutive_failures {
            return CompressedResult {
                messages: messages.to_vec(),
                original_count,
                compressed_count: original_count,
                stages_applied: vec![],
                tokens_saved_estimate: 0,
            };
        }

        let mut current = messages.to_vec();
        let mut stages_applied = Vec::new();

        for &stage in CompressionStage::all_ordered() {
            let before_len = current.len();
            current = match stage {
                CompressionStage::PruneToolResults => {
                    self.prune_tool_results(&current, self.config.tool_result_keep_count)
                }
                CompressionStage::SummarizeEarly => {
                    self.summarize_early(&current, self.config.summary_max_tokens)
                }
                CompressionStage::HideSystemDetails => self.hide_system_details(&current),
                CompressionStage::SlidingWindow => {
                    self.sliding_window(&current, self.config.sliding_window_size)
                }
            };
            // Record stage if it actually changed something.
            if current.len() != before_len {
                stages_applied.push(stage);
            }
        }

        let compressed_chars: usize = current.iter().map(|m| m.content.len()).sum();
        let chars_saved = original_chars.saturating_sub(compressed_chars);
        let tokens_saved_estimate = (chars_saved / 4) as u64;
        let compressed_count = current.len();

        if compressed_count <= original_count {
            self.consecutive_failures = 0;
        } else {
            self.consecutive_failures += 1;
        }

        CompressedResult {
            messages: current,
            original_count,
            compressed_count,
            stages_applied,
            tokens_saved_estimate,
        }
    }

    /// **Stage 1**: Prune old tool-result messages, keeping only the last
    /// `keep` tool results. System, user, and assistant messages are preserved.
    pub fn prune_tool_results(&self, messages: &[Message], keep: usize) -> Vec<Message> {
        // Find indices of all Tool-role messages.
        let tool_indices: Vec<usize> = messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.role == Role::Tool)
            .map(|(i, _)| i)
            .collect();

        if tool_indices.len() <= keep {
            return messages.to_vec();
        }

        // Indices to remove: all tool messages except the last `keep`.
        let remove_count = tool_indices.len() - keep;
        let remove_set: std::collections::HashSet<usize> =
            tool_indices[..remove_count].iter().copied().collect();

        messages
            .iter()
            .enumerate()
            .filter(|(i, _)| !remove_set.contains(i))
            .map(|(_, m)| m.clone())
            .collect()
    }

    /// **Stage 2**: Summarize early conversation turns.
    ///
    /// In production this would call an LLM to produce a summary. The current
    /// implementation performs a deterministic in-process summarization:
    /// it keeps the first message (usually the system prompt), replaces the
    /// early user/assistant turns with a single synthetic summary message,
    /// and keeps the most recent half of the conversation intact.
    pub fn summarize_early(&self, messages: &[Message], max_tokens: u32) -> Vec<Message> {
        if messages.len() <= 4 {
            return messages.to_vec();
        }

        // Keep the first message (system prompt) and the last half.
        let keep_tail = messages.len() / 2;
        let early_end = messages.len() - keep_tail;

        // Build a brief summary of the early turns (skip index 0 = system).
        let early_start = if messages
            .first()
            .map(|m| m.role == Role::System)
            .unwrap_or(false)
        {
            1
        } else {
            0
        };

        if early_start >= early_end {
            return messages.to_vec();
        }

        let mut summary_parts: Vec<String> = Vec::new();
        let char_budget = (max_tokens as usize) * 4; // rough chars budget

        let mut chars_used: usize = 0;
        for msg in &messages[early_start..early_end] {
            let role_label = match msg.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
                Role::Tool => "Tool",
                Role::System => "System",
            };
            // Truncate individual message content to avoid blowup.
            let truncated: String = msg.content.chars().take(120).collect();
            let part = format!("[{role_label}] {truncated}");
            chars_used += part.len();
            if chars_used > char_budget {
                summary_parts.push("[...earlier turns omitted...]".to_string());
                break;
            }
            summary_parts.push(part);
        }

        let summary_text = format!(
            "[Context summary of {} earlier messages]\n{}",
            early_end - early_start,
            summary_parts.join("\n")
        );

        let mut result = Vec::with_capacity(2 + keep_tail);
        // Keep system message if present.
        if early_start == 1 {
            result.push(messages[0].clone());
        }
        // Insert synthetic summary as a system message.
        result.push(Message::system(summary_text));
        // Append the recent tail.
        result.extend_from_slice(&messages[messages.len() - keep_tail..]);

        result
    }

    /// **Stage 3**: Strip verbose details from system prompt messages.
    ///
    /// Replaces system messages longer than 500 characters with a truncated
    /// version, keeping only the first 500 characters plus an ellipsis.
    /// The synthetic summary messages produced by stage 2 are preserved.
    pub fn hide_system_details(&self, messages: &[Message]) -> Vec<Message> {
        const MAX_SYSTEM_CHARS: usize = 500;

        messages
            .iter()
            .map(|m| {
                if m.role == Role::System
                    && m.content.len() > MAX_SYSTEM_CHARS
                    && !m.content.starts_with("[Context summary")
                {
                    let truncated: String = m.content.chars().take(MAX_SYSTEM_CHARS).collect();
                    Message::system(format!("{truncated}..."))
                } else {
                    m.clone()
                }
            })
            .collect()
    }

    /// **Stage 4**: Hard-truncate to the last `size` messages.
    ///
    /// Always preserves the first message if it is a system prompt, so the
    /// agent retains its instructions.
    pub fn sliding_window(&self, messages: &[Message], size: usize) -> Vec<Message> {
        if messages.len() <= size {
            return messages.to_vec();
        }

        if size == 0 {
            return vec![];
        }

        // Preserve the system prompt (first message) if present.
        let has_system = messages
            .first()
            .map(|m| m.role == Role::System)
            .unwrap_or(false);

        if has_system && size >= 2 {
            let mut result = Vec::with_capacity(size);
            result.push(messages[0].clone());
            let tail_count = size - 1;
            let start = messages.len().saturating_sub(tail_count);
            result.extend_from_slice(&messages[start..]);
            result
        } else {
            let start = messages.len().saturating_sub(size);
            messages[start..].to_vec()
        }
    }

    /// Record an external compression failure (e.g. an LLM summary call that
    /// timed out). Increments the consecutive failure counter.
    pub fn record_failure(&mut self) {
        self.consecutive_failures += 1;
    }

    /// Reset the circuit breaker.
    pub fn reset_circuit_breaker(&mut self) {
        self.consecutive_failures = 0;
    }

    /// Return the current consecutive failure count.
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agentic_loop::IterationBudget;

    /// Helper: build a LoopState with the given token count.
    fn make_state(tokens: u64) -> LoopState {
        LoopState {
            messages: vec![],
            iteration_count: 0,
            tokens_consumed: tokens,
        }
    }

    /// Helper: build a budget with the given max tokens.
    fn make_budget(max_tokens: u64) -> IterationBudget {
        IterationBudget {
            max_tokens,
            ..Default::default()
        }
    }

    // -- 1. should_compress trigger threshold ---------------------------------

    #[test]
    fn test_should_compress_trigger_threshold() {
        let compressor = ContextCompressor::new(CompressionConfig {
            trigger_threshold: 0.8,
            ..Default::default()
        });

        // 79% usage -> should NOT trigger.
        assert!(!compressor.should_compress(&make_state(79), &make_budget(100)));
        // 80% usage -> should trigger.
        assert!(compressor.should_compress(&make_state(80), &make_budget(100)));
        // 100% usage -> should trigger.
        assert!(compressor.should_compress(&make_state(100), &make_budget(100)));
    }

    #[test]
    fn test_should_compress_unlimited_budget() {
        let compressor = ContextCompressor::new(Default::default());
        // max_tokens = 0 means unlimited; cannot compute ratio.
        assert!(!compressor.should_compress(&make_state(999), &make_budget(0)));
    }

    // -- 2. prune_tool_results keeps last N -----------------------------------

    #[test]
    fn test_prune_tool_results_keeps_last_n() {
        let compressor = ContextCompressor::new(Default::default());

        let messages = vec![
            Message::system("You are an agent."),
            Message::user("Do something"),
            Message::tool("t1".into(), "result 1"),
            Message::assistant("Calling tool"),
            Message::tool("t2".into(), "result 2"),
            Message::assistant("Calling tool"),
            Message::tool("t3".into(), "result 3"),
            Message::assistant("Calling tool"),
            Message::tool("t4".into(), "result 4"),
            Message::user("Continue"),
        ];

        let pruned = compressor.prune_tool_results(&messages, 2);

        // Should keep the last 2 tool messages (t3, t4), remove t1 and t2.
        let tool_contents: Vec<&str> = pruned
            .iter()
            .filter(|m| m.role == Role::Tool)
            .map(|m| m.content.as_str())
            .collect();
        assert_eq!(tool_contents, vec!["result 3", "result 4"]);

        // Non-tool messages should all be preserved.
        let non_tool_count = pruned.iter().filter(|m| m.role != Role::Tool).count();
        assert_eq!(non_tool_count, 6); // system + 2 user + 3 assistant
    }

    #[test]
    fn test_prune_tool_results_fewer_than_keep() {
        let compressor = ContextCompressor::new(Default::default());
        let messages = vec![Message::user("Hello"), Message::tool("t1".into(), "result")];
        let pruned = compressor.prune_tool_results(&messages, 10);
        assert_eq!(pruned.len(), 2); // nothing removed
    }

    // -- 3. sliding_window truncation -----------------------------------------

    #[test]
    fn test_sliding_window_truncation() {
        let compressor = ContextCompressor::new(Default::default());

        let messages: Vec<Message> = (0..20)
            .map(|i| Message::user(format!("Message {i}")))
            .collect();

        let windowed = compressor.sliding_window(&messages, 5);
        assert_eq!(windowed.len(), 5);
        assert_eq!(windowed[0].content, "Message 15");
        assert_eq!(windowed[4].content, "Message 19");
    }

    #[test]
    fn test_sliding_window_preserves_system() {
        let compressor = ContextCompressor::new(Default::default());

        let mut messages = vec![Message::system("System prompt")];
        for i in 0..10 {
            messages.push(Message::user(format!("msg {i}")));
        }

        let windowed = compressor.sliding_window(&messages, 4);
        assert_eq!(windowed.len(), 4);
        assert_eq!(windowed[0].role, Role::System);
        assert_eq!(windowed[0].content, "System prompt");
        // Last 3 user messages.
        assert_eq!(windowed[3].content, "msg 9");
    }

    #[test]
    fn test_sliding_window_no_truncation_needed() {
        let compressor = ContextCompressor::new(Default::default());
        let messages = vec![Message::user("a"), Message::user("b")];
        let windowed = compressor.sliding_window(&messages, 10);
        assert_eq!(windowed.len(), 2);
    }

    // -- 4. compress_all runs all stages --------------------------------------

    #[test]
    fn test_compress_all_runs_all_stages() {
        let mut compressor = ContextCompressor::new(CompressionConfig {
            tool_result_keep_count: 1,
            sliding_window_size: 5,
            ..Default::default()
        });

        let mut messages = vec![Message::system(
            "You are an agent with a very detailed system prompt.",
        )];
        for i in 0..15 {
            messages.push(Message::user(format!("Turn {i}")));
            messages.push(Message::tool(format!("t{i}"), format!("result {i}")));
            messages.push(Message::assistant(format!("Response {i}")));
        }

        let result = compressor.compress_all(&messages);

        assert!(result.compressed_count < result.original_count);
        assert!(!result.stages_applied.is_empty());
        // Sliding window ensures at most 5 messages.
        assert!(result.messages.len() <= 5);
    }

    // -- 5. circuit breaker after N failures ----------------------------------

    #[test]
    fn test_circuit_breaker_stops_compression() {
        let mut compressor = ContextCompressor::new(CompressionConfig {
            max_consecutive_failures: 3,
            ..Default::default()
        });

        // Simulate 3 consecutive failures.
        compressor.record_failure();
        compressor.record_failure();
        compressor.record_failure();

        // should_compress should return false.
        assert!(!compressor.should_compress(&make_state(100), &make_budget(100)));

        // compress_all should return original messages unchanged.
        let messages = vec![Message::user("hello"), Message::user("world")];
        let result = compressor.compress_all(&messages);
        assert_eq!(result.messages.len(), 2);
        assert!(result.stages_applied.is_empty());
        assert_eq!(result.tokens_saved_estimate, 0);
    }

    #[test]
    fn test_circuit_breaker_resets_on_success() {
        let mut compressor = ContextCompressor::new(CompressionConfig {
            max_consecutive_failures: 3,
            tool_result_keep_count: 1,
            ..Default::default()
        });

        compressor.record_failure();
        compressor.record_failure();
        assert_eq!(compressor.consecutive_failures(), 2);

        // A successful compress resets the counter.
        let messages = vec![
            Message::user("a"),
            Message::tool("t1".into(), "r1"),
            Message::tool("t2".into(), "r2"),
            Message::tool("t3".into(), "r3"),
        ];
        let _result = compressor.compress(&messages, CompressionStage::PruneToolResults);
        assert_eq!(compressor.consecutive_failures(), 0);
    }

    // -- 6. empty messages returns empty --------------------------------------

    #[test]
    fn test_empty_messages_returns_empty() {
        let mut compressor = ContextCompressor::new(Default::default());
        let empty: Vec<Message> = vec![];

        let result = compressor.compress_all(&empty);
        assert_eq!(result.original_count, 0);
        assert_eq!(result.compressed_count, 0);
        assert!(result.messages.is_empty());
        assert_eq!(result.tokens_saved_estimate, 0);
    }

    // -- 7. MemoryLevel ordering ----------------------------------------------

    #[test]
    fn test_memory_level_ordering() {
        assert!(MemoryLevel::L0 < MemoryLevel::L1);
        assert!(MemoryLevel::L1 < MemoryLevel::L2);
        assert!(MemoryLevel::L0 < MemoryLevel::L2);

        // Verify discriminant values.
        assert_eq!(MemoryLevel::L0 as u8, 0);
        assert_eq!(MemoryLevel::L1 as u8, 1);
        assert_eq!(MemoryLevel::L2 as u8, 2);
    }

    // -- 8. CompressedResult statistics ---------------------------------------

    #[test]
    fn test_compressed_result_statistics() {
        let mut compressor = ContextCompressor::new(CompressionConfig {
            tool_result_keep_count: 1,
            ..Default::default()
        });

        let messages = vec![
            Message::user("Hello world"),
            Message::tool("t1".into(), "A long tool result with lots of detail"),
            Message::tool("t2".into(), "Another tool result"),
            Message::tool("t3".into(), "Yet another result"),
        ];

        let result = compressor.compress(&messages, CompressionStage::PruneToolResults);

        assert_eq!(result.original_count, 4);
        assert_eq!(result.compressed_count, 2); // user + last tool
        assert_eq!(
            result.stages_applied,
            vec![CompressionStage::PruneToolResults]
        );
        // We removed 2 tool messages' content, so tokens_saved > 0.
        assert!(result.tokens_saved_estimate > 0);
    }

    // -- 9. hide_system_details truncation ------------------------------------

    #[test]
    fn test_hide_system_details_truncates_long() {
        let compressor = ContextCompressor::new(Default::default());

        let long_system = "x".repeat(1000);
        let messages = vec![
            Message::system(long_system),
            Message::user("short user msg"),
        ];

        let result = compressor.hide_system_details(&messages);
        assert_eq!(result.len(), 2);
        // System message should be truncated to 500 chars + "..."
        assert!(result[0].content.len() < 510);
        assert!(result[0].content.ends_with("..."));
        // User message unchanged.
        assert_eq!(result[1].content, "short user msg");
    }

    #[test]
    fn test_hide_system_details_preserves_short() {
        let compressor = ContextCompressor::new(Default::default());
        let messages = vec![Message::system("Short prompt"), Message::user("Hello")];
        let result = compressor.hide_system_details(&messages);
        assert_eq!(result[0].content, "Short prompt");
    }

    // -- 10. summarize_early produces summary ---------------------------------

    #[test]
    fn test_summarize_early_produces_summary() {
        let compressor = ContextCompressor::new(Default::default());

        let mut messages = vec![Message::system("System prompt")];
        for i in 0..10 {
            messages.push(Message::user(format!("User message {i}")));
            messages.push(Message::assistant(format!("Assistant reply {i}")));
        }
        // 1 system + 20 conversation = 21 messages.
        assert_eq!(messages.len(), 21);

        let result = compressor.summarize_early(&messages, 500);
        // Should be shorter than the original.
        assert!(result.len() < messages.len());
        // First message should still be the system prompt.
        assert_eq!(result[0].role, Role::System);
        assert_eq!(result[0].content, "System prompt");
        // Second message should be the synthetic summary.
        assert_eq!(result[1].role, Role::System);
        assert!(result[1].content.contains("[Context summary"));
    }

    #[test]
    fn test_summarize_early_short_conversation_unchanged() {
        let compressor = ContextCompressor::new(Default::default());
        let messages = vec![
            Message::system("sys"),
            Message::user("hi"),
            Message::assistant("hello"),
        ];
        let result = compressor.summarize_early(&messages, 500);
        assert_eq!(result.len(), messages.len());
    }
}
