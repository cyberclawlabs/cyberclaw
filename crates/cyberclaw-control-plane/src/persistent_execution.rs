//! Persistent Execution Engine
//!
//! Implements a story-driven persistent execution loop inspired by PRD-driven
//! task completion patterns. This module provides structured execution with:
//!
//! - **Story-based planning**: Tasks decomposed into discrete stories with
//!   testable acceptance criteria
//! - **Persistent iteration**: Loop until all stories pass, with cross-iteration
//!   learning
//! - **Verification gates**: Reviewer-based completion gates against specific
//!   criteria
//! - **Progress journaling**: Records learnings across iterations for retry
//!   improvement
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────┐     pick_next      ┌──────────────┐
//! │ ExecutionPlan │ ─────────────────> │    Story     │
//! │  (stories)   │                    │  (criteria)  │
//! └──────┬───────┘                    └──────┬───────┘
//!        │                                   │
//!        │  all_complete?                    │ execute + verify
//!        ▼                                   ▼
//! ┌──────────────┐                    ┌──────────────┐
//! │ Verification │ <───── pass ────── │ StoryResult  │
//! │    Gate      │                    │  (evidence)  │
//! └──────────────┘                    └──────────────┘
//!        │
//!        ▼
//! ┌──────────────┐
//! │   Journal    │
//! │  (learnings) │
//! └──────────────┘
//! ```
//!
//! # Integration
//!
//! This module integrates with the existing Autopilot system:
//! - Uses `ExecutionId` for tracking
//! - Leverages `SharedStateStore` for persistence
//! - Feeds into `ProvenanceTracker` for audit trails
//!
//! # Example
//!
//! ```rust
//! use cyberclaw_control_plane::persistent_execution::*;
//!
//! // Create a plan with stories
//! let mut plan = ExecutionPlan::new("Deploy microservice");
//! plan.add_story(Story::new(
//!     "US-001",
//!     "Build Docker image",
//!     vec![
//!         AcceptanceCriterion::new("Dockerfile exists at ./Dockerfile"),
//!         AcceptanceCriterion::new("docker build succeeds with exit code 0"),
//!     ],
//! ));
//!
//! // Create the persistent loop
//! let mut loop_runner = PersistentLoop::new(plan, LoopConfig::default());
//! assert_eq!(loop_runner.pending_count(), 1);
//! ```

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cyberclaw_core::execution::ExecutionContext;
use cyberclaw_core::ids::{CapabilityId, ConnectorId, SkillId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

// ============================================================================
// Verifier Kind
// ============================================================================

/// Sprint D1: Rust type-driven verifier specification for an
/// [`AcceptanceCriterion`].
///
/// Each variant declares a verification strategy that the (Sprint D4) verifier
/// implementation will execute when judging whether a criterion is met. The
/// enum is **declaration-only** at this sprint — no executor lives here; the
/// type drives downstream dispatch.
///
/// Backward compatibility is preserved by attaching `VerifierKind` to a new
/// `verifier: Option<VerifierKind>` field on [`AcceptanceCriterion`] with
/// `#[serde(default)]`, so existing serialized criteria continue to decode.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum VerifierKind {
    /// File at `path` must exist on disk.
    FileExists { path: String },
    /// File at `path` must have a MIME type matching `expected`.
    FileMimeMatches { path: String, expected: String },
    /// File at `path` must exist and have non-zero size.
    FileNonEmpty { path: String },
    /// Audio file at `path` must have a duration greater than `min_secs`.
    /// Implementation will shell out to `ffprobe` in Sprint D4.
    AudioDurationGt { path: String, min_secs: f32 },
    /// Text file at `path` must be present and contain non-whitespace content.
    TextNonEmpty { path: String },
    /// Numeric aggregate over a CSV column must satisfy `formula` within
    /// `tolerance`. Reuses the `verify.numeric_aggregate` capability landed in
    /// commit aabcf34.
    NumericMatchesCsv {
        csv_path: String,
        formula: String,
        tolerance: f32,
    },
    /// Run `script` with `args` in `cwd`; verification passes iff exit code 0.
    /// Used for `pytest`, `cargo test`, etc.
    ScriptExitZero {
        script: String,
        args: Vec<String>,
        cwd: String,
    },
    /// LLM-driven semantic match between `reference` and the contents of
    /// `actual_path`; passes when similarity ≥ `threshold`.
    LlmSemanticMatch {
        reference: String,
        actual_path: String,
        threshold: f32,
    },
}

// ============================================================================
// Acceptance Criterion
// ============================================================================

/// A single testable acceptance criterion for a story.
///
/// Each criterion has a description and an optional evidence field that records
/// how verification was performed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptanceCriterion {
    /// Human-readable description of what must be true.
    pub description: String,
    /// Whether this criterion has been verified as passing.
    pub met: bool,
    /// Evidence of verification (e.g., test output, command result).
    pub evidence: Option<String>,
    /// When the criterion was verified.
    pub verified_at: Option<DateTime<Utc>>,
    /// Sprint D1: optional Rust type-driven verifier specification.
    ///
    /// When `Some(...)`, downstream Sprint D4 verifier dispatch consults this
    /// kind to decide how to evaluate the criterion. `#[serde(default)]` keeps
    /// existing serialized payloads decoding cleanly with `verifier = None`.
    #[serde(default)]
    pub verifier: Option<VerifierKind>,
}

impl AcceptanceCriterion {
    /// Create a new unverified criterion.
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            met: false,
            evidence: None,
            verified_at: None,
            verifier: None,
        }
    }

    /// Sprint D1: builder helper to attach a [`VerifierKind`] to this
    /// criterion. The verifier drives the (Sprint D4) verification dispatch.
    pub fn with_verifier(mut self, verifier: VerifierKind) -> Self {
        self.verifier = Some(verifier);
        self
    }

    /// Mark this criterion as met with evidence.
    pub fn mark_met(&mut self, evidence: impl Into<String>) {
        self.met = true;
        self.evidence = Some(evidence.into());
        self.verified_at = Some(Utc::now());
    }

    /// Reset this criterion to unverified (e.g., after regression).
    pub fn reset(&mut self) {
        self.met = false;
        self.evidence = None;
        self.verified_at = None;
    }
}

// ============================================================================
// Story State
// ============================================================================

/// The lifecycle state of a story.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoryState {
    /// Not yet started.
    Pending,
    /// Currently being worked on.
    InProgress,
    /// All acceptance criteria verified.
    Passed,
    /// Failed during execution or verification (may be retried).
    Failed,
    /// Explicitly blocked by a dependency or external factor.
    Blocked,
}

impl StoryState {
    /// Whether this story is considered complete.
    pub fn is_terminal(&self) -> bool {
        matches!(self, StoryState::Passed)
    }

    /// Whether this story can be picked for execution.
    pub fn is_actionable(&self) -> bool {
        matches!(self, StoryState::Pending | StoryState::Failed)
    }
}

// ============================================================================
// Capability Source
// ============================================================================

/// Sprint D1: where a [`Story`]'s capability came from in the discovery layer.
///
/// Tracking the source lets the (Sprint D3) PersistentLoop dispatch decide
/// how to invoke the underlying action — e.g. native connector vs. installed
/// skill vs. a remote SkillHub package that still needs to be installed.
///
/// Backward-compatible: attached to `Story.source` as
/// `Option<CapabilitySource>` with `#[serde(default)]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum CapabilitySource {
    /// Native capability provided by an in-process [`ConnectorId`].
    Native { connector: ConnectorId },
    /// Capability surfaced by an already-installed skill ([`SkillId`]).
    InstalledSkill { skill: SkillId },
    /// Capability available from the remote SkillHub catalog by `name`;
    /// `install_required` is `true` when the skill must be installed before
    /// dispatch.
    SkillHub {
        name: String,
        install_required: bool,
    },
    /// LLM provider modality (e.g. `provider = "openai"`, `api = "tts"`).
    ProviderModality { provider: String, api: String },
    /// External command-line runtime (`binary` + `args`).
    CmdRuntime { binary: String, args: Vec<String> },
    /// A capability request that has been queued and resolved by the
    /// CapabilityRequest pipeline; tracked by `request_id`.
    CapabilityRequest { request_id: String },
}

// ============================================================================
// Story
// ============================================================================

/// A discrete unit of work with testable acceptance criteria.
///
/// Stories are the fundamental building block of an execution plan.
/// Each story has a unique ID, a description, and one or more acceptance
/// criteria that must all be met for the story to pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Story {
    /// Unique story identifier (e.g., "US-001").
    pub id: String,
    /// Human-readable description of what this story accomplishes.
    pub description: String,
    /// Acceptance criteria that must all be met.
    pub criteria: Vec<AcceptanceCriterion>,
    /// Current lifecycle state.
    pub state: StoryState,
    /// Priority (lower = higher priority, 0 is highest).
    pub priority: u32,
    /// Number of times this story has been attempted.
    pub attempt_count: u32,
    /// Maximum allowed attempts before marking as permanently failed.
    pub max_attempts: u32,
    /// IDs of stories that must pass before this one can start.
    pub depends_on: Vec<String>,
    /// When this story was created.
    pub created_at: DateTime<Utc>,
    /// When this story was last updated.
    pub updated_at: DateTime<Utc>,
    /// Sprint D1: optional [`CapabilityId`] this story should dispatch to.
    ///
    /// `None` keeps current behaviour (story is solved by the agent loop with
    /// no fixed capability binding). `Some(id)` lets the PersistentLoop
    /// dispatch directly to that capability without re-resolving.
    /// `#[serde(default)]` preserves backward compatibility for existing
    /// serialized stories.
    #[serde(default)]
    pub capability_id: Option<CapabilityId>,
    /// Sprint D1: optional discovery [`CapabilitySource`] hit that produced
    /// `capability_id`.
    ///
    /// Populated by the discovery layer so dispatch can pick the right
    /// invocation strategy. `#[serde(default)]` keeps older snapshots
    /// decodable.
    #[serde(default)]
    pub source: Option<CapabilitySource>,
    /// 2026-05-06 — capability input payload for the story.
    ///
    /// `None` keeps the historical behaviour: the dispatch sink receives
    /// `serde_json::Value::Null` as input, which is fine for capabilities
    /// that take no parameters but breaks any capability that needs real
    /// data (e.g. `slides.render { markdown, output_path }`,
    /// `cmd.run { command }`). Setting this to `Some(value)` lets the
    /// planner / caller carry typed input through Story DAG execution.
    /// `#[serde(default)]` preserves backward compatibility.
    #[serde(default)]
    pub capability_input: Option<serde_json::Value>,
}

impl Story {
    /// Create a new story with the given criteria.
    pub fn new(
        id: impl Into<String>,
        description: impl Into<String>,
        criteria: Vec<AcceptanceCriterion>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            description: description.into(),
            criteria,
            state: StoryState::Pending,
            priority: 100,
            attempt_count: 0,
            max_attempts: 3,
            depends_on: Vec::new(),
            created_at: now,
            updated_at: now,
            capability_id: None,
            source: None,
            capability_input: None,
        }
    }

    /// Sprint D1: builder helper to bind this story to a specific
    /// [`CapabilityId`] for direct dispatch.
    pub fn with_capability_id(mut self, capability_id: CapabilityId) -> Self {
        self.capability_id = Some(capability_id);
        self
    }

    /// 2026-05-06 — builder helper to attach a JSON input payload that
    /// `PersistentLoop::execute` will pass to the dispatch sink instead
    /// of the historical `Value::Null`. Required for capabilities that
    /// take real parameters (e.g. `slides.render`, `cmd.run`).
    pub fn with_capability_input(mut self, input: serde_json::Value) -> Self {
        self.capability_input = Some(input);
        self
    }

    /// Sprint D1: builder helper to record the discovery [`CapabilitySource`]
    /// that produced this story's capability binding.
    pub fn with_source(mut self, source: CapabilitySource) -> Self {
        self.source = Some(source);
        self
    }

    /// Set the priority (lower = higher priority).
    pub fn with_priority(mut self, priority: u32) -> Self {
        self.priority = priority;
        self
    }

    /// Add a dependency on another story.
    pub fn with_dependency(mut self, story_id: impl Into<String>) -> Self {
        self.depends_on.push(story_id.into());
        self
    }

    /// Set maximum retry attempts.
    pub fn with_max_attempts(mut self, max: u32) -> Self {
        self.max_attempts = max;
        self
    }

    /// Check if all acceptance criteria are met.
    pub fn all_criteria_met(&self) -> bool {
        !self.criteria.is_empty() && self.criteria.iter().all(|c| c.met)
    }

    /// Count how many criteria are met.
    pub fn criteria_met_count(&self) -> usize {
        self.criteria.iter().filter(|c| c.met).count()
    }

    /// Mark the story as passed (all criteria verified).
    ///
    /// Returns `false` if not all criteria are actually met.
    pub fn mark_passed(&mut self) -> bool {
        if !self.all_criteria_met() {
            return false;
        }
        self.state = StoryState::Passed;
        self.updated_at = Utc::now();
        true
    }

    /// Mark the story as failed with a reason.
    pub fn mark_failed(&mut self) {
        self.state = StoryState::Failed;
        self.updated_at = Utc::now();
    }

    /// Begin working on this story.
    pub fn begin(&mut self) {
        self.state = StoryState::InProgress;
        self.attempt_count += 1;
        self.updated_at = Utc::now();
    }

    /// Whether retries are exhausted.
    pub fn retries_exhausted(&self) -> bool {
        self.attempt_count >= self.max_attempts
    }
}

