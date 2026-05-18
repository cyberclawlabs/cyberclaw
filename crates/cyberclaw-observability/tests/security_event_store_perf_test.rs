//! Performance stress tests for InMemorySecurityEventStore
//!
//! Validates that the store can handle production-level concurrent load.
//! All tests use only in-memory storage with no external dependencies.

use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::ActorId;
use cyberclaw_core::ids::{ExecutionId, SecurityEventId, TraceId};
use cyberclaw_core::security::{SecurityEvent, SecurityEventSource, SecurityEventType, Severity};
use cyberclaw_observability::security_event_store::{
    EventFilter, InMemorySecurityEventStore, SecurityEventStore,
};
use std::sync::Arc;
use std::time::Instant;

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn make_event(event_type: SecurityEventType, execution_id: Option<ExecutionId>) -> SecurityEvent {
    SecurityEvent {
        id: SecurityEventId::new(),
        actor: None,
        timestamp: chrono::Utc::now(),
        execution_id,
        case_id: None,
        node_id: None,
        runtime_instance_id: None,
        source: SecurityEventSource::RuntimeDetection,
        event_type,
        severity: Severity::Medium,
        summary: "perf-test event".to_string(),
        details: serde_json::json!({}),
        trace_id: TraceId::new(),
        credential_evidence: None,
    }
}

// ---------------------------------------------------------------------------
// Test 1: 1 000 concurrent store tasks
// ---------------------------------------------------------------------------

/// Spawns 1 000 concurrent tasks that each store a SecurityEvent.
/// Verifies all events are stored successfully and the operation
/// completes in under 5 seconds.
#[tokio::test]
async fn test_concurrent_store_1000_events() {
    let store = Arc::new(InMemorySecurityEventStore::new());
    let start = Instant::now();

    let handles: Vec<_> = (0_u32..1_000)
        .map(|i| {
            let store_ref = Arc::clone(&store);
            tokio::spawn(async move {
                // Each task builds its own unique execution_id
                let exec_id = ExecutionId::from_string(format!("conc-store-{:04}", i)).unwrap();
                store_ref
                    .store(make_event(
                        SecurityEventType::PermissionViolation,
                        Some(exec_id),
                    ))
                    .await
                    .unwrap();
            })
        })
        .collect();

    // Wait for every task to complete
    for handle in handles {
        handle.await.expect("spawned task panicked");
    }

    let elapsed = start.elapsed();

    // Correctness: every store call must have succeeded
    let total = store
        .count(EventFilter::default())
        .await
        .expect("count failed");
    assert_eq!(total, 1_000, "expected all 1 000 events to be stored");

    // Performance: must complete within 5 seconds
    assert!(
        elapsed.as_secs() < 5,
        "1 000 concurrent stores took {:.2}s (threshold: 5s)",
        elapsed.as_secs_f64()
    );

    println!(
        "[test_concurrent_store_1000_events] stored 1 000 events in {:.3}s",
        elapsed.as_secs_f64()
    );
}

// ---------------------------------------------------------------------------
// Test 2: query under concurrent load
// ---------------------------------------------------------------------------

