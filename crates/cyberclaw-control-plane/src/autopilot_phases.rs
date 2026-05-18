//! 6-phase Autopilot state machine (Sprint 9 leftover — Task #18).
//!
//! Introduces an explicit phase enum + transition validation + dispatcher
//! trait so future lanes can plug in phase-specific logic without
//! refactoring [`crate::autopilot_runtime::GovernedLoopRuntime`].
//!
//! Reference: `docs/implementation/2026-04-19-claw-research-comparison.md §4.4`.
//!
//! ```text
//!  ┌───────────┐  ┌──────────┐  ┌───────────┐  ┌────┐  ┌────────────┐  ┌─────────┐
//!  │ Expansion │─▶│ Planning │─▶│ Execution │─▶│ Qa │─▶│ Validation │─▶│ Cleanup │
//!  └───────────┘  └──────────┘  └───────────┘  └────┘  └────────────┘  └─────────┘
//! ```
//!
//! NOTE: This is intentionally additive. It lives alongside the legacy
//! 5-phase [`crate::autopilot_types::AutopilotPhase`] (Plan/Execute/Verify/
//! Fix/Done) used by [`drive_phase_loop`](crate::autopilot_runtime::drive_phase_loop).
//! Migration is out of scope for Task #18.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use tracing::info;

/// Explicit 6-phase model mirroring `oh-my-claudecode/skills/autopilot/SKILL.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutopilotPhase {
    /// Phase 0 — widen problem surface, gather context.
    #[default]
    Expansion,
    /// Phase 1 — break the problem down into concrete steps.
    Planning,
    /// Phase 2 — ralph / ultrawork doing the actual work.
    Execution,
    /// Phase 3 — ultraqa self-check.
    Qa,
    /// Phase 4 — parallel reviewers (architect + security-reviewer + code-reviewer).
    Validation,
    /// Phase 5 — finalize, persist, audit.
    Cleanup,
}

impl fmt::Display for AutopilotPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Expansion => f.write_str("expansion"),
            Self::Planning => f.write_str("planning"),
            Self::Execution => f.write_str("execution"),
            Self::Qa => f.write_str("qa"),
            Self::Validation => f.write_str("validation"),
            Self::Cleanup => f.write_str("cleanup"),
        }
    }
}

impl AutopilotPhase {
    /// Zero-based ordinal used for forward/backward comparisons.
    pub fn order(&self) -> u8 {
        match self {
            Self::Expansion => 0,
            Self::Planning => 1,
            Self::Execution => 2,
            Self::Qa => 3,
            Self::Validation => 4,
            Self::Cleanup => 5,
        }
    }

    /// Phase immediately following `self`, if any.
    pub fn next(&self) -> Option<Self> {
        match self {
            Self::Expansion => Some(Self::Planning),
            Self::Planning => Some(Self::Execution),
            Self::Execution => Some(Self::Qa),
            Self::Qa => Some(Self::Validation),
            Self::Validation => Some(Self::Cleanup),
            Self::Cleanup => None,
        }
    }

    /// Cleanup is the terminal phase.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Cleanup)
    }

    /// All phases in canonical order.
    pub fn all() -> [AutopilotPhase; 6] {
        [
            Self::Expansion,
            Self::Planning,
            Self::Execution,
            Self::Qa,
            Self::Validation,
            Self::Cleanup,
        ]
    }
}

/// Policy controlling whether the state machine may skip phases forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PhaseSkipPolicy {
    /// No forward skips allowed. Each phase must be visited exactly once.
    #[default]
    Strict,
    /// Forward skips allowed (e.g. jump straight from `Planning` to `Cleanup`).
    AllowForwardSkip,
    /// Forward skips and explicit backward rollback allowed.
    AllowRollback,
}

impl PhaseSkipPolicy {
    fn allows_skip(self) -> bool {
        matches!(self, Self::AllowForwardSkip | Self::AllowRollback)
    }

    fn allows_rollback(self) -> bool {
        matches!(self, Self::AllowRollback)
    }
}

