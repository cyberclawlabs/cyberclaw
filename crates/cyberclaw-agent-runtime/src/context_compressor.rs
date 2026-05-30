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
//!
//! ## LLM-driven summarization (Gap 1.1)
//!
//! Stage 2 (`SummarizeEarly`) now delegates to a [`ContextSummarizer`] trait.
//! The production path uses [`LlmContextSummarizer`] which sends the early
//! conversation turns to an LLM using a structured 12-section prompt that
//! mirrors the hermes reference implementation. When no LLM client is
//! available, or when the LLM call fails, the [`DeterministicSummarizer`]
//! falls back to the original 120-char-per-message behaviour so the
//! circuit breaker always has a safe path.
//!
//! Iterative merge: the compressor stores `previous_summary` across calls.
//! On the second and subsequent compressions the prompt asks the LLM to
//! MERGE new turns into the existing summary instead of starting from scratch
//! (anti-thrashing pattern from hermes).

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::agentic_loop::{IterationBudget, LoopState};
use cyberclaw_llm::client::LlmClient;
use cyberclaw_llm::types::{ChatRequest, Message, Role};

// ---------------------------------------------------------------------------
// ContextCompressionError
// ---------------------------------------------------------------------------

/// Errors that can occur during context summarization.
#[derive(Debug, thiserror::Error)]
pub enum ContextCompressionError {
    /// The LLM call failed.
    #[error("LLM summarization failed: {0}")]
    LlmError(String),
    /// The LLM returned an empty or unparseable response.
    #[error("Summarizer returned empty response")]
    EmptyResponse,
}

// ---------------------------------------------------------------------------
// ContextSummarizer trait
// ---------------------------------------------------------------------------

/// Abstraction over the summarization back-end.
///
/// Implementing types are responsible for producing a concise textual summary
/// of the provided message slice. When a previous summary exists, the
/// implementation should merge new turns into it rather than regenerating
/// from scratch.
///
/// The trait is `async_trait`-derived so it can be used as `dyn
/// ContextSummarizer` in `Arc<dyn ContextSummarizer>`.
#[async_trait]
pub trait ContextSummarizer: Send + Sync {
    /// Produce a summary of `messages`.
    ///
    /// # Arguments
    ///
    /// * `messages` - The conversation turns to summarize.
    /// * `previous_summary` - An existing summary to merge into, if any.
    /// * `focus_topic` - Optional hint about what the summary should emphasize.
    async fn summarize(
        &self,
        messages: &[Message],
        previous_summary: Option<&str>,
        focus_topic: Option<&str>,
    ) -> Result<String, ContextCompressionError>;
}

// ---------------------------------------------------------------------------
// Compression system prompt constant
// ---------------------------------------------------------------------------

/// Structured system prompt used by [`LlmContextSummarizer`].
///
/// Ported from hermes `_summarize_with_llm()`. The placeholders
/// `{previous}`, `{new}`, and `{focus}` are filled in at call time.
pub const COMPRESSION_SYSTEM_PROMPT: &str = "\
You are a context compression assistant. Your job is to produce a concise, \
structured summary of a conversation so it can replace the original turns \
without losing information the agent needs to continue working.

Preserve ALL of the following that appear in the conversation:
- Active task & user intent
- Completed actions & their outcomes
- Decisions made & their rationale
- Files modified & their final state
- Errors encountered & their resolutions
- Tools used & their key results
- Open questions & next steps

Output the summary as plain text under the following section headers \
(omit a section if it has no content):

## Active Task
## User Intent
## Completed Actions
## Active State
## Decisions Made
## Files Modified
## Errors Encountered
## Tools Used
## Next Steps
## Open Questions
## External References

Target length: 500-1500 tokens. Be concise — do not repeat yourself.";

// ---------------------------------------------------------------------------
// LlmContextSummarizer
// ---------------------------------------------------------------------------

/// Production summarizer: calls an LLM using the structured 12-section prompt.
///
/// On subsequent calls, passes the previous summary back so the LLM can
/// merge new turns into it rather than regenerating from scratch.
pub struct LlmContextSummarizer {
    client: Arc<dyn LlmClient>,
    model: String,
}

impl LlmContextSummarizer {
    /// Create a new summarizer backed by the given LLM client.
    ///
    /// `model` is the model name to use for summarization. Using a cheaper /
    /// faster model (e.g. the provider's "mini" variant) is recommended.
    pub fn new(client: Arc<dyn LlmClient>, model: impl Into<String>) -> Self {
        Self {
            client,
            model: model.into(),
        }
    }
}

#[async_trait]
impl ContextSummarizer for LlmContextSummarizer {
    async fn summarize(
        &self,
        messages: &[Message],
        previous_summary: Option<&str>,
        focus_topic: Option<&str>,
    ) -> Result<String, ContextCompressionError> {
        // Build the user prompt.
        let mut user_prompt = String::new();

        if let Some(prev) = previous_summary {
            user_prompt.push_str("A previous summary already exists. MERGE the new turns below into it without losing prior context. Do NOT regenerate from scratch.\n\n");
            user_prompt.push_str("PREVIOUS SUMMARY:\n");
            user_prompt.push_str(prev);
            user_prompt.push_str("\n\nNEW TURNS TO MERGE:\n");
        } else {
            user_prompt.push_str("Summarize the following conversation turns:\n\n");
        }

        for msg in messages {
            let role_label = match msg.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
                Role::Tool => "Tool",
                Role::System => "System",
            };
            user_prompt.push_str(&format!("[{role_label}]: {}\n\n", msg.content));
        }