/// Stores 10 000 events spread across 100 distinct execution_ids, then fires
/// 100 concurrent queries (one per execution_id) and verifies correctness.
///
/// Expected distribution: 10 000 events / 100 execution_ids = 100 events each.
/// Asserts that every query returns exactly 100 events and the average query
/// time is below 50 ms.
#[tokio::test]
async fn test_query_under_load() {
    const TOTAL_EVENTS: u32 = 10_000;
    const NUM_EXEC_IDS: u32 = 100;
    const EVENTS_PER_EXEC: usize = (TOTAL_EVENTS / NUM_EXEC_IDS) as usize; // 100

    // Build the 100 execution_ids up front
    let exec_ids: Vec<ExecutionId> = (0..NUM_EXEC_IDS)
        .map(|i| ExecutionId::from_string(format!("query-load-exec-{:03}", i)).unwrap())
        .collect();

    let store = Arc::new(InMemorySecurityEventStore::new());

    // --- Phase 1: sequential insert of 10 000 events ---
    for i in 0..TOTAL_EVENTS {
        let exec_id = exec_ids[(i % NUM_EXEC_IDS) as usize].clone();
        store
            .store(make_event(
                SecurityEventType::RuntimeAnomalyDetected,
                Some(exec_id),
            ))
            .await
            .expect("store failed during setup");
    }

    // Sanity: all events present before querying
    let setup_count = store.count(EventFilter::default()).await.unwrap();
    assert_eq!(
        setup_count, TOTAL_EVENTS as usize,
        "setup sanity: expected {TOTAL_EVENTS} events"
    );

    // --- Phase 2: 100 concurrent queries ---
    let query_start = Instant::now();

    let handles: Vec<_> = exec_ids
        .iter()
        .map(|exec_id| {
            let store_ref = Arc::clone(&store);
            let eid = exec_id.clone();
            tokio::spawn(async move {
                let t0 = Instant::now();
                let filter = EventFilter {
                    execution_id: Some(eid.clone()),
                    ..Default::default()
                };
                let results = store_ref.query(filter).await.expect("query failed");
                let query_ms = t0.elapsed().as_micros(); // microseconds for precision
                (eid, results, query_ms)
            })
        })
        .collect();

    let mut total_query_us: u128 = 0;
    for handle in handles {
        let (eid, results, query_us) = handle.await.expect("query task panicked");
        total_query_us += query_us;

        // Correctness: each execution_id must have exactly EVENTS_PER_EXEC results
        assert_eq!(
            results.len(),
            EVENTS_PER_EXEC,
            "execution_id {} returned {} events, expected {}",
            eid,
            results.len(),
            EVENTS_PER_EXEC,
        );

        // No duplicates: all event ids must be unique within the result set
        let mut seen_ids = std::collections::HashSet::new();
        for ev in &results {
            let inserted = seen_ids.insert(ev.id.as_str().to_string());
            assert!(
                inserted,
                "duplicate event id {} in results for {}",
                ev.id, eid
            );
        }

        // All returned events must belong to the queried execution_id
        for ev in &results {
            assert_eq!(
                ev.execution_id.as_ref().unwrap(),
                &eid,
                "event {} has wrong execution_id",
                ev.id
            );
        }
    }

    let wall_ms = query_start.elapsed().as_millis();
    let avg_query_ms = total_query_us / (NUM_EXEC_IDS as u128) / 1_000; // convert µs → ms

    // Performance: average query time must be < 50 ms
    assert!(
        avg_query_ms < 50,
        "average query time {}ms exceeds 50ms threshold",
        avg_query_ms
    );

    println!(
        "[test_query_under_load] 100 concurrent queries over 10 000 events: \
         wall={wall_ms}ms avg_query={avg_query_ms}ms"
    );
}

// ---------------------------------------------------------------------------
// Test 3: FIFO eviction behavior
// ---------------------------------------------------------------------------

