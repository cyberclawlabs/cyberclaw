//! ## Relationship to other "multi-agent" primitives in this workspace
//!
//! This module is **one of three** distinct multi-agent mechanisms. They
//! have overlapping keywords but solve different problems:
//!
//! | Primitive | Crate / File | Semantics |
//! |---|---|---|
//! | `SubAgentOrchestrator` | `cyberclaw-agent-runtime/src/sub_agent.rs` | Parent agent calls a child agent **as a tool**; child returns a result; parent resumes its own loop. Depth ≤ 3, max children 5. |
//! | `StageHandoffDocument` | `cyberclaw-control-plane/src/stage_handoff.rs` | Pipeline stage-to-stage paseo-style **markdown briefing** (≤ 30 lines) persisted to `<root>/<pipeline_id>/<from_stage>.md`. Linear stage hop, not live. |
//! | `HandoffConnector` | `cyberclaw-connectors/src/handoff.rs` | **Live conversational** transfer (Sprint 21): user is talking to agent_A; agent_A `agent.handoff` capability hands the conversation permanently to agent_B, with a briefing ≤ 2KB. |
//!
//! **This module is the `SubAgentOrchestrator` (tool-call child spawn).** If that
//! isn't what you want, see the matching row above.
//!
//! # Sub-Agent Orchestration
//!
//! Manages child agent lifecycle for subtask delegation. The
//! [`SubAgentOrchestrator`] spawns, runs, cancels, and collects results from
//! child agents while enforcing safety constraints (recursion depth limit,
//! maximum concurrent children, budget fraction).
//!
//! Children always run in autopilot mode via [`AutopilotDelegate`] and inherit
//! a fraction of their parent's remaining budget.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cyberclaw_core::gateway::{CapabilityRequest, OrchestratorGateway};
use cyberclaw_core::ids::{AgentId, CapabilityId, ConnectorId, ExecutionId};
use cyberclaw_llm::client::LlmClient;

use crate::agentic_loop::{
    AgenticLoop, DefaultAgenticLoop, IterationBudget, IterationResult, LoopConfig,
};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors specific to sub-agent orchestration.
#[derive(Debug, thiserror::Error)]
pub enum SubAgentError {
    /// The spawn would exceed the maximum recursion depth.
    #[error("max depth exceeded: current {current}, max {max}")]
    MaxDepthExceeded {
        /// Current depth at which the spawn was attempted.
        current: u32,
        /// Configured maximum depth.
        max: u32,
    },

    /// The parent already has the maximum number of children.
    #[error("max children exceeded: current {current}, max {max}")]
    MaxChildrenExceeded {
        /// Number of children already spawned.
        current: u32,
        /// Configured maximum children.
        max: u32,
    },

    /// The referenced child agent was not found.
    #[error("child not found: {0}")]
    ChildNotFound(AgentId),

    /// A child agent failed during execution.
    #[error("child {agent_id} failed: {reason}")]
    ChildFailed {
        /// The child that failed.
        agent_id: AgentId,
        /// Failure reason.
        reason: String,
    },

    /// The budget has been exhausted.
    #[error("budget exhausted")]
    BudgetExhausted,
}

// ---------------------------------------------------------------------------
// AgentStatus
// ---------------------------------------------------------------------------

/// Lifecycle status of a child agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStatus {
    /// Spawned but not yet started.
    Pending,
    /// Currently executing.
    Running,
    /// Completed successfully with a result string.
    Completed(String),
    /// Failed with an error description.
    Failed(String),
    /// Cancelled before completion.
    Cancelled,
}

// ---------------------------------------------------------------------------
// AgentHandle
// ---------------------------------------------------------------------------

/// Handle representing a spawned child agent.
#[derive(Debug, Clone)]
pub struct AgentHandle {
    /// Unique identifier for this child agent.
    pub agent_id: AgentId,
    /// Identifier of the parent that spawned this child.
    pub parent_id: AgentId,
    /// Recursion depth of this child (parent depth + 1).
    pub depth: u32,
    /// Current lifecycle status.
    pub status: AgentStatus,
    /// Final result text (populated on completion).
    pub result: Option<String>,
    /// The task description assigned to this child.
    task_description: String,
    /// Iteration budget allocated to this child.
    budget: IterationBudget,
}

// ---------------------------------------------------------------------------
// SpawnPolicy
// ---------------------------------------------------------------------------

