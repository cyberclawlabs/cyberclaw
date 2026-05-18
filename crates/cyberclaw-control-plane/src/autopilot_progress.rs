// SPDX-License-Identifier: Apache-2.0

//! Autopilot Progress Evaluator
//!
//! This module provides progress evaluation and goal checking for Autopilot runs,
//! determining when to continue, pause, or complete execution.
//!
//! ## Architecture
//!
//! ```text
//! ┌──────────────────────┐       analyze          ┌──────────────────────┐
//! │  ExecutionResults    │ ───────────────────────>│  ProgressAnalysis    │
//! │                      │                         │  - has_progress      │
//! │                      │                         │  - progress_delta    │
//! └──────────────────────┘                         │  - decision          │
//!              │                                   └──────────────────────┘
//!              │                                             ▲
//!              ▼                                             │
//! ┌──────────────────────┐       is_goal_met      ┌──────────────────────┐
//! │  AutopilotJob Goal   │ ───────────────────────>│  Goal Completion     │
//! │  - success_criteria  │                         │  - percentage        │
//! │  - max_iterations    │                         │  - met/not met       │
//! └──────────────────────┘                         └──────────────────────┘
//! ```
//!
//! ## Key Features
//!
//! - **Progress Detection**: Analyzes execution results to detect forward movement
//! - **Goal Evaluation**: Checks success criteria against current state
//! - **Decision Support**: Provides recommendations (continue/pause/complete)
//! - **Stuck Detection**: Identifies when execution is not making progress
//!
//! ## Usage
//!
//! ```rust,ignore
//! // 注意: 此示例为不完整代码片段，缺少async上下文和变量定义
//! use cyberclaw_control_plane::autopilot_progress::{
//!     ProgressEvaluator,
//!     DefaultProgressEvaluator,
//! };
//!
//! let evaluator = DefaultProgressEvaluator::new();
//!
//! // Analyze progress after execution
//! let analysis = evaluator.analyze(&run_id, &execution_results).await?;
//! if !analysis.has_progress {
//!     // Handle stuck situation
//! }
//!
//! // Check if goal is met
//! if evaluator.is_goal_met(&run_id, &job).await? {
//!     // Complete the run
//! }
//! ```

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use cyberclaw_core::autopilot::{
    AutopilotJob, ExecutionResult, ProgressAnalysis, ProgressDecision,
};
use cyberclaw_core::execution::ExecutionStatus;
use cyberclaw_core::ids::AutopilotRunId;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

// ============================================================================
// ProgressEvaluator Trait
// ============================================================================

/// Progress evaluator for Autopilot runs
///
/// Analyzes execution results to determine progress and goal completion,
/// providing decision support for the Autopilot loop.
#[async_trait]
pub trait ProgressEvaluator: Send + Sync {
    /// Analyze execution results to determine progress
    ///
    /// # Arguments
    ///
    /// * `run_id` - Autopilot run identifier
    /// * `results` - Execution results to analyze
    ///
    /// # Returns
    ///
    /// Analysis containing:
    /// - has_progress: Whether forward movement was detected
    /// - progress_delta: Percentage change in completion
    /// - decision: Recommended next action
    /// - reasoning: Explanation of the analysis
    async fn analyze(
        &self,
        run_id: &AutopilotRunId,
        results: &[ExecutionResult],
    ) -> Result<ProgressAnalysis>;

    /// Check if the goal has been met
    ///
    /// # Arguments
    ///
    /// * `run_id` - Autopilot run identifier
    /// * `job` - Job specification with goal criteria
    ///
    /// # Returns
    ///
    /// - `true` if all success criteria are met
    /// - `false` if goal is not yet achieved
    async fn is_goal_met(&self, run_id: &AutopilotRunId, job: &AutopilotJob) -> Result<bool>;

    /// Get current goal completion percentage
    ///
    /// # Arguments
    ///
    /// * `run_id` - Autopilot run identifier
    /// * `job` - Job specification with goal criteria
    ///
    /// # Returns
    ///
    /// Percentage (0.0 - 100.0) of goal completion
    async fn get_completion_percentage(
        &self,
        run_id: &AutopilotRunId,
        job: &AutopilotJob,
    ) -> Result<f32>;
}

// ============================================================================
// Default Implementation
// ============================================================================

/// Progress tracking data for a run
#[derive(Debug, Clone)]
struct RunProgress {
    total_executions: u32,
    successful_executions: u32,
    failed_executions: u32,
    criteria_met: HashMap<String, bool>,
    last_progress_delta: f32,
    consecutive_no_change: u32,
}

