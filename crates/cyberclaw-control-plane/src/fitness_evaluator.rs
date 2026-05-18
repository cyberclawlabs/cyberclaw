//! Fitness Evaluator — multi-dimensional scoring for Skill variants.
//!
//! Borrowed from Hermes Self-Evolution (Nous Research, MIT).
//! See `docs/implementation/reports/research-borrowing-analysis.md` §10.2 (HE-H1).
//!
//! # Architectural Constraint
//!
//! Like [`StagedVerificationGate`](crate::verification_gate::StagedVerificationGate),
//! `FitnessEvaluator` is **declarative**. It defines *what* to measure and *how*
//! to score raw outcomes — it does **not** execute variants itself. The
//! orchestrator is responsible for dispatching the evaluation cases through
//! `Connector → Capability` and feeding the `CaseOutcome`s back into
//! [`FitnessScorer::score_deterministic`] or [`FitnessScorer::score_heuristic`].
//!
//! # Composite formula
//!
//! ```text
//! composite = 0.5 * correctness + 0.3 * procedure_following + 0.2 * conciseness
//! final     = max(0.0, composite - length_penalty)
//! ```
//!
//! The length penalty grows linearly from `0.0` to [`FitnessScorer::max_penalty`]
//! once `skill_text.len() / max_skill_size` exceeds [`FitnessScorer::size_penalty_threshold`].
//!
//! # LLM-as-judge path
//!
//! When the evaluator is configured with [`ScoringMode::LlmJudge`], the
//! asynchronous [`FitnessEvaluator::score_via_llm_judge`] method dispatches a
//! judge [`CapabilityRef`] through an injected
//! [`EvolutionCapabilityDispatcher`](crate::production_evolution_dispatcher::EvolutionCapabilityDispatcher)
//! (see EVOLUTION_IDIOMS §1). The judge capability must return a JSON payload
//! matching [`LlmJudgeResponse`]. The length penalty is computed locally from
//! `skill_text` — the judge is never asked to reason about skill size,
//! keeping it comparable across scoring modes.

use cyberclaw_connectors::types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, ExecutionStatus,
};
use cyberclaw_core::capability::CapabilityRef;
use cyberclaw_core::identity::Identity;
use cyberclaw_core::ids::{ExecutionId, VariantId, WorkspaceId};
use cyberclaw_core::workspace::{WorkspaceMode, WorkspaceRef};
use serde::{Deserialize, Serialize};

use crate::production_evolution_dispatcher::EvolutionCapabilityDispatcher;

// ============================================================================
// Fitness Breakdown
// ============================================================================

/// Multi-dimensional fitness breakdown (borrowed from Hermes HE-H1).
///
/// Stored alongside `SkillVariant.score` for diagnosis: when the composite
/// score drops, the breakdown lets operators tell whether it was correctness
/// that regressed or conciseness — guiding the next mutation direction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FitnessBreakdown {
    /// Fraction of cases solved correctly (0.0..=1.0).
    pub correctness: f32,
    /// Adherence to expected procedure/steps (0.0..=1.0).
    pub procedure_following: f32,
    /// Output brevity vs. verbosity (0.0..=1.0, higher is more concise).
    pub conciseness: f32,
    /// Penalty applied when skill text exceeds the size threshold
    /// (0.0..=`max_penalty`, subtracted from the composite).
    pub length_penalty: f32,
}

impl FitnessBreakdown {
    /// Hermes composite: `0.5 * correctness + 0.3 * procedure + 0.2 * conciseness - length_penalty`.
    ///
    /// Clamped to a non-negative floor (a Skill variant can never have a negative fitness).
    pub fn composite(&self) -> f32 {
        let weighted =
            0.5 * self.correctness + 0.3 * self.procedure_following + 0.2 * self.conciseness;
        (weighted - self.length_penalty).max(0.0)
    }

    /// Weighted composite with custom weights (for domain-specific tuning).
    ///
    /// Length penalty is still subtracted after the weighted sum.
    pub fn weighted(&self, w_correct: f32, w_procedure: f32, w_conciseness: f32) -> f32 {
        let weighted = w_correct * self.correctness
            + w_procedure * self.procedure_following
            + w_conciseness * self.conciseness;
        (weighted - self.length_penalty).max(0.0)
    }

    /// An all-zero breakdown (used when there are no outcomes to score).
    pub fn zero() -> Self {
        Self {
            correctness: 0.0,
            procedure_following: 0.0,
            conciseness: 0.0,
            length_penalty: 0.0,
        }
    }
}

// ============================================================================
// LLM Judge Response
// ============================================================================

/// Structured response returned by an LLM-as-judge capability.
///
/// The judge capability is expected to return a JSON payload of this shape.
/// `score_via_llm_judge` deserializes it and folds the length penalty in
/// locally — the judge itself never reasons about skill size.
///
/// The `feedback` field is free-form structured reasoning the orchestrator
/// can forward to the next mutation as GEPA-style reflective hints.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LlmJudgeResponse {
    /// Did the outputs satisfy the expected behavior? (0.0..=1.0)
    pub correctness: f32,
    /// Did the variant follow the expected process? (0.0..=1.0)
    pub procedure_following: f32,
    /// Was the skill text appropriately concise? (0.0..=1.0)
    pub conciseness: f32,
    /// Structured reasoning / feedback from the judge. Forwarded into the
    /// next mutation's prior-failure prompt.
    pub feedback: String,
}

