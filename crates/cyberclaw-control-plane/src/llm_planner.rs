//! Sprint 10 (gradual landing): LLM-driven [`ExecutionPlanner`].
//!
//! Replaces the toy [`crate::autopilot_runtime::NoopExecutionPlanner`] with a
//! planner that prompts an [`LlmClient`] to emit JSON shaped to feed the
//! Sprint 10 evidence framework (`ExecutionPlan.expected_outcomes`).
//!
//! # Why land this now
//!
//! The full LLM planning surface (multi-step decomposition, dependency
//! analysis, action authoring with input validation) is sprint-level work.
//! This file lands the **client → planner → plan** seam with three
//! invariants in place:
//!
//! 1. The planner is plug-compatible with `ExecutionPlanner` — the existing
//!    Plan-phase wiring picks it up with no driver changes.
//! 2. LLM call failures or unparseable responses gracefully fall back to a
//!    placeholder plan rather than failing the whole iteration.
//! 3. Output drives the Sprint 10 [`crate::types::ExpectedOutcome`] surface,
//!    so [`crate::autopilot_runtime::EvidenceBasedVerificationGate`] can
//!    consume it directly.
//!
//! Future expansion (action authoring, capability resolution, retry on
//! malformed JSON) layers on top of this same struct.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use cyberclaw_core::ids::{CapabilityId, ConnectorId};
use cyberclaw_llm::types::{ChatRequest, Message};
use cyberclaw_llm::LlmClient;

use crate::autopilot_runtime::ExecutionPlanner;
use crate::autopilot_types::V2IterationState;
use crate::types::{
    default_max_fix_loops, ExecutionPlan, ExpectedOutcome, PlannedAction, Resolution,
};

/// Base system prompt (no capability allowlist). Used when the planner is
/// configured without `capability_allowlist`.
const SYSTEM_PROMPT_BASE: &str = r#"You are an execution planner for an agent platform. Given a goal, return ONLY a JSON object with this exact shape (no prose, no markdown fences):

{
  "expected_outcomes": [
    {"kind": "output_contains", "value": "<substring the result must contain>"},
    {"kind": "status_equals", "value": "completed"}
  ]
}

Rules:
- Output ONLY the JSON object — no commentary, no fences.
- expected_outcomes must be an array (may be empty for trivial goals).
- "kind" is one of: "output_contains", "status_equals".
- Pick 1-3 outcomes that, if all observed, would let a verifier conclude the goal succeeded."#;

/// Extended system prompt suffix when a capability allowlist is configured —
/// instructs the LLM to additionally author concrete `actions`.
const SYSTEM_PROMPT_ACTIONS_SUFFIX: &str = r#"

Additionally, you may include an `actions` array with concrete steps:

{
  "expected_outcomes": [...],
  "actions": [
    {"connector_id": "<id>", "capability_id": "<id>", "input": {...}, "reason": "<why>"}
  ]
}

Rules for actions:
- Use ONLY connector_id + capability_id pairs from the AVAILABLE CAPABILITIES list below. Anything outside the list will be dropped.
- `input` is an arbitrary JSON object passed to the capability.
- `reason` is a one-sentence rationale.

AVAILABLE CAPABILITIES:
"#;

/// Sprint 10 (gradual landing) — corrective user message appended on JSON
/// parse failure when retries are enabled. Concatenated with up-to-200 chars
/// of the failed assistant output for diagnostic clarity.
const RETRY_CORRECTION_TEMPLATE: &str = "Your previous response was not valid JSON. Output ONLY a JSON object matching the schema, no prose, no fences. Previous output: ";

/// JSON shape the LLM is asked to produce. Matches the
/// `expected_outcomes` and `actions` fields of [`ExecutionPlan`].
#[derive(Debug, Deserialize, Default)]
struct LlmPlanShape {
    #[serde(default)]
    expected_outcomes: Vec<ExpectedOutcome>,
    #[serde(default)]
    actions: Vec<LlmActionShape>,
}

