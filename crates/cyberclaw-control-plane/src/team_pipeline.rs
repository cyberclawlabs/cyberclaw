//! Team Staged Pipeline (Sprint 9 L1).
//!
//! Implements the `team-plan → team-prd → team-exec → team-verify → team-fix`
//! staged pipeline borrowed from `oh-my-claudecode/skills/team/SKILL.md`. The
//! pipeline is a declarative state machine that runs one stage at a time,
//! collects artifacts, and (on verify failure) enters a bounded fix loop.
//!
//! # Architectural placement
//!
//! Per EVOLUTION_IDIOMS §1 this module is **plan-in + events-out**: it takes a
//! declarative [`TeamPipelineConfig`], dispatches every stage through an
//! injected [`PipelineStageRunner`] trait, and surfaces a
//! [`PipelineOutcome`]. The coordinator never calls a connector, spawns an
//! agent, or touches a capability directly — all I/O flows through the trait.
//! This preserves CyberClaw's five-object boundary (Agent / Skill / Connector /
//! Capability / Platform Plugin): the pipeline is a coordinator, not a sixth
//! object.
//!
//! # Pipeline shape
//!
//! ```text
//!   team-plan ──▶ team-prd ──▶ team-exec ──▶ team-verify ──┬─ pass ─▶ done
//!                                       ▲                  │
//!                                       │                  └─ fail ──▶ team-fix
//!                                       └──────────────────────────────────┘
//!                                         (bounded by max_fix_loops)
//! ```
//!
//! Stages are arbitrary user-defined strings; the coordinator only enforces
//! prerequisite ordering and the fix-loop budget. The canonical five-stage
//! shape above is produced by [`TeamPipelineConfig::canonical`].
//!
//! # Scope (what this module does **not** do)
//!
//! - Does not own agent dispatch. The [`PipelineStageRunner`] implementation
//!   is expected to resolve `agent_role` to a concrete Agent and execute it
//!   via `Connector → Capability` — this coordinator only calls the trait.
//! - Does not own handoff generation. Stage handoff documents are the
//!   concern of [`crate::stage_handoff`]; the coordinator exposes the
//!   artifacts list so handoff generation can be layered on top.
//! - Does not own retry within a single stage. Stage-local retries are the
//!   runner's job; the coordinator only retries **verification** via the
//!   `team-fix` stage.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use cyberclaw_core::ids::AgentId;

// ============================================================================
// Data model (plan-in per EVOLUTION_IDIOMS §3)
// ============================================================================

/// Declarative definition of a single pipeline stage.
///
/// A stage executes once per pipeline pass (plus once per fix loop for the
/// configured fix stage). Prerequisite stages must have reached
/// [`StageStatus::Done`] before this stage is eligible to run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StageDef {
    /// Stage name (e.g. `"team-plan"`, `"team-exec"`). Must be unique within a
    /// [`TeamPipelineConfig`].
    pub name: String,
    /// Stage names that must have reached [`StageStatus::Done`] before this
    /// stage runs. Unknown names are a validation error.
    pub prerequisite_stages: Vec<String>,
    /// Agent role resolved by the runner. The coordinator does not inspect
    /// this — it only forwards it through [`StageInvocation::agent_role`].
    pub agent_role: AgentId,
    /// Optional per-stage wall-clock budget. Enforced by the coordinator via
    /// the runner's response; `None` means "no coordinator-side timeout".
    pub timeout_secs: Option<u32>,
    /// Whether this stage is the **verification gate**. When the verify
    /// stage reports failure the coordinator enters the fix loop. There must
    /// be at most one verify stage per pipeline.
    #[serde(default)]
    pub is_verify: bool,
    /// Whether this stage is the **fix stage** triggered after a failed
    /// verify. At most one fix stage per pipeline.
    #[serde(default)]
    pub is_fix: bool,
}

impl StageDef {
    /// Build a standard stage with no timeout and no special role.
    pub fn new(
        name: impl Into<String>,
        agent_role: AgentId,
        prerequisite_stages: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            prerequisite_stages,
            agent_role,
            timeout_secs: None,
            is_verify: false,
            is_fix: false,
        }
    }

    /// Set a per-stage timeout.
    pub fn with_timeout_secs(mut self, secs: u32) -> Self {
        self.timeout_secs = Some(secs);
        self
    }

    /// Mark this stage as the pipeline's verification gate.
    pub fn as_verify(mut self) -> Self {
        self.is_verify = true;
        self
    }

    /// Mark this stage as the pipeline's fix stage.
    pub fn as_fix(mut self) -> Self {
        self.is_fix = true;
        self
    }
}

/// Declarative pipeline configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamPipelineConfig {
    /// Stages in declaration order. The coordinator respects declaration
    /// order as a tie-breaker when multiple stages are simultaneously
    /// eligible.
    pub stages: Vec<StageDef>,
    /// Upper bound on verify→fix→verify cycles. Once exceeded the pipeline
    /// terminates with [`PipelineError::MaxFixLoopsExceeded`]. A value of `0`
    /// means "never run team-fix" — first verify failure is terminal.
    #[serde(default = "default_max_fix_loops")]
    pub max_fix_loops: u32,
}

fn default_max_fix_loops() -> u32 {
    3
}

impl TeamPipelineConfig {
    /// Build a config with custom stages.
    pub fn new(stages: Vec<StageDef>, max_fix_loops: u32) -> Self {
        Self {
            stages,
            max_fix_loops,
        }
    }

    /// The canonical `team-plan → team-prd → team-exec → team-verify → team-fix`
    /// shape. Every stage is wired to the same `agent_role` placeholder; real
    /// callers override it per stage.
    pub fn canonical(agent_role: AgentId, max_fix_loops: u32) -> Self {
        let stages = vec![
            StageDef::new("team-plan", agent_role.clone(), vec![]),
            StageDef::new("team-prd", agent_role.clone(), vec!["team-plan".into()]),
            StageDef::new("team-exec", agent_role.clone(), vec!["team-prd".into()]),
            StageDef::new("team-verify", agent_role.clone(), vec!["team-exec".into()]).as_verify(),
            StageDef::new("team-fix", agent_role, vec!["team-verify".into()]).as_fix(),
        ];
        Self::new(stages, max_fix_loops)
    }

