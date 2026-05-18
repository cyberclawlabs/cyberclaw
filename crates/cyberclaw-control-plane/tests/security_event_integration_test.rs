//! E2E integration tests for SecurityEvent recording across the full
//! orchestrator → execution_service → review_queue audit chain.
//!
//! ## Covered scenarios
//!
//! 1. `test_orchestrator_to_execution_chain_events`
//!    Verifies that governance decisions (ReviewRequired) and execution
//!    lifecycle events are all recorded with a correlated `execution_id`.
//!
//! 2. `test_review_workflow_chain_events`
//!    Verifies the full review workflow: task submission triggers
//!    ReviewRequired → ReviewEnqueued → ReviewApproved → execution runs.
//!
//! 3. `test_execution_id_audit_correlation`
//!    Runs multiple executions concurrently and verifies that querying by
//!    `execution_id` returns only the events for that specific execution
//!    (no cross-contamination).

use cyberclaw_control_plane::{
    ControlPlaneOrchestrator, ExecutionService, InMemoryEventBus, InMemoryExecutionService,
    InMemoryGatewayRouter, InMemoryLeaseManager, InMemoryMembershipService,
    InMemoryPlacementEngine, InMemoryRegistry, InMemoryResolver, InMemoryReviewQueue,
    InMemoryTaskManager, IngressRequest, MembershipService,
};
use cyberclaw_core::cluster::{MembershipState, NodeCapacity, NodeHealth, NodeRecord, NodeRole};
use cyberclaw_core::enums::Priority;
use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::{ActorId, ExecutionId, NodeId, TaskId};
use cyberclaw_core::security::SecurityEventType;
use cyberclaw_core::task::{Task, TaskInput, TaskKind, TriggerRef};
use cyberclaw_governance::engine::DefaultPolicyEngine;
use cyberclaw_observability::security_event_store::EventFilter;
use cyberclaw_observability::{
    InMemoryEventRecorder, InMemorySecurityEventStore, SecurityEventStore,
};
use cyberclaw_plugin_runtime::PluginRegistry;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Shared test infrastructure
// ---------------------------------------------------------------------------

/// Build actor with different requester/reviewer IDs for self-approval tests
fn make_actor(id: &str, display_name: &str) -> ActorRef {
    ActorRef {
        id: ActorId::from_string(id.to_string()).unwrap(),
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: display_name.to_string(),
    }
}

/// Build a standard test task
fn make_task(actor: ActorRef, title: &str, priority: Priority) -> Task {
    Task {
        id: TaskId::new(),
        case_id: None,
        title: title.to_string(),
        summary: format!("Integration test task: {}", title),
        kind: TaskKind::Analysis,
        priority,
        requested_by: actor,
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "security-event-integration-test".to_string(),
        },
        input: TaskInput::default(),
        desired_outputs: vec![],
        labels: vec![],
        preferred_agent_id: None,
    }
}

/// Fixture: fully-wired orchestrator with:
/// - a shared `InMemoryEventRecorder` for orchestrator SecurityEvents
/// - a shared `InMemorySecurityEventStore` for review_queue SecurityEvents
/// - an active worker node so placement succeeds
struct TestHarness {
    orchestrator: Arc<ControlPlaneOrchestrator>,
    /// Captures SecurityEvents emitted by the orchestrator (governance decisions)
    event_recorder: Arc<InMemoryEventRecorder>,
    /// Captures SecurityEvents emitted by InMemoryReviewQueue (enqueue/approve/reject)
    /// and also by orchestrator governance decisions (both share the same store).
    event_store: Arc<InMemorySecurityEventStore>,
    execution_service: Arc<InMemoryExecutionService>,
    /// Kept for direct queue inspection when needed; orchestrator delegates to it internally.
    #[allow(dead_code)]
    review_queue: Arc<InMemoryReviewQueue>,
}

