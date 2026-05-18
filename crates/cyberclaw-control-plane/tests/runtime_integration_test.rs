//! Runtime integration tests for Task 6.3 / 6.4
//!
//! Tests the full execution lifecycle:
//! submit → running → completed/failed, verifying:
//! - correct state transitions
//! - observability events emitted
//! - error handling via runtime failures
//! - concurrent execution performance

use cyberclaw_agent_runtime::{AgentRuntime, MockAgentRuntime};
use cyberclaw_control_plane::{ExecutionRequest, ExecutionService, InMemoryExecutionService};
use cyberclaw_core::enums::Priority;
use cyberclaw_core::execution::ExecutionStatus;
use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::{ActorId, AgentId, ExecutionId, TaskId};
use cyberclaw_core::task::{Task, TaskInput, TaskKind, TriggerRef};
use cyberclaw_observability::events::InMemoryEventRecorder;
use cyberclaw_skill_runtime::{MinimalSkillRuntime, SkillRuntime};
use std::sync::Arc;
use tokio::time::{timeout, Duration};

// ─────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────

fn make_actor() -> ActorRef {
    ActorRef {
        id: ActorId::from_string("test-actor".to_string()).unwrap(),
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: "Test Actor".to_string(),
    }
}

fn make_task(title: &str) -> Task {
    Task {
        id: TaskId::new(),
        case_id: None,
        title: title.to_string(),
        summary: "Integration test task".to_string(),
        kind: TaskKind::Analysis,
        priority: Priority::Low,
        requested_by: make_actor(),
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "test".to_string(),
            source: "runtime_integration_test".to_string(),
        },
        input: TaskInput::default(),
        desired_outputs: vec![],
        labels: vec!["test".to_string()],
        preferred_agent_id: None,
    }
}

fn make_control_plane_context() -> cyberclaw_control_plane::ControlPlaneContext {
    cyberclaw_control_plane::ControlPlaneContext {
        actor: make_actor(),
        session: None,
        workspace: None,
    }
}

fn build_service_with_mock(
    mock: MockAgentRuntime,
) -> (Arc<InMemoryExecutionService>, Arc<InMemoryEventRecorder>) {
    let recorder = Arc::new(InMemoryEventRecorder::new());
    let agent_runtime: Arc<dyn AgentRuntime> = Arc::new(mock);
    let skill_runtime: Arc<dyn SkillRuntime> = Arc::new(MinimalSkillRuntime::new());
    let service = Arc::new(InMemoryExecutionService::with_runtimes_and_recorder(
        agent_runtime,
        skill_runtime,
        recorder.clone() as Arc<dyn cyberclaw_observability::events::EventRecorder>,
    ));
    (service, recorder)
}

async fn submit_and_get_id(service: &InMemoryExecutionService, title: &str) -> ExecutionId {
    let task = make_task(title);
    let execution_id = ExecutionId::new();
    let agent_id = AgentId::from_string("test-agent".to_string()).unwrap();
    let request = ExecutionRequest {
        execution_id: execution_id.clone(),
        task,
        case: None,
        context: make_control_plane_context(),
        agent: Some(cyberclaw_core::execution::AgentRef {
            id: agent_id,
            role: "worker".to_string(),
        }),
        trace_id: None,
        execution_mode: None,
        plan: None,
    };
    service.submit(request).await.unwrap();
    execution_id
}

// ─────────────────────────────────────────────
// Test 1: 完整执行流程：submit -> running -> completed
// ─────────────────────────────────────────────

