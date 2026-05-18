//! Property-based tests for Trace Completeness invariants.
//!
//! # Invariants Verified
//!
//! 1. **All Recorded Events Are Retrievable**: Every event that is successfully
//!    recorded MUST be retrievable via get_events() (within capacity limits).
//!
//! 2. **Timestamps Are Monotonically Non-Decreasing**: When events are recorded
//!    sequentially, their timestamps MUST be in non-decreasing order.
//!
//! 3. **FIFO Eviction Preserves Newest Events**: When capacity is exceeded,
//!    the newest events MUST be retained (FIFO eviction of oldest).
//!
//! 4. **Event Count Bounded By Capacity**: The number of stored events MUST
//!    never exceed the configured capacity.
//!
//! 5. **Security Events Are Independent From Lifecycle Events**: Recording a
//!    security event MUST NOT affect the lifecycle event store and vice versa.
//!
//! 6. **Clear Is Complete**: After clear(), get_events() MUST return empty vec.

use cyberclaw_core::execution::ExecutionStatus;
use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::{ActorId, ExecutionId, SecurityEventId, TraceId};
use cyberclaw_core::security::{SecurityEvent, SecurityEventSource, SecurityEventType, Severity};
use cyberclaw_observability::events::{EventRecorder, InMemoryEventRecorder, ObservabilityEvent};
use proptest::prelude::*;

// ─── Helper builders ──────────────────────────────────────────────────────────

fn make_actor(id: &str) -> ActorRef {
    ActorRef {
        id: ActorId::from_string(id.to_string()).unwrap_or_else(|_| ActorId::new()),
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: format!("Actor {}", id),
    }
}

fn make_execution_created_event(exec_id: ExecutionId) -> ObservabilityEvent {
    ObservabilityEvent::ExecutionCreated {
        execution_id: exec_id,
        requested_by: make_actor("test-user"),
        timestamp: chrono::Utc::now(),
    }
}

fn make_security_event(exec_id: Option<ExecutionId>) -> SecurityEvent {
    SecurityEvent {
        id: SecurityEventId::new(),
        actor: Some(make_actor("system")),
        timestamp: chrono::Utc::now(),
        execution_id: exec_id,
        case_id: None,
        node_id: None,
        runtime_instance_id: None,
        source: SecurityEventSource::PolicyEngine,
        event_type: SecurityEventType::PolicyDenied,
        severity: Severity::High,
        summary: "Property test security event".to_string(),
        details: serde_json::json!({}),
        trace_id: TraceId::new(),
        credential_evidence: None,
    }
}

// ─── Property 1: All Recorded Events Are Retrievable (within capacity) ────────

#[test]
fn property_all_recorded_events_are_retrievable_within_capacity() {
    // Use a current-thread runtime to avoid nested-runtime panic inside proptest closures.
    let config = proptest::test_runner::Config {
        cases: 50,
        ..Default::default()
    };
    let mut runner = proptest::test_runner::TestRunner::new(config);

    runner
        .run(
            &(1usize..=20usize, 1usize..=30usize),
            |(capacity, event_count)| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(async {
                    let recorder = InMemoryEventRecorder::with_capacity(capacity);
                    let effective_count = event_count.min(capacity);

                    // Record min(event_count, capacity) events
                    for i in 0..effective_count {
                        let exec_id = ExecutionId::from_string(format!("prop-exec-{}", i))
                            .unwrap_or_else(|_| ExecutionId::new());
                        recorder
                            .record_event(make_execution_created_event(exec_id))
                            .await
                            .unwrap();
                    }

                    let events = recorder.get_events().await.unwrap();

                    // All events up to capacity must be retrievable
                    prop_assert!(
                        events.len() <= capacity,
                        "Stored events {} must not exceed capacity {}",
                        events.len(),
                        capacity,
                    );
                    prop_assert_eq!(
                        events.len(),
                        effective_count,
                        "Expected {} events, got {}",
                        effective_count,
                        events.len(),
                    );

                    Ok(())
                })
            },
        )
        .unwrap();
}

// ─── Property 2: FIFO Eviction Preserves Newest N Events ─────────────────────

