//! Verification Gate — Reviewer-Based Completion Validation
//!
//! Provides a verification layer that checks whether an execution plan's stories
//! truly meet their acceptance criteria before declaring completion.
//!
//! The gate performs:
//! 1. **Criteria evidence check**: Every criterion must have recorded evidence
//! 2. **Regression verification**: Build/test/lint commands must pass
//! 3. **Reviewer tier selection**: Automatically selects review depth based on
//!    change scope and risk
//!
//! # Example
//!
//! ```rust
//! use cyberclaw_control_plane::verification_gate::*;
//! use cyberclaw_control_plane::persistent_execution::*;
//!
//! let gate = CriteriaBasedGate::default();
//!
//! // Create a plan with all criteria met
//! let mut plan = ExecutionPlan::new("Test");
//! let mut story = Story::new("S1", "Story", vec![
//!     AcceptanceCriterion::new("Test passes"),
//! ]);
//! story.criteria[0].mark_met("exit code 0");
//! story.mark_passed();
//! plan.add_story(story);
//!
//! let journal = ProgressJournal::new();
//! let result = gate.check(&plan, &journal);
//! assert!(result.approved);
//! ```

use crate::persistent_execution::{
    ExecutionPlan, ProgressJournal, ReviewerType, StoryState, VerificationVerdict,
};
use serde::{Deserialize, Serialize};

// ============================================================================
// Reviewer Tier
// ============================================================================

/// Depth of review based on change scope and risk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ReviewerTier {
    /// Lightweight: <5 files, <100 lines, full test coverage.
    Standard,
    /// Normal: standard changes with moderate scope.
    Thorough,
    /// Deep: >20 files, security-sensitive, or architectural changes.
    Critical,
}

// ============================================================================
// Gate Configuration
// ============================================================================

/// Configuration for the verification gate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateConfig {
    /// Minimum reviewer tier (floor).
    pub min_tier: ReviewerTier,
    /// File count threshold for upgrading to Thorough tier.
    pub thorough_file_threshold: usize,
    /// File count threshold for upgrading to Critical tier.
    pub critical_file_threshold: usize,
    /// Whether to require evidence on every criterion.
    pub require_evidence: bool,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            min_tier: ReviewerTier::Standard,
            thorough_file_threshold: 5,
            critical_file_threshold: 20,
            require_evidence: true,
        }
    }
}

// ============================================================================
// Criteria-Based Gate
// ============================================================================

/// Default verification gate that checks criteria evidence.
///
/// This gate verifies:
/// 1. All stories are in `Passed` state
/// 2. Every criterion has evidence (when `require_evidence` is true)
/// 3. No stories are stuck in `InProgress` or `Failed`
#[derive(Debug, Clone)]
pub struct CriteriaBasedGate {
    /// Gate configuration.
    pub config: GateConfig,
    /// Reviewer type.
    pub reviewer_type: ReviewerType,
}

impl Default for CriteriaBasedGate {
    fn default() -> Self {
        Self {
            config: GateConfig::default(),
            reviewer_type: ReviewerType::Architect,
        }
    }
}

impl CriteriaBasedGate {
    /// Create a gate with custom config and reviewer type.
    pub fn new(config: GateConfig, reviewer_type: ReviewerType) -> Self {
        Self {
            config,
            reviewer_type,
        }
    }

    /// Determine the appropriate review tier based on change scope.
    pub fn determine_tier(&self, journal: &ProgressJournal) -> ReviewerTier {
        let file_count = journal.changed_files.len();

        let computed = if file_count >= self.config.critical_file_threshold {
            ReviewerTier::Critical
        } else if file_count >= self.config.thorough_file_threshold {
            ReviewerTier::Thorough
        } else {
            ReviewerTier::Standard
        };

        // Apply minimum tier floor
        std::cmp::max(computed, self.config.min_tier)
    }