/// Fills InMemorySecurityEventStore to its capacity (10 000), then writes
/// 1 000 additional events.  Verifies that:
///   1. The store contains exactly 10 000 events after the overflow.
///   2. The 1 000 oldest events (indices 0-999) have been evicted.
///   3. The 10 000 most-recently-stored events remain accessible by
///      execution_id.
#[tokio::test]
async fn test_fifo_eviction_behavior() {
    const CAPACITY: usize = 10_000;
    const OVERFLOW: usize = 1_000;

    let store = InMemorySecurityEventStore::with_capacity(CAPACITY);

    // --- Phase 1: fill to capacity with uniquely-tagged events ---
    // Events 0 .. 9999 use execution_ids "evict-fill-{:05}"
    for i in 0..CAPACITY {
        let exec_id = ExecutionId::from_string(format!("evict-fill-{:05}", i)).unwrap();
        store
            .store(make_event(SecurityEventType::PolicyDenied, Some(exec_id)))
            .await
            .expect("fill store failed");
    }

    // Sanity: store is exactly at capacity
    let at_cap = store.count(EventFilter::default()).await.unwrap();
    assert_eq!(
        at_cap, CAPACITY,
        "store should be at capacity before overflow"
    );

    // --- Phase 2: overflow with OVERFLOW more events ---
    // These use execution_ids "evict-new-{:05}" to distinguish them from the fill set
    for i in 0..OVERFLOW {
        let exec_id = ExecutionId::from_string(format!("evict-new-{:05}", i)).unwrap();
        store
            .store(make_event(
                SecurityEventType::PermissionViolation,
                Some(exec_id),
            ))
            .await
            .expect("overflow store failed");
    }

    // --- Assertion 1: total count stays at CAPACITY ---
    let after_overflow = store.count(EventFilter::default()).await.unwrap();
    assert_eq!(
        after_overflow, CAPACITY,
        "store should still hold exactly {CAPACITY} events after FIFO eviction"
    );

    // --- Assertion 2: oldest OVERFLOW events are gone ---
    // The first OVERFLOW fill events (indices 0 .. OVERFLOW-1) must be evicted
    for i in 0..OVERFLOW {
        let evicted_exec_id = ExecutionId::from_string(format!("evict-fill-{:05}", i)).unwrap();
        let filter = EventFilter {
            execution_id: Some(evicted_exec_id.clone()),
            ..Default::default()
        };
        let found = store.count(filter).await.unwrap();
        assert_eq!(
            found, 0,
            "evict-fill-{:05} should have been evicted (FIFO) but was found",
            i
        );
    }

    // --- Assertion 3: remaining fill events (OVERFLOW .. CAPACITY-1) are present ---
    for i in OVERFLOW..CAPACITY {
        let retained_exec_id = ExecutionId::from_string(format!("evict-fill-{:05}", i)).unwrap();
        let filter = EventFilter {
            execution_id: Some(retained_exec_id.clone()),
            ..Default::default()
        };
        let found = store.count(filter).await.unwrap();
        assert_eq!(
            found, 1,
            "evict-fill-{:05} should be retained but was not found",
            i
        );
    }

    // --- Assertion 4: all OVERFLOW new events are accessible ---
    for i in 0..OVERFLOW {
        let new_exec_id = ExecutionId::from_string(format!("evict-new-{:05}", i)).unwrap();
        let filter = EventFilter {
            execution_id: Some(new_exec_id.clone()),
            ..Default::default()
        };
        let found = store.count(filter).await.unwrap();
        assert_eq!(
            found, 1,
            "evict-new-{:05} should be present but was not found",
            i
        );
    }

    println!(
        "[test_fifo_eviction_behavior] capacity={CAPACITY}, overflow={OVERFLOW}: \
         oldest {OVERFLOW} events correctly evicted, newest {CAPACITY} events retained"
    );
}

// ---------------------------------------------------------------------------
// Test 4: actor precise filter
// ---------------------------------------------------------------------------