#[tokio::test]
async fn test_full_execution_lifecycle_success() {
    let (service, recorder) = build_service_with_mock(MockAgentRuntime::new());

    let exec_id = submit_and_get_id(&service, "Full lifecycle task").await;

    // Verify initial state is Pending
    let exec = service.get(&exec_id).await.unwrap().unwrap();
    assert!(matches!(exec.status, ExecutionStatus::Pending));
    assert!(exec.started_at.is_none());

    // Execute
    service.execute(&exec_id).await.unwrap();

    // Verify final state
    let exec = service.get(&exec_id).await.unwrap().unwrap();
    assert!(
        matches!(exec.status, ExecutionStatus::Completed),
        "expected Completed, got {:?}",
        exec.status
    );
    assert!(exec.started_at.is_some(), "started_at should be set");
    assert!(exec.finished_at.is_some(), "finished_at should be set");

    // Wait briefly for async event writes to flush
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Verify observability events
    let events = recorder.get_events_for_execution(&exec_id).await;
    assert!(!events.is_empty(), "should have recorded events");

    let status_changes: Vec<_> = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                cyberclaw_observability::events::ObservabilityEvent::ExecutionStatusChanged { .. }
            )
        })
        .collect();

    assert!(
        status_changes.len() >= 2,
        "expected at least 2 status change events (Pending→Running, Running→Completed), got {}",
        status_changes.len()
    );

    let agent_completed = recorder
        .count_events_by_type("AgentExecutionCompleted")
        .await;
    assert_eq!(
        agent_completed, 1,
        "expected 1 AgentExecutionCompleted event"
    );
}

// ─────────────────────────────────────────────
// Test 2: 错误场景 - runtime 失败
// ─────────────────────────────────────────────

#[tokio::test]
async fn test_execution_runtime_failure() {
    let (service, recorder) =
        build_service_with_mock(MockAgentRuntime::with_failure("simulated runtime error"));

    let exec_id = submit_and_get_id(&service, "Failing task").await;

    let result = service.execute(&exec_id).await;
    assert!(result.is_err(), "execute should return an error");
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("simulated runtime error"),
        "error message should propagate"
    );

    // Execution should be in Failed state
    let exec = service.get(&exec_id).await.unwrap().unwrap();
    assert!(
        matches!(exec.status, ExecutionStatus::Failed),
        "expected Failed, got {:?}",
        exec.status
    );
    assert!(
        exec.finished_at.is_some(),
        "finished_at should be set on failure"
    );

    tokio::time::sleep(Duration::from_millis(20)).await;

    // Verify Failed status change event was recorded
    let events = recorder.get_events_for_execution(&exec_id).await;
    let failed_events: Vec<_> = events
        .iter()
        .filter(|e| {
            if let cyberclaw_observability::events::ObservabilityEvent::ExecutionStatusChanged {
                to_status,
                ..
            } = e
            {
                matches!(to_status, ExecutionStatus::Failed)
            } else {
                false
            }
        })
        .collect();

    assert!(
        !failed_events.is_empty(),
        "should have a Failed status change event"
    );
}

// ─────────────────────────────────────────────
// Test 3: execute 不存在的 execution 应返回错误
// ─────────────────────────────────────────────

#[tokio::test]
async fn test_execute_nonexistent_returns_error() {
    let (service, _recorder) = build_service_with_mock(MockAgentRuntime::new());

    let missing_id = ExecutionId::new();
    let result = service.execute(&missing_id).await;
    assert!(
        result.is_err(),
        "executing a missing execution should error"
    );
    assert!(
        result.unwrap_err().to_string().contains("not found"),
        "error should mention 'not found'"
    );
}

// ─────────────────────────────────────────────
// Test 4: cancel 后状态正确
// ─────────────────────────────────────────────

#[tokio::test]
async fn test_cancel_transitions_to_cancelled() {
    let (service, _recorder) = build_service_with_mock(MockAgentRuntime::new());

    let exec_id = submit_and_get_id(&service, "Cancel test task").await;

    service.cancel(&exec_id).await.unwrap();

    let exec = service.get(&exec_id).await.unwrap().unwrap();
    assert!(
        matches!(exec.status, ExecutionStatus::Cancelled),
        "expected Cancelled, got {:?}",
        exec.status
    );
    assert!(exec.finished_at.is_some());
}

// ─────────────────────────────────────────────
// Test 5: observability 事件正确按 execution_id 过滤
// ─────────────────────────────────────────────
// NOTE: 原 Test 5 "无 runtime 时 execute 仍然成功（no-op 路径）" 已删除，
//       因为它测试的是 silent no-op 行为，这是 P0 修复要解决的 bug。
//       现在，没有 runtime 且没有 actions 的执行会明确失败。
// ─────────────────────────────────────────────