    /// Check the plan against acceptance criteria.
    ///
    /// Returns a `VerificationVerdict` with approval/rejection and details.
    pub fn check(&self, plan: &ExecutionPlan, journal: &ProgressJournal) -> VerificationVerdict {
        let mut issues: Vec<String> = Vec::new();

        // Check 1: All stories must be in Passed state
        for story in &plan.stories {
            match story.state {
                StoryState::Passed => {
                    // Check evidence on criteria
                    if self.config.require_evidence {
                        for criterion in &story.criteria {
                            if criterion.met && criterion.evidence.is_none() {
                                issues.push(format!(
                                    "Story {}: criterion '{}' is marked met but has no evidence",
                                    story.id, criterion.description
                                ));
                            }
                        }
                    }
                }
                StoryState::Failed => {
                    issues.push(format!(
                        "Story {} is in Failed state (attempted {} times)",
                        story.id, story.attempt_count
                    ));
                }
                StoryState::InProgress => {
                    issues.push(format!(
                        "Story {} is still InProgress — not yet verified",
                        story.id
                    ));
                }
                StoryState::Pending => {
                    issues.push(format!("Story {} has not been started", story.id));
                }
                StoryState::Blocked => {
                    issues.push(format!(
                        "Story {} is blocked and cannot be completed",
                        story.id
                    ));
                }
            }
        }

        // Check 2: Plan must have at least one story
        if plan.stories.is_empty() {
            issues.push("Plan has no stories".to_string());
        }

        let tier = self.determine_tier(journal);
        let reviewer_name = format!("{:?}", self.reviewer_type);

        if issues.is_empty() {
            VerificationVerdict::approve(
                &reviewer_name,
                format!(
                    "All {} stories passed with evidence. Review tier: {:?}. {} files changed, {} learnings recorded.",
                    plan.stories.len(),
                    tier,
                    journal.changed_files.len(),
                    journal.entries.len(),
                ),
            )
        } else {
            VerificationVerdict::reject(
                &reviewer_name,
                format!(
                    "Verification failed: {} issue(s) found. Review tier: {:?}.",
                    issues.len(),
                    tier,
                ),
                issues,
            )
        }
    }
}

// ============================================================================
// Regression Verifier
// ============================================================================

/// Holds the specification for regression checks to run after completion.
///
/// This is a data structure, not an executor — the caller is responsible
/// for actually running the commands and reporting results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegressionSpec {
    /// Commands to execute for regression verification.
    pub commands: Vec<RegressionCommand>,
}

/// A single regression check command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegressionCommand {
    /// Human-readable label (e.g., "Build check").
    pub label: String,
    /// The command to execute.
    pub command: String,
    /// Whether failure of this command blocks completion.
    pub blocking: bool,
}

impl RegressionSpec {
    /// Create a default Rust project regression spec.
    pub fn rust_default() -> Self {
        Self {
            commands: vec![
                RegressionCommand {
                    label: "Format check".to_string(),
                    command: "cargo fmt --all -- --check".to_string(),
                    blocking: true,
                },
                RegressionCommand {
                    label: "Clippy lint".to_string(),
                    command: "cargo clippy --workspace --all-targets -- -D warnings".to_string(),
                    blocking: true,
                },
                RegressionCommand {
                    label: "Test suite".to_string(),
                    command: "cargo test --workspace".to_string(),
                    blocking: true,
                },
            ],
        }
    }

    /// Create a minimal regression spec with just build + test.
    pub fn minimal() -> Self {
        Self {
            commands: vec![
                RegressionCommand {
                    label: "Build".to_string(),
                    command: "cargo check --workspace".to_string(),
                    blocking: true,
                },
                RegressionCommand {
                    label: "Test".to_string(),
                    command: "cargo test --workspace".to_string(),
                    blocking: true,
                },
            ],
        }
    }
}

/// Result of a regression verification run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegressionResult {
    /// Results per command.
    pub results: Vec<CommandResult>,
    /// Whether all blocking commands passed.
    pub all_passed: bool,
}

/// Result of a single regression command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResult {
    /// The command label.
    pub label: String,
    /// Whether it passed.
    pub passed: bool,
    /// Output excerpt (truncated for storage).
    pub output_excerpt: Option<String>,
}

