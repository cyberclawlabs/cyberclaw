//! PRD Generator — Goal-to-Stories Decomposition
//!
//! Provides structured decomposition of a high-level goal into an `ExecutionPlan`
//! with discrete stories, acceptance criteria, and dependency ordering.
//!
//! Inspired by the Ralph PRD workflow:
//! 1. Receive a goal description
//! 2. Decompose into right-sized stories (completable in one iteration)
//! 3. Generate verifiable acceptance criteria per story
//! 4. Order by dependency (foundational work first)
//! 5. Refine generic criteria into task-specific ones
//!
//! # Example
//!
//! ```rust
//! use cyberclaw_control_plane::prd_generator::*;
//! use cyberclaw_control_plane::persistent_execution::*;
//!
//! let generator = RuleBasedPrdGenerator::default();
//! let plan = generator.generate_from_stories(
//!     "Add user authentication",
//!     vec![
//!         StoryDraft::new("US-001", "Add user table")
//!             .with_criterion("Migration creates users table")
//!             .with_criterion("Table has email, password_hash columns"),
//!         StoryDraft::new("US-002", "Implement login endpoint")
//!             .with_criterion("POST /login returns JWT on valid credentials")
//!             .with_dependency("US-001"),
//!     ],
//! );
//! assert_eq!(plan.stories.len(), 2);
//! ```

use crate::persistent_execution::{AcceptanceCriterion, ExecutionPlan, Story};
use serde::{Deserialize, Serialize};

// ============================================================================
// Story Draft (input for PRD generation)
// ============================================================================

/// A draft story used as input for PRD generation.
///
/// Unlike `Story`, this is a lightweight input type that doesn't carry
/// runtime state (no `StoryState`, no `attempt_count`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoryDraft {
    /// Story identifier (e.g., "US-001").
    pub id: String,
    /// Description of what this story accomplishes.
    pub description: String,
    /// Draft acceptance criteria (plain text).
    pub criteria: Vec<String>,
    /// IDs of stories this depends on.
    pub depends_on: Vec<String>,
}

impl StoryDraft {
    /// Create a new story draft.
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            criteria: Vec::new(),
            depends_on: Vec::new(),
        }
    }

    /// Add an acceptance criterion.
    pub fn with_criterion(mut self, criterion: impl Into<String>) -> Self {
        self.criteria.push(criterion.into());
        self
    }

    /// Add a dependency on another story.
    pub fn with_dependency(mut self, story_id: impl Into<String>) -> Self {
        self.depends_on.push(story_id.into());
        self
    }
}

// ============================================================================
// Refinement Report
// ============================================================================

/// Report from a PRD refinement pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefinementReport {
    /// Number of generic criteria that were flagged.
    pub generic_criteria_found: usize,
    /// Number of stories that were flagged as too large.
    pub oversized_stories: usize,
    /// Number of quality-check criteria auto-added.
    pub quality_checks_added: usize,
    /// Warning messages for the user.
    pub warnings: Vec<String>,
}

// ============================================================================
// Generator Configuration
// ============================================================================

/// Configuration for the PRD generator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrdGeneratorConfig {
    /// Whether to automatically add quality-check criteria to every story.
    pub auto_add_quality_checks: bool,
    /// Quality check criteria to auto-add (e.g., "cargo test passes").
    pub quality_check_criteria: Vec<String>,
    /// Maximum number of criteria per story before warning.
    pub max_criteria_per_story: usize,
    /// Phrases that indicate a criterion is too generic.
    pub generic_phrases: Vec<String>,
    /// Maximum retry attempts per story (default for generated stories).
    pub default_max_attempts: u32,
}

impl Default for PrdGeneratorConfig {
    fn default() -> Self {
        Self {
            auto_add_quality_checks: true,
            quality_check_criteria: vec![
                "cargo test passes for affected crate".to_string(),
                "cargo clippy passes with zero warnings".to_string(),
            ],
            max_criteria_per_story: 10,
            generic_phrases: vec![
                "implementation is complete".to_string(),
                "code works".to_string(),
                "works correctly".to_string(),
                "good enough".to_string(),
                "everything works".to_string(),
                "no issues".to_string(),
            ],
            default_max_attempts: 3,
        }
    }
}

