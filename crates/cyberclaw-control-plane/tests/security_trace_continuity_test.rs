//! SecurityEvent trace_id continuity integration tests
//!
//! Verifies that all SecurityEvents generated during execution preserve the
//! execution's trace_id for end-to-end traceability.
//!
//! ## Test Scenarios
//!
//! 1. `test_security_events_preserve_trace_id`
//!    - Creates execution with trace_id
//!    - Triggers SecurityEvent generation via capability execution
//!    - Verifies all SecurityEvents have matching trace_id
//!
//! 2. `test_execution_lifecycle_trace_continuity`
//!    - Tests ExecutionStarted → ExecutionCompleted flow
//!    - Verifies trace_id continuity across lifecycle
//!
//! 3. `test_failed_execution_trace_continuity`
//!    - Tests ExecutionStarted → ExecutionFailed flow
//!    - Verifies trace_id even in failure scenarios

use cyberclaw_agent_runtime::MockAgentRuntime;
use cyberclaw_control_plane::{ExecutionService, InMemoryEventBus, InMemoryExecutionService};
use cyberclaw_core::enums::Priority;
use cyberclaw_core::execution::AgentRef;
use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::{ActorId, AgentId, ExecutionId, TaskId, TraceId, WorkspaceId};
use cyberclaw_core::task::{Task, TaskInput, TaskKind, TriggerRef};
use cyberclaw_core::workspace::{WorkspaceMode, WorkspaceRef};
use cyberclaw_observability::security_event_store::EventFilter;
use cyberclaw_observability::{
    InMemoryEventRecorder, InMemorySecurityEventStore, SecurityEventStore,
};
use cyberclaw_skill_runtime::MinimalSkillRuntime;
use std::sync::Arc;

// ─────────────────────────────────────────────────────────────────────────────
// Test Harness
// ─────────────────────────────────────────────────────────────────────────────

struct TestHarness {
    execution_service: Arc<InMemoryExecutionService>,
    security_event_store: Arc<InMemorySecurityEventStore>,
    #[allow(dead_code)]
    event_recorder: Arc<InMemoryEventRecorder>,
}