#[tokio::test]
async fn test_observability_events_filtered_by_execution_id() {
    let (service, recorder) = build_service_with_mock(MockAgentRuntime::new());

    let id1 = submit_and_get_id(&service, "Task A").await;
    let id2 = submit_and_get_id(&service, "Task B").await;

    service.execute(&id1).await.unwrap();
    service.execute(&id2).await.unwrap();

    tokio::time::sleep(Duration::from_millis(30)).await;

    let events1 = recorder.get_events_for_execution(&id1).await;
    let events2 = recorder.get_events_for_execution(&id2).await;

    assert!(!events1.is_empty(), "should have events for execution 1");
    assert!(!events2.is_empty(), "should have events for execution 2");

    // Events for id1 should not contain id2's events
    for event in &events1 {
        if let cyberclaw_observability::events::ObservabilityEvent::ExecutionStatusChanged {
            execution_id,
            ..
        } = event
        {
            assert_eq!(execution_id, &id1, "event should belong to id1");
        }
    }
}

// ─────────────────────────────────────────────
// Test 7: 性能基准 - 100 个并发 execution
// ─────────────────────────────────────────────

#[tokio::test]
async fn test_concurrent_100_executions_performance() {
    let agent_runtime: Arc<dyn AgentRuntime> = Arc::new(MockAgentRuntime::new());
    let skill_runtime: Arc<dyn SkillRuntime> = Arc::new(MinimalSkillRuntime::new());
    let service = Arc::new(InMemoryExecutionService::with_runtimes(
        agent_runtime,
        skill_runtime,
    ));

    const N: usize = 100;
    let start = std::time::Instant::now();

    // Submit all executions
    let mut exec_ids = Vec::with_capacity(N);
    for i in 0..N {
        let exec_id = submit_and_get_id(&service, &format!("Concurrent task {}", i)).await;
        exec_ids.push(exec_id);
    }

    // Execute all concurrently
    let handles: Vec<_> = exec_ids
        .iter()
        .map(|exec_id| {
            let svc = service.clone();
            let id = exec_id.clone();
            tokio::spawn(async move { svc.execute(&id).await })
        })
        .collect();

    // Wait for all with a generous timeout (10 seconds for 100 executions)
    let results = timeout(Duration::from_secs(10), async {
        let mut results = Vec::with_capacity(N);
        for h in handles {
            results.push(h.await.unwrap());
        }
        results
    })
    .await
    .expect("concurrent executions timed out");

    let elapsed = start.elapsed();

    // All should succeed
    let failures: Vec<_> = results.iter().filter(|r| r.is_err()).collect();
    assert!(
        failures.is_empty(),
        "{} executions failed: {:?}",
        failures.len(),
        failures.first()
    );

    // Verify all are in Completed state
    for exec_id in &exec_ids {
        let exec = service.get(exec_id).await.unwrap().unwrap();
        assert!(
            matches!(exec.status, ExecutionStatus::Completed),
            "execution {} should be Completed",
            exec_id
        );
    }

    // Performance assertion: 100 executions should complete within 5 seconds
    assert!(
        elapsed.as_secs() < 5,
        "100 concurrent executions took {}ms (expected < 5000ms)",
        elapsed.as_millis()
    );
}

// ─────────────────────────────────────────────
// Test 8: 模拟超时场景（runtime 带延迟）
// ─────────────────────────────────────────────

#[tokio::test]
async fn test_execution_with_delayed_runtime() {
    // 50ms delay – should still complete
    let (service, _recorder) = build_service_with_mock(MockAgentRuntime::with_delay(50));

    let exec_id = submit_and_get_id(&service, "Delayed task").await;

    let result = timeout(Duration::from_secs(2), service.execute(&exec_id)).await;

    assert!(
        result.is_ok(),
        "execution with delay should complete within timeout"
    );
    assert!(result.unwrap().is_ok(), "delayed execution should succeed");

    let exec = service.get(&exec_id).await.unwrap().unwrap();
    assert!(matches!(exec.status, ExecutionStatus::Completed));
}