impl RegressionResult {
    /// Create a result from individual command results.
    pub fn from_results(results: Vec<CommandResult>, spec: &RegressionSpec) -> Self {
        let all_passed = results.iter().enumerate().all(|(i, r)| {
            if i < spec.commands.len() && spec.commands[i].blocking {
                r.passed
            } else {
                true
            }
        });
        Self {
            results,
            all_passed,
        }
    }
}

// ============================================================================
// Staged Verification Gate (HyperAgents HA-H1 + Paseo P-H1)
// ============================================================================

/// Two-stage verification gate that avoids expensive LLM validation when
/// cheap smoke tests fail.
///
/// Borrowed from HyperAgents staged evaluation (threshold 0.4 gate) and
/// Paseo Loop Service dual verification (shell check + LLM verify).
///
/// Stage 1 (Smoke): Run regression commands (cargo check/test).
///   - Pass -> proceed to Stage 2
///   - Fail -> reject immediately, skip Stage 2
///
/// Stage 2 (Full): Run criteria-based gate (evidence + story state check).
///   - Only reached if Stage 1 passes
#[derive(Debug, Clone)]
pub struct StagedVerificationGate {
    /// Stage 1: smoke test specification.
    pub smoke_spec: RegressionSpec,
    /// Stage 2: full criteria-based gate.
    pub criteria_gate: CriteriaBasedGate,
}

impl Default for StagedVerificationGate {
    fn default() -> Self {
        Self {
            smoke_spec: RegressionSpec::minimal(),
            criteria_gate: CriteriaBasedGate::default(),
        }
    }
}

/// Result of staged verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagedVerdict {
    /// Stage 1 regression result.
    pub smoke_result: RegressionResult,
    /// Stage 2 criteria verdict (None if Stage 1 failed).
    pub full_verdict: Option<VerificationVerdict>,
    /// Whether Stage 2 was skipped due to Stage 1 failure.
    pub stage2_skipped: bool,
    /// Final approval (both stages must pass).
    pub approved: bool,
}

impl StagedVerificationGate {
    /// Create a staged gate with custom components.
    pub fn new(smoke_spec: RegressionSpec, criteria_gate: CriteriaBasedGate) -> Self {
        Self {
            smoke_spec,
            criteria_gate,
        }
    }

    /// Run staged verification.
    ///
    /// The caller is responsible for executing the smoke commands and providing
    /// the `RegressionResult`. This gate does not execute shell commands itself
    /// (consistent with the existing gate pattern).
    ///
    /// If smoke passes, the criteria gate runs on the plan + journal.
    pub fn check(
        &self,
        smoke_result: RegressionResult,
        plan: &ExecutionPlan,
        journal: &ProgressJournal,
    ) -> StagedVerdict {
        if !smoke_result.all_passed {
            // Stage 1 failed — skip Stage 2
            return StagedVerdict {
                smoke_result,
                full_verdict: None,
                stage2_skipped: true,
                approved: false,
            };
        }

        // Stage 1 passed — run Stage 2
        let full_verdict = self.criteria_gate.check(plan, journal);
        let approved = full_verdict.approved;

        StagedVerdict {
            smoke_result,
            full_verdict: Some(full_verdict),
            stage2_skipped: false,
            approved,
        }
    }