impl Default for RunProgress {
    fn default() -> Self {
        Self {
            total_executions: 0,
            successful_executions: 0,
            failed_executions: 0,
            criteria_met: HashMap::new(),
            last_progress_delta: 0.0,
            consecutive_no_change: 0,
        }
    }
}

/// Default implementation of ProgressEvaluator
pub struct DefaultProgressEvaluator {
    progress_data: Arc<RwLock<HashMap<AutopilotRunId, RunProgress>>>,
    stuck_threshold: u32,
}

impl DefaultProgressEvaluator {
    /// Create a new progress evaluator with default settings
    pub fn new() -> Self {
        Self {
            progress_data: Arc::new(RwLock::new(HashMap::new())),
            stuck_threshold: 3, // Consider stuck after 3 iterations with no change
        }
    }

    /// Create with custom stuck threshold
    pub fn with_stuck_threshold(stuck_threshold: u32) -> Self {
        Self {
            progress_data: Arc::new(RwLock::new(HashMap::new())),
            stuck_threshold,
        }
    }

    /// Get or create progress data for a run
    async fn get_or_create_progress(&self, run_id: &AutopilotRunId) -> RunProgress {
        let mut data = self.progress_data.write().await;
        data.entry(run_id.clone()).or_default().clone()
    }

    /// Update progress data
    async fn update_progress<F>(&self, run_id: &AutopilotRunId, f: F) -> Result<()>
    where
        F: FnOnce(&mut RunProgress),
    {
        let mut data = self.progress_data.write().await;
        let progress = data
            .get_mut(run_id)
            .ok_or_else(|| anyhow!("Progress data not found for run_id={}", run_id.as_str()))?;
        f(progress);
        Ok(())
    }

    /// Analyze results for progress indicators
    fn detect_progress(&self, results: &[ExecutionResult], progress: &RunProgress) -> (bool, f32) {
        if results.is_empty() {
            return (false, 0.0);
        }

        // Count successful and failed executions
        let successful = results
            .iter()
            .filter(|r| r.status == ExecutionStatus::Completed)
            .count() as u32;
        let _failed = results
            .iter()
            .filter(|r| r.status == ExecutionStatus::Failed)
            .count() as u32;

        // Calculate success rate
        // usize/u32 → f32 精度损失是可接受的 (用于进度统计)
        #[allow(clippy::cast_precision_loss)]
        let total = results.len() as f32;
        #[allow(clippy::cast_precision_loss)]
        let success_rate = successful as f32 / total * 100.0;

        // Detect progress based on multiple factors
        let has_new_artifacts = results.iter().any(|r| !r.artifacts.is_empty());
        let has_successful_ops = successful > 0;
        let improving_success_rate = success_rate > 50.0;

        // Calculate progress delta
        let new_total =
            progress.total_executions + u32::try_from(results.len()).unwrap_or(u32::MAX);
        let new_successful = progress.successful_executions + successful;
        #[allow(clippy::cast_precision_loss)]
        let old_rate = if progress.total_executions > 0 {
            progress.successful_executions as f32 / progress.total_executions as f32 * 100.0
        } else {
            0.0
        };
        #[allow(clippy::cast_precision_loss)]
        let new_rate = new_successful as f32 / new_total as f32 * 100.0;
        let progress_delta = new_rate - old_rate;

        let has_progress = has_new_artifacts || has_successful_ops || improving_success_rate;

        (has_progress, progress_delta)
    }