// ─────────────────────────────────────────────
// Test 9: AgentExecutionStarted 事件在 Running 之后发出
// ─────────────────────────────────────────────

#[tokio::test]
async fn test_agent_started_event_emitted() {
    let (service, recorder) = build_service_with_mock(MockAgentRuntime::new());

    let exec_id = submit_and_get_id(&service, "Event order test").await;
    service.execute(&exec_id).await.unwrap();

    tokio::time::sleep(Duration::from_millis(20)).await;

    let started_count = recorder.count_events_by_type("AgentExecutionStarted").await;
    assert_eq!(
        started_count, 1,
        "should have 1 AgentExecutionStarted event"
    );
}

// ─────────────────────────────────────────────
// Test 10: update_status API 仍然可用
// ─────────────────────────────────────────────

#[tokio::test]
async fn test_update_status_api_still_works() {
    let service = InMemoryExecutionService::default();

    let exec_id = submit_and_get_id(&service, "Status update test").await;

    service
        .update_status(&exec_id, ExecutionStatus::Running)
        .await
        .unwrap();

    let exec = service.get(&exec_id).await.unwrap().unwrap();
    assert!(matches!(exec.status, ExecutionStatus::Running));
    assert!(exec.started_at.is_some());

    service
        .update_status(&exec_id, ExecutionStatus::Completed)
        .await
        .unwrap();

    let exec = service.get(&exec_id).await.unwrap().unwrap();
    assert!(matches!(exec.status, ExecutionStatus::Completed));
    assert!(exec.finished_at.is_some());
}

// ─────────────────────────────────────────────
// Test 11: skill runtime 集成 - 使用 skill:: 前缀触发技能执行
// ─────────────────────────────────────────────

#[tokio::test]
async fn test_skill_runtime_integration() {
    use cyberclaw_skill_runtime::{EchoSkill, SkillHandler};

    let agent_runtime: Arc<dyn AgentRuntime> = Arc::new(MockAgentRuntime::new());
    let skill_runtime = Arc::new(MinimalSkillRuntime::new());

    // Register echo skill
    let skill_id = cyberclaw_core::ids::SkillId::from_string("echo".to_string()).unwrap();
    skill_runtime
        .register_skill(
            skill_id.clone(),
            Arc::new(EchoSkill) as Arc<dyn SkillHandler>,
        )
        .await;

    let recorder = Arc::new(InMemoryEventRecorder::new());
    let service = Arc::new(InMemoryExecutionService::with_runtimes_and_recorder(
        agent_runtime,
        skill_runtime,
        recorder.clone() as Arc<dyn cyberclaw_observability::events::EventRecorder>,
    ));

    // Create execution with skill:: prefix to trigger skill execution
    let task = make_task("Skill execution test");
    let execution_id = ExecutionId::new();
    let agent_id = AgentId::from_string("skill::echo".to_string()).unwrap();
    let request = ExecutionRequest {
        execution_id: execution_id.clone(),
        task,
        case: None,
        context: make_control_plane_context(),
        agent: Some(cyberclaw_core::execution::AgentRef {
            id: agent_id,
            role: "skill".to_string(),
        }),
        trace_id: None,
        execution_mode: None,
        plan: None,
    };
    service.submit(request).await.unwrap();

    // Execute
    service.execute(&execution_id).await.unwrap();

    // Verify execution completed successfully
    let exec = service.get(&execution_id).await.unwrap().unwrap();
    assert!(
        matches!(exec.status, ExecutionStatus::Completed),
        "expected Completed, got {:?}",
        exec.status
    );

    tokio::time::sleep(Duration::from_millis(20)).await;

    // Verify SkillInvoked and SkillCompleted events were recorded
    let skill_invoked = recorder.count_events_by_type("SkillInvoked").await;
    assert_eq!(skill_invoked, 1, "should have 1 SkillInvoked event");

    let skill_completed = recorder.count_events_by_type("SkillCompleted").await;
    assert_eq!(skill_completed, 1, "should have 1 SkillCompleted event");
}

