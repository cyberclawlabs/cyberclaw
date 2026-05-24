//! # Agentic Loop
//!
//! Core reasoning loop for CyberClaw agents. The `AgenticLoop` trait defines
//! the iteration protocol: LLM call -> parse response -> tool execution -> repeat.
//!
//! Tool calls are dispatched through `OrchestratorGateway` (never directly
//! to connectors), preserving the governance and audit chain.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use cyberclaw_core::capability_contract::CapabilityBehaviorContract;
use cyberclaw_core::execution::ExecutionContext;
use cyberclaw_core::gateway::OrchestratorGateway;
use cyberclaw_core::ids::SkillId;
use cyberclaw_llm::client::LlmClient;
use cyberclaw_llm::rate_limit_tracker::RateLimitSnapshot;
use cyberclaw_llm::types::{ChatRequest, Message, Role, ToolCall, ToolDefinition};

use crate::loop_governor::{AgenticLoopGovernor, LoopCtx, LoopDecision, LoopProfile};
use crate::streaming::{StreamEvent, StreamSink};
use crate::verify::{VerificationDirective, VerifierChain, VerifyCtx};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Budget constraints for a single agentic loop session.
#[derive(Debug, Clone)]
pub struct IterationBudget {
    /// Maximum number of reasoning iterations (default: 90).
    pub max_iterations: u32,
    /// Maximum total tokens consumed across all LLM calls (0 = unlimited).
    pub max_tokens: u64,
    /// Hard timeout for the entire loop session.
    pub timeout: Duration,
}

impl Default for IterationBudget {
    fn default() -> Self {
        Self {
            max_iterations: 90,
            max_tokens: 0,
            timeout: Duration::from_secs(600),
        }
    }
}

/// Configuration for the agentic loop.
#[derive(Debug, Clone)]
pub struct LoopConfig {
    /// System prompt injected as the first message.
    pub system_prompt: String,
    /// LLM model name.
    pub model: String,
    /// Budget constraints.
    pub budget: IterationBudget,
    /// Stuck detector threshold (default: 3 consecutive identical tool calls).
    pub stuck_threshold: u32,
    /// Tools the agent may call. Forwarded into every `ChatRequest` so
    /// the LLM sees a tool palette and can emit `tool_calls` instead of
    /// describing the call in plain text. Empty list disables tool use.
    pub tools: Vec<ToolDefinition>,
    /// P1.1 — 给注入的 system prompt 打上 ephemeral cache_control。
    /// Anthropic native cache 会缓存整个 system 前缀；OpenAI 自动
    /// 缓存 ≥1024 token 不依赖该字段；MiniMax/Ark/Generic 通过
    /// serde skip_if_none 自然缺省，不会拒绝。
    pub cache_system_prompt: bool,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            system_prompt: String::new(),
            model: "gpt-4".to_string(),
            budget: IterationBudget::default(),
            stuck_threshold: 3,
            tools: Vec::new(),
            cache_system_prompt: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Content / Multimodal
// ---------------------------------------------------------------------------

/// A single content part for multimodal messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    /// Plain text content.
    Text {
        /// The text value.
        value: String,
    },
    /// Binary blob with a MIME media type.
    Binary {
        /// Raw binary data.
        data: Vec<u8>,
        /// MIME media type (e.g. "image/png").
        media_type: String,
    },
}

// ---------------------------------------------------------------------------
// Iteration Result
// ---------------------------------------------------------------------------

/// Outcome of a single reasoning iteration.
#[derive(Debug, Clone)]
pub enum IterationResult {
    /// The LLM wants to continue reasoning (no tool calls, no final answer).
    Continue,
    /// The LLM requested one or more tool calls.
    ToolCalls(Vec<ToolCall>),
    /// The LLM produced a text response (intermediate).
    TextResponse(String),
    /// The LLM signalled task completion.
    Done(String),
    /// The stuck detector fired.
    Stuck(String),
    /// The iteration budget has been exhausted. The string carries a
    /// human-readable reason (BT-30): which budget triggered the exit
    /// and the actual usage vs. limit (e.g. "iteration budget 90/90
    /// exhausted" or "token budget 5000/5000 exhausted"). Surface this
    /// to operators / end users so silent truncation never happens.
    BudgetExhausted(String),
}

// ---------------------------------------------------------------------------
// Loop Summary
// ---------------------------------------------------------------------------

/// Summary statistics returned when the loop finalizes.
#[derive(Debug, Clone, Default)]
pub struct LoopSummary {
    /// Total number of iterations executed.
    pub iterations: u32,
    /// Total tokens consumed (prompt + completion).
    pub tokens_used: u64,
    /// Final output text (if any).
    pub final_output: Option<String>,
    /// Rate-limit snapshot from the last LLM call in this loop.
    /// `None` when the provider did not return `x-ratelimit-*` headers.
    pub last_rate_limit: Option<RateLimitSnapshot>,
}

// ---------------------------------------------------------------------------
// Loop State
// ---------------------------------------------------------------------------

/// Internal mutable state of the agentic loop.
#[derive(Debug, Clone, Default)]
pub struct LoopState {
    /// Conversation messages accumulated so far.
    pub messages: Vec<Message>,
    /// Number of completed iterations.
    pub iteration_count: u32,
    /// Total tokens consumed across all LLM calls.
    pub tokens_consumed: u64,
    /// Rate-limit snapshot from the most recent LLM call.
    pub last_rate_limit: Option<RateLimitSnapshot>,
}

