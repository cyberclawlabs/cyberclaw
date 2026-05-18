//! GovernedAutopilotStepRunner — bridges `AutopilotStepRunner` (execution_service)
//! to real platform services (SecurityGate, ProgressEvaluator, IterationTracker, etc.).
//!
//! This struct breaks the circular dependency:
//!   GovernedLoopRuntime -> Arc<dyn ExecutionService>
//!   InMemoryExecutionService -> Arc<dyn AutopilotStepRunner>
//!   GovernedAutopilotStepRunner -> real services (no ExecutionService ref)

use async_trait::async_trait;
use cyberclaw_core::ids::AutopilotRunId;
use std::sync::Arc;
use tracing::{info, warn};

use crate::autopilot_progress::ProgressEvaluator;
use crate::autopilot_runtime::SecurityGate;
use crate::execution_service::{
    AutopilotStep, AutopilotStepRunner, IterationTracker, StateSyncCoordinator, StepResult,
};

/// Real autopilot step runner backed by platform governance services.
pub struct GovernedAutopilotStepRunner {
    security_gate: Arc<dyn SecurityGate>,
    progress_evaluator: Arc<dyn ProgressEvaluator>,
    iteration_tracker: Arc<dyn IterationTracker>,
    state_sync: Option<Arc<dyn StateSyncCoordinator>>,
}

impl GovernedAutopilotStepRunner {
    pub fn new(
        security_gate: Arc<dyn SecurityGate>,
        progress_evaluator: Arc<dyn ProgressEvaluator>,
        iteration_tracker: Arc<dyn IterationTracker>,
    ) -> Self {
        Self {
            security_gate,
            progress_evaluator,
            iteration_tracker,
            state_sync: None,
        }
    }

    pub fn with_state_sync(mut self, sync: Arc<dyn StateSyncCoordinator>) -> Self {
        self.state_sync = Some(sync);
        self
    }
}