// ─────────────────────────────────────────────
// Test 12: 不带 skill:: 前缀的 agent 不触发 skill runtime
// ─────────────────────────────────────────────

#[tokio::test]
async fn test_non_skill_agent_does_not_invoke_skill_runtime() {
    let agent_runtime: Arc<dyn AgentRuntime> = Arc::new(MockAgentRuntime::new());
    let skill_runtime = Arc::new(MinimalSkillRuntime::new());

    let recorder = Arc::new(InMemoryEventRecorder::new());
    let service = Arc::new(InMemoryExecutionService::with_runtimes_and_recorder(
        agent_runtime,
        skill_runtime,
        recorder.clone() as Arc<dyn cyberclaw_observability::events::EventRecorder>,
    ));

    // Create execution without skill:: prefix
    let exec_id = submit_and_get_id(&service, "Regular agent task").await;

    service.execute(&exec_id).await.unwrap();

    tokio::time::sleep(Duration::from_millis(20)).await;

    // Verify no SkillInvoked events were recorded
    let skill_invoked = recorder.count_events_by_type("SkillInvoked").await;
    assert_eq!(
        skill_invoked, 0,
        "should have 0 SkillInvoked events for non-skill agents"
    );
}

// ─────────────────────────────────────────────
// Test 13: skill runtime 执行失败时正确处理错误
// ─────────────────────────────────────────────

#[tokio::test]
async fn test_skill_runtime_failure_handling() {
    let agent_runtime: Arc<dyn AgentRuntime> = Arc::new(MockAgentRuntime::new());
    let skill_runtime = Arc::new(MinimalSkillRuntime::new());
    // Note: We do NOT register the skill, so invocation will fail

    let recorder = Arc::new(InMemoryEventRecorder::new());
    let service = Arc::new(InMemoryExecutionService::with_runtimes_and_recorder(
        agent_runtime,
        skill_runtime,
        recorder.clone() as Arc<dyn cyberclaw_observability::events::EventRecorder>,
    ));

    // Create execution with unregistered skill
    let task = make_task("Unregistered skill test");
    let execution_id = ExecutionId::new();
    let agent_id = AgentId::from_string("skill::nonexistent".to_string()).unwrap();
    let request = ExecutionRequest {
        execution_id: execution_id.clone(),
        task,
        case: None,
        context: make_control_plane_context(),
        agent: Some(cyberclaw_core::execution::AgentRef {
            id: agent_id,
            role: "skill".to_string(),
        }),
        trace_id: None,
        execution_mode: None,
        plan: None,
    };
    service.submit(request).await.unwrap();

    // Execute should fail
    let result = service.execute(&execution_id).await;
    assert!(
        result.is_err(),
        "execution should fail when skill is not found"
    );

    // Execution should be in Failed state
    let exec = service.get(&execution_id).await.unwrap().unwrap();
    assert!(
        matches!(exec.status, ExecutionStatus::Failed),
        "expected Failed, got {:?}",
        exec.status
    );

    tokio::time::sleep(Duration::from_millis(20)).await;

    // Verify SkillInvoked was recorded but not SkillCompleted
    let skill_invoked = recorder.count_events_by_type("SkillInvoked").await;
    assert_eq!(
        skill_invoked, 1,
        "should have 1 SkillInvoked event even on failure"
    );

    let skill_completed = recorder.count_events_by_type("SkillCompleted").await;
    assert_eq!(
        skill_completed, 0,
        "should have 0 SkillCompleted events on failure"
    );
}

