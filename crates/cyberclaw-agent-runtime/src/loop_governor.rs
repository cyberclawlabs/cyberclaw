//! # AgenticLoopGovernor — unified 4-gate runaway protection
//!
//! Sprint R4 (v1.x optimization). Closes the coordination gap between
//! `IterationBudget`, `CircuitBreaker`, and `ContextCompressor`: previously
//! each component decided independently, so e.g. a single prompt could run
//! 1637 s and time out without any of them stopping it.
//!
//! The governor wraps all three legacy primitives and adds a wall-clock
//! deadline + repetition detector + token-cost accountant. Every iteration
//! the loop should call `pre_iteration(...)` to obtain a [`LoopDecision`]:
//!
//! ```text
//!   Continue              -> proceed with the next LLM call
//!   CompressAndContinue   -> run ContextCompressor, then proceed
//!   HardStop(reason)      -> fail the loop with reason as user-facing message
//! ```
//!
//! ## The four gates (evaluated in priority order)
//!
//! 1. **Wall-clock gate** — `wall_clock_deadline` is the absolute `Instant`
//!    by which the loop must finish. Default: 60 s (L1) / 180 s (L2) /
//!    240 s (L3). Trips first because runaway prompts are the most damaging
//!    failure mode (G-L3 1637 s outlier).
//! 2. **Token-budget gate** — accumulated `tokens_consumed_total` (input +
//!    output) above `max_total_tokens` triggers HardStop. Independent from
//!    IterationBudget's per-iteration `max_tokens` so it survives state
//!    carry-over between turns.
//! 3. **Repetition gate** — if the last `repetition_window` iteration
//!    results are too similar (Jaccard-on-token-shingles ≥
//!    `repetition_similarity_threshold`), declare death-loop and HardStop.
//!    Complements the existing `StuckDetector` which only sees identical
//!    *tool calls*; this one also catches identical *text* output.
//! 4. **Compression gate** — if context length crosses
//!    `compress_threshold_chars`, return `CompressAndContinue` so the
//!    caller drives the compressor without ad-hoc thresholds scattered
//!    across the loop.
//!
//! `IterationBudget` (max_iterations) is still consulted by the underlying
//! loop, and the governor does **not** replace it — it is an additional
//! safety layer.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::agentic_loop::{IterationBudget, IterationResult, LoopState};
use crate::context_compressor::{CompressionConfig, ContextCompressor};

// ---------------------------------------------------------------------------
// Governor preset profiles
// ---------------------------------------------------------------------------

/// Difficulty / depth class of the prompt being executed. Picks default
/// wall-clock and token budgets that match the baseline ladder:
///
/// - `L1` — short single-turn QA (default 60 s)
/// - `L2` — multi-step reasoning, light tool use (default 180 s; was 120 s
///   pre-GAP-4 fix — code generation needed more headroom)
/// - `L3` — complex multi-turn with heavy tool calls (default 240 s)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopProfile {
    /// Short prompts, default 60 s wall-clock.
    L1,
    /// Medium prompts, default 180 s wall-clock (raised from 120 s in
    /// GAP-4 fix; F-L2 code generation tasks needed headroom).
    L2,
    /// Complex prompts, default 240 s wall-clock.
    L3,
}

impl LoopProfile {
    /// Default wall-clock deadline for this profile.
    ///
    /// GAP-4 fix (2026-05-22 v1.x business matrix): F-L2 code generation
    /// observed to exceed the previous 120 s ceiling and trip the
    /// wall-clock gate before the loop could finalize. Raised to 180 s
    /// so realistic two-stage code-gen prompts complete; L1/L3 unchanged.
    pub fn default_wall_clock(&self) -> Duration {
        match self {
            LoopProfile::L1 => Duration::from_secs(60),
            LoopProfile::L2 => Duration::from_secs(180),
            LoopProfile::L3 => Duration::from_secs(240),
        }
    }

    /// Default total token budget for this profile.
    pub fn default_token_budget(&self) -> u64 {
        match self {
            LoopProfile::L1 => 8_000,
            LoopProfile::L2 => 32_000,
            LoopProfile::L3 => 128_000,
        }
    }
}

// ---------------------------------------------------------------------------
// GovernorConfig
// ---------------------------------------------------------------------------