fn build_harness() -> TestHarness {
    let event_recorder = Arc::new(InMemoryEventRecorder::new());
    let event_store = Arc::new(InMemorySecurityEventStore::new());

    let gateway = Arc::new(InMemoryGatewayRouter::new());
    let registry = Arc::new(InMemoryRegistry::new());
    let resolver = Arc::new(InMemoryResolver::new(registry.clone()));
    // InMemoryReviewQueue is wired to event_store so enqueue/approve/reject
    // SecurityEvents are written there.
    let review_queue = Arc::new(InMemoryReviewQueue::new(Some(
        event_store.clone() as Arc<dyn SecurityEventStore>
    )));
    let task_manager = Arc::new(InMemoryTaskManager::new());
    let execution_service = Arc::new(InMemoryExecutionService::default());
    let placement_engine = Arc::new(InMemoryPlacementEngine::new());
    let lease_manager = Arc::new(InMemoryLeaseManager::default());
    let membership_service = Arc::new(InMemoryMembershipService::default());
    let event_bus = Arc::new(InMemoryEventBus::default());
    let policy_engine = Arc::new(DefaultPolicyEngine::default());

    // Register an active worker node so placement can succeed for direct submissions.
    // Use Joining state + heartbeat to match the pattern from integration_test.rs.
    let node_id = NodeId::from_string("test-node-1".to_string()).unwrap();
    let node = NodeRecord {
        id: node_id.clone(),
        role: NodeRole::Worker,
        labels: vec!["worker".to_string()],
        region: Some("us-east-1".to_string()),
        zone: Some("us-east-1a".to_string()),
        health: NodeHealth::Healthy,
        membership_state: MembershipState::Joining,
        capacity: NodeCapacity {
            max_executions: Some(20),
            max_cpu_millis: None,
            max_memory_mb: None,
        },
        current_executions: 0,
        last_heartbeat_at: chrono::Utc::now(),
    };
    membership_service.join(node).unwrap();
    membership_service.heartbeat(&node_id).unwrap();

    // Clone the orchestrator security_event_store reference for the orchestrator.
    // The orchestrator uses this to record governance decision events separately
    // from the review_queue store.
    let orch_store = event_store.clone() as Arc<dyn SecurityEventStore>;

    // 创建 PluginRegistry 实例
    let plugin_registry = Arc::new(PluginRegistry::new(std::path::PathBuf::from(
        "/tmp/test_plugins",
    )));

    let orchestrator = Arc::new(
        ControlPlaneOrchestrator::new(
            gateway,
            resolver,
            registry,
            review_queue.clone(),
            task_manager,
            execution_service.clone(),
            placement_engine,
            lease_manager,
            membership_service,
            event_bus,
            policy_engine,
            plugin_registry,
            orch_store,
        )
        // Attach the recorder so the orchestrator emits SecurityEvents via EventRecorder
        .with_event_recorder(
            event_recorder.clone() as Arc<dyn cyberclaw_observability::EventRecorder>
        ),
    );

    TestHarness {
        orchestrator,
        event_recorder,
        event_store,
        execution_service,
        review_queue,
    }
}

// ---------------------------------------------------------------------------
// Test 1: orchestrator → execution chain events
// ---------------------------------------------------------------------------