#[async_trait]
impl AutopilotStepRunner for GovernedAutopilotStepRunner {
    async fn run_step(
        &self,
        execution_id: &cyberclaw_core::prelude::ExecutionId,
        step: &AutopilotStep,
        iteration: u32,
        step_context: &serde_json::Value,
    ) -> anyhow::Result<StepResult> {
        let start = std::time::Instant::now();

        let run_id = AutopilotRunId::from_string(execution_id.to_string())
            .unwrap_or_else(|_| AutopilotRunId::new());

        let (success, output, error) = match step {
            AutopilotStep::Plan => {
                info!(
                    execution_id = %execution_id,
                    iteration = iteration,
                    "AutopilotStep::Plan — building plan from context"
                );
                let plan_meta = serde_json::json!({
                    "iteration": iteration,
                    "context_keys": step_context.as_object()
                        .map(|m| m.keys().cloned().collect::<Vec<_>>())
                        .unwrap_or_default(),
                    "status": "planned",
                });
                (true, Some(plan_meta), None)
            }

            AutopilotStep::Execute => {
                info!(
                    execution_id = %execution_id,
                    iteration = iteration,
                    "AutopilotStep::Execute — execution driven by outer loop"
                );
                let ack = serde_json::json!({
                    "iteration": iteration,
                    "status": "execution_acknowledged",
                });
                (true, Some(ack), None)
            }

            AutopilotStep::Review => {
                info!(
                    execution_id = %execution_id,
                    iteration = iteration,
                    "AutopilotStep::Review — calling security gate"
                );
                match self.security_gate.check_execution_results(&[]).await {
                    Ok(result) => {
                        if !result.passed && !result.issues.is_empty() {
                            warn!(
                                execution_id = %execution_id,
                                issues = ?result.issues,
                                "Security gate flagged issues"
                            );
                        }
                        let output = serde_json::json!({
                            "passed": result.passed,
                            "issues": result.issues,
                            "requires_review": result.requires_review,
                        });
                        (result.passed, Some(output), None)
                    }
                    Err(e) => {
                        let msg = format!("security_gate.check_execution_results failed: {e}");
                        warn!("{}", msg);
                        (false, None, Some(msg))
                    }
                }
            }

            AutopilotStep::Analyze => {
                info!(
                    execution_id = %execution_id,
                    iteration = iteration,
                    "AutopilotStep::Analyze — calling progress evaluator"
                );
                match self.progress_evaluator.analyze(&run_id, &[]).await {
                    Ok(analysis) => {
                        let output = serde_json::json!({
                            "has_progress": analysis.has_progress,
                            "progress_delta": analysis.progress_delta,
                            "decision": format!("{:?}", analysis.decision),
                            "reasoning": analysis.reasoning,
                            "suggested_actions": analysis.suggested_actions,
                        });
                        (true, Some(output), None)
                    }
                    Err(e) => {
                        let msg = format!("progress_evaluator.analyze failed: {e}");
                        warn!("{}", msg);
                        (false, None, Some(msg))
                    }
                }
            }

            AutopilotStep::Decide => {
                info!(
                    execution_id = %execution_id,
                    iteration = iteration,
                    "AutopilotStep::Decide — reading iteration counter and analyze output"
                );
                let current = match self.iteration_tracker.current_iteration(execution_id).await {
                    Ok(n) => n,
                    Err(e) => {
                        let msg = format!("iteration_tracker.current_iteration failed: {e}");
                        warn!("{}", msg);
                        return Ok(StepResult {
                            step: step.clone(),
                            success: false,
                            output: None,
                            error: Some(msg),
                            duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
                        });
                    }
                };
                let analyze_out = step_context.get("Analyze").cloned();
                let decision = analyze_out
                    .as_ref()
                    .and_then(|v| v.get("decision"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Continue");
                let output = serde_json::json!({
                    "current_iteration": current,
                    "decision": decision,
                    "analyze_summary": analyze_out,
                });
                (true, Some(output), None)
            }

            AutopilotStep::Update => {
                info!(
                    execution_id = %execution_id,
                    iteration = iteration,
                    "AutopilotStep::Update — syncing state after iteration"
                );
                if let Some(sync) = &self.state_sync {
                    let iter_result = crate::execution_service::IterationResult {
                        iteration,
                        steps_completed: vec![],
                        decision: crate::execution_service::Decision::Continue,
                        progress_made: true,
                        output: step_context.get("Analyze").cloned(),
                        errors: vec![],
                    };
                    if let Err(e) = sync
                        .sync_after_iteration(execution_id, iteration, &iter_result)
                        .await
                    {
                        let msg = format!("state_sync.sync_after_iteration failed: {e}");
                        warn!("{}", msg);
                        return Ok(StepResult {
                            step: step.clone(),
                            success: false,
                            output: None,
                            error: Some(msg),
                            duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
                        });
                    }
                    let output = serde_json::json!({ "synced": true, "iteration": iteration });
                    (true, Some(output), None)
                } else {
                    info!(
                        execution_id = %execution_id,
                        "No StateSyncCoordinator configured; skipping state sync"
                    );
                    let output =
                        serde_json::json!({ "synced": false, "reason": "no_sync_coordinator" });
                    (true, Some(output), None)
                }
            }

            AutopilotStep::Check => {
                info!(
                    execution_id = %execution_id,
                    iteration = iteration,
                    "AutopilotStep::Check — reading Decide output for goal status"
                );
                let decide_out = step_context.get("Decide").cloned();
                let decision = decide_out
                    .as_ref()
                    .and_then(|v| v.get("decision"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Continue");
                let goal_met = decision == "GoalMet";
                let output = serde_json::json!({
                    "goal_met": goal_met,
                    "decision": decision,
                });
                (true, Some(output), None)
            }

            AutopilotStep::Iterate => {
                info!(
                    execution_id = %execution_id,
                    iteration = iteration,
                    "AutopilotStep::Iterate — incrementing iteration counter"
                );
                match self.iteration_tracker.increment(execution_id).await {
                    Ok(next) => {
                        let output = serde_json::json!({
                            "previous_iteration": iteration,
                            "next_iteration": next,
                        });
                        (true, Some(output), None)
                    }
                    Err(e) => {
                        let msg = format!("iteration_tracker.increment failed: {e}");
                        warn!("{}", msg);
                        (false, None, Some(msg))
                    }
                }
            }

            AutopilotStep::Finalize => {
                info!(
                    execution_id = %execution_id,
                    iteration = iteration,
                    "AutopilotStep::Finalize — autopilot loop complete"
                );
                let check_out = step_context.get("Check").cloned();
                let goal_met = check_out
                    .as_ref()
                    .and_then(|v| v.get("goal_met"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let output = serde_json::json!({
                    "total_iterations": iteration,
                    "goal_met": goal_met,
                    "status": if goal_met { "success" } else { "incomplete" },
                });
                (true, Some(output), None)
            }
        };

        Ok(StepResult {
            step: step.clone(),
            success,
            output,
            error,
            duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
        })
    }
}