impl LoopState {
    fn new() -> Self {
        Self {
            messages: Vec::new(),
            iteration_count: 0,
            tokens_consumed: 0,
            last_rate_limit: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Stuck Detector
// ---------------------------------------------------------------------------

/// Detects when the LLM is stuck in a loop making identical tool calls.
#[derive(Debug)]
pub struct StuckDetector {
    threshold: u32,
    /// Recent tool call signatures (function name + arguments hash).
    recent: Vec<String>,
}

impl StuckDetector {
    /// Create a new detector with the given repetition threshold.
    pub fn new(threshold: u32) -> Self {
        Self {
            threshold,
            recent: Vec::new(),
        }
    }

    /// Record a set of tool calls and return true if stuck.
    pub fn record(&mut self, calls: &[ToolCall]) -> bool {
        let sig = Self::signature(calls);
        self.recent.push(sig);

        if self.recent.len() < self.threshold as usize {
            return false;
        }

        // Check if the last N signatures are identical.
        let tail = &self.recent[self.recent.len() - self.threshold as usize..];
        tail.iter().all(|s| s == &tail[0])
    }

    /// Reset the detector state.
    pub fn reset(&mut self) {
        self.recent.clear();
    }

    fn signature(calls: &[ToolCall]) -> String {
        let mut parts: Vec<String> = calls
            .iter()
            .map(|c| format!("{}:{}", c.function.name, c.function.arguments))
            .collect();
        parts.sort();
        parts.join("|")
    }
}

// ---------------------------------------------------------------------------
// Tool Parallelism
// ---------------------------------------------------------------------------

/// Plan splitting tool calls into parallel vs sequential groups.
#[derive(Debug, Clone, Default)]
pub struct ParallelPlan {
    /// Tool calls that can be executed concurrently.
    pub parallel: Vec<ToolCall>,
    /// Tool calls that must be executed sequentially.
    pub sequential: Vec<ToolCall>,
}

/// Classify tool calls into parallel and sequential buckets based on their
/// `CapabilityBehaviorContract`.
///
/// A tool call is eligible for parallel execution if and only if its contract
/// declares `is_read_only = true` AND `is_concurrency_safe = true`.
pub fn classify_tool_parallelism(
    calls: &[ToolCall],
    contracts: &HashMap<String, CapabilityBehaviorContract>,
) -> ParallelPlan {
    let mut plan = ParallelPlan::default();

    for call in calls {
        let name = &call.function.name;
        match contracts.get(name) {
            Some(contract) if contract.is_read_only && contract.is_concurrency_safe => {
                plan.parallel.push(call.clone());
            }
            _ => {
                // Unknown or non-parallel-safe -> sequential.
                plan.sequential.push(call.clone());
            }
        }
    }

    plan
}

// ---------------------------------------------------------------------------
// AgenticLoop Trait
// ---------------------------------------------------------------------------

/// Core agentic reasoning loop trait.
///
/// Implementations drive the LLM -> tool-call -> observe cycle.
#[async_trait]
pub trait AgenticLoop: Send + Sync {
    /// Initialize the loop with configuration.
    async fn init(&mut self, config: LoopConfig) -> anyhow::Result<()>;

    /// Execute a single reasoning iteration.
    async fn next_iteration(&mut self) -> anyhow::Result<IterationResult>;

    /// Finalize the loop and return summary statistics.
    async fn finalize(&mut self) -> anyhow::Result<LoopSummary>;

    /// Sprint 44+1 — return the skill bindings active for this loop
    /// instance. The default empty slice keeps every existing
    /// `AgenticLoop` implementor source-compatible (no opt-in needed).
    /// `DefaultAgenticLoop` overrides this with the resolved set from
    /// `agent.default_skills` + `ExecutionContext.runtime_skill_bindings`.
    fn active_skill_bindings(&self) -> &[SkillId] {
        &[]
    }
}

// ---------------------------------------------------------------------------
// DefaultAgenticLoop
// ---------------------------------------------------------------------------

/// Default implementation of `AgenticLoop`.
///
/// Drives: LLM call -> parse response -> tool call via gateway -> loop.
pub struct DefaultAgenticLoop {
    llm: Arc<dyn LlmClient>,
    gateway: Arc<dyn OrchestratorGateway>,
    config: LoopConfig,
    state: LoopState,
    stuck_detector: StuckDetector,
    initialized: bool,
    memory: Option<crate::memory_integration::MemoryIntegration>,
    /// Sprint 44+1 — skill bindings active for this loop instance.
    ///
    /// Resolved from `agent.default_skills` + `execution_context
    /// .runtime_skill_bindings`. The chat handler / orchestrator builds
    /// the list once via [`Self::resolve_skill_bindings`] (or sets it
    /// directly via [`Self::set_active_skill_bindings`]) and the
    /// dispatch path reads it via [`AgenticLoop::active_skill_bindings`]
    /// on every tool-call to feed the SkillCompat translator.
    active_skill_bindings: Vec<SkillId>,
    /// US-103b — optional governor that enforces the 4 gates
    /// (wall-clock / token-budget / repetition / compress) on every
    /// iteration. When `None`, the loop preserves its legacy behaviour
    /// (only `IterationBudget` + `StuckDetector` apply).
    governor: Option<AgenticLoopGovernor>,
    /// US-103b — optional output verifier chain. When set, the loop
    /// runs the chain on the assistant's final text before declaring
    /// `Done`; non-Accept directives turn the Done into a feedback
    /// loop (the directive's feedback becomes a new user-role
    /// message and the iteration continues).
    verifier_chain: Option<Arc<VerifierChain>>,
    /// US-103b — verifier context used by the chain. Static for a
    /// session; updated via [`Self::with_verify_ctx`].
    verify_ctx: VerifyCtx,
    /// US-103b — last user prompt captured at `add_user_message` time
    /// so the verifier chain can be invoked with the user's intent
    /// even after multiple iterations have appended tool / assistant
    /// turns. Empty until the first user message is added.
    last_user_prompt: String,
    /// US-103b — whether the most recent verifier-directive triggered a
    /// retry. Caps the retries at 1 per session so a hostile verifier
    /// can't burn the full iteration budget.
    verifier_retry_used: bool,
    /// GAP-4 (2026-05-23) — whether we already nudged the model after an
    /// empty-content completion. Some LLMs (observed: MiniMax) consume
    /// 10k+ thinking tokens then return finish_reason=stop with
    /// content="" or content="   \n". Once per session we inject a
    /// system hint reminding the model to actually emit the answer
    /// inline, then let the loop iterate again. Caps at 1 to avoid
    /// pathological loops (stuck_detector covers repeated identical
    /// behavior beyond that).
    empty_completion_nudged: bool,
    /// BUG-R8-01 — optional stream sink for emitting `StreamEvent::BudgetUpgraded`
    /// SSE frames when a dynamic budget tier promotion fires. When `None`,
    /// the upgrade is still performed and logged via `tracing::info!` but
    /// no SSE frame is emitted.
    sink: Option<std::sync::Arc<dyn StreamSink>>,
}

impl DefaultAgenticLoop {
    /// Create a new loop with the given LLM client and orchestrator gateway.
    pub fn new(llm: Arc<dyn LlmClient>, gateway: Arc<dyn OrchestratorGateway>) -> Self {
        Self {
            llm,
            gateway,
            config: LoopConfig::default(),
            state: LoopState::new(),
            stuck_detector: StuckDetector::new(3),
            initialized: false,
            memory: None,
            active_skill_bindings: Vec::new(),
            governor: None,
            verifier_chain: None,
            verify_ctx: VerifyCtx::default(),
            last_user_prompt: String::new(),
            verifier_retry_used: false,
            empty_completion_nudged: false,
            sink: None,
        }
    }

    /// BUG-R8-01 — install a [`StreamSink`] to receive `BudgetUpgraded` SSE
    /// frames when a dynamic budget tier promotion fires. Calling this is
    /// optional; without it, upgrades are still logged via `tracing::info!`.
    pub fn with_sink(mut self, sink: std::sync::Arc<dyn StreamSink>) -> Self {
        self.sink = Some(sink);
        self
    }

    /// US-103b — install an [`AgenticLoopGovernor`] on this loop.
    ///
    /// When set, every `next_iteration()` consults the governor before
    /// the LLM call. Returns `Self` so it chains naturally with
    /// `with_memory(...)`.
    pub fn with_governor(mut self, governor: AgenticLoopGovernor) -> Self {
        self.governor = Some(governor);
        self
    }

    /// US-103b — convenience: install a governor built from a
    /// [`LoopProfile`] preset. `L1` for short prompts, `L2` for
    /// medium, `L3` for complex multi-turn.
    pub fn with_loop_profile(self, profile: LoopProfile) -> Self {
        self.with_governor(AgenticLoopGovernor::from_profile(profile))
    }

    /// US-103b — install a [`VerifierChain`] on this loop. The chain
    /// runs once per `Done` iteration. Wrapped in `Arc` so callers can
    /// share a single chain across multiple loop instances without
    /// rebuilding it.
    pub fn with_verifier_chain(mut self, chain: Arc<VerifierChain>) -> Self {
        self.verifier_chain = Some(chain);
        self
    }

    /// US-103b — replace the verifier context. Useful when the prompt
    /// metadata declares explicit expectations (e.g. `expected_json_keys`).
    pub fn with_verify_ctx(mut self, ctx: VerifyCtx) -> Self {
        self.verify_ctx = ctx;
        self
    }

    /// US-103b — get a mutable reference to the verifier context so
    /// callers can populate it after-the-fact (e.g. from prompt
    /// metadata extracted during request parsing).
    pub fn verify_ctx_mut(&mut self) -> &mut VerifyCtx {
        &mut self.verify_ctx
    }

    /// US-103b — auto-bind a domain expert skill body into the loop's
    /// system prompt. This is the in-loop equivalent of the
    /// `auto_bind_extra_skills` helper used by the server's
    /// `chat_handler`; tests and alternative callers can use it
    /// without taking a hard dep on the chat-handler crate.
    ///
    /// `skill_name` and `body` are typically `(rule.skill_name,
    /// fs::read_to_string(rule.skill_dir.join("SKILL.md")))` produced
    /// by `cyberclaw_skill_runtime::SkillBinder::match_prompt`. The
    /// caller stays responsible for matching against the user prompt;
    /// the loop only knows how to append the body once the caller has
    /// decided it's relevant. This keeps the agent runtime free of a
    /// hard dep on `cyberclaw-skill-runtime`.
    ///
    /// Works in two modes:
    ///
    /// - **Pre-init** (no `init()` called yet): the section is appended
    ///   to `self.config.system_prompt` so the next `init()` injects it
    ///   into the message list.
    /// - **Post-init**: appended to the existing system message in
    ///   `self.state.messages` (or pushed as a fresh system message if
    ///   none exists). Callers that auto-bind based on the *first* user
    ///   prompt can therefore call this after `init()` + `add_user_message()`.
    ///
    /// Multiple skills can be appended; each is prefixed with a
    /// `## Auto-bound skill: {name}` heading for parity with the
    /// chat-handler injection format.
    pub fn append_skill_to_system_prompt(&mut self, skill_name: &str, body: &str) -> &mut Self {
        let section = format!("\n\n## Auto-bound skill: {skill_name}\n\n{body}");
        // Always update the config so a later re-init still has the bound text.
        self.config.system_prompt.push_str(&section);
        // If init() has already run, also fold the section into the
        // active state's system message so the very next LLM call sees it.
        if self.initialized {
            if let Some(sys_msg) = self
                .state
                .messages
                .iter_mut()
                .find(|m| m.role == Role::System)
            {
                sys_msg.content.push_str(&section);
            } else {
                self.state.messages.insert(0, Message::system(&section));
            }
        }
        self
    }

    /// US-103b — load a SKILL.md from `<skill_dir>/SKILL.md` and append
    /// it to the base system prompt under the canonical
    /// `## Auto-bound skill: {name}` heading. On read failure the loop
    /// logs a warning at `warn` and proceeds (auto-bind is best-effort
    /// — a missing SKILL.md must never break the request path).
    ///
    /// Returns `true` on success, `false` if the file could not be read.
    pub fn append_skill_from_dir(
        &mut self,
        skill_name: &str,
        skill_dir: impl Into<PathBuf>,
    ) -> bool {
        let path = skill_dir.into().join("SKILL.md");
        match std::fs::read_to_string(&path) {
            Ok(body) => {
                self.append_skill_to_system_prompt(skill_name, &body);
                tracing::debug!(
                    skill = skill_name,
                    path = %path.display(),
                    "agentic_loop: SKILL.md auto-bound"
                );
                true
            }
            Err(err) => {
                tracing::warn!(
                    skill = skill_name,
                    path = %path.display(),
                    ?err,
                    "agentic_loop: SKILL.md unreadable, skipping auto-bind"
                );
                false
            }
        }
    }

    /// Attach optional memory integration to this loop.
    ///
    /// When set, memory context is loaded on `init()`, summaries are written
    /// after each iteration, and buffered state is flushed on `finalize()`.
    pub fn with_memory(mut self, memory: crate::memory_integration::MemoryIntegration) -> Self {
        self.memory = Some(memory);
        self
    }

    /// Sprint 44+1 — overwrite the active skill bindings directly.
    ///
    /// Lower-level setter for callers that already resolved the list (e.g.
    /// the chat handler computed `req.skill_ids` -> `Vec<SkillId>` and just
    /// wants to install them). Most callers should prefer
    /// [`Self::resolve_skill_bindings`] which encodes the
    /// agent-default + runtime-override precedence.
    pub fn set_active_skill_bindings(&mut self, bindings: Vec<SkillId>) {
        self.active_skill_bindings = bindings;
    }

    /// Sprint 44+1 — resolve the active skill bindings from the agent's
    /// declared defaults and an optional [`ExecutionContext`] runtime
    /// override.
    ///
    /// Precedence rules (matches the chat-handler contract):
    ///
    /// - `ctx.runtime_skill_bindings = Some(list)` -> the override wins,
    ///   even if `list` is empty (explicit clear).
    /// - `ctx.runtime_skill_bindings = None` (or `ctx = None`) -> fall
    ///   back to `agent_defaults`.
    ///
    /// Returns `&mut Self` to chain after `with_memory(...)`.
    pub fn resolve_skill_bindings(
        &mut self,
        agent_defaults: &[SkillId],
        ctx: Option<&ExecutionContext>,
    ) -> &mut Self {
        self.active_skill_bindings = match ctx.and_then(|c| c.runtime_skill_bindings.as_ref()) {
            Some(override_list) => override_list.clone(),
            None => agent_defaults.to_vec(),
        };
        self
    }

    /// Get a reference to the orchestrator gateway for executing tool calls.
    pub fn gateway(&self) -> &Arc<dyn OrchestratorGateway> {
        &self.gateway
    }

    /// Append a user message to the conversation.
    pub fn add_user_message(&mut self, content: impl Into<String>) {
        let content_str = content.into();
        // US-103b — remember the most recent user prompt so the verifier
        // chain can be invoked with the user's original intent even after
        // tool / assistant turns intervene.
        self.last_user_prompt = content_str.clone();
        self.state.messages.push(Message::user(content_str));
    }

    /// Append a tool result message to the conversation.
    pub fn add_tool_result(&mut self, tool_call_id: String, content: impl Into<String>) {
        self.state
            .messages
            .push(Message::tool(tool_call_id, content));
    }

    /// Inject a synthetic system-role nudge into the conversation. Used by
    /// the dispatch layer to enforce IRON LAW 6 (universal-resilience reflex)
    /// when the model abandons after a single tool rejection — server-side
    /// enforcement of "try one alternative or inline-deliver" before
    /// accepting a Done.
    ///
    /// 2026-05-17: added for the "silent abandon after tool error" fix. See
    /// `chat_handler.rs` `last_iter_all_tools_errored` enforcement logic.
    pub fn add_system_hint(&mut self, content: impl Into<String>) {
        self.state.messages.push(Message::system(content));
    }

    /// Check if the iteration budget has been exhausted. Returns
    /// `Some(reason)` when exhausted with a user-facing message naming
    /// which budget triggered (BT-30); `None` when there is still room.
    fn is_budget_exhausted(&self) -> Option<String> {
        if self.state.iteration_count >= self.config.budget.max_iterations {
            return Some(format!(
                "iteration budget {}/{} exhausted",
                self.state.iteration_count, self.config.budget.max_iterations
            ));
        }
        if self.config.budget.max_tokens > 0
            && self.state.tokens_consumed >= self.config.budget.max_tokens
        {
            return Some(format!(
                "token budget {}/{} exhausted",
                self.state.tokens_consumed, self.config.budget.max_tokens
            ));
        }
        None
    }

    /// Build the ChatRequest for the current state.
    fn build_request(&self) -> ChatRequest {
        let tools = if self.config.tools.is_empty() {
            None
        } else {
            Some(self.config.tools.clone())
        };
        ChatRequest {
            model: self.config.model.clone(),
            messages: self.state.messages.clone(),
            tools,
            // 默认 max_tokens：很多 provider（如 DeepSeek）默认 256-512 → 中文聊天极易截断。
            // 4096 是一个保守的安全上限，可被上游 budget 覆盖。
            max_tokens: Some(4096),
            ..Default::default()
        }
    }
}

#[async_trait]
impl AgenticLoop for DefaultAgenticLoop {
    async fn init(&mut self, config: LoopConfig) -> anyhow::Result<()> {
        self.stuck_detector = StuckDetector::new(config.stuck_threshold);
        self.config = config;
        self.state = LoopState::new();
        self.initialized = true;

        // Inject system prompt as the first message.
        if !self.config.system_prompt.is_empty() {
            let mut sys_msg = Message::system(&self.config.system_prompt);
            // P1.1 — 当配置启用时给 system prompt 打上 ephemeral 缓存
            // 标记。Anthropic 会缓存整段 system 前缀，OpenAI 忽略，
            // 其他 provider 通过 skip_if_none 自然缺省。
            if self.config.cache_system_prompt {
                sys_msg.cache_control = Some(cyberclaw_llm::types::CacheControl::ephemeral());
            }
            self.state.messages.push(sys_msg);
        }

        // If memory integration is configured, load stored context and
        // prepend it as an additional system message so the LLM has
        // prior-session awareness from the start.
        if let Some(ref mut memory) = self.memory {
            let snapshot = memory.load_context()?;
            if !snapshot.formatted_context.is_empty() {
                self.state
                    .messages
                    .push(Message::system(&snapshot.formatted_context));
            }
        }

        Ok(())
    }

    async fn next_iteration(&mut self) -> anyhow::Result<IterationResult> {
        if !self.initialized {
            anyhow::bail!("AgenticLoop not initialized; call init() first");
        }

        // Budget check.
        if let Some(reason) = self.is_budget_exhausted() {
            return Ok(IterationResult::BudgetExhausted(reason));
        }

        // US-103b — governor gate (wall-clock / token-budget / repetition
        // / compress). Runs BEFORE the iteration counter increments so a
        // HardStop on iteration N does not consume budget N.
        if let Some(ref mut governor) = self.governor {
            let ctx = LoopCtx {
                state: &self.state,
                budget: &self.config.budget,
            };
            match governor.pre_iteration(&ctx) {
                LoopDecision::HardStop(reason) => {
                    tracing::warn!(
                        reason = %reason,
                        elapsed_ms = governor.elapsed().as_millis() as u64,
                        "agentic_loop: governor HardStop"
                    );
                    return Ok(IterationResult::BudgetExhausted(reason.to_string()));
                }
                LoopDecision::CompressAndContinue => {
                    tracing::info!(
                        msgs_before = self.state.messages.len(),
                        "agentic_loop: governor CompressAndContinue"
                    );
                    let compressed = governor.compressor_mut().compress_all(&self.state.messages);
                    tracing::info!(
                        msgs_after = compressed.messages.len(),
                        tokens_saved = compressed.tokens_saved_estimate,
                        stages = ?compressed.stages_applied,
                        "agentic_loop: compression applied"
                    );
                    self.state.messages = compressed.messages;
                }
                LoopDecision::Continue { upgrade } => {
                    // BUG-R8-01 — if a budget tier upgrade fired, emit an SSE
                    // frame to the sink (if installed). The tracing::info! log
                    // is emitted inside try_upgrade() regardless of sink.
                    if let Some(ev) = upgrade {
                        if let Some(ref sink) = self.sink {
                            let stream_event = StreamEvent::BudgetUpgraded {
                                from_profile: format!("{:?}", ev.from),
                                to_profile: format!("{:?}", ev.to),
                                new_token_ceiling: ev.new_ceiling,
                            };
                            // Best-effort: ignore send errors (sink may be
                            // closed if the client disconnected).
                            let _ = sink.send(stream_event).await;
                        }
                    }
                }
            }
        }

        self.state.iteration_count += 1;

        // Call the LLM.
        let request = self.build_request();
        let response = self
            .llm
            .chat_completion(request)
            .await
            .map_err(|e| anyhow::anyhow!("LLM call failed: {e}"))?;

        // Capture rate-limit headers from this LLM response.
        if response.rate_limit.is_some() {
            self.state.last_rate_limit = response.rate_limit.clone();
        }

        // Track token usage.
        if let Some(usage) = &response.usage {
            self.state.tokens_consumed += usage.total_tokens as u64;

            // US-103b — feed the governor so its token gate sees the
            // real spend. Even if the governor wasn't installed at
            // construction time this is a no-op (Option::as_mut is None).
            if let Some(ref mut governor) = self.governor {
                governor.record_tokens(usage.total_tokens as u64);
            }

            // P1.1 — emit cache hit/miss telemetry so operators can verify
            // prompt cache 工作。Anthropic 返回 cache_read /
            // cache_creation；OpenAI 在 prompt_tokens_details.cached_tokens
            // 中（需 provider 解析后填入）；MiniMax/Ark/Generic 不返
            // 回 cache 字段时两值为 None，整行 None 跳过日志。
            if usage.cache_read_input_tokens.is_some()
                || usage.cache_creation_input_tokens.is_some()
            {
                tracing::info!(
                    cache_read_input_tokens = usage.cache_read_input_tokens.unwrap_or(0),
                    cache_creation_input_tokens = usage.cache_creation_input_tokens.unwrap_or(0),
                    total_input_tokens = usage.prompt_tokens,
                    "prompt_cache usage"
                );
            }
        }

        // Extract the first choice.
        let choice = response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("LLM returned no choices"))?;

        let mut message = choice.message;

        // DSML synthesis: some vendors (DeepSeek v4) emit tool intent as
        // in-content markup instead of populating tool_calls. Detect and
        // convert so the dispatch path below handles it uniformly.
        let dsml_empty = message.tool_calls.as_ref().is_none_or(|tc| tc.is_empty());
        if dsml_empty {
            if let Some(parsed) = crate::dsml_parser::parse_dsml_tool_calls(&message.content) {
                message.content = crate::dsml_parser::strip_dsml(&message.content);
                message.tool_calls = Some(parsed);
            }
        }

        // Check for tool calls.
        if let Some(ref tool_calls) = message.tool_calls {
            if !tool_calls.is_empty() {
                // Stuck detection.
                if self.stuck_detector.record(tool_calls) {
                    return Ok(IterationResult::Stuck(format!(
                        "Detected {} consecutive identical tool calls",
                        self.config.stuck_threshold
                    )));
                }

                // Record the assistant message with tool calls.
                self.state.messages.push(message.clone());

                // Write iteration summary to memory after tool-call dispatch.
                if let Some(ref mut memory) = self.memory {
                    memory.write_iteration_summary(&self.state)?;
                }

                // US-103b — feed the governor's repetition detector with
                // the ToolCalls signature so a repeating "call X, ignore"
                // pattern trips gate 3 even when the underlying
                // StuckDetector hasn't yet — they use independent
                // thresholds.
                let tc_clone = tool_calls.clone();
                if let Some(ref mut governor) = self.governor {
                    let ctx = LoopCtx {
                        state: &self.state,
                        budget: &self.config.budget,
                    };
                    governor.record_iteration_result(
                        &ctx,
                        &IterationResult::ToolCalls(tc_clone.clone()),
                    );
                }

                return Ok(IterationResult::ToolCalls(tc_clone));
            }
        }

        // No tool calls -- check finish reason.
        let content = message.content.clone();
        self.state.messages.push(message);

        // GAP-4 — treat whitespace-only content as empty. Some LLMs return
        // " \n" or just "\n" after consuming thousands of thinking tokens,
        // and `String::is_empty()` would let that through as a valid Done.
        let content_effectively_empty = content.trim().is_empty();

        // GAP-4 (2026-05-23) — when the model returns finish_reason=stop with
        // empty (or whitespace-only) content for the first time in a session,
        // inject a system nudge before the next iteration. Observed on
        // MiniMax-M2.7 with Rust/SQL code-generation prompts: 11k thinking
        // tokens consumed, 0 content emitted, model exits cleanly. The nudge
        // explicitly tells the model to STOP thinking and output the answer
        // inline as a code block / text reply. Capped at 1 nudge per session
        // (StuckDetector covers identical-repeat failures beyond that).
        if content_effectively_empty
            && choice.finish_reason.as_deref() == Some("stop")
            && !self.empty_completion_nudged
        {
            tracing::warn!(
                "agentic_loop: empty content with finish_reason=stop — injecting GAP-4 retry hint"
            );
            self.state.messages.push(Message::system(
                "ENFORCEMENT (GAP-4 retry): Your previous response returned \
                 finish_reason=stop with empty content. You likely consumed \
                 thinking tokens without emitting the actual answer. Please \
                 output your answer NOW as plain text or a code block — do \
                 not loop into thinking again. If the task requires code, \
                 emit it directly inline; if it requires a count or fact, \
                 state it directly. The user is waiting for output, not \
                 silent reasoning.",
            ));
            self.empty_completion_nudged = true;
        }

        let mut result = match choice.finish_reason.as_deref() {
            Some("stop") => {
                if content_effectively_empty {
                    IterationResult::Continue
                } else {
                    IterationResult::Done(content)
                }
            }
            _ => {
                if content_effectively_empty {
                    IterationResult::Continue
                } else {
                    IterationResult::TextResponse(content)
                }
            }
        };

        // US-103b — VerifierChain hook: if the iteration finished with a
        // `Done`, give the verifier chain a chance to gate the answer.
        // Non-Accept directives:
        //  - `ContinueWithFeedback(msg)` → inject the feedback as a new
        //    user-role message, downgrade the result to `Continue` so
        //    the outer loop runs another iteration.
        //  - `RejectAndRetry(msg)` → same plumbing, but only once per
        //    session (the `verifier_retry_used` latch). After the first
        //    rejection, subsequent Done is accepted as-is to avoid an
        //    adversarial chain burning the iteration budget.
        if let IterationResult::Done(ref text) = result {
            if let Some(chain) = self.verifier_chain.clone() {
                let directive = chain
                    .run(&self.last_user_prompt, text, &self.verify_ctx)
                    .await;
                tracing::debug!(
                    directive = ?directive,
                    retry_used = self.verifier_retry_used,
                    "agentic_loop: verifier_chain directive"
                );
                match directive {
                    VerificationDirective::Accept => {
                        // Pass through.
                    }
                    VerificationDirective::ContinueWithFeedback(fb) => {
                        self.state
                            .messages
                            .push(Message::user(format!("[verifier feedback]\n{fb}")));
                        result = IterationResult::Continue;
                    }
                    VerificationDirective::RejectAndRetry(fb) => {
                        if self.verifier_retry_used {
                            tracing::warn!(
                                feedback = %fb,
                                "agentic_loop: verifier rejected but retry quota exhausted; accepting Done"
                            );
                            // Keep result as Done(text) — accept after one retry.
                        } else {
                            self.state
                                .messages
                                .push(Message::user(format!("[verifier rejected — retry]\n{fb}")));
                            self.verifier_retry_used = true;
                            result = IterationResult::Continue;
                        }
                    }
                }
            }
        }

        // US-103b — feed the governor's repetition detector with whatever
        // the iteration produced (text / tool calls — already handled in
        // the tool-calls early-return branch).
        if let Some(ref mut governor) = self.governor {
            let ctx = LoopCtx {
                state: &self.state,
                budget: &self.config.budget,
            };
            governor.record_iteration_result(&ctx, &result);
        }

        // Write iteration summary to memory at the end of a non-tool-call iteration.
        if let Some(ref mut memory) = self.memory {
            memory.write_iteration_summary(&self.state)?;
        }

        Ok(result)
    }

    async fn finalize(&mut self) -> anyhow::Result<LoopSummary> {
        // Extract final output from the last assistant message.
        let final_output = self
            .state
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Assistant)
            .map(|m| m.content.clone());

        // Flush any buffered memory writes before the loop exits.
        if let Some(ref mut memory) = self.memory {
            memory.flush()?;
        }

        Ok(LoopSummary {
            iterations: self.state.iteration_count,
            tokens_used: self.state.tokens_consumed,
            final_output,
            last_rate_limit: self.state.last_rate_limit.clone(),
        })
    }

    fn active_skill_bindings(&self) -> &[SkillId] {
        &self.active_skill_bindings
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::gateway::{
        CapabilityInfo, CapabilityRequest, CapabilityResult, GatewayError,
    };
    use cyberclaw_llm::error::LlmResult;
    use cyberclaw_llm::prelude::Stream;
    use cyberclaw_llm::types::{ChatChunk, ChatResponse, Choice, FunctionCall, ToolCall, Usage};
    use std::sync::Mutex;

    // -- Mock LLM Client ---------------------------------------------------

    /// A mock LLM that returns pre-configured responses in order.
    struct MockLlm {
        responses: Mutex<Vec<ChatResponse>>,
    }

    impl MockLlm {
        fn new(responses: Vec<ChatResponse>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }

        fn make_text_response(content: &str, finish_reason: &str) -> ChatResponse {
            ChatResponse {
                id: "mock".to_string(),
                object: "chat.completion".to_string(),
                created: 0,
                model: "mock".to_string(),
                choices: vec![Choice {
                    index: 0,
                    message: Message::assistant(content),
                    finish_reason: Some(finish_reason.to_string()),
                }],
                usage: Some(Usage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    total_tokens: 15,
                    ..Default::default()
                }),
                rate_limit: None,
            }
        }

        fn make_tool_call_response(calls: Vec<ToolCall>) -> ChatResponse {
            ChatResponse {
                id: "mock".to_string(),
                object: "chat.completion".to_string(),
                created: 0,
                model: "mock".to_string(),
                choices: vec![Choice {
                    index: 0,
                    message: Message::assistant_with_tools("", calls),
                    finish_reason: Some("tool_calls".to_string()),
                }],
                usage: Some(Usage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    total_tokens: 15,
                    ..Default::default()
                }),
                rate_limit: None,
            }
        }
    }

