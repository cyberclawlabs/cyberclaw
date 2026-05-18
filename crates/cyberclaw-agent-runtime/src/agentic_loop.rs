//! # Agentic Loop
//!
//! Core reasoning loop for CyberClaw agents. The `AgenticLoop` trait defines
//! the iteration protocol: LLM call -> parse response -> tool execution -> repeat.
//!
//! Tool calls are dispatched through `OrchestratorGateway` (never directly
//! to connectors), preserving the governance and audit chain.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use cyberclaw_core::capability_contract::CapabilityBehaviorContract;
use cyberclaw_core::execution::ExecutionContext;
use cyberclaw_core::gateway::OrchestratorGateway;
use cyberclaw_core::ids::SkillId;
use cyberclaw_llm::client::LlmClient;
use cyberclaw_llm::types::{ChatRequest, Message, Role, ToolCall, ToolDefinition};

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
}

// ---------------------------------------------------------------------------
// Loop State
// ---------------------------------------------------------------------------

/// Internal mutable state of the agentic loop.
#[derive(Debug, Clone)]
pub struct LoopState {
    /// Conversation messages accumulated so far.
    pub messages: Vec<Message>,
    /// Number of completed iterations.
    pub iteration_count: u32,
    /// Total tokens consumed across all LLM calls.
    pub tokens_consumed: u64,
}

impl LoopState {
    fn new() -> Self {
        Self {
            messages: Vec::new(),
            iteration_count: 0,
            tokens_consumed: 0,
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
        self.state.messages.push(Message::user(content));
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

        self.state.iteration_count += 1;

        // Call the LLM.
        let request = self.build_request();
        let response = self
            .llm
            .chat_completion(request)
            .await
            .map_err(|e| anyhow::anyhow!("LLM call failed: {e}"))?;

        // Track token usage.
        if let Some(usage) = &response.usage {
            self.state.tokens_consumed += usage.total_tokens as u64;

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
        let dsml_empty = message
            .tool_calls
            .as_ref()
            .is_none_or(|tc| tc.is_empty());
        if dsml_empty {
            if let Some(parsed) =
                crate::dsml_parser::parse_dsml_tool_calls(&message.content)
            {
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

                return Ok(IterationResult::ToolCalls(tool_calls.clone()));
            }
        }

        // No tool calls -- check finish reason.
        let content = message.content.clone();
        self.state.messages.push(message);

        let result = match choice.finish_reason.as_deref() {
            Some("stop") => {
                if content.is_empty() {
                    IterationResult::Continue
                } else {
                    IterationResult::Done(content)
                }
            }
            _ => {
                if content.is_empty() {
                    IterationResult::Continue
                } else {
                    IterationResult::TextResponse(content)
                }
            }
        };

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
        let llm = Arc::new(MockLlm::new(vec![MockLlm::make_text_response("ok", "stop")]));
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
        assert_eq!(after, before + 1, "add_system_hint must append exactly one message");

        let last = loop_.state.messages.last().expect("at least one message");
        assert_eq!(last.role, Role::System, "appended message must have System role");
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
}