/// Errors that can arise during phase transitions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PhaseError {
    /// Transition does not follow the canonical next() chain.
    #[error("invalid transition from {from} to {to}")]
    InvalidTransition {
        from: AutopilotPhase,
        to: AutopilotPhase,
    },
    /// Forward skip attempted without a permissive `PhaseSkipPolicy`.
    #[error("skip from {from} to {to} is not allowed under current policy")]
    SkipNotAllowed {
        from: AutopilotPhase,
        to: AutopilotPhase,
    },
    /// Already at terminal phase; no further transitions permitted.
    #[error("phase {0} is terminal; cannot advance further")]
    AlreadyTerminal(AutopilotPhase),
}

/// Audit record for a single phase transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PhaseTransition {
    pub from: AutopilotPhase,
    pub to: AutopilotPhase,
    pub reason: String,
    pub at: DateTime<Utc>,
}

impl PhaseTransition {
    pub fn new(from: AutopilotPhase, to: AutopilotPhase, reason: impl Into<String>) -> Self {
        Self {
            from,
            to,
            reason: reason.into(),
            at: Utc::now(),
        }
    }
}

/// Validate a prospective phase transition against a skip policy.
///
/// Semantics:
/// * Self-transitions (`from == to`) are always rejected as `InvalidTransition`.
/// * Backward transitions require [`PhaseSkipPolicy::AllowRollback`].
/// * Forward single-step transitions (`to == from.next()`) are always OK.
/// * Forward multi-step transitions require [`PhaseSkipPolicy::AllowForwardSkip`]
///   or [`PhaseSkipPolicy::AllowRollback`].
/// * Once `from.is_terminal()`, nothing is allowed → `AlreadyTerminal`.
pub fn validate_transition(
    from: AutopilotPhase,
    to: AutopilotPhase,
    policy: PhaseSkipPolicy,
) -> Result<(), PhaseError> {
    if from.is_terminal() {
        return Err(PhaseError::AlreadyTerminal(from));
    }

    if from == to {
        return Err(PhaseError::InvalidTransition { from, to });
    }

    let from_ord = from.order();
    let to_ord = to.order();

    if to_ord < from_ord {
        if policy.allows_rollback() {
            return Ok(());
        }
        return Err(PhaseError::InvalidTransition { from, to });
    }

    // Forward-only from here.
    let is_single_step = from.next() == Some(to);
    if is_single_step {
        return Ok(());
    }

    if policy.allows_skip() {
        Ok(())
    } else {
        Err(PhaseError::SkipNotAllowed { from, to })
    }
}

/// Pure helper behind `GovernedLoopRuntime::advance_phase`.
///
/// Given a current phase and skip policy, compute `(next_phase, transition_record)`
/// with canonical forward-by-one semantics. Isolated from the runtime so the
/// state transition contract can be unit-tested independently of the 8 traits
/// required to instantiate the runtime.
pub fn compute_advance_phase(
    from: AutopilotPhase,
    policy: PhaseSkipPolicy,
    reason: impl Into<String>,
) -> Result<(AutopilotPhase, PhaseTransition), PhaseError> {
    let Some(to) = from.next() else {
        return Err(PhaseError::AlreadyTerminal(from));
    };
    validate_transition(from, to, policy)?;
    let reason = reason.into();
    Ok((to, PhaseTransition::new(from, to, reason)))
}

/// Artifact produced by a phase, referenced by ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PhaseArtifact {
    pub id: String,
    pub kind: String,
}

/// Outcome surfaced by a [`AutopilotPhaseDispatcher`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum PhaseOutcome {
    /// Phase finished successfully and produced the listed artifacts.
    Completed { artifacts: Vec<PhaseArtifact> },
    /// Phase was skipped (dispatcher chose not to run anything).
    Skipped { reason: String },
    /// Phase ran but deferred its output to a later re-run.
    Deferred { reason: String },
}

/// Context handed to a [`AutopilotPhaseDispatcher`] for a single phase run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PhaseContext {
    /// Logical run identifier (string form; does not depend on core id types).
    pub run_id: String,
    /// Iteration counter for the enclosing autopilot loop.
    pub iteration: u32,
    /// Free-form goal/task description propagated from the job spec.
    pub goal: String,
}

impl PhaseContext {
    pub fn new(run_id: impl Into<String>, iteration: u32, goal: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
            iteration,
            goal: goal.into(),
        }
    }
}