// ============================================================================
// Evaluation Case
// ============================================================================

/// A single evaluation case: task input + expected behavior reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationCase {
    /// Stable case identifier (used to correlate outcomes back to cases).
    pub case_id: String,
    /// Task input that will be fed to the Skill variant.
    pub task_input: serde_json::Value,
    /// Textual description of what a correct variant should do.
    pub expected_behavior: String,
    /// Deterministic expected output, when one exists.
    pub expected_output: Option<serde_json::Value>,
}

// ============================================================================
// Scoring Mode
// ============================================================================

/// How outcomes should be scored for this evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScoringMode {
    /// Deterministic: pass/fail based on output equality against `expected_output`.
    Deterministic,
    /// Heuristic: structural match + text similarity, no LLM required.
    Heuristic,
    /// LLM-as-judge: requires dispatching a judge Capability.
    ///
    /// Placeholder variant — actual implementation is future work.
    LlmJudge {
        /// Capability that should be invoked to judge each outcome.
        judge_capability: CapabilityRef,
    },
}

// ============================================================================
// Evaluation Plan
// ============================================================================

/// A plan describing how to evaluate a variant.
///
/// This is a **declarative** record. The orchestrator consumes the plan,
/// runs each `case` via `Connector → Capability`, and then hands the
/// resulting `CaseOutcome`s plus the plan back to `FitnessScorer`.
#[derive(Debug, Clone)]
pub struct EvaluationPlan {
    /// The variant being evaluated.
    pub variant_id: VariantId,
    /// Cases that the orchestrator must execute.
    pub cases: Vec<EvaluationCase>,
    /// How outcomes should be scored once they come back.
    pub scoring_mode: ScoringMode,
}

// ============================================================================
// Case Outcome
// ============================================================================

/// A raw execution result from running one case against one variant.
///
/// Produced by the orchestrator after capability dispatch, then fed back
/// into the scorer. `FitnessEvaluator` never produces this directly.
#[derive(Debug, Clone)]
pub struct CaseOutcome {
    /// Correlates the outcome back to its `EvaluationCase`.
    pub case_id: String,
    /// What the variant actually produced.
    pub actual_output: serde_json::Value,
    /// Whether execution completed without runtime errors.
    pub execution_passed: bool,
    /// Wall-clock time the case took, in milliseconds.
    pub elapsed_ms: u64,
}

// ============================================================================
// Fitness Scorer
// ============================================================================

/// Scores a batch of outcomes into a [`FitnessBreakdown`]. Pure function —
/// no side effects, no I/O, no capability dispatch.
#[derive(Debug, Clone)]
pub struct FitnessScorer {
    /// Maximum acceptable size for a Skill's text body (bytes).
    pub max_skill_size: usize,
    /// Fraction of `max_skill_size` at which the length penalty starts to
    /// grow (e.g. `0.9` means penalty kicks in at 90% of the max).
    pub size_penalty_threshold: f32,
    /// Upper bound on the length penalty (e.g. `0.3`).
    pub max_penalty: f32,
}

impl Default for FitnessScorer {
    fn default() -> Self {
        Self::new()
    }
}

impl FitnessScorer {
    /// Sensible Hermes defaults: 15000 bytes, 0.9 threshold, 0.3 max penalty.
    pub fn new() -> Self {
        Self {
            max_skill_size: 15_000,
            size_penalty_threshold: 0.9,
            max_penalty: 0.3,
        }
    }

    /// Compute the length penalty for a piece of skill text.
    ///
    /// Returns `0.0` while `ratio <= threshold`, then grows linearly so that
    /// at `ratio == 1.0` the penalty equals `max_penalty`. The penalty keeps
    /// growing past 1.0 but is clamped to `max_penalty`.
    pub fn length_penalty(&self, skill_text: &str) -> f32 {
        if self.max_skill_size == 0 {
            return 0.0;
        }
        let ratio = skill_text.len() as f32 / self.max_skill_size as f32;
        if ratio <= self.size_penalty_threshold {
            return 0.0;
        }
        let span = 1.0 - self.size_penalty_threshold;
        if span <= 0.0 {
            return self.max_penalty;
        }
        let over = (ratio - self.size_penalty_threshold) / span;
        (over * self.max_penalty).min(self.max_penalty)
    }