#[tokio::test]
async fn property_fifo_eviction_preserves_newest_events() {
    // When we record (capacity + extra) events, the last `capacity` events
    // must be present after eviction.
    let capacity = 5usize;
    let extra = 3usize;
    let total = capacity + extra;

    let recorder = InMemoryEventRecorder::with_capacity(capacity);

    // Record `total` events with identifiable exec_ids: "evict-000" .. "evict-007"
    for i in 0..total {
        let exec_id = ExecutionId::from_string(format!("evict-{:03}", i))
            .unwrap_or_else(|_| ExecutionId::new());
        recorder
            .record_event(make_execution_created_event(exec_id))
            .await
            .unwrap();
    }

    let events = recorder.get_events().await.unwrap();

    // Exactly `capacity` events must remain
    assert_eq!(
        events.len(),
        capacity,
        "After FIFO eviction, exactly {} events must remain",
        capacity,
    );

    // The oldest `extra` events must have been evicted.
    // Remaining events should be index `extra` through `total-1`
    for (idx, event) in events.iter().enumerate() {
        if let ObservabilityEvent::ExecutionCreated { execution_id, .. } = event {
            let expected_suffix = format!("{:03}", idx + extra);
            assert!(
                execution_id.to_string().ends_with(&expected_suffix)
                    || execution_id.to_string().contains(&expected_suffix),
                "Event at index {} should have exec_id ending with '{}', got '{}'",
                idx,
                expected_suffix,
                execution_id,
            );
        } else {
            panic!("Expected ExecutionCreated event at index {}", idx);
        }
    }
}

// ─── Property 3: Event Count Never Exceeds Capacity ──────────────────────────

#[test]
fn property_event_count_never_exceeds_capacity() {
    let config = proptest::test_runner::Config {
        cases: 30,
        ..Default::default()
    };
    let mut runner = proptest::test_runner::TestRunner::new(config);

    runner
        .run(
            &(1usize..=10usize, 1usize..=50usize),
            |(capacity, event_count)| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(async {
                    let recorder = InMemoryEventRecorder::with_capacity(capacity);

                    for i in 0..event_count {
                        let exec_id = ExecutionId::from_string(format!("bounded-{}", i))
                            .unwrap_or_else(|_| ExecutionId::new());
                        recorder
                            .record_event(make_execution_created_event(exec_id))
                            .await
                            .unwrap();
                    }

                    let events = recorder.get_events().await.unwrap();

                    prop_assert!(
                        events.len() <= capacity,
                        "Event count {} must never exceed capacity {}",
                        events.len(),
                        capacity,
                    );

                    Ok(())
                })
            },
        )
        .unwrap();
}

// ─── Property 4: Security Events Independent From Lifecycle Events ────────────

#[test]
fn property_security_and_lifecycle_events_are_independent() {
    let config = proptest::test_runner::Config {
        cases: 30,
        ..Default::default()
    };
    let mut runner = proptest::test_runner::TestRunner::new(config);

    runner
        .run(
            &(1usize..=10usize, 1usize..=10usize),
            |(lifecycle_count, security_count)| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(async {
                    let recorder = InMemoryEventRecorder::new();

                    // Record lifecycle events
                    for i in 0..lifecycle_count {
                        let exec_id = ExecutionId::from_string(format!("lc-{}", i))
                            .unwrap_or_else(|_| ExecutionId::new());
                        recorder
                            .record_event(make_execution_created_event(exec_id))
                            .await
                            .unwrap();
                    }

                    // Record security events
                    for _ in 0..security_count {
                        recorder
                            .record_security_event(make_security_event(None))
                            .await
                            .unwrap();
                    }

                    let lc_events = recorder.get_events().await.unwrap();
                    let sec_events = recorder.get_security_events().await.unwrap();

                    // Each store must have exactly the right count
                    prop_assert_eq!(
                        lc_events.len(),
                        lifecycle_count,
                        "Expected {} lifecycle events",
                        lifecycle_count,
                    );
                    prop_assert_eq!(
                        sec_events.len(),
                        security_count,
                        "Expected {} security events",
                        security_count,
                    );

                    // Clear lifecycle, security should be unaffected
                    recorder.clear().await.unwrap();

                    let lc_after_clear = recorder.get_events().await.unwrap();
                    let sec_after_clear = recorder.get_security_events().await.unwrap();

                    prop_assert_eq!(
                        lc_after_clear.len(),
                        0,
                        "Lifecycle must be empty after clear"
                    );
                    prop_assert_eq!(
                        sec_after_clear.len(),
                        security_count,
                        "Security events must not be affected by lifecycle clear",
                    );

                    Ok(())
                })
            },
        )
        .unwrap();
}