    /// Validate shape. Returns `Err` when stage names collide, prerequisites
    /// are unknown, multiple verify/fix stages exist, or the declared stage
    /// list is empty.
    pub fn validate(&self) -> Result<(), PipelineError> {
        if self.stages.is_empty() {
            return Err(PipelineError::InvalidConfig(
                "pipeline must have at least one stage".into(),
            ));
        }
        let mut seen: HashSet<&str> = HashSet::new();
        for stage in &self.stages {
            if !seen.insert(stage.name.as_str()) {
                return Err(PipelineError::InvalidConfig(format!(
                    "duplicate stage name: {}",
                    stage.name
                )));
            }
        }
        for stage in &self.stages {
            for prereq in &stage.prerequisite_stages {
                if !seen.contains(prereq.as_str()) {
                    return Err(PipelineError::InvalidConfig(format!(
                        "stage {} has unknown prerequisite: {}",
                        stage.name, prereq
                    )));
                }
            }
        }
        let verify_count = self.stages.iter().filter(|s| s.is_verify).count();
        if verify_count > 1 {
            return Err(PipelineError::InvalidConfig(
                "at most one verify stage is allowed".into(),
            ));
        }
        let fix_count = self.stages.iter().filter(|s| s.is_fix).count();
        if fix_count > 1 {
            return Err(PipelineError::InvalidConfig(
                "at most one fix stage is allowed".into(),
            ));
        }
        Ok(())
    }

    fn verify_stage_name(&self) -> Option<&str> {
        self.stages
            .iter()
            .find(|s| s.is_verify)
            .map(|s| s.name.as_str())
    }

    fn fix_stage_name(&self) -> Option<&str> {
        self.stages
            .iter()
            .find(|s| s.is_fix)
            .map(|s| s.name.as_str())
    }
}

/// Artifact reference produced by a stage. Intentionally lightweight — the
/// coordinator stores these as opaque strings so consumers can map them to
/// file paths, ArtifactIds, or skill outputs without coupling back here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactRef {
    /// Stable identifier (e.g. file path, ArtifactId string, skill output
    /// URI). Opaque to the coordinator.
    pub id: String,
    /// Human-readable kind (`"file"`, `"prd"`, `"test-report"`).
    pub kind: String,
}

impl ArtifactRef {
    pub fn new(id: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: kind.into(),
        }
    }
}

/// Lifecycle state of a single stage execution.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum StageStatus {
    /// Not yet eligible (prerequisites pending) or not yet scheduled.
    Pending,
    /// Coordinator has invoked the runner and is awaiting a result.
    Running,
    /// Stage produced a successful outcome (for verify stages this also means
    /// the verification passed).
    Done,
    /// Stage produced a failure outcome. For verify stages this triggers the
    /// fix loop; for any other stage this terminates the pipeline.
    Failed,
}

/// Per-stage execution record maintained by the coordinator.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StageExecution {
    pub stage_name: String,
    pub status: StageStatus,
    /// Artifacts the runner reported. Ordering preserved from the runner.
    pub artifacts_produced: Vec<ArtifactRef>,
    /// Runner-supplied summary (one-line). `None` until the stage has been
    /// attempted.
    pub summary: Option<String>,
    /// When the coordinator scheduled this stage.
    pub started_at: Option<DateTime<Utc>>,
    /// When the coordinator received a terminal result.
    pub finished_at: Option<DateTime<Utc>>,
    /// Which pipeline pass produced this record. `0` is the initial pass,
    /// `1..=max_fix_loops` are fix-loop retries of the verify+fix pair.
    pub pass: u32,
}

impl StageExecution {
    fn new(stage_name: impl Into<String>, pass: u32) -> Self {
        Self {
            stage_name: stage_name.into(),
            status: StageStatus::Pending,
            artifacts_produced: Vec::new(),
            summary: None,
            started_at: None,
            finished_at: None,
            pass,
        }
    }
}

// ============================================================================
// Stage runner trait (injected per EVOLUTION_IDIOMS §1)
// ============================================================================

/// Invocation payload passed to the runner for one stage attempt.
///
/// Wraps the declarative [`StageDef`] with pipeline-level context: which pass
/// we are on, which stages have already completed (for handoff lookup), and
/// an optional budget the coordinator derived from `timeout_secs`.
#[derive(Debug, Clone)]
pub struct StageInvocation<'a> {
    pub stage: &'a StageDef,
    /// Zero for the initial pass, `1..` when this stage is being retried as
    /// part of the fix loop.
    pub pass: u32,
    /// Stage executions completed before this invocation, most-recent-first.
    /// Useful for runners that build prompts from the history.
    pub history: &'a [StageExecution],
    /// Coordinator-derived deadline. `None` when the stage has no timeout.
    pub deadline: Option<Duration>,
}

/// Outcome a runner returns for one stage attempt.
#[derive(Debug, Clone)]
pub struct StageOutcome {
    pub artifacts: Vec<ArtifactRef>,
    pub summary: String,
    /// `true` means the stage succeeded. For the verify stage this also
    /// signals "verification passed"; `false` triggers the fix loop.
    pub success: bool,
}

/// Runner trait injected into [`PipelineCoordinator`]. Implementations live
/// above the coordinator (e.g. a SubAgent-backed runner); this module does
/// not dispatch capabilities itself.
#[async_trait]
pub trait PipelineStageRunner: Send + Sync {
    async fn run_stage<'a>(
        &'a self,
        invocation: StageInvocation<'a>,
    ) -> Result<StageOutcome, PipelineError>;
}

// ============================================================================
// Errors
// ============================================================================

/// Errors the coordinator can surface. Runner-level errors are wrapped via
/// [`PipelineError::Runner`]; structural errors (bad config, deadlocks, loop
/// budget exhaustion) have dedicated variants.
#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("invalid pipeline config: {0}")]
    InvalidConfig(String),
    #[error("runner failed on stage {stage}: {message}")]
    Runner { stage: String, message: String },
    #[error("prerequisite {prereq} of stage {stage} has not completed (status: {status:?})")]
    PrerequisiteFailed {
        stage: String,
        prereq: String,
        status: StageStatus,
    },
    #[error("stage {0} exceeded its {1:?} timeout budget")]
    StageTimeout(String, Duration),
    #[error("team-fix exceeded max_fix_loops={limit}; verification never passed")]
    MaxFixLoopsExceeded { limit: u32 },
    #[error("no actionable stage — pipeline stuck")]
    Deadlock,
}

// ============================================================================
// Outcome
// ============================================================================

/// Terminal outcome surfaced by [`PipelineCoordinator::run`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineOutcome {
    /// Whether the pipeline reached a verified-Done state.
    pub verified: bool,
    /// Number of fix loops consumed (`0` means verify passed on first pass).
    pub fix_loops_used: u32,
    /// Full execution history, insertion-ordered.
    pub history: Vec<StageExecution>,
    /// When the coordinator started.
    pub started_at: DateTime<Utc>,
    /// When the coordinator finished (terminal state reached).
    pub finished_at: DateTime<Utc>,
}

