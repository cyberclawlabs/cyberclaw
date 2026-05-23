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

        let mut result = Vec::with_capacity(2 + keep_tail);
        if early_start == 1 {
            result.push(messages[0].clone());
        }
        result.push(Message::system(summary_text));
        result.extend_from_slice(&messages[messages.len() - keep_tail..]);
        result
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

                        let mut result = Vec::with_capacity(2 + keep_tail);
                        if early_start == 1 {
                            result.push(messages[0].clone());
                        }
                        result.push(Message::system(format!(
                            "[Context summary of {} earlier messages]\n{}",
                            early_end - early_start,
                            summary_text
                        )));
                        result.extend_from_slice(&messages[messages.len() - keep_tail..]);
                        return result;
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
