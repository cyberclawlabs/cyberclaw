//! Cycle-level summary for the self-evolution outer loop.
//!
//! [`EvolutionCycleSummary`] is intentionally COARSER than
//! [`crate::evolution_orchestrator::EvolutionEvent`] (per-state-transition).
//! One summary is the condensed fingerprint of a completed `step()` run,
//! suitable for JSONL persistence and history-aware analysis.
//!
//! Shape mirrors Evolver's cycle-level event at `signals.js:38-58` (see
//! `tmp/claw-research/evolver/src/gep/signals.js`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Evolution intent category — the direction a cycle is trying to push the
/// system. Matches Evolver `intent` field (`signals.js:38-51`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleIntent {
    /// Fix errors, failures, regressions.
    Repair,
    /// Improve existing behavior (performance, clarity, reliability).
    Optimize,
    /// Add new capability or try an uncharted path.
    Innovate,
}

/// Terminal status of a cycle. Matches Evolver `outcome.status`
/// (`signals.js:108-115`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleStatus {
    /// Cycle produced measurable improvement.
    Success,
    /// Cycle completed but failed verification / evaluation.
    Failed,
    /// Cycle was skipped (no parent, solidify pending, resource gate, ...).
    Skipped,
}

/// How much the cycle touched. Matches Evolver `blast_radius`
/// (`signals.js:88-100`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlastRadius {
    pub files: u32,
    pub lines: u32,
}

impl BlastRadius {
    /// True when the cycle produced zero observable change.
    /// Stagnation threshold from Evolver `signals.js:92`.
    pub fn is_empty(&self) -> bool {
        self.files == 0 && self.lines == 0
    }
}

/// Outcome envelope — separates status from scoring so downstream analyzers
/// can branch on both dimensions independently.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CycleOutcome {
    pub status: CycleStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One evolution-cycle summary — the unit that feeds back into future
/// selection via [`crate::history_analyzer::analyze_recent`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvolutionCycleSummary {
    /// Stable identifier.
    pub id: String,
    /// Absolute timestamp when the cycle finalized.
    pub timestamp: DateTime<Utc>,
    /// What the cycle was trying to achieve.
    pub intent: CycleIntent,
    /// Signals that triggered this cycle. Convention: `bare_name` or
    /// `bare_name:detail_suffix` (see Evolver `signals.js:64-68`).
    #[serde(default)]
    pub signals: Vec<String>,
    /// Outcome — status + optional score / error.
    pub outcome: CycleOutcome,
    /// How much the cycle changed (0/0 ⇒ stagnation contribution).
    #[serde(default)]
    pub blast_radius: BlastRadius,
    /// Variant the cycle produced / inspected (opaque string ID for
    /// JSONL round-trip; orchestrator-side callers convert from
    /// [`cyberclaw_core::ids::VariantId`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant_id: Option<String>,
    /// Gene IDs exercised during the cycle. Empty until `SignalRouter` lands.
    #[serde(default)]
    pub genes_used: Vec<String>,
    /// Free-form metadata for downstream analyzers (e.g. `empty_cycle: true`).
    #[serde(default)]
    pub meta: serde_json::Map<String, serde_json::Value>,
}
