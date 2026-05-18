//! EvolutionOrchestrator — drives the self-evolution closed loop.
//!
//! One `step()` call advances one generation: select parent → mutate →
//! evaluate → score → verify → archive. The orchestrator is a pure state
//! machine; actual side effects (LLM calls, variant execution, shell
//! regressions) are delegated to an injected [`EvolutionDispatcher`]. This
//! keeps the main loop testable with a mock dispatcher and preserves the
//! Connector → Capability execution invariant (see `CLAUDE.md` §3).
//!
//! The orchestrator itself never touches a connector, spawns a subprocess,
//! or talks to an LLM. Every piece of I/O flows through the trait.
//!
//! # Observability
//!
//! Both [`EvolutionOrchestrator::step`] and [`EvolutionOrchestrator::run`]
//! are instrumented with `tracing` spans. For structured downstream
//! consumption an optional [`EvolutionEventSink`] can be attached via
//! [`EvolutionOrchestrator::with_event_sink`]; the orchestrator emits an
//! [`EvolutionEvent`] at every state-transition point (parent selected,
//! mutation complete, evaluation complete, fitness scored, archived /
//! rejected / failed). The default is [`NoopEventSink`] — no emission.
//!
//! # Budget
//!
//! [`EvolutionConfig`] carries an iteration cap (`max_iterations`), an
//! optional wall-clock cap (`max_wall_time`), and an optional cost cap
//! (`max_cost_usd`). [`BudgetTracker`] is the runtime counterpart owned by
//! [`EvolutionOrchestrator::run`]. `step()` stays budget-unaware — budget
//! checks live purely in the `run()` loop between steps.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cyberclaw_core::ids::{SkillId, VariantId};

use crate::fitness_evaluator::{
    CaseOutcome, EvaluationCase, FitnessBreakdown, FitnessEvaluator, ScoringMode,
};
use crate::mutation_engine::{MutationEngine, MutationPlan, MutationResult};
use crate::persistent_execution::{ExecutionPlan, ProgressJournal};
use crate::production_evolution_dispatcher::EvolutionCapabilityDispatcher;
use crate::skill_archive::{SelectionMethod, SkillArchive, SkillVariant};
use crate::skill_evolution::CaseTrack;
use crate::verification_gate::{RegressionResult, StagedVerificationGate};

// ============================================================================
// Dispatcher trait — the single I/O boundary
// ============================================================================

/// The orchestrator's dispatch interface. Concrete implementations wire this
/// to the connector runtime; tests provide a mock. The orchestrator never
/// calls outside this trait for I/O — every LLM call, every variant
/// execution, every smoke-test run flows through it.
#[async_trait]
pub trait EvolutionDispatcher: Send + Sync {
    /// Execute a mutation plan. Implementations are expected to dispatch
    /// `plan.target_capability` through a connector and return the resulting
    /// diff / new skill text as a [`MutationResult`].
    async fn execute_mutation(&self, plan: &MutationPlan) -> anyhow::Result<MutationResult>;

    /// Execute a single evaluation case against a candidate variant's skill
    /// text. The dispatcher is responsible for running the variant in
    /// whatever sandbox/runtime is appropriate (e.g. SandboxConnector).
    async fn execute_case(
        &self,
        variant: &SkillVariant,
        skill_text: &str,
        case: &EvaluationCase,
    ) -> anyhow::Result<CaseOutcome>;

    /// Run the smoke regression suite for [`StagedVerificationGate`].
    /// Separate from case execution because smoke-tests cover broader
    /// invariants (compile, basic sanity) not tied to any one case.
    async fn run_smoke_regression(
        &self,
        variant: &SkillVariant,
        skill_text: &str,
    ) -> anyhow::Result<RegressionResult>;
}

// ============================================================================
// Step outcomes
// ============================================================================

/// Why a step ended without adding a new variant to the archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepRejection {
    /// No parent found in archive (empty archive, no seed).
    NoParent,
    /// Dispatcher returned a mutation error.
    MutationError(String),
    /// Dispatcher returned an evaluation error.
    EvaluationError(String),
    /// Dispatcher returned an error running the smoke regression suite.
    SmokeError(String),
    /// Smoke regression failed — stage-2 verification was skipped.
    SmokeFailed,
    /// Stage-2 verification did not approve the plan/journal.
    VerificationRejected,
    /// Child produced but below the configured archive score threshold.
    BelowScoreThreshold,
}

/// The outcome of one generation step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepOutcome {
    /// A new variant was scored, verified, and added to the archive.
    Archived {
        variant_id: VariantId,
        score: f32,
        breakdown: FitnessBreakdown,
    },
    /// The step produced (or attempted) a candidate variant but it was
    /// rejected before landing in the archive. `breakdown` is `Some` only
    /// when scoring actually completed.
    Rejected {
        reason: StepRejection,
        breakdown: Option<FitnessBreakdown>,
    },
    /// A hard error that should stop the caller's loop (logic bug,
    /// irrecoverable state). Dispatcher errors are NOT hard errors — they
    /// become `Rejected`.
    Error(String),
}

// ============================================================================
// Configuration
// ============================================================================

/// Configuration knobs for one orchestrator instance.
#[derive(Debug, Clone)]
pub struct EvolutionConfig {
    /// Parent-selection strategy handed to the archive each step.
    pub selection_method: SelectionMethod,
    /// Score threshold (on composite breakdown) for archive admission.
    /// `0.0` means "archive everything that passes verification".
    pub min_score_to_archive: f32,
    /// Maximum iterations per `run()` call. `step()` ignores this.
    pub max_iterations: u32,
    /// Which [`CaseTrack`] label to attach to children produced by this
    /// orchestrator. Defaults to [`CaseTrack::Success`].
    pub default_track: CaseTrack,
    /// Hard wall-clock cap for a single [`EvolutionOrchestrator::run`]
    /// invocation. `None` disables the check. Only consulted by `run()` —
    /// `step()` remains budget-unaware.
    pub max_wall_time: Option<Duration>,
    /// Optional cost ceiling (USD). `None` disables the check. Cost
    /// accounting is caller-driven: dispatchers (or the surrounding
    /// runtime) should call [`BudgetTracker::record_cost`] to increment.
    pub max_cost_usd: Option<f32>,
}

impl Default for EvolutionConfig {
    fn default() -> Self {
        Self {
            selection_method: SelectionMethod::ScoreChildProp,
            min_score_to_archive: 0.0,
            max_iterations: 10,
            default_track: CaseTrack::Success,
            max_wall_time: None,
            max_cost_usd: None,
        }
    }
}

// ============================================================================
// Budget
// ============================================================================

/// Which budget limit was breached. Returned by
/// [`BudgetTracker::exceeded`] so [`EvolutionOrchestrator::run`] can surface
/// a descriptive reason on early termination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetBreach {
    /// `iterations_used >= config.max_iterations`.
    MaxIterations,
    /// Elapsed wall time exceeds `config.max_wall_time`.
    MaxWallTime,
    /// Accumulated cost exceeds `config.max_cost_usd`.
    MaxCostUsd,
}

/// Per-run budget accounting. Cheap to construct, cheap to query.
///
/// Uses [`Instant`] (monotonic) rather than [`std::time::SystemTime`] because
/// wall-clock budget checks should not be fooled by clock skew / NTP jumps.
#[derive(Debug, Clone)]
pub struct BudgetTracker {
    iterations_used: u32,
    started_at: Instant,
    estimated_cost_usd: f32,
}

impl BudgetTracker {
    /// Start a fresh tracker with `started_at = Instant::now()`.
    pub fn new() -> Self {
        Self {
            iterations_used: 0,
            started_at: Instant::now(),
            estimated_cost_usd: 0.0,
        }
    }

    /// Mark that one more step iteration completed.
    pub fn record_iteration(&mut self) {
        self.iterations_used = self.iterations_used.saturating_add(1);
    }

    /// Accumulate estimated cost (USD). Callers (typically the dispatcher
    /// implementation) decide what counts as a cost and when to record it.
    pub fn record_cost(&mut self, usd: f32) {
        // Clamp pathological negatives to zero; keep accumulation monotonic.
        let delta = if usd.is_finite() && usd > 0.0 {
            usd
        } else {
            0.0
        };
        self.estimated_cost_usd += delta;
    }

    /// How many full step iterations have completed.
    pub fn iterations_used(&self) -> u32 {
        self.iterations_used
    }

    /// Wall-clock time since construction.
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Current accumulated cost estimate (USD).
    pub fn estimated_cost_usd(&self) -> f32 {
        self.estimated_cost_usd
    }