// ============================================================================
// Execution Plan
// ============================================================================

/// A structured execution plan composed of ordered stories.
///
/// The plan tracks overall completion and provides story selection logic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    /// Human-readable goal description.
    pub goal: String,
    /// Ordered list of stories.
    pub stories: Vec<Story>,
    /// When the plan was created.
    pub created_at: DateTime<Utc>,
    /// Plan-level metadata.
    pub metadata: HashMap<String, String>,
    /// Hard upper limit on fix-loop iterations. Defaults to 5.
    ///
    /// When the loop has advanced this many times in fix/retry mode (i.e. at
    /// least one story has been attempted and failed at least once) the loop
    /// exits with [`PersistentLoopOutcome::MaxFixLoopsExceeded`].  A value of
    /// `0` is invalid and is rejected by [`ExecutionPlan::validate`].
    #[serde(default = "default_max_fix_loops")]
    pub max_fix_loops: u32,
}

fn default_max_fix_loops() -> u32 {
    5
}

impl ExecutionPlan {
    /// Create a new empty plan with a goal.
    pub fn new(goal: impl Into<String>) -> Self {
        Self {
            goal: goal.into(),
            stories: Vec::new(),
            created_at: Utc::now(),
            metadata: HashMap::new(),
            max_fix_loops: default_max_fix_loops(),
        }
    }

    /// Validate plan configuration.
    ///
    /// Returns an error string describing the first problem found.
    ///
    /// # Errors
    ///
    /// - `max_fix_loops` is `0` — zero means "never allow any fix iteration",
    ///   which is almost certainly a misconfiguration.
    pub fn validate(&self) -> Result<(), String> {
        if self.max_fix_loops == 0 {
            return Err(
                "max_fix_loops must be at least 1; use LoopConfig::max_iterations to set \
                 an overall cap instead"
                    .to_string(),
            );
        }
        Ok(())
    }

    /// Add a story to the plan.
    pub fn add_story(&mut self, story: Story) {
        self.stories.push(story);
    }

    /// Pick the next actionable story (highest priority, dependencies met).
    pub fn pick_next(&self) -> Option<&Story> {
        let passed_ids: Vec<&str> = self
            .stories
            .iter()
            .filter(|s| s.state == StoryState::Passed)
            .map(|s| s.id.as_str())
            .collect();

        self.stories
            .iter()
            .filter(|s| s.state.is_actionable())
            .filter(|s| !s.retries_exhausted())
            .filter(|s| {
                s.depends_on
                    .iter()
                    .all(|dep| passed_ids.contains(&dep.as_str()))
            })
            .min_by_key(|s| s.priority)
    }

    /// Get a mutable reference to a story by ID.
    pub fn story_mut(&mut self, id: &str) -> Option<&mut Story> {
        self.stories.iter_mut().find(|s| s.id == id)
    }

    /// Get a reference to a story by ID.
    pub fn story(&self, id: &str) -> Option<&Story> {
        self.stories.iter().find(|s| s.id == id)
    }

    /// Check if all stories have passed.
    pub fn all_complete(&self) -> bool {
        !self.stories.is_empty() && self.stories.iter().all(|s| s.state == StoryState::Passed)
    }

    /// Count stories in each state.
    pub fn state_counts(&self) -> HashMap<&'static str, usize> {
        let mut counts = HashMap::new();
        for story in &self.stories {
            let key = match story.state {
                StoryState::Pending => "pending",
                StoryState::InProgress => "in_progress",
                StoryState::Passed => "passed",
                StoryState::Failed => "failed",
                StoryState::Blocked => "blocked",
            };
            *counts.entry(key).or_insert(0) += 1;
        }
        counts
    }

    /// Number of stories not yet passed.
    pub fn pending_count(&self) -> usize {
        self.stories
            .iter()
            .filter(|s| s.state != StoryState::Passed)
            .count()
    }

    /// Overall completion percentage (0.0 to 1.0).
    pub fn completion_ratio(&self) -> f64 {
        if self.stories.is_empty() {
            return 0.0;
        }
        let passed = self
            .stories
            .iter()
            .filter(|s| s.state == StoryState::Passed)
            .count();
        passed as f64 / self.stories.len() as f64
    }
}

// ============================================================================
// Learning Record
// ============================================================================

/// A single learning entry recorded during execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningEntry {
    /// Which story this learning came from.
    pub story_id: String,
    /// Iteration number when this was learned.
    pub iteration: u32,
    /// What was learned (pattern, failure cause, workaround).
    pub insight: String,
    /// Category of learning.
    pub category: LearningCategory,
    /// When the learning was recorded.
    pub recorded_at: DateTime<Utc>,
}

/// Category of a learning entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LearningCategory {
    /// A failure cause that was identified.
    FailureCause,
    /// A codebase pattern that was discovered.
    CodebasePattern,
    /// A workaround that was applied.
    Workaround,
    /// A dependency or constraint that was found.
    Constraint,
}

// ============================================================================
// Progress Journal
// ============================================================================

/// Cross-iteration learning journal.
///
/// Records insights, failures, and patterns discovered during execution.
/// This information is carried forward to improve retry success rates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressJournal {
    /// All learning entries, ordered by time.
    pub entries: Vec<LearningEntry>,
    /// Files changed during the entire execution.
    pub changed_files: Vec<String>,
}

impl ProgressJournal {
    /// Create a new empty journal.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            changed_files: Vec::new(),
        }
    }

    /// Record a learning entry.
    pub fn record(
        &mut self,
        story_id: impl Into<String>,
        iteration: u32,
        insight: impl Into<String>,
        category: LearningCategory,
    ) {
        self.entries.push(LearningEntry {
            story_id: story_id.into(),
            iteration,
            insight: insight.into(),
            category,
            recorded_at: Utc::now(),
        });
    }

    /// Add a changed file path.
    pub fn track_file(&mut self, path: impl Into<String>) {
        let path = path.into();
        if !self.changed_files.contains(&path) {
            self.changed_files.push(path);
        }
    }

    /// Get learnings for a specific story.
    pub fn learnings_for(&self, story_id: &str) -> Vec<&LearningEntry> {
        self.entries
            .iter()
            .filter(|e| e.story_id == story_id)
            .collect()
    }

    /// Get all failure-cause learnings (useful for retry context).
    pub fn failure_causes(&self) -> Vec<&LearningEntry> {
        self.entries
            .iter()
            .filter(|e| e.category == LearningCategory::FailureCause)
            .collect()
    }
}

impl Default for ProgressJournal {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Verification Verdict
// ============================================================================

/// Outcome of a verification gate review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationVerdict {
    /// Whether the reviewer approved.
    pub approved: bool,
    /// Reviewer identifier (e.g., "architect", "critic", "security").
    pub reviewer: String,
    /// Detailed feedback from the reviewer.
    pub feedback: String,
    /// Specific issues that need fixing (if rejected).
    pub issues: Vec<String>,
    /// When the review was performed.
    pub reviewed_at: DateTime<Utc>,
}

impl VerificationVerdict {
    /// Create an approval verdict.
    pub fn approve(reviewer: impl Into<String>, feedback: impl Into<String>) -> Self {
        Self {
            approved: true,
            reviewer: reviewer.into(),
            feedback: feedback.into(),
            issues: Vec::new(),
            reviewed_at: Utc::now(),
        }
    }

    /// Create a rejection verdict with issues.
    pub fn reject(
        reviewer: impl Into<String>,
        feedback: impl Into<String>,
        issues: Vec<String>,
    ) -> Self {
        Self {
            approved: false,
            reviewer: reviewer.into(),
            feedback: feedback.into(),
            issues,
            reviewed_at: Utc::now(),
        }
    }
}

// ============================================================================
// Verification Gate
// ============================================================================

/// The outcome of a single verification phase in the persistent loop.
///
/// Per OMC autopilot SKILL §7.5/7.6, verification after a Pass expands into a
/// three-phase sequence (Pass → Deslop → PostDeslopRegression). The
/// [`VerificationGate`] enum records which phase produced which verdict so that
/// callers can distinguish between "original tests pass", "deslop cleanup ran",
/// and "tests still pass after deslop".
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationGate {
    /// Original verification passed — all acceptance criteria met.
    Pass,
    /// Original verification failed — at least one criterion or reviewer
    /// rejected the iteration.
    Fail,
    /// Verification is still running (e.g., reviewer tokens streaming in, async
    /// regression suite active).
    InProgress,
    /// Deslop cleanup ran after an initial Pass. The embedded
    /// [`DeslopOutcome`] records which files changed and whether the
    /// post-deslop regression verdict re-passed.
    Deslop {
        /// Structured result of the deslop pass + post-deslop regression re-run.
        outcome: DeslopOutcome,
    },
}

impl VerificationGate {
    /// Returns `true` when the gate represents a terminal approval (i.e., a
    /// Pass that has been confirmed by deslop + regression).
    pub fn is_terminal_pass(&self) -> bool {
        match self {
            VerificationGate::Pass => true,
            VerificationGate::Deslop { outcome } => outcome.regression_passed,
            _ => false,
        }
    }

    /// Returns `true` when the gate represents a failure or a deslop-induced
    /// regression.
    pub fn is_failure(&self) -> bool {
        match self {
            VerificationGate::Fail => true,
            VerificationGate::Deslop { outcome } => !outcome.regression_passed,
            _ => false,
        }
    }
}

// ============================================================================
// Deslop Gate (post-pass cleanup + regression)
// ============================================================================

/// Structured result of a deslop cleanup pass and its post-deslop regression
/// re-verification.
///
/// Produced by implementations of [`DeslopRunner::run_deslop`] and embedded in
/// [`VerificationGate::Deslop`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeslopOutcome {
    /// Files modified by the deslop cleanup pass (e.g., dead-code removal,
    /// formatting fixes). Empty when the runner made no changes.
    pub changed_files: Vec<PathBuf>,
    /// Whether the post-deslop regression suite still passed after cleanup.
    ///
    /// When `false` the caller must roll back: the iteration must not advance
    /// and the Pass verdict must be treated as degraded.
    pub regression_passed: bool,
    /// Human-readable details (e.g., test command output, rule counts).
    pub regression_details: String,
}

impl DeslopOutcome {
    /// Construct an outcome representing a clean no-op (no files changed,
    /// regression still passing).
    pub fn clean_noop() -> Self {
        Self {
            changed_files: Vec::new(),
            regression_passed: true,
            regression_details: "no-op: no deslop actions applied".to_string(),
        }
    }
}

/// Error returned from a [`DeslopRunner`] implementation.
///
/// The default implementation uses a thin wrapper around a `String` so that
/// callers can propagate both hard failures (process spawn errors) and soft
/// failures (regression suite output) through the same channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeslopError {
    /// Human-readable error message.
    pub message: String,
}

impl DeslopError {
    /// Construct an error with the given message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for DeslopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "deslop error: {}", self.message)
    }
}

impl std::error::Error for DeslopError {}

/// Strategy trait for running the deslop cleanup + post-deslop regression
/// phase.
///
/// Implementations are responsible for:
/// 1. Applying any deslop transformations (dead-code removal, format fixes).
/// 2. Re-running the regression suite on the modified tree.
/// 3. Returning a [`DeslopOutcome`] that records the changed files and whether
///    the regression still passes.
///
/// The default implementation [`NoopDeslopRunner`] performs no changes and
/// reports `regression_passed = true`, making it safe to use when deslop is
/// disabled (e.g., in unit tests).
///
/// # Panic contract
///
/// A panic inside [`DeslopRunner::run_deslop`] **propagates out** of
/// [`PersistentLoop::verify_after_pass`] and leaves
/// [`PersistentLoop::gate_phases`] in an inconsistent state: the Pass phase
/// has already been appended (phase A in `verify_after_pass`) but the matching
/// Deslop phase (phases B+C) will not be recorded. Callers that invoke
/// untrusted runners should wrap the call in
/// [`std::panic::catch_unwind`] and, on caught panic, either pop the trailing
/// Pass phase or convert it into an explicit failure verdict before reusing
/// the loop.
pub trait DeslopRunner: Send + Sync + std::fmt::Debug {
    /// Run the deslop cleanup + post-deslop regression over the given changed
    /// file set.
    ///
    /// `changed_files` is the union of files the loop has modified during the
    /// current iteration — implementations may narrow their analysis to these
    /// paths.
    ///
    /// # Panics
    ///
    /// Implementations that panic will corrupt the host loop's `gate_phases`
    /// log as described in the trait-level docs. Wrap untrusted runners in
    /// [`std::panic::catch_unwind`] at the call site.
    fn run_deslop(&self, changed_files: &[PathBuf]) -> Result<DeslopOutcome, DeslopError>;
}

/// Default no-op [`DeslopRunner`] — returns an outcome with no changed files
/// and `regression_passed = true`.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopDeslopRunner;

impl DeslopRunner for NoopDeslopRunner {
    fn run_deslop(&self, _changed_files: &[PathBuf]) -> Result<DeslopOutcome, DeslopError> {
        Ok(DeslopOutcome::clean_noop())
    }
}

// ============================================================================
// Deslop Gate
// ============================================================================