    /// Check criteria against current state using keyword-based matching.
    ///
    /// Criteria are plain-language strings from `AutopilotGoal::success_criteria`.
    /// This evaluator applies the following rules in order:
    ///
    /// 1. Empty string → always met (vacuous truth).
    /// 2. Prefix `"always:"` → always met (used in tests or fixed milestones).
    /// 3. Prefix `"never:"` → never met.
    /// 4. Prefix `"threshold:<value>:<current>"` → met when `current >= value`
    ///    (both parsed as f64; malformed entries fall through to `false`).
    /// 5. Keyword `"all_passed"` / `"all_criteria_met"` → met only when the run
    ///    has recorded zero failures so far.
    /// 6. All other strings → unmet, because they describe external system state
    ///    (e.g. "Service deployed") that cannot be verified without a real probe.
    async fn check_criteria(
        &self,
        criteria: &[String],
        run_id: &AutopilotRunId,
    ) -> HashMap<String, bool> {
        let mut results = HashMap::new();

        // Read current run progress once for rules that depend on it.
        let progress = {
            let data = self.progress_data.read().await;
            data.get(run_id).cloned().unwrap_or_default()
        };

        for criterion in criteria {
            let trimmed = criterion.trim();

            let met = if trimmed.is_empty() {
                // Rule 1: empty criterion is vacuously met.
                true
            } else if let Some(rest) = trimmed.strip_prefix("always:") {
                // Rule 2: explicit always-met marker; the suffix is a human label.
                debug!(
                    "Criterion '{}' matched always: rule (label='{}')",
                    criterion, rest
                );
                true
            } else if trimmed.starts_with("never:") {
                // Rule 3: explicit never-met marker.
                debug!("Criterion '{}' matched never: rule", criterion);
                false
            } else if let Some(rest) = trimmed.strip_prefix("threshold:") {
                // Rule 4: "threshold:<required>:<current>" – numeric comparison.
                // Format: threshold:80.0:95.3  →  95.3 >= 80.0 → met
                let parts: Vec<&str> = rest.splitn(2, ':').collect();
                if parts.len() == 2 {
                    let required = parts[0].trim().parse::<f64>();
                    let current = parts[1].trim().parse::<f64>();
                    match (required, current) {
                        (Ok(req), Ok(cur)) => {
                            let met = cur >= req;
                            debug!(
                                "Criterion '{}': threshold check {:.2} >= {:.2} → {}",
                                criterion, cur, req, met
                            );
                            met
                        }
                        _ => {
                            warn!(
                                "Criterion '{}': malformed threshold values, treating as unmet",
                                criterion
                            );
                            false
                        }
                    }
                } else {
                    warn!(
                        "Criterion '{}': missing current value in threshold rule, treating as unmet",
                        criterion
                    );
                    false
                }
            } else if trimmed.eq_ignore_ascii_case("all_passed")
                || trimmed.eq_ignore_ascii_case("all_criteria_met")
            {
                // Rule 5: met only when no failures recorded for this run.
                let met = progress.total_executions > 0 && progress.failed_executions == 0;
                debug!(
                    "Criterion '{}': all_passed check (total={}, failed={}) → {}",
                    criterion, progress.total_executions, progress.failed_executions, met
                );
                met
            } else {
                // Rule 6: natural-language description of external state; cannot
                // be verified without a real system probe.  Mark as unmet so that
                // the autopilot loop continues to make progress toward explicit goals.
                debug!(
                    "Criterion '{}': unverifiable external state, treating as unmet",
                    criterion
                );
                false
            };

            results.insert(criterion.clone(), met);
        }

        results
    }
}

impl Default for DefaultProgressEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ProgressEvaluator for DefaultProgressEvaluator {
    async fn analyze(
        &self,
        run_id: &AutopilotRunId,
        results: &[ExecutionResult],
    ) -> Result<ProgressAnalysis> {
        let progress = self.get_or_create_progress(run_id).await;

        // Detect progress
        let (has_progress, progress_delta) = self.detect_progress(results, &progress);

        // Update consecutive no-change counter
        let consecutive_no_change = if has_progress {
            0
        } else {
            progress.consecutive_no_change + 1
        };

        // Determine decision based on progress
        let decision = if consecutive_no_change >= self.stuck_threshold {
            warn!(
                "Stuck detected for run_id={}: {} consecutive iterations without progress",
                run_id.as_str(),
                consecutive_no_change
            );
            ProgressDecision::Abort
        } else if !has_progress {
            info!(
                "No progress for run_id={}, attempt {}/{}",
                run_id.as_str(),
                consecutive_no_change,
                self.stuck_threshold
            );
            ProgressDecision::Retry
        } else if progress_delta < -10.0 {
            warn!(
                "Regression detected for run_id={}: {:.2}% decrease in success rate",
                run_id.as_str(),
                -progress_delta
            );
            ProgressDecision::PauseForReview
        } else {
            debug!(
                "Progress detected for run_id={}: {:.2}% improvement",
                run_id.as_str(),
                progress_delta
            );
            ProgressDecision::Continue
        };

        // Build reasoning
        let reasoning = format!(
            "Analyzed {} results: {} successful, {} failed. Progress delta: {:.2}%. {}",
            results.len(),
            results
                .iter()
                .filter(|r| r.status == ExecutionStatus::Completed)
                .count(),
            results
                .iter()
                .filter(|r| r.status == ExecutionStatus::Failed)
                .count(),
            progress_delta,
            if has_progress {
                "Forward movement detected."
            } else {
                "No significant progress."
            }
        );

        // Suggest next actions
        let suggested_actions = match decision {
            ProgressDecision::Continue => vec!["Proceed with next iteration".to_string()],
            ProgressDecision::Retry => vec![
                "Retry with adjusted parameters".to_string(),
                "Consider alternative approach".to_string(),
            ],
            ProgressDecision::PauseForReview => vec![
                "Request manual review".to_string(),
                "Analyze failure patterns".to_string(),
            ],
            ProgressDecision::Escalate => vec![
                "Escalate to supervisor".to_string(),
                "Request additional resources".to_string(),
            ],
            ProgressDecision::Complete => vec!["Finalize execution".to_string()],
            ProgressDecision::Abort => vec![
                "Abort execution".to_string(),
                "Document blockers".to_string(),
            ],
        };

        // Update progress data
        self.update_progress(run_id, |p| {
            p.total_executions += u32::try_from(results.len()).unwrap_or(u32::MAX);
            p.successful_executions += u32::try_from(
                results
                    .iter()
                    .filter(|r| r.status == ExecutionStatus::Completed)
                    .count(),
            )
            .unwrap_or(u32::MAX);
            p.failed_executions += u32::try_from(
                results
                    .iter()
                    .filter(|r| r.status == ExecutionStatus::Failed)
                    .count(),
            )
            .unwrap_or(u32::MAX);
            p.last_progress_delta = progress_delta;
            p.consecutive_no_change = consecutive_no_change;
        })
        .await?;

        Ok(ProgressAnalysis {
            has_progress,
            progress_delta,
            decision,
            reasoning,
            suggested_actions,
        })
    }