    /// Returns the first tripped budget if any limit has been breached.
    /// Checked in order: iterations → wall-time → cost.
    pub fn exceeded(&self, config: &EvolutionConfig) -> Option<BudgetBreach> {
        if self.iterations_used >= config.max_iterations {
            return Some(BudgetBreach::MaxIterations);
        }
        if let Some(max) = config.max_wall_time {
            if self.elapsed() >= max {
                return Some(BudgetBreach::MaxWallTime);
            }
        }
        if let Some(max) = config.max_cost_usd {
            if self.estimated_cost_usd >= max {
                return Some(BudgetBreach::MaxCostUsd);
            }
        }
        None
    }
}

impl Default for BudgetTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Observability — structured events
// ============================================================================

/// Structured events emitted at every state transition inside `step()`.
///
/// These are intentionally richer than the `tracing` span fields: downstream
/// consumers (test harnesses, audit logs, dashboards) frequently want to
/// pattern-match on the variant rather than parse span attributes.
#[derive(Debug, Clone)]
pub enum EvolutionEvent {
    /// A step began. Emitted before any parent selection.
    StepStarted { iteration: u32, skill_id: SkillId },
    /// Archive selected a parent variant.
    ParentSelected { variant_id: VariantId, score: f32 },
    /// Dispatcher returned a mutation result.
    MutationCompleted {
        parent_id: VariantId,
        child_text_len: usize,
    },
    /// Every evaluation case executed successfully (no dispatcher errors).
    EvaluationCompleted {
        variant_id: VariantId,
        case_count: usize,
        all_passed: bool,
    },
    /// Fitness scoring produced a breakdown.
    FitnessScored {
        variant_id: VariantId,
        breakdown: FitnessBreakdown,
    },
    /// A child variant was admitted to the archive.
    VariantArchived {
        variant_id: VariantId,
        composite_score: f32,
    },
    /// The step produced a candidate that was not archived. When scoring
    /// completed, `breakdown` is `Some` — otherwise `None`.
    VariantRejected {
        reason: StepRejection,
        breakdown: Option<FitnessBreakdown>,
    },
    /// A hard error short-circuited the step.
    StepFailed { error: String },
}

/// Optional subscriber for [`EvolutionEvent`]. Trait methods are async so
/// implementations may persist events to storage / publish to a bus.
#[async_trait]
pub trait EvolutionEventSink: Send + Sync {
    /// Record one event. Implementations should not panic or block on
    /// failure — the orchestrator treats sinks as best-effort.
    async fn record(&self, event: EvolutionEvent);
}

/// Default sink: discards every event. Use when instrumentation isn't
/// needed (unit tests, embedded use).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopEventSink;

#[async_trait]
impl EvolutionEventSink for NoopEventSink {
    async fn record(&self, _event: EvolutionEvent) {
        // intentionally empty
    }
}

/// In-memory sink that captures every event into a `Vec`. Intended for
/// tests — for production use, push events to an async channel or a
/// persistent [`cyberclaw_observability::EventRecorder`].
#[derive(Debug, Default)]
pub struct VecEventSink {
    inner: Mutex<Vec<EvolutionEvent>>,
}

impl VecEventSink {
    /// Empty sink.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Vec::new()),
        }
    }

    /// Consume and return the captured events so far. Recovers from a
    /// poisoned lock — tests that panic mid-step should still read the
    /// trail.
    pub fn take(&self) -> Vec<EvolutionEvent> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *guard)
    }

    /// Non-consuming clone of the captured events.
    pub fn snapshot(&self) -> Vec<EvolutionEvent> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.clone()
    }

    /// How many events have been captured.
    pub fn len(&self) -> usize {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.len()
    }

    /// Whether no events have been captured yet.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl EvolutionEventSink for VecEventSink {
    async fn record(&self, event: EvolutionEvent) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.push(event);
    }
}

// ============================================================================
// Orchestrator
// ============================================================================

/// The orchestrator. Holds the evolution components plus a live archive,
/// driven step-by-step by the caller (or by `run()` for batch advancement).
pub struct EvolutionOrchestrator {
    archive: SkillArchive,
    mutation_engine: MutationEngine,
    fitness_evaluator: FitnessEvaluator,
    verification_gate: StagedVerificationGate,
    config: EvolutionConfig,
    /// Optional structured-event subscriber. `None` means no emission.
    event_sink: Option<Arc<dyn EvolutionEventSink>>,
    /// Optional capability-level dispatcher used for the LLM-as-judge
    /// scoring path. When `None` and the evaluator is configured with
    /// [`ScoringMode::LlmJudge`], `step()` falls back to the synchronous
    /// heuristic path via [`FitnessEvaluator::score`]. When `Some` and
    /// the evaluator is configured with `LlmJudge`, `step()` dispatches
    /// through this trait object (see EVOLUTION_IDIOMS §1).
    llm_judge_dispatcher: Option<Arc<dyn EvolutionCapabilityDispatcher>>,
}

impl EvolutionOrchestrator {
    /// Build a new orchestrator. The caller supplies all components — the
    /// orchestrator takes ownership and becomes the single driver over them.
    pub fn new(
        archive: SkillArchive,
        mutation_engine: MutationEngine,
        fitness_evaluator: FitnessEvaluator,
        verification_gate: StagedVerificationGate,
        config: EvolutionConfig,
    ) -> Self {
        Self {
            archive,
            mutation_engine,
            fitness_evaluator,
            verification_gate,
            config,
            event_sink: None,
            llm_judge_dispatcher: None,
        }
    }

    /// Attach (or replace) the structured-event sink. Returns `self` so this
    /// can be chained onto [`Self::new`].
    pub fn with_event_sink(mut self, sink: Arc<dyn EvolutionEventSink>) -> Self {
        self.event_sink = Some(sink);
        self
    }

    /// Set the event sink without consuming the orchestrator. Useful when
    /// the sink needs to be attached after construction.
    pub fn set_event_sink(&mut self, sink: Arc<dyn EvolutionEventSink>) {
        self.event_sink = Some(sink);
    }

    /// Attach (or replace) the LLM-judge capability dispatcher. Required for
    /// the async [`ScoringMode::LlmJudge`] scoring path to be active; when
    /// omitted, `LlmJudge` falls back to the synchronous heuristic path.
    /// Returns `self` so this can be chained onto [`Self::new`].
    pub fn with_llm_judge_dispatcher(
        mut self,
        dispatcher: Arc<dyn EvolutionCapabilityDispatcher>,
    ) -> Self {
        self.llm_judge_dispatcher = Some(dispatcher);
        self
    }

    /// Set the LLM-judge capability dispatcher without consuming the
    /// orchestrator. See [`Self::with_llm_judge_dispatcher`] for semantics.
    pub fn set_llm_judge_dispatcher(&mut self, dispatcher: Arc<dyn EvolutionCapabilityDispatcher>) {
        self.llm_judge_dispatcher = Some(dispatcher);
    }

    /// Emit one event. Cheap no-op when no sink is attached.
    async fn emit(&self, event: EvolutionEvent) {
        if let Some(sink) = self.event_sink.as_ref() {
            sink.record(event).await;
        }
    }

    /// Read-only archive accessor (for inspection / serialization).
    pub fn archive(&self) -> &SkillArchive {
        &self.archive
    }

    /// Read-only config accessor.
    pub fn config(&self) -> &EvolutionConfig {
        &self.config
    }