/// Signals that indicate real progress in a single iteration verdict.
///
/// At least one of these must be true in a verdict for the iteration to count
/// as productive. If N consecutive iterations all lack these signals, the
/// deslop gate trips.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProgressSignals {
    /// One or more artifacts were added (files created, tests added, etc.).
    pub artifacts_added: bool,
    /// The number of acceptance criteria that transitioned to `met` this
    /// iteration (positive = real progress).
    pub criteria_met_delta: u32,
    /// At least one story advanced its state (e.g., Pending → InProgress,
    /// InProgress → Passed).
    pub story_progressed: bool,
}

impl ProgressSignals {
    /// Returns `true` when at least one genuine progress signal is present.
    pub fn has_real_progress(&self) -> bool {
        self.artifacts_added || self.criteria_met_delta > 0 || self.story_progressed
    }
}

/// Detects "slop loops" — consecutive iterations with no real progress.
///
/// The detector maintains a rolling window of the last `window` iteration
/// results. When every slot in the window is a no-progress verdict it trips
/// and the caller should exit with [`PersistentLoopOutcome::Deslopped`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeslopDetector {
    /// Number of consecutive no-progress rounds that triggers deslop.
    pub window: u32,
    /// Ring buffer of recent progress results (`true` = had real progress).
    recent: Vec<bool>,
}

impl DeslopDetector {
    /// Create a new detector with the given window size (must be >= 1).
    pub fn new(window: u32) -> Self {
        assert!(window >= 1, "deslop window must be at least 1");
        Self {
            window,
            recent: Vec::new(),
        }
    }

    /// Record the progress signals for the just-completed iteration.
    ///
    /// Returns `true` if the deslop gate has now tripped (i.e., the last
    /// `window` iterations all had no real progress).
    pub fn record(&mut self, signals: &ProgressSignals) -> bool {
        self.recent.push(signals.has_real_progress());
        // Keep only the last `window` entries.
        let w = self.window as usize;
        if self.recent.len() > w {
            self.recent.drain(0..self.recent.len() - w);
        }
        self.is_deslopped()
    }

    /// Returns `true` when the detector has seen a full window of no-progress
    /// iterations.
    pub fn is_deslopped(&self) -> bool {
        let w = self.window as usize;
        self.recent.len() >= w && self.recent.iter().all(|&had_progress| !had_progress)
    }

    /// Reset the detector (e.g., after genuine progress is externally confirmed).
    pub fn reset(&mut self) {
        self.recent.clear();
    }
}

impl Default for DeslopDetector {
    fn default() -> Self {
        Self::new(3)
    }
}

// ============================================================================
// Loop Configuration
// ============================================================================

/// Configuration for the persistent execution loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopConfig {
    /// Maximum number of iterations before forced stop.
    pub max_iterations: u32,
    /// Maximum consecutive failures on a single story before escalation.
    pub max_story_failures: u32,
    /// Whether to require reviewer verification before completion.
    pub require_verification: bool,
    /// Reviewer type for the verification gate.
    pub reviewer_type: ReviewerType,
}

/// Type of reviewer for verification gate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReviewerType {
    /// Architecture reviewer (default).
    Architect,
    /// Code quality critic.
    Critic,
    /// Security reviewer.
    Security,
    /// Custom reviewer identifier.
    Custom(String),
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 100,
            max_story_failures: 3,
            require_verification: true,
            reviewer_type: ReviewerType::Architect,
        }
    }
}

// ============================================================================
// Loop State
// ============================================================================

/// The decision from a loop iteration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopDecision {
    /// Continue to the next story.
    Continue,
    /// All stories passed; proceed to verification.
    ReadyForVerification,
    /// Verification passed; execution complete.
    Complete,
    /// Maximum iterations reached.
    MaxIterationsReached,
    /// A story has exhausted its retries.
    StoryRetryExhausted(String),
    /// No actionable stories remain (all blocked or exhausted).
    Stuck,
}

/// Terminal outcome of a persistent execution loop.
///
/// Returned by [`PersistentLoop::outcome`] when the loop has ended for any
/// reason other than normal per-iteration flow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PersistentLoopOutcome {
    /// All stories passed and the verification gate approved.
    Completed,
    /// All stories passed; verification was not required.
    CompletedWithoutVerification,
    /// The overall iteration cap was hit before all stories passed.
    MaxIterationsReached,
    /// A story exhausted all retry attempts.
    StoryRetryExhausted(String),
    /// No actionable stories remain (all blocked or retries exhausted).
    Stuck,
    /// Deslop gate tripped: N consecutive iterations showed no real progress.
    Deslopped,
    /// The fix-loop hard cap (`ExecutionPlan::max_fix_loops`) was exceeded.
    MaxFixLoopsExceeded,
}

// ============================================================================
// Persistent Loop
// ============================================================================

/// The persistent execution loop engine.
///
/// Manages the lifecycle of story-based execution:
/// 1. Pick next actionable story
/// 2. Track execution attempts
/// 3. Record learnings on failure
/// 4. Verify completion with reviewer gate
///
/// This struct is the state machine; actual execution is delegated to the
/// caller (typically the Autopilot runtime or agent loop).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentLoop {
    /// The execution plan with stories.
    pub plan: ExecutionPlan,
    /// Loop configuration.
    pub config: LoopConfig,
    /// Progress journal for cross-iteration learning.
    pub journal: ProgressJournal,
    /// Current iteration number.
    pub current_iteration: u32,
    /// ID of the story currently being worked on.
    pub current_story_id: Option<String>,
    /// Verification verdicts received.
    pub verdicts: Vec<VerificationVerdict>,
    /// When the loop started.
    pub started_at: DateTime<Utc>,
    /// Deslop gate: detects stalled loops with no real progress.
    pub deslop: DeslopDetector,
    /// Number of fix/retry iterations consumed so far.
    ///
    /// Incremented each time a story that has already failed at least once is
    /// picked for re-execution.  Compared against
    /// [`ExecutionPlan::max_fix_loops`] before each such pick.
    pub fix_loop_count: u32,
    /// Verification gate phases recorded during the loop.
    ///
    /// Each call to [`PersistentLoop::verify_after_pass`] appends the phases
    /// it observed (typically `Pass`, then `Deslop { .. }`). Skipped in
    /// serde so stale snapshots remain decodable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gate_phases: Vec<VerificationGate>,
    /// Strategy for running the deslop cleanup + post-deslop regression phase.
    ///
    /// Defaults to [`NoopDeslopRunner`]. Replace via
    /// [`PersistentLoop::with_deslop_runner`] when real cleanup is required.
    #[serde(skip, default = "default_deslop_runner")]
    pub deslop_runner: Arc<dyn DeslopRunner>,
    /// Sprint D3: strategy that runs each criterion's [`VerifierKind`].
    ///
    /// Defaults to [`NoopVerifierExecutor`] (always returns Fail). Replace via
    /// [`PersistentLoop::with_verifier_executor`] when real verification
    /// dispatch is required.
    #[serde(skip, default = "default_verifier_executor")]
    pub verifier_executor: Arc<dyn VerifierExecutor>,
    /// Sprint D3: strategy that dispatches per-story `capability_id` calls.
    ///
    /// Defaults to [`NoopCapabilityDispatchSink`] (returns `Value::Null`).
    /// Replace via [`PersistentLoop::with_capability_dispatcher`] when real
    /// connector dispatch is required.
    #[serde(skip, default = "default_capability_dispatcher")]
    pub capability_dispatcher: Arc<dyn CapabilityDispatchSink>,
}

fn default_deslop_runner() -> Arc<dyn DeslopRunner> {
    Arc::new(NoopDeslopRunner)
}

fn default_capability_dispatcher() -> Arc<dyn CapabilityDispatchSink> {
    Arc::new(NoopCapabilityDispatchSink)
}

impl PersistentLoop {
    /// Create a new persistent loop with a plan and config.
    pub fn new(plan: ExecutionPlan, config: LoopConfig) -> Self {
        Self {
            plan,
            config,
            journal: ProgressJournal::new(),
            current_iteration: 0,
            current_story_id: None,
            verdicts: Vec::new(),
            started_at: Utc::now(),
            deslop: DeslopDetector::default(),
            fix_loop_count: 0,
            gate_phases: Vec::new(),
            deslop_runner: default_deslop_runner(),
            verifier_executor: default_verifier_executor(),
            capability_dispatcher: default_capability_dispatcher(),
        }
    }

    /// Replace the default [`NoopDeslopRunner`] with a custom implementation.
    ///
    /// Use this builder when the loop must invoke real cleanup and regression
    /// tooling (e.g., an ai-slop-cleaner + `cargo test` re-run).
    ///
    /// # Serde-rebind note
    ///
    /// The `deslop_runner` field is marked `#[serde(skip)]` and is rebuilt
    /// with the default [`NoopDeslopRunner`] when a [`PersistentLoop`] snapshot
    /// is deserialised. Callers that serialise the loop for checkpointing
    /// must **rebind** their custom runner via `with_deslop_runner` after
    /// reloading, or the next `verify_after_pass` call will silently regress
    /// to no-op behaviour.
    pub fn with_deslop_runner(mut self, runner: Arc<dyn DeslopRunner>) -> Self {
        self.deslop_runner = runner;
        self
    }

    /// Expand a Pass verdict into the three-phase OMC sequence
    /// (Pass → Deslop → PostDeslopRegression).
    ///
    /// Steps:
    /// - Phase A (Pass): record a [`VerificationGate::Pass`] phase.
    /// - Phase B (Deslop): invoke `self.deslop_runner.run_deslop(&changed_files)`.
    /// - Phase C (PostDeslopRegression): the runner's returned
    ///   [`DeslopOutcome`] is the verdict; its `regression_passed` field
    ///   decides whether the Pass stands or must be rolled back.
    ///
    /// On rollback (`regression_passed = false`) the method records the
    /// degraded [`VerificationGate::Deslop`] phase, marks the current story as
    /// failed (so the loop can re-attempt), and returns the outcome without
    /// advancing iteration state.
    ///
    /// Returns the [`DeslopOutcome`] produced by the runner on success, or a
    /// [`DeslopError`] if the runner itself errored. On runner error the
    /// `gate_phases` log records only the Pass phase (the Deslop phase did not
    /// complete).
    pub fn verify_after_pass(
        &mut self,
        changed_files: &[PathBuf],
    ) -> Result<DeslopOutcome, DeslopError> {
        // Phase A: record the initial Pass.
        self.gate_phases.push(VerificationGate::Pass);

        // Phase B + C: run deslop and the post-deslop regression re-verify.
        let outcome = self.deslop_runner.run_deslop(changed_files)?;

        // Record the composite Deslop phase (contains the regression verdict).
        self.gate_phases.push(VerificationGate::Deslop {
            outcome: outcome.clone(),
        });

        // Mirror changed files into the journal so downstream reports see them.
        for path in &outcome.changed_files {
            if let Some(s) = path.to_str() {
                self.journal.track_file(s);
            }
        }

        // Rollback: if regression failed, mark the current story as failed
        // (so the loop can retry) and record a learning. Do NOT advance.
        if !outcome.regression_passed {
            let reason = format!(
                "post-deslop regression failed: {}",
                outcome.regression_details
            );
            if self.current_story_id.is_some() {
                self.fail_current_story(reason);
            } else {
                // No current story to mark; still log as a learning entry under
                // a synthetic bucket so it surfaces in the journal.
                self.journal.record(
                    "__deslop__",
                    self.current_iteration,
                    &reason,
                    LearningCategory::FailureCause,
                );
            }
        }

        Ok(outcome)
    }

    /// Report progress signals for the iteration that just finished.
    ///
    /// Call this **after** completing or failing the current story, before the
    /// next [`advance`](Self::advance).  If the deslop gate trips this returns
    /// the terminal outcome immediately so the caller can short-circuit.
    ///
    /// Returns `Some(PersistentLoopOutcome::Deslopped)` when tripped, otherwise `None`.
    pub fn report_progress(&mut self, signals: ProgressSignals) -> Option<PersistentLoopOutcome> {
        if self.deslop.record(&signals) {
            Some(PersistentLoopOutcome::Deslopped)
        } else {
            None
        }
    }

    /// Advance to the next iteration and pick the next story.
    ///
    /// Returns the loop decision and optionally the story ID to work on.
    pub fn advance(&mut self) -> (LoopDecision, Option<String>) {
        self.current_iteration += 1;

        // Check iteration limit
        if self.current_iteration > self.config.max_iterations {
            return (LoopDecision::MaxIterationsReached, None);
        }

        // Check if all stories are complete
        if self.plan.all_complete() {
            if !self.config.require_verification {
                return (LoopDecision::Complete, None);
            }
            // Check if we already have an approval
            if self.verdicts.iter().any(|v| v.approved) {
                return (LoopDecision::Complete, None);
            }
            return (LoopDecision::ReadyForVerification, None);
        }

        // Pick the next story
        match self.plan.pick_next() {
            Some(story) => {
                let story_id = story.id.clone();
                let is_retry = story.attempt_count > 0;

                // Enforce max_fix_loops hard cap for retry iterations
                if is_retry {
                    if self.fix_loop_count >= self.plan.max_fix_loops {
                        return (LoopDecision::Stuck, None);
                    }
                    self.fix_loop_count += 1;
                }

                // Begin the story
                if let Some(s) = self.plan.story_mut(&story_id) {
                    s.begin();
                }
                self.current_story_id = Some(story_id.clone());
                (LoopDecision::Continue, Some(story_id))
            }
            None => {
                // No actionable stories — check for exhausted retries
                let exhausted: Vec<String> = self
                    .plan
                    .stories
                    .iter()
                    .filter(|s| s.retries_exhausted() && s.state == StoryState::Failed)
                    .map(|s| s.id.clone())
                    .collect();

                if let Some(id) = exhausted.first() {
                    (LoopDecision::StoryRetryExhausted(id.clone()), None)
                } else {
                    (LoopDecision::Stuck, None)
                }
            }
        }
    }