// ============================================================================
// Coordinator (events-out)
// ============================================================================

/// The staged pipeline coordinator.
///
/// # Run flow (pseudocode)
///
/// ```text
/// validate(config)
/// loop {
///     pick next stage whose prerequisites are Done and not yet run this pass
///     invoke runner
///     record StageExecution (artifacts + summary)
///     if stage is verify and outcome.success == false {
///         if fix_loops_used >= max_fix_loops → MaxFixLoopsExceeded
///         run fix stage (if any)
///         reset verify+downstream to pending for a new pass
///         fix_loops_used += 1
///         continue
///     }
///     if all stages Done → return verified PipelineOutcome
///     if no actionable stage → Deadlock
/// }
/// ```
pub struct PipelineCoordinator {
    config: TeamPipelineConfig,
    runner: Box<dyn PipelineStageRunner>,
}

// Manual Debug impl — `Box<dyn PipelineStageRunner>` is not `Debug` and
// adding `Debug` as a supertrait would ripple to every impl. Skipping the
// runner field and printing its trait name is enough for test assertions
// that use `.expect_err()` on `Result<Self, _>`.
impl std::fmt::Debug for PipelineCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipelineCoordinator")
            .field("config", &self.config)
            .field("runner", &"<dyn PipelineStageRunner>")
            .finish()
    }
}

impl PipelineCoordinator {
    /// Build a coordinator from a config and runner. Validates the config
    /// eagerly so callers see shape errors at construction time.
    pub fn new(
        config: TeamPipelineConfig,
        runner: Box<dyn PipelineStageRunner>,
    ) -> Result<Self, PipelineError> {
        config.validate()?;
        Ok(Self { config, runner })
    }

    /// Drive the pipeline to a terminal state.
    pub async fn run(&self) -> Result<PipelineOutcome, PipelineError> {
        let started_at = Utc::now();
        let mut history: Vec<StageExecution> = Vec::new();
        let mut pass: u32 = 0;
        let mut fix_loops_used: u32 = 0;
        // Stages already completed in the current pass.
        let mut done_in_pass: HashSet<String> = HashSet::new();

        loop {
            // Did we finish the pipeline cleanly this pass?
            // Fix stages never enter `done_in_pass` (they're triggered on
            // verify-failure, not by regular scheduling), so the completion
            // predicate must ignore them.
            let all_non_fix_done = self
                .config
                .stages
                .iter()
                .filter(|s| !s.is_fix)
                .all(|s| done_in_pass.contains(&s.name));
            if all_non_fix_done {
                return Ok(PipelineOutcome {
                    verified: true,
                    fix_loops_used,
                    history,
                    started_at,
                    finished_at: Utc::now(),
                });
            }

            // Pick next actionable stage: prerequisites must be Done this
            // pass and the stage must not yet have run this pass.
            let next = self.pick_next(&done_in_pass);
            let Some(stage) = next else {
                return Err(PipelineError::Deadlock);
            };

            let deadline = stage.timeout_secs.map(|s| Duration::from_secs(s as u64));
            let invocation = StageInvocation {
                stage,
                pass,
                history: &history,
                deadline,
            };

            let mut exec = StageExecution::new(&stage.name, pass);
            exec.status = StageStatus::Running;
            exec.started_at = Some(Utc::now());

            let outcome = self.runner.run_stage(invocation).await.map_err(|e| {
                // Wrap opaque runner errors with the stage name for
                // actionable diagnostics.
                match e {
                    PipelineError::Runner { .. } => e,
                    other => PipelineError::Runner {
                        stage: stage.name.clone(),
                        message: other.to_string(),
                    },
                }
            })?;

            exec.finished_at = Some(Utc::now());
            exec.artifacts_produced = outcome.artifacts;
            exec.summary = Some(outcome.summary);

            // Enforce deadline after the call returns. We don't abort mid-
            // stage (runner owns cancellation); we only fail if the wall-
            // clock observed by the coordinator exceeded the budget.
            if let (Some(budget), Some(start), Some(end)) =
                (deadline, exec.started_at, exec.finished_at)
            {
                let observed = (end - start).to_std().unwrap_or(Duration::from_secs(0));
                if observed > budget {
                    exec.status = StageStatus::Failed;
                    history.push(exec);
                    return Err(PipelineError::StageTimeout(stage.name.clone(), budget));
                }
            }

            if outcome.success {
                exec.status = StageStatus::Done;
            } else {
                exec.status = StageStatus::Failed;
            }
            history.push(exec);

            if outcome.success {
                done_in_pass.insert(stage.name.clone());
                continue;
            }

            // Failure path. Verify failures enter the fix loop; other
            // failures terminate immediately.
            let is_verify = stage.is_verify;
            if !is_verify {
                return Err(PipelineError::Runner {
                    stage: stage.name.clone(),
                    message: "stage reported failure".into(),
                });
            }

            // Verify failed — check the fix-loop budget.
            if fix_loops_used >= self.config.max_fix_loops {
                return Err(PipelineError::MaxFixLoopsExceeded {
                    limit: self.config.max_fix_loops,
                });
            }

            // Run the fix stage if configured. Its prerequisites are
            // satisfied by the verify stage being Failed-this-pass; we treat
            // that as a pseudo-Done for the purpose of picking team-fix.
            if let Some(fix_name) = self.config.fix_stage_name() {
                let fix_stage = self
                    .config
                    .stages
                    .iter()
                    .find(|s| s.name == fix_name)
                    .expect("fix_stage_name returned an existing stage");
                let mut fix_exec = StageExecution::new(&fix_stage.name, pass);
                fix_exec.status = StageStatus::Running;
                fix_exec.started_at = Some(Utc::now());

                let fix_invocation = StageInvocation {
                    stage: fix_stage,
                    pass,
                    history: &history,
                    deadline: fix_stage
                        .timeout_secs
                        .map(|s| Duration::from_secs(s as u64)),
                };
                let fix_outcome =
                    self.runner
                        .run_stage(fix_invocation)
                        .await
                        .map_err(|e| match e {
                            PipelineError::Runner { .. } => e,
                            other => PipelineError::Runner {
                                stage: fix_stage.name.clone(),
                                message: other.to_string(),
                            },
                        })?;
                fix_exec.finished_at = Some(Utc::now());
                fix_exec.artifacts_produced = fix_outcome.artifacts;
                fix_exec.summary = Some(fix_outcome.summary);
                fix_exec.status = if fix_outcome.success {
                    StageStatus::Done
                } else {
                    StageStatus::Failed
                };
                history.push(fix_exec);

                if !fix_outcome.success {
                    return Err(PipelineError::Runner {
                        stage: fix_stage.name.clone(),
                        message: "fix stage reported failure".into(),
                    });
                }
            }

            // Start the next pass: reset the verify stage (and the fix stage
            // itself, which otherwise also counts toward "run this pass") so
            // they can re-run against the patched state. Non-verify
            // prerequisites stay Done; their artifacts carry forward.
            fix_loops_used += 1;
            pass += 1;
            if let Some(verify_name) = self.config.verify_stage_name() {
                done_in_pass.remove(verify_name);
            }
            if let Some(fix_name) = self.config.fix_stage_name() {
                done_in_pass.remove(fix_name);
            }
        }
    }