/// Safety constraints governing sub-agent spawning.
#[derive(Debug, Clone)]
pub struct SpawnPolicy {
    /// Maximum recursion depth (default: 3).
    pub max_depth: u32,
    /// Maximum concurrent children per parent (default: 5).
    pub max_children: u32,
    /// Budget fraction for each child (default: 0.5 = 50% of parent's remaining).
    pub budget_fraction: f64,
    /// Whether children inherit parent's policy rules.
    pub inherit_policies: bool,
}

impl Default for SpawnPolicy {
    fn default() -> Self {
        Self {
            max_depth: 3,
            max_children: 5,
            budget_fraction: 0.5,
            inherit_policies: true,
        }
    }
}

// ---------------------------------------------------------------------------
// ReduceStrategy
// ---------------------------------------------------------------------------

/// Strategy for combining outputs from a batch of completed child agents.
///
/// Used by [`SubAgentOrchestrator::reduce_children`] to fold N child results
/// into a single string the parent agent can hand back to its caller.
#[derive(Debug, Clone)]
pub enum ReduceStrategy {
    /// Join all outputs with `separator` between them.
    Concat {
        /// Separator inserted between every pair of adjacent outputs.
        separator: String,
    },
    /// Pick the trimmed output that appears most often. On ties, the earlier
    /// occurrence wins so the order callers passed children matters.
    MajorityVote,
    /// Send all outputs to the orchestrator's LLM with a synthesizer prompt
    /// and return the LLM's summary text.
    LlmSummary {
        /// Model identifier to use for the summary call.
        model: String,
        /// System prompt steering the summary (tone, length, structure).
        system_prompt: String,
    },
}