    /// Derive the terminal [`PersistentLoopOutcome`] from the last
    /// [`LoopDecision`] returned by [`advance`](Self::advance).
    ///
    /// This is a convenience mapper; callers that track the decision directly
    /// don't need it.
    pub fn outcome_from_decision(&self, decision: &LoopDecision) -> Option<PersistentLoopOutcome> {
        match decision {
            LoopDecision::Complete => {
                if self.config.require_verification {
                    Some(PersistentLoopOutcome::Completed)
                } else {
                    Some(PersistentLoopOutcome::CompletedWithoutVerification)
                }
            }
            LoopDecision::MaxIterationsReached => Some(PersistentLoopOutcome::MaxIterationsReached),
            LoopDecision::StoryRetryExhausted(id) => {
                Some(PersistentLoopOutcome::StoryRetryExhausted(id.clone()))
            }
            LoopDecision::Stuck => {
                // Distinguish max_fix_loops exhaustion from a genuine stuck state.
                if self.fix_loop_count >= self.plan.max_fix_loops
                    && self.plan.stories.iter().any(|s| s.attempt_count > 0)
                {
                    Some(PersistentLoopOutcome::MaxFixLoopsExceeded)
                } else {
                    Some(PersistentLoopOutcome::Stuck)
                }
            }
            LoopDecision::Continue | LoopDecision::ReadyForVerification => None,
        }
    }

    /// Mark the current story as having passed all criteria.
    ///
    /// Returns `false` if the story's criteria are not actually all met.
    pub fn complete_current_story(&mut self) -> bool {
        let story_id = match &self.current_story_id {
            Some(id) => id.clone(),
            None => return false,
        };
        if let Some(story) = self.plan.story_mut(&story_id) {
            if story.mark_passed() {
                self.current_story_id = None;
                return true;
            }
        }
        false
    }

    /// Mark the current story as failed and record a learning.
    pub fn fail_current_story(&mut self, reason: impl Into<String>) {
        let reason = reason.into();
        if let Some(story_id) = self.current_story_id.take() {
            // Record the failure as a learning
            self.journal.record(
                &story_id,
                self.current_iteration,
                &reason,
                LearningCategory::FailureCause,
            );
            // Mark the story as failed
            if let Some(story) = self.plan.story_mut(&story_id) {
                story.mark_failed();
            }
        }
    }

    /// Record a verification verdict.
    pub fn record_verdict(&mut self, verdict: VerificationVerdict) {
        self.verdicts.push(verdict);
    }

    /// Get learnings relevant to the current story (for retry context).
    pub fn current_story_learnings(&self) -> Vec<&LearningEntry> {
        match &self.current_story_id {
            Some(id) => self.journal.learnings_for(id),
            None => Vec::new(),
        }
    }

    /// Get the number of pending (non-passed) stories.
    pub fn pending_count(&self) -> usize {
        self.plan.pending_count()
    }

    /// Get a summary of current loop state.
    pub fn summary(&self) -> LoopSummary {
        let counts = self.plan.state_counts();
        LoopSummary {
            iteration: self.current_iteration,
            max_iterations: self.config.max_iterations,
            total_stories: self.plan.stories.len(),
            passed: *counts.get("passed").unwrap_or(&0),
            failed: *counts.get("failed").unwrap_or(&0),
            pending: *counts.get("pending").unwrap_or(&0),
            in_progress: *counts.get("in_progress").unwrap_or(&0),
            blocked: *counts.get("blocked").unwrap_or(&0),
            completion_ratio: self.plan.completion_ratio(),
            total_learnings: self.journal.entries.len(),
            files_changed: self.journal.changed_files.len(),
            verified: self.verdicts.iter().any(|v| v.approved),
        }
    }
}

/// Summary of loop state for reporting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopSummary {
    pub iteration: u32,
    pub max_iterations: u32,
    pub total_stories: usize,
    pub passed: usize,
    pub failed: usize,
    pub pending: usize,
    pub in_progress: usize,
    pub blocked: usize,
    pub completion_ratio: f64,
    pub total_learnings: usize,
    pub files_changed: usize,
    pub verified: bool,
}

// ============================================================================
// Sprint D3: Verifier Executor Trait
// ============================================================================

/// Sprint D3: stub trait that lets the (Sprint D4) verifier dispatch
/// implementation execute a [`VerifierKind`] against a real environment
/// (filesystem, ffprobe, scripts, LLMs).
///
/// D3 ships a [`NoopVerifierExecutor`] default that always returns
/// [`VerifierVerdict::Fail`]; D4 will replace it with the real per-variant
/// runner.
#[async_trait]
pub trait VerifierExecutor: Send + Sync + std::fmt::Debug {
    /// Run `kind` against `ctx` and return a [`VerifierVerdict`].
    async fn run(&self, kind: &VerifierKind, ctx: &VerifierContext) -> VerifierVerdict;
}

/// Sprint D3: per-call context handed to a [`VerifierExecutor`].
///
/// Carries the workspace root, named artifact paths produced by earlier
/// stories, and an optional [`cyberclaw_llm::LlmClient`] for variants like
/// [`VerifierKind::LlmSemanticMatch`].
#[derive(Clone, Default)]
pub struct VerifierContext {
    /// Working directory for this verification round.
    pub workspace_root: PathBuf,
    /// Named artifacts produced by earlier stories (e.g. `"report.csv"`).
    pub artifacts: HashMap<String, PathBuf>,
    /// Optional LLM client for semantic verifiers (LlmSemanticMatch).
    pub llm_client: Option<Arc<dyn cyberclaw_llm::LlmClient>>,
}

impl std::fmt::Debug for VerifierContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifierContext")
            .field("workspace_root", &self.workspace_root)
            .field("artifacts", &self.artifacts)
            .field("has_llm_client", &self.llm_client.is_some())
            .finish()
    }
}

/// Sprint D3: outcome returned by a [`VerifierExecutor::run`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifierVerdict {
    /// Verification passed — `evidence` is a free-form string captured into
    /// the [`AcceptanceCriterion::evidence`] field on the parent story.
    Pass { evidence: String },
    /// Verification failed — `reason` is surfaced into the journal failure
    /// log so the loop can decide whether to retry.
    Fail { reason: String },
}

/// Sprint D3: default no-op [`VerifierExecutor`] that always returns
/// [`VerifierVerdict::Fail`] with the "not wired" sentinel reason.
///
/// Used by [`PersistentLoop`] when no real executor has been attached. Sprint
/// D4 will replace this with a per-variant implementation.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopVerifierExecutor;

#[async_trait]
impl VerifierExecutor for NoopVerifierExecutor {
    async fn run(&self, _kind: &VerifierKind, _ctx: &VerifierContext) -> VerifierVerdict {
        VerifierVerdict::Fail {
            reason: "VerifierExecutor not wired (Sprint D4 pending)".to_string(),
        }
    }
}

fn default_verifier_executor() -> Arc<dyn VerifierExecutor> {
    Arc::new(NoopVerifierExecutor)
}

// ============================================================================
// Sprint D3: Persistent Execution Plan / Result / Error
// ============================================================================

/// Sprint D3: alias for [`ExecutionPlan`] when used as input to
/// [`PersistentLoop::execute`]. Disambiguates from `crate::types::ExecutionPlan`
/// at call sites.
pub type PersistentExecutionPlan = ExecutionPlan;

/// Sprint D3: outcome of a [`PersistentLoop::execute`] orchestration run.
#[derive(Debug, Clone, Default)]
pub struct PersistentExecutionResult {
    /// IDs of stories whose acceptance criteria all verified Pass.
    pub stories_completed: Vec<String>,
    /// IDs of stories that exhausted retries or failed verification.
    pub stories_failed: Vec<String>,
    /// Per-criterion `(story_id, criterion_index) -> evidence` map captured
    /// from VerifierVerdict::Pass on the criteria that did verify.
    pub verification_evidence: HashMap<String, Vec<String>>,
    /// Final artifact paths recorded in [`VerifierContext::artifacts`] at the
    /// end of the run.
    pub final_artifacts: HashMap<String, PathBuf>,
}

impl PersistentExecutionResult {
    /// Convenience: did every story in the plan complete?
    pub fn all_completed(&self) -> bool {
        self.stories_failed.is_empty() && !self.stories_completed.is_empty()
    }
}

/// Sprint D3: errors that abort a [`PersistentLoop::execute`] run before any
/// per-story work begins.
#[derive(Debug, thiserror::Error)]
pub enum PersistentExecutionError {
    /// The Story DAG contains a cycle — no valid topological order exists.
    /// Detected by Kahn's algorithm in [`topological_order`].
    #[error("dependency cycle detected: {stories:?}")]
    DependencyCycle { stories: Vec<String> },
    /// A story declared `depends_on` referring to a story id that does not
    /// exist in the plan.
    #[error("story '{story}' depends on unknown story '{missing}'")]
    UnknownDependency { story: String, missing: String },
    /// Capability dispatch produced an error (connector returned an error,
    /// dispatcher rejected the call, etc.). The wrapped string is the
    /// human-readable error message from the connector layer.
    #[error("capability dispatch failed for story '{story}': {message}")]
    DispatchFailed { story: String, message: String },
}

// ============================================================================
// Sprint D3: PersistentLoop async execute()
// ============================================================================

/// Sprint D3: free helper — Kahn topological sort over the Story DAG.
///
/// Returns the story IDs in dependency-respecting order, or a
/// [`PersistentExecutionError::DependencyCycle`] when a cycle exists.
///
/// Visible at the module level so the resolver can run cycle detection
/// without instantiating a [`PersistentLoop`].
pub fn topological_order(plan: &ExecutionPlan) -> Result<Vec<String>, PersistentExecutionError> {
    let ids: Vec<String> = plan.stories.iter().map(|s| s.id.clone()).collect();
    let id_set: std::collections::HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();

    // Validate every depends_on points to a known story.
    for story in &plan.stories {
        for dep in &story.depends_on {
            if !id_set.contains(dep.as_str()) {
                return Err(PersistentExecutionError::UnknownDependency {
                    story: story.id.clone(),
                    missing: dep.clone(),
                });
            }
        }
    }

    // Build in-degree map and adjacency list.
    let mut in_degree: HashMap<String, usize> = ids.iter().map(|i| (i.clone(), 0)).collect();
    let mut deps_of: HashMap<&str, Vec<&str>> = HashMap::new();
    for story in &plan.stories {
        for dep in &story.depends_on {
            *in_degree.get_mut(&story.id).unwrap() += 1;
            deps_of.entry(dep.as_str()).or_default().push(&story.id);
        }
    }

    // Seed queue with zero-in-degree nodes, in original story order so the
    // result is deterministic.
    let mut queue: std::collections::VecDeque<&str> = ids
        .iter()
        .filter(|i| *in_degree.get(*i).unwrap() == 0)
        .map(|s| s.as_str())
        .collect();

    let mut ordered: Vec<String> = Vec::with_capacity(ids.len());
    while let Some(node) = queue.pop_front() {
        ordered.push(node.to_string());
        if let Some(children) = deps_of.get(node) {
            for child in children {
                let degree = in_degree.get_mut(*child).unwrap();
                *degree -= 1;
                if *degree == 0 {
                    queue.push_back(child);
                }
            }
        }
    }

    if ordered.len() != ids.len() {
        // Stories left with non-zero in-degree are part of a cycle.
        let cyclic: Vec<String> = in_degree
            .into_iter()
            .filter(|(_, d)| *d > 0)
            .map(|(k, _)| k)
            .collect();
        return Err(PersistentExecutionError::DependencyCycle { stories: cyclic });
    }

    Ok(ordered)
}

/// Sprint D3: trait that dispatches a `(connector, capability, input)` triple
/// to the underlying connector and returns its raw output value.
///
/// Mirrors the read-only seam used by [`crate::dispatcher::CapabilityFacade`]
/// so [`PersistentLoop::execute`] can be exercised in unit tests without
/// instantiating the full `cyberclaw_connectors::CapabilityDispatcher` stack.
#[async_trait]
pub trait CapabilityDispatchSink: Send + Sync + std::fmt::Debug {
    /// Dispatch a capability call. Returns `Ok(output)` on success or `Err(msg)`
    /// on failure (the message is wrapped in
    /// [`PersistentExecutionError::DispatchFailed`]).
    async fn dispatch(
        &self,
        connector_id: &ConnectorId,
        capability_id: &CapabilityId,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, String>;
}

/// Sprint D3: default sink that drops every dispatch — used when the loop is
/// not wired to a real connector dispatcher (e.g. in unit tests that only
/// exercise verification logic).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopCapabilityDispatchSink;

#[async_trait]
impl CapabilityDispatchSink for NoopCapabilityDispatchSink {
    async fn dispatch(
        &self,
        _connector_id: &ConnectorId,
        _capability_id: &CapabilityId,
        _input: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        Ok(serde_json::Value::Null)
    }
}

impl PersistentLoop {
    /// Sprint D3: replace the default [`NoopVerifierExecutor`] with a custom
    /// implementation. Returns `Self` so callers can chain the builder.
    pub fn with_verifier_executor(mut self, executor: Arc<dyn VerifierExecutor>) -> Self {
        self.verifier_executor = executor;
        self
    }