    /// Get the smoke test spec (for the caller to know what commands to run).
    pub fn smoke_spec(&self) -> &RegressionSpec {
        &self.smoke_spec
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistent_execution::*;

    fn make_passed_plan() -> (ExecutionPlan, ProgressJournal) {
        let mut plan = ExecutionPlan::new("Test");
        let mut story = Story::new("S1", "Story", vec![AcceptanceCriterion::new("Test passes")]);
        story.criteria[0].mark_met("exit code 0");
        story.mark_passed();
        plan.add_story(story);

        let mut journal = ProgressJournal::new();
        journal.track_file("src/main.rs");

        (plan, journal)
    }

    #[test]
    fn test_gate_approves_clean_plan() {
        let gate = CriteriaBasedGate::default();
        let (plan, journal) = make_passed_plan();

        let verdict = gate.check(&plan, &journal);
        assert!(verdict.approved);
        assert!(verdict.issues.is_empty());
    }

    #[test]
    fn test_gate_rejects_failed_story() {
        let gate = CriteriaBasedGate::default();
        let mut plan = ExecutionPlan::new("Test");
        let mut story = Story::new("S1", "Story", vec![]);
        story.state = StoryState::Failed;
        story.attempt_count = 2;
        plan.add_story(story);

        let verdict = gate.check(&plan, &ProgressJournal::new());
        assert!(!verdict.approved);
        assert!(verdict.issues.iter().any(|i| i.contains("Failed")));
    }

    #[test]
    fn test_gate_rejects_in_progress_story() {
        let gate = CriteriaBasedGate::default();
        let mut plan = ExecutionPlan::new("Test");
        let mut story = Story::new("S1", "Story", vec![]);
        story.state = StoryState::InProgress;
        plan.add_story(story);

        let verdict = gate.check(&plan, &ProgressJournal::new());
        assert!(!verdict.approved);
        assert!(verdict.issues.iter().any(|i| i.contains("InProgress")));
    }

    #[test]
    fn test_gate_rejects_pending_story() {
        let gate = CriteriaBasedGate::default();
        let mut plan = ExecutionPlan::new("Test");
        plan.add_story(Story::new("S1", "Story", vec![]));

        let verdict = gate.check(&plan, &ProgressJournal::new());
        assert!(!verdict.approved);
    }

    #[test]
    fn test_gate_rejects_missing_evidence() {
        let gate = CriteriaBasedGate::default();
        let mut plan = ExecutionPlan::new("Test");
        let mut story = Story::new("S1", "Story", vec![AcceptanceCriterion::new("Criterion")]);
        // Mark met but without evidence (directly set met=true)
        story.criteria[0].met = true;
        story.state = StoryState::Passed;
        plan.add_story(story);

        let verdict = gate.check(&plan, &ProgressJournal::new());
        assert!(!verdict.approved);
        assert!(verdict.issues.iter().any(|i| i.contains("no evidence")));
    }

    #[test]
    fn test_gate_skips_evidence_check_when_disabled() {
        let config = GateConfig {
            require_evidence: false,
            ..GateConfig::default()
        };
        let gate = CriteriaBasedGate::new(config, ReviewerType::Architect);

        let mut plan = ExecutionPlan::new("Test");
        let mut story = Story::new("S1", "Story", vec![AcceptanceCriterion::new("C")]);
        story.criteria[0].met = true; // No evidence
        story.state = StoryState::Passed;
        plan.add_story(story);

        let verdict = gate.check(&plan, &ProgressJournal::new());
        assert!(verdict.approved);
    }

    #[test]
    fn test_gate_rejects_empty_plan() {
        let gate = CriteriaBasedGate::default();
        let plan = ExecutionPlan::new("Empty");

        let verdict = gate.check(&plan, &ProgressJournal::new());
        assert!(!verdict.approved);
        assert!(verdict.issues.iter().any(|i| i.contains("no stories")));
    }

    #[test]
    fn test_tier_determination_standard() {
        let gate = CriteriaBasedGate::default();
        let mut journal = ProgressJournal::new();
        journal.track_file("a.rs");
        journal.track_file("b.rs");

        assert_eq!(gate.determine_tier(&journal), ReviewerTier::Standard);
    }

    #[test]
    fn test_tier_determination_thorough() {
        let gate = CriteriaBasedGate::default();
        let mut journal = ProgressJournal::new();
        for i in 0..6 {
            journal.track_file(format!("file_{}.rs", i));
        }

        assert_eq!(gate.determine_tier(&journal), ReviewerTier::Thorough);
    }

    #[test]
    fn test_tier_determination_critical() {
        let gate = CriteriaBasedGate::default();
        let mut journal = ProgressJournal::new();
        for i in 0..25 {
            journal.track_file(format!("file_{}.rs", i));
        }

        assert_eq!(gate.determine_tier(&journal), ReviewerTier::Critical);
    }

    #[test]
    fn test_tier_minimum_floor() {
        let config = GateConfig {
            min_tier: ReviewerTier::Thorough,
            ..GateConfig::default()
        };
        let gate = CriteriaBasedGate::new(config, ReviewerType::Architect);
        let journal = ProgressJournal::new(); // 0 files

        // Even with 0 files, minimum floor is Thorough
        assert_eq!(gate.determine_tier(&journal), ReviewerTier::Thorough);
    }

    #[test]
    fn test_regression_spec_rust_default() {
        let spec = RegressionSpec::rust_default();
        assert_eq!(spec.commands.len(), 3);
        assert!(spec.commands.iter().all(|c| c.blocking));
    }

    #[test]
    fn test_regression_result_all_passed() {
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

        let result = RegressionResult::from_results(results, &spec);
        assert!(result.all_passed);
    }

    #[test]
    fn test_regression_result_blocking_failure() {
        let spec = RegressionSpec::minimal();
        let results = vec![
            CommandResult {
                label: "Build".to_string(),
                passed: true,
                output_excerpt: None,
            },
            CommandResult {
                label: "Test".to_string(),
                passed: false,
                output_excerpt: Some("1 test failed".to_string()),
            },
        ];

        let result = RegressionResult::from_results(results, &spec);
        assert!(!result.all_passed);
    }

    // -- StagedVerificationGate tests --

    fn make_smoke_passed() -> RegressionResult {
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

    fn make_smoke_failed() -> RegressionResult {
        let spec = RegressionSpec::minimal();
        let results = vec![
            CommandResult {
                label: "Build".to_string(),
                passed: false,
                output_excerpt: Some("error[E0308]: mismatched types".to_string()),
            },
            CommandResult {
                label: "Test".to_string(),
                passed: false,
                output_excerpt: None,
            },
        ];
        RegressionResult::from_results(results, &spec)
    }

    #[test]
    fn test_staged_smoke_pass_criteria_pass() {
        let gate = StagedVerificationGate::default();
        let (plan, journal) = make_passed_plan();

        let verdict = gate.check(make_smoke_passed(), &plan, &journal);
        assert!(verdict.approved);
        assert!(!verdict.stage2_skipped);
        assert!(verdict.full_verdict.is_some());
        assert!(verdict.full_verdict.unwrap().approved);
    }

    #[test]
    fn test_staged_smoke_fail_skips_stage2() {
        let gate = StagedVerificationGate::default();
        let (plan, journal) = make_passed_plan();

        let verdict = gate.check(make_smoke_failed(), &plan, &journal);
        assert!(!verdict.approved);
        assert!(verdict.stage2_skipped);
        assert!(verdict.full_verdict.is_none());
    }

    #[test]
    fn test_staged_smoke_pass_criteria_fail() {
        let gate = StagedVerificationGate::default();
        let mut plan = ExecutionPlan::new("Test");
        let mut story = Story::new("S1", "Story", vec![]);
        story.state = StoryState::Failed;
        plan.add_story(story);
        let journal = ProgressJournal::new();

        let verdict = gate.check(make_smoke_passed(), &plan, &journal);
        assert!(!verdict.approved);
        assert!(!verdict.stage2_skipped);
        assert!(verdict.full_verdict.is_some());
        assert!(!verdict.full_verdict.unwrap().approved);
    }

    #[test]
    fn test_staged_default_smoke_spec_is_minimal() {
        let gate = StagedVerificationGate::default();
        assert_eq!(gate.smoke_spec().commands.len(), 2);
    }

    #[test]
    fn test_staged_serde_verdict() {
        let gate = StagedVerificationGate::default();
        let (plan, journal) = make_passed_plan();
        let verdict = gate.check(make_smoke_passed(), &plan, &journal);

        let json = serde_json::to_string(&verdict).expect("serialize");
        let deser: StagedVerdict = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.approved, verdict.approved);
        assert_eq!(deser.stage2_skipped, verdict.stage2_skipped);
    }
}