        if let Some(topic) = focus_topic {
            user_prompt.push_str(&format!("\nPrioritize information related to: {topic}\n"));
        }

        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                Message::system(COMPRESSION_SYSTEM_PROMPT),
                Message::user(user_prompt),
            ],
            temperature: Some(0.3),
            max_tokens: Some(2048),
            ..Default::default()
        };

        let response = self
            .client
            .chat_completion(request)
            .await
            .map_err(|e| ContextCompressionError::LlmError(e.to_string()))?;

        let content = response
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default();

        if content.trim().is_empty() {
            return Err(ContextCompressionError::EmptyResponse);
        }

        Ok(content)
    }
}

// ---------------------------------------------------------------------------
// DeterministicSummarizer
// ---------------------------------------------------------------------------

/// Fallback summarizer: 120-char-per-message truncation (original behaviour).
///
/// Used when no LLM client is available, or as the final safety net when
/// the LLM summarizer itself fails.
pub struct DeterministicSummarizer;

#[async_trait]
impl ContextSummarizer for DeterministicSummarizer {
    async fn summarize(
        &self,
        messages: &[Message],
        _previous_summary: Option<&str>,
        _focus_topic: Option<&str>,
    ) -> Result<String, ContextCompressionError> {
        let parts: Vec<String> = messages
            .iter()
            .map(|m| {
                let role_label = match m.role {
                    Role::User => "User",
                    Role::Assistant => "Assistant",
                    Role::Tool => "Tool",
                    Role::System => "System",
                };
                let truncated: String = m.content.chars().take(120).collect();
                format!("[{role_label}] {truncated}")
            })
            .collect();
        Ok(parts.join("\n"))
    }
}

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
    /// Model to use for LLM-driven summarization. `None` means use the
    /// same model as the main loop.
    /// Default: `None`.
    pub compression_model: Option<String>,
    /// Whether LLM-driven compression is enabled.
    /// Default: `true`.
    pub compression_enabled: bool,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            trigger_threshold: 0.8,
            tool_result_keep_count: 10,
            summary_max_tokens: 500,
            sliding_window_size: 50,
            max_consecutive_failures: 3,
            compression_model: None,
            compression_enabled: true,
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

/// Four-stage context compressor with circuit breaker and LLM summarization.
///
/// ## LLM summarization (Gap 1.1)
///
/// Stage 2 delegates to a [`ContextSummarizer`] implementation. Inject the
/// production [`LlmContextSummarizer`] via [`ContextCompressor::with_summarizer`].
/// Without injection the compressor uses the [`DeterministicSummarizer`]
/// fallback.
///
/// `previous_summary` is stored across calls so subsequent compressions merge
/// new turns into the existing summary (anti-thrashing).
pub struct ContextCompressor {
    config: CompressionConfig,
    consecutive_failures: u32,
    /// Optional LLM-backed summarizer. When `None` the deterministic
    /// fallback is used.
    summarizer: Option<Arc<dyn ContextSummarizer>>,
    /// Last successful summary text. Passed back to the summarizer on
    /// subsequent calls so it can MERGE rather than regenerate.
    previous_summary: Option<String>,
}

impl std::fmt::Debug for ContextCompressor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContextCompressor")
            .field("config", &self.config)
            .field("consecutive_failures", &self.consecutive_failures)
            .field("has_summarizer", &self.summarizer.is_some())
            .field(
                "previous_summary_len",
                &self.previous_summary.as_ref().map(|s| s.len()),
            )
            .finish()
    }
}

impl ContextCompressor {
    /// Create a new compressor with the given configuration.
    ///
    /// Uses the [`DeterministicSummarizer`] fallback until
    /// [`Self::with_summarizer`] is called.
    pub fn new(config: CompressionConfig) -> Self {
        Self {
            config,
            consecutive_failures: 0,
            summarizer: None,
            previous_summary: None,
        }
    }

    /// Inject an LLM-backed summarizer for stage 2.
    ///
    /// Call this right after construction to enable LLM-driven summarization.
    /// The `Arc` allows sharing the summarizer across multiple compressor
    /// instances (e.g. sub-agent loops).
    pub fn with_summarizer(mut self, summarizer: Arc<dyn ContextSummarizer>) -> Self {
        self.summarizer = Some(summarizer);
        self
    }