// ============================================================================
// Rule-Based PRD Generator
// ============================================================================

/// Default PRD generator that applies rule-based decomposition and refinement.
///
/// This generator:
/// 1. Converts `StoryDraft`s into full `Story` objects with proper state
/// 2. Auto-adds quality check criteria when configured
/// 3. Detects and flags generic criteria
/// 4. Validates dependency ordering
#[derive(Debug, Clone, Default)]
pub struct RuleBasedPrdGenerator {
    /// Generator configuration.
    pub config: PrdGeneratorConfig,
}

impl RuleBasedPrdGenerator {
    /// Create a generator with custom config.
    pub fn new(config: PrdGeneratorConfig) -> Self {
        Self { config }
    }

    /// Generate an ExecutionPlan from a goal and story drafts.
    ///
    /// This is the primary entry point. Each draft is converted into a
    /// full `Story` with proper state, and quality checks are auto-added.
    pub fn generate_from_stories(
        &self,
        goal: impl Into<String>,
        drafts: Vec<StoryDraft>,
    ) -> ExecutionPlan {
        let mut plan = ExecutionPlan::new(goal);

        for (idx, draft) in drafts.into_iter().enumerate() {
            let mut criteria: Vec<AcceptanceCriterion> = draft
                .criteria
                .into_iter()
                .map(AcceptanceCriterion::new)
                .collect();

            // Auto-add quality checks if enabled
            if self.config.auto_add_quality_checks {
                for check in &self.config.quality_check_criteria {
                    // Avoid duplicates
                    if !criteria.iter().any(|c| c.description == *check) {
                        criteria.push(AcceptanceCriterion::new(check.clone()));
                    }
                }
            }

            let mut story = Story::new(draft.id, draft.description, criteria)
                .with_priority(idx as u32 + 1)
                .with_max_attempts(self.config.default_max_attempts);

            for dep in draft.depends_on {
                story = story.with_dependency(dep);
            }

            plan.add_story(story);
        }

        plan
    }

    /// Refine an existing plan by detecting issues.
    ///
    /// Returns a report of what was found. Does NOT auto-fix generic criteria
    /// (that requires domain knowledge), but flags them for the caller.
    pub fn refine(&self, plan: &ExecutionPlan) -> RefinementReport {
        let mut report = RefinementReport {
            generic_criteria_found: 0,
            oversized_stories: 0,
            quality_checks_added: 0,
            warnings: Vec::new(),
        };

        for story in &plan.stories {
            // Check for generic criteria
            for criterion in &story.criteria {
                if self.is_generic(&criterion.description) {
                    report.generic_criteria_found += 1;
                    report.warnings.push(format!(
                        "Story {}: criterion '{}' is too generic — replace with specific, verifiable criteria",
                        story.id, criterion.description
                    ));
                }
            }

            // Check for oversized stories (too many criteria)
            if story.criteria.len() > self.config.max_criteria_per_story {
                report.oversized_stories += 1;
                report.warnings.push(format!(
                    "Story {}: has {} criteria (max {}), consider splitting",
                    story.id,
                    story.criteria.len(),
                    self.config.max_criteria_per_story
                ));
            }

            // Check for missing quality checks
            if self.config.auto_add_quality_checks {
                for check in &self.config.quality_check_criteria {
                    if !story.criteria.iter().any(|c| c.description == *check) {
                        report.quality_checks_added += 1;
                        report.warnings.push(format!(
                            "Story {}: missing quality check '{}'",
                            story.id, check
                        ));
                    }
                }
            }
        }

        // Validate dependency ordering
        self.validate_dependencies(plan, &mut report);

        report
    }

    /// Check if a criterion description matches known generic phrases.
    fn is_generic(&self, description: &str) -> bool {
        let lower = description.to_lowercase();
        self.config
            .generic_phrases
            .iter()
            .any(|phrase| lower.contains(phrase))
    }