    /// Sprint D3: attach a [`CapabilityDispatchSink`] used to invoke each
    /// story's `capability_id`. Defaults to [`NoopCapabilityDispatchSink`]
    /// (no-op).
    pub fn with_capability_dispatcher(mut self, sink: Arc<dyn CapabilityDispatchSink>) -> Self {
        self.capability_dispatcher = sink;
        self
    }

    /// Sprint D3: async orchestration entry point.
    ///
    /// Drives the Story DAG to completion:
    /// 1. Kahn topological sort over `plan.stories` (cycle → DependencyCycle).
    /// 2. For each story in order, dispatch `story.capability_id` (when set)
    ///    via the wired [`CapabilityDispatchSink`], then run each criterion's
    ///    `verifier` through the wired [`VerifierExecutor`].
    /// 3. On verifier failure, retry up to `story.max_attempts` times — each
    ///    retry re-dispatches the capability and re-runs all verifiers.
    /// 4. When every story passes, returns a [`PersistentExecutionResult`]
    ///    summarising completed/failed story IDs and the captured evidence.
    ///
    /// Note on `&self`: the loop's internal state machine (advance,
    /// complete_current_story) is **not** used by this method — D3 deliberately
    /// keeps the loop immutable so `Arc<PersistentLoop>` callers can run
    /// concurrent executions. Per-story state (attempt count, criterion `met`
    /// flags) is tracked on a private clone of the plan.
    pub async fn execute(
        &self,
        plan: &PersistentExecutionPlan,
        ctx: &mut ExecutionContext,
    ) -> Result<PersistentExecutionResult, PersistentExecutionError> {
        // Touch ctx so callers passing &mut don't get an unused-binding warning
        // and so future expansions (per-story workspace overrides) have a hook.
        let _ = ctx;

        // 1. Topological order — fails loud on cycles.
        let order = topological_order(plan)?;

        // 2. Per-story state lives on a private clone of the plan so we don't
        //    mutate the caller's input.
        let mut working_plan = plan.clone();

        let mut result = PersistentExecutionResult::default();
        let mut verifier_ctx = VerifierContext::default();

        for story_id in &order {
            // Skip stories that depended on a failed predecessor.
            let dep_failed = working_plan
                .story(story_id)
                .map(|s| {
                    s.depends_on
                        .iter()
                        .any(|dep| result.stories_failed.iter().any(|f| f == dep))
                })
                .unwrap_or(false);
            if dep_failed {
                result.stories_failed.push(story_id.clone());
                continue;
            }

            let max_attempts = working_plan
                .story(story_id)
                .map(|s| s.max_attempts.max(1))
                .unwrap_or(1);

            let mut story_passed = false;
            for _ in 0..max_attempts {
                if let Some(s) = working_plan.story_mut(story_id) {
                    s.begin();
                }

                // 2a. Capability dispatch (when story.capability_id is set).
                let dispatch_outcome: Result<(), PersistentExecutionError> = {
                    let story_snap = working_plan
                        .story(story_id)
                        .cloned()
                        .expect("story exists by topological order");
                    if let Some(cap_id) = story_snap.capability_id.clone() {
                        let connector_id = match story_snap.source.as_ref() {
                            Some(CapabilitySource::Native { connector }) => connector.clone(),
                            // For non-Native sources, fall back to the
                            // connector embedded in the source (best effort).
                            // Sprint D4 will fully wire skill/skillhub paths.
                            _ => ConnectorId::from_string("local".to_string())
                                .expect("local connector id is valid"),
                        };
                        // 2026-05-06 — pass the story's capability_input
                        // through to the sink instead of always sending
                        // Value::Null. Falls back to Null when the planner
                        // didn't specify one (back-compat with stories
                        // that solve themselves via verifiers without a
                        // real capability invocation).
                        let dispatch_input = story_snap
                            .capability_input
                            .clone()
                            .unwrap_or(serde_json::Value::Null);
                        match self
                            .capability_dispatcher
                            .dispatch(&connector_id, &cap_id, dispatch_input)
                            .await
                        {
                            Ok(_output) => Ok(()),
                            Err(msg) => Err(PersistentExecutionError::DispatchFailed {
                                story: story_id.clone(),
                                message: msg,
                            }),
                        }
                    } else {
                        Ok(())
                    }
                };

                if let Err(e) = dispatch_outcome {
                    if let Some(s) = working_plan.story_mut(story_id) {
                        s.mark_failed();
                    }
                    // Bubble dispatch errors out — a connector-level failure is
                    // not the same as a verification miss; callers should see it.
                    return Err(e);
                }

                // 2b. Verify every criterion.
                let crit_count = working_plan
                    .story(story_id)
                    .map(|s| s.criteria.len())
                    .unwrap_or(0);
                let mut all_met = crit_count > 0;
                let mut evidences: Vec<String> = Vec::new();
                for idx in 0..crit_count {
                    let kind = working_plan
                        .story(story_id)
                        .and_then(|s| s.criteria.get(idx).and_then(|c| c.verifier.clone()));
                    match kind {
                        Some(k) => {
                            let verdict = self.verifier_executor.run(&k, &verifier_ctx).await;
                            match verdict {
                                VerifierVerdict::Pass { evidence } => {
                                    if let Some(s) = working_plan.story_mut(story_id) {
                                        s.criteria[idx].mark_met(evidence.clone());
                                    }
                                    evidences.push(evidence);
                                }
                                VerifierVerdict::Fail { reason } => {
                                    all_met = false;
                                    if let Some(s) = working_plan.story_mut(story_id) {
                                        s.criteria[idx].reset();
                                        s.criteria[idx].evidence =
                                            Some(format!("verifier rejected: {}", reason));
                                    }
                                    break;
                                }
                            }
                        }
                        None => {
                            // No verifier specified — treat as auto-pass to
                            // preserve backward-compat with stories authored
                            // before VerifierKind existed.
                            if let Some(s) = working_plan.story_mut(story_id) {
                                s.criteria[idx].mark_met("auto-pass: no verifier configured");
                            }
                            evidences.push("auto-pass".to_string());
                        }
                    }
                }

                if all_met {
                    if let Some(s) = working_plan.story_mut(story_id) {
                        s.mark_passed();
                    }
                    result
                        .verification_evidence
                        .insert(story_id.clone(), evidences);
                    story_passed = true;
                    break;
                } else {
                    if let Some(s) = working_plan.story_mut(story_id) {
                        s.mark_failed();
                    }
                }
            }

            if story_passed {
                result.stories_completed.push(story_id.clone());
            } else {
                result.stories_failed.push(story_id.clone());
            }
        }

        result.final_artifacts = std::mem::take(&mut verifier_ctx.artifacts);
        Ok(result)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_plan() -> ExecutionPlan {
        let mut plan = ExecutionPlan::new("Test deployment");
        plan.add_story(
            Story::new(
                "US-001",
                "Build Docker image",
                vec![
                    AcceptanceCriterion::new("Dockerfile exists"),
                    AcceptanceCriterion::new("docker build succeeds"),
                ],
            )
            .with_priority(1),
        );
        plan.add_story(
            Story::new(
                "US-002",
                "Run integration tests",
                vec![AcceptanceCriterion::new("All tests pass")],
            )
            .with_priority(2)
            .with_dependency("US-001"),
        );
        plan.add_story(
            Story::new(
                "US-003",
                "Deploy to staging",
                vec![
                    AcceptanceCriterion::new("Deployment succeeds"),
                    AcceptanceCriterion::new("Health check passes"),
                ],
            )
            .with_priority(3)
            .with_dependency("US-002"),
        );
        plan
    }

    #[test]
    fn test_criterion_lifecycle() {
        let mut c = AcceptanceCriterion::new("Test passes");
        assert!(!c.met);
        assert!(c.evidence.is_none());

        c.mark_met("exit code 0");
        assert!(c.met);
        assert_eq!(c.evidence.as_deref(), Some("exit code 0"));
        assert!(c.verified_at.is_some());

        c.reset();
        assert!(!c.met);
        assert!(c.evidence.is_none());
    }

    #[test]
    fn test_story_state_transitions() {
        let mut story = Story::new(
            "S1",
            "Test story",
            vec![AcceptanceCriterion::new("Criterion A")],
        );
        assert_eq!(story.state, StoryState::Pending);
        assert!(story.state.is_actionable());

        story.begin();
        assert_eq!(story.state, StoryState::InProgress);
        assert_eq!(story.attempt_count, 1);

        // Can't pass without criteria met
        assert!(!story.mark_passed());
        assert_eq!(story.state, StoryState::InProgress);

        story.criteria[0].mark_met("evidence");
        assert!(story.mark_passed());
        assert_eq!(story.state, StoryState::Passed);
        assert!(story.state.is_terminal());
    }

    #[test]
    fn test_story_retry_exhaustion() {
        let mut story = Story::new("S1", "Retry test", vec![]).with_max_attempts(2);

        story.begin(); // attempt 1
        story.mark_failed();
        assert!(!story.retries_exhausted());

        story.begin(); // attempt 2
        story.mark_failed();
        assert!(story.retries_exhausted());
    }

    #[test]
    fn test_plan_pick_next_respects_priority() {
        let mut plan = ExecutionPlan::new("Priority test");
        plan.add_story(Story::new("A", "Low priority", vec![]).with_priority(10));
        plan.add_story(Story::new("B", "High priority", vec![]).with_priority(1));

        let next = plan.pick_next().unwrap();
        assert_eq!(next.id, "B");
    }

    #[test]
    fn test_plan_pick_next_respects_dependencies() {
        let plan = make_plan();
        // US-002 depends on US-001, US-003 depends on US-002
        // Only US-001 should be actionable
        let next = plan.pick_next().unwrap();
        assert_eq!(next.id, "US-001");
    }

    #[test]
    fn test_plan_pick_next_unblocks_after_dependency_passes() {
        let mut plan = make_plan();
        // Pass US-001
        plan.story_mut("US-001").unwrap().criteria[0].mark_met("ok");
        plan.story_mut("US-001").unwrap().criteria[1].mark_met("ok");
        plan.story_mut("US-001").unwrap().mark_passed();

        // Now US-002 should be available
        let next = plan.pick_next().unwrap();
        assert_eq!(next.id, "US-002");
    }

    #[test]
    fn test_plan_completion() {
        let mut plan = ExecutionPlan::new("Completion test");
        plan.add_story(Story::new(
            "S1",
            "Story 1",
            vec![AcceptanceCriterion::new("C1")],
        ));
        assert!(!plan.all_complete());
        assert_eq!(plan.completion_ratio(), 0.0);

        plan.story_mut("S1").unwrap().criteria[0].mark_met("ok");
        plan.story_mut("S1").unwrap().mark_passed();
        assert!(plan.all_complete());
        assert!((plan.completion_ratio() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_plan_state_counts() {
        let mut plan = make_plan();
        let counts = plan.state_counts();
        assert_eq!(*counts.get("pending").unwrap_or(&0), 3);

        plan.story_mut("US-001").unwrap().begin();
        let counts = plan.state_counts();
        assert_eq!(*counts.get("in_progress").unwrap_or(&0), 1);
        assert_eq!(*counts.get("pending").unwrap_or(&0), 2);
    }

    #[test]
    fn test_journal_records_and_filters() {
        let mut journal = ProgressJournal::new();
        journal.record(
            "S1",
            1,
            "Build failed: missing dep",
            LearningCategory::FailureCause,
        );
        journal.record(
            "S1",
            2,
            "Added dep, now works",
            LearningCategory::Workaround,
        );
        journal.record(
            "S2",
            1,
            "Uses factory pattern",
            LearningCategory::CodebasePattern,
        );

        assert_eq!(journal.entries.len(), 3);
        assert_eq!(journal.learnings_for("S1").len(), 2);
        assert_eq!(journal.learnings_for("S2").len(), 1);
        assert_eq!(journal.failure_causes().len(), 1);
    }

    #[test]
    fn test_journal_tracks_files() {
        let mut journal = ProgressJournal::new();
        journal.track_file("src/main.rs");
        journal.track_file("src/lib.rs");
        journal.track_file("src/main.rs"); // duplicate
        assert_eq!(journal.changed_files.len(), 2);
    }

    #[test]
    fn test_persistent_loop_basic_flow() {
        let plan = make_plan();
        let mut ploop = PersistentLoop::new(plan, LoopConfig::default());
        assert_eq!(ploop.pending_count(), 3);

        // Advance: should pick US-001
        let (decision, story_id) = ploop.advance();
        assert_eq!(decision, LoopDecision::Continue);
        assert_eq!(story_id, Some("US-001".to_string()));
        assert_eq!(ploop.current_iteration, 1);

        // Mark criteria met and complete
        ploop.plan.story_mut("US-001").unwrap().criteria[0].mark_met("ok");
        ploop.plan.story_mut("US-001").unwrap().criteria[1].mark_met("ok");
        assert!(ploop.complete_current_story());

        // Advance: should pick US-002
        let (decision, story_id) = ploop.advance();
        assert_eq!(decision, LoopDecision::Continue);
        assert_eq!(story_id, Some("US-002".to_string()));
    }

    #[test]
    fn test_persistent_loop_failure_and_retry() {
        let mut plan = ExecutionPlan::new("Retry test");
        plan.add_story(
            Story::new(
                "S1",
                "Flaky story",
                vec![AcceptanceCriterion::new("Must pass")],
            )
            .with_max_attempts(3),
        );
        let mut ploop = PersistentLoop::new(plan, LoopConfig::default());

        // First attempt: fail
        let (decision, _) = ploop.advance();
        assert_eq!(decision, LoopDecision::Continue);
        ploop.fail_current_story("Connection timeout");

        // Learning should be recorded
        assert_eq!(ploop.journal.failure_causes().len(), 1);

        // Second attempt: should retry
        let (decision, story_id) = ploop.advance();
        assert_eq!(decision, LoopDecision::Continue);
        assert_eq!(story_id, Some("S1".to_string()));
    }

    #[test]
    fn test_persistent_loop_retry_exhaustion() {
        let mut plan = ExecutionPlan::new("Exhaust test");
        plan.add_story(
            Story::new(
                "S1",
                "Always fails",
                vec![AcceptanceCriterion::new("Impossible")],
            )
            .with_max_attempts(2),
        );
        let mut ploop = PersistentLoop::new(plan, LoopConfig::default());

        // Attempt 1: fail
        ploop.advance();
        ploop.fail_current_story("Error 1");

        // Attempt 2: fail
        ploop.advance();
        ploop.fail_current_story("Error 2");

        // Attempt 3: should report exhaustion
        let (decision, _) = ploop.advance();
        assert_eq!(
            decision,
            LoopDecision::StoryRetryExhausted("S1".to_string())
        );
    }

    #[test]
    fn test_persistent_loop_completion_with_verification() {
        let mut plan = ExecutionPlan::new("Verify test");
        plan.add_story(Story::new(
            "S1",
            "Simple",
            vec![AcceptanceCriterion::new("Done")],
        ));
        let mut ploop = PersistentLoop::new(plan, LoopConfig::default());

        // Complete the story
        ploop.advance();
        ploop.plan.story_mut("S1").unwrap().criteria[0].mark_met("ok");
        ploop.complete_current_story();

        // Should be ready for verification
        let (decision, _) = ploop.advance();
        assert_eq!(decision, LoopDecision::ReadyForVerification);

        // Record approval
        ploop.record_verdict(VerificationVerdict::approve("architect", "LGTM"));

        // Now should be complete
        let (decision, _) = ploop.advance();
        assert_eq!(decision, LoopDecision::Complete);
    }

    #[test]
    fn test_persistent_loop_completion_without_verification() {
        let mut plan = ExecutionPlan::new("No verify");
        plan.add_story(Story::new(
            "S1",
            "Simple",
            vec![AcceptanceCriterion::new("Done")],
        ));
        let config = LoopConfig {
            require_verification: false,
            ..LoopConfig::default()
        };
        let mut ploop = PersistentLoop::new(plan, config);

        ploop.advance();
        ploop.plan.story_mut("S1").unwrap().criteria[0].mark_met("ok");
        ploop.complete_current_story();

        let (decision, _) = ploop.advance();
        assert_eq!(decision, LoopDecision::Complete);
    }

    #[test]
    fn test_persistent_loop_max_iterations() {
        let mut plan = ExecutionPlan::new("Iteration limit");
        plan.add_story(Story::new("S1", "Never finishes", vec![]));
        let config = LoopConfig {
            max_iterations: 2,
            ..LoopConfig::default()
        };
        let mut ploop = PersistentLoop::new(plan, config);

        ploop.advance(); // iteration 1
        ploop.advance(); // iteration 2
        let (decision, _) = ploop.advance(); // iteration 3 > limit
        assert_eq!(decision, LoopDecision::MaxIterationsReached);
    }

    #[test]
    fn test_persistent_loop_summary() {
        let plan = make_plan();
        let ploop = PersistentLoop::new(plan, LoopConfig::default());
        let summary = ploop.summary();

        assert_eq!(summary.total_stories, 3);
        assert_eq!(summary.pending, 3);
        assert_eq!(summary.passed, 0);
        assert!(!summary.verified);
        assert_eq!(summary.iteration, 0);
    }

    #[test]
    fn test_verification_verdict_types() {
        let approval = VerificationVerdict::approve("architect", "All criteria met");
        assert!(approval.approved);
        assert!(approval.issues.is_empty());

        let rejection = VerificationVerdict::reject(
            "critic",
            "Issues found",
            vec!["Missing error handling".to_string()],
        );
        assert!(!rejection.approved);
        assert_eq!(rejection.issues.len(), 1);
    }

    #[test]
    fn test_stuck_when_all_blocked() {
        let mut plan = ExecutionPlan::new("Blocked test");
        let mut story = Story::new("S1", "Blocked", vec![]);
        story.state = StoryState::Blocked;
        plan.add_story(story);

        let mut ploop = PersistentLoop::new(plan, LoopConfig::default());
        let (decision, _) = ploop.advance();
        assert_eq!(decision, LoopDecision::Stuck);
    }

    #[test]
    fn test_empty_plan_pick_next_returns_none() {
        let plan = ExecutionPlan::new("Empty");
        assert!(plan.pick_next().is_none());
        assert!(!plan.all_complete());
    }

    #[test]
    fn test_current_story_learnings() {
        let mut plan = ExecutionPlan::new("Learnings test");
        plan.add_story(
            Story::new("S1", "Story 1", vec![AcceptanceCriterion::new("C1")]).with_max_attempts(5),
        );
        let mut ploop = PersistentLoop::new(plan, LoopConfig::default());

        ploop.advance();
        ploop.fail_current_story("First failure reason");

        ploop.advance(); // retry S1
                         // Should have learnings from first attempt
        let learnings = ploop.current_story_learnings();
        assert_eq!(learnings.len(), 1);
        assert_eq!(learnings[0].insight, "First failure reason");
    }

    #[test]
    fn test_reviewer_type_default() {
        let config = LoopConfig::default();
        assert_eq!(config.reviewer_type, ReviewerType::Architect);
        assert!(config.require_verification);
    }

    // -------------------------------------------------------------------------
    // Sprint-9 L3: Deslop Gate + max_fix_loops tests
    // -------------------------------------------------------------------------

    #[test]
    fn deslop_detector_n_rounds_no_progress_trips() {
        let mut det = DeslopDetector::new(3);
        let no_progress = ProgressSignals::default(); // all false / 0

        assert!(!det.record(&no_progress)); // round 1: window not full
        assert!(!det.record(&no_progress)); // round 2: window not full
        assert!(det.record(&no_progress)); // round 3: window full, all no-progress → tripped
        assert!(det.is_deslopped());
    }

    #[test]
    fn deslop_detector_resets_on_real_progress() {
        let mut det = DeslopDetector::new(3);
        let no_progress = ProgressSignals::default();
        let real_progress = ProgressSignals {
            story_progressed: true,
            ..Default::default()
        };

        det.record(&no_progress);
        det.record(&no_progress);
        // Real progress in round 3 — window now has [false, false, true], not all-false
        assert!(!det.record(&real_progress));
        assert!(!det.is_deslopped());

        // Round 4: window shifts to [false, true, false] — still has the true
        // marker from round 3, so the gate does NOT trip yet.
        assert!(!det.record(&no_progress));
        assert!(!det.is_deslopped());

        // Round 5: window shifts to [true, false, false] — the true is still
        // inside the window, so the gate still does NOT trip.
        assert!(!det.record(&no_progress));
        assert!(!det.is_deslopped());

        // Round 6: the real_progress marker finally slides out of the window
        // which becomes [false, false, false] — three consecutive no-progress
        // rounds since the last real progress → gate trips.
        assert!(det.record(&no_progress));
        assert!(det.is_deslopped());
    }

    #[test]
    fn max_fix_loops_zero_rejected() {
        let mut plan = ExecutionPlan::new("Validate test");
        plan.max_fix_loops = 0;
        assert!(plan.validate().is_err());

        let valid_plan = ExecutionPlan::new("Valid");
        assert!(valid_plan.validate().is_ok());
    }

    #[test]
    fn max_fix_loops_exceeded_outcome() {
        let mut plan = ExecutionPlan::new("Fix cap test");
        plan.max_fix_loops = 2;
        plan.add_story(
            Story::new("S1", "Flaky", vec![AcceptanceCriterion::new("Must pass")])
                .with_max_attempts(10),
        );
        let mut ploop = PersistentLoop::new(plan, LoopConfig::default());

        // First attempt (not a retry) → fix_loop_count stays 0
        let (d, _) = ploop.advance();
        assert_eq!(d, LoopDecision::Continue);
        ploop.fail_current_story("err 1");

        // Retry 1 → fix_loop_count becomes 1
        let (d, _) = ploop.advance();
        assert_eq!(d, LoopDecision::Continue);
        ploop.fail_current_story("err 2");

        // Retry 2 → fix_loop_count becomes 2  (== max_fix_loops)
        let (d, _) = ploop.advance();
        assert_eq!(d, LoopDecision::Continue);
        ploop.fail_current_story("err 3");

        // Retry 3 would exceed cap → Stuck returned, outcome maps to MaxFixLoopsExceeded
        let (d, _) = ploop.advance();
        assert_eq!(d, LoopDecision::Stuck);
        assert_eq!(
            ploop.outcome_from_decision(&d),
            Some(PersistentLoopOutcome::MaxFixLoopsExceeded)
        );
    }

    #[test]
    fn backward_compat_missing_max_fix_loops() {
        // Simulate deserializing a plan JSON that has no max_fix_loops field
        // (written before the field existed). The serde default must apply.
        let json =
            r#"{"goal":"legacy","stories":[],"created_at":"2024-01-01T00:00:00Z","metadata":{}}"#;
        let plan: ExecutionPlan = serde_json::from_str(json).expect("deserialize failed");
        assert_eq!(plan.max_fix_loops, 5, "serde default must be 5");
        assert!(plan.validate().is_ok());
    }

    // -------------------------------------------------------------------------
    // Sprint-13 S13-1: VerificationGate::Deslop + DeslopRunner + verify_after_pass
    // -------------------------------------------------------------------------

    use std::sync::Mutex;

    /// Configurable test runner: fails regression on demand, tracks call count.
    #[derive(Debug)]
    struct FlakyDeslopRunner {
        fail_regression: bool,
        call_count: Mutex<u32>,
        changed_files: Vec<PathBuf>,
    }

    impl FlakyDeslopRunner {
        fn new(fail_regression: bool, changed_files: Vec<PathBuf>) -> Self {
            Self {
                fail_regression,
                call_count: Mutex::new(0),
                changed_files,
            }
        }

        fn calls(&self) -> u32 {
            *self.call_count.lock().unwrap()
        }
    }

    impl DeslopRunner for FlakyDeslopRunner {
        fn run_deslop(&self, _changed: &[PathBuf]) -> Result<DeslopOutcome, DeslopError> {
            *self.call_count.lock().unwrap() += 1;
            Ok(DeslopOutcome {
                changed_files: self.changed_files.clone(),
                regression_passed: !self.fail_regression,
                regression_details: if self.fail_regression {
                    "regression: 2 tests broken".to_string()
                } else {
                    "regression: all green".to_string()
                },
            })
        }
    }

    /// Always-erroring runner to exercise error propagation.
    #[derive(Debug)]
    struct ErroringDeslopRunner;

    impl DeslopRunner for ErroringDeslopRunner {
        fn run_deslop(&self, _changed: &[PathBuf]) -> Result<DeslopOutcome, DeslopError> {
            Err(DeslopError::new("cargo test failed to spawn"))
        }
    }

    #[test]
    fn deslop_variant_serde_roundtrip() {
        let outcome = DeslopOutcome {
            changed_files: vec![PathBuf::from("src/a.rs"), PathBuf::from("src/b.rs")],
            regression_passed: true,
            regression_details: "all green".to_string(),
        };
        let gate = VerificationGate::Deslop { outcome };

        let json = serde_json::to_string(&gate).expect("serialize");
        // snake_case rename: variant tag is "deslop", field is "outcome".
        assert!(json.contains("\"deslop\""), "expected deslop tag: {}", json);
        assert!(json.contains("outcome"));
        assert!(json.contains("regression_passed"));

        let back: VerificationGate = serde_json::from_str(&json).expect("deserialize");
        assert!(back.is_terminal_pass());
        assert!(!back.is_failure());
    }

    #[test]
    fn verification_gate_other_variants_serde() {
        // Pass / Fail / InProgress should all roundtrip with snake_case tags.
        for (gate, tag) in [
            (VerificationGate::Pass, "pass"),
            (VerificationGate::Fail, "fail"),
            (VerificationGate::InProgress, "in_progress"),
        ] {
            let json = serde_json::to_string(&gate).expect("serialize");
            assert!(json.contains(tag), "missing {} in {}", tag, json);
            let _back: VerificationGate = serde_json::from_str(&json).expect("deserialize");
        }
    }

    #[test]
    fn noop_runner_returns_clean_outcome() {
        let runner = NoopDeslopRunner;
        let outcome = runner
            .run_deslop(&[PathBuf::from("src/foo.rs")])
            .expect("noop should not fail");
        assert!(outcome.changed_files.is_empty());
        assert!(outcome.regression_passed);
        assert!(outcome.regression_details.contains("no-op"));
    }

    #[test]
    fn verify_after_pass_runs_three_phase_sequence() {
        let mut plan = ExecutionPlan::new("three-phase");
        plan.add_story(Story::new(
            "S1",
            "do work",
            vec![AcceptanceCriterion::new("ok")],
        ));
        let mut ploop = PersistentLoop::new(plan, LoopConfig::default());

        // Drive the loop to have a current story then complete it.
        ploop.advance();
        ploop.plan.story_mut("S1").unwrap().criteria[0].mark_met("green");
        ploop.complete_current_story();

        let runner = Arc::new(FlakyDeslopRunner::new(
            false,
            vec![PathBuf::from("src/cleaned.rs")],
        ));
        ploop = ploop.with_deslop_runner(runner.clone());

        let outcome = ploop
            .verify_after_pass(&[PathBuf::from("src/cleaned.rs")])
            .expect("verify should succeed");
        assert!(outcome.regression_passed);
        assert_eq!(runner.calls(), 1);

        // Phase log: [Pass, Deslop { .. }]
        assert_eq!(ploop.gate_phases.len(), 2);
        assert!(matches!(ploop.gate_phases[0], VerificationGate::Pass));
        assert!(matches!(
            ploop.gate_phases[1],
            VerificationGate::Deslop { .. }
        ));

        // Journal tracked the changed file from the runner.
        assert!(ploop
            .journal
            .changed_files
            .iter()
            .any(|p| p == "src/cleaned.rs"));
    }

    #[test]
    fn verify_after_pass_rollback_on_regression_failure() {
        let mut plan = ExecutionPlan::new("rollback");
        plan.add_story(
            Story::new(
                "S1",
                "do work",
                vec![AcceptanceCriterion::new("builds clean")],
            )
            .with_max_attempts(3),
        );
        let mut ploop = PersistentLoop::new(plan, LoopConfig::default());
        ploop.advance();
        ploop.plan.story_mut("S1").unwrap().criteria[0].mark_met("ok");
        ploop.complete_current_story();
        // After complete_current_story, current_story_id is cleared.  We re-attach
        // it here to simulate a caller that still holds the story context.
        ploop.current_story_id = Some("S1".to_string());

        let runner = Arc::new(FlakyDeslopRunner::new(true, vec![PathBuf::from("x.rs")]));
        ploop = ploop.with_deslop_runner(runner);

        let outcome = ploop
            .verify_after_pass(&[PathBuf::from("x.rs")])
            .expect("runner returned Ok even though regression_passed=false");
        assert!(!outcome.regression_passed);

        // Phase log still shows the two phases; the Deslop phase's outcome is
        // a failure.
        assert_eq!(ploop.gate_phases.len(), 2);
        match &ploop.gate_phases[1] {
            VerificationGate::Deslop { outcome } => {
                assert!(!outcome.regression_passed);
                assert!(outcome.regression_details.contains("regression"));
            }
            other => panic!("expected Deslop phase, got {:?}", other),
        }

        // Rollback side-effect: current story is now failed, journal has the
        // failure-cause learning.
        assert!(ploop
            .journal
            .failure_causes()
            .iter()
            .any(|e| e.insight.contains("post-deslop regression failed")));
    }

    #[test]
    fn pass_to_deslop_transition_is_valid() {
        // Build a fresh loop and directly exercise the phase log ordering.
        let plan = ExecutionPlan::new("ordering");
        let mut ploop = PersistentLoop::new(plan, LoopConfig::default());
        ploop = ploop.with_deslop_runner(Arc::new(NoopDeslopRunner));

        let _ = ploop.verify_after_pass(&[]).expect("noop succeeds");

        // Must be exactly: Pass then Deslop, in that order.
        assert_eq!(ploop.gate_phases.len(), 2);
        assert!(
            matches!(ploop.gate_phases[0], VerificationGate::Pass),
            "phase 0 must be Pass"
        );
        assert!(
            matches!(ploop.gate_phases[1], VerificationGate::Deslop { .. }),
            "phase 1 must be Deslop"
        );
    }

    #[test]
    fn builder_overrides_default_runner() {
        let plan = ExecutionPlan::new("builder");
        let ploop = PersistentLoop::new(plan, LoopConfig::default());
        // Default runner: NoopDeslopRunner (Debug prints "NoopDeslopRunner").
        let dbg_default = format!("{:?}", ploop.deslop_runner);
        assert!(
            dbg_default.contains("NoopDeslopRunner"),
            "default runner should be NoopDeslopRunner, got {}",
            dbg_default
        );

        let custom = Arc::new(ErroringDeslopRunner);
        let ploop = ploop.with_deslop_runner(custom);
        let dbg_custom = format!("{:?}", ploop.deslop_runner);
        assert!(
            dbg_custom.contains("ErroringDeslopRunner"),
            "builder should have swapped runner, got {}",
            dbg_custom
        );
    }

    #[test]
    fn multiple_deslop_cycles_preserve_iteration_state() {
        let mut plan = ExecutionPlan::new("multi-cycle");
        plan.add_story(Story::new(
            "S1",
            "work",
            vec![AcceptanceCriterion::new("ok")],
        ));
        let mut ploop = PersistentLoop::new(plan, LoopConfig::default());

        ploop.advance();
        ploop.plan.story_mut("S1").unwrap().criteria[0].mark_met("green");
        ploop.complete_current_story();

        let runner = Arc::new(FlakyDeslopRunner::new(false, vec![]));
        ploop = ploop.with_deslop_runner(runner.clone());

        let iter_before = ploop.current_iteration;
        // Run the Pass→Deslop sequence three times in a row.
        for _ in 0..3 {
            let out = ploop.verify_after_pass(&[]).expect("noop path");
            assert!(out.regression_passed);
        }
        // Iteration counter is NOT touched by verify_after_pass; it's only
        // advanced by `advance()`.
        assert_eq!(ploop.current_iteration, iter_before);
        assert_eq!(runner.calls(), 3);
        // Each cycle appends two phase entries (Pass + Deslop).
        assert_eq!(ploop.gate_phases.len(), 6);
    }

    #[test]
    fn deslop_runner_error_propagates() {
        let plan = ExecutionPlan::new("err path");
        let mut ploop = PersistentLoop::new(plan, LoopConfig::default());
        ploop = ploop.with_deslop_runner(Arc::new(ErroringDeslopRunner));

        let err = ploop
            .verify_after_pass(&[PathBuf::from("x.rs")])
            .expect_err("runner error should propagate");
        assert!(err.message.contains("cargo test"));

        // The Pass phase was logged before the error; the Deslop phase was NOT
        // (because the runner never returned a verdict).
        assert_eq!(ploop.gate_phases.len(), 1);
        assert!(matches!(ploop.gate_phases[0], VerificationGate::Pass));
    }

    #[test]
    fn deslop_outcome_and_error_display_helpers() {
        // DeslopOutcome::clean_noop sanity.
        let noop = DeslopOutcome::clean_noop();
        assert!(noop.regression_passed);
        assert!(noop.changed_files.is_empty());

        // DeslopError Display format is stable.
        let e = DeslopError::new("spawn denied");
        assert_eq!(format!("{}", e), "deslop error: spawn denied");

        // VerificationGate helpers: Pass is terminal; Fail is a failure;
        // InProgress is neither; Deslop depends on regression_passed.
        assert!(VerificationGate::Pass.is_terminal_pass());
        assert!(!VerificationGate::Pass.is_failure());
        assert!(VerificationGate::Fail.is_failure());
        assert!(!VerificationGate::Fail.is_terminal_pass());
        assert!(!VerificationGate::InProgress.is_terminal_pass());
        assert!(!VerificationGate::InProgress.is_failure());

        let degraded = VerificationGate::Deslop {
            outcome: DeslopOutcome {
                changed_files: vec![],
                regression_passed: false,
                regression_details: "broken".to_string(),
            },
        };
        assert!(degraded.is_failure());
        assert!(!degraded.is_terminal_pass());
    }

    // -------------------------------------------------------------------------
    // Sprint D1: VerifierKind / AcceptanceCriterion.verifier serde tests
    // -------------------------------------------------------------------------

    #[test]
    fn acceptance_criterion_default_verifier_is_none() {
        let c = AcceptanceCriterion::new("Some thing must be true");
        assert!(
            c.verifier.is_none(),
            "AcceptanceCriterion::new must default verifier to None"
        );
    }

    #[test]
    fn acceptance_criterion_serde_roundtrip_with_no_verifier() {
        let c = AcceptanceCriterion::new("plain criterion");
        let json = serde_json::to_string(&c).expect("serialize");
        let back: AcceptanceCriterion = serde_json::from_str(&json).expect("deserialize roundtrip");
        assert!(back.verifier.is_none());
        assert_eq!(back.description, "plain criterion");
    }

    #[test]
    fn acceptance_criterion_backward_compat_missing_verifier_field() {
        // Sprint D1 backward compat: a JSON payload from before the verifier
        // field existed must still deserialize cleanly with verifier = None.
        let legacy = r#"{
            "description": "legacy criterion",
            "met": false,
            "evidence": null,
            "verified_at": null
        }"#;
        let c: AcceptanceCriterion =
            serde_json::from_str(legacy).expect("legacy payload must decode");
        assert!(c.verifier.is_none());
        assert_eq!(c.description, "legacy criterion");
    }

    #[test]
    fn verifier_kind_all_variants_serde_roundtrip() {
        // Exercises every VerifierKind variant through JSON serde.
        let cases: Vec<VerifierKind> = vec![
            VerifierKind::FileExists {
                path: "/tmp/out.txt".to_string(),
            },
            VerifierKind::FileMimeMatches {
                path: "/tmp/out.png".to_string(),
                expected: "image/png".to_string(),
            },
            VerifierKind::FileNonEmpty {
                path: "/tmp/out.bin".to_string(),
            },
            VerifierKind::AudioDurationGt {
                path: "/tmp/voice.wav".to_string(),
                min_secs: 1.5,
            },
            VerifierKind::TextNonEmpty {
                path: "/tmp/notes.md".to_string(),
            },
            VerifierKind::NumericMatchesCsv {
                csv_path: "/tmp/data.csv".to_string(),
                formula: "sum(col_a)".to_string(),
                tolerance: 0.01,
            },
            VerifierKind::ScriptExitZero {
                script: "pytest".to_string(),
                args: vec!["-q".to_string(), "tests/".to_string()],
                cwd: "/tmp/proj".to_string(),
            },
            VerifierKind::LlmSemanticMatch {
                reference: "the answer is 42".to_string(),
                actual_path: "/tmp/answer.txt".to_string(),
                threshold: 0.85,
            },
        ];

        for v in cases {
            let json = serde_json::to_string(&v).expect("serialize verifier");
            let back: VerifierKind = serde_json::from_str(&json).expect("deserialize verifier");
            assert_eq!(v, back, "roundtrip changed value: {:?}", v);
            // Tag must be present (serde tag = "type", snake_case).
            assert!(json.contains("\"type\""), "missing tag in {}", json);
        }
    }

    #[test]
    fn acceptance_criterion_with_each_verifier_variant_roundtrips() {
        for v in [
            VerifierKind::FileExists {
                path: "p".to_string(),
            },
            VerifierKind::ScriptExitZero {
                script: "cargo".to_string(),
                args: vec!["test".to_string()],
                cwd: ".".to_string(),
            },
        ] {
            let c = AcceptanceCriterion::new("crit").with_verifier(v.clone());
            let json = serde_json::to_string(&c).expect("serialize");
            let back: AcceptanceCriterion = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back.verifier.as_ref(), Some(&v));
        }
    }

    // -------------------------------------------------------------------------
    // Sprint D1: Story.capability_id / Story.source serde tests
    // -------------------------------------------------------------------------

    #[test]
    fn story_default_capability_fields_are_none() {
        let s = Story::new("S1", "desc", vec![]);
        assert!(s.capability_id.is_none());
        assert!(s.source.is_none());
    }

    #[test]
    fn story_serde_roundtrip_with_capability_id_and_source() {
        let cap =
            CapabilityId::from_string("voice.transcribe".to_string()).expect("valid capability id");
        let conn = ConnectorId::from_string("voice".to_string()).expect("valid connector id");
        let s = Story::new("US-007", "Transcribe voice memo", vec![])
            .with_capability_id(cap.clone())
            .with_source(CapabilitySource::Native {
                connector: conn.clone(),
            });

        let json = serde_json::to_string(&s).expect("serialize");
        let back: Story = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(
            back.capability_id.as_ref().map(|c| c.as_str().to_string()),
            Some(cap.as_str().to_string())
        );
        match back.source.as_ref().expect("source set") {
            CapabilitySource::Native { connector } => {
                assert_eq!(connector.as_str(), conn.as_str())
            }
            other => panic!("expected Native, got {:?}", other),
        }
    }

    #[test]
    fn story_backward_compat_missing_capability_fields() {
        // Sprint D1 backward compat: a Story JSON payload from before the
        // capability_id / source fields existed must still deserialize cleanly
        // with both fields = None.
        let legacy = r#"{
            "id": "S1",
            "description": "old story",
            "criteria": [],
            "state": "Pending",
            "priority": 100,
            "attempt_count": 0,
            "max_attempts": 3,
            "depends_on": [],
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z"
        }"#;
        let s: Story = serde_json::from_str(legacy).expect("legacy story must decode");
        assert!(s.capability_id.is_none());
        assert!(s.source.is_none());
    }

    #[test]
    fn capability_source_all_variants_serde_roundtrip() {
        let conn = ConnectorId::from_string("voice".to_string()).expect("valid connector id");
        let skill = SkillId::from_string("powerpoint".to_string()).expect("valid skill id");
        let cases: Vec<CapabilitySource> = vec![
            CapabilitySource::Native { connector: conn },
            CapabilitySource::InstalledSkill { skill },
            CapabilitySource::SkillHub {
                name: "doc-extract".to_string(),
                install_required: true,
            },
            CapabilitySource::ProviderModality {
                provider: "openai".to_string(),
                api: "tts".to_string(),
            },
            CapabilitySource::CmdRuntime {
                binary: "ffmpeg".to_string(),
                args: vec!["-i".to_string(), "in.mp3".to_string()],
            },
            CapabilitySource::CapabilityRequest {
                request_id: "req-123".to_string(),
            },
        ];
        for src in cases {
            let json = serde_json::to_string(&src).expect("serialize source");
            let back: CapabilitySource = serde_json::from_str(&json).expect("deserialize source");
            assert_eq!(src, back, "roundtrip changed value: {:?}", src);
            assert!(json.contains("\"type\""), "missing tag in {}", json);
        }
    }

    #[test]
    fn gate_phases_serde_is_stable_and_backcompat() {
        // Old snapshot without gate_phases still deserializes (skip_serializing_if
        // + default). We also verify the new field serializes with snake_case
        // variant tags for forward-compat.
        let plan_json =
            r#"{"goal":"legacy","stories":[],"created_at":"2024-01-01T00:00:00Z","metadata":{}}"#;
        let plan: ExecutionPlan = serde_json::from_str(plan_json).unwrap();
        let ploop = PersistentLoop::new(plan, LoopConfig::default());

        // Empty gate_phases should NOT appear in the serialized form.
        let dumped = serde_json::to_string(&ploop).expect("serialize ploop");
        assert!(
            !dumped.contains("gate_phases"),
            "empty gate_phases must be skipped: {}",
            dumped
        );

        // Roundtrip with a populated gate_phases.
        let mut ploop2 = ploop.clone();
        ploop2.gate_phases.push(VerificationGate::Pass);
        ploop2.gate_phases.push(VerificationGate::Deslop {
            outcome: DeslopOutcome::clean_noop(),
        });
        let dumped2 = serde_json::to_string(&ploop2).expect("serialize ploop2");
        assert!(dumped2.contains("gate_phases"));
        assert!(dumped2.contains("\"pass\""));
        assert!(dumped2.contains("\"deslop\""));
        let back: PersistentLoop = serde_json::from_str(&dumped2).expect("deserialize");
        assert_eq!(back.gate_phases.len(), 2);
    }

    // -------------------------------------------------------------------------
    // Sprint D3: PersistentLoop::execute orchestration
    // -------------------------------------------------------------------------

    /// Verifier executor that always returns Pass with constant evidence —
    /// used to drive the loop to completion in tests.
    #[derive(Debug)]
    struct AlwaysPassVerifier;

    #[async_trait]
    impl VerifierExecutor for AlwaysPassVerifier {
        async fn run(&self, _kind: &VerifierKind, _ctx: &VerifierContext) -> VerifierVerdict {
            VerifierVerdict::Pass {
                evidence: "always-pass".to_string(),
            }
        }
    }

    /// Verifier that fails N times then passes (per-call counter).
    #[derive(Debug)]
    struct FlakyVerifier {
        fails_remaining: std::sync::Mutex<u32>,
    }

    #[async_trait]
    impl VerifierExecutor for FlakyVerifier {
        async fn run(&self, _kind: &VerifierKind, _ctx: &VerifierContext) -> VerifierVerdict {
            let mut g = self.fails_remaining.lock().unwrap();
            if *g > 0 {
                *g -= 1;
                VerifierVerdict::Fail {
                    reason: format!("flaky: {} fails left", *g + 1),
                }
            } else {
                VerifierVerdict::Pass {
                    evidence: "recovered".to_string(),
                }
            }
        }
    }

    fn make_verified_story(id: &str, deps: &[&str]) -> Story {
        let mut s = Story::new(
            id,
            format!("story {}", id),
            vec![
                AcceptanceCriterion::new("must verify").with_verifier(VerifierKind::FileExists {
                    path: "doesnt-matter".to_string(),
                }),
            ],
        );
        for d in deps {
            s = s.with_dependency(*d);
        }
        s
    }

    #[tokio::test]
    async fn execute_topological_order_runs_3_story_chain() {
        // Sprint D3: A→B→C chain; with an always-pass verifier all three
        // complete and stories_failed is empty.
        let mut plan = ExecutionPlan::new("D3 chain");
        plan.add_story(make_verified_story("A", &[]));
        plan.add_story(make_verified_story("B", &["A"]));
        plan.add_story(make_verified_story("C", &["B"]));

        let ploop = PersistentLoop::new(plan.clone(), LoopConfig::default())
            .with_verifier_executor(Arc::new(AlwaysPassVerifier));

        let mut ctx = ExecutionContext::default();
        let result = ploop
            .execute(&plan, &mut ctx)
            .await
            .expect("topological execute must succeed");

        assert_eq!(
            result.stories_completed,
            vec!["A".to_string(), "B".to_string(), "C".to_string()],
            "stories must complete in topological order"
        );
        assert!(result.stories_failed.is_empty());
        assert_eq!(result.verification_evidence.len(), 3);
    }

    #[tokio::test]
    async fn execute_dependency_cycle_returns_error() {
        // Sprint D3: A→B→A cycle is rejected by Kahn detection.
        let mut plan = ExecutionPlan::new("D3 cycle");
        plan.add_story(make_verified_story("A", &["B"]));
        plan.add_story(make_verified_story("B", &["A"]));

        let ploop = PersistentLoop::new(plan.clone(), LoopConfig::default())
            .with_verifier_executor(Arc::new(AlwaysPassVerifier));

        let mut ctx = ExecutionContext::default();
        let err = ploop.execute(&plan, &mut ctx).await.unwrap_err();
        match err {
            PersistentExecutionError::DependencyCycle { stories } => {
                assert!(stories.contains(&"A".to_string()));
                assert!(stories.contains(&"B".to_string()));
            }
            other => panic!("expected DependencyCycle, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn execute_retries_until_max_attempts() {
        // Sprint D3: a story with a flaky verifier (fails twice) and
        // max_attempts=3 must eventually pass on the third attempt.
        let mut plan = ExecutionPlan::new("D3 retry");
        let mut s = make_verified_story("S1", &[]);
        s.max_attempts = 3;
        plan.add_story(s);

        let verifier = Arc::new(FlakyVerifier {
            fails_remaining: std::sync::Mutex::new(2),
        });
        let ploop = PersistentLoop::new(plan.clone(), LoopConfig::default())
            .with_verifier_executor(verifier);

        let mut ctx = ExecutionContext::default();
        let result = ploop
            .execute(&plan, &mut ctx)
            .await
            .expect("flaky-then-pass must succeed");
        assert_eq!(result.stories_completed, vec!["S1".to_string()]);
        assert!(result.stories_failed.is_empty());
    }

    #[tokio::test]
    async fn execute_default_noop_verifier_returns_fail() {
        // Sprint D3: default NoopVerifierExecutor must Fail every story so
        // stories_failed contains S1 and stories_completed is empty.
        let mut plan = ExecutionPlan::new("D3 default-noop");
        let mut s = make_verified_story("S1", &[]);
        s.max_attempts = 1;
        plan.add_story(s);

        let ploop = PersistentLoop::new(plan.clone(), LoopConfig::default());

        let mut ctx = ExecutionContext::default();
        let result = ploop
            .execute(&plan, &mut ctx)
            .await
            .expect("execute itself must not error");
        assert_eq!(result.stories_failed, vec!["S1".to_string()]);
        assert!(result.stories_completed.is_empty());
    }

    #[tokio::test]
    async fn execute_custom_pass_verifier_yields_completion() {
        // Sprint D3: a single story with a passing verifier completes and
        // emits an evidence entry for its criterion.
        let mut plan = ExecutionPlan::new("D3 single");
        plan.add_story(make_verified_story("S1", &[]));

        let ploop = PersistentLoop::new(plan.clone(), LoopConfig::default())
            .with_verifier_executor(Arc::new(AlwaysPassVerifier));

        let mut ctx = ExecutionContext::default();
        let result = ploop.execute(&plan, &mut ctx).await.unwrap();
        assert_eq!(result.stories_completed, vec!["S1".to_string()]);
        let evidences = result
            .verification_evidence
            .get("S1")
            .expect("evidence captured");
        assert_eq!(evidences.len(), 1);
        assert_eq!(evidences[0], "always-pass");
    }

    #[tokio::test]
    async fn execute_unknown_dependency_is_rejected() {
        // Sprint D3: a story that depends on a non-existent id must error
        // out before any work runs.
        let mut plan = ExecutionPlan::new("D3 missing dep");
        plan.add_story(make_verified_story("S1", &["ghost"]));

        let ploop = PersistentLoop::new(plan.clone(), LoopConfig::default())
            .with_verifier_executor(Arc::new(AlwaysPassVerifier));
        let mut ctx = ExecutionContext::default();
        let err = ploop.execute(&plan, &mut ctx).await.unwrap_err();
        assert!(
            matches!(
                err,
                PersistentExecutionError::UnknownDependency { ref story, ref missing }
                    if story == "S1" && missing == "ghost"
            ),
            "expected UnknownDependency, got {:?}",
            err
        );
    }

    #[tokio::test]
    async fn topological_order_respects_input_order_for_independent_nodes() {
        // Sprint D3: independent stories appear in their input order in the
        // sorted output (deterministic Kahn seed).
        let mut plan = ExecutionPlan::new("D3 independent");
        plan.add_story(make_verified_story("X", &[]));
        plan.add_story(make_verified_story("Y", &[]));
        plan.add_story(make_verified_story("Z", &[]));

        let order = topological_order(&plan).expect("no cycle");
        assert_eq!(
            order,
            vec!["X".to_string(), "Y".to_string(), "Z".to_string()]
        );
    }

    #[tokio::test]
    async fn execute_no_verifier_means_auto_pass() {
        // Sprint D3: when a criterion has no `verifier` configured, the loop
        // treats it as auto-pass to preserve backward-compat with stories
        // authored before VerifierKind existed.
        let mut plan = ExecutionPlan::new("D3 auto-pass");
        let s = Story::new(
            "S1",
            "no verifier criterion",
            vec![AcceptanceCriterion::new("legacy criterion")],
        );
        plan.add_story(s);

        let ploop = PersistentLoop::new(plan.clone(), LoopConfig::default());
        let mut ctx = ExecutionContext::default();
        let result = ploop.execute(&plan, &mut ctx).await.unwrap();
        assert_eq!(result.stories_completed, vec!["S1".to_string()]);
    }

    #[tokio::test]
    async fn execute_dependent_story_skipped_when_predecessor_failed() {
        // Sprint D3: B depends on A; A fails (default noop verifier) → B
        // is recorded as failed-with-skipped (no work runs for B).
        let mut plan = ExecutionPlan::new("D3 cascade");
        let mut a = make_verified_story("A", &[]);
        a.max_attempts = 1;
        plan.add_story(a);
        plan.add_story(make_verified_story("B", &["A"]));

        let ploop = PersistentLoop::new(plan.clone(), LoopConfig::default()); // noop verifier
        let mut ctx = ExecutionContext::default();
        let result = ploop.execute(&plan, &mut ctx).await.unwrap();
        assert!(result.stories_completed.is_empty());
        assert_eq!(
            result.stories_failed,
            vec!["A".to_string(), "B".to_string()]
        );
    }

    #[tokio::test]
    async fn execute_capability_dispatch_is_invoked_when_configured() {
        // Sprint D3: a story with capability_id + Native source should result
        // in exactly one dispatch call before the verifier is consulted.
        use std::sync::atomic::{AtomicUsize, Ordering};

        #[derive(Debug)]
        struct CountingSink {
            calls: AtomicUsize,
        }

        #[async_trait]
        impl CapabilityDispatchSink for CountingSink {
            async fn dispatch(
                &self,
                _connector_id: &ConnectorId,
                _capability_id: &CapabilityId,
                _input: serde_json::Value,
            ) -> Result<serde_json::Value, String> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::Value::Null)
            }
        }

        let mut plan = ExecutionPlan::new("D3 dispatch");
        let cap = CapabilityId::from_string("voice.transcribe".to_string()).unwrap();
        let conn = ConnectorId::from_string("voice".to_string()).unwrap();
        let s = make_verified_story("S1", &[])
            .with_capability_id(cap)
            .with_source(CapabilitySource::Native { connector: conn });
        plan.add_story(s);

        let sink = Arc::new(CountingSink {
            calls: AtomicUsize::new(0),
        });
        let ploop = PersistentLoop::new(plan.clone(), LoopConfig::default())
            .with_verifier_executor(Arc::new(AlwaysPassVerifier))
            .with_capability_dispatcher(sink.clone());

        let mut ctx = ExecutionContext::default();
        ploop.execute(&plan, &mut ctx).await.unwrap();
        assert_eq!(
            sink.calls.load(Ordering::SeqCst),
            1,
            "capability_id must be dispatched once per successful story attempt"
        );
    }
}