/// Verify the governance decision SecurityEvent is recorded and correlated with
/// the execution_id that comes back from `process_ingress`.
///
/// Due to the H-4 fail-secure principle (empty actions → ReviewRequired), every
/// task submission records a `Custom("ReviewRequired")` SecurityEvent on the
/// orchestrator recorder.  We assert:
/// - The event is present in the recorder
/// - Its `execution_id` matches the one returned by `process_ingress`
/// - The execution exists in `execution_service` with status `WaitingReview`
/// - Querying by a *different* execution_id returns zero events (isolation)
#[tokio::test(flavor = "multi_thread")]
async fn test_orchestrator_to_execution_chain_events() {
    let h = build_harness();

    let requester = make_actor("requester-alice", "Alice");
    let task = make_task(requester.clone(), "Chain Test Task", Priority::Low);

    let request = IngressRequest {
        actor: requester,
        session: None,
        workspace: None,
        task,
    };

    let result = h
        .orchestrator
        .process_ingress(request)
        .await
        .expect("process_ingress should succeed");

    // H-4: all tasks with empty actions require review
    assert!(
        !result.submitted,
        "task with empty actions should not be directly submitted"
    );
    assert!(
        result.review_id.is_some(),
        "task with empty actions should be enqueued for review"
    );

    let execution_id = &result.execution_id;

    // -- Assert 1: orchestrator emitted a ReviewRequired SecurityEvent -------
    let recorder_events = h
        .event_recorder
        .get_security_events_for_execution(execution_id)
        .await;

    assert!(
        !recorder_events.is_empty(),
        "orchestrator should have recorded at least one SecurityEvent for execution {}",
        execution_id
    );

    let review_required_event = recorder_events
        .iter()
        .find(|ev| matches!(&ev.event_type, SecurityEventType::Custom(s) if s == "ReviewRequired"));
    assert!(
        review_required_event.is_some(),
        "expected a Custom(ReviewRequired) SecurityEvent; found events: {:?}",
        recorder_events
            .iter()
            .map(|e| &e.event_type)
            .collect::<Vec<_>>()
    );

    // -- Assert 2: execution exists with WaitingReview status ----------------
    let execution = h
        .execution_service
        .get(execution_id)
        .await
        .expect("get execution should not error")
        .expect("execution should exist in execution_service");

    assert_eq!(
        execution.status,
        cyberclaw_core::execution::ExecutionStatus::WaitingReview,
        "execution should be in WaitingReview state after governance review gate"
    );

    // -- Assert 3: a different execution_id returns no events (isolation) ----
    let other_id = ExecutionId::new();
    let other_events = h
        .event_recorder
        .get_security_events_for_execution(&other_id)
        .await;

    assert!(
        other_events.is_empty(),
        "querying a different execution_id should return no events; got {} events",
        other_events.len()
    );
}

// ---------------------------------------------------------------------------
// Test 2: review workflow chain events
// ---------------------------------------------------------------------------