    async fn is_goal_met(&self, run_id: &AutopilotRunId, job: &AutopilotJob) -> Result<bool> {
        let _progress = self.get_or_create_progress(run_id).await;

        // Check success criteria
        let criteria_status = self
            .check_criteria(&job.spec.goal.success_criteria, run_id)
            .await;

        // Update progress data with criteria status
        self.update_progress(run_id, |p| {
            p.criteria_met = criteria_status.clone();
        })
        .await?;

        // All criteria must be met
        let all_met = !criteria_status.is_empty() && criteria_status.values().all(|&met| met);

        if all_met {
            info!("Goal achieved for run_id={}", run_id.as_str());
        }

        Ok(all_met)
    }

    async fn get_completion_percentage(
        &self,
        run_id: &AutopilotRunId,
        job: &AutopilotJob,
    ) -> Result<f32> {
        let progress = self.get_or_create_progress(run_id).await;

        if job.spec.goal.success_criteria.is_empty() {
            // No criteria defined, use execution success rate
            if progress.total_executions == 0 {
                return Ok(0.0);
            }
            #[allow(clippy::cast_precision_loss)]
            return Ok(
                progress.successful_executions as f32 / progress.total_executions as f32 * 100.0,
            );
        }

        // Calculate based on criteria met
        let criteria_status = self
            .check_criteria(&job.spec.goal.success_criteria, run_id)
            .await;
        #[allow(clippy::cast_precision_loss)]
        let total_criteria = criteria_status.len() as f32;
        #[allow(clippy::cast_precision_loss)]
        let met_criteria = criteria_status.values().filter(|&&met| met).count() as f32;

        Ok(met_criteria / total_criteria * 100.0)
    }
}

// ============================================================================
// Advanced Progress Evaluator with ML Support
// ============================================================================

/// Advanced progress evaluator with machine learning support
///
/// This implementation could integrate with ML models for more sophisticated
/// progress prediction and decision making.
pub struct MLProgressEvaluator {
    base_evaluator: DefaultProgressEvaluator,
    // ML model fields will be added when ML integration is implemented.
}

impl Default for MLProgressEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

impl MLProgressEvaluator {
    pub fn new() -> Self {
        Self {
            base_evaluator: DefaultProgressEvaluator::new(),
        }
    }

    /// Predict future progress based on historical patterns.
    ///
    /// Returns 0.0 until an ML model is integrated.
    async fn predict_progress(&self, _run_id: &AutopilotRunId) -> Result<f32> {
        Ok(0.0)
    }

    /// Detect anomalies in execution patterns.
    ///
    /// Returns an empty list until an anomaly detection model is integrated.
    async fn detect_anomalies(&self, _results: &[ExecutionResult]) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}

#[async_trait]
impl ProgressEvaluator for MLProgressEvaluator {
    async fn analyze(
        &self,
        run_id: &AutopilotRunId,
        results: &[ExecutionResult],
    ) -> Result<ProgressAnalysis> {
        // Use base analysis
        let mut analysis = self.base_evaluator.analyze(run_id, results).await?;

        // Enhance with ML predictions
        if let Ok(predicted_progress) = self.predict_progress(run_id).await {
            if predicted_progress < 10.0 {
                analysis.suggested_actions.push(format!(
                    "ML prediction suggests low future progress ({:.1}%)",
                    predicted_progress
                ));
            }
        }

        // Check for anomalies
        if let Ok(anomalies) = self.detect_anomalies(results).await {
            for anomaly in anomalies {
                analysis
                    .suggested_actions
                    .push(format!("Anomaly detected: {}", anomaly));
            }
        }

        Ok(analysis)
    }