/// Configuration for [`AgenticLoopGovernor`].
///
/// All thresholds are hard-cap (≥ means HardStop). To disable a gate, set the
/// field to 0 / `u64::MAX` / `f64::INFINITY` as documented per field.
#[derive(Debug, Clone)]
pub struct GovernorConfig {
    /// Wall-clock budget for the **entire** loop run. Trips gate 1.
    /// Default: profile-dependent (60/120/240 s).
    pub wall_clock_budget: Duration,
    /// Maximum cumulative tokens (input + output) across all iterations.
    /// Trips gate 2. Default: profile-dependent (8k/32k/128k).
    /// `0` means unlimited.
    pub max_total_tokens: u64,
    /// How many recent iteration text-outputs to compare for repetition
    /// detection. Default: `3`. `0` disables gate 3.
    pub repetition_window: usize,
    /// Jaccard similarity threshold above which `repetition_window`
    /// consecutive outputs count as a death-loop. Default: `0.90`.
    /// Set to `f64::INFINITY` to disable gate 3.
    pub repetition_similarity_threshold: f64,
    /// Total context length in characters above which the governor
    /// emits [`LoopDecision::CompressAndContinue`]. Default: `40_000`
    /// (≈ 10k tokens). `0` disables gate 4.
    pub compress_threshold_chars: usize,
    /// Underlying compressor configuration. Forwarded into the embedded
    /// [`ContextCompressor`].
    pub compression: CompressionConfig,
}

impl Default for GovernorConfig {
    fn default() -> Self {
        Self::from_profile(LoopProfile::L2)
    }
}