    #[async_trait]
    impl LlmClient for MockLlm {
        async fn chat_completion(&self, _request: ChatRequest) -> LlmResult<ChatResponse> {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                panic!("MockLlm: no more responses");
            }
            Ok(responses.remove(0))
        }

        async fn chat_completion_stream(
            &self,
            _request: ChatRequest,
        ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>> {
            unimplemented!("streaming not used in agentic loop tests")
        }

        fn provider(&self) -> &str {
            "mock"
        }

        async fn validate_connection(&self) -> LlmResult<()> {
            Ok(())
        }
    }

    // -- Mock Gateway -------------------------------------------------------

    struct MockGateway;

    #[async_trait]
    impl OrchestratorGateway for MockGateway {
        async fn execute_capability(
            &self,
            request: CapabilityRequest,
        ) -> Result<CapabilityResult, GatewayError> {
            Ok(CapabilityResult {
                execution_id: request.execution_id,
                capability_id: request.capability_id,
                output: serde_json::json!({"result": "ok"}),
            })
        }

        async fn list_capabilities(&self) -> Result<Vec<CapabilityInfo>, GatewayError> {
            Ok(vec![])
        }
    }

    // -- Helper -------------------------------------------------------------

    fn make_tool_call(name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: format!("call_{name}"),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: name.to_string(),
                arguments: args.to_string(),
            },
        }
    }

    // -- Tests --------------------------------------------------------------

    #[tokio::test]
    async fn test_normal_loop_tool_call_then_done() {
        let llm = Arc::new(MockLlm::new(vec![
            MockLlm::make_tool_call_response(vec![make_tool_call("file.read", "{}")]),
            MockLlm::make_text_response("Task complete.", "stop"),
        ]));
        let gw = Arc::new(MockGateway);
        let mut loop_ = DefaultAgenticLoop::new(llm, gw);

        loop_
            .init(LoopConfig {
                system_prompt: "You are a helpful agent.".to_string(),
                ..Default::default()
            })
            .await
            .unwrap();

        loop_.add_user_message("Read /tmp/test.txt");

        // Iteration 1: should return tool calls.
        let result = loop_.next_iteration().await.unwrap();
        match &result {
            IterationResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].function.name, "file.read");
            }
            other => panic!("expected ToolCalls, got {other:?}"),
        }

        // Feed tool result back.
        loop_.add_tool_result("call_file.read".to_string(), "file contents here");

        // Iteration 2: should be done.
        let result = loop_.next_iteration().await.unwrap();
        match result {
            IterationResult::Done(text) => assert_eq!(text, "Task complete."),
            other => panic!("expected Done, got {other:?}"),
        }

        // Finalize.
        let summary = loop_.finalize().await.unwrap();
        assert_eq!(summary.iterations, 2);
        assert_eq!(summary.tokens_used, 30); // 15 per call * 2
        assert!(summary.final_output.is_some());
    }

    #[tokio::test]
    async fn test_gap4_empty_stop_continues_then_done() {
        // GAP-4 regression: model returns finish_reason=stop with empty
        // (or whitespace-only) content on iteration 1. Loop must NOT
        // treat that as Done — must inject system nudge and continue.
        // Iteration 2 returns real content -> Done.
        let llm = Arc::new(MockLlm::new(vec![
            // Iter 1: 11k thinking tokens consumed, content is just "\n  ".
            MockLlm::make_text_response("\n  ", "stop"),
            // Iter 2: model produces the actual answer.
            MockLlm::make_text_response(
                "fn parse_kv() -> Vec<(String, String)> { vec![] }",
                "stop",
            ),
        ]));
        let gw = Arc::new(MockGateway);
        let mut loop_ = DefaultAgenticLoop::new(llm, gw);

        loop_
            .init(LoopConfig {
                system_prompt: "You are an agent.".to_string(),
                ..Default::default()
            })
            .await
            .unwrap();

        loop_.add_user_message("Write a Rust function parse_kv.");

        // Iteration 1: whitespace content + stop -> must NOT be Done.
        let result = loop_.next_iteration().await.unwrap();
        assert!(
            matches!(result, IterationResult::Continue),
            "GAP-4: whitespace content + stop must yield Continue, not Done, got {result:?}"
        );
        // Verify the nudge was injected (system message appended).
        assert!(
            loop_
                .state
                .messages
                .iter()
                .any(|m| m.content.contains("ENFORCEMENT (GAP-4 retry)")),
            "GAP-4: nudge system message must be injected on first empty stop"
        );

        // Iteration 2: real content + stop -> Done.
        let result = loop_.next_iteration().await.unwrap();
        match result {
            IterationResult::Done(text) => {
                assert!(text.contains("parse_kv"), "iter 2 must carry real content");
            }
            other => panic!("iter 2 expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_gap4_nudge_capped_at_one_per_session() {
        // Two consecutive empty-stop iterations: first must nudge, second
        // must still Continue (no second nudge — stuck_detector takes
        // over for adversarial loops).
        let llm = Arc::new(MockLlm::new(vec![
            MockLlm::make_text_response("", "stop"),
            MockLlm::make_text_response("   ", "stop"),
        ]));
        let gw = Arc::new(MockGateway);
        let mut loop_ = DefaultAgenticLoop::new(llm, gw);

        loop_.init(LoopConfig::default()).await.unwrap();
        loop_.add_user_message("test");

        let r1 = loop_.next_iteration().await.unwrap();
        assert!(matches!(r1, IterationResult::Continue));
        let nudges_after_iter1 = loop_
            .state
            .messages
            .iter()
            .filter(|m| m.content.contains("ENFORCEMENT (GAP-4 retry)"))
            .count();
        assert_eq!(nudges_after_iter1, 1, "first empty stop must nudge once");

        let r2 = loop_.next_iteration().await.unwrap();
        assert!(matches!(r2, IterationResult::Continue));
        let nudges_after_iter2 = loop_
            .state
            .messages
            .iter()
            .filter(|m| m.content.contains("ENFORCEMENT (GAP-4 retry)"))
            .count();
        assert_eq!(
            nudges_after_iter2, 1,
            "second empty stop must NOT add a second nudge (cap=1)"
        );
    }

    #[tokio::test]
    async fn test_budget_exhausted() {
        let llm = Arc::new(MockLlm::new(vec![
            MockLlm::make_text_response("thinking...", "length"),
            MockLlm::make_text_response("still thinking...", "length"),
        ]));
        let gw = Arc::new(MockGateway);
        let mut loop_ = DefaultAgenticLoop::new(llm, gw);

        loop_
            .init(LoopConfig {
                budget: IterationBudget {
                    max_iterations: 1,
                    ..Default::default()
                },
                ..Default::default()
            })
            .await
            .unwrap();

        loop_.add_user_message("Do something");

        // First iteration uses up the budget (count becomes 1).
        let r = loop_.next_iteration().await.unwrap();
        assert!(matches!(r, IterationResult::TextResponse(_)));

        // Second call should be budget exhausted.
        let r = loop_.next_iteration().await.unwrap();
        // BT-30: reason string must name the budget that triggered + show usage.
        match r {
            IterationResult::BudgetExhausted(reason) => {
                assert!(
                    reason.contains("iteration budget"),
                    "reason should name the budget kind: {reason}"
                );
                assert!(
                    reason.contains("1/1"),
                    "reason should show usage/limit: {reason}"
                );
            }
            other => panic!("expected BudgetExhausted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_token_budget_exhausted() {
        let llm = Arc::new(MockLlm::new(vec![
            MockLlm::make_text_response("first", "length"),
            MockLlm::make_text_response("second", "length"),
        ]));
        let gw = Arc::new(MockGateway);
        let mut loop_ = DefaultAgenticLoop::new(llm, gw);

        loop_
            .init(LoopConfig {
                budget: IterationBudget {
                    max_iterations: 100,
                    max_tokens: 20, // first call uses 15 tokens, second check: 15 >= 20 -> no; need second call
                    ..Default::default()
                },
                ..Default::default()
            })
            .await
            .unwrap();

        loop_.add_user_message("go");

        // First iteration: 15 tokens consumed, budget=20, not exhausted yet.
        let r = loop_.next_iteration().await.unwrap();
        assert!(matches!(r, IterationResult::TextResponse(_)));

        // Second iteration: 30 tokens consumed after this, but budget check is before call.
        // tokens_consumed=15, max_tokens=20, 15 < 20 -> not exhausted, call goes through.
        let r = loop_.next_iteration().await.unwrap();
        assert!(matches!(r, IterationResult::TextResponse(_)));

        // Now tokens_consumed=30, 30 >= 20 -> exhausted.
        let r = loop_.next_iteration().await.unwrap();
        // BT-30: token-budget reason string must name "token budget" + show usage.
        match r {
            IterationResult::BudgetExhausted(reason) => {
                assert!(
                    reason.contains("token budget"),
                    "reason should name token budget: {reason}"
                );
                assert!(
                    reason.contains("30/20"),
                    "reason should show consumed/limit: {reason}"
                );
            }
            other => panic!("expected BudgetExhausted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_stuck_detection() {
        let same_call = vec![make_tool_call("file.read", r#"{"path":"/a"}"#)];
        let llm = Arc::new(MockLlm::new(vec![
            MockLlm::make_tool_call_response(same_call.clone()),
            MockLlm::make_tool_call_response(same_call.clone()),
            MockLlm::make_tool_call_response(same_call.clone()),
        ]));
        let gw = Arc::new(MockGateway);
        let mut loop_ = DefaultAgenticLoop::new(llm, gw);

        loop_
            .init(LoopConfig {
                stuck_threshold: 3,
                ..Default::default()
            })
            .await
            .unwrap();

        loop_.add_user_message("do it");

        // Iterations 1 and 2 return tool calls normally.
        let r1 = loop_.next_iteration().await.unwrap();
        assert!(matches!(r1, IterationResult::ToolCalls(_)));
        loop_.add_tool_result("call_file.read".to_string(), "ok");

        let r2 = loop_.next_iteration().await.unwrap();
        assert!(matches!(r2, IterationResult::ToolCalls(_)));
        loop_.add_tool_result("call_file.read".to_string(), "ok");

        // Iteration 3: stuck detector fires.
        let r3 = loop_.next_iteration().await.unwrap();
        assert!(matches!(r3, IterationResult::Stuck(_)));
    }

    #[tokio::test]
    async fn test_content_part_serde() {
        // Text variant.
        let text = ContentPart::Text {
            value: "hello".to_string(),
        };
        let json = serde_json::to_string(&text).unwrap();
        let back: ContentPart = serde_json::from_str(&json).unwrap();
        match back {
            ContentPart::Text { value } => assert_eq!(value, "hello"),
            _ => panic!("expected Text"),
        }

        // Binary variant.
        let binary = ContentPart::Binary {
            data: vec![0xFF, 0x00, 0xAB],
            media_type: "image/png".to_string(),
        };
        let json = serde_json::to_string(&binary).unwrap();
        let back: ContentPart = serde_json::from_str(&json).unwrap();
        match back {
            ContentPart::Binary { data, media_type } => {
                assert_eq!(data, vec![0xFF, 0x00, 0xAB]);
                assert_eq!(media_type, "image/png");
            }
            _ => panic!("expected Binary"),
        }
    }

    #[tokio::test]
    async fn test_classify_tool_parallelism() {
        let mut contracts = HashMap::new();
        contracts.insert(
            "file.read".to_string(),
            CapabilityBehaviorContract {
                is_read_only: true,
                is_concurrency_safe: true,
                ..Default::default()
            },
        );
        contracts.insert(
            "file.write".to_string(),
            CapabilityBehaviorContract {
                is_read_only: false,
                is_concurrency_safe: false,
                ..Default::default()
            },
        );
        contracts.insert(
            "db.query".to_string(),
            CapabilityBehaviorContract {
                is_read_only: true,
                is_concurrency_safe: false, // read-only but not concurrency-safe
                ..Default::default()
            },
        );

        let calls = vec![
            make_tool_call("file.read", "{}"),
            make_tool_call("file.write", "{}"),
            make_tool_call("db.query", "{}"),
            make_tool_call("unknown.tool", "{}"),
        ];

        let plan = classify_tool_parallelism(&calls, &contracts);

        // Only file.read qualifies for parallel (read_only + concurrency_safe).
        assert_eq!(plan.parallel.len(), 1);
        assert_eq!(plan.parallel[0].function.name, "file.read");

        // file.write, db.query, unknown.tool go sequential.
        assert_eq!(plan.sequential.len(), 3);
        let seq_names: Vec<&str> = plan
            .sequential
            .iter()
            .map(|c| c.function.name.as_str())
            .collect();
        assert!(seq_names.contains(&"file.write"));
        assert!(seq_names.contains(&"db.query"));
        assert!(seq_names.contains(&"unknown.tool"));
    }

    #[test]
    fn test_stuck_detector_reset() {
        let mut detector = StuckDetector::new(2);
        let calls = vec![make_tool_call("x", "{}"), make_tool_call("x", "{}")];

        assert!(!detector.record(&calls));
        assert!(detector.record(&calls));

        detector.reset();
        // After reset, should not be stuck.
        assert!(!detector.record(&calls));
    }

    #[test]
    fn test_stuck_detector_different_calls() {
        let mut detector = StuckDetector::new(3);

        let a = vec![make_tool_call("a", "{}")];
        let b = vec![make_tool_call("b", "{}")];

        assert!(!detector.record(&a));
        assert!(!detector.record(&b));
        assert!(!detector.record(&a)); // a, b, a -- not stuck
    }

    /// 2026-05-17 — Silent-abandon enforcement end-to-end behaviour test.
    ///
    /// Faithfully replays the exact state-tracking logic from
    /// `apps/cyberclaw-server/src/api/chat_handler.rs::run_agentic_loop`
    /// against a scripted mock LLM that *deliberately ignores* the
    /// `guidance` field on tool errors and tries to abandon. The test
    /// drives 3 iterations:
    ///
    /// 1. Mock LLM emits a `fs.list_dir(/)` tool call.
    /// 2. Test simulates the dispatch failure (governance reject)
    ///    via `add_tool_result(..., {"error":"failed"})` and sets
    ///    `last_iter_all_errored = true`.
    /// 3. Mock LLM emits Done("I cannot help.") — the silent-abandon
    ///    failure mode.
    /// 4. Enforcement detects the abandon, injects a system hint via
    ///    `add_system_hint(...)`, sets `forced_retry_used = true`, and
    ///    continues the loop.
    /// 5. Mock LLM emits Done("// code: fn main() {}") — the inline
    ///    delivery that should have happened in step 3.
    /// 6. Enforcement accepts the Done (forced_retry_used short-circuits
    ///    the re-check) and loop terminates.
    ///
    /// Asserts: exactly 3 LLM calls, forced retry fired, final text is
    /// the inline code from step 5 (not the abandon text from step 3).
    /// Without enforcement, iter_count would be 2 and final_text would
    /// be "I cannot help." — proving enforcement physically rescued the
    /// session.
    #[tokio::test]
    async fn test_silent_abandon_enforcement_forces_extra_iteration() {
        let llm = Arc::new(MockLlm::new(vec![
            MockLlm::make_tool_call_response(vec![make_tool_call(
                "fs.list_dir",
                r#"{"path":"/"}"#,
            )]),
            MockLlm::make_text_response("I cannot help.", "stop"),
            MockLlm::make_text_response("// code: fn main() {}", "stop"),
        ]));
        let mut loop_ = DefaultAgenticLoop::new(llm.clone(), Arc::new(MockGateway));
        loop_
            .init(LoopConfig {
                system_prompt: "sys".to_string(),
                ..Default::default()
            })
            .await
            .unwrap();
        loop_.add_user_message("test");

        let mut last_iter_all_errored = false;
        let mut forced_retry_used = false;
        let mut iter_count = 0u32;
        #[allow(unused_assignments)]
        let mut final_text: Option<String> = None;

        loop {
            iter_count += 1;
            assert!(iter_count <= 5, "runaway loop");
            let r = loop_.next_iteration().await.unwrap();
            match r {
                IterationResult::ToolCalls(calls) => {
                    // Simulate all tools failing (governance reject).
                    for c in &calls {
                        loop_.add_tool_result(
                            c.id.clone(),
                            r#"{"error":"failed","guidance":"try again"}"#,
                        );
                    }
                    last_iter_all_errored = !calls.is_empty();
                }
                IterationResult::Done(t) => {
                    if last_iter_all_errored && !forced_retry_used {
                        loop_.add_system_hint("ENFORCEMENT: retry or inline-deliver now");
                        forced_retry_used = true;
                        last_iter_all_errored = false;
                        continue;
                    }
                    final_text = Some(t);
                    break;
                }
                other => panic!("unexpected: {other:?}"),
            }
        }

        assert_eq!(
            iter_count, 3,
            "enforcement should have forced a third iteration (would be 2 without)"
        );
        assert!(forced_retry_used, "forced retry flag should have triggered");
        assert_eq!(
            final_text.as_deref(),
            Some("// code: fn main() {}"),
            "final text must be the post-enforcement inline code, not the pre-enforcement abandon"
        );
    }

    /// 2026-05-17 — Silent-abandon enforcement support test.
    /// Verifies `add_system_hint` appends a Message::system to the conversation
    /// state. The dispatch layer in chat_handler.rs uses this to inject an
    /// IRON LAW 6 enforcement nudge when the model abandons after every tool
    /// call in a batch failed.
    #[tokio::test]
    async fn test_add_system_hint_appends_system_message() {
        let llm = Arc::new(MockLlm::new(vec![MockLlm::make_text_response(
            "ok", "stop",
        )]));
        let gw = Arc::new(MockGateway);
        let mut loop_ = DefaultAgenticLoop::new(llm, gw);

        loop_
            .init(LoopConfig {
                system_prompt: "sys".to_string(),
                ..Default::default()
            })
            .await
            .unwrap();
        loop_.add_user_message("hi");

        let before = loop_.state.messages.len();
        loop_.add_system_hint("ENFORCEMENT: try again");
        let after = loop_.state.messages.len();
        assert_eq!(
            after,
            before + 1,
            "add_system_hint must append exactly one message"
        );

        let last = loop_.state.messages.last().expect("at least one message");
        assert_eq!(
            last.role,
            Role::System,
            "appended message must have System role"
        );
        assert_eq!(last.content, "ENFORCEMENT: try again");
    }

    #[tokio::test]
    async fn test_init_not_called() {
        let llm = Arc::new(MockLlm::new(vec![]));
        let gw = Arc::new(MockGateway);
        let mut loop_ = DefaultAgenticLoop::new(llm, gw);

        let result = loop_.next_iteration().await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not initialized"));
    }

    // ---- Sprint 44+1: skill bindings on AgenticLoop ----

    fn skill(name: &str) -> SkillId {
        SkillId::from_string(name.to_string()).expect("valid skill id")
    }

    #[test]
    fn active_skill_bindings_default_is_empty() {
        let llm = Arc::new(MockLlm::new(vec![]));
        let gw = Arc::new(MockGateway);
        let loop_ = DefaultAgenticLoop::new(llm, gw);
        assert!(loop_.active_skill_bindings().is_empty());
    }

    #[test]
    fn resolve_skill_bindings_uses_agent_defaults_when_no_runtime_override() {
        let llm = Arc::new(MockLlm::new(vec![]));
        let gw = Arc::new(MockGateway);
        let mut loop_ = DefaultAgenticLoop::new(llm, gw);

        let agent_defaults = vec![skill("powerpoint")];
        loop_.resolve_skill_bindings(&agent_defaults, None);
        assert_eq!(loop_.active_skill_bindings().len(), 1);
        assert_eq!(loop_.active_skill_bindings()[0].as_str(), "powerpoint");
    }

    #[test]
    fn resolve_skill_bindings_uses_agent_defaults_when_ctx_has_no_override() {
        let llm = Arc::new(MockLlm::new(vec![]));
        let gw = Arc::new(MockGateway);
        let mut loop_ = DefaultAgenticLoop::new(llm, gw);

        let agent_defaults = vec![skill("powerpoint")];
        let ctx = ExecutionContext::default(); // runtime_skill_bindings = None
        loop_.resolve_skill_bindings(&agent_defaults, Some(&ctx));
        assert_eq!(loop_.active_skill_bindings().len(), 1);
        assert_eq!(loop_.active_skill_bindings()[0].as_str(), "powerpoint");
    }

    #[test]
    fn resolve_skill_bindings_runtime_override_wins_over_agent_defaults() {
        let llm = Arc::new(MockLlm::new(vec![]));
        let gw = Arc::new(MockGateway);
        let mut loop_ = DefaultAgenticLoop::new(llm, gw);

        let agent_defaults = vec![skill("powerpoint")];
        let ctx =
            ExecutionContext::new().with_runtime_skill_bindings(vec![skill("excel"), skill("doc")]);
        loop_.resolve_skill_bindings(&agent_defaults, Some(&ctx));
        let bound = loop_.active_skill_bindings();
        assert_eq!(bound.len(), 2);
        assert_eq!(bound[0].as_str(), "excel");
        assert_eq!(bound[1].as_str(), "doc");
    }

    #[test]
    fn resolve_skill_bindings_runtime_override_explicit_empty_clears_defaults() {
        // Sprint 44+1 contract: Some(vec![]) means "explicit clear".
        let llm = Arc::new(MockLlm::new(vec![]));
        let gw = Arc::new(MockGateway);
        let mut loop_ = DefaultAgenticLoop::new(llm, gw);

        let agent_defaults = vec![skill("powerpoint")];
        let ctx = ExecutionContext::new().with_runtime_skill_bindings(Vec::new());
        loop_.resolve_skill_bindings(&agent_defaults, Some(&ctx));
        assert!(loop_.active_skill_bindings().is_empty());
    }

    #[test]
    fn set_active_skill_bindings_overwrites_in_place() {
        let llm = Arc::new(MockLlm::new(vec![]));
        let gw = Arc::new(MockGateway);
        let mut loop_ = DefaultAgenticLoop::new(llm, gw);

        loop_.set_active_skill_bindings(vec![skill("a"), skill("b")]);
        assert_eq!(loop_.active_skill_bindings().len(), 2);
        loop_.set_active_skill_bindings(vec![skill("c")]);
        assert_eq!(loop_.active_skill_bindings().len(), 1);
        assert_eq!(loop_.active_skill_bindings()[0].as_str(), "c");
    }

    #[test]
    fn agentic_loop_trait_default_is_empty_slice() {
        // Confirms the trait's default impl returns an empty slice — keeps
        // existing implementors source-compatible without forcing them to
        // override `active_skill_bindings`.
        struct EmptyLoop;
        #[async_trait]
        impl AgenticLoop for EmptyLoop {
            async fn init(&mut self, _config: LoopConfig) -> anyhow::Result<()> {
                Ok(())
            }
            async fn next_iteration(&mut self) -> anyhow::Result<IterationResult> {
                Ok(IterationResult::Continue)
            }
            async fn finalize(&mut self) -> anyhow::Result<LoopSummary> {
                Ok(LoopSummary::default())
            }
        }
        let l = EmptyLoop;
        assert!(l.active_skill_bindings().is_empty());
    }

    // ---------------------------------------------------------------------
    // US-103b stitching tests — verify the 3 components (skill auto-bind,
    // governor, verifier_chain) wire together inside DefaultAgenticLoop.
    // ---------------------------------------------------------------------

    /// US-103b — auto-bound SKILL.md body appears in the initial system
    /// prompt so the LLM sees domain expert vocabulary on the first call.
    #[tokio::test]
    async fn us103b_skill_auto_bind_appears_in_system_prompt() {
        use std::fs;
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("domain-expert-web3");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: domain-expert-web3\n---\n# Domain Expert — Web3\nVocabulary: multisig, Safe, Tenderly, Forta.\n",
        )
        .unwrap();

        let llm = Arc::new(MockLlm::new(vec![MockLlm::make_text_response(
            "ok", "stop",
        )]));
        let gw = Arc::new(MockGateway);
        let mut loop_ = DefaultAgenticLoop::new(llm, gw);

        loop_
            .init(LoopConfig {
                system_prompt: "base sys".to_string(),
                ..Default::default()
            })
            .await
            .unwrap();

        // Caller (chat_handler or a test) auto-binds the web3 skill once
        // it has seen the user's first message. Post-init append folds
        // the body into the existing system message.
        let bound = loop_.append_skill_from_dir("domain-expert-web3", &skill_dir);
        assert!(bound, "SKILL.md should be readable");

        // The init message list should now contain a system message that
        // includes both the base prompt and the auto-bound skill section.
        let sys_msg = loop_
            .state
            .messages
            .iter()
            .find(|m| m.role == Role::System)
            .expect("system message exists");
        assert!(sys_msg.content.contains("base sys"));
        assert!(sys_msg
            .content
            .contains("Auto-bound skill: domain-expert-web3"));
        assert!(sys_msg.content.contains("multisig"));
        assert!(sys_msg.content.contains("Tenderly"));
    }

    /// US-103b — governor's token gate trips and turns the next iteration
    /// into a `BudgetExhausted` with the governor's reason.
    #[tokio::test]
    async fn us103b_governor_hard_stop_surfaces_as_budget_exhausted() {
        use crate::loop_governor::{AgenticLoopGovernor, GovernorConfig};
        use std::time::Duration;

        let llm = Arc::new(MockLlm::new(vec![
            MockLlm::make_text_response("first", "stop"),
            MockLlm::make_text_response("never seen", "stop"),
        ]));
        let gw = Arc::new(MockGateway);

        // Token budget so tight the governor trips on iteration 2.
        let governor = AgenticLoopGovernor::new(GovernorConfig {
            wall_clock_budget: Duration::from_secs(60),
            max_total_tokens: 10, // first call records 15 tokens -> trips next time
            compress_threshold_chars: 0,
            repetition_window: 0,
            ..Default::default()
        });

        let mut loop_ = DefaultAgenticLoop::new(llm, gw).with_governor(governor);

        loop_.init(LoopConfig::default()).await.unwrap();
        loop_.add_user_message("hello");

        // First iteration succeeds and records tokens.
        let r1 = loop_.next_iteration().await.unwrap();
        assert!(matches!(r1, IterationResult::Done(_)));

        // Second iteration: governor's token gate has tripped (15 >= 10).
        let r2 = loop_.next_iteration().await.unwrap();
        match r2 {
            IterationResult::BudgetExhausted(reason) => {
                assert!(
                    reason.contains("token"),
                    "expected token-budget reason, got {reason}"
                );
            }
            other => panic!("expected BudgetExhausted from governor, got {other:?}"),
        }
    }

    /// US-103b — full 3-way stitch: skill auto-bind (web3 vocabulary in
    /// system prompt) + governor (records tokens) + verifier_chain
    /// (`RegexAssertVerifier` rejects first Done, accepts second).
    #[tokio::test]
    async fn us103b_skill_governor_verifier_stitched_end_to_end() {
        use crate::loop_governor::{AgenticLoopGovernor, LoopProfile};
        use crate::verify::{RegexAssertVerifier, VerifierChain, VerifyCtx};
        use std::fs;

        // 1) Prepare a fake web3 SKILL.md and bind it.
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("domain-expert-web3");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "# Web3 expert\nDefault vocabulary: multisig Safe Tenderly threshold.\n",
        )
        .unwrap();

        // 2) Mock LLM: iteration 1 emits a reply WITHOUT the required
        //    "multisig" token — verifier RejectAndRetry. Iteration 2 emits
        //    a compliant reply — verifier Accept.
        let llm = Arc::new(MockLlm::new(vec![
            MockLlm::make_text_response("here is your answer (no domain words)", "stop"),
            MockLlm::make_text_response("Use the Safe multisig with threshold 3/5", "stop"),
        ]));
        let gw = Arc::new(MockGateway);

        // 3) Build the verifier chain: response MUST contain "multisig".
        let chain = Arc::new(VerifierChain::new().add(Box::new(
            RegexAssertVerifier::new().with_patterns(["multisig".to_string()]),
        )));

        // 4) Build governor with generous budgets so it does NOT trip — we
        //    only want to confirm it observes the iterations.
        let governor = AgenticLoopGovernor::from_profile(LoopProfile::L3);

        let mut loop_ = DefaultAgenticLoop::new(llm, gw)
            .with_governor(governor)
            .with_verifier_chain(chain)
            .with_verify_ctx(VerifyCtx::new());

        loop_
            .init(LoopConfig {
                system_prompt: "You are a senior on-chain operator.".to_string(),
                ..Default::default()
            })
            .await
            .unwrap();

        loop_.add_user_message("Outline a Safe multisig USDC→ETH runbook");

        // After init + first user msg, simulate the chat_handler's
        // auto-bind step: skill keyword matched, body loaded from
        // disk, folded into the active system message.
        loop_.append_skill_from_dir("domain-expert-web3", &skill_dir);

        // Iteration 1: verifier rejects the first Done; loop returns Continue.
        let r1 = loop_.next_iteration().await.unwrap();
        assert!(
            matches!(r1, IterationResult::Continue),
            "iteration 1 should be downgraded to Continue by verifier rejection, got {r1:?}"
        );

        // Iteration 2: compliant Done passes the verifier.
        let r2 = loop_.next_iteration().await.unwrap();
        match r2 {
            IterationResult::Done(text) => {
                assert!(text.contains("multisig"));
            }
            other => panic!("expected Done on iteration 2, got {other:?}"),
        }

        // Skill auto-bind landed in the system prompt.
        let sys_msg = loop_
            .state
            .messages
            .iter()
            .find(|m| m.role == Role::System)
            .expect("system message");
        assert!(sys_msg.content.contains("Web3 expert"));
        assert!(sys_msg
            .content
            .contains("Auto-bound skill: domain-expert-web3"));

        // Governor observed real token spend (>0 across 2 iterations).
        let gov = loop_.governor.as_ref().expect("governor installed");
        assert!(
            gov.cost_tracker().tokens_consumed_total > 0,
            "governor should have recorded LLM token usage"
        );

        // Verifier retry latch is set (single retry consumed).
        assert!(
            loop_.verifier_retry_used,
            "verifier retry should have fired exactly once"
        );
    }
}