    async fn is_goal_met(&self, run_id: &AutopilotRunId, job: &AutopilotJob) -> Result<bool> {
        self.base_evaluator.is_goal_met(run_id, job).await
    }

    async fn get_completion_percentage(
        &self,
        run_id: &AutopilotRunId,
        job: &AutopilotJob,
    ) -> Result<f32> {
        self.base_evaluator
            .get_completion_percentage(run_id, job)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::autopilot::AutopilotGoal;
    use cyberclaw_core::execution::ExecutionBudget;
    use cyberclaw_core::ids::ExecutionId;

    fn create_test_job() -> AutopilotJob {
        AutopilotJob {
            id: cyberclaw_core::ids::AutopilotJobId::new(),
            name: "test_job".to_string(),
            description: Some("Test job".to_string()),
            case_id: None,
            spec: cyberclaw_core::autopilot::AutopilotSpec {
                goal: AutopilotGoal {
                    description: "Test goal".to_string(),
                    success_criteria: vec![
                        "Service deployed".to_string(),
                        "Tests passed".to_string(),
                    ],
                    max_iterations: 10,
                    max_duration_ms: Some(60000),
                },
                trigger: cyberclaw_core::autopilot::AutopilotTrigger::Manual,
                stop_conditions: vec![cyberclaw_core::autopilot::StopCondition::OnGoalReached],
                session_mode: cyberclaw_core::autopilot::SessionMode::IsolatedSession,
                agent: cyberclaw_core::execution::AgentRef {
                    id: cyberclaw_core::ids::AgentId::new(),
                    role: "test".to_string(),
                },
                budget: ExecutionBudget::default(),
                max_risk_level: cyberclaw_core::capability::RiskLevel::Medium,
                auto_approve_low_risk: true,
            },
            enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_progress_detection() {
        let evaluator = DefaultProgressEvaluator::new();
        let run_id = AutopilotRunId::new();

        // Test with successful results
        let results = vec![
            ExecutionResult {
                execution_id: ExecutionId::new(),
                status: ExecutionStatus::Completed,
                output: None,
                error: None,
                artifacts: vec!["artifact1".to_string()],
                duration_ms: 0,
            },
            ExecutionResult {
                execution_id: ExecutionId::new(),
                status: ExecutionStatus::Completed,
                output: None,
                error: None,
                artifacts: vec![],
                duration_ms: 0,
            },
        ];

        let analysis = evaluator.analyze(&run_id, &results).await.unwrap();
        assert!(analysis.has_progress);
        assert_eq!(analysis.decision, ProgressDecision::Continue);
    }

    #[tokio::test]
    async fn test_stuck_detection() {
        let evaluator = DefaultProgressEvaluator::with_stuck_threshold(2);
        let run_id = AutopilotRunId::new();

        // Simulate no progress for multiple iterations
        let empty_results: Vec<ExecutionResult> = vec![];

        // First iteration - no progress
        let analysis1 = evaluator.analyze(&run_id, &empty_results).await.unwrap();
        assert!(!analysis1.has_progress);
        assert_eq!(analysis1.decision, ProgressDecision::Retry);

        // Second iteration - still no progress, should detect stuck
        let analysis2 = evaluator.analyze(&run_id, &empty_results).await.unwrap();
        assert!(!analysis2.has_progress);
        assert_eq!(analysis2.decision, ProgressDecision::Abort);
    }

    #[tokio::test]
    async fn test_goal_completion_percentage() {
        let evaluator = DefaultProgressEvaluator::new();
        let run_id = AutopilotRunId::new();
        let job = create_test_job();

        // Initial state - 0% complete
        let percentage = evaluator
            .get_completion_percentage(&run_id, &job)
            .await
            .unwrap();
        assert_eq!(percentage, 0.0);

        // After some successful executions
        let results = vec![ExecutionResult {
            execution_id: ExecutionId::new(),
            status: ExecutionStatus::Completed,
            output: None,
            error: None,
            artifacts: vec![],
            duration_ms: 0,
        }];

        evaluator.analyze(&run_id, &results).await.unwrap();

        // Check percentage again (criteria-based, will still be 0% as criteria aren't met)
        let percentage = evaluator
            .get_completion_percentage(&run_id, &job)
            .await
            .unwrap();
        assert_eq!(percentage, 0.0);
    }
}