    /// Validate that dependencies reference existing stories and form no cycles.
    fn validate_dependencies(&self, plan: &ExecutionPlan, report: &mut RefinementReport) {
        let story_ids: Vec<&str> = plan.stories.iter().map(|s| s.id.as_str()).collect();

        for story in &plan.stories {
            for dep in &story.depends_on {
                if !story_ids.contains(&dep.as_str()) {
                    report.warnings.push(format!(
                        "Story {}: depends on '{}' which does not exist in the plan",
                        story.id, dep
                    ));
                }
                if dep == &story.id {
                    report
                        .warnings
                        .push(format!("Story {}: has a self-dependency", story.id));
                }
            }
        }

        // Simple cycle detection via topological sort attempt
        if self.has_cycle(plan) {
            report.warnings.push(
                "Dependency cycle detected in story graph — stories may deadlock".to_string(),
            );
        }
    }

    /// Simple cycle detection using Kahn's algorithm.
    fn has_cycle(&self, plan: &ExecutionPlan) -> bool {
        use std::collections::{HashMap, VecDeque};

        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();

        // Initialize
        for story in &plan.stories {
            in_degree.entry(story.id.as_str()).or_insert(0);
            adjacency.entry(story.id.as_str()).or_default();
        }

        // Build graph (dependency → story means dep must come before story)
        for story in &plan.stories {
            for dep in &story.depends_on {
                if let Some(deg) = in_degree.get_mut(story.id.as_str()) {
                    *deg += 1;
                }
                adjacency
                    .entry(dep.as_str())
                    .or_default()
                    .push(story.id.as_str());
            }
        }

        // Kahn's algorithm
        let mut queue: VecDeque<&str> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(&id, _)| id)
            .collect();

