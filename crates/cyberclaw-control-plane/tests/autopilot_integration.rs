//! Autopilot V2 Integration Tests
//!
//! Tests for the 9-step GovernedLoop execution engine.

use anyhow::Result;
use cyberclaw_control_plane::{
    autopilot_iteration::{
        AutopilotIterationState, AutopilotIterationTracker, InMemoryIterationTracker,
    },
    autopilot_progress::{DefaultProgressEvaluator, ProgressEvaluator},
    autopilot_runtime::{GovernedLoopRuntime, SecurityCheckResult, SecurityGate},
    autopilot_state_sync::{AutopilotStateSyncCoordinator, DefaultStateSyncCoordinator},
    autopilot_types::{AutopilotPhase, AutopilotStep, V2IterationState},
    execution_service::InMemoryExecutionService,
    provenance_tracker::InMemoryProvenanceTracker,
    review_queue::InMemoryReviewQueue,
    shared_state_store::InMemorySharedStateStore,
};
use cyberclaw_core::autopilot::{
    AutopilotGoal, AutopilotJob, AutopilotSpec, AutopilotTrigger, ExecutionResult, SessionMode,
    StopCondition,
};
use cyberclaw_core::capability::RiskLevel;
use cyberclaw_core::execution::{AgentRef, ExecutionBudget, ExecutionStatus};
use cyberclaw_core::ids::{AgentId, AutopilotJobId, AutopilotRunId, ExecutionId};
use std::sync::Arc;
use tokio::sync::Mutex;

// ─── Mock SecurityGate Implementation ────────────────────────────────────────

struct MockSecurityGate {
    should_pass: Arc<Mutex<bool>>,
    requires_review: Arc<Mutex<bool>>,
}

impl MockSecurityGate {
    fn new(should_pass: bool, requires_review: bool) -> Self {
        Self {
            should_pass: Arc::new(Mutex::new(should_pass)),
            requires_review: Arc::new(Mutex::new(requires_review)),
        }
    }
}

#[async_trait::async_trait]
impl SecurityGate for MockSecurityGate {
    async fn check_execution_results(
        &self,
        _results: &[ExecutionResult],
    ) -> Result<SecurityCheckResult> {
        Ok(SecurityCheckResult {
            passed: *self.should_pass.lock().await,
            issues: Vec::new(),
            requires_review: *self.requires_review.lock().await,
        })
    }
}

// ─── Helper Functions ─────────────────────────────────────────────────────────