/// Dispatch hook — each phase gets routed through one of these.
///
/// Real implementations are deferred; the runtime ships with
/// [`StubPhaseDispatcher`] which logs dispatch and always returns
/// [`PhaseOutcome::Completed`] with no artifacts.
pub trait AutopilotPhaseDispatcher: Send + Sync {
    fn dispatch(
        &self,
        phase: AutopilotPhase,
        ctx: &PhaseContext,
    ) -> Result<PhaseOutcome, PhaseError>;
}

/// Default dispatcher that logs and returns [`PhaseOutcome::Completed`].
///
/// Lets the state machine be exercised end-to-end before the real
/// phase agents (architect / security-reviewer / code-reviewer / …) are
/// wired up.
#[derive(Debug, Default, Clone, Copy)]
pub struct StubPhaseDispatcher;

impl AutopilotPhaseDispatcher for StubPhaseDispatcher {
    fn dispatch(
        &self,
        phase: AutopilotPhase,
        ctx: &PhaseContext,
    ) -> Result<PhaseOutcome, PhaseError> {
        info!(
            target = "autopilot.phase",
            phase = %phase,
            run_id = %ctx.run_id,
            iteration = ctx.iteration,
            "StubPhaseDispatcher dispatch"
        );
        Ok(PhaseOutcome::Completed { artifacts: vec![] })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Enum ordering + next() chain ---------------------------------------

    #[test]
    fn phase_order_is_dense_and_monotonic() {
        let phases = AutopilotPhase::all();
        for (i, p) in phases.iter().enumerate() {
            assert_eq!(p.order() as usize, i, "phase {p:?} has wrong order");
        }
    }

    #[test]
    fn next_chain_visits_every_phase_exactly_once() {
        let mut current = AutopilotPhase::Expansion;
        let mut visited = vec![current];
        while let Some(n) = current.next() {
            visited.push(n);
            current = n;
        }
        assert_eq!(
            visited,
            vec![
                AutopilotPhase::Expansion,
                AutopilotPhase::Planning,
                AutopilotPhase::Execution,
                AutopilotPhase::Qa,
                AutopilotPhase::Validation,
                AutopilotPhase::Cleanup,
            ]
        );
    }

    #[test]
    fn cleanup_is_terminal_and_has_no_next() {
        assert!(AutopilotPhase::Cleanup.is_terminal());
        assert_eq!(AutopilotPhase::Cleanup.next(), None);
        for p in AutopilotPhase::all() {
            if p != AutopilotPhase::Cleanup {
                assert!(!p.is_terminal(), "{p:?} should not be terminal");
            }
        }
    }

    // -- Transition validation ----------------------------------------------

    #[test]
    fn validate_forward_single_step_allowed_under_strict() {
        assert!(validate_transition(
            AutopilotPhase::Expansion,
            AutopilotPhase::Planning,
            PhaseSkipPolicy::Strict,
        )
        .is_ok());
        assert!(validate_transition(
            AutopilotPhase::Validation,
            AutopilotPhase::Cleanup,
            PhaseSkipPolicy::Strict,
        )
        .is_ok());
    }

    #[test]
    fn validate_backward_rejected_under_strict() {
        let err = validate_transition(
            AutopilotPhase::Execution,
            AutopilotPhase::Expansion,
            PhaseSkipPolicy::Strict,
        )
        .unwrap_err();
        match err {
            PhaseError::InvalidTransition { from, to } => {
                assert_eq!(from, AutopilotPhase::Execution);
                assert_eq!(to, AutopilotPhase::Expansion);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn validate_backward_allowed_under_rollback_policy() {
        assert!(validate_transition(
            AutopilotPhase::Execution,
            AutopilotPhase::Expansion,
            PhaseSkipPolicy::AllowRollback,
        )
        .is_ok());
    }

    #[test]
    fn validate_skip_rejected_under_strict() {
        let err = validate_transition(
            AutopilotPhase::Expansion,
            AutopilotPhase::Execution,
            PhaseSkipPolicy::Strict,
        )
        .unwrap_err();
        assert!(matches!(err, PhaseError::SkipNotAllowed { .. }));
    }

    #[test]
    fn validate_skip_allowed_under_forward_skip_policy() {
        assert!(validate_transition(
            AutopilotPhase::Planning,
            AutopilotPhase::Validation,
            PhaseSkipPolicy::AllowForwardSkip,
        )
        .is_ok());
    }

    #[test]
    fn validate_terminal_rejects_every_transition() {
        for p in AutopilotPhase::all() {
            let result =
                validate_transition(AutopilotPhase::Cleanup, p, PhaseSkipPolicy::AllowRollback);
            assert!(
                matches!(result, Err(PhaseError::AlreadyTerminal(_))),
                "expected AlreadyTerminal from Cleanup->{p:?}, got {result:?}",
            );
        }
    }

    #[test]
    fn validate_self_transition_rejected() {
        let err = validate_transition(
            AutopilotPhase::Planning,
            AutopilotPhase::Planning,
            PhaseSkipPolicy::AllowRollback,
        )
        .unwrap_err();
        assert!(matches!(err, PhaseError::InvalidTransition { .. }));
    }

    // -- Dispatcher stub ----------------------------------------------------

    #[test]
    fn stub_dispatcher_handles_every_phase() {
        let d = StubPhaseDispatcher;
        let ctx = PhaseContext::new("run-1", 0, "test goal");
        for phase in AutopilotPhase::all() {
            let outcome = d.dispatch(phase, &ctx).expect("stub must succeed");
            assert_eq!(outcome, PhaseOutcome::Completed { artifacts: vec![] });
        }
    }

    #[test]
    fn phase_transition_audit_trail_preserves_order() {
        let mut trail: Vec<PhaseTransition> = Vec::new();
        let mut current = AutopilotPhase::Expansion;
        while let Some(next) = current.next() {
            trail.push(PhaseTransition::new(
                current,
                next,
                format!("advance from {current}"),
            ));
            current = next;
        }
        assert_eq!(trail.len(), 5);
        assert_eq!(trail[0].from, AutopilotPhase::Expansion);
        assert_eq!(trail[0].to, AutopilotPhase::Planning);
        assert_eq!(trail[4].from, AutopilotPhase::Validation);
        assert_eq!(trail[4].to, AutopilotPhase::Cleanup);

        // Timestamps should be non-decreasing.
        for pair in trail.windows(2) {
            assert!(pair[0].at <= pair[1].at);
        }
    }

    #[test]
    fn phase_serde_round_trip_snake_case() {
        let phase = AutopilotPhase::Validation;
        let json = serde_json::to_string(&phase).unwrap();
        assert_eq!(json, "\"validation\"");
        let decoded: AutopilotPhase = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, phase);
    }

    #[test]
    fn phase_outcome_serde_tagged() {
        let outcome = PhaseOutcome::Completed { artifacts: vec![] };
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(json.contains("\"status\":\"completed\""));
    }

    // -- compute_advance_phase (backs AutopilotRuntime::advance_phase) -------

    #[test]
    fn compute_advance_phase_walks_full_chain() {
        let mut current = AutopilotPhase::Expansion;
        let mut history: Vec<PhaseTransition> = Vec::new();
        for expected in [
            AutopilotPhase::Planning,
            AutopilotPhase::Execution,
            AutopilotPhase::Qa,
            AutopilotPhase::Validation,
            AutopilotPhase::Cleanup,
        ] {
            let (next, t) =
                compute_advance_phase(current, PhaseSkipPolicy::Strict, "step").unwrap();
            assert_eq!(next, expected);
            assert_eq!(t.from, current);
            assert_eq!(t.to, expected);
            history.push(t);
            current = next;
        }
        assert_eq!(history.len(), 5);
        assert!(current.is_terminal());
    }

    #[test]
    fn compute_advance_phase_rejects_terminal() {
        let err = compute_advance_phase(
            AutopilotPhase::Cleanup,
            PhaseSkipPolicy::AllowRollback,
            "after done",
        )
        .unwrap_err();
        assert!(matches!(
            err,
            PhaseError::AlreadyTerminal(AutopilotPhase::Cleanup)
        ));
    }

    #[test]
    fn compute_advance_phase_records_reason_verbatim() {
        let (_, t) = compute_advance_phase(
            AutopilotPhase::Planning,
            PhaseSkipPolicy::Strict,
            "planning complete",
        )
        .unwrap();
        assert_eq!(t.reason, "planning complete");
    }
}
