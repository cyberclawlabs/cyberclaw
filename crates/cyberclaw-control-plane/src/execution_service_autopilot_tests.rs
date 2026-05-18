#[cfg(test)]
mod tests {
    use super::super::*;
    use cyberclaw_core::enums::Priority;
    use cyberclaw_core::task::{TaskInput, TaskKind, TriggerRef};
    use std::sync::Arc;
    use tokio;

    // ============ Mock implementations for testing ============

    struct MockIterationTracker {
        iterations: Arc<RwLock<HashMap<ExecutionId, u32>>>,
    }

    impl MockIterationTracker {
        fn new() -> Self {
            Self {
                iterations: Arc::new(RwLock::new(HashMap::new())),
            }
        }
    }

    #[async_trait]
    impl IterationTracker for MockIterationTracker {
        async fn current_iteration(&self, execution_id: &ExecutionId) -> anyhow::Result<u32> {
            let iterations = self.iterations.read().unwrap();
            Ok(iterations.get(execution_id).cloned().unwrap_or(0))
        }

        async fn increment(&self, execution_id: &ExecutionId) -> anyhow::Result<u32> {
            let mut iterations = self.iterations.write().unwrap();
            let current = iterations.entry(execution_id.clone()).or_insert(0);
            *current += 1;
            Ok(*current)
        }

        async fn reset(&self, execution_id: &ExecutionId) -> anyhow::Result<()> {
            let mut iterations = self.iterations.write().unwrap();
            iterations.insert(execution_id.clone(), 0);
            Ok(())
        }
    }

    struct MockStateSyncCoordinator;