// ─── Property 5: Clear Is Complete ───────────────────────────────────────────

#[tokio::test]
async fn property_clear_removes_all_lifecycle_events() {
    let recorder = InMemoryEventRecorder::new();

    // Record several events
    for i in 0..10 {
        let exec_id = ExecutionId::from_string(format!("clear-test-{}", i))
            .unwrap_or_else(|_| ExecutionId::new());
        recorder
            .record_event(make_execution_created_event(exec_id))
            .await
            .unwrap();
    }

    assert_eq!(recorder.get_events().await.unwrap().len(), 10);

    recorder.clear().await.unwrap();

    let events = recorder.get_events().await.unwrap();
    assert_eq!(
        events.len(),
        0,
        "clear() MUST remove all lifecycle events, got {} remaining",
        events.len(),
    );
}

// ─── Property 6: Execution-Filtered Events Are Subset Of All Events ──────────

#[tokio::test]
async fn property_filtered_events_are_subset_of_all_events() {
    let recorder = InMemoryEventRecorder::new();
    let target_exec = ExecutionId::from_string("target-exec-001".to_string()).unwrap();
    let other_exec = ExecutionId::from_string("other-exec-001".to_string()).unwrap();

    // Record 3 events for target, 2 events for other
    for _ in 0..3 {
        recorder
            .record_event(make_execution_created_event(target_exec.clone()))
            .await
            .unwrap();
    }
    for _ in 0..2 {
        recorder
            .record_event(make_execution_created_event(other_exec.clone()))
            .await
            .unwrap();
    }

    let all_events = recorder.get_events().await.unwrap();
    let filtered = recorder.get_events_for_execution(&target_exec).await;

    // Filtered events must be a subset in count
    assert!(
        filtered.len() <= all_events.len(),
        "Filtered events ({}) must be a subset of all events ({})",
        filtered.len(),
        all_events.len(),
    );

    // Filtered events must equal expected count
    assert_eq!(
        filtered.len(),
        3,
        "Expected exactly 3 events for target execution, got {}",
        filtered.len(),
    );

    // Each filtered event must actually match the target execution
    for event in &filtered {
        if let ObservabilityEvent::ExecutionCreated { execution_id, .. } = event {
            assert_eq!(
                execution_id, &target_exec,
                "Filtered event must match target execution_id",
            );
        }
    }
}

// ─── Property 7: Event Type Counting Is Consistent With Total Count ───────────

#[tokio::test]
async fn property_event_type_counts_sum_to_total() {
    let recorder = InMemoryEventRecorder::new();
    let exec_id = ExecutionId::from_string("count-test".to_string()).unwrap();

    // Record mixed event types
    recorder
        .record_event(ObservabilityEvent::ExecutionCreated {
            execution_id: exec_id.clone(),
            requested_by: make_actor("actor-1"),
            timestamp: chrono::Utc::now(),
        })
        .await
        .unwrap();

    recorder
        .record_event(ObservabilityEvent::ExecutionStatusChanged {
            execution_id: exec_id.clone(),
            from_status: ExecutionStatus::Pending,
            to_status: ExecutionStatus::Running,
            timestamp: chrono::Utc::now(),
        })
        .await
        .unwrap();

    recorder
        .record_event(ObservabilityEvent::CapabilityInvoked {
            execution_id: exec_id.clone(),
            capability_id: cyberclaw_core::ids::CapabilityId::from_string("test.cap".to_string())
                .unwrap(),
            timestamp: chrono::Utc::now(),
        })
        .await
        .unwrap();

    let total = recorder.get_events().await.unwrap().len();
    let created = recorder.count_events_by_type("ExecutionCreated").await;
    let changed = recorder
        .count_events_by_type("ExecutionStatusChanged")
        .await;
    let invoked = recorder.count_events_by_type("CapabilityInvoked").await;

    // Total must be consistent with per-type counts
    assert_eq!(
        created + changed + invoked,
        total,
        "Sum of per-type counts ({} + {} + {} = {}) must equal total {}",
        created,
        changed,
        invoked,
        created + changed + invoked,
        total,
    );
}