    /// Advance one generation.
    ///
    /// * `dispatcher` — the single I/O boundary. All capability dispatch
    ///   happens through it.
    /// * `get_parent_text` — closure used to fetch the full SKILL.md text of
    ///   the selected parent. The archive stores only metadata (artifact ids
    ///   and scores), so the caller bridges the archive to the artifact
    ///   store.
    /// * `cases` — the evaluation cases to score against.
    /// * `plan_for_verification`, `journal_for_verification` — inputs for
    ///   [`StagedVerificationGate`] stage 2.
    /// * `failure_feedback` — GEPA-style prior-failure hint threaded into
    ///   the mutation prompt.
    ///
    /// # Observability
    ///
    /// Each phase emits a `tracing` log at info/debug/warn level plus a
    /// corresponding [`EvolutionEvent`] through the configured sink (if
    /// any). The `iteration` tracing field is populated by the caller
    /// (`run()` sets it via `Span::current().record`); when invoked
    /// directly it is left as the default `0`.
    ///
    /// # Budget
    ///
    /// `step()` is budget-unaware — `run()` owns budget enforcement
    /// between steps.
    #[tracing::instrument(
        level = "info",
        name = "evolution.step",
        skip(
            self,
            dispatcher,
            get_parent_text,
            cases,
            plan_for_verification,
            journal_for_verification,
            failure_feedback,
        ),
        fields(
            archive_size = self.archive.len(),
            case_count = cases.len(),
            min_score = self.config.min_score_to_archive,
            iteration = tracing::field::Empty,
            skill_id = tracing::field::Empty,
            outcome = tracing::field::Empty,
        ),
    )]
    pub async fn step(
        &mut self,
        dispatcher: &dyn EvolutionDispatcher,
        get_parent_text: impl Fn(&SkillVariant) -> String,
        cases: &[EvaluationCase],
        plan_for_verification: &ExecutionPlan,
        journal_for_verification: &ProgressJournal,
        failure_feedback: Option<String>,
    ) -> StepOutcome {
        // 1. Select parent. Clone it so we can release the immutable borrow
        //    on `self.archive` before any mutation happens at the end.
        let parent: SkillVariant = match self.archive.select_parent(self.config.selection_method) {
            Some(p) => p.clone(),
            None => {
                tracing::warn!(reason = "no_parent", "evolution step rejected");
                tracing::Span::current().record("outcome", "no_parent");
                self.emit(EvolutionEvent::VariantRejected {
                    reason: StepRejection::NoParent,
                    breakdown: None,
                })
                .await;
                return StepOutcome::Rejected {
                    reason: StepRejection::NoParent,
                    breakdown: None,
                };
            }
        };

        // Record dynamic span fields now that we know the skill id.
        tracing::Span::current().record("skill_id", parent.skill_id.as_str());

        self.emit(EvolutionEvent::StepStarted {
            iteration: 0,
            skill_id: parent.skill_id.clone(),
        })
        .await;

        tracing::info!(
            parent_id = %parent.variant_id,
            parent_score = parent.score,
            "selected parent",
        );
        self.emit(EvolutionEvent::ParentSelected {
            variant_id: parent.variant_id.clone(),
            score: parent.score,
        })
        .await;

        // 2. Fetch parent SKILL.md text via the caller-provided closure.
        let parent_text = get_parent_text(&parent);

        // 3. Ask the mutation engine for a pure plan.
        let mutation_plan = self
            .mutation_engine
            .plan(&parent, parent_text, failure_feedback);
        tracing::debug!(
            target_capability = %mutation_plan.target_capability.id,
            "mutation plan built",
        );

        // 4. Dispatch the plan. Errors here become Rejected(MutationError)
        //    — the caller's loop should keep going.
        let mutation_result = match dispatcher.execute_mutation(&mutation_plan).await {
            Ok(r) => r,
            Err(e) => {
                let err = e.to_string();
                tracing::warn!(error = %err, "mutation dispatch failed");
                tracing::Span::current().record("outcome", "mutation_error");
                let reason = StepRejection::MutationError(err);
                self.emit(EvolutionEvent::VariantRejected {
                    reason: reason.clone(),
                    breakdown: None,
                })
                .await;
                return StepOutcome::Rejected {
                    reason,
                    breakdown: None,
                };
            }
        };

        let child_text_len = mutation_result.child_skill_text.len();
        tracing::info!(
            result_size = child_text_len,
            diff_artifact = ?mutation_result.diff_artifact_id,
            "mutation dispatched",
        );
        self.emit(EvolutionEvent::MutationCompleted {
            parent_id: parent.variant_id.clone(),
            child_text_len,
        })
        .await;

        // 5. Execute every evaluation case against the child skill text.
        //    First dispatcher error aborts and becomes Rejected(EvaluationError).
        //
        //    We build a temporary candidate `SkillVariant` (not yet in the
        //    archive) so the dispatcher can inspect lineage/skill id if it
        //    needs to.
        let candidate = SkillVariant {
            variant_id: VariantId::new(),
            parent_variant_id: Some(parent.variant_id.clone()),
            skill_id: parent.skill_id.clone(),
            score: 0.0,
            child_count: 0,
            track: self.config.default_track,
            patch_artifact_id: mutation_result.diff_artifact_id.clone(),
        };

        let mut outcomes: Vec<CaseOutcome> = Vec::with_capacity(cases.len());
        for case in cases {
            match dispatcher
                .execute_case(&candidate, &mutation_result.child_skill_text, case)
                .await
            {
                Ok(o) => outcomes.push(o),
                Err(e) => {
                    let err = e.to_string();
                    tracing::warn!(
                        case_id = %case.case_id,
                        error = %err,
                        "case evaluation failed",
                    );
                    tracing::Span::current().record("outcome", "evaluation_error");
                    let reason = StepRejection::EvaluationError(err);
                    self.emit(EvolutionEvent::VariantRejected {
                        reason: reason.clone(),
                        breakdown: None,
                    })
                    .await;
                    return StepOutcome::Rejected {
                        reason,
                        breakdown: None,
                    };
                }
            }
        }

        let passed_count = outcomes.iter().filter(|o| o.execution_passed).count();
        let all_passed = passed_count == outcomes.len();
        tracing::info!(passed_count, total = outcomes.len(), "cases evaluated",);
        self.emit(EvolutionEvent::EvaluationCompleted {
            variant_id: candidate.variant_id.clone(),
            case_count: outcomes.len(),
            all_passed,
        })
        .await;

        // 6. Score the outcomes.
        //
        // When the evaluator is configured for LlmJudge AND an LLM-judge
        // dispatcher is attached, route through the async
        // `score_via_llm_judge` path; otherwise fall back to the synchronous
        // scorer (deterministic / heuristic / heuristic-fallback-for-llm).
        let breakdown = match (
            &self.fitness_evaluator.default_scoring,
            self.llm_judge_dispatcher.as_ref(),
        ) {
            (ScoringMode::LlmJudge { judge_capability }, Some(llm_dispatcher)) => {
                match self
                    .fitness_evaluator
                    .score_via_llm_judge(
                        llm_dispatcher.as_ref(),
                        judge_capability,
                        &outcomes,
                        cases,
                        &mutation_result.child_skill_text,
                    )
                    .await
                {
                    Ok(b) => b,
                    Err(e) => {
                        let err = e.to_string();
                        tracing::warn!(
                            error = %err,
                            "llm judge scoring failed; falling back to synchronous scorer",
                        );
                        self.fitness_evaluator.score(
                            &outcomes,
                            cases,
                            &mutation_result.child_skill_text,
                        )
                    }
                }
            }
            _ => self
                .fitness_evaluator
                .score(&outcomes, cases, &mutation_result.child_skill_text),
        };
        let composite = breakdown.composite();
        tracing::info!(composite, ?breakdown, "fitness scored",);
        self.emit(EvolutionEvent::FitnessScored {
            variant_id: candidate.variant_id.clone(),
            breakdown: breakdown.clone(),
        })
        .await;

        // 7. Run the smoke regression via the dispatcher.
        let smoke = match dispatcher
            .run_smoke_regression(&candidate, &mutation_result.child_skill_text)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let err = e.to_string();
                tracing::warn!(error = %err, "smoke regression failed");
                tracing::Span::current().record("outcome", "smoke_error");
                let reason = StepRejection::SmokeError(err);
                self.emit(EvolutionEvent::VariantRejected {
                    reason: reason.clone(),
                    breakdown: Some(breakdown.clone()),
                })
                .await;
                return StepOutcome::Rejected {
                    reason,
                    breakdown: Some(breakdown),
                };
            }
        };

        // 8. Stage-2 verification. StagedVerificationGate::check skips stage 2
        //    itself when smoke fails, so we inspect the verdict afterwards.
        let verdict =
            self.verification_gate
                .check(smoke, plan_for_verification, journal_for_verification);

        tracing::info!(
            approved = verdict.approved,
            stage2_skipped = verdict.stage2_skipped,
            "verification verdict",
        );

        if verdict.stage2_skipped {
            tracing::warn!(reason = "smoke_failed", "variant rejected");
            tracing::Span::current().record("outcome", "smoke_failed");
            self.emit(EvolutionEvent::VariantRejected {
                reason: StepRejection::SmokeFailed,
                breakdown: Some(breakdown.clone()),
            })
            .await;
            return StepOutcome::Rejected {
                reason: StepRejection::SmokeFailed,
                breakdown: Some(breakdown),
            };
        }

        if !verdict.approved {
            tracing::warn!(reason = "verification_rejected", "variant rejected");
            tracing::Span::current().record("outcome", "verification_rejected");
            self.emit(EvolutionEvent::VariantRejected {
                reason: StepRejection::VerificationRejected,
                breakdown: Some(breakdown.clone()),
            })
            .await;
            return StepOutcome::Rejected {
                reason: StepRejection::VerificationRejected,
                breakdown: Some(breakdown),
            };
        }

        // 9. Score gate for archive admission.
        if composite < self.config.min_score_to_archive {
            tracing::warn!(
                composite,
                min_score = self.config.min_score_to_archive,
                "variant rejected: below threshold",
            );
            tracing::Span::current().record("outcome", "below_threshold");
            self.emit(EvolutionEvent::VariantRejected {
                reason: StepRejection::BelowScoreThreshold,
                breakdown: Some(breakdown.clone()),
            })
            .await;
            return StepOutcome::Rejected {
                reason: StepRejection::BelowScoreThreshold,
                breakdown: Some(breakdown),
            };
        }

        // 10. Build and archive the child variant.
        let child = SkillVariant {
            variant_id: candidate.variant_id.clone(),
            parent_variant_id: Some(parent.variant_id.clone()),
            skill_id: parent.skill_id.clone(),
            score: composite,
            child_count: 0,
            track: self.config.default_track,
            patch_artifact_id: mutation_result.diff_artifact_id.clone(),
        };

        let archived_id = child.variant_id.clone();
        self.archive.add_variant(child);

        tracing::info!(
            variant_id = %archived_id,
            score = composite,
            "variant archived",
        );
        tracing::Span::current().record("outcome", "archived");
        self.emit(EvolutionEvent::VariantArchived {
            variant_id: archived_id.clone(),
            composite_score: composite,
        })
        .await;

        StepOutcome::Archived {
            variant_id: archived_id,
            score: composite,
            breakdown,
        }
    }

    /// Convenience: drive [`Self::step`] in a loop up to the configured
    /// budget limits. Returns all step outcomes in order.
    ///
    /// # Termination
    ///
    /// The loop stops on any of the following:
    ///
    /// - `config.max_iterations` reached (always configured).
    /// - `config.max_wall_time` elapsed (if `Some`).
    /// - `config.max_cost_usd` accumulated (if `Some`).
    /// - `step()` returned [`StepOutcome::Error`] (hard error).
    ///
    /// When a budget limit trips, the final element of the returned `Vec`
    /// is a [`StepOutcome::Error`] whose message is `"budget: <variant>"`
    /// (e.g. `"budget: MaxWallTime"`) so downstream callers can
    /// distinguish budget termination from normal completion without a
    /// separate channel.
    #[tracing::instrument(
        level = "info",
        name = "evolution.run",
        skip(self, dispatcher, get_parent_text, cases, plan, journal),
        fields(
            max_iterations = self.config.max_iterations,
            max_wall_time_secs = tracing::field::Empty,
            max_cost_usd = tracing::field::Empty,
            iterations_completed = tracing::field::Empty,
            termination = tracing::field::Empty,
        ),
    )]
    pub async fn run(
        &mut self,
        dispatcher: &dyn EvolutionDispatcher,
        get_parent_text: impl Fn(&SkillVariant) -> String + Copy,
        cases: &[EvaluationCase],
        plan: &ExecutionPlan,
        journal: &ProgressJournal,
    ) -> Vec<StepOutcome> {
        if let Some(w) = self.config.max_wall_time {
            tracing::Span::current().record("max_wall_time_secs", w.as_secs());
        }
        if let Some(c) = self.config.max_cost_usd {
            tracing::Span::current().record("max_cost_usd", c);
        }

        let mut out = Vec::with_capacity(self.config.max_iterations as usize);
        let mut tracker = BudgetTracker::new();

        loop {
            if let Some(breach) = tracker.exceeded(&self.config) {
                // Budget tripped before step(): surface as an Error so the
                // caller can see why the loop terminated. We don't emit a
                // StepFailed event for "MaxIterations" when iterations ==
                // max — that's normal completion; only non-iteration
                // breaches warrant a warn.
                match breach {
                    BudgetBreach::MaxIterations => {
                        tracing::info!(
                            iterations = tracker.iterations_used(),
                            "run terminated: max_iterations reached",
                        );
                    }
                    BudgetBreach::MaxWallTime => {
                        tracing::warn!(
                            elapsed_secs = tracker.elapsed().as_secs(),
                            "run terminated: wall-clock budget exceeded",
                        );
                        let msg = format!("budget: {:?}", breach);
                        self.emit(EvolutionEvent::StepFailed { error: msg.clone() })
                            .await;
                        out.push(StepOutcome::Error(msg));
                    }
                    BudgetBreach::MaxCostUsd => {
                        tracing::warn!(
                            estimated_cost_usd = tracker.estimated_cost_usd(),
                            "run terminated: cost budget exceeded",
                        );
                        let msg = format!("budget: {:?}", breach);
                        self.emit(EvolutionEvent::StepFailed { error: msg.clone() })
                            .await;
                        out.push(StepOutcome::Error(msg));
                    }
                }
                tracing::Span::current().record("termination", format!("{:?}", breach).as_str());
                break;
            }

            let outcome = self
                .step(dispatcher, get_parent_text, cases, plan, journal, None)
                .await;
            let is_hard_error = matches!(&outcome, StepOutcome::Error(_));
            out.push(outcome);
            tracker.record_iteration();
            if is_hard_error {
                tracing::Span::current().record("termination", "hard_error");
                break;
            }
        }

        tracing::Span::current().record("iterations_completed", tracker.iterations_used());
        out
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use cyberclaw_core::capability::{CapabilityEffect, CapabilityRef, RiskLevel};
    use cyberclaw_core::ids::{ArtifactId, CapabilityId, ConnectorId, SkillId, VariantId};
    use serde_json::json;

    use crate::persistent_execution::{AcceptanceCriterion, ExecutionPlan, ProgressJournal, Story};
    use crate::verification_gate::{
        CommandResult, RegressionResult, RegressionSpec, StagedVerificationGate,
    };

    // ------------------------------------------------------------------------
    // MockDispatcher — the tests' single I/O boundary.
    // ------------------------------------------------------------------------

    /// Mock dispatcher. Each method can be told to succeed or fail.
    struct MockDispatcher {
        mutation_result: Result<MutationResult, String>,
        // keyed by case_id → CaseOutcome result
        case_outputs: HashMap<String, Result<CaseOutcome, String>>,
        smoke_result: Result<RegressionResult, String>,
        // call counters for assertions
        mutation_calls: Mutex<u32>,
        case_calls: Mutex<u32>,
        smoke_calls: Mutex<u32>,
    }

    impl MockDispatcher {
        fn new(
            mutation_result: Result<MutationResult, String>,
            case_outputs: HashMap<String, Result<CaseOutcome, String>>,
            smoke_result: Result<RegressionResult, String>,
        ) -> Self {
            Self {
                mutation_result,
                case_outputs,
                smoke_result,
                mutation_calls: Mutex::new(0),
                case_calls: Mutex::new(0),
                smoke_calls: Mutex::new(0),
            }
        }
    }

    #[async_trait]
    impl EvolutionDispatcher for MockDispatcher {
        async fn execute_mutation(&self, _plan: &MutationPlan) -> anyhow::Result<MutationResult> {
            *self.mutation_calls.lock().unwrap() += 1;
            self.mutation_result.clone().map_err(|e| anyhow::anyhow!(e))
        }

        async fn execute_case(
            &self,
            _variant: &SkillVariant,
            _skill_text: &str,
            case: &EvaluationCase,
        ) -> anyhow::Result<CaseOutcome> {
            *self.case_calls.lock().unwrap() += 1;
            match self.case_outputs.get(&case.case_id) {
                Some(Ok(o)) => Ok(o.clone()),
                Some(Err(e)) => Err(anyhow::anyhow!(e.clone())),
                None => Err(anyhow::anyhow!("no mock output for case {}", case.case_id)),
            }
        }

        async fn run_smoke_regression(
            &self,
            _variant: &SkillVariant,
            _skill_text: &str,
        ) -> anyhow::Result<RegressionResult> {
            *self.smoke_calls.lock().unwrap() += 1;
            self.smoke_result.clone().map_err(|e| anyhow::anyhow!(e))
        }
    }

    // ------------------------------------------------------------------------
    // Helpers.
    // ------------------------------------------------------------------------

    fn mk_cap(id: &str) -> CapabilityRef {
        CapabilityRef {
            id: CapabilityId::from_string(id.to_string()).expect("cap id"),
            connector_id: ConnectorId::from_string("llm".to_string()).expect("conn id"),
            risk: RiskLevel::Medium,
            effects: vec![CapabilityEffect::Execute],
            placement: None,
        }
    }

    fn mk_case(id: &str, expected: serde_json::Value) -> EvaluationCase {
        EvaluationCase {
            case_id: id.to_string(),
            task_input: json!({"in": id}),
            expected_behavior: "match the expected output".to_string(),
            expected_output: Some(expected),
        }
    }

    fn mk_outcome(id: &str, output: serde_json::Value, ok: bool) -> CaseOutcome {
        CaseOutcome {
            case_id: id.to_string(),
            actual_output: output,
            execution_passed: ok,
            elapsed_ms: 5,
        }
    }

    fn mk_seed(skill_id: SkillId, score: f32) -> SkillVariant {
        SkillVariant {
            variant_id: VariantId::new(),
            parent_variant_id: None,
            skill_id,
            score,
            child_count: 0,
            track: CaseTrack::Success,
            patch_artifact_id: None,
        }
    }

    fn mk_mutation_result(text: &str) -> MutationResult {
        MutationResult {
            child_skill_text: text.to_string(),
            diff_artifact_id: Some(ArtifactId::new()),
            reasoning: Some("improved".to_string()),
        }
    }

    fn mk_smoke_pass() -> RegressionResult {
        let spec = RegressionSpec::minimal();
        let results = vec![
            CommandResult {
                label: "Build".to_string(),
                passed: true,
                output_excerpt: None,
            },
            CommandResult {
                label: "Test".to_string(),
                passed: true,
                output_excerpt: None,
            },
        ];
        RegressionResult::from_results(results, &spec)
    }

    fn mk_smoke_fail() -> RegressionResult {
        let spec = RegressionSpec::minimal();
        let results = vec![
            CommandResult {
                label: "Build".to_string(),
                passed: false,
                output_excerpt: Some("build failed".to_string()),
            },
            CommandResult {
                label: "Test".to_string(),
                passed: false,
                output_excerpt: None,
            },
        ];
        RegressionResult::from_results(results, &spec)
    }

    /// Build an ExecutionPlan with a single passed story + evidence — this
    /// is what `StagedVerificationGate::check` will approve.
    fn mk_passed_plan() -> (ExecutionPlan, ProgressJournal) {
        let mut plan = ExecutionPlan::new("Evolve");
        let mut story = Story::new(
            "S1",
            "Evolve skill",
            vec![AcceptanceCriterion::new("Scored above threshold")],
        );
        story.criteria[0].mark_met("score=0.8");
        story.mark_passed();
        plan.add_story(story);

        let mut journal = ProgressJournal::new();
        journal.track_file("skill.md");
        (plan, journal)
    }

    /// Build an ExecutionPlan that the gate will reject (failed story).
    fn mk_rejecting_plan() -> (ExecutionPlan, ProgressJournal) {
        let mut plan = ExecutionPlan::new("Evolve");
        let mut story = Story::new("S1", "Evolve skill", vec![]);
        // Failed state causes criteria gate to reject.
        story.state = crate::persistent_execution::StoryState::Failed;
        story.attempt_count = 2;
        plan.add_story(story);
        (plan, ProgressJournal::new())
    }

    fn mk_orchestrator_with_seed(min_score: f32) -> (EvolutionOrchestrator, SkillId, SkillVariant) {
        let skill_id = SkillId::new();
        let mut archive = SkillArchive::new(skill_id.clone());
        let seed = mk_seed(skill_id.clone(), 0.5);
        let seed_clone = seed.clone();
        archive.add_variant(seed);

        let engine = MutationEngine::new(mk_cap("llm.generate_diff"));
        let evaluator = FitnessEvaluator::new();
        let gate = StagedVerificationGate::default();
        let config = EvolutionConfig {
            min_score_to_archive: min_score,
            max_iterations: 3,
            ..EvolutionConfig::default()
        };
        (
            EvolutionOrchestrator::new(archive, engine, evaluator, gate, config),
            skill_id,
            seed_clone,
        )
    }

    fn empty_orchestrator() -> EvolutionOrchestrator {
        let skill_id = SkillId::new();
        let archive = SkillArchive::new(skill_id);
        let engine = MutationEngine::new(mk_cap("llm.generate_diff"));
        let evaluator = FitnessEvaluator::new();
        let gate = StagedVerificationGate::default();
        EvolutionOrchestrator::new(archive, engine, evaluator, gate, EvolutionConfig::default())
    }

    // ------------------------------------------------------------------------
    // Tests.
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn step_with_empty_archive_returns_no_parent() {
        let mut orch = empty_orchestrator();
        let dispatcher = MockDispatcher::new(
            Ok(mk_mutation_result("# child")),
            HashMap::new(),
            Ok(mk_smoke_pass()),
        );
        let (plan, journal) = mk_passed_plan();

        let outcome = orch
            .step(
                &dispatcher,
                |_| "# parent".to_string(),
                &[],
                &plan,
                &journal,
                None,
            )
            .await;

        match outcome {
            StepOutcome::Rejected {
                reason: StepRejection::NoParent,
                breakdown,
            } => {
                assert!(breakdown.is_none());
            }
            other => panic!("expected NoParent rejection, got {:?}", other),
        }
        assert_eq!(*dispatcher.mutation_calls.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn step_happy_path_archives_child() {
        let (mut orch, _skill, _seed) = mk_orchestrator_with_seed(0.0);
        let cases = vec![mk_case("a", json!({"ok": true}))];
        let mut outputs = HashMap::new();
        outputs.insert(
            "a".to_string(),
            Ok(mk_outcome("a", json!({"ok": true}), true)),
        );

        let dispatcher = MockDispatcher::new(
            Ok(mk_mutation_result("# improved")),
            outputs,
            Ok(mk_smoke_pass()),
        );
        let (plan, journal) = mk_passed_plan();

        let outcome = orch
            .step(
                &dispatcher,
                |_| "# parent".to_string(),
                &cases,
                &plan,
                &journal,
                None,
            )
            .await;

        match outcome {
            StepOutcome::Archived { score, .. } => {
                assert!(score > 0.0, "expected positive composite, got {}", score);
            }
            other => panic!("expected Archived, got {:?}", other),
        }
        assert_eq!(*dispatcher.mutation_calls.lock().unwrap(), 1);
        assert_eq!(*dispatcher.case_calls.lock().unwrap(), 1);
        assert_eq!(*dispatcher.smoke_calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn step_mutation_error_rejects() {
        let (mut orch, _skill, _seed) = mk_orchestrator_with_seed(0.0);
        let dispatcher = MockDispatcher::new(
            Err("LLM timeout".to_string()),
            HashMap::new(),
            Ok(mk_smoke_pass()),
        );
        let (plan, journal) = mk_passed_plan();

        let outcome = orch
            .step(
                &dispatcher,
                |_| "# parent".to_string(),
                &[],
                &plan,
                &journal,
                None,
            )
            .await;

        match outcome {
            StepOutcome::Rejected {
                reason: StepRejection::MutationError(msg),
                ..
            } => {
                assert!(msg.contains("LLM timeout"));
            }
            other => panic!("expected MutationError, got {:?}", other),
        }
        assert_eq!(*dispatcher.case_calls.lock().unwrap(), 0);
        assert_eq!(*dispatcher.smoke_calls.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn step_case_error_rejects() {
        let (mut orch, _skill, _seed) = mk_orchestrator_with_seed(0.0);
        let cases = vec![mk_case("a", json!({"ok": true}))];
        let mut outputs = HashMap::new();
        outputs.insert("a".to_string(), Err("sandbox crash".to_string()));

        let dispatcher = MockDispatcher::new(
            Ok(mk_mutation_result("# child")),
            outputs,
            Ok(mk_smoke_pass()),
        );
        let (plan, journal) = mk_passed_plan();

        let outcome = orch
            .step(
                &dispatcher,
                |_| "# parent".to_string(),
                &cases,
                &plan,
                &journal,
                None,
            )
            .await;

        match outcome {
            StepOutcome::Rejected {
                reason: StepRejection::EvaluationError(msg),
                ..
            } => {
                assert!(msg.contains("sandbox crash"));
            }
            other => panic!("expected EvaluationError, got {:?}", other),
        }
        assert_eq!(*dispatcher.smoke_calls.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn step_smoke_fail_rejects_and_skips_stage2() {
        let (mut orch, _skill, _seed) = mk_orchestrator_with_seed(0.0);
        let cases = vec![mk_case("a", json!({"ok": true}))];
        let mut outputs = HashMap::new();
        outputs.insert(
            "a".to_string(),
            Ok(mk_outcome("a", json!({"ok": true}), true)),
        );

        let dispatcher = MockDispatcher::new(
            Ok(mk_mutation_result("# child")),
            outputs,
            Ok(mk_smoke_fail()),
        );
        let (plan, journal) = mk_passed_plan();

        let outcome = orch
            .step(
                &dispatcher,
                |_| "# parent".to_string(),
                &cases,
                &plan,
                &journal,
                None,
            )
            .await;

        match outcome {
            StepOutcome::Rejected {
                reason: StepRejection::SmokeFailed,
                breakdown,
            } => {
                assert!(breakdown.is_some(), "scoring should have run before smoke");
            }
            other => panic!("expected SmokeFailed, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn step_verification_reject() {
        let (mut orch, _skill, _seed) = mk_orchestrator_with_seed(0.0);
        let cases = vec![mk_case("a", json!({"ok": true}))];
        let mut outputs = HashMap::new();
        outputs.insert(
            "a".to_string(),
            Ok(mk_outcome("a", json!({"ok": true}), true)),
        );

        let dispatcher = MockDispatcher::new(
            Ok(mk_mutation_result("# child")),
            outputs,
            Ok(mk_smoke_pass()),
        );
        // Reject at stage 2 (failed story in plan).
        let (plan, journal) = mk_rejecting_plan();

        let outcome = orch
            .step(
                &dispatcher,
                |_| "# parent".to_string(),
                &cases,
                &plan,
                &journal,
                None,
            )
            .await;

        match outcome {
            StepOutcome::Rejected {
                reason: StepRejection::VerificationRejected,
                breakdown,
            } => {
                assert!(breakdown.is_some());
            }
            other => panic!("expected VerificationRejected, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn step_below_score_threshold_rejects() {
        // Threshold 0.8, but we'll score well below that.
        let (mut orch, _skill, _seed) = mk_orchestrator_with_seed(0.8);

        // One case, execution-passed but wrong output → correctness=0,
        // procedure=1, conciseness small → composite well under 0.8.
        let cases = vec![mk_case("a", json!({"ok": true}))];
        let mut outputs = HashMap::new();
        outputs.insert(
            "a".to_string(),
            Ok(mk_outcome("a", json!({"ok": false}), true)),
        );

        let dispatcher = MockDispatcher::new(
            Ok(mk_mutation_result("# child")),
            outputs,
            Ok(mk_smoke_pass()),
        );
        let (plan, journal) = mk_passed_plan();

        let outcome = orch
            .step(
                &dispatcher,
                |_| "# parent".to_string(),
                &cases,
                &plan,
                &journal,
                None,
            )
            .await;

        match outcome {
            StepOutcome::Rejected {
                reason: StepRejection::BelowScoreThreshold,
                breakdown,
            } => {
                let b = breakdown.expect("breakdown present");
                assert!(b.composite() < 0.8);
            }
            other => panic!("expected BelowScoreThreshold, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn step_adds_variant_to_archive() {
        let (mut orch, _skill, _seed) = mk_orchestrator_with_seed(0.0);
        let initial_len = orch.archive().len();
        let cases = vec![mk_case("a", json!({"ok": true}))];
        let mut outputs = HashMap::new();
        outputs.insert(
            "a".to_string(),
            Ok(mk_outcome("a", json!({"ok": true}), true)),
        );

        let dispatcher = MockDispatcher::new(
            Ok(mk_mutation_result("# improved")),
            outputs,
            Ok(mk_smoke_pass()),
        );
        let (plan, journal) = mk_passed_plan();

        let _ = orch
            .step(
                &dispatcher,
                |_| "# parent".to_string(),
                &cases,
                &plan,
                &journal,
                None,
            )
            .await;

        assert_eq!(orch.archive().len(), initial_len + 1);
    }

    #[tokio::test]
    async fn step_child_has_correct_parent_link() {
        let (mut orch, _skill, seed) = mk_orchestrator_with_seed(0.0);
        let cases = vec![mk_case("a", json!({"ok": true}))];
        let mut outputs = HashMap::new();
        outputs.insert(
            "a".to_string(),
            Ok(mk_outcome("a", json!({"ok": true}), true)),
        );

        let dispatcher = MockDispatcher::new(
            Ok(mk_mutation_result("# improved")),
            outputs,
            Ok(mk_smoke_pass()),
        );
        let (plan, journal) = mk_passed_plan();

        let _ = orch
            .step(
                &dispatcher,
                |_| "# parent".to_string(),
                &cases,
                &plan,
                &journal,
                None,
            )
            .await;

        let last = orch.archive().variants.last().expect("child variant");
        assert_eq!(last.parent_variant_id.as_ref(), Some(&seed.variant_id));
    }

    #[tokio::test]
    async fn step_child_inherits_skill_id() {
        let (mut orch, skill, _seed) = mk_orchestrator_with_seed(0.0);
        let cases = vec![mk_case("a", json!({"ok": true}))];
        let mut outputs = HashMap::new();
        outputs.insert(
            "a".to_string(),
            Ok(mk_outcome("a", json!({"ok": true}), true)),
        );

        let dispatcher = MockDispatcher::new(
            Ok(mk_mutation_result("# improved")),
            outputs,
            Ok(mk_smoke_pass()),
        );
        let (plan, journal) = mk_passed_plan();

        let _ = orch
            .step(
                &dispatcher,
                |_| "# parent".to_string(),
                &cases,
                &plan,
                &journal,
                None,
            )
            .await;

        let last = orch.archive().variants.last().expect("child variant");
        assert_eq!(last.skill_id, skill);
    }

    #[tokio::test]
    async fn run_respects_max_iterations() {
        let (mut orch, _skill, _seed) = mk_orchestrator_with_seed(0.0);
        // config is max_iterations=3.
        let cases = vec![mk_case("a", json!({"ok": true}))];
        let mut outputs = HashMap::new();
        outputs.insert(
            "a".to_string(),
            Ok(mk_outcome("a", json!({"ok": true}), true)),
        );

        let dispatcher = MockDispatcher::new(
            Ok(mk_mutation_result("# improved")),
            outputs,
            Ok(mk_smoke_pass()),
        );
        let (plan, journal) = mk_passed_plan();

        let outcomes = orch
            .run(
                &dispatcher,
                |_| "# parent".to_string(),
                &cases,
                &plan,
                &journal,
            )
            .await;

        assert!(outcomes.len() <= 3);
        assert_eq!(outcomes.len(), 3);
    }

    #[tokio::test]
    async fn dispatcher_error_does_not_poison_archive() {
        let (mut orch, _skill, _seed) = mk_orchestrator_with_seed(0.0);
        let before = orch.archive().len();

        let dispatcher =
            MockDispatcher::new(Err("boom".to_string()), HashMap::new(), Ok(mk_smoke_pass()));
        let (plan, journal) = mk_passed_plan();

        let _ = orch
            .step(
                &dispatcher,
                |_| "# parent".to_string(),
                &[],
                &plan,
                &journal,
                None,
            )
            .await;

        assert_eq!(orch.archive().len(), before);
    }

    #[test]
    fn evolution_config_default_sensible() {
        let cfg = EvolutionConfig::default();
        assert!(matches!(
            cfg.selection_method,
            SelectionMethod::ScoreChildProp
        ));
        assert!((cfg.min_score_to_archive - 0.0).abs() < f32::EPSILON);
        assert_eq!(cfg.max_iterations, 10);
        assert!(matches!(cfg.default_track, CaseTrack::Success));
        assert!(cfg.max_wall_time.is_none());
        assert!(cfg.max_cost_usd.is_none());
    }

    // ------------------------------------------------------------------------
    // Sprint-2 additions: observability + budget.
    // ------------------------------------------------------------------------

    use std::sync::Arc;
    use std::time::Duration;

    /// Dispatcher that sleeps `delay` inside `execute_mutation` so wall-time
    /// budget tests can trip the limit deterministically.
    struct SlowDispatcher {
        delay: Duration,
        mutation_result: MutationResult,
        smoke_result: RegressionResult,
    }

    #[async_trait]
    impl EvolutionDispatcher for SlowDispatcher {
        async fn execute_mutation(&self, _plan: &MutationPlan) -> anyhow::Result<MutationResult> {
            tokio::time::sleep(self.delay).await;
            Ok(self.mutation_result.clone())
        }
        async fn execute_case(
            &self,
            _variant: &SkillVariant,
            _skill_text: &str,
            case: &EvaluationCase,
        ) -> anyhow::Result<CaseOutcome> {
            Ok(mk_outcome(
                &case.case_id,
                case.expected_output.clone().unwrap_or(json!({})),
                true,
            ))
        }
        async fn run_smoke_regression(
            &self,
            _variant: &SkillVariant,
            _skill_text: &str,
        ) -> anyhow::Result<RegressionResult> {
            Ok(self.smoke_result.clone())
        }
    }

    fn happy_mock() -> MockDispatcher {
        let mut outputs = HashMap::new();
        outputs.insert(
            "a".to_string(),
            Ok(mk_outcome("a", json!({"ok": true}), true)),
        );
        MockDispatcher::new(
            Ok(mk_mutation_result("# improved")),
            outputs,
            Ok(mk_smoke_pass()),
        )
    }

    #[tokio::test]
    async fn noop_sink_no_panic() {
        let (mut orch, _skill, _seed) = mk_orchestrator_with_seed(0.0);
        orch.set_event_sink(Arc::new(NoopEventSink));
        let cases = vec![mk_case("a", json!({"ok": true}))];
        let dispatcher = happy_mock();
        let (plan, journal) = mk_passed_plan();

        let outcome = orch
            .step(
                &dispatcher,
                |_| "# parent".to_string(),
                &cases,
                &plan,
                &journal,
                None,
            )
            .await;
        assert!(matches!(outcome, StepOutcome::Archived { .. }));
    }

    #[tokio::test]
    async fn vec_sink_captures_all_events_on_happy_path() {
        let (mut orch, _skill, _seed) = mk_orchestrator_with_seed(0.0);
        let sink = Arc::new(VecEventSink::new());
        orch.set_event_sink(sink.clone());

        let cases = vec![mk_case("a", json!({"ok": true}))];
        let dispatcher = happy_mock();
        let (plan, journal) = mk_passed_plan();

        let _ = orch
            .step(
                &dispatcher,
                |_| "# parent".to_string(),
                &cases,
                &plan,
                &journal,
                None,
            )
            .await;

        let events = sink.snapshot();
        // Expect: StepStarted, ParentSelected, MutationCompleted,
        // EvaluationCompleted, FitnessScored, VariantArchived.
        assert_eq!(events.len(), 6, "got {:?}", events);
        assert!(matches!(events[0], EvolutionEvent::StepStarted { .. }));
        assert!(matches!(events[1], EvolutionEvent::ParentSelected { .. }));
        assert!(matches!(
            events[2],
            EvolutionEvent::MutationCompleted { .. }
        ));
        assert!(matches!(
            events[3],
            EvolutionEvent::EvaluationCompleted { .. }
        ));
        assert!(matches!(events[4], EvolutionEvent::FitnessScored { .. }));
        assert!(matches!(events[5], EvolutionEvent::VariantArchived { .. }));
    }

    #[tokio::test]
    async fn vec_sink_captures_rejection() {
        // No parent → VariantRejected emitted directly, nothing else.
        let mut orch = empty_orchestrator();
        let sink = Arc::new(VecEventSink::new());
        orch.set_event_sink(sink.clone());

        let dispatcher = happy_mock();
        let (plan, journal) = mk_passed_plan();

        let _ = orch
            .step(
                &dispatcher,
                |_| "# parent".to_string(),
                &[],
                &plan,
                &journal,
                None,
            )
            .await;

        let events = sink.snapshot();
        assert_eq!(events.len(), 1);
        match &events[0] {
            EvolutionEvent::VariantRejected { reason, breakdown } => {
                assert!(matches!(reason, StepRejection::NoParent));
                assert!(breakdown.is_none());
            }
            other => panic!("expected VariantRejected, got {:?}", other),
        }
    }

    #[test]
    fn budget_tracker_records_iterations() {
        let mut tracker = BudgetTracker::new();
        assert_eq!(tracker.iterations_used(), 0);
        tracker.record_iteration();
        tracker.record_iteration();
        assert_eq!(tracker.iterations_used(), 2);
    }

    #[test]
    fn budget_tracker_records_cost() {
        let mut tracker = BudgetTracker::new();
        assert!((tracker.estimated_cost_usd() - 0.0).abs() < f32::EPSILON);
        tracker.record_cost(0.5);
        tracker.record_cost(0.25);
        // Negatives / NaN must not decrement the accumulator.
        tracker.record_cost(-10.0);
        tracker.record_cost(f32::NAN);
        assert!((tracker.estimated_cost_usd() - 0.75).abs() < 1e-6);
    }

    #[test]
    fn budget_tracker_exceeded_checks_order() {
        // Construct a config that trips max_iterations at 1.
        let cfg = EvolutionConfig {
            max_iterations: 1,
            max_wall_time: Some(Duration::from_secs(3600)),
            max_cost_usd: Some(1000.0),
            ..EvolutionConfig::default()
        };

        let mut tracker = BudgetTracker::new();
        assert!(tracker.exceeded(&cfg).is_none());
        tracker.record_iteration();
        assert!(matches!(
            tracker.exceeded(&cfg),
            Some(BudgetBreach::MaxIterations)
        ));

        // Cost-only trip.
        let cfg_cost = EvolutionConfig {
            max_iterations: 1000,
            max_wall_time: None,
            max_cost_usd: Some(1.0),
            ..EvolutionConfig::default()
        };
        let mut t2 = BudgetTracker::new();
        t2.record_cost(1.5);
        assert!(matches!(
            t2.exceeded(&cfg_cost),
            Some(BudgetBreach::MaxCostUsd)
        ));
    }

    #[tokio::test]
    async fn budget_exceeded_max_iterations() {
        // Default config has max_iterations = 3 via mk_orchestrator_with_seed.
        let (mut orch, _skill, _seed) = mk_orchestrator_with_seed(0.0);
        let cases = vec![mk_case("a", json!({"ok": true}))];
        let dispatcher = happy_mock();
        let (plan, journal) = mk_passed_plan();

        let outcomes = orch
            .run(
                &dispatcher,
                |_| "# parent".to_string(),
                &cases,
                &plan,
                &journal,
            )
            .await;

        assert_eq!(outcomes.len(), 3);
        // None of the outcomes should be the budget-Error sentinel because
        // MaxIterations is treated as normal completion (no error appended).
        assert!(outcomes.iter().all(|o| !matches!(o, StepOutcome::Error(_))));
    }

    #[tokio::test]
    async fn budget_exceeded_max_wall_time() {
        // Uses real wall time: dispatcher sleeps ~30ms per step, budget is
        // 10ms, so the check between step 1 and step 2 trips. We keep the
        // timings small so the test remains sub-second; max_iterations is
        // high enough that the budget (not iterations) is what stops us.
        let skill_id = SkillId::new();
        let mut archive = SkillArchive::new(skill_id.clone());
        archive.add_variant(mk_seed(skill_id, 0.5));

        let config = EvolutionConfig {
            min_score_to_archive: 0.0,
            max_iterations: 100,
            max_wall_time: Some(Duration::from_millis(10)),
            max_cost_usd: None,
            ..EvolutionConfig::default()
        };
        let mut orch = EvolutionOrchestrator::new(
            archive,
            MutationEngine::new(mk_cap("llm.generate_diff")),
            FitnessEvaluator::new(),
            StagedVerificationGate::default(),
            config,
        );

        let dispatcher = SlowDispatcher {
            delay: Duration::from_millis(30),
            mutation_result: mk_mutation_result("# improved"),
            smoke_result: mk_smoke_pass(),
        };
        let cases = vec![mk_case("a", json!({"ok": true}))];
        let (plan, journal) = mk_passed_plan();

        let outcomes = orch
            .run(
                &dispatcher,
                |_| "# parent".to_string(),
                &cases,
                &plan,
                &journal,
            )
            .await;

        assert!(
            outcomes.len() < 100,
            "expected wall-time budget to stop early, got {} outcomes",
            outcomes.len(),
        );
        match outcomes.last().expect("at least one outcome") {
            StepOutcome::Error(msg) => assert!(
                msg.contains("MaxWallTime"),
                "expected MaxWallTime sentinel, got {:?}",
                msg
            ),
            other => panic!("expected Error sentinel, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn budget_unconfigured_runs_full_iterations() {
        let skill_id = SkillId::new();
        let mut archive = SkillArchive::new(skill_id.clone());
        archive.add_variant(mk_seed(skill_id, 0.5));

        let config = EvolutionConfig {
            max_iterations: 4,
            max_wall_time: None,
            max_cost_usd: None,
            ..EvolutionConfig::default()
        };
        let mut orch = EvolutionOrchestrator::new(
            archive,
            MutationEngine::new(mk_cap("llm.generate_diff")),
            FitnessEvaluator::new(),
            StagedVerificationGate::default(),
            config,
        );

        let dispatcher = happy_mock();
        let cases = vec![mk_case("a", json!({"ok": true}))];
        let (plan, journal) = mk_passed_plan();

        let outcomes = orch
            .run(
                &dispatcher,
                |_| "# parent".to_string(),
                &cases,
                &plan,
                &journal,
            )
            .await;

        assert_eq!(outcomes.len(), 4);
        assert!(outcomes
            .iter()
            .all(|o| matches!(o, StepOutcome::Archived { .. })));
    }

    /// Symbolic: simply exercise step() to prove the `#[tracing::instrument]`
    /// attribute compiled and the dynamic span fields are valid.
    #[tokio::test]
    async fn tracing_instrument_on_step_compiles() {
        let (mut orch, _skill, _seed) = mk_orchestrator_with_seed(0.0);
        let cases = vec![mk_case("a", json!({"ok": true}))];
        let dispatcher = happy_mock();
        let (plan, journal) = mk_passed_plan();

        let outcome = orch
            .step(
                &dispatcher,
                |_| "# parent".to_string(),
                &cases,
                &plan,
                &journal,
                None,
            )
            .await;
        assert!(matches!(outcome, StepOutcome::Archived { .. }));
    }

    // ------------------------------------------------------------------------
    // Sprint-5 additions: LLM-as-judge integration.
    // ------------------------------------------------------------------------

    use crate::fitness_evaluator::{LlmJudgeResponse, ScoringMode};
    use crate::production_evolution_dispatcher::EvolutionCapabilityDispatcher;
    use cyberclaw_connectors::types::{
        CapabilityExecutionRequest, CapabilityExecutionResult, ExecutionStatus,
    };
    use cyberclaw_core::ids::ExecutionId;

    /// Mock capability dispatcher used by the LLM-judge orchestrator tests.
    struct MockCapDispatcher {
        response: Mutex<Result<CapabilityExecutionResult, String>>,
        last_request: Mutex<Option<CapabilityExecutionRequest>>,
        call_count: Mutex<u32>,
    }

    impl MockCapDispatcher {
        fn new(response: Result<CapabilityExecutionResult, String>) -> Arc<Self> {
            Arc::new(Self {
                response: Mutex::new(response),
                last_request: Mutex::new(None),
                call_count: Mutex::new(0),
            })
        }
    }

    #[async_trait]
    impl EvolutionCapabilityDispatcher for MockCapDispatcher {
        async fn dispatch(
            &self,
            request: CapabilityExecutionRequest,
        ) -> anyhow::Result<CapabilityExecutionResult> {
            *self.call_count.lock().unwrap() += 1;
            *self.last_request.lock().unwrap() = Some(request);
            self.response
                .lock()
                .unwrap()
                .clone()
                .map_err(|e| anyhow::anyhow!(e))
        }
    }

    fn mk_judge_cap_ref() -> CapabilityRef {
        CapabilityRef {
            id: CapabilityId::from_string("llm.judge".to_string()).unwrap(),
            connector_id: ConnectorId::from_string("llm".to_string()).unwrap(),
            risk: RiskLevel::Low,
            effects: vec![CapabilityEffect::Read],
            placement: None,
        }
    }

    fn mk_judge_success(response: LlmJudgeResponse) -> CapabilityExecutionResult {
        CapabilityExecutionResult {
            execution_id: ExecutionId::new(),
            trace_id: "t".to_string(),
            connector_id: ConnectorId::from_string("llm".to_string()).unwrap(),
            capability_id: CapabilityId::from_string("llm.judge".to_string()).unwrap(),
            output: serde_json::to_value(response).unwrap(),
            status: ExecutionStatus::Success,
            error: None,
            actual_runtime: None,
        }
    }

    /// Build an orchestrator whose evaluator is configured for LlmJudge.
    fn mk_orchestrator_with_llm_judge(
        judge_capability: CapabilityRef,
        min_score: f32,
    ) -> (EvolutionOrchestrator, SkillId, SkillVariant) {
        let skill_id = SkillId::new();
        let mut archive = SkillArchive::new(skill_id.clone());
        let seed = mk_seed(skill_id.clone(), 0.5);
        let seed_clone = seed.clone();
        archive.add_variant(seed);

        let engine = MutationEngine::new(mk_cap("llm.generate_diff"));
        let mut evaluator = FitnessEvaluator::new();
        evaluator.default_scoring = ScoringMode::LlmJudge { judge_capability };
        let gate = StagedVerificationGate::default();
        let config = EvolutionConfig {
            min_score_to_archive: min_score,
            max_iterations: 3,
            ..EvolutionConfig::default()
        };
        (
            EvolutionOrchestrator::new(archive, engine, evaluator, gate, config),
            skill_id,
            seed_clone,
        )
    }

    #[tokio::test]
    async fn orchestrator_step_with_llm_judge_mode() {
        // End-to-end: orchestrator configured for LlmJudge + dispatcher
        // attached → step() routes scoring through the judge and archives
        // the variant with the judge-derived composite.
        let (mut orch, _skill, _seed) = mk_orchestrator_with_llm_judge(mk_judge_cap_ref(), 0.0);

        let judge_response = LlmJudgeResponse {
            correctness: 0.8,
            procedure_following: 0.7,
            conciseness: 0.9,
            feedback: "solid".to_string(),
        };
        let llm_dispatcher = MockCapDispatcher::new(Ok(mk_judge_success(judge_response)));
        orch.set_llm_judge_dispatcher(llm_dispatcher.clone());

        let cases = vec![mk_case("a", json!({"ok": true}))];
        let mut outputs = HashMap::new();
        outputs.insert(
            "a".to_string(),
            Ok(mk_outcome("a", json!({"ok": true}), true)),
        );
        let evo_dispatcher = MockDispatcher::new(
            Ok(mk_mutation_result("# improved")),
            outputs,
            Ok(mk_smoke_pass()),
        );
        let (plan, journal) = mk_passed_plan();

        let outcome = orch
            .step(
                &evo_dispatcher,
                |_| "# parent".to_string(),
                &cases,
                &plan,
                &journal,
                None,
            )
            .await;

        // Expected composite with judge scores (no length penalty):
        //   0.5*0.8 + 0.3*0.7 + 0.2*0.9 = 0.79
        match outcome {
            StepOutcome::Archived {
                score, breakdown, ..
            } => {
                assert!(
                    (score - 0.79).abs() < 1e-4,
                    "expected composite ~0.79 from judge scores, got {}",
                    score
                );
                assert!((breakdown.correctness - 0.8).abs() < 1e-6);
                assert!((breakdown.procedure_following - 0.7).abs() < 1e-6);
                assert!((breakdown.conciseness - 0.9).abs() < 1e-6);
            }
            other => panic!("expected Archived via LLM judge, got {:?}", other),
        }
        // Judge capability must have been dispatched exactly once.
        assert_eq!(*llm_dispatcher.call_count.lock().unwrap(), 1);
        let req = llm_dispatcher
            .last_request
            .lock()
            .unwrap()
            .clone()
            .expect("req");
        assert_eq!(req.capability_id.as_str(), "llm.judge");
    }

    #[tokio::test]
    async fn orchestrator_step_falls_back_to_sync_when_no_llm_judge() {
        // Even when configured for LlmJudge, if no dispatcher is attached the
        // orchestrator must fall back to the synchronous scorer (heuristic
        // for LlmJudge) and NOT panic or fail the step.
        let (mut orch, _skill, _seed) = mk_orchestrator_with_llm_judge(mk_judge_cap_ref(), 0.0);
        // Deliberately do NOT attach an llm_judge_dispatcher.

        let cases = vec![mk_case("a", json!({"ok": true}))];
        let mut outputs = HashMap::new();
        outputs.insert(
            "a".to_string(),
            Ok(mk_outcome("a", json!({"ok": true}), true)),
        );
        let evo_dispatcher = MockDispatcher::new(
            Ok(mk_mutation_result("# improved")),
            outputs,
            Ok(mk_smoke_pass()),
        );
        let (plan, journal) = mk_passed_plan();

        let outcome = orch
            .step(
                &evo_dispatcher,
                |_| "# parent".to_string(),
                &cases,
                &plan,
                &journal,
                None,
            )
            .await;

        // Fallback path archives under the heuristic scorer.
        assert!(
            matches!(outcome, StepOutcome::Archived { .. }),
            "expected Archived via sync fallback, got {:?}",
            outcome
        );
    }

    #[tokio::test]
    async fn with_event_sink_chained_constructor() {
        let skill_id = SkillId::new();
        let mut archive = SkillArchive::new(skill_id.clone());
        archive.add_variant(mk_seed(skill_id, 0.5));
        let sink = Arc::new(VecEventSink::new());

        let mut orch = EvolutionOrchestrator::new(
            archive,
            MutationEngine::new(mk_cap("llm.generate_diff")),
            FitnessEvaluator::new(),
            StagedVerificationGate::default(),
            EvolutionConfig {
                min_score_to_archive: 0.0,
                max_iterations: 1,
                ..EvolutionConfig::default()
            },
        )
        .with_event_sink(sink.clone());

        let dispatcher = happy_mock();
        let cases = vec![mk_case("a", json!({"ok": true}))];
        let (plan, journal) = mk_passed_plan();

        let _ = orch
            .step(
                &dispatcher,
                |_| "# parent".to_string(),
                &cases,
                &plan,
                &journal,
                None,
            )
            .await;

        assert!(!sink.is_empty());
    }
}