impl GovernorConfig {
    /// Build a config from a `LoopProfile` preset.
    pub fn from_profile(profile: LoopProfile) -> Self {
        Self {
            wall_clock_budget: profile.default_wall_clock(),
            max_total_tokens: profile.default_token_budget(),
            repetition_window: 3,
            repetition_similarity_threshold: 0.90,
            compress_threshold_chars: 40_000,
            compression: CompressionConfig::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// CostTracker
// ---------------------------------------------------------------------------

/// Tracks cumulative token usage across all iterations of a loop run.
///
/// Distinct from `LoopState.tokens_consumed` which is the inner loop's view
/// — the governor needs an independent counter so it can survive
/// `LoopState::new()` resets between turns of a multi-turn conversation.
#[derive(Debug, Default, Clone)]
pub struct CostTracker {
    /// Cumulative tokens consumed.
    pub tokens_consumed_total: u64,
    /// Number of iterations recorded.
    pub iterations_observed: u32,
}

impl CostTracker {
    /// Create a fresh tracker with zero counters.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one iteration's token delta.
    pub fn record(&mut self, tokens: u64) {
        self.tokens_consumed_total = self.tokens_consumed_total.saturating_add(tokens);
        self.iterations_observed = self.iterations_observed.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// LoopCtx
// ---------------------------------------------------------------------------

/// Mutable per-iteration context handed to the governor.
///
/// The governor inspects `state` and the embedded [`IterationBudget`] read-
/// only; it never mutates them. Callers should populate `state` with the
/// **current** values before calling `pre_iteration`.
#[derive(Debug)]
pub struct LoopCtx<'a> {
    /// Snapshot of the inner-loop state. The governor reads
    /// `messages` (for length) and `iteration_count` only.
    pub state: &'a LoopState,
    /// The inner-loop iteration budget. Used to delegate the
    /// underlying compressor's `should_compress` check.
    pub budget: &'a IterationBudget,
}

// ---------------------------------------------------------------------------
// LoopDecision
// ---------------------------------------------------------------------------

/// Outcome of a single governor evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopDecision {
    /// All gates green — proceed with the next LLM call.
    Continue,
    /// Context length crossed the compression threshold — caller should
    /// run [`ContextCompressor::compress_all`] (or stage-by-stage) before
    /// the next LLM call.
    CompressAndContinue,
    /// A hard-stop gate fired. The included `&'static str` is the
    /// user-facing reason that the loop should surface via
    /// `IterationResult::BudgetExhausted(reason)` or equivalent.
    HardStop(&'static str),
}

// ---------------------------------------------------------------------------
// AgenticLoopGovernor
// ---------------------------------------------------------------------------

/// Unified 4-gate runaway protection for agentic loops.
///
/// See the module-level docs for the gate semantics.
#[derive(Debug)]
pub struct AgenticLoopGovernor {
    config: GovernorConfig,
    iteration_budget: IterationBudget,
    compressor: ContextCompressor,
    cost_tracker: CostTracker,
    recent_outputs: VecDeque<String>,
    started_at: Instant,
    wall_clock_deadline: Instant,
    /// True once any HardStop gate has fired. Used so subsequent
    /// `pre_iteration` calls keep returning HardStop instead of flipping
    /// back to Continue.
    tripped: Option<&'static str>,
}

impl AgenticLoopGovernor {
    /// Create a governor with the given configuration. The wall-clock
    /// deadline is measured from "now" — call this immediately before
    /// the first iteration.
    pub fn new(config: GovernorConfig) -> Self {
        let started_at = Instant::now();
        let wall_clock_deadline = started_at + config.wall_clock_budget;
        let compressor = ContextCompressor::new(config.compression.clone());
        Self {
            config,
            iteration_budget: IterationBudget::default(),
            compressor,
            cost_tracker: CostTracker::new(),
            recent_outputs: VecDeque::new(),
            started_at,
            wall_clock_deadline,
            tripped: None,
        }
    }

    /// Create a governor from a [`LoopProfile`] preset.
    pub fn from_profile(profile: LoopProfile) -> Self {
        Self::new(GovernorConfig::from_profile(profile))
    }

    /// Set the underlying [`IterationBudget`] used when consulting the
    /// embedded compressor's `should_compress`.
    pub fn with_iteration_budget(mut self, budget: IterationBudget) -> Self {
        self.iteration_budget = budget;
        self
    }

    /// Read-only access to the governor's config.
    pub fn config(&self) -> &GovernorConfig {
        &self.config
    }

    /// Read-only access to the embedded cost tracker.
    pub fn cost_tracker(&self) -> &CostTracker {
        &self.cost_tracker
    }

    /// Read-only access to the embedded context compressor. Callers may
    /// invoke `compressor.compress_all(...)` when the governor returns
    /// `CompressAndContinue`.
    pub fn compressor_mut(&mut self) -> &mut ContextCompressor {
        &mut self.compressor
    }

    /// Read-only access to the embedded context compressor.
    pub fn compressor(&self) -> &ContextCompressor {
        &self.compressor
    }

    /// How long the loop has been running.
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Absolute wall-clock deadline.
    pub fn deadline(&self) -> Instant {
        self.wall_clock_deadline
    }

    /// Evaluate the four gates against the current loop state.
    ///
    /// Returns one of [`LoopDecision`]. The governor is sticky: once any
    /// HardStop has fired, subsequent calls return the same HardStop.
    pub fn pre_iteration(&mut self, ctx: &LoopCtx<'_>) -> LoopDecision {
        if let Some(reason) = self.tripped {
            return LoopDecision::HardStop(reason);
        }

        // Gate 1 — wall clock.
        if Instant::now() >= self.wall_clock_deadline {
            return self.trip("wall-clock budget exhausted");
        }

        // Gate 2 — total token budget.
        if self.config.max_total_tokens > 0
            && self.cost_tracker.tokens_consumed_total >= self.config.max_total_tokens
        {
            return self.trip("total token budget exhausted");
        }

        // Gate 3 — repetition / death-loop.
        if self.is_repetition_loop() {
            return self.trip("repetition death-loop detected");
        }

        // Gate 4 — compression threshold.
        if self.config.compress_threshold_chars > 0 {
            let total_chars: usize = ctx.state.messages.iter().map(|m| m.content.len()).sum();
            if total_chars >= self.config.compress_threshold_chars {
                return LoopDecision::CompressAndContinue;
            }
        }

        // Honour the underlying compressor's own threshold-vs-token check.
        if self.compressor.should_compress(ctx.state, ctx.budget) {
            return LoopDecision::CompressAndContinue;
        }

        LoopDecision::Continue
    }

    /// Record metadata about an iteration that just finished. Used by the
    /// repetition detector and the cost tracker. Must be called after
    /// `pre_iteration -> Continue` regardless of whether the iteration
    /// ended in tool-call, text response, or error.
    pub fn record_iteration_result(&mut self, _ctx: &LoopCtx<'_>, result: &IterationResult) {
        // Capture latest text output for the repetition window.
        let text_snapshot = match result {
            IterationResult::Done(s)
            | IterationResult::TextResponse(s)
            | IterationResult::Stuck(s)
            | IterationResult::BudgetExhausted(s) => Some(s.clone()),
            // Capture tool-call signature so a repeating "call X, ignore"
            // pattern still trips gate 3 even before StuckDetector fires.
            IterationResult::ToolCalls(calls) => {
                let mut parts: Vec<String> = calls
                    .iter()
                    .map(|c| format!("{}:{}", c.function.name, c.function.arguments))
                    .collect();
                parts.sort();
                Some(parts.join("|"))
            }
            IterationResult::Continue => None,
        };

        if let Some(s) = text_snapshot {
            if self.config.repetition_window > 0 {
                self.recent_outputs.push_back(s);
                while self.recent_outputs.len() > self.config.repetition_window {
                    self.recent_outputs.pop_front();
                }
            }
        }
    }

    /// Manually account for tokens consumed by an iteration. Call this
    /// after the LLM client returns usage statistics so gate 2 reflects
    /// real spend.
    pub fn record_tokens(&mut self, delta_tokens: u64) {
        self.cost_tracker.record(delta_tokens);
    }

    /// Has any HardStop gate fired yet?
    pub fn is_tripped(&self) -> bool {
        self.tripped.is_some()
    }

    /// The HardStop reason if any gate has fired.
    pub fn tripped_reason(&self) -> Option<&'static str> {
        self.tripped
    }

    // -- internals ----------------------------------------------------------

    fn trip(&mut self, reason: &'static str) -> LoopDecision {
        self.tripped = Some(reason);
        LoopDecision::HardStop(reason)
    }

    /// True when the last `repetition_window` recorded outputs are pairwise
    /// similar above `repetition_similarity_threshold` (Jaccard on
    /// whitespace-split token sets).
    fn is_repetition_loop(&self) -> bool {
        let window = self.config.repetition_window;
        if window < 2 {
            return false;
        }
        if self.recent_outputs.len() < window {
            return false;
        }
        if !self.config.repetition_similarity_threshold.is_finite() {
            return false;
        }

        // Compare every adjacent pair in the window. If *all* pairs exceed
        // the threshold, we declare death loop.
        let outputs: Vec<&String> = self.recent_outputs.iter().collect();
        for pair in outputs.windows(2) {
            let sim = jaccard_token_similarity(pair[0], pair[1]);
            if sim < self.config.repetition_similarity_threshold {
                return false;
            }
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Jaccard similarity helper
// ---------------------------------------------------------------------------

/// Whitespace-tokenised Jaccard similarity in `[0.0, 1.0]`. Returns `1.0`
/// when both strings are empty (vacuously equal), `0.0` when only one is.
fn jaccard_token_similarity(a: &str, b: &str) -> f64 {
    use std::collections::HashSet;

    let tokens_a: HashSet<&str> = a.split_whitespace().collect();
    let tokens_b: HashSet<&str> = b.split_whitespace().collect();

    if tokens_a.is_empty() && tokens_b.is_empty() {
        return 1.0;
    }
    if tokens_a.is_empty() || tokens_b.is_empty() {
        return 0.0;
    }

    let intersection = tokens_a.intersection(&tokens_b).count();
    let union = tokens_a.union(&tokens_b).count();
    if union == 0 {
        return 0.0;
    }
    intersection as f64 / union as f64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agentic_loop::{IterationBudget, IterationResult, LoopState};
    use cyberclaw_llm::types::{FunctionCall, Message, ToolCall};

    fn empty_state() -> LoopState {
        LoopState {
            messages: vec![],
            iteration_count: 0,
            tokens_consumed: 0,
            ..Default::default()
        }
    }

    fn state_with_messages(msgs: Vec<Message>) -> LoopState {
        LoopState {
            messages: msgs,
            iteration_count: 0,
            tokens_consumed: 0,
            ..Default::default()
        }
    }

    fn ctx<'a>(state: &'a LoopState, budget: &'a IterationBudget) -> LoopCtx<'a> {
        LoopCtx { state, budget }
    }

    // -- Gate 1: wall clock -------------------------------------------------

    #[test]
    fn gate1_wall_clock_trips_when_deadline_passed() {
        let mut gov = AgenticLoopGovernor::new(GovernorConfig {
            wall_clock_budget: Duration::from_millis(1),
            ..Default::default()
        });
        std::thread::sleep(Duration::from_millis(20));
        let st = empty_state();
        let b = IterationBudget::default();
        let dec = gov.pre_iteration(&ctx(&st, &b));
        assert!(matches!(dec, LoopDecision::HardStop(r) if r.contains("wall-clock")));
        assert!(gov.is_tripped());
    }

    #[test]
    fn gate1_wall_clock_passes_when_within_budget() {
        let mut gov = AgenticLoopGovernor::new(GovernorConfig {
            wall_clock_budget: Duration::from_secs(60),
            compress_threshold_chars: 0,
            max_total_tokens: 0,
            repetition_window: 0,
            ..Default::default()
        });
        let st = empty_state();
        let b = IterationBudget::default();
        assert_eq!(gov.pre_iteration(&ctx(&st, &b)), LoopDecision::Continue);
    }

    #[test]
    fn gate1_sticky_after_trip() {
        let mut gov = AgenticLoopGovernor::new(GovernorConfig {
            wall_clock_budget: Duration::from_millis(1),
            ..Default::default()
        });
        std::thread::sleep(Duration::from_millis(20));
        let st = empty_state();
        let b = IterationBudget::default();
        let _ = gov.pre_iteration(&ctx(&st, &b));
        // Second call must still return HardStop.
        let dec = gov.pre_iteration(&ctx(&st, &b));
        assert!(matches!(dec, LoopDecision::HardStop(_)));
    }

    // -- Gate 2: token budget ----------------------------------------------

    #[test]
    fn gate2_token_budget_trips_above_limit() {
        let mut gov = AgenticLoopGovernor::new(GovernorConfig {
            wall_clock_budget: Duration::from_secs(60),
            max_total_tokens: 100,
            compress_threshold_chars: 0,
            repetition_window: 0,
            ..Default::default()
        });
        gov.record_tokens(150);
        let st = empty_state();
        let b = IterationBudget::default();
        let dec = gov.pre_iteration(&ctx(&st, &b));
        assert!(matches!(dec, LoopDecision::HardStop(r) if r.contains("token")));
    }

    #[test]
    fn gate2_token_budget_passes_below_limit() {
        let mut gov = AgenticLoopGovernor::new(GovernorConfig {
            wall_clock_budget: Duration::from_secs(60),
            max_total_tokens: 100,
            compress_threshold_chars: 0,
            repetition_window: 0,
            ..Default::default()
        });
        gov.record_tokens(50);
        let st = empty_state();
        let b = IterationBudget::default();
        assert_eq!(gov.pre_iteration(&ctx(&st, &b)), LoopDecision::Continue);
    }

    #[test]
    fn gate2_disabled_when_max_tokens_zero() {
        let mut gov = AgenticLoopGovernor::new(GovernorConfig {
            wall_clock_budget: Duration::from_secs(60),
            max_total_tokens: 0,
            compress_threshold_chars: 0,
            repetition_window: 0,
            ..Default::default()
        });
        gov.record_tokens(999_999_999);
        let st = empty_state();
        let b = IterationBudget::default();
        assert_eq!(gov.pre_iteration(&ctx(&st, &b)), LoopDecision::Continue);
    }

    // -- Gate 3: repetition / death loop -----------------------------------

    #[test]
    fn gate3_repetition_trips_on_three_identical_outputs() {
        let mut gov = AgenticLoopGovernor::new(GovernorConfig {
            wall_clock_budget: Duration::from_secs(60),
            max_total_tokens: 0,
            compress_threshold_chars: 0,
            repetition_window: 3,
            repetition_similarity_threshold: 0.9,
            ..Default::default()
        });
        let st = empty_state();
        let b = IterationBudget::default();
        let c = ctx(&st, &b);

        let r = IterationResult::TextResponse("the same exact sentence here".into());
        gov.record_iteration_result(&c, &r);
        gov.record_iteration_result(&c, &r);
        gov.record_iteration_result(&c, &r);

        let dec = gov.pre_iteration(&c);
        assert!(matches!(dec, LoopDecision::HardStop(r) if r.contains("repetition")));
    }

    #[test]
    fn gate3_repetition_no_trip_on_diverse_outputs() {
        let mut gov = AgenticLoopGovernor::new(GovernorConfig {
            wall_clock_budget: Duration::from_secs(60),
            max_total_tokens: 0,
            compress_threshold_chars: 0,
            repetition_window: 3,
            repetition_similarity_threshold: 0.9,
            ..Default::default()
        });
        let st = empty_state();
        let b = IterationBudget::default();
        let c = ctx(&st, &b);

        gov.record_iteration_result(
            &c,
            &IterationResult::TextResponse("alpha bravo charlie".into()),
        );
        gov.record_iteration_result(
            &c,
            &IterationResult::TextResponse("delta echo foxtrot".into()),
        );
        gov.record_iteration_result(
            &c,
            &IterationResult::TextResponse("golf hotel india".into()),
        );

        assert_eq!(gov.pre_iteration(&c), LoopDecision::Continue);
    }

    #[test]
    fn gate3_repetition_disabled_when_window_zero() {
        let mut gov = AgenticLoopGovernor::new(GovernorConfig {
            wall_clock_budget: Duration::from_secs(60),
            max_total_tokens: 0,
            compress_threshold_chars: 0,
            repetition_window: 0,
            ..Default::default()
        });
        let st = empty_state();
        let b = IterationBudget::default();
        let c = ctx(&st, &b);
        let r = IterationResult::TextResponse("xxxxxx".into());
        gov.record_iteration_result(&c, &r);
        gov.record_iteration_result(&c, &r);
        gov.record_iteration_result(&c, &r);
        assert_eq!(gov.pre_iteration(&c), LoopDecision::Continue);
    }

    #[test]
    fn gate3_repetition_catches_identical_tool_calls() {
        let mut gov = AgenticLoopGovernor::new(GovernorConfig {
            wall_clock_budget: Duration::from_secs(60),
            max_total_tokens: 0,
            compress_threshold_chars: 0,
            repetition_window: 3,
            repetition_similarity_threshold: 0.9,
            ..Default::default()
        });
        let st = empty_state();
        let b = IterationBudget::default();
        let c = ctx(&st, &b);

        let same_call = vec![ToolCall {
            id: "x".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "fs.read".into(),
                arguments: r#"{"path":"/a"}"#.into(),
            },
        }];
        let r = IterationResult::ToolCalls(same_call);
        gov.record_iteration_result(&c, &r);
        gov.record_iteration_result(&c, &r);
        gov.record_iteration_result(&c, &r);

        assert!(matches!(
            gov.pre_iteration(&c),
            LoopDecision::HardStop(reason) if reason.contains("repetition")
        ));
    }

    // -- Gate 4: compress threshold ----------------------------------------

    #[test]
    fn gate4_compress_triggers_when_chars_exceed_threshold() {
        let mut gov = AgenticLoopGovernor::new(GovernorConfig {
            wall_clock_budget: Duration::from_secs(60),
            max_total_tokens: 0,
            compress_threshold_chars: 100,
            repetition_window: 0,
            ..Default::default()
        });
        let big = "x".repeat(200);
        let st = state_with_messages(vec![Message::user(big)]);
        let b = IterationBudget::default();
        assert_eq!(
            gov.pre_iteration(&ctx(&st, &b)),
            LoopDecision::CompressAndContinue
        );
    }

    #[test]
    fn gate4_no_compress_below_threshold() {
        let mut gov = AgenticLoopGovernor::new(GovernorConfig {
            wall_clock_budget: Duration::from_secs(60),
            max_total_tokens: 0,
            compress_threshold_chars: 1_000,
            repetition_window: 0,
            ..Default::default()
        });
        let st = state_with_messages(vec![Message::user("short")]);
        let b = IterationBudget::default();
        assert_eq!(gov.pre_iteration(&ctx(&st, &b)), LoopDecision::Continue);
    }

    // -- Integration: profile defaults --------------------------------------

    #[test]
    fn profile_l1_l2_l3_have_increasing_budgets() {
        let l1 = LoopProfile::L1;
        let l2 = LoopProfile::L2;
        let l3 = LoopProfile::L3;
        assert!(l1.default_wall_clock() < l2.default_wall_clock());
        assert!(l2.default_wall_clock() < l3.default_wall_clock());
        assert!(l1.default_token_budget() < l2.default_token_budget());
        assert!(l2.default_token_budget() < l3.default_token_budget());
    }

    #[test]
    fn governor_from_profile_uses_preset() {
        let gov = AgenticLoopGovernor::from_profile(LoopProfile::L3);
        assert_eq!(gov.config().wall_clock_budget, Duration::from_secs(240));
        assert_eq!(gov.config().max_total_tokens, 128_000);
    }

    // -- Integration: realistic runaway scenarios --------------------------

    /// Simulates the G-L3 outlier: 50 iterations, each one inflating
    /// token spend by 3 000 tokens. Gate 2 should trip well before the
    /// runaway completes.
    #[test]
    fn integration_long_running_prompt_trips_token_gate_first() {
        let mut gov = AgenticLoopGovernor::new(GovernorConfig {
            wall_clock_budget: Duration::from_secs(600),
            max_total_tokens: 10_000,
            compress_threshold_chars: 0,
            repetition_window: 0,
            ..Default::default()
        });
        let st = empty_state();
        let b = IterationBudget::default();
        let c = ctx(&st, &b);

        let mut tripped_at: Option<u32> = None;
        for i in 0..50 {
            match gov.pre_iteration(&c) {
                LoopDecision::HardStop(_) => {
                    tripped_at = Some(i);
                    break;
                }
                _ => gov.record_tokens(3_000),
            }
        }
        let i = tripped_at.expect("token gate must trip");
        assert!(
            i <= 5,
            "should trip within first few iterations, tripped at i={i}"
        );
    }

    #[test]
    fn integration_compress_continue_does_not_trip_hardstop() {
        let mut gov = AgenticLoopGovernor::new(GovernorConfig {
            wall_clock_budget: Duration::from_secs(60),
            max_total_tokens: 0,
            compress_threshold_chars: 50,
            repetition_window: 0,
            ..Default::default()
        });
        let st = state_with_messages(vec![Message::user("x".repeat(200))]);
        let b = IterationBudget::default();
        let c = ctx(&st, &b);

        assert_eq!(gov.pre_iteration(&c), LoopDecision::CompressAndContinue);
        // CompressAndContinue is NOT a HardStop — calling again still returns it.
        assert_eq!(gov.pre_iteration(&c), LoopDecision::CompressAndContinue);
        assert!(!gov.is_tripped());
    }

    // -- CostTracker --------------------------------------------------------

    #[test]
    fn cost_tracker_records_cumulative_tokens() {
        let mut t = CostTracker::new();
        t.record(100);
        t.record(200);
        assert_eq!(t.tokens_consumed_total, 300);
        assert_eq!(t.iterations_observed, 2);
    }

    #[test]
    fn cost_tracker_saturates_on_overflow() {
        let mut t = CostTracker::new();
        t.record(u64::MAX);
        t.record(1);
        assert_eq!(t.tokens_consumed_total, u64::MAX);
    }

    // -- Jaccard helper -----------------------------------------------------

    #[test]
    fn jaccard_both_empty_returns_one() {
        assert!((jaccard_token_similarity("", "") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn jaccard_one_empty_returns_zero() {
        assert!((jaccard_token_similarity("abc", "") - 0.0).abs() < f64::EPSILON);
        assert!((jaccard_token_similarity("", "abc") - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn jaccard_identical_strings_returns_one() {
        let s = "the quick brown fox";
        assert!((jaccard_token_similarity(s, s) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn jaccard_disjoint_strings_returns_zero() {
        assert_eq!(jaccard_token_similarity("a b c", "d e f"), 0.0);
    }

    #[test]
    fn jaccard_partial_overlap_returns_fraction() {
        // {a,b,c} ∩ {b,c,d} = 2, ∪ = 4 → 0.5
        let sim = jaccard_token_similarity("a b c", "b c d");
        assert!((sim - 0.5).abs() < 0.0001);
    }

    // -- GAP-4 regression guard: L2 wall-clock must be 180 s, not 120 s -----

    #[test]
    fn l2_wall_clock_is_180s_not_120s() {
        // GAP-4 (2026-05-22): F-L2 code generation observed to need
        // >120 s wall-clock. If a future change drops L2 back to 120 s
        // without explicit ADR, this assertion catches it before the
        // business matrix regresses.
        assert_eq!(
            LoopProfile::L2.default_wall_clock(),
            Duration::from_secs(180),
            "L2 wall-clock must remain 180 s (GAP-4 fix)"
        );

        let cfg = GovernorConfig::from_profile(LoopProfile::L2);
        assert_eq!(cfg.wall_clock_budget, Duration::from_secs(180));
    }
}