/// JSON shape for one action entry — strings to defer ID validation until
/// after parsing (so a malformed entry doesn't fail the whole shape).
#[derive(Debug, Deserialize)]
struct LlmActionShape {
    connector_id: String,
    capability_id: String,
    #[serde(default)]
    input: serde_json::Value,
    #[serde(default)]
    reason: Option<String>,
}

/// LLM-driven planner. Prompts the configured [`LlmClient`] to produce a
/// JSON-shaped plan, then converts it into an [`ExecutionPlan`].
///
/// On any failure (LLM error, empty content, JSON parse failure) returns a
/// fallback placeholder plan with empty `expected_outcomes` — the Verify
/// phase falls back to `Pass` (see
/// [`crate::autopilot_runtime::EvidenceBasedVerificationGate`] semantics) so
/// the iteration is not blocked.
pub struct LlmExecutionPlanner {
    llm_client: Arc<dyn LlmClient>,
    model: String,
    /// Sprint 10 (gradual landing): when set, the planner asks the LLM to also
    /// author concrete `actions` and validates each `(connector_id, capability_id)`
    /// against this whitelist. Actions referencing pairs not in the list are
    /// dropped (logged via `tracing::warn`). When `None`, the prompt asks for
    /// `expected_outcomes` only and any `actions` field in the response is
    /// ignored — preserves v1 behavior.
    capability_allowlist: Option<Vec<(ConnectorId, CapabilityId)>>,
    /// Sprint 10 (gradual landing): on JSON parse failure, retry up to N times
    /// with a corrective "ONLY JSON" prompt + the failed assistant output
    /// appended. Default 0 (single-shot, identical to legacy behavior). LLM-
    /// level errors (network/timeout) bypass retry — only parse errors retry.
    max_retries: u32,
}

impl LlmExecutionPlanner {
    /// Build a planner with the given client and model name. No capability
    /// allowlist is configured by default; retries default to 0 (single-shot).
    pub fn new(llm_client: Arc<dyn LlmClient>, model: impl Into<String>) -> Self {
        Self {
            llm_client,
            model: model.into(),
            capability_allowlist: None,
            max_retries: 0,
        }
    }

    /// Sprint 10 (gradual landing): enable retry-on-parse-failure with the
    /// given max retry count. `0` = single-shot fallback (default). `1-2`
    /// typical for production.
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Sprint 10 (gradual landing): convenience builder — derives the
    /// capability allowlist directly from a [`ConnectorRegistry`] so
    /// callers don't need to construct the Vec manually. Equivalent to
    /// `self.with_capability_allowlist(registry.list_capabilities())`.
    pub fn with_connector_registry(
        self,
        registry: Arc<cyberclaw_connectors::ConnectorRegistry>,
    ) -> Self {
        let allowlist = registry.list_capabilities();
        self.with_capability_allowlist(allowlist)
    }

    /// Sprint 10: enable action authoring by registering a capability allowlist.
    /// The LLM will be prompted with the whitelist and any actions referencing
    /// pairs outside it will be dropped at parse time.
    pub fn with_capability_allowlist(
        mut self,
        allowlist: Vec<(ConnectorId, CapabilityId)>,
    ) -> Self {
        self.capability_allowlist = Some(allowlist);
        self
    }

    fn system_prompt(&self) -> String {
        match &self.capability_allowlist {
            Some(list) => {
                let mut s = String::from(SYSTEM_PROMPT_BASE);
                s.push_str(SYSTEM_PROMPT_ACTIONS_SUFFIX);
                for (cid, cap) in list {
                    s.push_str(&format!(
                        "- connector_id: {}, capability_id: {}\n",
                        cid, cap
                    ));
                }
                s
            }
            None => SYSTEM_PROMPT_BASE.to_string(),
        }
    }
}

#[async_trait]
impl ExecutionPlanner for LlmExecutionPlanner {
    async fn plan(
        &self,
        job: &cyberclaw_core::autopilot::AutopilotJob,
        iteration: &V2IterationState,
    ) -> anyhow::Result<ExecutionPlan> {
        // Trait method discards usage. Callers that need cost tracking
        // call `plan_with_usage` directly on the concrete planner.
        let (plan, _usage) = self.plan_with_usage(job, iteration).await?;
        Ok(plan)
    }
}