/// Tests that actor filter precisely matches events by actor id field on SecurityEvent.
/// Events with matching actor are returned; others are excluded.
#[tokio::test]
async fn test_actor_filter() {
    let store = InMemorySecurityEventStore::new();

    let actor_id = ActorId::from_string("actor-filter-test-001".to_string()).unwrap();
    let other_actor_id = ActorId::from_string("actor-filter-test-002".to_string()).unwrap();

    let actor = ActorRef {
        id: actor_id.clone(),
        actor_type: ActorType::Agent,
        tenant_id: None,
        home_node_id: None,
        display_name: "FilterTestAgent".to_string(),
    };
    let other_actor = ActorRef {
        id: other_actor_id.clone(),
        actor_type: ActorType::Agent,
        tenant_id: None,
        home_node_id: None,
        display_name: "OtherFilterTestAgent".to_string(),
    };

    // 3 events with matching actor set on the actor field
    for _ in 0..3 {
        let event = SecurityEvent {
            id: cyberclaw_core::ids::SecurityEventId::new(),
            actor: Some(actor.clone()),
            timestamp: chrono::Utc::now(),
            execution_id: None,
            case_id: None,
            node_id: None,
            runtime_instance_id: None,
            source: SecurityEventSource::RuntimeDetection,
            event_type: SecurityEventType::PermissionViolation,
            severity: Severity::High,
            summary: "actor filter test event".to_string(),
            details: serde_json::json!({}),
            trace_id: TraceId::new(),
            credential_evidence: None,
        };
        store.store(event).await.unwrap();
    }

    // 2 events with a different actor
    for _ in 0..2 {
        let event = SecurityEvent {
            id: cyberclaw_core::ids::SecurityEventId::new(),
            actor: Some(other_actor.clone()),
            timestamp: chrono::Utc::now(),
            execution_id: None,
            case_id: None,
            node_id: None,
            runtime_instance_id: None,
            source: SecurityEventSource::RuntimeDetection,
            event_type: SecurityEventType::PolicyDenied,
            severity: Severity::Medium,
            summary: "other actor event".to_string(),
            details: serde_json::json!({}),
            trace_id: TraceId::new(),
            credential_evidence: None,
        };
        store.store(event).await.unwrap();
    }

    // 1 event with no actor info
    let event = SecurityEvent {
        id: cyberclaw_core::ids::SecurityEventId::new(),
        actor: None,
        timestamp: chrono::Utc::now(),
        execution_id: None,
        case_id: None,
        node_id: None,
        runtime_instance_id: None,
        source: SecurityEventSource::PolicyEngine,
        event_type: SecurityEventType::RuntimeAnomalyDetected,
        severity: Severity::Low,
        summary: "no actor event".to_string(),
        details: serde_json::json!({}),
        trace_id: TraceId::new(),
        credential_evidence: None,
    };
    store.store(event).await.unwrap();

    // Filter by actor: should return exactly 3 matching events
    let filter = EventFilter {
        actor: Some(actor.clone()),
        ..Default::default()
    };
    let results = store.query(filter).await.unwrap();
    assert_eq!(
        results.len(),
        3,
        "actor filter should return exactly 3 matching events, got {}",
        results.len()
    );

    // All returned events must have the matching actor id
    for ev in &results {
        assert_eq!(
            ev.actor.as_ref().map(|a| &a.id),
            Some(&actor_id),
            "returned event must have the target actor_id"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 5: time_range filter
// ---------------------------------------------------------------------------

/// Tests that time_range filter correctly filters events based on event.timestamp.
/// Events with timestamp within [start, end] are returned; others are excluded.
#[tokio::test]
async fn test_time_range_filter() {
    use chrono::{Duration, Utc};

    let store = InMemorySecurityEventStore::new();

    // Store 5 events with current timestamp (within range)
    for i in 0..5_u32 {
        let exec_id = ExecutionId::from_string(format!("time-range-exec-{:02}", i)).unwrap();
        let event = SecurityEvent {
            id: cyberclaw_core::ids::SecurityEventId::new(),
            actor: None,
            timestamp: chrono::Utc::now(),
            execution_id: Some(exec_id),
            case_id: None,
            node_id: None,
            runtime_instance_id: None,
            source: SecurityEventSource::RuntimeDetection,
            event_type: SecurityEventType::PermissionViolation,
            severity: Severity::Medium,
            summary: "time range test event".to_string(),
            details: serde_json::json!({}),
            trace_id: TraceId::new(),
            credential_evidence: None,
        };
        store.store(event).await.unwrap();
    }

    let now = Utc::now();
    let start = now - Duration::hours(1);
    let end = now + Duration::hours(1);

    // All 5 events have timestamp near now, so they all fall in [now-1h, now+1h]
    let filter = EventFilter {
        time_range: Some((start, end)),
        ..Default::default()
    };
    let results = store.query(filter).await.unwrap();
    assert_eq!(
        results.len(),
        5,
        "time_range filter should pass all 5 events with current timestamps, got {}",
        results.len()
    );
}

// ---------------------------------------------------------------------------
// Test 6: combined filters (execution_id + actor + time_range)
// ---------------------------------------------------------------------------

/// Tests that multiple filters combined with AND logic work correctly.
/// Uses execution_id + actor + time_range together to narrow results.
#[tokio::test]
async fn test_combined_filters() {
    use chrono::{Duration, Utc};

    let store = InMemorySecurityEventStore::new();

    let target_exec_id = ExecutionId::from_string("combined-filter-exec-001".to_string()).unwrap();
    let other_exec_id = ExecutionId::from_string("combined-filter-exec-002".to_string()).unwrap();
    let target_actor_id = ActorId::from_string("combined-actor-target".to_string()).unwrap();
    let other_actor_id = ActorId::from_string("combined-actor-other".to_string()).unwrap();

    let target_actor = ActorRef {
        id: target_actor_id.clone(),
        actor_type: ActorType::Agent,
        tenant_id: None,
        home_node_id: None,
        display_name: "CombinedTestAgent".to_string(),
    };
    let other_actor = ActorRef {
        id: other_actor_id.clone(),
        actor_type: ActorType::Agent,
        tenant_id: None,
        home_node_id: None,
        display_name: "OtherCombinedAgent".to_string(),
    };

    // Event 1: target exec + target actor -> should match
    store
        .store(SecurityEvent {
            id: cyberclaw_core::ids::SecurityEventId::new(),
            actor: Some(target_actor.clone()),
            timestamp: chrono::Utc::now(),
            execution_id: Some(target_exec_id.clone()),
            case_id: None,
            node_id: None,
            runtime_instance_id: None,
            source: SecurityEventSource::RuntimeDetection,
            event_type: SecurityEventType::PermissionViolation,
            severity: Severity::High,
            summary: "combined match event".to_string(),
            details: serde_json::json!({}),
            trace_id: TraceId::new(),
            credential_evidence: None,
        })
        .await
        .unwrap();

    // Event 2: target exec + other actor -> should NOT match (actor mismatch)
    store
        .store(SecurityEvent {
            id: cyberclaw_core::ids::SecurityEventId::new(),
            actor: Some(other_actor.clone()),
            timestamp: chrono::Utc::now(),
            execution_id: Some(target_exec_id.clone()),
            case_id: None,
            node_id: None,
            runtime_instance_id: None,
            source: SecurityEventSource::PolicyEngine,
            event_type: SecurityEventType::PolicyDenied,
            severity: Severity::Medium,
            summary: "combined actor mismatch event".to_string(),
            details: serde_json::json!({}),
            trace_id: TraceId::new(),
            credential_evidence: None,
        })
        .await
        .unwrap();

    // Event 3: other exec + target actor -> should NOT match (exec_id mismatch)
    store
        .store(SecurityEvent {
            id: cyberclaw_core::ids::SecurityEventId::new(),
            actor: Some(target_actor.clone()),
            timestamp: chrono::Utc::now(),
            execution_id: Some(other_exec_id.clone()),
            case_id: None,
            node_id: None,
            runtime_instance_id: None,
            source: SecurityEventSource::RuntimeDetection,
            event_type: SecurityEventType::RuntimeAnomalyDetected,
            severity: Severity::Low,
            summary: "combined exec mismatch event".to_string(),
            details: serde_json::json!({}),
            trace_id: TraceId::new(),
            credential_evidence: None,
        })
        .await
        .unwrap();

    let now = Utc::now();
    let start = now - Duration::hours(1);
    let end = now + Duration::hours(1);

    // Combined filter: execution_id + actor + time_range
    let filter = EventFilter {
        execution_id: Some(target_exec_id.clone()),
        actor: Some(target_actor),
        time_range: Some((start, end)),
        ..Default::default()
    };
    let results = store.query(filter).await.unwrap();

    // Only event 1 satisfies all three conditions
    assert_eq!(
        results.len(),
        1,
        "combined filter should return exactly 1 matching event, got {}",
        results.len()
    );
    assert_eq!(
        results[0].execution_id.as_ref().unwrap(),
        &target_exec_id,
        "returned event must have target execution_id"
    );
    assert_eq!(
        results[0].actor.as_ref().map(|a| &a.id),
        Some(&target_actor_id),
        "returned event must have target actor_id"
    );
}