    /// Pick the next stage whose prerequisites are Done-this-pass and which
    /// has not yet completed this pass. Deterministic: honors declaration
    /// order. Skips the fix stage — it is triggered explicitly by verify
    /// failures, not by normal scheduling.
    fn pick_next<'a>(&'a self, done_in_pass: &HashSet<String>) -> Option<&'a StageDef> {
        for stage in &self.config.stages {
            if stage.is_fix {
                continue;
            }
            if done_in_pass.contains(&stage.name) {
                continue;
            }
            let prereq_ok = stage
                .prerequisite_stages
                .iter()
                .all(|p| done_in_pass.contains(p));
            if prereq_ok {
                return Some(stage);
            }
        }
        None
    }
}

// ============================================================================
// Small convenience helpers (intentionally private)
// ============================================================================

#[allow(dead_code)]
fn count_by_status(history: &[StageExecution]) -> HashMap<StageStatus, usize> {
    let mut counts: HashMap<StageStatus, usize> = HashMap::new();
    for exec in history {
        *counts.entry(exec.status).or_insert(0) += 1;
    }
    counts
}

// ============================================================================
// Sprint 13 S13-2: Lightweight staged-pipeline state machine
// ============================================================================
//
// The `PipelineCoordinator` above is the heavyweight, runner-driven coordinator
// (Sprint 9 L1). Sprint 13 S13-2 adds a complementary *state-machine only*
// representation of the same `team-plan → team-prd → team-exec → team-verify →
// team-fix` lifecycle: a plain enum + state struct + transition helper that
// can be owned by a [`crate::SubAgentOrchestrator`]-equivalent caller without
// pulling in a runner trait.
//
// This is the structure described in `oh-my-claudecode/skills/team/SKILL.md`
// under "Stage Progression": a stage pointer, a fix-loop counter bounded by
// `max_fix_loops`, and a list of handoff files written on each transition. It
// mirrors the shape of `PipelineCoordinator` but exposes a synchronous
// transition API for callers that only need to track "which stage am I in"
// rather than drive an end-to-end pipeline.

/// Canonical stage enum for the OMC team 5-stage pipeline. Values serialise as
/// kebab-case strings (`plan`, `prd`, `exec`, `verify`, `fix`, `done`)
/// matching the filenames the skill writes to `.omc/handoffs/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TeamStage {
    /// `team-plan` — decomposition, acceptance criteria, risk surfacing.
    Plan,
    /// `team-prd` — product/design document authored from the plan.
    Prd,
    /// `team-exec` — implementation against the PRD.
    Exec,
    /// `team-verify` — verification of the implementation. A failure here
    /// pushes the state machine into [`TeamStage::Fix`] (bounded by
    /// `max_fix_loops`).
    Verify,
    /// `team-fix` — remediation after a failed verify. Always transitions
    /// back to [`TeamStage::Verify`] to re-check the repair.
    Fix,
    /// Terminal marker — the pipeline has been explicitly declared finished
    /// by the caller via [`TeamPipelineState::complete`]. No forward
    /// transition is legal from `Done`; [`TeamStage::next`] returns `None`
    /// and [`TeamPipelineState::advance`] rejects with
    /// [`TeamPipelineError::InvalidTransition`].
    Done,
}

impl TeamStage {
    /// Compute the legal next stage under normal forward progression.
    ///
    /// - `Plan → Prd`
    /// - `Prd → Exec`
    /// - `Exec → Verify`
    /// - `Verify → Fix` (soft: the caller decides whether to go to Fix based
    ///   on whether verify succeeded; `.next()` returns `Fix` so the caller
    ///   can compare transitions cheaply, and `Verify → None` is modelled by
    ///   the caller treating a passing verify as terminal).
    /// - `Fix → Verify` (re-check after repair).
    /// - `Done → None` (terminal).
    ///
    /// Returns `None` when there is no forward transition — currently only
    /// for [`TeamStage::Done`], which is the explicit terminal marker.
    pub fn next(&self) -> Option<TeamStage> {
        match self {
            TeamStage::Plan => Some(TeamStage::Prd),
            TeamStage::Prd => Some(TeamStage::Exec),
            TeamStage::Exec => Some(TeamStage::Verify),
            TeamStage::Verify => Some(TeamStage::Fix),
            TeamStage::Fix => Some(TeamStage::Verify),
            TeamStage::Done => None,
        }
    }

    /// Lower-case kebab name matching the OMC team skill's stage IDs.
    pub fn as_str(&self) -> &'static str {
        match self {
            TeamStage::Plan => "plan",
            TeamStage::Prd => "prd",
            TeamStage::Exec => "exec",
            TeamStage::Verify => "verify",
            TeamStage::Fix => "fix",
            TeamStage::Done => "done",
        }
    }
}

/// Durable pipeline state owned by the orchestrator. Serialisable so a
/// distributed orchestrator can checkpoint between stages and resume after
/// restart.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamPipelineState {
    /// Current stage pointer. Default: [`TeamStage::Plan`].
    pub stage: TeamStage,
    /// Number of times the state machine has transitioned *into*
    /// [`TeamStage::Fix`]. Counted to enforce [`Self::max_fix_loops`].
    pub fix_loop_count: u32,
    /// Paths of the handoff files written on each transition, oldest-first.
    /// Empty in the initial state (no transition has yet occurred).
    #[serde(default)]
    pub handoff_paths: Vec<PathBuf>,
    /// Hard ceiling on `Verify → Fix` transitions. Once `fix_loop_count`
    /// exceeds this value [`TeamPipelineState::advance`] returns
    /// [`TeamPipelineError::FixLoopExceeded`]. Default: `3`.
    pub max_fix_loops: u32,
}

impl Default for TeamPipelineState {
    fn default() -> Self {
        Self {
            stage: TeamStage::Plan,
            fix_loop_count: 0,
            handoff_paths: vec![],
            max_fix_loops: 3,
        }
    }
}