        let mut visited = 0;
        while let Some(node) = queue.pop_front() {
            visited += 1;
            if let Some(neighbors) = adjacency.get(node) {
                for &neighbor in neighbors {
                    if let Some(deg) = in_degree.get_mut(neighbor) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(neighbor);
                        }
                    }
                }
            }
        }

        visited < plan.stories.len()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_story_draft_builder() {
        let draft = StoryDraft::new("US-001", "Add user table")
            .with_criterion("Migration creates table")
            .with_criterion("Table has email column")
            .with_dependency("US-000");

        assert_eq!(draft.id, "US-001");
        assert_eq!(draft.criteria.len(), 2);
        assert_eq!(draft.depends_on, vec!["US-000"]);
    }

    #[test]
    fn test_generate_from_stories_basic() {
        let gen = RuleBasedPrdGenerator::default();
        let plan = gen.generate_from_stories(
            "Test goal",
            vec![
                StoryDraft::new("S1", "First story").with_criterion("Thing works"),
                StoryDraft::new("S2", "Second story")
                    .with_criterion("Other thing works")
                    .with_dependency("S1"),
            ],
        );

        assert_eq!(plan.stories.len(), 2);
        assert_eq!(plan.goal, "Test goal");
        // Quality checks auto-added
        assert!(plan.stories[0].criteria.len() >= 3); // 1 + 2 quality checks
                                                      // Priority assigned by order
        assert_eq!(plan.stories[0].priority, 1);
        assert_eq!(plan.stories[1].priority, 2);
        // Dependency preserved
        assert_eq!(plan.stories[1].depends_on, vec!["S1"]);
    }

    #[test]
    fn test_generate_no_quality_checks() {
        let config = PrdGeneratorConfig {
            auto_add_quality_checks: false,
            ..PrdGeneratorConfig::default()
        };
        let gen = RuleBasedPrdGenerator::new(config);
        let plan = gen.generate_from_stories(
            "No checks",
            vec![StoryDraft::new("S1", "Story").with_criterion("Only criterion")],
        );

        assert_eq!(plan.stories[0].criteria.len(), 1);
    }

    #[test]
    fn test_generate_avoids_duplicate_quality_checks() {
        let gen = RuleBasedPrdGenerator::default();
        let plan = gen.generate_from_stories(
            "Dup check",
            vec![StoryDraft::new("S1", "Story")
                .with_criterion("cargo test passes for affected crate")
                .with_criterion("Custom criterion")],
        );

        // Should not duplicate the cargo test criterion
        let test_criteria_count = plan.stories[0]
            .criteria
            .iter()
            .filter(|c| c.description == "cargo test passes for affected crate")
            .count();
        assert_eq!(test_criteria_count, 1);
    }

    #[test]
    fn test_refine_detects_generic_criteria() {
        let gen = RuleBasedPrdGenerator::default();
        let plan = gen.generate_from_stories(
            "Generic test",
            vec![StoryDraft::new("S1", "Story")
                .with_criterion("Implementation is complete")
                .with_criterion("Code works correctly")
                .with_criterion("Specific: function returns 42")],
        );

        let report = gen.refine(&plan);
        assert_eq!(report.generic_criteria_found, 2);
        assert!(report.warnings.iter().any(|w| w.contains("too generic")));
    }

    #[test]
    fn test_refine_detects_oversized_stories() {
        let config = PrdGeneratorConfig {
            max_criteria_per_story: 3,
            auto_add_quality_checks: false,
            ..PrdGeneratorConfig::default()
        };
        let gen = RuleBasedPrdGenerator::new(config);

        let plan = gen.generate_from_stories(
            "Big story",
            vec![StoryDraft::new("S1", "Story")
                .with_criterion("C1")
                .with_criterion("C2")
                .with_criterion("C3")
                .with_criterion("C4")],
        );

        let report = gen.refine(&plan);
        assert_eq!(report.oversized_stories, 1);
    }

    #[test]
    fn test_refine_detects_missing_deps() {
        let gen = RuleBasedPrdGenerator::default();
        let plan = gen.generate_from_stories(
            "Bad deps",
            vec![StoryDraft::new("S1", "Story").with_dependency("NONEXISTENT")],
        );

        let report = gen.refine(&plan);
        assert!(report.warnings.iter().any(|w| w.contains("does not exist")));
    }

    #[test]
    fn test_refine_detects_self_dependency() {
        let gen = RuleBasedPrdGenerator::default();
        let plan = gen.generate_from_stories(
            "Self dep",
            vec![StoryDraft::new("S1", "Story").with_dependency("S1")],
        );

        let report = gen.refine(&plan);
        assert!(report
            .warnings
            .iter()
            .any(|w| w.contains("self-dependency")));
    }

    #[test]
    fn test_cycle_detection_no_cycle() {
        let gen = RuleBasedPrdGenerator::default();
        let plan = gen.generate_from_stories(
            "No cycle",
            vec![
                StoryDraft::new("S1", "First"),
                StoryDraft::new("S2", "Second").with_dependency("S1"),
                StoryDraft::new("S3", "Third").with_dependency("S2"),
            ],
        );

        assert!(!gen.has_cycle(&plan));
    }

    #[test]
    fn test_cycle_detection_with_cycle() {
        let gen = RuleBasedPrdGenerator::default();
        let plan = gen.generate_from_stories(
            "Has cycle",
            vec![
                StoryDraft::new("S1", "First").with_dependency("S3"),
                StoryDraft::new("S2", "Second").with_dependency("S1"),
                StoryDraft::new("S3", "Third").with_dependency("S2"),
            ],
        );

        assert!(gen.has_cycle(&plan));
        let report = gen.refine(&plan);
        assert!(report.warnings.iter().any(|w| w.contains("cycle")));
    }

    #[test]
    fn test_default_config() {
        let config = PrdGeneratorConfig::default();
        assert!(config.auto_add_quality_checks);
        assert_eq!(config.quality_check_criteria.len(), 2);
        assert_eq!(config.max_criteria_per_story, 10);
        assert!(!config.generic_phrases.is_empty());
        assert_eq!(config.default_max_attempts, 3);
    }

    #[test]
    fn test_refinement_report_clean_plan() {
        let gen = RuleBasedPrdGenerator::default();
        let plan = gen.generate_from_stories(
            "Clean plan",
            vec![StoryDraft::new("S1", "Story").with_criterion("Specific: returns 200 OK")],
        );

        let report = gen.refine(&plan);
        assert_eq!(report.generic_criteria_found, 0);
        assert_eq!(report.oversized_stories, 0);
    }

    #[test]
    fn test_max_attempts_propagated() {
        let config = PrdGeneratorConfig {
            default_max_attempts: 5,
            auto_add_quality_checks: false,
            ..PrdGeneratorConfig::default()
        };
        let gen = RuleBasedPrdGenerator::new(config);
        let plan = gen.generate_from_stories("Retry config", vec![StoryDraft::new("S1", "Story")]);

        assert_eq!(plan.stories[0].max_attempts, 5);
    }
}
