// SPDX-License-Identifier: Apache-2.0

//! Standalone tests for autopilot_state_sync module
//!
//! This test file is designed to run independently without requiring
//! the full cyberclaw-control-plane crate to compile.

use cyberclaw_control_plane::{
    autopilot_state_sync::{AutopilotStateSyncCoordinator, DefaultStateSyncCoordinator},
    autopilot_types::{AutopilotPhase, AutopilotStep, V2IterationState},
    shared_state_store::InMemorySharedStateStore,
};
use cyberclaw_core::prelude::*;
use std::sync::Arc;

fn create_test_iteration(iteration_id: u32, step: AutopilotStep) -> V2IterationState {
    V2IterationState {
        iteration_id,
        step,
        start_time: chrono::Utc::now(),
        end_time: None,
        state_hash: format!("hash_{}", iteration_id),
        progress_delta: None,
        execution_results: Vec::new(),
        current_phase: AutopilotPhase::Plan,
        fix_loop_count: 0,
        plan_mode_snapshot: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_basic_sync_workflow() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = DefaultStateSyncCoordinator::new(store.clone());

    let run_id = ExecutionId::new();

    // 1. Sync first iteration
    let state1 = create_test_iteration(1, AutopilotStep::Initialize);
    sync.sync_to_store(&run_id, &state1).await.unwrap();

    // 2. Retrieve state
    let retrieved = sync.sync_from_store(&run_id).await.unwrap();
    assert!(retrieved.is_some());

    let run_state = retrieved.unwrap();
    assert_eq!(run_state.run_id, run_id);
    assert_eq!(run_state.iterations.len(), 1);
    assert_eq!(run_state.current_iteration, 1);

    println!("✅ Basic sync workflow test passed");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_10_concurrent_writes() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = Arc::new(DefaultStateSyncCoordinator::new(store.clone()));

    let run_id = ExecutionId::new();

    let handles: Vec<_> = (1..=10)
        .map(|i| {
            let sync = sync.clone();
            let run_id = run_id.clone();
            tokio::spawn(async move {
                let state = create_test_iteration(i, AutopilotStep::Initialize);
                sync.sync_to_store(&run_id, &state).await
            })
        })
        .collect();

    let mut success_count = 0;
    for handle in handles {
        if handle.await.unwrap().is_ok() {
            success_count += 1;
        }
    }

    assert_eq!(success_count, 10, "All 10 concurrent writes should succeed");

    let final_state = sync.sync_from_store(&run_id).await.unwrap().unwrap();
    assert_eq!(
        final_state.iterations.len(),
        10,
        "All iterations should be stored"
    );

    println!("✅ 10 concurrent writes test passed");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_100_concurrent_writes_stress() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = Arc::new(DefaultStateSyncCoordinator::new(store.clone()));

    let run_id = ExecutionId::new();

    let start = std::time::Instant::now();

    let handles: Vec<_> = (1..=100)
        .map(|i| {
            let sync = sync.clone();
            let run_id = run_id.clone();
            tokio::spawn(async move {
                let state = create_test_iteration(i, AutopilotStep::Execute);
                sync.sync_to_store(&run_id, &state).await
            })
        })
        .collect();

    let mut success_count = 0;
    for handle in handles {
        if handle.await.unwrap().is_ok() {
            success_count += 1;
        }
    }

    let duration = start.elapsed();

    assert_eq!(
        success_count, 100,
        "All 100 concurrent writes should succeed"
    );

    let final_state = sync.sync_from_store(&run_id).await.unwrap().unwrap();
    assert_eq!(
        final_state.iterations.len(),
        100,
        "All iterations should be stored"
    );

    println!(
        "✅ 100 concurrent writes completed in {:?} ({:.2} writes/sec)",
        duration,
        100.0 / duration.as_secs_f64()
    );

    // Should complete within 30 seconds
    assert!(
        duration.as_secs() < 30,
        "Should complete within 30s, took {:?}",
        duration
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_metrics_validation() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = Arc::new(DefaultStateSyncCoordinator::new(store.clone()));

    let run_id = ExecutionId::new();

    // Perform 20 sequential writes
    for i in 1..=20 {
        let state = create_test_iteration(i, AutopilotStep::Plan);
        sync.sync_to_store(&run_id, &state).await.unwrap();
    }

    let metrics = store.metrics();
    println!("Metrics after 20 sequential writes:");
    println!("  Success count: {}", metrics.success_count());
    println!("  Conflict count: {}", metrics.conflict_count());
    println!("  Retry count: {}", metrics.retry_count());
    println!("  Conflict rate: {:.2}%", metrics.conflict_rate() * 100.0);

    assert_eq!(metrics.success_count(), 20);
    // Sequential writes should have low conflict rate
    assert!(
        metrics.conflict_rate() < 0.1,
        "Sequential writes should have <10% conflict rate"
    );

    println!("✅ Metrics validation test passed");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_recovery_scenario() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let sync1 = DefaultStateSyncCoordinator::new(store.clone());

    let run_id = ExecutionId::new();

    // Phase 1: Normal execution
    for i in 1..=3 {
        let state = create_test_iteration(i, AutopilotStep::Execute);
        sync1.sync_to_store(&run_id, &state).await.unwrap();
    }

    // Phase 2: Simulate restart with new coordinator
    let sync2 = DefaultStateSyncCoordinator::new(store.clone());
    sync2.bidirectional_sync(&run_id).await.unwrap();

    // Phase 3: Verify state consistency
    let recovered = sync2.sync_from_store(&run_id).await.unwrap().unwrap();
    assert_eq!(recovered.iterations.len(), 3);
    assert_eq!(recovered.current_iteration, 3);

    // Phase 4: Continue execution
    let state4 = create_test_iteration(4, AutopilotStep::Check);
    sync2.sync_to_store(&run_id, &state4).await.unwrap();

    let final_state = sync2.sync_from_store(&run_id).await.unwrap().unwrap();
    assert_eq!(final_state.iterations.len(), 4);

    println!("✅ Recovery scenario test passed");
}