    /// Score outcomes under [`ScoringMode::Deterministic`].
    ///
    /// * `correctness` — fraction of outcomes whose `actual_output`
    ///   matches the case's `expected_output`.
    /// * `procedure_following` — fraction of outcomes that ran without
    ///   runtime errors (`execution_passed`).
    /// * `conciseness` — inverse of mean output size, normalized
    ///   against `max_skill_size`.
    /// * `length_penalty` — from `skill_text` length.
    pub fn score_deterministic(
        &self,
        outcomes: &[CaseOutcome],
        cases: &[EvaluationCase],
        skill_text: &str,
    ) -> FitnessBreakdown {
        if outcomes.is_empty() {
            return FitnessBreakdown::zero();
        }

        let total = outcomes.len() as f32;

        let correct = outcomes
            .iter()
            .filter(|o| match cases.iter().find(|c| c.case_id == o.case_id) {
                Some(c) => match &c.expected_output {
                    Some(expected) => &o.actual_output == expected,
                    // No expected_output → deterministic cannot verify, treat as mismatch.
                    None => false,
                },
                None => false,
            })
            .count() as f32;

        let procedure = outcomes.iter().filter(|o| o.execution_passed).count() as f32;

        let conciseness = mean_conciseness(outcomes, self.max_skill_size);

        FitnessBreakdown {
            correctness: correct / total,
            procedure_following: procedure / total,
            conciseness,
            length_penalty: self.length_penalty(skill_text),
        }
    }

    /// Score outcomes under [`ScoringMode::Heuristic`].
    ///
    /// `correctness` uses a lenient structural match: an outcome is considered
    /// correct if it executed *and* its serialized output overlaps textually
    /// with the case's `expected_behavior` description (Jaccard-on-tokens ≥ 0.2),
    /// or if `expected_output` is present and equal.
    pub fn score_heuristic(
        &self,
        outcomes: &[CaseOutcome],
        cases: &[EvaluationCase],
        skill_text: &str,
    ) -> FitnessBreakdown {
        if outcomes.is_empty() {
            return FitnessBreakdown::zero();
        }

        let total = outcomes.len() as f32;

        let correct = outcomes
            .iter()
            .filter(|o| {
                let Some(case) = cases.iter().find(|c| c.case_id == o.case_id) else {
                    return false;
                };
                if !o.execution_passed {
                    return false;
                }
                if let Some(expected) = &case.expected_output {
                    if &o.actual_output == expected {
                        return true;
                    }
                }
                let actual_text = serde_json::to_string(&o.actual_output).unwrap_or_default();
                token_overlap(&actual_text, &case.expected_behavior) >= 0.2
            })
            .count() as f32;

        let procedure = outcomes.iter().filter(|o| o.execution_passed).count() as f32;

        let conciseness = mean_conciseness(outcomes, self.max_skill_size);

        FitnessBreakdown {
            correctness: correct / total,
            procedure_following: procedure / total,
            conciseness,
            length_penalty: self.length_penalty(skill_text),
        }
    }
}

// ============================================================================
// Fitness Evaluator (planner)
// ============================================================================

/// Plans evaluations and scores outcomes — the declarative counterpart of
/// `MutationEngine::plan`.
///
/// `FitnessEvaluator` never executes variants. It only:
/// 1. Produces an [`EvaluationPlan`] the orchestrator can act on.
/// 2. Scores the orchestrator's [`CaseOutcome`]s into a [`FitnessBreakdown`].
#[derive(Debug, Clone)]
pub struct FitnessEvaluator {
    /// Scoring mode applied when building a plan (callers may override).
    pub default_scoring: ScoringMode,
    /// Underlying pure scorer.
    pub scorer: FitnessScorer,
}

impl Default for FitnessEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

impl FitnessEvaluator {
    /// Build an evaluator with `ScoringMode::Deterministic` defaults.
    pub fn new() -> Self {
        Self {
            default_scoring: ScoringMode::Deterministic,
            scorer: FitnessScorer::new(),
        }
    }

    /// Build a plan for evaluating `variant` against the given `cases`.
    ///
    /// The plan is purely declarative — it describes *what* the orchestrator
    /// must run.
    pub fn plan(
        &self,
        variant: &crate::skill_archive::SkillVariant,
        cases: Vec<EvaluationCase>,
    ) -> EvaluationPlan {
        EvaluationPlan {
            variant_id: variant.variant_id.clone(),
            cases,
            scoring_mode: self.default_scoring.clone(),
        }
    }

    /// Score outcomes using the evaluator's configured scorer.
    ///
    /// Uses deterministic scoring when `default_scoring == Deterministic`,
    /// otherwise falls through to heuristic. `LlmJudge` is the async path
    /// ([`Self::score_via_llm_judge`]) — callers that hit this sync method
    /// while configured for `LlmJudge` get the heuristic fallback so the
    /// synchronous contract is preserved.
    pub fn score(
        &self,
        outcomes: &[CaseOutcome],
        cases: &[EvaluationCase],
        skill_text: &str,
    ) -> FitnessBreakdown {
        match self.default_scoring {
            ScoringMode::Deterministic => {
                self.scorer.score_deterministic(outcomes, cases, skill_text)
            }
            ScoringMode::Heuristic | ScoringMode::LlmJudge { .. } => {
                self.scorer.score_heuristic(outcomes, cases, skill_text)
            }
        }
    }