    /// Convenience: inject an [`LlmContextSummarizer`] wrapping `client`.
    pub fn with_llm_client(self, client: Arc<dyn LlmClient>, model: impl Into<String>) -> Self {
        self.with_summarizer(Arc::new(LlmContextSummarizer::new(client, model)))
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
    ///
    /// Stage 2 (`SummarizeEarly`) is synchronous here for backward
    /// compatibility. Callers that want the async LLM path should use
    /// [`Self::compress_all_async`] or call [`Self::summarize_early_async`]
    /// directly.
    pub fn compress(&mut self, messages: &[Message], stage: CompressionStage) -> CompressedResult {
        let original_count = messages.len();
        let original_chars: usize = messages.iter().map(|m| m.content.len()).sum();

        let compressed = match stage {
            CompressionStage::PruneToolResults => {
                self.prune_tool_results(messages, self.config.tool_result_keep_count)
            }
            CompressionStage::SummarizeEarly => {
                self.summarize_early_sync(messages, self.config.summary_max_tokens)
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

    /// Run all four compression stages in order (sync version).
    ///
    /// Stage 2 uses the deterministic fallback. For LLM-driven summarization
    /// call [`Self::compress_all_async`] instead.
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
                    self.summarize_early_sync(&current, self.config.summary_max_tokens)
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

    /// Run all four compression stages, using the async LLM summarizer for
    /// stage 2 when available.
    ///
    /// Falls back to the deterministic summarizer when:
    /// - No summarizer is injected.
    /// - The LLM call fails (prevents infinite compression loops on
    ///   `ContextOverflow` errors where summarization itself triggers a new
    ///   overflow).
    /// - `compression_enabled` is `false` in config.
    ///
    /// If the circuit breaker has tripped, returns the original messages
    /// unchanged.
    pub async fn compress_all_async(&mut self, messages: &[Message]) -> CompressedResult {
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
                    self.summarize_early_async_inner(&current, self.config.summary_max_tokens)
                        .await
                }
                CompressionStage::HideSystemDetails => self.hide_system_details(&current),
                CompressionStage::SlidingWindow => {
                    self.sliding_window(&current, self.config.sliding_window_size)
                }
            };
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

    // -----------------------------------------------------------------------
    // Stage implementations
    // -----------------------------------------------------------------------

    /// **Stage 1**: Prune old tool-result messages, keeping only the last
    /// `keep` tool results. System, user, and assistant messages are preserved.
    ///
    /// Pruning is *atomic with respect to tool-call pairing* (Bug G): when a
    /// `Role::Tool` message is dropped, the matching `tool_calls` entry is also
    /// removed from its owning assistant message. If an assistant's
    /// `tool_calls` becomes empty and its `content` is empty, the assistant
    /// message itself is dropped; otherwise the content is kept with
    /// `tool_calls` set to `None`. This guarantees the output never contains an
    /// orphan tool result (a `tool_call_id` not referenced by any surviving
    /// assistant `tool_calls`) — the condition MiniMax rejects with
    /// "tool result's tool id not found".
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

        // Collect the tool_call_ids of the tool results we are removing, so we
        // can strip the matching tool_calls entries from assistant messages.
        let removed_call_ids: std::collections::HashSet<String> = remove_set
            .iter()
            .filter_map(|&i| messages[i].tool_call_id.clone())
            .collect();

        let mut result = Vec::with_capacity(messages.len() - remove_count);
        for (i, m) in messages.iter().enumerate() {
            if remove_set.contains(&i) {
                continue;
            }

            // For assistant messages with tool_calls, drop any tool_calls whose
            // result was pruned, keeping pairing atomic.
            if m.role == Role::Assistant {
                if let Some(calls) = &m.tool_calls {
                    let kept: Vec<_> = calls
                        .iter()
                        .filter(|tc| !removed_call_ids.contains(&tc.id))
                        .cloned()
                        .collect();
                    if kept.len() != calls.len() {
                        if kept.is_empty() && m.content.trim().is_empty() {
                            // No surviving calls and no content → drop entirely.
                            continue;
                        }
                        let mut cloned = m.clone();
                        cloned.tool_calls = if kept.is_empty() { None } else { Some(kept) };
                        result.push(cloned);
                        continue;
                    }
                }
            }

            result.push(m.clone());
        }

        result
    }

    /// **Stage 2** (sync): deterministic 120-char-per-message summarization.
    ///
    /// This is the legacy path used by [`Self::compress`] and
    /// [`Self::compress_all`] for backward compatibility. Production code
    /// should call [`Self::compress_all_async`] to get the LLM path.
    pub fn summarize_early(&self, messages: &[Message], max_tokens: u32) -> Vec<Message> {
        self.summarize_early_sync(messages, max_tokens)
    }

    fn summarize_early_sync(&self, messages: &[Message], max_tokens: u32) -> Vec<Message> {
        if messages.len() <= 4 {
            return messages.to_vec();
        }

        let keep_tail = messages.len() / 2;
        let early_end = messages.len() - keep_tail;

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

        let early_msgs = &messages[early_start..early_end];
        // Use the DeterministicSummarizer synchronously.
        let summary_parts: Vec<String> = {
            let mut parts = Vec::new();
            let char_budget = (max_tokens as usize) * 4;
            let mut chars_used: usize = 0;
            for msg in early_msgs {
                let role_label = match msg.role {
                    Role::User => "User",
                    Role::Assistant => "Assistant",
                    Role::Tool => "Tool",
                    Role::System => "System",
                };
                let truncated: String = msg.content.chars().take(120).collect();
                let part = format!("[{role_label}] {truncated}");
                chars_used += part.len();
                if chars_used > char_budget {
                    parts.push("[...earlier turns omitted...]".to_string());
                    break;
                }
                parts.push(part);
            }
            parts
        };

        let summary_text = format!(
            "[Context summary of {} earlier messages]\n{}",
            early_end - early_start,
            summary_parts.join("\n")
        );

        Self::build_summarized_result(messages, early_start, early_end, keep_tail, summary_text)
    }

    /// 组装 SummarizeEarly 的结果列表，保留原始 user 任务消息。
    ///
    /// 结构: `[system[0]?, 保留的首条 user?, summary_system, ...tail]`。
    ///
    /// **根本修复 (Bug I-d 类)**: 早先压缩会把 early 区里的原始 user turn
    /// 一并折进 summary，多轮工具任务的 tail 半段又全是 assistant(tool_use)/
    /// tool(result) 对 → 压缩后 live 窗口**没有任何 user 消息**。这让对话不以
    /// 可应答的 user turn 锚定: generic provider 上 MiniMax 返空 200，anthropic
    /// provider 上 (Anthropic 要求 messages 首条为 user) MiniMax shim 报
    /// 500 "input json is empty"。此处在压缩根部保留 early 区第一条 user
    /// (原始任务)，既维持 user 锚点又保留任务上下文 (provider 侧 ensure_user
    /// 兜底降级为冗余防线)。
    fn build_summarized_result(
        messages: &[Message],
        early_start: usize,
        early_end: usize,
        keep_tail: usize,
        summary_text: String,
    ) -> Vec<Message> {
        let mut result = Vec::with_capacity(3 + keep_tail);
        if early_start == 1 {
            result.push(messages[0].clone());
        }
        // 保留 early 区第一条 user 消息 (原始任务)，防止压缩后 live 窗口无 user。
        if let Some(user_msg) = messages[early_start..early_end]
            .iter()
            .find(|m| m.role == Role::User)
        {
            result.push(user_msg.clone());
        }
        result.push(Message::system(summary_text));
        result.extend_from_slice(&messages[messages.len() - keep_tail..]);
        // Bug I: folding early turns into the summary can orphan a tool result
        // whose owning assistant was folded away. Drop any such orphans.
        Self::drop_orphan_tool_results(result)
    }

    /// **Stage 2** (async inner): calls the injected [`ContextSummarizer`],
    /// updates `previous_summary` on success, falls back to deterministic on
    /// failure.
    ///
    /// Anti-infinite-loop guard: if the summarizer fails (e.g. context
    /// overflow during summarization itself), we fall back to the sync path
    /// and record a failure on the circuit breaker.
    async fn summarize_early_async_inner(
        &mut self,
        messages: &[Message],
        max_tokens: u32,
    ) -> Vec<Message> {
        if messages.len() <= 4 {
            return messages.to_vec();
        }

        let keep_tail = messages.len() / 2;
        let early_end = messages.len() - keep_tail;

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

        let early_msgs = &messages[early_start..early_end];

        // Try LLM-backed summarizer if available and enabled.
        if self.config.compression_enabled {
            if let Some(summarizer) = self.summarizer.clone() {
                let prev = self.previous_summary.clone();
                match summarizer
                    .summarize(early_msgs, prev.as_deref(), None)
                    .await
                {
                    Ok(summary_text) => {
                        // Update the iterative-merge state.
                        self.previous_summary = Some(summary_text.clone());

                        let summary = format!(
                            "[Context summary of {} earlier messages]\n{}",
                            early_end - early_start,
                            summary_text
                        );
                        return Self::build_summarized_result(
                            messages,
                            early_start,
                            early_end,
                            keep_tail,
                            summary,
                        );
                    }
                    Err(err) => {
                        // LLM summarizer failed — log and fall through to deterministic.
                        // Increment the failure counter so repeated LLM failures
                        // eventually trip the circuit breaker.
                        tracing::warn!(
                            error = %err,
                            "context_compressor: LLM summarizer failed, \
                             falling back to deterministic summarizer"
                        );
                        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                    }
                }
            }
        }

        // Deterministic fallback.
        self.summarize_early_sync(messages, max_tokens)
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

        let windowed = if has_system && size >= 2 {
            let mut result = Vec::with_capacity(size);
            result.push(messages[0].clone());
            let tail_count = size - 1;
            let start = messages.len().saturating_sub(tail_count);
            result.extend_from_slice(&messages[start..]);
            result
        } else {
            let start = messages.len().saturating_sub(size);
            messages[start..].to_vec()
        };

        Self::drop_leading_orphan_tool_results(windowed)
    }

    /// Drop leading `Role::Tool` messages whose `tool_call_id` is not referenced
    /// by any surviving assistant `tool_calls` (Bug G).
    ///
    /// Hard truncation can slice off an assistant-with-tool_calls while keeping
    /// a following tool result, leaving an orphan that MiniMax rejects. We only
    /// strip *leading* orphans: a tool result that appears after its owning
    /// assistant message is, by construction, still paired.
    fn drop_leading_orphan_tool_results(messages: Vec<Message>) -> Vec<Message> {
        let call_ids: std::collections::HashSet<String> = messages
            .iter()
            .filter(|m| m.role == Role::Assistant)
            .filter_map(|m| m.tool_calls.as_ref())
            .flatten()
            .map(|tc| tc.id.clone())
            .collect();

        // A leading system prompt is preserved verbatim by the window; skip
        // past it before scanning for orphan tool results.
        let system_offset = usize::from(
            messages
                .first()
                .map(|m| m.role == Role::System)
                .unwrap_or(false),
        );

        let mut orphan_indices = Vec::new();
        for (i, m) in messages.iter().enumerate().skip(system_offset) {
            if m.role == Role::Tool {
                let paired = m
                    .tool_call_id
                    .as_deref()
                    .is_some_and(|id| call_ids.contains(id));
                if !paired {
                    orphan_indices.push(i);
                    continue;
                }
            }
            // Stop at the first non-orphan message: once we hit an assistant /
            // user / paired tool message the remaining tool results are paired.
            break;
        }

        if orphan_indices.is_empty() {
            return messages;
        }
        let drop: std::collections::HashSet<usize> = orphan_indices.into_iter().collect();
        messages
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !drop.contains(i))
            .map(|(_, m)| m)
            .collect()
    }

    /// Drop *any* `Role::Tool` message whose `tool_call_id` is not referenced
    /// by a surviving assistant `tool_calls` entry (Bug I).
    ///
    /// Unlike [`Self::drop_leading_orphan_tool_results`], which only strips
    /// orphans at the front of the window (relying on positional pairing), this
    /// scans the entire list. `SummarizeEarly` folds a contiguous span of early
    /// turns into one summary message, so an assistant-with-tool_calls can be
    /// folded away while its tool result remains in the preserved tail — the
    /// orphan can therefore appear anywhere after the summary, not just at the
    /// leading edge. Removing every unpaired tool result restores the invariant
    /// MiniMax requires.
    fn drop_orphan_tool_results(messages: Vec<Message>) -> Vec<Message> {
        let call_ids: std::collections::HashSet<String> = messages
            .iter()
            .filter(|m| m.role == Role::Assistant)
            .filter_map(|m| m.tool_calls.as_ref())
            .flatten()
            .map(|tc| tc.id.clone())
            .collect();

        let has_orphan = messages.iter().any(|m| {
            m.role == Role::Tool
                && !m
                    .tool_call_id
                    .as_deref()
                    .is_some_and(|id| call_ids.contains(id))
        });
        if !has_orphan {
            return messages;
        }

        messages
            .into_iter()
            .filter(|m| {
                if m.role != Role::Tool {
                    return true;
                }
                m.tool_call_id
                    .as_deref()
                    .is_some_and(|id| call_ids.contains(id))
            })
            .collect()
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

    /// Return the stored previous summary (for testing / inspection).
    pub fn previous_summary(&self) -> Option<&str> {
        self.previous_summary.as_deref()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agentic_loop::IterationBudget;
    use std::sync::Mutex;

    // -----------------------------------------------------------------------
    // MockSummarizer
    // -----------------------------------------------------------------------

    /// Captured call record: (messages, previous_summary, focus_topic).
    type CallRecord = (Vec<Message>, Option<String>, Option<String>);
    /// Shared call log used by [`MockSummarizer`].
    type CallLog = Arc<Mutex<Vec<CallRecord>>>;

    /// Test double that records every call and returns a fixed response.
    struct MockSummarizer {
        /// Captured (messages, previous_summary, focus_topic) tuples.
        calls: CallLog,
        /// Response to return (Ok / Err).
        response: Result<String, ContextCompressionError>,
    }

    impl MockSummarizer {
        fn succeeds(text: impl Into<String>) -> (CallLog, Self) {
            let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
            let s = Self {
                calls: Arc::clone(&calls),
                response: Ok(text.into()),
            };
            (calls, s)
        }

        fn fails() -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                response: Err(ContextCompressionError::LlmError(
                    "mock failure".to_string(),
                )),
            }
        }
    }

    #[async_trait]
    impl ContextSummarizer for MockSummarizer {
        async fn summarize(
            &self,
            messages: &[Message],
            previous_summary: Option<&str>,
            focus_topic: Option<&str>,
        ) -> Result<String, ContextCompressionError> {
            self.calls.lock().unwrap().push((
                messages.to_vec(),
                previous_summary.map(str::to_string),
                focus_topic.map(str::to_string),
            ));
            match &self.response {
                Ok(s) => Ok(s.clone()),
                Err(_) => Err(ContextCompressionError::LlmError(
                    "mock failure".to_string(),
                )),
            }
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_state(tokens: u64) -> LoopState {
        LoopState {
            messages: vec![],
            iteration_count: 0,
            tokens_consumed: tokens,
            ..Default::default()
        }
    }

    fn make_budget(max_tokens: u64) -> IterationBudget {
        IterationBudget {
            max_tokens,
            ..Default::default()
        }
    }

    fn long_conversation() -> Vec<Message> {
        let mut msgs = vec![Message::system("System prompt")];
        for i in 0..10 {
            msgs.push(Message::user(format!("User message {i}")));
            msgs.push(Message::assistant(format!("Assistant reply {i}")));
        }
        msgs
    }

    /// Build a `ToolCall` with the given id (function name/args are irrelevant
    /// for pairing tests).
    fn tool_call(id: &str) -> cyberclaw_llm::types::ToolCall {
        cyberclaw_llm::types::ToolCall {
            id: id.to_string(),
            call_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionCall {
                name: "noop".to_string(),
                arguments: "{}".to_string(),
            },
        }
    }

    /// Invariant: every surviving `Role::Tool` message's `tool_call_id` must be
    /// referenced by some surviving assistant message's `tool_calls`, and every
    /// surviving assistant `tool_calls` id should have a matching tool result
    /// (no orphan call). This is the contract MiniMax enforces.
    fn assert_tool_pairing_intact(messages: &[Message]) {
        use std::collections::HashSet;

        let call_ids: HashSet<&str> = messages
            .iter()
            .filter(|m| m.role == Role::Assistant)
            .filter_map(|m| m.tool_calls.as_ref())
            .flatten()
            .map(|tc| tc.id.as_str())
            .collect();

        let result_ids: HashSet<&str> = messages
            .iter()
            .filter(|m| m.role == Role::Tool)
            .filter_map(|m| m.tool_call_id.as_deref())
            .collect();

        // Orphan tool result: result id with no matching assistant tool_call.
        for rid in &result_ids {
            assert!(
                call_ids.contains(rid),
                "orphan tool result: id {rid:?} has no matching assistant tool_call"
            );
        }
        // Orphan tool call: assistant tool_call id with no matching tool result.
        for cid in &call_ids {
            assert!(
                result_ids.contains(cid),
                "orphan tool call: id {cid:?} has no matching tool result"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Gap 1.1 new tests
    // -----------------------------------------------------------------------

    /// 1. Mock summarizer is called during `compress_all_async` with the
    ///    correct message slice (early turns only, not the full list).
    #[tokio::test]
    async fn test_llm_summarizer_called_on_summarize_early() {
        let (calls, mock) = MockSummarizer::succeeds("LLM summary text");
        let mut compressor =
            ContextCompressor::new(Default::default()).with_summarizer(Arc::new(mock));

        let msgs = long_conversation();
        let result = compressor.compress_all_async(&msgs).await;

        // Should have been compressed.
        assert!(result.compressed_count < result.original_count);

        // The mock should have been called at least once.
        let captured = calls.lock().unwrap();
        assert!(
            !captured.is_empty(),
            "MockSummarizer.summarize() was never called"
        );

        // The messages passed to summarize must all be early turns
        // (not the full list). early_end = len/2 so it must be < total.
        let summarized_msg_count = captured[0].0.len();
        assert!(
            summarized_msg_count < msgs.len(),
            "summarizer received the full list ({} msgs), expected early slice",
            msgs.len()
        );
    }

    /// 2. On the second `compress_all_async` call the summarizer receives
    ///    the previous summary text as `previous_summary`.
    #[tokio::test]
    async fn test_iterative_merge_passes_previous_summary() {
        let (calls, mock) = MockSummarizer::succeeds("First summary");
        let mut compressor =
            ContextCompressor::new(Default::default()).with_summarizer(Arc::new(mock));

        let msgs = long_conversation();

        // First compression: no previous summary.
        compressor.compress_all_async(&msgs).await;

        // Swap in a new mock that records the second call.
        let (calls2, mock2) = MockSummarizer::succeeds("Second summary");
        compressor.summarizer = Some(Arc::new(mock2));

        // Second compression: previous_summary should now be "First summary".
        compressor.compress_all_async(&msgs).await;

        let captured2 = calls2.lock().unwrap();
        assert!(
            !captured2.is_empty(),
            "second summarizer call never happened"
        );
        assert_eq!(
            captured2[0].1.as_deref(),
            Some("First summary"),
            "previous_summary not passed to second summarizer call"
        );
        drop(calls);
    }

    /// 3. When the injected summarizer returns an error, the compressor falls
    ///    back to the DeterministicSummarizer output (contains "[Context summary").
    #[tokio::test]
    async fn test_deterministic_fallback_when_summarizer_fails() {
        let mock = MockSummarizer::fails();
        let mut compressor =
            ContextCompressor::new(Default::default()).with_summarizer(Arc::new(mock));

        let msgs = long_conversation();
        let result = compressor.compress_all_async(&msgs).await;

        // Should still produce a compressed result.
        assert!(result.compressed_count < result.original_count);

        // The synthetic summary message must be present (deterministic path).
        let has_summary = result
            .messages
            .iter()
            .any(|m| m.role == Role::System && m.content.starts_with("[Context summary"));
        assert!(
            has_summary,
            "deterministic fallback summary not found in output"
        );
    }

    /// 4. DeterministicSummarizer produces the 120-char-per-message output
    ///    and preserves the same structure as the original `summarize_early`.
    #[tokio::test]
    async fn test_deterministic_summarizer_preserves_existing_behavior() {
        let det = DeterministicSummarizer;
        let msgs: Vec<Message> = vec![
            Message::user("Hello world, this is a very long message that exceeds 120 characters and should be truncated by the deterministic summarizer path"),
            Message::assistant("Short reply"),
        ];

        let result = det.summarize(&msgs, None, None).await.unwrap();

        // Each line should have a role prefix.
        assert!(result.contains("[User]"));
        assert!(result.contains("[Assistant]"));

        // The user message line must be truncated to 120 chars of content.
        let user_line = result.lines().find(|l| l.starts_with("[User]")).unwrap();
        // "[User] " prefix = 7 chars, so content portion ≤ 120.
        let content_len = user_line.len() - "[User] ".len();
        assert!(
            content_len <= 120,
            "content not truncated: {content_len} chars"
        );
    }

    /// 根本修复 (Bug I-d 类): SummarizeEarly 压缩后, 原始 user 任务消息必须
    /// 仍以 user 角色保留在结果里 — 否则多工具任务的 tail 全是 assistant/tool
    /// 对、压缩后 live 窗口无 user, 触发 provider 侧空响应/500。
    #[test]
    fn test_summarize_early_preserves_original_user_message() {
        let compressor = ContextCompressor::new(Default::default());
        // [system, user(任务), assistant×6] — len=8, tail 半段全是 assistant 无 user。
        let mut msgs = vec![
            Message::system("You are an agent."),
            Message::user("生成 6 页 pptx 介绍 CyberClaw"),
        ];
        for i in 0..6 {
            msgs.push(Message::assistant(format!("step {i}")));
        }

        let result = compressor.summarize_early(&msgs, 500);

        // 压缩后仍必须存在至少一条 user 角色消息 (原始任务)。
        let user_msg = result.iter().find(|m| m.role == Role::User);
        assert!(
            user_msg.is_some(),
            "压缩后 live 窗口必须保留 user 消息; 实际角色: {:?}",
            result.iter().map(|m| &m.role).collect::<Vec<_>>()
        );
        assert!(
            user_msg.unwrap().content.contains("生成 6 页 pptx"),
            "保留的 user 应是原始任务内容"
        );
    }

    /// 5. After a successful async compression the compressor updates
    ///    `previous_summary` so the next call can merge.
    #[tokio::test]
    async fn test_previous_summary_updated_after_successful_compression() {
        let (_, mock) = MockSummarizer::succeeds("My summary output");
        let mut compressor =
            ContextCompressor::new(Default::default()).with_summarizer(Arc::new(mock));

        assert!(compressor.previous_summary().is_none());

        let msgs = long_conversation();
        compressor.compress_all_async(&msgs).await;

        assert_eq!(
            compressor.previous_summary(),
            Some("My summary output"),
            "previous_summary not updated after successful LLM summarization"
        );
    }

    // -----------------------------------------------------------------------
    // Legacy tests (unchanged behaviour)
    // -----------------------------------------------------------------------

    #[test]
    fn test_should_compress_trigger_threshold() {
        let compressor = ContextCompressor::new(CompressionConfig {
            trigger_threshold: 0.8,
            ..Default::default()
        });

        assert!(!compressor.should_compress(&make_state(79), &make_budget(100)));
        assert!(compressor.should_compress(&make_state(80), &make_budget(100)));
        assert!(compressor.should_compress(&make_state(100), &make_budget(100)));
    }

    #[test]
    fn test_should_compress_unlimited_budget() {
        let compressor = ContextCompressor::new(Default::default());
        assert!(!compressor.should_compress(&make_state(999), &make_budget(0)));
    }

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

        let tool_contents: Vec<&str> = pruned
            .iter()
            .filter(|m| m.role == Role::Tool)
            .map(|m| m.content.as_str())
            .collect();
        assert_eq!(tool_contents, vec!["result 3", "result 4"]);

        let non_tool_count = pruned.iter().filter(|m| m.role != Role::Tool).count();
        assert_eq!(non_tool_count, 6);
    }

    /// Bug G: pruning a tool result must atomically remove the corresponding
    /// `tool_calls` entry from its assistant message, never leaving an orphan
    /// tool result (or an orphan tool call). MiniMax rejects a request when a
    /// tool result's id is not found in any assistant `tool_calls`.
    #[test]
    fn prune_keeps_tool_call_result_pairs_atomic() {
        let compressor = ContextCompressor::new(Default::default());

        let messages = vec![
            Message::user("Do A and B"),
            Message::assistant_with_tools("calling A and B", vec![tool_call("A"), tool_call("B")]),
            Message::tool("A".into(), "result A"),
            Message::tool("B".into(), "result B"),
        ];

        // keep = 1 → the older tool result (A) must be dropped, and its
        // matching assistant tool_call must be dropped too.
        let pruned = compressor.prune_tool_results(&messages, 1);

        assert_tool_pairing_intact(&pruned);

        // B's result survives, A's result is gone.
        let surviving_results: Vec<&str> = pruned
            .iter()
            .filter(|m| m.role == Role::Tool)
            .filter_map(|m| m.tool_call_id.as_deref())
            .collect();
        assert_eq!(surviving_results, vec!["B"]);
    }

    /// Bug G branch (c): when *all* tool_calls of an assistant message are
    /// pruned but the assistant has non-empty content, the assistant message
    /// must survive with its content intact and `tool_calls` set to `None`.
    #[test]
    fn prune_retains_assistant_content_when_all_tool_calls_pruned() {
        let compressor = ContextCompressor::new(Default::default());

        let messages = vec![
            Message::user("Do something"),
            Message::assistant_with_tools("思考中...", vec![tool_call("A")]),
            Message::tool("A".into(), "result A"),
        ];

        // keep = 0 → result A is pruned, so tool_call A must be removed from
        // the assistant. But the assistant has content "思考中..." so it must
        // NOT be dropped — only its tool_calls should become None.
        let pruned = compressor.prune_tool_results(&messages, 0);

        // Pairing invariant: no orphan result, no orphan call.
        assert_tool_pairing_intact(&pruned);

        // The assistant message must still be present.
        let assistant = pruned.iter().find(|m| m.role == Role::Assistant);
        assert!(assistant.is_some(), "assistant message was dropped unexpectedly");
        let assistant = assistant.unwrap();

        // Content preserved.
        assert_eq!(assistant.content, "思考中...");

        // tool_calls cleared.
        assert!(
            assistant.tool_calls.is_none()
                || assistant.tool_calls.as_ref().is_some_and(|v| v.is_empty()),
            "tool_calls should be None after all calls are pruned"
        );

        // The tool result is gone.
        assert!(
            !pruned.iter().any(|m| m.role == Role::Tool),
            "tool result A should have been pruned"
        );
    }

    /// Bug G: sliding-window truncation that slices off an
    /// assistant-with-tool_calls but keeps a following tool result must drop
    /// the now-orphaned leading tool result.
    #[test]
    fn sliding_window_drops_orphan_leading_tool_results() {
        let compressor = ContextCompressor::new(Default::default());

        let messages = vec![
            Message::user("start"),
            Message::assistant_with_tools("call A", vec![tool_call("A")]),
            Message::tool("A".into(), "result A"),
            Message::assistant_with_tools("call B", vec![tool_call("B")]),
            Message::tool("B".into(), "result B"),
            Message::user("more"),
        ];

        // size = 4 keeps the last 4: [tool(A), assistant(B), tool(B), user] —
        // tool(A) is now an orphan (its assistant call at index 1 was sliced
        // off). The window must drop the leading orphan tool result.
        let windowed = compressor.sliding_window(&messages, 4);
        assert_tool_pairing_intact(&windowed);
        // The orphan tool(A) must not survive.
        assert!(
            !windowed
                .iter()
                .any(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some("A")),
            "orphan leading tool result A should have been dropped"
        );
    }

    #[test]
    fn test_prune_tool_results_fewer_than_keep() {
        let compressor = ContextCompressor::new(Default::default());
        let messages = vec![Message::user("Hello"), Message::tool("t1".into(), "result")];
        let pruned = compressor.prune_tool_results(&messages, 10);
        assert_eq!(pruned.len(), 2);
    }

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
        assert_eq!(windowed[3].content, "msg 9");
    }

    #[test]
    fn test_sliding_window_no_truncation_needed() {
        let compressor = ContextCompressor::new(Default::default());
        let messages = vec![Message::user("a"), Message::user("b")];
        let windowed = compressor.sliding_window(&messages, 10);
        assert_eq!(windowed.len(), 2);
    }

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
        assert!(result.messages.len() <= 5);
    }

    #[test]
    fn test_circuit_breaker_stops_compression() {
        let mut compressor = ContextCompressor::new(CompressionConfig {
            max_consecutive_failures: 3,
            ..Default::default()
        });

        compressor.record_failure();
        compressor.record_failure();
        compressor.record_failure();

        assert!(!compressor.should_compress(&make_state(100), &make_budget(100)));

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

        let messages = vec![
            Message::user("a"),
            Message::tool("t1".into(), "r1"),
            Message::tool("t2".into(), "r2"),
            Message::tool("t3".into(), "r3"),
        ];
        let _result = compressor.compress(&messages, CompressionStage::PruneToolResults);
        assert_eq!(compressor.consecutive_failures(), 0);
    }

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

    #[test]
    fn test_memory_level_ordering() {
        assert!(MemoryLevel::L0 < MemoryLevel::L1);
        assert!(MemoryLevel::L1 < MemoryLevel::L2);
        assert!(MemoryLevel::L0 < MemoryLevel::L2);

        assert_eq!(MemoryLevel::L0 as u8, 0);
        assert_eq!(MemoryLevel::L1 as u8, 1);
        assert_eq!(MemoryLevel::L2 as u8, 2);
    }

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
        assert_eq!(result.compressed_count, 2);
        assert_eq!(
            result.stages_applied,
            vec![CompressionStage::PruneToolResults]
        );
        assert!(result.tokens_saved_estimate > 0);
    }

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
        assert!(result[0].content.len() < 510);
        assert!(result[0].content.ends_with("..."));
        assert_eq!(result[1].content, "short user msg");
    }

    #[test]
    fn test_hide_system_details_preserves_short() {
        let compressor = ContextCompressor::new(Default::default());
        let messages = vec![Message::system("Short prompt"), Message::user("Hello")];
        let result = compressor.hide_system_details(&messages);
        assert_eq!(result[0].content, "Short prompt");
    }

    #[test]
    fn test_summarize_early_produces_summary() {
        let compressor = ContextCompressor::new(Default::default());

        let mut messages = vec![Message::system("System prompt")];
        for i in 0..10 {
            messages.push(Message::user(format!("User message {i}")));
            messages.push(Message::assistant(format!("Assistant reply {i}")));
        }
        assert_eq!(messages.len(), 21);

        let result = compressor.summarize_early(&messages, 500);
        assert!(result.len() < messages.len());
        assert_eq!(result[0].role, Role::System);
        assert_eq!(result[0].content, "System prompt");
        // 根本修复 (Bug I-d 类): 原始 user 任务保留在 summary 之前, 确保
        // 压缩后 live 窗口仍以可应答的 user turn 锚定。
        assert_eq!(result[1].role, Role::User);
        assert!(result[1].content.contains("User message 0"));
        assert_eq!(result[2].role, Role::System);
        assert!(result[2].content.contains("[Context summary"));
    }

    /// Bug I: `SummarizeEarly` folds early turns into a single summary
    /// message. If a folded assistant message owned a `tool_calls` entry whose
    /// `Role::Tool` result lands in the preserved tail (or vice versa), the
    /// surviving tool result becomes an orphan — its `tool_call_id` is no
    /// longer referenced by any surviving assistant. MiniMax rejects this with
    /// "tool result's tool id not found". The summarize stage output must run
    /// the same full-list pairing cleanup as the other stages.
    #[tokio::test]
    async fn summarize_early_drops_orphan_tool_results() {
        // 14 messages: system + 13 turns. keep_tail = 14/2 = 7, so the early
        // slice is [1..7) (indices 1-6) and the preserved tail is indices
        // 7..14 (the last 7 messages). We place an assistant-with-tool_calls at
        // index 6 (gets folded into the summary) and its matching tool result
        // at index 7 (survives in the tail) → orphan after folding.
        let mut msgs = vec![Message::system("System prompt")];
        // indices 1..=5: filler turns that get folded.
        for i in 0..5 {
            msgs.push(Message::user(format!("User {i}")));
        }
        // index 6: assistant with tool_call "X" — folded into the summary.
        msgs.push(Message::assistant_with_tools(
            "calling X",
            vec![tool_call("X")],
        ));
        // index 7: tool result for "X" — survives in the tail → orphan.
        msgs.push(Message::tool("X".into(), "result X"));
        // indices 8..=13: more tail filler so keep_tail keeps index 7 in tail.
        for i in 0..6 {
            msgs.push(Message::user(format!("Tail {i}")));
        }
        assert_eq!(msgs.len(), 14);
        // Sanity: index 6 (the assistant) is folded (< early_end = 14-7 = 7),
        // index 7 (the tool result) is in the preserved tail.

        let (_, mock) = MockSummarizer::succeeds("LLM summary text");
        let mut compressor = ContextCompressor::new(CompressionConfig {
            // Disable other stages' interference: keep enough tool results so
            // PruneToolResults is a no-op, and a window large enough to not
            // truncate.
            tool_result_keep_count: 100,
            sliding_window_size: 100,
            ..Default::default()
        })
        .with_summarizer(Arc::new(mock));

        let result = compressor.compress_all_async(&msgs).await;

        // The orphan tool result "X" must not survive — pairing intact.
        assert_tool_pairing_intact(&result.messages);
        assert!(
            !result
                .messages
                .iter()
                .any(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some("X")),
            "orphan tool result X should have been dropped after SummarizeEarly folding"
        );
    }

    /// Bug I (sync path): the deterministic `summarize_early` must also drop
    /// orphan tool results produced by folding.
    #[test]
    fn summarize_early_sync_drops_orphan_tool_results() {
        let mut msgs = vec![Message::system("System prompt")];
        for i in 0..5 {
            msgs.push(Message::user(format!("User {i}")));
        }
        msgs.push(Message::assistant_with_tools(
            "calling X",
            vec![tool_call("X")],
        ));
        msgs.push(Message::tool("X".into(), "result X"));
        for i in 0..6 {
            msgs.push(Message::user(format!("Tail {i}")));
        }
        assert_eq!(msgs.len(), 14);

        let compressor = ContextCompressor::new(Default::default());
        let result = compressor.summarize_early(&msgs, 500);

        assert_tool_pairing_intact(&result);
        assert!(
            !result
                .iter()
                .any(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some("X")),
            "orphan tool result X should have been dropped after SummarizeEarly folding (sync)"
        );
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