impl ReduceStrategy {
    /// `Concat` with the default `"\n---\n"` separator.
    pub fn concat() -> Self {
        Self::Concat {
            separator: "\n---\n".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// SubAgentOrchestrator
// ---------------------------------------------------------------------------

/// Orchestrates spawning, execution, and lifecycle management of child agents.
///
/// Enforces safety constraints from [`SpawnPolicy`] and delegates execution
/// through the standard [`DefaultAgenticLoop`] with [`AutopilotDelegate`].
pub struct SubAgentOrchestrator {
    /// Safety policy for spawning children.
    policy: SpawnPolicy,
    /// Map of child agent ID to its handle.
    children: HashMap<AgentId, AgentHandle>,
    /// LLM client shared with child agents.
    llm: Arc<dyn LlmClient>,
    /// Orchestrator gateway shared with child agents.
    gateway: Arc<dyn OrchestratorGateway>,
    /// Current recursion depth of this orchestrator's parent.
    current_depth: u32,
    /// The identity of the actor that owns this orchestrator, propagated to children.
    caller_identity: cyberclaw_core::identity::ActorRef,
}

impl SubAgentOrchestrator {
    /// Create a new sub-agent orchestrator.
    ///
    /// # Arguments
    ///
    /// * `policy` — Safety constraints for spawning children.
    /// * `llm` — LLM client that children will use.
    /// * `gateway` — Orchestrator gateway for capability execution.
    /// * `current_depth` — The recursion depth of the parent agent.
    /// * `caller_identity` — The identity of the actor that owns this orchestrator.
    pub fn new(
        policy: SpawnPolicy,
        llm: Arc<dyn LlmClient>,
        gateway: Arc<dyn OrchestratorGateway>,
        current_depth: u32,
        caller_identity: cyberclaw_core::identity::ActorRef,
    ) -> Self {
        Self {
            policy,
            children: HashMap::new(),
            llm,
            gateway,
            current_depth,
            caller_identity,
        }
    }

    /// Spawn a new child agent for a subtask.
    ///
    /// Validates depth and children count constraints, then creates a pending
    /// [`AgentHandle`]. The child is not started until [`run_child`](Self::run_child)
    /// is called.
    ///
    /// # Arguments
    ///
    /// * `parent_id` — The parent agent's identifier.
    /// * `task_description` — Human-readable description of the subtask.
    /// * `budget` — The parent's remaining iteration budget; the child receives
    ///   `budget * policy.budget_fraction`.
    ///
    /// # Errors
    ///
    /// Returns [`SubAgentError::MaxDepthExceeded`] or
    /// [`SubAgentError::MaxChildrenExceeded`] if constraints are violated.
    pub fn spawn_child(
        &mut self,
        parent_id: AgentId,
        task_description: String,
        budget: &IterationBudget,
    ) -> Result<AgentId, SubAgentError> {
        let child_depth = self.current_depth + 1;

        // Check depth constraint.
        if child_depth > self.policy.max_depth {
            return Err(SubAgentError::MaxDepthExceeded {
                current: child_depth,
                max: self.policy.max_depth,
            });
        }

        // Check children count constraint.
        let active = self.children.len() as u32;
        if active >= self.policy.max_children {
            return Err(SubAgentError::MaxChildrenExceeded {
                current: active,
                max: self.policy.max_children,
            });
        }

        // Calculate child budget as a fraction of the parent's remaining budget.
        let child_iterations =
            ((budget.max_iterations as f64) * self.policy.budget_fraction).max(1.0) as u32;
        let child_max_tokens = if budget.max_tokens > 0 {
            ((budget.max_tokens as f64) * self.policy.budget_fraction).max(1.0) as u64
        } else {
            0
        };
        let child_timeout_secs =
            ((budget.timeout.as_secs_f64()) * self.policy.budget_fraction).max(1.0);
        let child_budget = IterationBudget {
            max_iterations: child_iterations,
            max_tokens: child_max_tokens,
            timeout: Duration::from_secs_f64(child_timeout_secs),
        };

        let child_id = AgentId::new();

        let handle = AgentHandle {
            agent_id: child_id.clone(),
            parent_id,
            depth: child_depth,
            status: AgentStatus::Pending,
            result: None,
            task_description,
            budget: child_budget,
        };

        self.children.insert(child_id.clone(), handle);
        Ok(child_id)
    }

    /// Run a previously spawned child agent to completion.
    ///
    /// Creates a [`DefaultAgenticLoop`] with the child's allocated budget,
    /// injects the task description as a user message, and iterates until
    /// the loop produces a final result or exhausts its budget.
    ///
    /// # Errors
    ///
    /// Returns [`SubAgentError::ChildNotFound`] if the child ID is unknown,
    /// or [`SubAgentError::ChildFailed`] if execution fails.
    pub async fn run_child(&mut self, child_id: &AgentId) -> Result<String, SubAgentError> {
        // Retrieve and validate the child handle.
        let handle = self
            .children
            .get(child_id)
            .ok_or_else(|| SubAgentError::ChildNotFound(child_id.clone()))?;

        let task_description = handle.task_description.clone();
        let child_budget = handle.budget.clone();

        // Mark as running.
        self.children.get_mut(child_id).unwrap().status = AgentStatus::Running;

        // Build the agentic loop for this child.
        let mut agent_loop = DefaultAgenticLoop::new(self.llm.clone(), self.gateway.clone());

        // Resolve model from LLM_DEFAULT_MODEL env (canonical workspace var)
        // with "gpt-4" as the last-resort fallback. Hardcoding "gpt-4"
        // broke real LLM delegation against MiniMax/Doubao/Claude/Ollama
        // base URLs because they reject the gpt-4 name at validation.
        // Mirrors the F1 fix in apps/cyberclaw-server/src/state.rs:709 for
        // PersistentStoryPlanner — same root cause class.
        let model = std::env::var("LLM_DEFAULT_MODEL").unwrap_or_else(|_| "gpt-4".to_string());
        let config = LoopConfig {
            system_prompt: "You are a sub-agent executing a delegated subtask. Complete the following task precisely.".to_string(),
            model,
            budget: child_budget,
            stuck_threshold: 3,
            // Sub-agents inherit no tool palette by default — the orchestrator
            // explicitly delegates capabilities via gateway scoping. If a
            // sub-agent needs tools, set them here from the parent context.
            tools: Vec::new(),
            // P1.1 — sub-agents don't currently cache: prompt is short and
            // re-injected per child anyway. Parent loop owns the cache hit.
            cache_system_prompt: false,
        };

        if let Err(e) = agent_loop.init(config).await {
            let reason = format!("init failed: {e}");
            let handle = self.children.get_mut(child_id).unwrap();
            handle.status = AgentStatus::Failed(reason.clone());
            return Err(SubAgentError::ChildFailed {
                agent_id: child_id.clone(),
                reason,
            });
        }

        // Inject task as user message.
        agent_loop.add_user_message(&task_description);

        // Run the loop until completion.
        let final_result = loop {
            match agent_loop.next_iteration().await {
                Ok(IterationResult::Done(text)) => break Ok(text),
                Ok(IterationResult::BudgetExhausted(_)) => {
                    // Finalize and return whatever we have.
                    match agent_loop.finalize().await {
                        Ok(summary) => {
                            break Ok(summary
                                .final_output
                                .unwrap_or_else(|| "budget exhausted".to_string()));
                        }
                        Err(e) => {
                            break Err(format!("finalize failed after budget exhaustion: {e}"))
                        }
                    }
                }
                Ok(IterationResult::Stuck(reason)) => {
                    break Err(format!("stuck: {reason}"));
                }
                Ok(IterationResult::ToolCalls(calls)) => {
                    // Dispatch each tool call through the gateway and feed results back.
                    let requested_by = self.caller_identity.clone();
                    for call in &calls {
                        // Parse "connector_id.capability_id" from the function name.
                        // If the name contains no dot, treat the whole name as the
                        // capability_id and use "default" as the connector_id.
                        let (connector_str, capability_str) =
                            if let Some(dot_pos) = call.function.name.find('.') {
                                (
                                    &call.function.name[..dot_pos],
                                    &call.function.name[dot_pos + 1..],
                                )
                            } else {
                                ("default", call.function.name.as_str())
                            };

                        // Parse the tool arguments as JSON; fall back to null on
                        // invalid JSON so the capability still receives a call.
                        let input: serde_json::Value =
                            serde_json::from_str(&call.function.arguments)
                                .unwrap_or(serde_json::Value::Null);

                        // Attempt to build typed IDs; if validation fails, surface
                        // the error as a tool result instead of crashing.
                        let capability_id_result =
                            CapabilityId::from_string(capability_str.to_string());
                        let connector_id_result =
                            ConnectorId::from_string(connector_str.to_string());

                        let tool_result = match (capability_id_result, connector_id_result) {
                            (Ok(capability_id), Ok(connector_id)) => {
                                let request = CapabilityRequest {
                                    execution_id: ExecutionId::new(),
                                    requested_by: requested_by.clone(),
                                    capability_id,
                                    connector_id,
                                    input,
                                    reason: format!("sub-agent tool call: {}", call.function.name),
                                };
                                match self.gateway.execute_capability(request).await {
                                    Ok(result) => result.output.to_string(),
                                    Err(e) => format!("{{\"error\": \"gateway error: {}\"}}", e),
                                }
                            }
                            (Err(e), _) | (_, Err(e)) => format!(
                                "{{\"error\": \"invalid tool name '{}': {}\"}}",
                                call.function.name, e
                            ),
                        };

                        agent_loop.add_tool_result(call.id.clone(), tool_result);
                    }
                    // Continue the loop.
                }
                Ok(IterationResult::TextResponse(_)) | Ok(IterationResult::Continue) => {
                    // Continue the loop.
                }
                Err(e) => {
                    break Err(format!("iteration error: {e}"));
                }
            }
        };

        // Update handle based on result.
        match final_result {
            Ok(text) => {
                let handle = self.children.get_mut(child_id).unwrap();
                handle.status = AgentStatus::Completed(text.clone());
                handle.result = Some(text.clone());
                Ok(text)
            }
            Err(reason) => {
                let handle = self.children.get_mut(child_id).unwrap();
                handle.status = AgentStatus::Failed(reason.clone());
                Err(SubAgentError::ChildFailed {
                    agent_id: child_id.clone(),
                    reason,
                })
            }
        }
    }

    /// Cancel a child agent, setting its status to [`AgentStatus::Cancelled`].
    ///
    /// # Errors
    ///
    /// Returns [`SubAgentError::ChildNotFound`] if the child ID is unknown.
    pub fn cancel_child(&mut self, child_id: &AgentId) -> Result<(), SubAgentError> {
        let handle = self
            .children
            .get_mut(child_id)
            .ok_or_else(|| SubAgentError::ChildNotFound(child_id.clone()))?;
        handle.status = AgentStatus::Cancelled;
        Ok(())
    }

    /// Get the current status of a child agent.
    pub fn get_status(&self, child_id: &AgentId) -> Option<&AgentHandle> {
        self.children.get(child_id)
    }

    /// Collect results from all completed children.
    ///
    /// Returns a vector of `(AgentId, result_text)` pairs for every child
    /// whose status is [`AgentStatus::Completed`].
    pub fn collect_results(&self) -> Vec<(AgentId, String)> {
        self.children
            .values()
            .filter_map(|h| match &h.status {
                AgentStatus::Completed(text) => Some((h.agent_id.clone(), text.clone())),
                _ => None,
            })
            .collect()
    }

    /// Count children that are still running or pending.
    pub fn active_count(&self) -> usize {
        self.children
            .values()
            .filter(|h| matches!(h.status, AgentStatus::Running | AgentStatus::Pending))
            .count()
    }

    /// Run a batch of previously-spawned children sequentially and reduce their
    /// outputs into a single string using the chosen [`ReduceStrategy`].
    ///
    /// Sequential rather than parallel: `run_child` takes `&mut self`, and
    /// switching to parallel needs interior mutability across the handle map.
    /// Sequential is correct enough for the typical "fan-out N short subtasks,
    /// fold into one answer" pattern — and avoids surprising the caller with
    /// concurrent LLM rate-limit hits.
    ///
    /// Failed children contribute an `[error: ...]` sentinel rather than aborting
    /// the whole reduce, so a single child failure doesn't waste all the work.
    ///
    /// # Errors
    ///
    /// Returns [`SubAgentError::ChildFailed`] only when [`ReduceStrategy::LlmSummary`]
    /// is chosen and the synthesis LLM call itself fails. Individual child
    /// failures are absorbed into the output.
    pub async fn reduce_children(
        &mut self,
        child_ids: &[AgentId],
        strategy: ReduceStrategy,
    ) -> Result<String, SubAgentError> {
        if child_ids.is_empty() {
            return Ok(String::new());
        }

        let mut outputs = Vec::with_capacity(child_ids.len());
        for cid in child_ids {
            match self.run_child(cid).await {
                Ok(text) => outputs.push(text),
                Err(e) => outputs.push(format!("[error: {e}]")),
            }
        }

        match strategy {
            ReduceStrategy::Concat { separator } => Ok(outputs.join(&separator)),
            ReduceStrategy::MajorityVote => {
                let mut counts: HashMap<String, usize> = HashMap::new();
                let mut first_seen: HashMap<String, usize> = HashMap::new();
                for (i, text) in outputs.iter().enumerate() {
                    let key = text.trim().to_string();
                    *counts.entry(key.clone()).or_insert(0) += 1;
                    first_seen.entry(key).or_insert(i);
                }
                let winner = counts
                    .iter()
                    .max_by(|(ka, ca), (kb, cb)| {
                        let cmp = ca.cmp(cb);
                        if cmp != std::cmp::Ordering::Equal {
                            cmp
                        } else {
                            // Tie: earlier first_seen should win, which in
                            // max_by semantics means it must compare "greater"
                            let fa = first_seen.get(*ka).copied().unwrap_or(usize::MAX);
                            let fb = first_seen.get(*kb).copied().unwrap_or(usize::MAX);
                            fb.cmp(&fa)
                        }
                    })
                    .map(|(k, _)| k.clone())
                    .unwrap_or_default();
                Ok(winner)
            }
            ReduceStrategy::LlmSummary {
                model,
                system_prompt,
            } => {
                let joined = outputs
                    .iter()
                    .enumerate()
                    .map(|(i, o)| format!("=== Output {} ===\n{}", i + 1, o))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let user_msg = format!(
                    "Synthesize a single response from these {} sub-agent outputs:\n\n{}",
                    outputs.len(),
                    joined
                );
                let request = cyberclaw_llm::types::ChatRequest {
                    model,
                    messages: vec![
                        cyberclaw_llm::types::Message::system(system_prompt),
                        cyberclaw_llm::types::Message::user(user_msg),
                    ],
                    ..Default::default()
                };
                let response = self.llm.chat_completion(request).await.map_err(|e| {
                    SubAgentError::ChildFailed {
                        agent_id: AgentId::new(),
                        reason: format!("LlmSummary reduce failed: {e}"),
                    }
                })?;
                let summary = response
                    .choices
                    .into_iter()
                    .next()
                    .map(|c| c.message.content)
                    .unwrap_or_default();
                Ok(summary)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use cyberclaw_core::gateway::{
        CapabilityInfo, CapabilityRequest, CapabilityResult, GatewayError,
    };
    use cyberclaw_core::identity::Identity;
    use cyberclaw_llm::error::LlmResult;
    use cyberclaw_llm::prelude::Stream;
    use cyberclaw_llm::types::{ChatChunk, ChatRequest, ChatResponse, Choice, Message, Usage};
    use std::sync::Mutex;

    // -- Mock LLM Client ---------------------------------------------------

    struct MockLlm {
        responses: Mutex<Vec<ChatResponse>>,
    }

    impl MockLlm {
        fn new(responses: Vec<ChatResponse>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }

        fn make_done_response(content: &str) -> ChatResponse {
            ChatResponse {
                id: "mock".to_string(),
                object: "chat.completion".to_string(),
                created: 0,
                model: "mock".to_string(),
                choices: vec![Choice {
                    index: 0,
                    message: Message::assistant(content),
                    finish_reason: Some("stop".to_string()),
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
            unimplemented!("streaming not used in sub_agent tests")
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

    // -- Helpers ------------------------------------------------------------

    fn make_policy() -> SpawnPolicy {
        SpawnPolicy::default()
    }

    fn make_budget() -> IterationBudget {
        IterationBudget {
            max_iterations: 10,
            max_tokens: 1000,
            timeout: Duration::from_secs(60),
        }
    }

    fn make_orchestrator(
        policy: SpawnPolicy,
        llm: Arc<dyn LlmClient>,
        gateway: Arc<dyn OrchestratorGateway>,
        depth: u32,
    ) -> SubAgentOrchestrator {
        let caller = Identity::System.to_actor_ref(None).unwrap();
        SubAgentOrchestrator::new(policy, llm, gateway, depth, caller)
    }

    // -- Tests --------------------------------------------------------------

    #[test]
    fn test_spawn_child_success() {
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new(vec![]));
        let gw: Arc<dyn OrchestratorGateway> = Arc::new(MockGateway);
        let mut orch = make_orchestrator(make_policy(), llm, gw, 0);

        let parent_id = AgentId::new();
        let budget = make_budget();

        let result = orch.spawn_child(parent_id.clone(), "do something".to_string(), &budget);
        assert!(result.is_ok());

        let child_id = result.unwrap();
        let handle = orch.get_status(&child_id);
        assert!(handle.is_some());

        let handle = handle.unwrap();
        assert_eq!(handle.parent_id, parent_id);
        assert_eq!(handle.depth, 1);
        assert_eq!(handle.status, AgentStatus::Pending);
        assert!(handle.result.is_none());
    }

    #[test]
    fn test_max_depth_exceeded() {
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new(vec![]));
        let gw: Arc<dyn OrchestratorGateway> = Arc::new(MockGateway);

        let policy = SpawnPolicy {
            max_depth: 2,
            ..Default::default()
        };
        // Current depth is 2, so child would be depth 3 which exceeds max_depth=2.
        let mut orch = make_orchestrator(policy, llm, gw, 2);

        let parent_id = AgentId::new();
        let budget = make_budget();

        let result = orch.spawn_child(parent_id, "task".to_string(), &budget);
        assert!(result.is_err());

        match result.unwrap_err() {
            SubAgentError::MaxDepthExceeded { current, max } => {
                assert_eq!(current, 3);
                assert_eq!(max, 2);
            }
            other => panic!("expected MaxDepthExceeded, got {other:?}"),
        }
    }

    #[test]
    fn test_max_children_exceeded() {
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new(vec![]));
        let gw: Arc<dyn OrchestratorGateway> = Arc::new(MockGateway);

        let policy = SpawnPolicy {
            max_children: 2,
            ..Default::default()
        };
        let mut orch = make_orchestrator(policy, llm, gw, 0);

        let parent_id = AgentId::new();
        let budget = make_budget();

        // Spawn two children successfully.
        assert!(orch
            .spawn_child(parent_id.clone(), "task1".to_string(), &budget)
            .is_ok());
        assert!(orch
            .spawn_child(parent_id.clone(), "task2".to_string(), &budget)
            .is_ok());

        // Third should fail.
        let result = orch.spawn_child(parent_id, "task3".to_string(), &budget);
        assert!(result.is_err());

        match result.unwrap_err() {
            SubAgentError::MaxChildrenExceeded { current, max } => {
                assert_eq!(current, 2);
                assert_eq!(max, 2);
            }
            other => panic!("expected MaxChildrenExceeded, got {other:?}"),
        }
    }

    #[test]
    fn test_budget_fraction_calculation() {
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new(vec![]));
        let gw: Arc<dyn OrchestratorGateway> = Arc::new(MockGateway);

        let policy = SpawnPolicy {
            budget_fraction: 0.25,
            ..Default::default()
        };
        let mut orch = make_orchestrator(policy, llm, gw, 0);

        let parent_id = AgentId::new();
        let budget = IterationBudget {
            max_iterations: 100,
            max_tokens: 10000,
            timeout: Duration::from_secs(200),
        };

        let child_id = orch
            .spawn_child(parent_id, "task".to_string(), &budget)
            .unwrap();

        let handle = orch.get_status(&child_id).unwrap();
        // 100 * 0.25 = 25 iterations
        assert_eq!(handle.budget.max_iterations, 25);
        // 10000 * 0.25 = 2500 tokens
        assert_eq!(handle.budget.max_tokens, 2500);
        // 200 * 0.25 = 50 seconds
        assert_eq!(handle.budget.timeout, Duration::from_secs(50));
    }

    #[test]
    fn test_cancel_child() {
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new(vec![]));
        let gw: Arc<dyn OrchestratorGateway> = Arc::new(MockGateway);
        let mut orch = make_orchestrator(make_policy(), llm, gw, 0);

        let parent_id = AgentId::new();
        let budget = make_budget();

        let child_id = orch
            .spawn_child(parent_id, "task".to_string(), &budget)
            .unwrap();

        assert!(orch.cancel_child(&child_id).is_ok());

        let handle = orch.get_status(&child_id).unwrap();
        assert_eq!(handle.status, AgentStatus::Cancelled);
    }

    #[test]
    fn test_cancel_unknown_child() {
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new(vec![]));
        let gw: Arc<dyn OrchestratorGateway> = Arc::new(MockGateway);
        let mut orch = make_orchestrator(make_policy(), llm, gw, 0);

        let unknown_id = AgentId::new();
        let result = orch.cancel_child(&unknown_id);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SubAgentError::ChildNotFound(_)
        ));
    }

    #[test]
    fn test_collect_results() {
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new(vec![]));
        let gw: Arc<dyn OrchestratorGateway> = Arc::new(MockGateway);
        let mut orch = make_orchestrator(make_policy(), llm, gw, 0);

        let parent_id = AgentId::new();
        let budget = make_budget();

        let child1 = orch
            .spawn_child(parent_id.clone(), "task1".to_string(), &budget)
            .unwrap();
        let child2 = orch
            .spawn_child(parent_id.clone(), "task2".to_string(), &budget)
            .unwrap();
        let child3 = orch
            .spawn_child(parent_id, "task3".to_string(), &budget)
            .unwrap();

        // Manually set statuses to simulate lifecycle.
        orch.children.get_mut(&child1).unwrap().status =
            AgentStatus::Completed("result1".to_string());
        orch.children.get_mut(&child1).unwrap().result = Some("result1".to_string());

        orch.children.get_mut(&child2).unwrap().status = AgentStatus::Failed("error".to_string());

        orch.children.get_mut(&child3).unwrap().status =
            AgentStatus::Completed("result3".to_string());
        orch.children.get_mut(&child3).unwrap().result = Some("result3".to_string());

        let results = orch.collect_results();
        assert_eq!(results.len(), 2);

        let result_ids: Vec<&AgentId> = results.iter().map(|(id, _)| id).collect();
        assert!(result_ids.contains(&&child1));
        assert!(result_ids.contains(&&child3));
    }

    #[test]
    fn test_active_count() {
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new(vec![]));
        let gw: Arc<dyn OrchestratorGateway> = Arc::new(MockGateway);
        let mut orch = make_orchestrator(make_policy(), llm, gw, 0);

        let parent_id = AgentId::new();
        let budget = make_budget();

        let child1 = orch
            .spawn_child(parent_id.clone(), "task1".to_string(), &budget)
            .unwrap();
        let child2 = orch
            .spawn_child(parent_id.clone(), "task2".to_string(), &budget)
            .unwrap();
        let child3 = orch
            .spawn_child(parent_id, "task3".to_string(), &budget)
            .unwrap();

        // All three are Pending -> active.
        assert_eq!(orch.active_count(), 3);

        // Mark one as running, one as completed, one stays pending.
        orch.children.get_mut(&child1).unwrap().status = AgentStatus::Running;
        orch.children.get_mut(&child2).unwrap().status = AgentStatus::Completed("done".to_string());

        // Running + Pending = 2 active.
        assert_eq!(orch.active_count(), 2);

        // Cancel the pending one.
        orch.cancel_child(&child3).unwrap();
        // Only Running = 1 active.
        assert_eq!(orch.active_count(), 1);
    }

    #[test]
    fn test_default_policy_values() {
        let policy = SpawnPolicy::default();
        assert_eq!(policy.max_depth, 3);
        assert_eq!(policy.max_children, 5);
        assert!((policy.budget_fraction - 0.5).abs() < f64::EPSILON);
        assert!(policy.inherit_policies);
    }

    #[tokio::test]
    async fn test_run_child_success() {
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new(vec![MockLlm::make_done_response(
            "subtask complete",
        )]));
        let gw: Arc<dyn OrchestratorGateway> = Arc::new(MockGateway);
        let mut orch = make_orchestrator(make_policy(), llm, gw, 0);

        let parent_id = AgentId::new();
        let budget = make_budget();

        let child_id = orch
            .spawn_child(parent_id, "do the thing".to_string(), &budget)
            .unwrap();

        let result = orch.run_child(&child_id).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "subtask complete");