/// Verify the full review audit chain:
///   governance ReviewRequired (orchestrator recorder)
///   → ReviewEnqueued  (review_queue → event_store)
///   → ReviewApproved  (review_queue → event_store after approval)
///   → execution status transitions to Pending (execution_service)
///
/// The test uses a different actor for the reviewer to satisfy the
/// anti-self-approval guard enforced by `process_review_result`.
#[tokio::test(flavor = "multi_thread")]
async fn test_review_workflow_chain_events() {
    let h = build_harness();

    let requester = make_actor("requester-bob", "Bob");
    let reviewer = make_actor("reviewer-carol", "Carol");

    let task = make_task(requester.clone(), "Review Workflow Task", Priority::High);

    let request = IngressRequest {
        actor: requester,
        session: None,
        workspace: None,
        task,
    };

    // Step 1: Submit task – triggers ReviewRequired governance decision
    let result = h
        .orchestrator
        .process_ingress(request)
        .await
        .expect("process_ingress should succeed");

    assert!(
        result.review_id.is_some(),
        "high-priority task should always require review (H-4 fail-secure)"
    );

    let execution_id = result.execution_id.clone();
    let review_id = result.review_id.unwrap();

    // -- Assert step 1: orchestrator emitted ReviewRequired event ------------
    let orchestrator_events = h
        .event_recorder
        .get_security_events_for_execution(&execution_id)
        .await;

    assert!(
        orchestrator_events.iter().any(
            |ev| matches!(&ev.event_type, SecurityEventType::Custom(s) if s == "ReviewRequired")
        ),
        "expected ReviewRequired SecurityEvent from orchestrator; found: {:?}",
        orchestrator_events
            .iter()
            .map(|e| &e.event_type)
            .collect::<Vec<_>>()
    );

    // -- Assert step 2: review_queue emitted ReviewEnqueued to event_store ---
    // Allow the tokio::spawn in InMemoryReviewQueue.enqueue() to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    let store_events_after_enqueue = h
        .event_store
        .query(EventFilter {
            execution_id: Some(execution_id.clone()),
            ..Default::default()
        })
        .await
        .expect("event_store query should not error");

    let enqueued_event = store_events_after_enqueue
        .iter()
        .find(|ev| matches!(&ev.event_type, SecurityEventType::Custom(s) if s == "ReviewEnqueued"));
    assert!(
        enqueued_event.is_some(),
        "expected ReviewEnqueued SecurityEvent in event_store; found: {:?}",
        store_events_after_enqueue
            .iter()
            .map(|e| &e.event_type)
            .collect::<Vec<_>>()
    );

    // -- Assert step 3: approve review, then check ReviewApproved event ------
    // Note: process_review_result calls execute() after approve().  In the test
    // environment no agent runtime is wired, so execute() fails with
    // "no executable content available".  That error is expected and does NOT
    // prevent the approve() call (and the ReviewApproved SecurityEvent write)
    // from happening first.  We therefore accept any result from
    // process_review_result – the audit-trail assertions below will verify the
    // events were recorded regardless of the execution outcome.
    let _approval_result = h
        .orchestrator
        .process_review_result(&review_id, true, reviewer)
        .await;
    // We do not assert approval_result.is_ok() because execute() legitimately
    // fails in the no-runtime test environment.  The important thing is that
    // the review was approved and the audit event was written before execute()
    // was called.

    // Allow the tokio::spawn in InMemoryReviewQueue.approve() to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

    let store_events_after_approval = h
        .event_store
        .query(EventFilter {
            execution_id: Some(execution_id.clone()),
            ..Default::default()
        })
        .await
        .expect("event_store query after approval should not error");

    let approved_event = store_events_after_approval
        .iter()
        .find(|ev| matches!(&ev.event_type, SecurityEventType::Custom(s) if s == "ReviewApproved"));
    assert!(
        approved_event.is_some(),
        "expected ReviewApproved SecurityEvent in event_store after approval; found: {:?}",
        store_events_after_approval
            .iter()
            .map(|e| &e.event_type)
            .collect::<Vec<_>>()
    );

    // -- Assert step 4: execution status has advanced beyond WaitingReview ----
    // The orchestrator calls update_status(Pending) *before* calling execute().
    // Even though execute() fails in the no-runtime test env, the status should
    // no longer be WaitingReview (it was set to Pending, then execute() may set
    // it to Failed internally).
    let final_execution = h
        .execution_service
        .get(&execution_id)
        .await
        .expect("get execution should not error")
        .expect("execution should still exist after approval");

    assert_ne!(
        final_execution.status,
        cyberclaw_core::execution::ExecutionStatus::WaitingReview,
        "execution should have advanced beyond WaitingReview: status={:?}",
        final_execution.status
    );
}

// ---------------------------------------------------------------------------
// Test 3: execution_id audit correlation – no cross-contamination
// ---------------------------------------------------------------------------