impl TeamPipelineState {
    /// Construct a state with a non-default `max_fix_loops` ceiling. Useful
    /// for tests and for callers who want strict single-retry semantics.
    pub fn with_max_fix_loops(max_fix_loops: u32) -> Self {
        Self {
            max_fix_loops,
            ..Self::default()
        }
    }

    /// Advance the state machine by one step. Returns the new stage on
    /// success or an error describing why the transition was rejected.
    ///
    /// The handoff file is written to `<handoff_root>/<current_stage>.md`
    /// (matching the OMC team skill's `.omc/handoffs/` layout). The path is
    /// appended to `handoff_paths` on success.
    ///
    /// When transitioning *into* [`TeamStage::Fix`] the counter is incremented
    /// first and checked against `max_fix_loops`; overflow returns
    /// [`TeamPipelineError::FixLoopExceeded`] **before** the file is written.
    ///
    /// Calling `advance` while the state is already [`TeamStage::Done`] is
    /// rejected with [`TeamPipelineError::InvalidTransition`] — `Done` is the
    /// explicit terminal state and has no forward transition.
    pub fn advance(
        &mut self,
        reason: &str,
        handoff_root: &Path,
    ) -> Result<TeamStage, TeamPipelineError> {
        let from = self.stage;
        let to = from.next().ok_or(TeamPipelineError::InvalidTransition {
            from,
            to: from, // no forward step — surface with `to == from`
        })?;

        // Fix-loop accounting: increment counter *before* the write so an
        // overflow aborts cleanly without producing a handoff file.
        if to == TeamStage::Fix {
            let next_count = self.fix_loop_count.saturating_add(1);
            if next_count > self.max_fix_loops {
                return Err(TeamPipelineError::FixLoopExceeded(self.max_fix_loops));
            }
            self.fix_loop_count = next_count;
        }

        let handoff_path = write_team_handoff(handoff_root, from, to, reason)
            .map_err(|e| TeamPipelineError::HandoffWriteFailed(e.to_string()))?;
        self.handoff_paths.push(handoff_path);
        self.stage = to;
        Ok(to)
    }

    /// Declare the pipeline finished and transition to [`TeamStage::Done`].
    ///
    /// Unlike [`Self::advance`], `complete` is legal from any stage — the
    /// caller owns the decision that the pipeline has reached a satisfactory
    /// terminal state (e.g. verify passed, or the operator aborted). A
    /// `<current_stage>.md` handoff file is written the same way
    /// [`Self::advance`] writes one, and the path is appended to
    /// `handoff_paths`.
    ///
    /// Calling `complete` while the state is already [`TeamStage::Done`] is a
    /// no-op: no handoff is written, and `TeamStage::Done` is returned.
    pub fn complete(
        &mut self,
        reason: &str,
        handoff_root: &Path,
    ) -> Result<TeamStage, TeamPipelineError> {
        if self.stage == TeamStage::Done {
            return Ok(TeamStage::Done);
        }
        let from = self.stage;
        let to = TeamStage::Done;
        let handoff_path = write_team_handoff(handoff_root, from, to, reason)
            .map_err(|e| TeamPipelineError::HandoffWriteFailed(e.to_string()))?;
        self.handoff_paths.push(handoff_path);
        self.stage = to;
        Ok(to)
    }
}

/// Errors surfaced by [`TeamPipelineState::advance`] and related helpers.
#[derive(Debug, Error)]
pub enum TeamPipelineError {
    /// `fix_loop_count` has exceeded `max_fix_loops`. The inner value is the
    /// ceiling that was violated so callers can log it without re-reading the
    /// state.
    #[error("Fix loop exceeded max ({0})")]
    FixLoopExceeded(u32),
    /// The requested transition is not legal from the current stage. Carries
    /// both endpoints for diagnostics.
    #[error("Invalid stage transition from {from:?} to {to:?}")]
    InvalidTransition {
        /// Source stage of the rejected transition.
        from: TeamStage,
        /// Destination stage of the rejected transition.
        to: TeamStage,
    },
    /// Filesystem write of the handoff markdown failed.
    #[error("Handoff write failed: {0}")]
    HandoffWriteFailed(String),
}