    /// Score outcomes by dispatching an LLM judge capability through the
    /// injected [`EvolutionCapabilityDispatcher`].
    ///
    /// Contract:
    ///
    /// 1. Builds a deterministic judge prompt from `cases`, `outcomes`, and
    ///    `skill_text` (see [`build_llm_judge_prompt`]).
    /// 2. Dispatches `judge_capability` with `{"prompt": <prompt>}` as input.
    /// 3. Expects the capability output to deserialize into an
    ///    [`LlmJudgeResponse`]. Capability outputs are permitted to nest the
    ///    response under an `"output"` / `"response"` field — the parser
    ///    tries the root shape first, then those keys.
    /// 4. Builds a [`FitnessBreakdown`] by combining the judge's scalar
    ///    scores with a locally-computed length penalty from `skill_text`.
    ///
    /// # Architectural constraint (EVOLUTION_IDIOMS §1)
    ///
    /// This method never touches HTTP, an LLM SDK, or a concrete connector.
    /// All I/O flows through the trait object.
    ///
    /// # Errors
    ///
    /// - Dispatcher returned an error → propagated.
    /// - Dispatcher returned a non-success status → returned as error.
    /// - Capability output does not deserialize into [`LlmJudgeResponse`] →
    ///   returned as error.
    pub async fn score_via_llm_judge(
        &self,
        dispatcher: &dyn EvolutionCapabilityDispatcher,
        judge_capability: &CapabilityRef,
        outcomes: &[CaseOutcome],
        cases: &[EvaluationCase],
        skill_text: &str,
    ) -> anyhow::Result<FitnessBreakdown> {
        let prompt = build_llm_judge_prompt(cases, outcomes, skill_text);

        // Build the capability dispatch request. An ephemeral evolution
        // workspace and Identity::System match how
        // ProductionEvolutionDispatcher constructs its requests.
        let actor = Identity::System
            .to_actor_ref(None)
            .ok_or_else(|| anyhow::anyhow!("Identity::System -> ActorRef failed"))?;
        let workspace = WorkspaceRef {
            id: WorkspaceId::from_string("evolution-llm-judge".to_string())
                .expect("'evolution-llm-judge' is a valid workspace id"),
            mode: WorkspaceMode::Ephemeral,
            materialization_mode: None,
            home_node_id: None,
            backing_store: None,
            root: "/tmp/cyberclaw-evolution-judge".to_string(),
            writable_roots: vec![],
        };

        let request = CapabilityExecutionRequest {
            execution_id: ExecutionId::new(),
            trace_id: format!("evo-judge-{}", uuid::Uuid::new_v4()),
            actor,
            workspace,
            connector_id: judge_capability.connector_id.clone(),
            capability_id: judge_capability.id.clone(),
            input: serde_json::json!({ "prompt": prompt }),
        };

        let result: CapabilityExecutionResult = dispatcher.dispatch(request).await?;

        if result.status != ExecutionStatus::Success {
            anyhow::bail!(
                "llm judge dispatch returned non-success status: {:?}, error: {:?}",
                result.status,
                result.error,
            );
        }

        let response = parse_llm_judge_response(&result.output)?;

        // Clamp scores defensively — the judge may emit values slightly
        // outside [0,1] even when well-behaved. Penalty is computed locally.
        let clamp = |v: f32| v.clamp(0.0, 1.0);
        Ok(FitnessBreakdown {
            correctness: clamp(response.correctness),
            procedure_following: clamp(response.procedure_following),
            conciseness: clamp(response.conciseness),
            length_penalty: self.scorer.length_penalty(skill_text),
        })
    }
}

// ============================================================================
// LLM Judge helpers
// ============================================================================

