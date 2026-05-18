//! Unit tests for Autopilot types (8 tests)

use cyberclaw_control_plane::autopilot_types::*;
use cyberclaw_core::ids::ExecutionId;
use chrono::Utc;

#[test]
fn test_autopilot_run_state_creation() {
    let run_id = ExecutionId::new();
    let state = AutopilotRunState::new(run_id.clone(), "job-1".to_string());

    assert_eq!(state.run_id, run_id);
    assert_eq!(state.current_iteration, 0);
    assert!(state.iterations.is_empty());
    assert_eq!(state.status, AutopilotStatus::Initialized);
    assert_eq!(state.stuck_count, 0);
    assert!(state.last_state_hash.is_none());
}

#[test]
fn test_iteration_state_hash_computation() {
    let mut state = IterationState {
        iteration_id: 1,
        step: AutopilotStep::Execute,
        start_time: Utc::now(),
        end_time: None,
        state_hash: String::new(),
        progress_delta: None,
        execution_results: vec![],
    };

    let hash1 = state.compute_state_hash();
    assert_eq!(hash1.len(), 64); // SHA256 produces 64 hex chars

    // Add execution result and verify hash changes
    state.execution_results.push(ExecutionResult {
        execution_id: ExecutionId::new(),
        status: cyberclaw_control_plane::execution::ExecutionStatus::Completed,
        output: None,
        error: None,
        duration_ms: 100,
        trace_id: "trace-1".to_string(),
    });

    let hash2 = state.compute_state_hash();
    assert_ne!(hash1, hash2);
}

#[test]
fn test_autopilot_status_transitions() {
    // Test all status transitions
    let init_status = AutopilotStatus::Initialized;
    assert_eq!(
        init_status.to_execution_status(),
        cyberclaw_control_plane::execution::ExecutionStatus::Pending
    );

    let running = AutopilotStatus::Running {
        current_step: AutopilotStep::Plan,
    };
    assert_eq!(
        running.to_execution_status(),
        cyberclaw_control_plane::execution::ExecutionStatus::Running
    );

    let stuck = AutopilotStatus::Stuck {
        reason: "No progress".to_string(),
    };
    assert_eq!(
        stuck.to_execution_status(),
        cyberclaw_control_plane::execution::ExecutionStatus::Running
    );

    let completed = AutopilotStatus::Completed { iterations: 10 };
    assert_eq!(
        completed.to_execution_status(),
        cyberclaw_control_plane::execution::ExecutionStatus::Completed
    );
}

#[test]
fn test_autopilot_step_progression() {
    // Test complete 9-step cycle
    let steps = vec![
        AutopilotStep::Initialize,
        AutopilotStep::Plan,
        AutopilotStep::Execute,
        AutopilotStep::Review,
        AutopilotStep::Analyze,
        AutopilotStep::Decide,
        AutopilotStep::Update,
        AutopilotStep::Check,
        AutopilotStep::Iterate,
    ];

    for i in 0..steps.len() {
        let current = steps[i];
        let expected = steps[(i + 1) % steps.len()];
        assert_eq!(current.next(), expected);
    }
}

#[test]
fn test_autopilot_step_criticality() {
    // Critical steps need special handling
    assert!(AutopilotStep::Execute.is_critical());
    assert!(AutopilotStep::Review.is_critical());

    // Non-critical steps
    assert!(!AutopilotStep::Initialize.is_critical());
    assert!(!AutopilotStep::Plan.is_critical());
    assert!(!AutopilotStep::Analyze.is_critical());
    assert!(!AutopilotStep::Decide.is_critical());
    assert!(!AutopilotStep::Update.is_critical());
    assert!(!AutopilotStep::Check.is_critical());
    assert!(!AutopilotStep::Iterate.is_critical());
}

#[test]
fn test_review_gate_types() {
    // Test all review gate variants
    let gates = vec![
        ReviewGate::BeforeExecution,
        ReviewGate::AfterExecution,
        ReviewGate::HighRisk,
        ReviewGate::Custom("CustomCondition".to_string()),
    ];

    for gate in gates {
        let json = serde_json::to_string(&gate).unwrap();
        let deserialized: ReviewGate = serde_json::from_str(&json).unwrap();
        assert_eq!(gate, deserialized);
    }
}

#[test]
fn test_autopilot_job_creation() {
    let job = AutopilotJob {
        job_id: "job-1".to_string(),
        goal: "Test automation".to_string(),
        max_iterations: 50,
        review_gates: vec![ReviewGate::HighRisk],
        created_at: Utc::now(),
    };

    assert_eq!(job.job_id, "job-1");
    assert_eq!(job.goal, "Test automation");
    assert_eq!(job.max_iterations, 50);
    assert_eq!(job.review_gates.len(), 1);
}

#[test]
fn test_state_serialization_deserialization() {
    let run_id = ExecutionId::new();
    let mut state = AutopilotRunState::new(run_id.clone(), "job-1".to_string());

    // Modify state
    state.start_iteration(AutopilotStep::Initialize);
    state.stuck_count = 2;
    state.metadata.insert("key".to_string(), serde_json::json!("value"));

    // Serialize
    let json = serde_json::to_string(&state).unwrap();

    // Deserialize
    let deserialized: AutopilotRunState = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.run_id, run_id);
    assert_eq!(deserialized.current_iteration, 1);
    assert_eq!(deserialized.stuck_count, 2);
    assert_eq!(deserialized.metadata.get("key").unwrap(), &serde_json::json!("value"));
}