impl LlmExecutionPlanner {
    /// Sprint 10 (gradual landing): plan + capture LLM token usage so callers
    /// can feed it into [`cyberclaw_core::execution::ExecutionBudget::record_tokens`]
    /// for cost tracking.
    ///
    /// Returns `(plan, Some(usage))` on the happy path when the LLM provider
    /// reports `usage`. Returns `(fallback_plan, None)` on any failure path
    /// (LLM error, empty content, JSON parse failure) — the fallback plan
    /// has empty `expected_outcomes` so the verifier passes through. Also
    /// returns `(plan, None)` when the provider doesn't report usage (some
    /// self-hosted LLMs omit it).
    pub async fn plan_with_usage(
        &self,
        job: &cyberclaw_core::autopilot::AutopilotJob,
        _iteration: &V2IterationState,
    ) -> anyhow::Result<(ExecutionPlan, Option<cyberclaw_llm::types::Usage>)> {
        let user_msg = format!(
            "Goal: {}\nSuccess criteria:\n{}",
            job.spec.goal.description,
            if job.spec.goal.success_criteria.is_empty() {
                "(none specified)".to_string()
            } else {
                job.spec
                    .goal
                    .success_criteria
                    .iter()
                    .enumerate()
                    .map(|(i, c)| format!("{}. {}", i + 1, c))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        );

        let mut messages = vec![
            Message::system(self.system_prompt()),
            Message::user(user_msg),
        ];

        let fallback = || ExecutionPlan {
            resolution: Resolution {
                agent: job.spec.agent.id.clone(),
                skills: vec![],
                workflow: None,
                connectors: vec![],
                capabilities: vec![],
                reasons: vec![
                    "LlmExecutionPlanner: fallback (LLM error or unparseable)".to_string()
                ],
            },
            actions: vec![],
            review_required: false,
            max_fix_loops: default_max_fix_loops(),
            expected_outcomes: vec![],
        };

        // Accumulated usage across all attempts (initial + retries).
        let mut accumulated_usage: Option<cyberclaw_llm::types::Usage> = None;

        for attempt in 0..=self.max_retries {
            let request = ChatRequest {
                model: self.model.clone(),
                messages: messages.clone(),
                temperature: Some(0.1),
                ..Default::default()
            };

            let response = match self.llm_client.chat_completion(request).await {
                Ok(r) => r,
                Err(e) => {
                    // LLM-level error: don't retry — fall back immediately.
                    tracing::warn!(error = %e, "LlmExecutionPlanner: chat_completion failed; using fallback plan");
                    return Ok((fallback(), accumulated_usage));
                }
            };

            // Accumulate token usage across all attempts.
            if let Some(u) = response.usage.clone() {
                accumulated_usage = Some(match accumulated_usage.take() {
                    None => u,
                    Some(prev) => cyberclaw_llm::types::Usage {
                        prompt_tokens: prev.prompt_tokens + u.prompt_tokens,
                        completion_tokens: prev.completion_tokens + u.completion_tokens,
                        total_tokens: prev.total_tokens + u.total_tokens,
                        // P1.1 — cache token fields accumulate by summing
                        // when present on either side; None on both sides
                        // stays None.
                        cache_read_input_tokens: match (
                            prev.cache_read_input_tokens,
                            u.cache_read_input_tokens,
                        ) {
                            (Some(a), Some(b)) => Some(a + b),
                            (Some(a), None) => Some(a),
                            (None, Some(b)) => Some(b),
                            (None, None) => None,
                        },
                        cache_creation_input_tokens: match (
                            prev.cache_creation_input_tokens,
                            u.cache_creation_input_tokens,
                        ) {
                            (Some(a), Some(b)) => Some(a + b),
                            (Some(a), None) => Some(a),
                            (None, Some(b)) => Some(b),
                            (None, None) => None,
                        },
                    },
                });
            }

            let content = response
                .choices
                .first()
                .map(|c| c.message.content.clone())
                .unwrap_or_default();

            let json_str = strip_optional_fences(&content);
            match serde_json::from_str::<LlmPlanShape>(&json_str) {
                Ok(parsed) => {
                    // Filter actions by allowlist (when configured). Without an
                    // allowlist, ignore actions entirely — the model wasn't told
                    // what's available and we won't dispatch arbitrary capabilities.
                    let actions: Vec<PlannedAction> = match &self.capability_allowlist {
                        Some(list) => parsed
                            .actions
                            .into_iter()
                            .filter_map(|a| {
                                let connector_id = ConnectorId::from_string(a.connector_id.clone())
                                    .ok()?;
                                let capability = CapabilityId::from_string(a.capability_id.clone())
                                    .ok()?;
                                let allowed = list
                                    .iter()
                                    .any(|(c, k)| c == &connector_id && k == &capability);
                                if !allowed {
                                    tracing::warn!(
                                        connector = %a.connector_id,
                                        capability = %a.capability_id,
                                        "LlmExecutionPlanner: action references non-allowlisted capability; dropping"
                                    );
                                    return None;
                                }
                                Some(PlannedAction {
                                    connector_id,
                                    capability,
                                    input: a.input,
                                    reason: a
                                        .reason
                                        .unwrap_or_else(|| "LlmExecutionPlanner action".to_string()),
                                })
                            })
                            .collect(),
                        None => Vec::new(),
                    };

                    let n_outcomes = parsed.expected_outcomes.len();
                    let n_actions = actions.len();
                    let plan = ExecutionPlan {
                        resolution: Resolution {
                            agent: job.spec.agent.id.clone(),
                            skills: vec![],
                            workflow: None,
                            connectors: vec![],
                            capabilities: vec![],
                            reasons: vec![format!(
                                "LlmExecutionPlanner: {} expected outcome(s), {} action(s) extracted from goal",
                                n_outcomes, n_actions
                            )],
                        },
                        actions,
                        review_required: false,
                        max_fix_loops: default_max_fix_loops(),
                        expected_outcomes: parsed.expected_outcomes,
                    };
                    return Ok((plan, accumulated_usage));
                }
                Err(e) => {
                    let preview: String = content.chars().take(200).collect();
                    if attempt < self.max_retries {
                        tracing::warn!(
                            error = %e,
                            attempt = attempt + 1,
                            max_retries = self.max_retries,
                            content_preview = %preview,
                            "LlmExecutionPlanner: JSON parse failed; retrying with corrective prompt"
                        );
                        // Append the failed assistant response and a corrective
                        // user message for the next attempt.
                        messages.push(Message::assistant(content));
                        let correction = format!("{}{}", RETRY_CORRECTION_TEMPLATE, preview);
                        messages.push(Message::user(correction));
                    } else {
                        tracing::warn!(
                            error = %e,
                            content_preview = %preview,
                            "LlmExecutionPlanner: JSON parse failed; using fallback plan"
                        );
                    }
                }
            }
        }

        // All retries exhausted — fall back.
        Ok((fallback(), accumulated_usage))
    }
}

/// Best-effort markdown-fence strip. Some models wrap JSON in ```json ... ```
/// despite the prompt; we tolerate it. Returns the trimmed inner body, or the
/// trimmed input if no fence is detected.
fn strip_optional_fences(content: &str) -> String {
    let trimmed = content.trim();
    for fence in ["```json", "```"] {
        if let Some(stripped) = trimmed.strip_prefix(fence) {
            if let Some(end) = stripped.rfind("```") {
                return stripped[..end].trim().to_string();
            }
        }
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use cyberclaw_core::autopilot::{
        AutopilotGoal, AutopilotJob, AutopilotSpec, AutopilotTrigger, SessionMode,
    };
    use cyberclaw_core::capability::RiskLevel;
    use cyberclaw_core::execution::{AgentRef, ExecutionBudget};
    use cyberclaw_core::ids::{AgentId, AutopilotJobId};
    use cyberclaw_llm::error::LlmResult;
    use cyberclaw_llm::types::{ChatChunk, ChatResponse, Choice};
    use futures::stream::Stream;
    use std::sync::Mutex;

    use crate::autopilot_types::{AutopilotPhase, AutopilotStep, V2IterationState};

    fn job_with_goal(description: &str, success_criteria: Vec<String>) -> AutopilotJob {
        let agent_id = AgentId::from_string("test-agent".to_string()).unwrap();
        AutopilotJob {
            id: AutopilotJobId::new(),
            name: "test-job".to_string(),
            description: None,
            case_id: None,
            spec: AutopilotSpec {
                goal: AutopilotGoal {
                    description: description.to_string(),
                    success_criteria,
                    max_iterations: 10,
                    max_duration_ms: None,
                },
                trigger: AutopilotTrigger::Manual,
                stop_conditions: vec![],
                session_mode: SessionMode::IsolatedSession,
                agent: AgentRef {
                    id: agent_id,
                    role: "test".to_string(),
                },
                budget: ExecutionBudget::default(),
                max_risk_level: RiskLevel::Low,
                auto_approve_low_risk: true,
            },
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn make_iter() -> V2IterationState {
        V2IterationState {
            iteration_id: 1,
            step: AutopilotStep::Plan,
            start_time: Utc::now(),
            end_time: None,
            state_hash: String::new(),
            progress_delta: None,
            execution_results: Vec::new(),
            current_phase: AutopilotPhase::Plan,
            fix_loop_count: 0,
            plan_mode_snapshot: None,
        }
    }

    /// Test LlmClient that returns a preset content body and optional usage.
    /// `content` is a Mutex<String> for legacy single-shot tests; for retry
    /// tests use `content_sequence` (consumed in order; falls back to the
    /// last element when exhausted).
    struct StubLlmClient {
        content: Mutex<String>,
        content_sequence: Mutex<Vec<String>>,
        call_count: std::sync::atomic::AtomicUsize,
        should_fail: bool,
        usage_to_return: Option<cyberclaw_llm::types::Usage>,
    }

    #[async_trait]
    impl LlmClient for StubLlmClient {
        async fn chat_completion(&self, _req: ChatRequest) -> LlmResult<ChatResponse> {
            if self.should_fail {
                return Err(cyberclaw_llm::error::LlmError::Internal(
                    "injected failure".to_string(),
                ));
            }
            let idx = self
                .call_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // If sequence is configured use it; else fall back to single content.
            let body = {
                let seq = self.content_sequence.lock().unwrap();
                if !seq.is_empty() {
                    let i = idx.min(seq.len() - 1);
                    seq[i].clone()
                } else {
                    self.content.lock().unwrap().clone()
                }
            };
            Ok(ChatResponse {
                id: "test-id".to_string(),
                object: "chat.completion".to_string(),
                created: 0,
                model: "stub".to_string(),
                choices: vec![Choice {
                    index: 0,
                    message: Message::assistant(body),
                    finish_reason: Some("stop".to_string()),
                }],
                usage: self.usage_to_return.clone(),
                rate_limit: None,
            })
        }
        async fn chat_completion_stream(
            &self,
            _req: ChatRequest,
        ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>> {
            unreachable!("streaming not exercised in planner tests")
        }
        fn provider(&self) -> &str {
            "stub"
        }
        async fn validate_connection(&self) -> LlmResult<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn plan_with_usage_returns_usage_when_provider_reports_it() {
        // Sprint 10 (gradual landing): plan_with_usage must surface the LLM's
        // token usage so callers can charge it against ExecutionBudget.
        use cyberclaw_core::execution::ExecutionBudget;
        use cyberclaw_llm::types::Usage;

        let usage = Usage {
            prompt_tokens: 50,
            completion_tokens: 25,
            total_tokens: 75,
            ..Default::default()
        };
        let client = Arc::new(StubLlmClient {
            content: Mutex::new(r#"{"expected_outcomes":[]}"#.to_string()),
            content_sequence: Mutex::new(Vec::new()),
            call_count: std::sync::atomic::AtomicUsize::new(0),
            should_fail: false,
            usage_to_return: Some(usage.clone()),
        });
        let planner = LlmExecutionPlanner::new(client, "stub-model");
        let job = job_with_goal("test", vec![]);
        let iter = make_iter();
        let (_plan, returned_usage) = planner.plan_with_usage(&job, &iter).await.unwrap();
        let returned = returned_usage.expect("provider reported usage");
        assert_eq!(returned.prompt_tokens, 50);
        assert_eq!(returned.completion_tokens, 25);

        // Show the contract: caller can feed the usage into ExecutionBudget.
        let mut budget = ExecutionBudget::default();
        budget.record_tokens(returned.prompt_tokens, returned.completion_tokens);
        assert_eq!(budget.tokens_used, 75);
    }

    #[tokio::test]
    async fn plan_with_usage_returns_none_when_provider_omits_usage() {
        // Some self-hosted LLMs don't report usage. Callers must tolerate None.
        let client = Arc::new(StubLlmClient {
            content: Mutex::new(r#"{"expected_outcomes":[]}"#.to_string()),
            content_sequence: Mutex::new(Vec::new()),
            call_count: std::sync::atomic::AtomicUsize::new(0),
            should_fail: false,
            usage_to_return: None,
        });
        let planner = LlmExecutionPlanner::new(client, "stub-model");
        let job = job_with_goal("test", vec![]);
        let iter = make_iter();
        let (_plan, returned_usage) = planner.plan_with_usage(&job, &iter).await.unwrap();
        assert!(returned_usage.is_none());
    }

    #[tokio::test]
    async fn plan_with_usage_returns_none_on_llm_error_fallback() {
        // Fallback path must return (fallback_plan, None) — caller doesn't
        // charge tokens for a failed call.
        let client = Arc::new(StubLlmClient {
            content: Mutex::new(String::new()),
            content_sequence: Mutex::new(Vec::new()),
            call_count: std::sync::atomic::AtomicUsize::new(0),
            should_fail: true,
            usage_to_return: None,
        });
        let planner = LlmExecutionPlanner::new(client, "stub-model");
        let job = job_with_goal("test", vec![]);
        let iter = make_iter();
        let (plan, returned_usage) = planner.plan_with_usage(&job, &iter).await.unwrap();
        assert!(returned_usage.is_none());
        assert!(plan.resolution.reasons[0].contains("fallback"));
    }

    #[tokio::test]
    async fn planner_extracts_outcomes_from_well_formed_json() {
        let client = Arc::new(StubLlmClient {
            content: Mutex::new(
                r#"{"expected_outcomes":[
                    {"kind":"output_contains","value":"hello"},
                    {"kind":"status_equals","value":"completed"}
                ]}"#
                .to_string(),
            ),
            content_sequence: Mutex::new(Vec::new()),
            call_count: std::sync::atomic::AtomicUsize::new(0),
            should_fail: false,
            usage_to_return: None,
        });
        let planner = LlmExecutionPlanner::new(client, "stub-model");
        let job = job_with_goal("greet the user", vec!["should say hello".to_string()]);
        let iter = make_iter();
        let plan = planner.plan(&job, &iter).await.unwrap();
        assert_eq!(plan.expected_outcomes.len(), 2);
        assert!(matches!(
            plan.expected_outcomes[0],
            ExpectedOutcome::OutputContains(ref s) if s == "hello"
        ));
        assert!(matches!(
            plan.expected_outcomes[1],
            ExpectedOutcome::StatusEquals(ref s) if s == "completed"
        ));
    }

    #[tokio::test]
    async fn planner_strips_markdown_fences() {
        let client = Arc::new(StubLlmClient {
            content: Mutex::new(
                "```json\n{\"expected_outcomes\":[{\"kind\":\"output_contains\",\"value\":\"foo\"}]}\n```"
                    .to_string(),
            ),
            content_sequence: Mutex::new(Vec::new()),
            call_count: std::sync::atomic::AtomicUsize::new(0),
            should_fail: false,
            usage_to_return: None,
        });
        let planner = LlmExecutionPlanner::new(client, "stub-model");
        let job = job_with_goal("test", vec![]);
        let iter = make_iter();
        let plan = planner.plan(&job, &iter).await.unwrap();
        assert_eq!(plan.expected_outcomes.len(), 1);
    }

    #[tokio::test]
    async fn planner_falls_back_when_llm_errors() {
        let client = Arc::new(StubLlmClient {
            content: Mutex::new(String::new()),
            content_sequence: Mutex::new(Vec::new()),
            call_count: std::sync::atomic::AtomicUsize::new(0),
            should_fail: true,
            usage_to_return: None,
        });
        let planner = LlmExecutionPlanner::new(client, "stub-model");
        let job = job_with_goal("test", vec![]);
        let iter = make_iter();
        let plan = planner.plan(&job, &iter).await.unwrap();
        assert!(plan.expected_outcomes.is_empty());
        assert!(plan.resolution.reasons[0].contains("fallback"));
    }

    #[tokio::test]
    async fn with_connector_registry_derives_allowlist_from_registry() {
        // Sprint 10 (gradual landing): with_connector_registry pulls the
        // capability list directly from a ConnectorRegistry, bypassing manual
        // Vec construction. Even an empty registry produces an allowlist
        // (length 0) which means actions are still gated on (no allowlist
        // → all actions dropped via the existing safety net).
        let registry = Arc::new(cyberclaw_connectors::ConnectorRegistry::new());
        let client = Arc::new(StubLlmClient {
            content: Mutex::new(
                r#"{"expected_outcomes":[],"actions":[
                    {"connector_id":"local","capability_id":"fs.read","input":{},"reason":"x"}
                ]}"#
                .to_string(),
            ),
            content_sequence: Mutex::new(Vec::new()),
            call_count: std::sync::atomic::AtomicUsize::new(0),
            should_fail: false,
            usage_to_return: None,
        });
        let planner = LlmExecutionPlanner::new(client, "stub-model")
            .with_connector_registry(registry.clone());
        let job = job_with_goal("test", vec![]);
        let iter = make_iter();
        let plan = planner.plan(&job, &iter).await.unwrap();
        // Empty registry → empty allowlist → no actions survive (defense:
        // even with allowlist set, every LLM-emitted action is dropped
        // because nothing matches).
        assert!(plan.actions.is_empty());
    }

    #[tokio::test]
    async fn planner_with_allowlist_keeps_actions_in_list_drops_others() {
        // Sprint 10 (gradual landing): allowlist filters LLM-authored actions.
        // LLM emits 2 actions: one allowlisted, one not. Only the first survives.
        let client = Arc::new(StubLlmClient {
            content: Mutex::new(
                r#"{
                    "expected_outcomes": [],
                    "actions": [
                        {"connector_id":"local","capability_id":"fs.read","input":{"path":"/tmp/x"},"reason":"read file"},
                        {"connector_id":"evil","capability_id":"shell.exec","input":{},"reason":"escape allowlist"}
                    ]
                }"#
                    .to_string(),
            ),
            content_sequence: Mutex::new(Vec::new()),
            call_count: std::sync::atomic::AtomicUsize::new(0),
            should_fail: false,
            usage_to_return: None,
        });
        let allowlist = vec![(
            ConnectorId::from_string("local".to_string()).unwrap(),
            CapabilityId::from_string("fs.read".to_string()).unwrap(),
        )];
        let planner =
            LlmExecutionPlanner::new(client, "stub-model").with_capability_allowlist(allowlist);
        let job = job_with_goal("read x", vec![]);
        let iter = make_iter();
        let plan = planner.plan(&job, &iter).await.unwrap();
        assert_eq!(plan.actions.len(), 1, "non-allowlisted action dropped");
        assert_eq!(plan.actions[0].connector_id.as_str(), "local");
        assert_eq!(plan.actions[0].capability.as_str(), "fs.read");
        assert_eq!(plan.actions[0].reason, "read file");
    }

    #[tokio::test]
    async fn planner_without_allowlist_ignores_actions_field() {
        // Sprint 10 (gradual landing): without an allowlist, the planner does
        // NOT dispatch arbitrary actions even if the LLM hallucinates them.
        let client = Arc::new(StubLlmClient {
            content: Mutex::new(
                r#"{
                    "expected_outcomes": [],
                    "actions": [
                        {"connector_id":"any","capability_id":"any.cap","input":{},"reason":"unsafe"}
                    ]
                }"#
                    .to_string(),
            ),
            content_sequence: Mutex::new(Vec::new()),
            call_count: std::sync::atomic::AtomicUsize::new(0),
            should_fail: false,
            usage_to_return: None,
        });
        let planner = LlmExecutionPlanner::new(client, "stub-model");
        let job = job_with_goal("test", vec![]);
        let iter = make_iter();
        let plan = planner.plan(&job, &iter).await.unwrap();
        assert!(
            plan.actions.is_empty(),
            "no allowlist → actions dropped regardless of LLM output"
        );
    }

    #[tokio::test]
    async fn planner_retries_on_parse_failure_then_succeeds() {
        // Sprint 10 (gradual landing): max_retries enabled; 1st response is
        // garbage, 2nd is valid JSON. Planner should succeed on the retry.
        let client = Arc::new(StubLlmClient {
            content: Mutex::new(String::new()),
            content_sequence: Mutex::new(vec![
                "not valid json — model hallucination".to_string(),
                r#"{"expected_outcomes":[{"kind":"output_contains","value":"recovered"}]}"#
                    .to_string(),
            ]),
            call_count: std::sync::atomic::AtomicUsize::new(0),
            should_fail: false,
            usage_to_return: Some(cyberclaw_llm::types::Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
                ..Default::default()
            }),
        });
        let planner = LlmExecutionPlanner::new(client.clone(), "stub-model").with_max_retries(1);
        let job = job_with_goal("recover from bad output", vec![]);
        let iter = make_iter();
        let (plan, usage) = planner.plan_with_usage(&job, &iter).await.unwrap();
        assert_eq!(plan.expected_outcomes.len(), 1);
        assert!(matches!(
            plan.expected_outcomes[0],
            ExpectedOutcome::OutputContains(ref s) if s == "recovered"
        ));
        // Two calls × 15 total_tokens = 30 accumulated.
        let u = usage.expect("usage accumulated across attempts");
        assert_eq!(u.total_tokens, 30);
        assert_eq!(
            client.call_count.load(std::sync::atomic::Ordering::SeqCst),
            2
        );
    }

    #[tokio::test]
    async fn planner_falls_back_after_max_retries_exhausted() {
        // Every call returns garbage. With max_retries=2, the planner makes
        // 3 total attempts (initial + 2 retries) then falls back.
        let client = Arc::new(StubLlmClient {
            content: Mutex::new("permanent garbage".to_string()),
            content_sequence: Mutex::new(Vec::new()),
            call_count: std::sync::atomic::AtomicUsize::new(0),
            should_fail: false,
            usage_to_return: None,
        });
        let planner = LlmExecutionPlanner::new(client.clone(), "stub-model").with_max_retries(2);
        let job = job_with_goal("test", vec![]);
        let iter = make_iter();
        let (plan, _usage) = planner.plan_with_usage(&job, &iter).await.unwrap();
        assert!(plan.expected_outcomes.is_empty());
        assert!(plan.resolution.reasons[0].contains("fallback"));
        assert_eq!(
            client.call_count.load(std::sync::atomic::Ordering::SeqCst),
            3
        );
    }

    #[tokio::test]
    async fn planner_falls_back_when_json_parse_fails() {
        let client = Arc::new(StubLlmClient {
            content: Mutex::new("not valid JSON at all".to_string()),
            content_sequence: Mutex::new(Vec::new()),
            call_count: std::sync::atomic::AtomicUsize::new(0),
            should_fail: false,
            usage_to_return: None,
        });
        let planner = LlmExecutionPlanner::new(client, "stub-model");
        let job = job_with_goal("test", vec![]);
        let iter = make_iter();
        let plan = planner.plan(&job, &iter).await.unwrap();
        assert!(plan.expected_outcomes.is_empty());
        assert!(plan.resolution.reasons[0].contains("fallback"));
    }
}