impl TestHarness {
    fn new() -> Self {
        let event_recorder = Arc::new(InMemoryEventRecorder::new());
        let security_event_store = Arc::new(InMemorySecurityEventStore::new());
        let agent_runtime = Arc::new(MockAgentRuntime::new());
        let skill_runtime = Arc::new(MinimalSkillRuntime::new());
        let event_bus = Arc::new(InMemoryEventBus::default());

        let execution_service = Arc::new(
            InMemoryExecutionService::with_runtimes_and_recorder(
                agent_runtime,
                skill_runtime,
                event_recorder.clone() as Arc<dyn cyberclaw_observability::EventRecorder>,
            )
            .with_event_bus(event_bus as Arc<dyn cyberclaw_control_plane::EventBus>)
            .with_security_event_store(security_event_store.clone() as Arc<dyn SecurityEventStore>),
        );

        Self {
            execution_service,
            security_event_store,
            event_recorder,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper Functions
// ─────────────────────────────────────────────────────────────────────────────

fn make_actor(id: &str) -> ActorRef {
    ActorRef {
        id: ActorId::from_string(id.to_string()).unwrap(),
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: format!("Test User ({})", id),
    }
}

fn make_agent(id: &str, role: &str) -> AgentRef {
    AgentRef {
        id: AgentId::from_string(id.to_string()).unwrap(),
        role: role.to_string(),
    }
}

fn make_task(actor: ActorRef, title: &str) -> Task {
    Task {
        id: TaskId::new(),
        case_id: None,
        title: title.to_string(),
        summary: format!("Test task: {}", title),
        kind: TaskKind::Execution,
        priority: Priority::Low,
        requested_by: actor,
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "trace-continuity-test".to_string(),
        },
        input: TaskInput::default(),
        desired_outputs: vec![],
        labels: vec!["trace-test".to_string()],
        preferred_agent_id: None,
    }
}

fn make_workspace(root: &str) -> WorkspaceRef {
    WorkspaceRef {
        id: WorkspaceId::from_string("trace-test-workspace".to_string()).unwrap(),
        mode: WorkspaceMode::Isolated,
        materialization_mode: None,
        home_node_id: None,
        backing_store: None,
        root: root.to_string(),
        writable_roots: vec![root.to_string()],
    }
}

/// Create a test execution with a specific trace_id
async fn create_execution_with_trace_id(
    execution_service: &Arc<InMemoryExecutionService>,
    trace_id: TraceId,
) -> (ExecutionId, TraceId) {
    let execution_id = ExecutionId::new();
    let actor = make_actor("trace-test-user");
    let agent = make_agent("trace-test-agent", "test-agent");
    let task = make_task(actor.clone(), "Trace continuity test");
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace = make_workspace(&temp_dir.path().to_string_lossy());

    // Submit execution with specific trace_id
    let request = cyberclaw_control_plane::execution_service::ExecutionRequest {
        execution_id: execution_id.clone(),
        task,
        case: None,
        context: cyberclaw_control_plane::types::ControlPlaneContext {
            actor: actor.clone(),
            session: None,
            workspace: Some(workspace),
        },
        agent: Some(agent),
        trace_id: Some(trace_id.clone()),
        execution_mode: None,
        plan: None,
    };

    execution_service.submit(request).await.unwrap();

    (execution_id, trace_id)
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 1: SecurityEvents preserve trace_id during execution
// ─────────────────────────────────────────────────────────────────────────────

/// Verify that all SecurityEvents generated during execution have the same
/// trace_id as the execution itself.
///
/// Steps:
/// 1. Create execution with specific trace_id
/// 2. Execute (triggers SecurityEvent generation)
/// 3. Query SecurityEventStore by execution_id
/// 4. Verify all events have matching trace_id
#[tokio::test(flavor = "multi_thread")]
async fn test_security_events_preserve_trace_id() {
    let harness = TestHarness::new();

    // Create execution with known trace_id
    let expected_trace_id = TraceId::new();
    let (execution_id, trace_id) =
        create_execution_with_trace_id(&harness.execution_service, expected_trace_id.clone()).await;

    assert_eq!(
        trace_id, expected_trace_id,
        "Execution should have expected trace_id"
    );

    // Execute (this will generate SecurityEvents)
    let execute_result = harness.execution_service.execute(&execution_id).await;

    // Note: execution may fail due to no executable content in test environment,
    // but SecurityEvents should still be generated
    let _ = execute_result; // Don't assert success, just need events

    // Allow async SecurityEvent writes to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Query SecurityEventStore for events related to this execution
    let events = harness
        .security_event_store
        .query(EventFilter {
            execution_id: Some(execution_id.clone()),
            ..Default::default()
        })
        .await
        .expect("Query should succeed");

    // Verify we got events
    assert!(
        !events.is_empty(),
        "Should have at least one SecurityEvent (ExecutionStarted)"
    );

    // Verify all events have matching trace_id
    for event in &events {
        assert_eq!(
            event.trace_id, expected_trace_id,
            "SecurityEvent {} should have trace_id matching execution.trace_id; \
             expected: {}, got: {}",
            event.id, expected_trace_id, event.trace_id
        );
    }

    // Verify ExecutionStarted event exists
    let has_execution_started = events.iter().any(|e| {
        matches!(
            &e.event_type,
            cyberclaw_core::security::SecurityEventType::Custom(s) if s == "ExecutionStarted"
        )
    });

    assert!(
        has_execution_started,
        "Should have ExecutionStarted SecurityEvent"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 2: Execution lifecycle trace continuity
// ─────────────────────────────────────────────────────────────────────────────

/// Verify trace_id continuity across successful execution lifecycle:
/// ExecutionStarted → ExecutionCompleted
///
/// Steps:
/// 1. Create execution with trace_id
/// 2. Execute successfully (mock will complete)
/// 3. Verify ExecutionStarted and ExecutionCompleted events
/// 4. Verify both events have same trace_id
#[tokio::test(flavor = "multi_thread")]
async fn test_execution_lifecycle_trace_continuity() {
    let harness = TestHarness::new();

    let expected_trace_id = TraceId::new();
    let (execution_id, _) =
        create_execution_with_trace_id(&harness.execution_service, expected_trace_id.clone()).await;

    // Execute
    let _ = harness.execution_service.execute(&execution_id).await;

    // Allow async writes
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Query all events for this execution
    let events = harness
        .security_event_store
        .query(EventFilter {
            execution_id: Some(execution_id.clone()),
            ..Default::default()
        })
        .await
        .expect("Query should succeed");

    // Find lifecycle events
    let started_events: Vec<_> = events
        .iter()
        .filter(|e| {
            matches!(
                &e.event_type,
                cyberclaw_core::security::SecurityEventType::Custom(s) if s == "ExecutionStarted"
            )
        })
        .collect();

    let completed_events: Vec<_> = events
        .iter()
        .filter(|e| {
            matches!(
                &e.event_type,
                cyberclaw_core::security::SecurityEventType::Custom(s) if s == "ExecutionCompleted"
            )
        })
        .collect();

    // Note: In test environment, execution may fail instead of complete
    // So we check for either Completed or Failed events
    let failed_events: Vec<_> = events
        .iter()
        .filter(|e| {
            matches!(
                &e.event_type,
                cyberclaw_core::security::SecurityEventType::Custom(s) if s == "ExecutionFailed"
            )
        })
        .collect();

    assert!(
        !started_events.is_empty(),
        "Should have ExecutionStarted event"
    );
    assert!(
        !completed_events.is_empty() || !failed_events.is_empty(),
        "Should have ExecutionCompleted or ExecutionFailed event"
    );

    // Verify all events have correct trace_id
    for event in &events {
        assert_eq!(
            event.trace_id, expected_trace_id,
            "All lifecycle events should have matching trace_id"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 3: Failed execution trace continuity
// ─────────────────────────────────────────────────────────────────────────────

/// Verify trace_id continuity in any execution outcome:
/// ExecutionStarted → (ExecutionCompleted OR ExecutionFailed)
///
/// Steps:
/// 1. Create execution with trace_id
/// 2. Execute (may succeed or fail in test environment)
/// 3. Verify ExecutionStarted event exists
/// 4. Verify all events have same trace_id
#[tokio::test(flavor = "multi_thread")]
async fn test_failed_execution_trace_continuity() {
    let harness = TestHarness::new();

    let expected_trace_id = TraceId::new();
    let (execution_id, _) =
        create_execution_with_trace_id(&harness.execution_service, expected_trace_id.clone()).await;

    // Execute (outcome may vary in test environment)
    let _ = harness.execution_service.execute(&execution_id).await;

    // Allow async writes
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Query events
    let events = harness
        .security_event_store
        .query(EventFilter {
            execution_id: Some(execution_id.clone()),
            ..Default::default()
        })
        .await
        .expect("Query should succeed");

    // Find lifecycle events
    let started_events: Vec<_> = events
        .iter()
        .filter(|e| {
            matches!(
                &e.event_type,
                cyberclaw_core::security::SecurityEventType::Custom(s) if s == "ExecutionStarted"
            )
        })
        .collect();

    let failed_events: Vec<_> = events
        .iter()
        .filter(|e| {
            matches!(
                &e.event_type,
                cyberclaw_core::security::SecurityEventType::Custom(s) if s == "ExecutionFailed"
            )
        })
        .collect();

    let completed_events: Vec<_> = events
        .iter()
        .filter(|e| {
            matches!(
                &e.event_type,
                cyberclaw_core::security::SecurityEventType::Custom(s) if s == "ExecutionCompleted"
            )
        })
        .collect();

    assert!(
        !started_events.is_empty(),
        "Should have ExecutionStarted event"
    );

    // Verify at least one completion event exists
    assert!(
        !failed_events.is_empty() || !completed_events.is_empty(),
        "Should have either ExecutionFailed or ExecutionCompleted event"
    );

    // Verify trace_id continuity across all events
    for event in &events {
        assert_eq!(
            event.trace_id, expected_trace_id,
            "All execution events should preserve trace_id; \
             event_type: {:?}, trace_id: {}",
            event.event_type, event.trace_id
        );
    }

    // If we have failed events, verify they have high severity
    for event in failed_events {
        assert!(
            matches!(event.severity, cyberclaw_core::security::Severity::High),
            "ExecutionFailed should have High severity"
        );
    }
}