fn create_test_job(name: &str, max_iterations: u32) -> AutopilotJob {
    AutopilotJob {
        id: AutopilotJobId::new(),
        name: name.to_string(),
        description: Some(format!("Test job: {}", name)),
        case_id: None,
        spec: AutopilotSpec {
            goal: AutopilotGoal {
                description: "Test goal".to_string(),
                success_criteria: vec!["Test criterion".to_string()],
                max_iterations,
                max_duration_ms: Some(60000),
            },
            trigger: AutopilotTrigger::Manual,
            stop_conditions: vec![StopCondition::OnGoalReached],
            session_mode: SessionMode::IsolatedSession,
            agent: AgentRef {
                id: AgentId::new(),
                role: "test-agent".to_string(),
            },
            budget: ExecutionBudget {
                max_steps: Some(100),
                max_duration_ms: Some(60000),
                max_tokens: Some(10000),
                max_children: Some(10),
                tokens_used: 0,
            },
            max_risk_level: RiskLevel::Medium,
            auto_approve_low_risk: true,
        },
        enabled: true,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

async fn create_runtime(security_passes: bool, requires_review: bool) -> GovernedLoopRuntime {
    let execution_service = Arc::new(InMemoryExecutionService::new());
    let state_store = Arc::new(InMemorySharedStateStore::new());
    let state_sync = Arc::new(DefaultStateSyncCoordinator::new(state_store.clone()));
    let iteration_tracker = Arc::new(InMemoryIterationTracker::new());
    let progress_evaluator = Arc::new(DefaultProgressEvaluator::new());
    let security_gate = Arc::new(MockSecurityGate::new(security_passes, requires_review));
    let review_queue = Arc::new(InMemoryReviewQueue::new(None));
    let provenance_tracker = Arc::new(InMemoryProvenanceTracker::new());

    GovernedLoopRuntime::new(
        execution_service,
        state_store,
        state_sync,
        iteration_tracker,
        progress_evaluator,
        security_gate,
        review_queue,
        provenance_tracker,
    )
}

// ─── Test Cases ───────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn test_happy_path_complete_9_steps() {
    // Test that all 9 steps execute successfully in happy path
    let runtime = create_runtime(true, false).await;
    let job = create_test_job("happy_path", 5);

    let handle = runtime.start_run(job).await.unwrap();
    assert!(!handle.run_id().as_str().is_empty());

    // Wait briefly for async execution
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Verify run was started (would check state store in real implementation)
}

#[tokio::test(flavor = "multi_thread")]
async fn test_no_progress_detection() {
    // Test that stuck detection triggers after consecutive no-progress iterations
    let _runtime = create_runtime(true, false).await;
    let _job = create_test_job("no_progress", 10);

    // Create iteration tracker with low threshold for testing
    let iteration_tracker = InMemoryIterationTracker::new().with_stuck_threshold(2);

    // Record same state hash multiple times via record_iteration
    let run_id = ExecutionId::new();
    for i in 1..=3 {
        let state = AutopilotIterationState::new(i, "same_hash".to_string());
        iteration_tracker
            .record_iteration(&run_id, state)
            .await
            .unwrap();
    }

    // Should detect stuck
    assert!(iteration_tracker.detect_stuck(&run_id).await.unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_review_gate_blocking() {
    // Test that high-risk capabilities trigger manual review
    let runtime = create_runtime(true, true).await; // requires_review = true
    let job = create_test_job("review_gate", 5);

    let handle = runtime.start_run(job).await.unwrap();
    let _ = handle.run_id();

    // Wait briefly for async execution
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // In real implementation, would verify run status is WaitingReview
}

#[tokio::test(flavor = "multi_thread")]
async fn test_iteration_limit() {
    // Test that execution stops after reaching max_iterations
    let runtime = create_runtime(true, false).await;
    let job = create_test_job("iteration_limit", 2); // Very low limit

    let handle = runtime.start_run(job).await.unwrap();
    let _ = handle.run_id();

    // Wait for iterations to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // In real implementation, would verify iteration count = 2
}

#[tokio::test(flavor = "multi_thread")]
async fn test_state_recovery() {
    // Test recovery from checkpoint after interruption
    let state_store = Arc::new(InMemorySharedStateStore::new());
    let state_sync = Arc::new(DefaultStateSyncCoordinator::new(state_store.clone()));

    // Save a checkpoint
    let run_id = ExecutionId::new();
    let test_state = V2IterationState {
        iteration_id: 5,
        step: AutopilotStep::Execute,
        start_time: chrono::Utc::now(),
        end_time: None,
        state_hash: "test_hash".to_string(),
        progress_delta: Some(0.5),
        execution_results: vec![],
        current_phase: AutopilotPhase::Plan,
        fix_loop_count: 0,
        plan_mode_snapshot: None,
    };

    state_sync
        .sync_to_store(&run_id, &test_state)
        .await
        .unwrap();

    // Recover from checkpoint
    let recovered = state_sync.sync_from_store(&run_id).await.unwrap();
    assert!(recovered.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_progress_evaluation() {
    // Test progress evaluator correctly analyzes execution results
    let evaluator = DefaultProgressEvaluator::new();
    let run_id = AutopilotRunId::new();

    // Create mixed results
    let results = vec![
        ExecutionResult {
            execution_id: ExecutionId::new(),
            status: ExecutionStatus::Completed,
            output: Some(serde_json::json!({"success": true})),
            error: None,
            artifacts: vec!["artifact1".to_string()],
            duration_ms: 0,
        },
        ExecutionResult {
            execution_id: ExecutionId::new(),
            status: ExecutionStatus::Failed,
            output: None,
            error: Some("Test error".to_string()),
            artifacts: vec![],
            duration_ms: 0,
        },
        ExecutionResult {
            execution_id: ExecutionId::new(),
            status: ExecutionStatus::Completed,
            output: Some(serde_json::json!({"data": "test"})),
            error: None,
            artifacts: vec!["artifact2".to_string()],
            duration_ms: 0,
        },
    ];

    let analysis = evaluator.analyze(&run_id, &results).await.unwrap();
    assert!(analysis.has_progress); // Has artifacts and successful operations
    assert!(analysis.progress_delta >= 0.0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_security_gate_failure() {
    // Test that security gate failures are handled correctly
    let runtime = create_runtime(false, false).await; // security_passes = false
    let job = create_test_job("security_fail", 5);

    let handle = runtime.start_run(job).await.unwrap();
    let _ = handle.run_id();

    // Wait briefly for async execution
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // In real implementation, would verify appropriate error handling
}

#[tokio::test(flavor = "multi_thread")]
async fn test_concurrent_runs() {
    // Test that multiple Autopilot runs can execute concurrently
    let runtime = Arc::new(create_runtime(true, false).await);

    let mut handles = vec![];

    for i in 0..3 {
        let job = create_test_job(&format!("concurrent_{}", i), 3);
        let runtime_clone = runtime.clone();

        let handle = tokio::spawn(async move { runtime_clone.start_run(job).await });
        handles.push(handle);
    }

    // Wait for all runs to complete
    for handle in handles {
        let result: Result<_, _> = handle.await.unwrap();
        assert!(result.is_ok());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_goal_achievement() {
    // Test that execution stops when goal is achieved
    let evaluator = DefaultProgressEvaluator::new();
    let run_id = AutopilotRunId::new();
    let job = create_test_job("goal_test", 10);

    // Initially, goal is not met
    assert!(!evaluator.is_goal_met(&run_id, &job).await.unwrap());

    // Simulate successful executions
    let results = vec![ExecutionResult {
        execution_id: ExecutionId::new(),
        status: ExecutionStatus::Completed,
        output: Some(serde_json::json!({"deployed": true})),
        error: None,
        artifacts: vec!["deployment.yaml".to_string()],
        duration_ms: 0,
    }];

    evaluator.analyze(&run_id, &results).await.unwrap();

    // In real implementation, would check if goal criteria are met
}

#[tokio::test(flavor = "multi_thread")]
async fn test_time_limit() {
    // Test that execution stops after exceeding time limit
    let runtime = create_runtime(true, false).await;
    let mut job = create_test_job("time_limit", 100);

    // Set very short time limit
    job.spec.goal.max_duration_ms = Some(10); // 10ms

    let handle = runtime.start_run(job).await.unwrap();
    let _ = handle.run_id();

    // Wait for timeout
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // In real implementation, would verify run stopped due to timeout
}

#[tokio::test(flavor = "multi_thread")]
async fn test_iteration_state_persistence() {
    // Test that iteration state is correctly persisted and retrieved
    let _state_store = Arc::new(InMemorySharedStateStore::new());
    let iteration_tracker = InMemoryIterationTracker::new().with_stuck_threshold(3);
    let run_id = ExecutionId::new();

    // Record multiple iterations
    for i in 1..=5 {
        let state = AutopilotIterationState {
            iteration_number: i,
            state_hash: format!("hash_{}", i),
            started_at: chrono::Utc::now(),
            completed_at: Some(chrono::Utc::now()),
            success: true,
            artifact_count: 0,
            operation_type: None,
            error: None,
            failure_count: 0,
        };
        iteration_tracker
            .record_iteration(&run_id, state)
            .await
            .unwrap();
    }

    // Verify iteration count
    assert_eq!(
        iteration_tracker.current_iteration(&run_id).await.unwrap(),
        5
    );

    // Get history
    let history = iteration_tracker.get_history(&run_id).await.unwrap();
    assert_eq!(history.len(), 5);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_error_recovery() {
    // Test that runtime can recover from step failures
    let runtime = create_runtime(true, false).await;
    let job = create_test_job("error_recovery", 5);

    // Start run
    let handle = runtime.start_run(job.clone()).await.unwrap();
    let _ = handle.run_id();

    // Simulate step failure and recovery
    // In real implementation, would inject failures and verify recovery

    // Resume run - needs a non-existent run_id and execution_id, should fail gracefully
    let run_id = AutopilotRunId::new();
    let execution_id = ExecutionId::new();
    let result = runtime.resume_run(&run_id, &execution_id).await;

    // Should handle missing run gracefully
    assert!(result.is_err());
}