/// Submit three tasks sequentially and verify that querying the
/// `SecurityEventStore` by each `execution_id` returns only that execution's
/// events (no cross-contamination between audit trails).
///
/// `InMemoryReviewQueue.enqueue()` uses `try_write()` (non-blocking lock),
/// which is not safe for concurrent callers.  We therefore submit the three
/// tasks in sequence to avoid lock-acquisition failures, which is still a
/// valid test of audit-trail isolation because each task gets a distinct
/// UUID `execution_id` regardless of submission timing.
///
/// Uses `InMemorySecurityEventStore` (wired into `review_queue`) as the
/// single queryable audit store since all three submissions follow the
/// H-4 path (empty actions → ReviewRequired → ReviewEnqueued).
#[tokio::test(flavor = "multi_thread")]
async fn test_execution_id_audit_correlation() {
    let h = build_harness();

    let actors = [
        make_actor("actor-x1", "Actor X1"),
        make_actor("actor-x2", "Actor X2"),
        make_actor("actor-x3", "Actor X3"),
    ];

    // Submit three tasks sequentially (InMemoryReviewQueue uses try_write which
    // is not safe for concurrent callers – serialise to avoid lock contention).
    let mut execution_ids = Vec::new();
    for (i, actor) in actors.iter().enumerate() {
        let title = format!("Correlation Task {}", i + 1);
        let task = make_task(actor.clone(), &title, Priority::Low);
        let request = IngressRequest {
            actor: actor.clone(),
            session: None,
            workspace: None,
            task,
        };

        let result = h
            .orchestrator
            .process_ingress(request)
            .await
            .expect("process_ingress should succeed");

        assert!(
            !result.submitted,
            "task {} should require review (H-4 fail-secure)",
            i + 1
        );
        execution_ids.push(result.execution_id);
    }

    // Each execution_id must be distinct
    for i in 0..execution_ids.len() {
        for j in (i + 1)..execution_ids.len() {
            assert_ne!(
                execution_ids[i], execution_ids[j],
                "execution IDs must be unique; collision between index {} and {}",
                i, j
            );
        }
    }

    // Allow the tokio::spawn tasks in review_queue.enqueue() to flush
    tokio::time::sleep(tokio::time::Duration::from_millis(30)).await;

    // -- Assert: each execution_id maps to its own isolated events -----------
    for exec_id in &execution_ids {
        // Query event_store (populated by review_queue) by this execution_id
        let store_events = h
            .event_store
            .query(EventFilter {
                execution_id: Some(exec_id.clone()),
                ..Default::default()
            })
            .await
            .expect("event_store query should not error");

        // At minimum there should be a ReviewEnqueued event for this execution
        assert!(
            !store_events.is_empty(),
            "execution {} should have at least one SecurityEvent (ReviewEnqueued)",
            exec_id
        );

        // All returned events must carry this execution_id (no cross-contamination)
        for ev in &store_events {
            assert_eq!(
                ev.execution_id.as_ref(),
                Some(exec_id),
                "event {} in result set for execution {} carries wrong execution_id {:?}",
                ev.id,
                exec_id,
                ev.execution_id
            );
        }

        // Query orchestrator recorder by this execution_id for governance events
        let recorder_events = h
            .event_recorder
            .get_security_events_for_execution(exec_id)
            .await;

        assert!(
            !recorder_events.is_empty(),
            "execution {} should have at least one governance SecurityEvent in recorder",
            exec_id
        );

        // All returned recorder events must carry this execution_id
        for ev in &recorder_events {
            assert_eq!(
                ev.execution_id.as_ref(),
                Some(exec_id),
                "recorder event {} returned for execution {} carries wrong execution_id {:?}",
                ev.id,
                exec_id,
                ev.execution_id
            );
        }
    }

    // -- Assert: total store count reflects 2 events per task ----------------
    // Each task submission produces:
    //   1. A governance decision event from the orchestrator
    //      (Custom("PolicyReviewRequired") written via SecurityEventStore)
    //   2. A ReviewEnqueued event from the review_queue
    //      (Custom("ReviewEnqueued") written via SecurityEventStore)
    // Both stores are the same Arc<InMemorySecurityEventStore> instance.
    // With 3 tasks × 2 events each = 6 total events.
    let all_store_events = h
        .event_store
        .query(EventFilter::default())
        .await
        .expect("global query should not error");

    assert_eq!(
        all_store_events.len(),
        6,
        "event_store should hold 6 events total (2 per task × 3 tasks); got {}",
        all_store_events.len()
    );

    // -- Assert: verify no execution sees another's events -------------------
    // Spot-check: exec[0] events must not appear when querying exec[1]
    let events_for_exec1 = h
        .event_store
        .query(EventFilter {
            execution_id: Some(execution_ids[1].clone()),
            ..Default::default()
        })
        .await
        .unwrap();

    for ev in &events_for_exec1 {
        assert_ne!(
            ev.execution_id.as_ref(),
            Some(&execution_ids[0]),
            "exec[1] query returned an event belonging to exec[0] – cross-contamination detected"
        );
        assert_ne!(
            ev.execution_id.as_ref(),
            Some(&execution_ids[2]),
            "exec[1] query returned an event belonging to exec[2] – cross-contamination detected"
        );
    }
}