/// Build the deterministic prompt fed to the LLM judge capability.
///
/// The prompt is intentionally compact and template-driven — the judge
/// capability is free to add its own system preamble around it.
pub fn build_llm_judge_prompt(
    cases: &[EvaluationCase],
    outcomes: &[CaseOutcome],
    skill_text: &str,
) -> String {
    let cases_json = serde_json::to_string(cases).unwrap_or_else(|_| "[]".to_string());
    let outcomes_json = serde_json::to_string(
        &outcomes
            .iter()
            .map(|o| {
                serde_json::json!({
                    "case_id": o.case_id,
                    "actual_output": o.actual_output,
                    "execution_passed": o.execution_passed,
                    "elapsed_ms": o.elapsed_ms,
                })
            })
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".to_string());

    format!(
        "You are evaluating an Agent Skill candidate. Given:\n\
         - Skill text: {skill_text}\n\
         - Tasks and expected behaviors: {cases_json}\n\
         - Actual outputs: {outcomes_json}\n\n\
         Score on:\n\
         - correctness (0-1): did outputs satisfy expected behaviors?\n\
         - procedure_following (0-1): did the agent follow the expected process?\n\
         - conciseness (0-1): was the skill text appropriately brief?\n\
         Provide JSON: {{\"correctness\": ..., \"procedure_following\": ..., \"conciseness\": ..., \"feedback\": \"...\"}}",
    )
}

/// Parse an LLM judge capability's raw output into an [`LlmJudgeResponse`].
///
/// Accepts three shapes:
/// 1. The `LlmJudgeResponse` JSON object directly.
/// 2. An object with an `"output"` field containing the response.
/// 3. An object with a `"response"` field containing the response.
fn parse_llm_judge_response(raw: &serde_json::Value) -> anyhow::Result<LlmJudgeResponse> {
    // Shape 1: direct.
    if let Ok(r) = serde_json::from_value::<LlmJudgeResponse>(raw.clone()) {
        return Ok(r);
    }
    // Shape 2/3: nested.
    for key in ["output", "response"] {
        if let Some(nested) = raw.get(key) {
            if let Ok(r) = serde_json::from_value::<LlmJudgeResponse>(nested.clone()) {
                return Ok(r);
            }
        }
    }
    Err(anyhow::anyhow!(
        "failed to parse LlmJudgeResponse from judge capability output: {raw}"
    ))
}

// ============================================================================
// Internal helpers
// ============================================================================

/// Mean conciseness across outcomes: `1 - mean(output_len) / max_size`,
/// clamped to `[0, 1]`.
fn mean_conciseness(outcomes: &[CaseOutcome], max_size: usize) -> f32 {
    if outcomes.is_empty() || max_size == 0 {
        return 0.0;
    }
    let total_len: usize = outcomes
        .iter()
        .map(|o| {
            serde_json::to_string(&o.actual_output)
                .unwrap_or_default()
                .len()
        })
        .sum();
    let mean = total_len as f32 / outcomes.len() as f32;
    let ratio = (mean / max_size as f32).clamp(0.0, 1.0);
    1.0 - ratio
}

/// Jaccard overlap between the lowercased token sets of `a` and `b`.
fn token_overlap(a: &str, b: &str) -> f32 {
    use std::collections::HashSet;
    let tok = |s: &str| -> HashSet<String> {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
            .collect()
    };
    let sa = tok(a);
    let sb = tok(b);
    if sa.is_empty() || sb.is_empty() {
        return 0.0;
    }
    let inter = sa.intersection(&sb).count() as f32;
    let union = sa.union(&sb).count() as f32;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_archive::SkillVariant;
    use crate::skill_evolution::CaseTrack;
    use cyberclaw_core::capability::{CapabilityEffect, CapabilityRef, RiskLevel};
    use cyberclaw_core::ids::{CapabilityId, ConnectorId, SkillId, VariantId};
    use serde_json::json;

    fn make_case(id: &str, expected: Option<serde_json::Value>) -> EvaluationCase {
        EvaluationCase {
            case_id: id.to_string(),
            task_input: json!({"in": id}),
            expected_behavior: "return a greeting containing hello world".to_string(),
            expected_output: expected,
        }
    }

    fn make_outcome(id: &str, output: serde_json::Value, ok: bool) -> CaseOutcome {
        CaseOutcome {
            case_id: id.to_string(),
            actual_output: output,
            execution_passed: ok,
            elapsed_ms: 10,
        }
    }

    fn make_variant() -> SkillVariant {
        SkillVariant {
            variant_id: VariantId::new(),
            parent_variant_id: None,
            skill_id: SkillId::new(),
            score: 0.0,
            child_count: 0,
            track: CaseTrack::Success,
            patch_artifact_id: None,
        }
    }

    // -- FitnessBreakdown::composite --

    #[test]
    fn test_composite_all_ones() {
        let b = FitnessBreakdown {
            correctness: 1.0,
            procedure_following: 1.0,
            conciseness: 1.0,
            length_penalty: 0.0,
        };
        assert!((b.composite() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_composite_all_zeros() {
        let b = FitnessBreakdown::zero();
        assert_eq!(b.composite(), 0.0);
    }

    #[test]
    fn test_composite_never_negative() {
        // Even an overwhelming penalty must not drop below zero.
        let b = FitnessBreakdown {
            correctness: 0.1,
            procedure_following: 0.1,
            conciseness: 0.1,
            length_penalty: 1.0,
        };
        assert_eq!(b.composite(), 0.0);
    }

    #[test]
    fn test_composite_mix() {
        // 0.5*1 + 0.3*0 + 0.2*0 - 0 = 0.5
        let b = FitnessBreakdown {
            correctness: 1.0,
            procedure_following: 0.0,
            conciseness: 0.0,
            length_penalty: 0.0,
        };
        assert!((b.composite() - 0.5).abs() < 1e-6);
    }

    // -- FitnessBreakdown::weighted --

    #[test]
    fn test_weighted_custom_weights() {
        let b = FitnessBreakdown {
            correctness: 1.0,
            procedure_following: 1.0,
            conciseness: 1.0,
            length_penalty: 0.0,
        };
        // 0.2 + 0.7 + 0.1 = 1.0
        assert!((b.weighted(0.2, 0.7, 0.1) - 1.0).abs() < 1e-6);
        // 0.9 + 0 + 0 = 0.9
        assert!((b.weighted(0.9, 0.0, 0.0) - 0.9).abs() < 1e-6);
    }

    // -- length_penalty --

    #[test]
    fn test_length_penalty_below_threshold() {
        let scorer = FitnessScorer::new(); // 15000, 0.9, 0.3
        let text = "a".repeat(1000); // ratio ~0.066
        assert_eq!(scorer.length_penalty(&text), 0.0);
    }

    #[test]
    fn test_length_penalty_at_90_percent_starts() {
        let scorer = FitnessScorer::new();
        // Just at threshold → penalty still zero
        let at = "a".repeat((15_000.0 * 0.9) as usize);
        assert!(scorer.length_penalty(&at).abs() < 1e-6);
        // Slightly above 90% → small positive penalty
        let above = "a".repeat((15_000.0 * 0.92) as usize);
        let p = scorer.length_penalty(&above);
        assert!(p > 0.0 && p < 0.3, "expected 0 < p < 0.3, got {}", p);
    }

    #[test]
    fn test_length_penalty_caps_at_max_penalty() {
        let scorer = FitnessScorer::new();
        let huge = "a".repeat(100_000); // ratio ~6.67
        assert!((scorer.length_penalty(&huge) - 0.3).abs() < 1e-6);
    }

    // -- score_deterministic --

    #[test]
    fn test_deterministic_all_match() {
        let scorer = FitnessScorer::new();
        let cases = vec![
            make_case("a", Some(json!({"ok": true}))),
            make_case("b", Some(json!({"ok": true}))),
        ];
        let outcomes = vec![
            make_outcome("a", json!({"ok": true}), true),
            make_outcome("b", json!({"ok": true}), true),
        ];
        let b = scorer.score_deterministic(&outcomes, &cases, "tiny skill");
        assert!((b.correctness - 1.0).abs() < 1e-6);
        assert!((b.procedure_following - 1.0).abs() < 1e-6);
        assert_eq!(b.length_penalty, 0.0);
    }

    #[test]
    fn test_deterministic_half_match() {
        let scorer = FitnessScorer::new();
        let cases = vec![
            make_case("a", Some(json!({"ok": true}))),
            make_case("b", Some(json!({"ok": true}))),
        ];
        let outcomes = vec![
            make_outcome("a", json!({"ok": true}), true),
            make_outcome("b", json!({"ok": false}), true),
        ];
        let b = scorer.score_deterministic(&outcomes, &cases, "tiny skill");
        assert!((b.correctness - 0.5).abs() < 1e-6);
        assert!((b.procedure_following - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_procedure_and_conciseness_independent() {
        let scorer = FitnessScorer::new();
        // One execution error → procedure_following drops, but correctness
        // (for the two matching outcomes) is still 1.0 over 2 matched cases.
        let cases = vec![
            make_case("a", Some(json!({"ok": true}))),
            make_case("b", Some(json!({"ok": true}))),
        ];
        let outcomes = vec![
            make_outcome("a", json!({"ok": true}), true),
            make_outcome("b", json!({"ok": true}), false), // runtime error
        ];
        let b = scorer.score_deterministic(&outcomes, &cases, "tiny skill");
        // Both outputs matched → correctness = 1.0.
        assert!((b.correctness - 1.0).abs() < 1e-6);
        // One of two ran cleanly → procedure_following = 0.5.
        assert!((b.procedure_following - 0.5).abs() < 1e-6);
        // Conciseness is a separate axis, not tied to correctness/procedure.
        assert!(b.conciseness > 0.0 && b.conciseness <= 1.0);
    }

    // -- score_heuristic --

    #[test]
    fn test_heuristic_returns_valid_breakdown() {
        let scorer = FitnessScorer::new();
        let cases = vec![make_case("a", None)];
        let outcomes = vec![make_outcome(
            "a",
            json!("hello world, greeting contains both"),
            true,
        )];
        let b = scorer.score_heuristic(&outcomes, &cases, "tiny skill");
        assert!(b.correctness >= 0.0 && b.correctness <= 1.0);
        assert!((b.procedure_following - 1.0).abs() < 1e-6);
        assert!(b.conciseness >= 0.0 && b.conciseness <= 1.0);
    }

    // -- EvaluationPlan --

    #[test]
    fn test_evaluation_plan_creation() {
        let ev = FitnessEvaluator::new();
        let variant = make_variant();
        let cases = vec![make_case("a", None), make_case("b", None)];
        let plan = ev.plan(&variant, cases);
        assert_eq!(plan.variant_id, variant.variant_id);
        assert_eq!(plan.cases.len(), 2);
        assert!(matches!(plan.scoring_mode, ScoringMode::Deterministic));
    }

    // -- ScoringMode serde roundtrip --

    #[test]
    fn test_scoring_mode_serde_roundtrip_all_variants() {
        // Deterministic
        let d = ScoringMode::Deterministic;
        let j = serde_json::to_string(&d).unwrap();
        let back: ScoringMode = serde_json::from_str(&j).unwrap();
        assert!(matches!(back, ScoringMode::Deterministic));

        // Heuristic
        let h = ScoringMode::Heuristic;
        let j = serde_json::to_string(&h).unwrap();
        let back: ScoringMode = serde_json::from_str(&j).unwrap();
        assert!(matches!(back, ScoringMode::Heuristic));

        // LlmJudge
        let judge = CapabilityRef {
            id: CapabilityId::from_string("llm.judge".to_string()).unwrap(),
            connector_id: ConnectorId::from_string("llm".to_string()).unwrap(),
            risk: RiskLevel::Low,
            effects: vec![CapabilityEffect::Read],
            placement: None,
        };
        let l = ScoringMode::LlmJudge {
            judge_capability: judge,
        };
        let j = serde_json::to_string(&l).unwrap();
        let back: ScoringMode = serde_json::from_str(&j).unwrap();
        assert!(matches!(back, ScoringMode::LlmJudge { .. }));
    }

    // -- empty outcomes --

    #[test]
    fn test_empty_outcomes_zero_breakdown_no_panic() {
        let scorer = FitnessScorer::new();
        let cases: Vec<EvaluationCase> = vec![];
        let outcomes: Vec<CaseOutcome> = vec![];
        let b_det = scorer.score_deterministic(&outcomes, &cases, "skill");
        assert_eq!(b_det, FitnessBreakdown::zero());

        let b_heur = scorer.score_heuristic(&outcomes, &cases, "skill");
        assert_eq!(b_heur, FitnessBreakdown::zero());
    }

    // -- FitnessBreakdown serde roundtrip --

    #[test]
    fn test_breakdown_serde_roundtrip() {
        let b = FitnessBreakdown {
            correctness: 0.7,
            procedure_following: 0.8,
            conciseness: 0.9,
            length_penalty: 0.05,
        };
        let j = serde_json::to_string(&b).unwrap();
        let back: FitnessBreakdown = serde_json::from_str(&j).unwrap();
        assert_eq!(back, b);
        assert!((back.composite() - b.composite()).abs() < 1e-6);
    }

    // ========================================================================
    // LLM-as-judge tests (Sprint 5)
    // ========================================================================

    use async_trait::async_trait;
    use cyberclaw_connectors::types::{
        CapabilityExecutionRequest, CapabilityExecutionResult, ExecutionStatus,
    };
    use cyberclaw_core::ids::{CapabilityId as _CapId, ConnectorId as _ConnId, ExecutionId};
    use std::sync::{Arc, Mutex};

    /// Minimal mock dispatcher for the LLM-as-judge unit tests.
    struct MockLlmDispatcher {
        response: Result<CapabilityExecutionResult, String>,
        last_request: Mutex<Option<CapabilityExecutionRequest>>,
        call_count: Mutex<u32>,
    }

    impl MockLlmDispatcher {
        fn new(response: Result<CapabilityExecutionResult, String>) -> Arc<Self> {
            Arc::new(Self {
                response,
                last_request: Mutex::new(None),
                call_count: Mutex::new(0),
            })
        }
    }

    #[async_trait]
    impl EvolutionCapabilityDispatcher for MockLlmDispatcher {
        async fn dispatch(
            &self,
            request: CapabilityExecutionRequest,
        ) -> anyhow::Result<CapabilityExecutionResult> {
            *self.call_count.lock().unwrap() += 1;
            *self.last_request.lock().unwrap() = Some(request);
            self.response.clone().map_err(|e| anyhow::anyhow!(e))
        }
    }

    fn mk_judge_cap() -> CapabilityRef {
        CapabilityRef {
            id: _CapId::from_string("llm.judge".to_string()).unwrap(),
            connector_id: _ConnId::from_string("llm".to_string()).unwrap(),
            risk: RiskLevel::Low,
            effects: vec![CapabilityEffect::Read],
            placement: None,
        }
    }

    fn mk_success_result(output: serde_json::Value) -> CapabilityExecutionResult {
        CapabilityExecutionResult {
            execution_id: ExecutionId::new(),
            trace_id: "t".to_string(),
            connector_id: _ConnId::from_string("llm".to_string()).unwrap(),
            capability_id: _CapId::from_string("llm.judge".to_string()).unwrap(),
            output,
            status: ExecutionStatus::Success,
            error: None,
            actual_runtime: None,
        }
    }

    #[test]
    fn llm_judge_response_serde_roundtrip() {
        let r = LlmJudgeResponse {
            correctness: 0.85,
            procedure_following: 0.7,
            conciseness: 0.95,
            feedback: "Good structure, slightly verbose in section 2.".to_string(),
        };
        let j = serde_json::to_string(&r).unwrap();
        let back: LlmJudgeResponse = serde_json::from_str(&j).unwrap();
        assert_eq!(back, r);
    }

    #[tokio::test]
    async fn score_via_llm_judge_happy_path() {
        // Judge returns valid JSON → FitnessBreakdown computed from judge scores
        // + locally-computed length penalty (zero for tiny skill text).
        let judge_output = json!({
            "correctness": 0.8,
            "procedure_following": 0.9,
            "conciseness": 0.6,
            "feedback": "Looks good overall."
        });
        let mock = MockLlmDispatcher::new(Ok(mk_success_result(judge_output)));
        let evaluator = FitnessEvaluator::new();
        let cases = vec![make_case("a", None)];
        let outcomes = vec![make_outcome("a", json!({"ok": true}), true)];

        let breakdown = evaluator
            .score_via_llm_judge(
                mock.as_ref(),
                &mk_judge_cap(),
                &outcomes,
                &cases,
                "tiny skill text",
            )
            .await
            .expect("judge ok");

        assert!((breakdown.correctness - 0.8).abs() < 1e-6);
        assert!((breakdown.procedure_following - 0.9).abs() < 1e-6);
        assert!((breakdown.conciseness - 0.6).abs() < 1e-6);
        assert_eq!(breakdown.length_penalty, 0.0);
        assert_eq!(*mock.call_count.lock().unwrap(), 1);

        // Verify the dispatched request routed to the judge capability.
        let req = mock.last_request.lock().unwrap().clone().expect("req");
        assert_eq!(req.capability_id.as_str(), "llm.judge");
        assert_eq!(req.connector_id.as_str(), "llm");
        assert!(req.input.get("prompt").is_some(), "prompt must be present");
    }

    #[tokio::test]
    async fn score_via_llm_judge_dispatcher_error_propagates() {
        let mock = MockLlmDispatcher::new(Err("upstream LLM 503".to_string()));
        let evaluator = FitnessEvaluator::new();
        let cases = vec![make_case("a", None)];
        let outcomes = vec![make_outcome("a", json!({}), true)];

        let err = evaluator
            .score_via_llm_judge(mock.as_ref(), &mk_judge_cap(), &outcomes, &cases, "skill")
            .await
            .expect_err("should propagate dispatcher error");
        assert!(
            err.to_string().contains("upstream LLM 503"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn score_via_llm_judge_malformed_response_errors() {
        // Output is missing `correctness` → parse fails.
        let malformed = json!({ "unexpected": "shape" });
        let mock = MockLlmDispatcher::new(Ok(mk_success_result(malformed)));
        let evaluator = FitnessEvaluator::new();
        let cases = vec![make_case("a", None)];
        let outcomes = vec![make_outcome("a", json!({}), true)];

        let err = evaluator
            .score_via_llm_judge(mock.as_ref(), &mk_judge_cap(), &outcomes, &cases, "skill")
            .await
            .expect_err("malformed response should error");
        assert!(
            err.to_string().contains("failed to parse LlmJudgeResponse"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn score_via_llm_judge_applies_length_penalty_independently() {
        // Judge reports perfect scores but skill text is huge → penalty must
        // still be applied locally (the judge never sees skill size).
        let judge_output = json!({
            "correctness": 1.0,
            "procedure_following": 1.0,
            "conciseness": 1.0,
            "feedback": "Perfect."
        });
        let mock = MockLlmDispatcher::new(Ok(mk_success_result(judge_output)));
        let evaluator = FitnessEvaluator::new();
        let cases = vec![make_case("a", None)];
        let outcomes = vec![make_outcome("a", json!({}), true)];

        // Build a skill text well over the 15k threshold → length penalty
        // should saturate to max_penalty (0.3) independently of the judge.
        let huge_skill = "a".repeat(100_000);
        let breakdown = evaluator
            .score_via_llm_judge(
                mock.as_ref(),
                &mk_judge_cap(),
                &outcomes,
                &cases,
                &huge_skill,
            )
            .await
            .expect("judge ok");
        assert!((breakdown.correctness - 1.0).abs() < 1e-6);
        assert!((breakdown.length_penalty - 0.3).abs() < 1e-6);
        // Composite must reflect the penalty: 1.0 - 0.3 = 0.7.
        assert!(
            (breakdown.composite() - 0.7).abs() < 1e-6,
            "composite should fold in length penalty, got {}",
            breakdown.composite(),
        );
    }

    #[tokio::test]
    async fn score_via_llm_judge_parses_nested_output_field() {
        // Some LLM wrappers emit {"output": <LlmJudgeResponse>}; parser must
        // unwrap that shape.
        let judge_output = json!({
            "output": {
                "correctness": 0.5,
                "procedure_following": 0.4,
                "conciseness": 0.3,
                "feedback": "nested shape"
            }
        });
        let mock = MockLlmDispatcher::new(Ok(mk_success_result(judge_output)));
        let evaluator = FitnessEvaluator::new();
        let cases = vec![make_case("a", None)];
        let outcomes = vec![make_outcome("a", json!({}), true)];

        let breakdown = evaluator
            .score_via_llm_judge(mock.as_ref(), &mk_judge_cap(), &outcomes, &cases, "skill")
            .await
            .expect("judge ok");
        assert!((breakdown.correctness - 0.5).abs() < 1e-6);
        assert!((breakdown.procedure_following - 0.4).abs() < 1e-6);
        assert!((breakdown.conciseness - 0.3).abs() < 1e-6);
    }

    #[tokio::test]
    async fn score_via_llm_judge_rejects_non_success_status() {
        let mut bad = mk_success_result(json!({
            "correctness": 0.9,
            "procedure_following": 0.9,
            "conciseness": 0.9,
            "feedback": "ignored"
        }));
        bad.status = ExecutionStatus::Failed;
        bad.error = Some("judge downstream error".to_string());
        let mock = MockLlmDispatcher::new(Ok(bad));
        let evaluator = FitnessEvaluator::new();
        let cases = vec![make_case("a", None)];
        let outcomes = vec![make_outcome("a", json!({}), true)];

        let err = evaluator
            .score_via_llm_judge(mock.as_ref(), &mk_judge_cap(), &outcomes, &cases, "skill")
            .await
            .expect_err("non-success status should error");
        assert!(
            err.to_string().contains("non-success"),
            "unexpected error: {err}"
        );
    }
}
