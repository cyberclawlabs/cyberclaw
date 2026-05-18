//! Sprint 21 path #2 — LLM-backed implementation of `EvolutionDispatcher`.
//!
//! Wires the `EvolutionOrchestrator` (already in this crate) to a real
//! LLM client so the description-optimisation loop can actually run on
//! a deployed CyberClaw agent. Before this commit, the orchestrator
//! infrastructure existed but had no concrete dispatcher — the
//! `learning/evolution/timeline` HTTP endpoint returned `{"cycles":[]}`
//! because nothing was producing cycles.
//!
//! ## Production caveats (honest scope)
//!
//! This is a **starter** dispatcher. The contract is correct (the trait
//! is satisfied, the orchestrator can iterate), but production-grade
//! quality needs more sprint work specifically on the LLM prompt
//! engineering side:
//!
//! 1. **Mutation prompt** — the `execute_mutation` impl asks the LLM to
//!    "improve" the description against optional failure feedback. Real
//!    deployments need few-shot examples of *good* improvements
//!    calibrated against your specific operator workflow.
//! 2. **Case scoring is binary hit/miss.** Production needs partial
//!    credit (e.g. "matched but recommended a wrong tool"), confidence
//!    weighting, and per-model calibration since different LLMs answer
//!    "would you select this skill?" with different temperatures.
//! 3. **No multi-model voting.** Today the same LLM both rewrites and
//!    scores, which biases evaluation. Production should split: cheap
//!    model rewrites, stronger/different model scores.
//! 4. **Smoke regression returns trivially passing.** Real smoke checks
//!    should re-run a curated set of high-value baseline triggers and
//!    fail if the variant breaks any of them. The infrastructure
//!    accepts that data shape; we just don't populate it yet.
//!
//! These are documented inline as `// PRODUCTION:` comments. Replacing
//! them is a focused 2-3 day sprint.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use cyberclaw_llm::types::{ChatRequest, Message, Role};
use cyberclaw_llm::LlmClient;
use tracing::{debug, warn};

use crate::evolution_orchestrator::EvolutionDispatcher;
use crate::fitness_evaluator::{CaseOutcome, EvaluationCase};
use crate::mutation_engine::{MutationPlan, MutationResult};
use crate::skill_archive::SkillVariant;
use crate::verification_gate::{CommandResult, RegressionResult};

/// LLM-backed dispatcher. Owns no state beyond the LLM client + model
/// selection — every call is independent.
pub struct LlmEvolutionDispatcher {
    llm_client: Arc<dyn LlmClient>,
    model: String,
}

impl LlmEvolutionDispatcher {
    pub fn new(llm_client: Arc<dyn LlmClient>, model: impl Into<String>) -> Self {
        Self {
            llm_client,
            model: model.into(),
        }
    }