#[tokio::test]
async fn test_duplicate_execution_submission_prevented() {
    let service = InMemoryExecutionService::default();

    // Use a specific execution ID
    let execution_id = ExecutionId::from_string("test-duplicate-exec".to_string()).unwrap();

    let request = ExecutionRequest {
        execution_id: execution_id.clone(),
        case: None,
        task: make_task("duplicate test"),
        context: make_control_plane_context(),
        trace_id: None,
        agent: None,
        execution_mode: None,
        plan: None,
    };

    // First submission should succeed
    let result1 = service.submit(request.clone()).await;
    assert!(
        result1.is_ok(),
        "first submission should succeed: {:?}",
        result1
    );

    // Second submission with same execution_id should fail (idempotency check)
    let result2 = service.submit(request).await;
    assert!(
        result2.is_err(),
        "second submission with same execution_id should fail"
    );
    assert!(
        result2.unwrap_err().to_string().contains("already exists"),
        "error should indicate execution already exists"
    );
}

// ─────────────────────────────────────────────
// P0-4 Security Tests: Verify silent success paths are eliminated
// ─────────────────────────────────────────────

/// P0-4: Test that execution fails when there's no runtime and no actions
/// This verifies the fix for the "silent success" bug where executions
/// would complete without doing anything.
#[tokio::test]
async fn test_execution_fails_when_no_runtime_and_no_actions() {
    // Create service with NO runtimes (neither agent nor skill runtime)
    let service = InMemoryExecutionService::new(); // Uses default() which has all runtimes as None

    let exec_id = submit_and_get_id(&service, "No runtime no actions test").await;

    // Execution should fail because there's nothing to execute
    let result = service.execute(&exec_id).await;
    assert!(
        result.is_err(),
        "execution should fail when no runtime and no actions available"
    );

    // Verify error message mentions "no executable content"
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("no executable content") || error_msg.contains("cannot be processed"),
        "error should mention missing executable content, got: {}",
        error_msg
    );

    // Verify execution status is Failed
    let exec = service.get(&exec_id).await.unwrap().unwrap();
    assert!(
        matches!(exec.status, ExecutionStatus::Failed),
        "expected Failed, got {:?}",
        exec.status
    );
}

/// P0-4: Test that execution fails when dispatcher is missing but plan has actions
/// This prevents silent failures when configuration is incomplete.
#[tokio::test]
async fn test_execution_fails_when_dispatcher_missing_but_actions_exist() {
    use cyberclaw_control_plane::{ExecutionPlan, PlannedAction, Resolution};
    use cyberclaw_core::ids::{AgentId, CapabilityId, ConnectorId};

    // Create service without capability dispatcher
    let service = InMemoryExecutionService::new(); // No dispatcher configured

    // Create a plan with actions (simulating what resolver.plan() would return)
    let plan = ExecutionPlan {
        resolution: Resolution {
            agent: AgentId::from_string("test-agent".to_string()).unwrap(),
            skills: vec![],
            workflow: None,
            connectors: vec![ConnectorId::from_string("test-connector".to_string()).unwrap()],
            capabilities: vec![CapabilityId::from_string("test.capability".to_string()).unwrap()],
            reasons: vec!["test reason".to_string()],
        },
        actions: vec![PlannedAction {
            capability: CapabilityId::from_string("test.capability".to_string()).unwrap(),
            connector_id: ConnectorId::from_string("test-connector".to_string()).unwrap(),
            input: serde_json::json!({"test": "data"}),
            reason: "test action".to_string(),
        }],
        review_required: false,
        max_fix_loops: cyberclaw_control_plane::default_max_fix_loops(),
        expected_outcomes: vec![],
    };

    // Submit the plan and get execution ID
    let exec_id = service.submit_plan(plan).await.unwrap();

    // Execution should fail because dispatcher is not configured
    let result = service.execute(&exec_id).await;
    assert!(
        result.is_err(),
        "execution should fail when dispatcher is missing but plan has actions"
    );

    // Verify error message mentions dispatcher configuration
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("capability dispatcher not configured")
            || error_msg.contains("with_capability_dispatcher"),
        "error should mention missing dispatcher configuration, got: {}",
        error_msg
    );

    // Verify execution status is Failed
    let exec = service.get(&exec_id).await.unwrap().unwrap();
    assert!(
        matches!(exec.status, ExecutionStatus::Failed),
        "expected Failed, got {:?}",
        exec.status
    );
}