        let handle = orch.get_status(&child_id).unwrap();
        assert!(matches!(handle.status, AgentStatus::Completed(_)));
        assert_eq!(handle.result.as_deref(), Some("subtask complete"));
    }

    #[tokio::test]
    async fn test_run_unknown_child() {
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new(vec![]));
        let gw: Arc<dyn OrchestratorGateway> = Arc::new(MockGateway);
        let mut orch = make_orchestrator(make_policy(), llm, gw, 0);

        let unknown_id = AgentId::new();
        let result = orch.run_child(&unknown_id).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SubAgentError::ChildNotFound(_)
        ));
    }

    #[tokio::test]
    async fn test_reduce_children_concat() {
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new(vec![
            MockLlm::make_done_response("alpha"),
            MockLlm::make_done_response("beta"),
            MockLlm::make_done_response("gamma"),
        ]));
        let gw: Arc<dyn OrchestratorGateway> = Arc::new(MockGateway);
        let mut orch = make_orchestrator(make_policy(), llm, gw, 0);
        let pid = AgentId::new();
        let b = make_budget();
        let c1 = orch.spawn_child(pid.clone(), "t1".to_string(), &b).unwrap();
        let c2 = orch.spawn_child(pid.clone(), "t2".to_string(), &b).unwrap();
        let c3 = orch.spawn_child(pid, "t3".to_string(), &b).unwrap();
        let out = orch
            .reduce_children(
                &[c1, c2, c3],
                ReduceStrategy::Concat {
                    separator: " | ".to_string(),
                },
            )
            .await
            .unwrap();
        assert_eq!(out, "alpha | beta | gamma");
    }

    #[tokio::test]
    async fn test_reduce_children_majority_vote() {
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new(vec![
            MockLlm::make_done_response("yes"),
            MockLlm::make_done_response("no"),
            MockLlm::make_done_response("yes"),
        ]));
        let gw: Arc<dyn OrchestratorGateway> = Arc::new(MockGateway);
        let mut orch = make_orchestrator(make_policy(), llm, gw, 0);
        let pid = AgentId::new();
        let b = make_budget();
        let c1 = orch.spawn_child(pid.clone(), "q1".to_string(), &b).unwrap();
        let c2 = orch.spawn_child(pid.clone(), "q2".to_string(), &b).unwrap();
        let c3 = orch.spawn_child(pid, "q3".to_string(), &b).unwrap();
        let out = orch
            .reduce_children(&[c1, c2, c3], ReduceStrategy::MajorityVote)
            .await
            .unwrap();
        assert_eq!(out, "yes", "'yes' wins 2-to-1");
    }

    #[tokio::test]
    async fn test_reduce_children_llm_summary() {
        // 3 responses queued: 2 child completions + 1 synthesis call.
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new(vec![
            MockLlm::make_done_response("child 1 finding"),
            MockLlm::make_done_response("child 2 finding"),
            MockLlm::make_done_response("synthesis: both findings agree"),
        ]));
        let gw: Arc<dyn OrchestratorGateway> = Arc::new(MockGateway);
        let mut orch = make_orchestrator(make_policy(), llm, gw, 0);
        let pid = AgentId::new();
        let b = make_budget();
        let c1 = orch.spawn_child(pid.clone(), "t1".to_string(), &b).unwrap();
        let c2 = orch.spawn_child(pid, "t2".to_string(), &b).unwrap();
        let out = orch
            .reduce_children(
                &[c1, c2],
                ReduceStrategy::LlmSummary {
                    model: "mock".to_string(),
                    system_prompt: "synthesize".to_string(),
                },
            )
            .await
            .unwrap();
        assert_eq!(out, "synthesis: both findings agree");
    }

    #[tokio::test]
    async fn test_reduce_children_empty_returns_empty_string() {
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new(vec![]));
        let gw: Arc<dyn OrchestratorGateway> = Arc::new(MockGateway);
        let mut orch = make_orchestrator(make_policy(), llm, gw, 0);
        let out = orch
            .reduce_children(&[], ReduceStrategy::concat())
            .await
            .unwrap();
        assert!(out.is_empty());
    }
}