    /// Build a one-shot chat completion request. The orchestrator never
    /// streams; it always wants a complete reply per round-trip.
    fn build_request(&self, system: &str, user: &str) -> ChatRequest {
        ChatRequest {
            model: self.model.clone(),
            messages: vec![
                Message {
                    role: Role::System,
                    content: system.to_string(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    cache_control: None,
                },
                Message {
                    role: Role::User,
                    content: user.to_string(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    cache_control: None,
                },
            ],
            temperature: Some(0.2),
            max_tokens: Some(2048),
            stream: Some(false),
            ..Default::default()
        }
    }
}

#[async_trait]
impl EvolutionDispatcher for LlmEvolutionDispatcher {
    /// Generate a child variant by asking the LLM to rewrite the
    /// parent skill text against any feedback from the previous round.
    ///
    /// PRODUCTION: this prompt is intentionally simple. Real deployments
    /// should add few-shot examples of *good* description rewrites
    /// calibrated for the operator's workflow.
    async fn execute_mutation(&self, plan: &MutationPlan) -> anyhow::Result<MutationResult> {
        let parent = &plan.input.parent_skill_text;
        let feedback = plan
            .input
            .failure_feedback
            .as_deref()
            .unwrap_or("(no prior feedback — first iteration)");
        let system = "You are a CyberClaw skill description optimizer. \
                      You rewrite SKILL.md bodies to be more accurate \
                      and trigger-friendly while preserving the contract.";
        let user = format!(
            "Parent SKILL.md:\n```\n{parent}\n```\n\n\
             Feedback from previous evaluation:\n{feedback}\n\n\
             Output a SINGLE rewritten SKILL.md body that addresses the \
             feedback. Keep frontmatter (---...---) intact. Improve the \
             description and Steps sections. Output ONLY the new SKILL.md \
             text, no preamble."
        );
        let req = self.build_request(system, &user);
        let resp = self
            .llm_client
            .chat_completion(req)
            .await
            .map_err(|e| anyhow::anyhow!("LLM mutation call failed: {e}"))?;
        let child_text = resp
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();
        if child_text.trim().is_empty() {
            anyhow::bail!("LLM mutation returned empty body");
        }
        Ok(MutationResult {
            child_skill_text: child_text,
            diff_artifact_id: None,
            reasoning: Some(format!(
                "LlmEvolutionDispatcher: {} round-trip produced child of {} bytes",
                self.model,
                resp.choices
                    .first()
                    .map(|c| c.message.content.len())
                    .unwrap_or(0)
            )),
        })
    }

    /// Score one case against a candidate variant.
    ///
    /// We feed the LLM the candidate `skill_text` as system prompt and
    /// the case's `task_input` as user message. Then we ask the LLM
    /// whether its planned response matches `expected_behavior`. The
    /// answer parses to hit/miss; the elapsed time is wall-clock.
    ///
    /// PRODUCTION: scoring is binary (hit / miss). Add partial credit
    /// (matched-but-wrong-tool) and confidence weighting for real
    /// deployments. Also split mutation and scoring across different
    /// models to avoid same-model bias.
    async fn execute_case(
        &self,
        variant: &SkillVariant,
        skill_text: &str,
        case: &EvaluationCase,
    ) -> anyhow::Result<CaseOutcome> {
        let started = Instant::now();
        let system = format!(
            "You are evaluating a skill methodology. Skill body follows.\n\n\
             ```\n{skill_text}\n```\n\n\
             For each task input, decide:\n\
             1. Would this skill methodology be applied?\n\
             2. Would the response match the expected behavior?\n\
             Reply with ONLY the JSON object: \
             {{\"hit\": true|false, \"why\": \"<one short sentence>\"}}"
        );
        let user_str =
            serde_json::to_string(&case.task_input).unwrap_or_else(|_| case.task_input.to_string());
        let user = format!(
            "Task input: {user_str}\n\nExpected behavior: {}\n\nVerdict?",
            case.expected_behavior
        );
        let req = self.build_request(&system, &user);
        let resp = self
            .llm_client
            .chat_completion(req)
            .await
            .map_err(|e| anyhow::anyhow!("LLM scoring call failed: {e}"))?;
        let raw = resp
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        // Lenient JSON extraction — LLMs often wrap with markdown / preamble.
        let hit = raw
            .find('{')
            .and_then(|start| {
                raw[start..]
                    .find('}')
                    .map(|end| &raw[start..start + end + 1])
            })
            .and_then(|json_slice| serde_json::from_str::<serde_json::Value>(json_slice).ok())
            .and_then(|v| v.get("hit").and_then(|h| h.as_bool()))
            .unwrap_or(false);

        debug!(
            variant = ?variant.variant_id,
            case_id = %case.case_id,
            hit,
            "execute_case: scored"
        );

        Ok(CaseOutcome {
            case_id: case.case_id.clone(),
            actual_output: serde_json::json!({
                "raw_llm_reply": raw,
                "hit": hit,
            }),
            execution_passed: hit,
            elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        })
    }

    /// Smoke regression — for now, returns a trivially passing result.
    ///
    /// PRODUCTION: re-run a curated baseline trigger set. The orchestrator
    /// fails the variant if ANY blocking command fails. Right now we
    /// emit one synthetic "smoke baseline" command that always passes,
    /// which means smoke is effectively a no-op gate. That's the right
    /// shape — not the right rigor.
    async fn run_smoke_regression(
        &self,
        variant: &SkillVariant,
        _skill_text: &str,
    ) -> anyhow::Result<RegressionResult> {
        warn!(
            variant = ?variant.variant_id,
            "run_smoke_regression: stub — returns all_passed=true. Production wiring TBD."
        );
        Ok(RegressionResult {
            results: vec![CommandResult {
                label: "smoke-stub".to_string(),
                passed: true,
                output_excerpt: Some(
                    "Smoke regression is currently a stub (no real baseline run). \
                     Replace this with a curated trigger set for production."
                        .to_string(),
                ),
            }],
            all_passed: true,
        })
    }
}

// Sprint 21 path #2 — unit tests for this dispatcher are intentionally
// deferred. The dispatcher's correctness contract is the
// `EvolutionDispatcher` trait, exercised end-to-end against a deployed
// LLM via the `/api/v1/learning/evolution/run` HTTP endpoint
// (introduced separately). Mocking the LLM response through the full
// orchestrator pipeline requires constructing every sub-type
// (`SkillVariant` + `CaseTrack` + `EvaluationCase` + `MutationPlan` +
// `MutationInput` + `CapabilityRef` + `RegressionSpec` etc.); each has
// invariants that change with the orchestrator's evolution. Mocking
// them here would be brittle. Integration testing happens at the
// HTTP layer where the dispatcher is the only moving piece.
#[cfg(test)]
#[allow(dead_code)]
mod tests {
    use super::*;
    use cyberclaw_llm::error::LlmResult;
    use cyberclaw_llm::types::{ChatChunk, ChatResponse, Choice};
    use futures::Stream;

    /// Mock LLM that returns a fixed reply per call. Lets us exercise
    /// the dispatcher's plumbing without burning real API tokens.
    struct MockLlm {
        reply: String,
    }

    #[async_trait]
    impl LlmClient for MockLlm {
        async fn chat_completion(&self, _req: ChatRequest) -> LlmResult<ChatResponse> {
            Ok(ChatResponse {
                id: "mock".to_string(),
                object: "chat.completion".to_string(),
                created: 0,
                model: "mock".to_string(),
                choices: vec![Choice {
                    index: 0,
                    message: Message {
                        role: Role::Assistant,
                        content: self.reply.clone(),
                        name: None,
                        tool_calls: None,
                        tool_call_id: None,
                        cache_control: None,
                    },
                    finish_reason: Some("stop".to_string()),
                }],
                usage: None,
            })
        }
        async fn chat_completion_stream(
            &self,
            _req: ChatRequest,
        ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>> {
            unimplemented!("mock does not support streaming")
        }
        fn provider(&self) -> &str {
            "mock"
        }
        async fn validate_connection(&self) -> LlmResult<()> {
            Ok(())
        }
    }

    /// Helper that exists only so the MockLlm type stays linked into
    /// the test build. Future test work that builds full SkillVariant
    /// + EvaluationCase fixtures can use this scaffold.
    #[tokio::test]
    async fn dispatcher_constructs_with_mock_llm() {
        let llm = Arc::new(MockLlm {
            reply: r#"{"hit": true, "why": "ok"}"#.to_string(),
        });
        let dispatcher = LlmEvolutionDispatcher::new(llm, "mock-model");
        assert_eq!(dispatcher.model, "mock-model");
    }
}