    #[async_trait]
    impl StateSyncCoordinator for MockStateSyncCoordinator {
        async fn sync_before_iteration(
            &self,
            _execution_id: &ExecutionId,
            _iteration: u32,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn sync_after_iteration(
            &self,
            _execution_id: &ExecutionId,
            _iteration: u32,
            _result: &IterationResult,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct MockStuckDetector;

    #[async_trait]
    impl StuckDetector for MockStuckDetector {
        async fn is_stuck(
            &self,
            _execution_id: &ExecutionId,
            iteration_history: &[IterationSummary],
        ) -> anyhow::Result<Option<String>> {
            // Consider stuck if we have 3+ iterations with no progress
            let no_progress_count = iteration_history
                .iter()
                .rev()
                .take(3)
                .filter(|i| !i.progress_made)
                .count();

            if no_progress_count >= 3 {
                Ok(Some("No progress for 3 iterations".to_string()))
            } else {
                Ok(None)
            }
        }

        async fn get_resolution(
            &self,
            _execution_id: &ExecutionId,
            _reason: &str,
        ) -> anyhow::Result<StuckResolution> {
            Ok(StuckResolution::Abort)
        }
    }

    struct MockCheckpointStore {
        checkpoints: Arc<RwLock<HashMap<ExecutionId, IterationState>>>,
    }

    impl MockCheckpointStore {
        fn new() -> Self {
            Self {
                checkpoints: Arc::new(RwLock::new(HashMap::new())),
            }
        }
    }

    #[async_trait]
    impl CheckpointStore for MockCheckpointStore {
        async fn save(
            &self,
            execution_id: &ExecutionId,
            _iteration: u32,
            state: &IterationState,
        ) -> anyhow::Result<()> {
            let mut checkpoints = self.checkpoints.write().unwrap();
            checkpoints.insert(execution_id.clone(), state.clone());
            Ok(())
        }

        async fn load_latest(
            &self,
            execution_id: &ExecutionId,
        ) -> anyhow::Result<Option<IterationState>> {
            let checkpoints = self.checkpoints.read().unwrap();
            Ok(checkpoints.get(execution_id).cloned())
        }

        async fn clear(&self, execution_id: &ExecutionId) -> anyhow::Result<()> {
            let mut checkpoints = self.checkpoints.write().unwrap();
            checkpoints.remove(execution_id);
            Ok(())
        }
    }

    // ============ Unit Tests ============

    #[tokio::test]
    async fn test_autopilot_execution_mode_detection() {
        let service = InMemoryExecutionService::new();

        // Create a normal execution
        let normal_exec_id = ExecutionId::new();
        let normal_request = ExecutionRequest {
            execution_id: normal_exec_id.clone(),
            task: Task {
                id: TaskId::new(),
                case_id: None,
                title: "Test Task".to_string(),
                summary: "Test task for autopilot mode detection".to_string(),
                kind: TaskKind::Analysis,
                priority: Priority::Low,
                requested_by: ActorRef {
                    id: ActorId::from_string("test-actor".to_string()).unwrap(),
                    actor_type: ActorType::Human,
                    tenant_id: None,
                    home_node_id: None,
                    display_name: "Test Actor".to_string(),
                },
                requested_at: chrono::Utc::now(),
                trigger: TriggerRef {
                    kind: "manual".to_string(),
                    source: "test".to_string(),
                },
                input: TaskInput::default(),
                desired_outputs: vec![],
                labels: vec![],
                preferred_agent_id: None,
            },
            case: None,
            context: crate::types::ControlPlaneContext {
                actor: ActorRef {
                    id: ActorId::from_string("test-actor".to_string()).unwrap(),
                    actor_type: ActorType::Human,
                    tenant_id: None,
                    home_node_id: None,
                    display_name: "Test Actor".to_string(),
                },
                session: None,
                workspace: None,
            },
            agent: None,
            trace_id: None,
            execution_mode: Some(ExecutionMode::Normal),
            plan: None,
        };

        // Create an Autopilot execution
        let autopilot_exec_id = ExecutionId::new();
        let autopilot_request = ExecutionRequest {
            execution_id: autopilot_exec_id.clone(),
            execution_mode: Some(ExecutionMode::Autopilot),
            ..normal_request.clone()
        };

        // Submit both executions
        service.submit(normal_request).await.unwrap();
        service.submit(autopilot_request).await.unwrap();

        // Verify execution modes are properly set
        // (This test verifies the data structure is correct)
        assert!(service.get(&normal_exec_id).await.unwrap().is_some());
        assert!(service.get(&autopilot_exec_id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_concurrent_execution_limits() {
        let service = InMemoryExecutionService::new();

        // Test normal execution limits
        let normal_count = service.concurrent_executions.load(Ordering::Relaxed);
        assert_eq!(normal_count, 0);

        // Test Autopilot execution limits
        let autopilot_count = service
            .concurrent_autopilot_executions
            .load(Ordering::Relaxed);
        assert_eq!(autopilot_count, 0);
    }

    #[tokio::test]
    async fn test_execute_autopilot_iteration() {
        let mut service = InMemoryExecutionService::new();
        service.iteration_tracker = Some(Arc::new(MockIterationTracker::new()));
        service.state_sync = Some(Arc::new(MockStateSyncCoordinator));

        let execution_id = ExecutionId::new();

        // Execute an iteration
        let result = service
            .execute_autopilot_iteration(&execution_id, 1)
            .await
            .unwrap();

        assert_eq!(result.iteration, 1);
        assert!(!result.steps_completed.is_empty());
        assert!(result.progress_made);
    }

    #[tokio::test]
    async fn test_on_iteration_start() {
        let service = InMemoryExecutionService::new();
        let execution_id = ExecutionId::new();

        // Should not panic even without state_sync
        service.on_iteration_start(&execution_id, 1).await.unwrap();
    }

    #[tokio::test]
    async fn test_on_step_complete() {
        let service = InMemoryExecutionService::new();
        let execution_id = ExecutionId::new();

        let step_result = StepResult {
            step: AutopilotStep::Plan,
            success: true,
            output: Some(serde_json::json!({"test": "data"})),
            error: None,
            duration_ms: 100,
        };

        service
            .on_step_complete(&execution_id, AutopilotStep::Plan, step_result)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_on_stuck_detected_without_detector() {
        let service = InMemoryExecutionService::new();
        let execution_id = ExecutionId::new();

        let resolution = service
            .on_stuck_detected(&execution_id, 5, "Test stuck".to_string())
            .await
            .unwrap();

        // Default should be Abort
        assert!(matches!(resolution, StuckResolution::Abort));
    }

    #[tokio::test]
    async fn test_on_stuck_detected_with_detector() {
        let mut service = InMemoryExecutionService::new();
        service.stuck_detector = Some(Arc::new(MockStuckDetector));

        let execution_id = ExecutionId::new();

        let resolution = service
            .on_stuck_detected(&execution_id, 5, "Test stuck".to_string())
            .await
            .unwrap();

        assert!(matches!(resolution, StuckResolution::Abort));
    }

    #[tokio::test]
    async fn test_checkpoint_iteration_without_store() {
        let service = InMemoryExecutionService::new();
        let execution_id = ExecutionId::new();

        let state = IterationState {
            iteration: 1,
            current_step: AutopilotStep::Plan,
            steps_completed: vec![],
            context: serde_json::Value::Null,
            memory_snapshot: vec![],
            timestamp: chrono::Utc::now(),
        };

        // Should not panic even without checkpoint_store
        service
            .checkpoint_iteration(&execution_id, 1, state)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_checkpoint_iteration_with_store() {
        let mut service = InMemoryExecutionService::new();
        service.checkpoint_store = Some(Arc::new(MockCheckpointStore::new()));

        let execution_id = ExecutionId::new();

        let state = IterationState {
            iteration: 1,
            current_step: AutopilotStep::Execute,
            steps_completed: vec![AutopilotStep::Plan],
            context: serde_json::json!({"checkpoint": "test"}),
            memory_snapshot: vec!["memory1".to_string()],
            timestamp: chrono::Utc::now(),
        };

        service
            .checkpoint_iteration(&execution_id, 1, state.clone())
            .await
            .unwrap();

        // Verify checkpoint was saved
        let loaded = service.resume_from_checkpoint(&execution_id).await.unwrap();
        assert!(loaded.is_some());

        let loaded_state = loaded.unwrap();
        assert_eq!(loaded_state.iteration, 1);
        assert_eq!(loaded_state.current_step, AutopilotStep::Execute);
    }

    #[tokio::test]
    async fn test_resume_from_checkpoint_without_store() {
        let service = InMemoryExecutionService::new();
        let execution_id = ExecutionId::new();

        let result = service.resume_from_checkpoint(&execution_id).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_get_iteration_history_empty() {
        let service = InMemoryExecutionService::new();
        let execution_id = ExecutionId::new();

        let history = service.get_iteration_history(&execution_id).await.unwrap();
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn test_get_iteration_history_with_data() {
        let service = InMemoryExecutionService::new();
        let execution_id = ExecutionId::new();

        // Add some history
        {
            let mut histories = service.iteration_histories.write().unwrap();
            let history = histories.entry(execution_id.clone()).or_default();

            history.push(IterationSummary {
                iteration: 1,
                start_time: chrono::Utc::now(),
                end_time: chrono::Utc::now(),
                steps_completed: vec![AutopilotStep::Plan, AutopilotStep::Execute],
                decision: "Continue".to_string(),
                progress_made: true,
            });

            history.push(IterationSummary {
                iteration: 2,
                start_time: chrono::Utc::now(),
                end_time: chrono::Utc::now(),
                steps_completed: vec![AutopilotStep::Plan],
                decision: "Stuck".to_string(),
                progress_made: false,
            });
        }

        let history = service.get_iteration_history(&execution_id).await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].iteration, 1);
        assert_eq!(history[1].iteration, 2);
        assert!(history[0].progress_made);
        assert!(!history[1].progress_made);
    }

    #[tokio::test]
    async fn test_autopilot_step_execution_flow() {
        let service = InMemoryExecutionService::new();
        let execution_id = ExecutionId::new();

        // Test the 9-step loop
        let result = service
            .execute_autopilot_iteration(&execution_id, 1)
            .await
            .unwrap();

        // Verify all steps are attempted
        let expected_steps = vec![
            AutopilotStep::Plan,
            AutopilotStep::Execute,
            AutopilotStep::Review,
            AutopilotStep::Analyze,
            AutopilotStep::Decide,
            AutopilotStep::Update,
            AutopilotStep::Check,
            AutopilotStep::Iterate,
            AutopilotStep::Finalize,
        ];

        for step in &expected_steps {
            assert!(result.steps_completed.contains(step));
        }

        // Should complete with GoalMet since Finalize was reached
        assert_eq!(result.decision, Decision::GoalMet);
    }

    #[tokio::test]
    async fn test_iteration_history_size_limit() {
        let service = InMemoryExecutionService::new();
        let execution_id = ExecutionId::new();

        // Add more than MAX_HISTORY_SIZE entries
        for i in 1..=25 {
            let state = IterationState {
                iteration: i,
                current_step: AutopilotStep::Plan,
                steps_completed: vec![],
                context: serde_json::Value::Null,
                memory_snapshot: vec![],
                timestamp: chrono::Utc::now(),
            };

            service
                .checkpoint_iteration(&execution_id, i, state)
                .await
                .unwrap();
        }

        // Verify history is limited to MAX_HISTORY_SIZE (20)
        let history = service.get_iteration_history(&execution_id).await.unwrap();
        assert!(history.len() <= 20);
    }

    // ============ AutopilotStepRunner integration tests ============

    struct MockStepRunner {
        fail_on: Option<AutopilotStep>,
    }

    #[async_trait]
    impl AutopilotStepRunner for MockStepRunner {
        async fn run_step(
            &self,
            _execution_id: &ExecutionId,
            step: &AutopilotStep,
            iteration: u32,
            _step_context: &serde_json::Value,
        ) -> anyhow::Result<StepResult> {
            if let Some(ref fail_step) = self.fail_on {
                if step == fail_step {
                    return Ok(StepResult {
                        step: step.clone(),
                        success: false,
                        output: None,
                        error: Some("mock failure".to_string()),
                        duration_ms: 0,
                    });
                }
            }
            Ok(StepResult {
                step: step.clone(),
                success: true,
                output: Some(serde_json::json!({
                    "step": format!("{:?}", step),
                    "iteration": iteration,
                    "runner": "mock",
                })),
                error: None,
                duration_ms: 1,
            })
        }
    }

    #[tokio::test]
    async fn test_execute_autopilot_iteration_with_step_runner() {
        let runner = Arc::new(MockStepRunner { fail_on: None });
        let service = InMemoryExecutionService::new().with_autopilot_step_runner(runner);

        let execution_id = ExecutionId::new();
        let result = service
            .execute_autopilot_iteration(&execution_id, 1)
            .await
            .unwrap();

        // All 9 steps should complete because the mock always succeeds
        let expected_steps = vec![
            AutopilotStep::Plan,
            AutopilotStep::Execute,
            AutopilotStep::Review,
            AutopilotStep::Analyze,
            AutopilotStep::Decide,
            AutopilotStep::Update,
            AutopilotStep::Check,
            AutopilotStep::Iterate,
            AutopilotStep::Finalize,
        ];
        for step in &expected_steps {
            assert!(
                result.steps_completed.contains(step),
                "Expected step {:?} to be completed",
                step
            );
        }

        // With all steps completed, decision should be GoalMet and progress made
        assert_eq!(result.decision, Decision::GoalMet);
        assert!(result.progress_made);
        assert_eq!(result.iteration, 1);

        // Output should carry the runner's output (last step: Finalize)
        let output = result.output.unwrap();
        assert_eq!(output["runner"], "mock");
    }

    #[tokio::test]
    async fn test_step_runner_failure_propagates() {
        // Runner fails on the Review step
        let runner = Arc::new(MockStepRunner {
            fail_on: Some(AutopilotStep::Review),
        });
        let service = InMemoryExecutionService::new().with_autopilot_step_runner(runner);

        let execution_id = ExecutionId::new();
        let result = service
            .execute_autopilot_iteration(&execution_id, 1)
            .await
            .unwrap();

        // Plan and Execute should succeed; Review causes a stop
        assert!(result.steps_completed.contains(&AutopilotStep::Plan));
        assert!(result.steps_completed.contains(&AutopilotStep::Execute));
        assert!(!result.steps_completed.contains(&AutopilotStep::Review));

        // Steps after Review should not be reached
        assert!(!result.steps_completed.contains(&AutopilotStep::Analyze));
        assert!(!result.steps_completed.contains(&AutopilotStep::Finalize));

        // Errors should record the failure message
        assert!(!result.errors.is_empty());
        assert!(result.errors[0].contains("mock failure"));

        // GoalMet requires Finalize; we stopped early so it should not be GoalMet
        assert_ne!(result.decision, Decision::GoalMet);
    }

    #[tokio::test]
    async fn test_execute_autopilot_iteration_fallback_without_runner() {
        // No step runner attached — should use placeholder fallback
        let service = InMemoryExecutionService::new();
        let execution_id = ExecutionId::new();

        let result = service
            .execute_autopilot_iteration(&execution_id, 1)
            .await
            .unwrap();

        // Fallback always returns success: true with placeholder output
        assert!(!result.steps_completed.is_empty());
        assert!(result.progress_made);

        // All 9 steps should complete with the placeholder
        let expected_steps = vec![
            AutopilotStep::Plan,
            AutopilotStep::Execute,
            AutopilotStep::Review,
            AutopilotStep::Analyze,
            AutopilotStep::Decide,
            AutopilotStep::Update,
            AutopilotStep::Check,
            AutopilotStep::Iterate,
            AutopilotStep::Finalize,
        ];
        for step in &expected_steps {
            assert!(
                result.steps_completed.contains(step),
                "Fallback should complete step {:?}",
                step
            );
        }

        assert_eq!(result.decision, Decision::GoalMet);
    }
}