/// Write a minimal handoff markdown file for a stage transition.
///
/// Layout: `<root>/<from_stage>.md`. The content is deliberately tiny —
/// one h1 with the transition, one paragraph with the reason, and a timestamp
/// — so the `PipelineCoordinator`-level [`crate::stage_handoff`] module
/// remains the canonical long-form format. Callers that want the richer
/// `StageHandoffDocument` shape should reach for `FileHandoffStore` instead.
///
/// Returns the absolute path of the file written so callers can record it in
/// [`TeamPipelineState::handoff_paths`].
pub fn write_team_handoff(
    root: &Path,
    from: TeamStage,
    to: TeamStage,
    reason: &str,
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    let path = root.join(format!("{}.md", from.as_str()));
    let body = format!(
        "# Team Handoff: {from} → {to}\n\n- Authored: {ts}\n- Reason: {reason}\n",
        from = from.as_str(),
        to = to.as_str(),
        ts = Utc::now().to_rfc3339(),
        reason = reason.trim(),
    );
    std::fs::write(&path, body.as_bytes())?;
    Ok(path)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    fn agent(name: &str) -> AgentId {
        AgentId::from_string(name.to_string()).expect("valid agent id")
    }

    /// Deterministic mock runner. Returns a scripted success/failure for each
    /// stage invocation in order. Unknown stages default to success.
    struct MockRunner {
        /// (stage_name, success) tuples consumed in order of observation.
        script: std::sync::Mutex<Vec<(String, bool)>>,
        /// Simulated wall-clock for each stage (milliseconds). If set the
        /// runner sleeps before returning — used by the timeout test.
        simulated_delay_ms: std::sync::Mutex<HashMap<String, u64>>,
        /// Number of stages invoked so far.
        invocations: AtomicU32,
    }

    impl MockRunner {
        fn new(script: Vec<(&'static str, bool)>) -> Self {
            Self {
                script: std::sync::Mutex::new(
                    script
                        .into_iter()
                        .map(|(n, s)| (n.to_string(), s))
                        .collect(),
                ),
                simulated_delay_ms: std::sync::Mutex::new(HashMap::new()),
                invocations: AtomicU32::new(0),
            }
        }

        fn with_delay(self, stage: &str, ms: u64) -> Self {
            self.simulated_delay_ms
                .lock()
                .unwrap()
                .insert(stage.to_string(), ms);
            self
        }
    }

    #[async_trait]
    impl PipelineStageRunner for MockRunner {
        async fn run_stage<'a>(
            &'a self,
            invocation: StageInvocation<'a>,
        ) -> Result<StageOutcome, PipelineError> {
            self.invocations.fetch_add(1, Ordering::SeqCst);
            let name = invocation.stage.name.clone();
            let delay = self
                .simulated_delay_ms
                .lock()
                .unwrap()
                .get(&name)
                .copied()
                .unwrap_or(0);
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }

            // Look up scripted outcome; default to success.
            let mut script = self.script.lock().unwrap();
            let success = if let Some(pos) = script.iter().position(|(n, _)| n == &name) {
                script.remove(pos).1
            } else {
                true
            };

            Ok(StageOutcome {
                artifacts: vec![ArtifactRef::new(format!("artifact-{}", name), "file")],
                summary: format!("{} ran (success={})", name, success),
                success,
            })
        }
    }

    #[tokio::test]
    async fn canonical_happy_path_completes_without_fix_loop() {
        let config = TeamPipelineConfig::canonical(agent("executor"), 3);
        let runner = Box::new(MockRunner::new(vec![
            ("team-plan", true),
            ("team-prd", true),
            ("team-exec", true),
            ("team-verify", true),
        ]));
        let coordinator = PipelineCoordinator::new(config, runner).unwrap();
        let outcome = coordinator.run().await.expect("happy path succeeds");
        assert!(outcome.verified);
        assert_eq!(outcome.fix_loops_used, 0);
        // 4 stages (fix is never invoked on happy path).
        assert_eq!(outcome.history.len(), 4);
        assert!(outcome
            .history
            .iter()
            .all(|e| e.status == StageStatus::Done));
    }

    #[tokio::test]
    async fn verify_failure_triggers_fix_loop_and_recovers() {
        let config = TeamPipelineConfig::canonical(agent("executor"), 3);
        // First verify fails, fix succeeds, second verify passes.
        let runner = Box::new(MockRunner::new(vec![
            ("team-plan", true),
            ("team-prd", true),
            ("team-exec", true),
            ("team-verify", false),
            ("team-fix", true),
            ("team-verify", true),
        ]));
        let coordinator = PipelineCoordinator::new(config, runner).unwrap();
        let outcome = coordinator.run().await.expect("fix loop recovers");
        assert!(outcome.verified);
        assert_eq!(outcome.fix_loops_used, 1);
        // plan, prd, exec, verify(fail), fix(ok), verify(ok) → 6 entries.
        assert_eq!(outcome.history.len(), 6);
        let verify_entries: Vec<_> = outcome
            .history
            .iter()
            .filter(|e| e.stage_name == "team-verify")
            .collect();
        assert_eq!(verify_entries.len(), 2);
        assert_eq!(verify_entries[0].status, StageStatus::Failed);
        assert_eq!(verify_entries[1].status, StageStatus::Done);
    }

    #[tokio::test]
    async fn verify_repeated_failure_exhausts_max_fix_loops() {
        let config = TeamPipelineConfig::canonical(agent("executor"), 1);
        // Verify always fails; fix always "succeeds" but verify still fails.
        let runner = Box::new(MockRunner::new(vec![
            ("team-plan", true),
            ("team-prd", true),
            ("team-exec", true),
            ("team-verify", false),
            ("team-fix", true),
            ("team-verify", false),
            // Second fix would run but we hit the budget first.
        ]));
        let coordinator = PipelineCoordinator::new(config, runner).unwrap();
        let err = coordinator.run().await.expect_err("budget exhausted");
        assert!(matches!(
            err,
            PipelineError::MaxFixLoopsExceeded { limit: 1 }
        ));
    }

    #[tokio::test]
    async fn stage_exceeding_timeout_is_reported() {
        let stages =
            vec![StageDef::new("only-stage", agent("executor"), vec![]).with_timeout_secs(1)];
        let config = TeamPipelineConfig::new(stages, 0);
        // Sleep 1500ms on "only-stage" to beat the 1s budget.
        let runner = Box::new(MockRunner::new(vec![]).with_delay("only-stage", 1500));
        let coordinator = PipelineCoordinator::new(config, runner).unwrap();
        let err = coordinator.run().await.expect_err("budget exceeded");
        match err {
            PipelineError::StageTimeout(name, _) => assert_eq!(name, "only-stage"),
            other => panic!("expected StageTimeout, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn non_verify_stage_failure_terminates_pipeline() {
        let stages = vec![
            StageDef::new("team-plan", agent("planner"), vec![]),
            StageDef::new("team-exec", agent("executor"), vec!["team-plan".into()]),
        ];
        let config = TeamPipelineConfig::new(stages, 3);
        // team-plan reports failure. No verify configured → terminal error.
        let runner = Box::new(MockRunner::new(vec![("team-plan", false)]));
        let coordinator = PipelineCoordinator::new(config, runner).unwrap();
        let err = coordinator
            .run()
            .await
            .expect_err("non-verify fail = terminal");
        match err {
            PipelineError::Runner { stage, .. } => assert_eq!(stage, "team-plan"),
            other => panic!("expected Runner error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn invalid_config_duplicate_names_rejected() {
        let stages = vec![
            StageDef::new("team-plan", agent("planner"), vec![]),
            StageDef::new("team-plan", agent("planner"), vec![]),
        ];
        let config = TeamPipelineConfig::new(stages, 3);
        let runner = Box::new(MockRunner::new(vec![]));
        // `PipelineCoordinator` does not derive `Debug` (its runner is a
        // boxed trait object) so we cannot use `expect_err` — match instead.
        match PipelineCoordinator::new(config, runner) {
            Ok(_) => panic!("duplicates must be rejected"),
            Err(e) => assert!(matches!(e, PipelineError::InvalidConfig(_))),
        }
    }

    #[tokio::test]
    async fn invalid_config_unknown_prerequisite_rejected() {
        let stages = vec![StageDef::new(
            "team-exec",
            agent("executor"),
            vec!["team-plan".into()],
        )];
        let config = TeamPipelineConfig::new(stages, 3);
        let runner = Box::new(MockRunner::new(vec![]));
        match PipelineCoordinator::new(config, runner) {
            Ok(_) => panic!("unknown prereq must be rejected"),
            Err(e) => assert!(matches!(e, PipelineError::InvalidConfig(_))),
        }
    }

    #[tokio::test]
    async fn zero_max_fix_loops_makes_first_verify_failure_terminal() {
        let config = TeamPipelineConfig::canonical(agent("executor"), 0);
        let runner = Box::new(MockRunner::new(vec![
            ("team-plan", true),
            ("team-prd", true),
            ("team-exec", true),
            ("team-verify", false),
        ]));
        let coordinator = PipelineCoordinator::new(config, runner).unwrap();
        let err = coordinator.run().await.expect_err("no fix budget");
        assert!(matches!(
            err,
            PipelineError::MaxFixLoopsExceeded { limit: 0 }
        ));
    }

    #[tokio::test]
    async fn artifacts_preserved_across_passes() {
        let config = TeamPipelineConfig::canonical(agent("executor"), 3);
        let runner = Box::new(MockRunner::new(vec![
            ("team-plan", true),
            ("team-prd", true),
            ("team-exec", true),
            ("team-verify", false),
            ("team-fix", true),
            ("team-verify", true),
        ]));
        let coordinator = PipelineCoordinator::new(config, runner).unwrap();
        let outcome = coordinator.run().await.unwrap();
        // Every entry has at least one artifact recorded.
        assert!(outcome
            .history
            .iter()
            .all(|e| !e.artifacts_produced.is_empty()));
        // The team-exec artifact from pass 0 is still in history after pass 1.
        let exec_artifacts: Vec<_> = outcome
            .history
            .iter()
            .filter(|e| e.stage_name == "team-exec")
            .collect();
        assert_eq!(exec_artifacts.len(), 1);
        assert_eq!(
            exec_artifacts[0].artifacts_produced[0].id,
            "artifact-team-exec"
        );
    }

    // ------------------------------------------------------------------
    // Sprint 13 S13-2 — TeamStage / TeamPipelineState tests
    // ------------------------------------------------------------------

    use tempfile::TempDir;

    #[test]
    fn team_stage_next_chain_is_plan_prd_exec_verify_fix_verify() {
        assert_eq!(TeamStage::Plan.next(), Some(TeamStage::Prd));
        assert_eq!(TeamStage::Prd.next(), Some(TeamStage::Exec));
        assert_eq!(TeamStage::Exec.next(), Some(TeamStage::Verify));
        assert_eq!(TeamStage::Verify.next(), Some(TeamStage::Fix));
        assert_eq!(TeamStage::Fix.next(), Some(TeamStage::Verify));
    }

    #[test]
    fn team_stage_as_str_maps_each_variant_to_kebab_name() {
        assert_eq!(TeamStage::Plan.as_str(), "plan");
        assert_eq!(TeamStage::Prd.as_str(), "prd");
        assert_eq!(TeamStage::Exec.as_str(), "exec");
        assert_eq!(TeamStage::Verify.as_str(), "verify");
        assert_eq!(TeamStage::Fix.as_str(), "fix");
    }

    #[test]
    fn team_pipeline_state_default_is_plan_zero_fix_loops_and_max_three() {
        let state = TeamPipelineState::default();
        assert_eq!(state.stage, TeamStage::Plan);
        assert_eq!(state.fix_loop_count, 0);
        assert!(state.handoff_paths.is_empty());
        assert_eq!(state.max_fix_loops, 3);
    }

    #[test]
    fn team_pipeline_state_serde_roundtrip_preserves_fields() {
        let original = TeamPipelineState {
            stage: TeamStage::Verify,
            fix_loop_count: 2,
            handoff_paths: vec![PathBuf::from("/tmp/plan.md"), PathBuf::from("/tmp/prd.md")],
            max_fix_loops: 5,
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let parsed: TeamPipelineState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, original);
        // Sanity: stage is kebab-case in the wire representation.
        assert!(json.contains("\"verify\""));
    }

    #[test]
    fn advance_writes_handoff_file_and_records_path() {
        let tmp = TempDir::new().unwrap();
        let mut state = TeamPipelineState::default();

        let next = state
            .advance("plan complete", tmp.path())
            .expect("plan→prd succeeds");
        assert_eq!(next, TeamStage::Prd);
        assert_eq!(state.stage, TeamStage::Prd);
        assert_eq!(state.handoff_paths.len(), 1);

        let written = &state.handoff_paths[0];
        assert!(written.exists(), "handoff file must be written to disk");
        let content = std::fs::read_to_string(written).unwrap();
        assert!(content.contains("plan → prd"));
        assert!(content.contains("plan complete"));
        // Filename is `<from_stage>.md`.
        assert_eq!(written.file_name().unwrap(), "plan.md");
    }

    #[test]
    fn advance_verify_to_fix_increments_fix_loop_counter() {
        let tmp = TempDir::new().unwrap();
        let mut state = TeamPipelineState {
            stage: TeamStage::Verify,
            ..Default::default()
        };
        state
            .advance("verify failed", tmp.path())
            .expect("verify→fix ok");
        assert_eq!(state.stage, TeamStage::Fix);
        assert_eq!(state.fix_loop_count, 1);

        // Fix → Verify does NOT bump the counter.
        state
            .advance("fix applied", tmp.path())
            .expect("fix→verify ok");
        assert_eq!(state.stage, TeamStage::Verify);
        assert_eq!(state.fix_loop_count, 1);
    }

    #[test]
    fn advance_past_max_fix_loops_returns_fix_loop_exceeded() {
        let tmp = TempDir::new().unwrap();
        let mut state = TeamPipelineState {
            stage: TeamStage::Verify,
            max_fix_loops: 2,
            ..Default::default()
        };

        // Fix loop 1: Verify → Fix.
        state.advance("fail 1", tmp.path()).unwrap();
        // Back to Verify.
        state.advance("fix 1", tmp.path()).unwrap();
        // Fix loop 2: Verify → Fix.
        state.advance("fail 2", tmp.path()).unwrap();
        assert_eq!(state.fix_loop_count, 2);
        // Back to Verify.
        state.advance("fix 2", tmp.path()).unwrap();
        // Fix loop 3 would exceed the ceiling.
        let err = state
            .advance("fail 3", tmp.path())
            .expect_err("budget exhausted");
        match err {
            TeamPipelineError::FixLoopExceeded(limit) => assert_eq!(limit, 2),
            other => panic!("expected FixLoopExceeded, got {other:?}"),
        }
        // State must remain at Verify; no partial transition.
        assert_eq!(state.stage, TeamStage::Verify);
        assert_eq!(state.fix_loop_count, 2);
    }

    #[test]
    fn advance_at_exact_boundary_max_one_succeeds_then_rejects() {
        let tmp = TempDir::new().unwrap();
        let mut state = TeamPipelineState {
            stage: TeamStage::Verify,
            max_fix_loops: 1,
            ..Default::default()
        };
        // Exactly one fix loop is legal.
        state
            .advance("first failure", tmp.path())
            .expect("first fix ok");
        assert_eq!(state.fix_loop_count, 1);
        // Back to Verify.
        state
            .advance("apply fix", tmp.path())
            .expect("fix→verify ok");
        // Second fix loop is one beyond the ceiling.
        let err = state
            .advance("second failure", tmp.path())
            .expect_err("boundary exceeded");
        assert!(matches!(err, TeamPipelineError::FixLoopExceeded(1)));
    }

    #[test]
    fn multiple_full_cycles_accumulate_fix_loop_count() {
        let tmp = TempDir::new().unwrap();
        let mut state = TeamPipelineState {
            stage: TeamStage::Verify,
            max_fix_loops: 3,
            ..Default::default()
        };
        // Three successful fix cycles must tally as 3.
        for i in 1..=3 {
            state.advance(&format!("fail {i}"), tmp.path()).unwrap();
            state.advance(&format!("repair {i}"), tmp.path()).unwrap();
        }
        assert_eq!(state.fix_loop_count, 3);
        assert_eq!(state.stage, TeamStage::Verify);
        // 4th attempt is rejected.
        assert!(state.advance("fail 4", tmp.path()).is_err());
    }

    #[test]
    fn advance_records_each_handoff_path_in_order() {
        let tmp = TempDir::new().unwrap();
        let mut state = TeamPipelineState::default();
        state.advance("p1", tmp.path()).unwrap(); // plan.md
        state.advance("p2", tmp.path()).unwrap(); // prd.md
        state.advance("p3", tmp.path()).unwrap(); // exec.md
        state.advance("p4", tmp.path()).unwrap(); // verify.md
        let names: Vec<_> = state
            .handoff_paths
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str().map(String::from)))
            .collect();
        assert_eq!(names, vec!["plan.md", "prd.md", "exec.md", "verify.md"]);
    }

    #[test]
    fn invalid_transition_when_handoff_write_fails() {
        // Point at a path that cannot be created (a file masquerading as root).
        let tmp = TempDir::new().unwrap();
        let blocker = tmp.path().join("not-a-dir");
        std::fs::write(&blocker, b"i am a file").unwrap();

        let mut state = TeamPipelineState::default();
        let err = state
            .advance("should fail", &blocker)
            .expect_err("write into a file must error");
        assert!(matches!(err, TeamPipelineError::HandoffWriteFailed(_)));
        // Stage must not have advanced.
        assert_eq!(state.stage, TeamStage::Plan);
    }

    #[test]
    fn with_max_fix_loops_builder_sets_ceiling() {
        let state = TeamPipelineState::with_max_fix_loops(7);
        assert_eq!(state.stage, TeamStage::Plan);
        assert_eq!(state.max_fix_loops, 7);
        assert_eq!(state.fix_loop_count, 0);
        assert!(state.handoff_paths.is_empty());
    }

    #[test]
    fn write_team_handoff_creates_parent_dirs_and_file() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("a/b/c");
        let path = write_team_handoff(&nested, TeamStage::Plan, TeamStage::Prd, "because").unwrap();
        assert!(path.exists());
        assert_eq!(path.file_name().unwrap(), "plan.md");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("# Team Handoff: plan → prd"));
        assert!(content.contains("- Reason: because"));
    }

    // ------------------------------------------------------------------
    // Sprint 15 S15-prep — TeamStage::Done + complete() tests
    // ------------------------------------------------------------------

    #[test]
    fn complete_transitions_any_stage_to_done() {
        let tmp = TempDir::new().unwrap();
        // From every non-terminal stage, complete() must move to Done and
        // write a handoff file named after the *source* stage.
        for stage in [
            TeamStage::Plan,
            TeamStage::Prd,
            TeamStage::Exec,
            TeamStage::Verify,
            TeamStage::Fix,
        ] {
            let mut state = TeamPipelineState {
                stage,
                ..Default::default()
            };
            let result = state
                .complete("operator declared finished", tmp.path())
                .expect("complete should succeed from any non-Done stage");
            assert_eq!(result, TeamStage::Done);
            assert_eq!(state.stage, TeamStage::Done);
            // A handoff path was recorded for this transition.
            assert!(
                !state.handoff_paths.is_empty(),
                "complete must append a handoff path"
            );
            let last = state.handoff_paths.last().unwrap();
            assert_eq!(
                last.file_name().unwrap(),
                format!("{}.md", stage.as_str()).as_str()
            );
            let body = std::fs::read_to_string(last).unwrap();
            assert!(body.contains(&format!("{} → done", stage.as_str())));
        }

        // Calling complete() from Done is a no-op: state stays Done and no
        // new handoff is written.
        let mut state = TeamPipelineState {
            stage: TeamStage::Done,
            ..Default::default()
        };
        let before = state.handoff_paths.len();
        let result = state
            .complete("already done", tmp.path())
            .expect("complete from Done is a no-op");
        assert_eq!(result, TeamStage::Done);
        assert_eq!(state.handoff_paths.len(), before);
    }

    #[test]
    fn advance_on_done_returns_invalid_transition() {
        let tmp = TempDir::new().unwrap();
        let mut state = TeamPipelineState {
            stage: TeamStage::Done,
            ..Default::default()
        };
        let err = state
            .advance("should be rejected", tmp.path())
            .expect_err("advance from Done must be invalid");
        match err {
            TeamPipelineError::InvalidTransition { from, to } => {
                assert_eq!(from, TeamStage::Done);
                assert_eq!(to, TeamStage::Done);
            }
            other => panic!("expected InvalidTransition, got {other:?}"),
        }
        // Stage must not have changed.
        assert_eq!(state.stage, TeamStage::Done);
        // next() on Done is None.
        assert!(TeamStage::Done.next().is_none());
        // Serde round-trip preserves the Done variant via kebab-case.
        let json = serde_json::to_string(&TeamStage::Done).unwrap();
        assert_eq!(json, "\"done\"");
        let back: TeamStage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, TeamStage::Done);
    }

    // Sanity check that our private helper actually compiles and counts.
    #[test]
    fn count_helper_sanity() {
        let mut a = StageExecution::new("a", 0);
        a.status = StageStatus::Done;
        let mut b = StageExecution::new("b", 0);
        b.status = StageStatus::Done;
        let mut c = StageExecution::new("c", 0);
        c.status = StageStatus::Failed;

        let history = vec![a, b, c];
        let counts = count_by_status(&history);
        assert_eq!(counts.get(&StageStatus::Done).copied().unwrap_or(0), 2);
        assert_eq!(counts.get(&StageStatus::Failed).copied().unwrap_or(0), 1);
        // silence "never used" for invocations counter in the runner mock.
        let runner = MockRunner::new(vec![]);
        assert_eq!(runner.invocations.load(Ordering::SeqCst), 0);
        let _ = Arc::new(runner);
    }
}
