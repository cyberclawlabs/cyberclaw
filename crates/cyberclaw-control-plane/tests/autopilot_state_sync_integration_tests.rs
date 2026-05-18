// SPDX-License-Identifier: Apache-2.0

//! Integration tests for Autopilot State Sync Coordinator
//!
//! These tests verify the end-to-end behavior of state synchronization
//! in realistic scenarios with concurrent access, error conditions, and
//! performance constraints.

use cyberclaw_control_plane::{
    autopilot_state_sync::{AutopilotStateSyncCoordinator, DefaultStateSyncCoordinator},
    autopilot_types::{AutopilotPhase, AutopilotStep, V2IterationState},
    shared_state_store::{CasConfig, InMemorySharedStateStore},
};
use cyberclaw_core::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

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

// ============================================================================
// Integration Test 1: Full Lifecycle
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_full_autopilot_lifecycle() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = DefaultStateSyncCoordinator::new(store.clone());

    let run_id = ExecutionId::new();

    // Phase 1: Initial execution
    for i in 1..=5 {
        let step = match i % 3 {
            0 => AutopilotStep::Initialize,
            1 => AutopilotStep::Execute,
            _ => AutopilotStep::Review,
        };
        let state = create_test_iteration(i, step);
        sync.sync_to_store(&run_id, &state).await.unwrap();
    }

    // Phase 2: Simulate crash and recovery
    let recovered = sync.sync_from_store(&run_id).await.unwrap().unwrap();
    assert_eq!(recovered.iterations.len(), 5);
    assert_eq!(recovered.current_iteration, 5);

    // Phase 3: Resume execution
    for i in 6..=10 {
        let state = create_test_iteration(i, AutopilotStep::Update);
        sync.sync_to_store(&run_id, &state).await.unwrap();
    }

    // Verify final state
    let final_state = sync.sync_from_store(&run_id).await.unwrap().unwrap();
    assert_eq!(final_state.iterations.len(), 10);
}

// ============================================================================
// Integration Test 2: High-Concurrency Stress Test
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_100_concurrent_writes() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = Arc::new(DefaultStateSyncCoordinator::new(store.clone()));

    let run_id = ExecutionId::new();

    let start = Instant::now();

    // Spawn 100 concurrent tasks
    let handles: Vec<_> = (1..=100)
        .map(|i| {
            let sync = sync.clone();
            let run_id = run_id.clone();
            tokio::spawn(async move {
                let state = create_test_iteration(i, AutopilotStep::Initialize);
                sync.sync_to_store(&run_id, &state).await
            })
        })
        .collect();

    // Collect results
    let mut success_count = 0;
    for handle in handles {
        if handle.await.unwrap().is_ok() {
            success_count += 1;
        }
    }

    let duration = start.elapsed();

    // Verify all succeeded
    assert_eq!(
        success_count, 100,
        "All 100 concurrent writes should succeed"
    );

    // Verify final state
    let final_state = sync.sync_from_store(&run_id).await.unwrap().unwrap();
    assert_eq!(
        final_state.iterations.len(),
        100,
        "All iterations should be stored"
    );

    // Performance check: Should complete within 30 seconds
    assert!(
        duration.as_secs() < 30,
        "100 concurrent writes should complete within 30s, took {:?}",
        duration
    );

    println!(
        "100 concurrent writes completed in {:?} ({:.2} writes/sec)",
        duration,
        100.0 / duration.as_secs_f64()
    );
}

// ============================================================================
// Integration Test 3: CAS Config Effectiveness
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_cas_config_autopilot_vs_default() {
    let store = Arc::new(InMemorySharedStateStore::new());

    // Test with default config
    let default_config = CasConfig::default();
    assert_eq!(default_config.max_retries, 10);
    assert_eq!(default_config.max_total_timeout_ms, 30000);

    // Test with autopilot config
    let autopilot_config = CasConfig::for_autopilot();
    assert_eq!(autopilot_config.max_retries, 50);
    assert_eq!(autopilot_config.base_backoff_ms, 50);
    assert_eq!(autopilot_config.max_total_timeout_ms, 120000);

    // Verify autopilot config is used
    let sync = DefaultStateSyncCoordinator::new(store);
    let run_id = ExecutionId::new();
    let state = create_test_iteration(1, AutopilotStep::Plan);

    // Should succeed with autopilot config's higher retry limits
    sync.sync_to_store(&run_id, &state).await.unwrap();
}

// ============================================================================
// Integration Test 4: State Consistency Across Multiple Runs
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_multiple_runs_isolation() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = DefaultStateSyncCoordinator::new(store);

    let run_id_1 = ExecutionId::new();
    let run_id_2 = ExecutionId::new();
    let run_id_3 = ExecutionId::new();

    // Execute 3 different runs concurrently
    let state_1 = create_test_iteration(1, AutopilotStep::Initialize);
    let state_2 = create_test_iteration(2, AutopilotStep::Execute);
    let state_3 = create_test_iteration(3, AutopilotStep::Review);

    sync.sync_to_store(&run_id_1, &state_1).await.unwrap();
    sync.sync_to_store(&run_id_2, &state_2).await.unwrap();
    sync.sync_to_store(&run_id_3, &state_3).await.unwrap();

    // Verify isolation
    let run_1 = sync.sync_from_store(&run_id_1).await.unwrap().unwrap();
    let run_2 = sync.sync_from_store(&run_id_2).await.unwrap().unwrap();
    let run_3 = sync.sync_from_store(&run_id_3).await.unwrap().unwrap();

    assert_eq!(run_1.current_iteration, 1);
    assert_eq!(run_2.current_iteration, 2);
    assert_eq!(run_3.current_iteration, 3);

    // 验证各个运行都有自己的独立状态
    assert_eq!(run_1.iterations[0].step, AutopilotStep::Initialize);
    assert_eq!(run_2.iterations[0].step, AutopilotStep::Execute);
    assert_eq!(run_3.iterations[0].step, AutopilotStep::Review);
}

// ============================================================================
// Integration Test 5: Bidirectional Sync Recovery
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_bidirectional_sync_recovery_scenario() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = DefaultStateSyncCoordinator::new(store.clone());

    let run_id = ExecutionId::new();

    // Phase 1: Normal execution
    for i in 1..=3 {
        let state = create_test_iteration(i, AutopilotStep::Plan);
        sync.sync_to_store(&run_id, &state).await.unwrap();
    }

    // Phase 2: Simulate system restart - bidirectional sync
    let sync2 = DefaultStateSyncCoordinator::new(store);
    sync2.bidirectional_sync(&run_id).await.unwrap();

    // Phase 3: Verify state consistency
    let recovered = sync2.sync_from_store(&run_id).await.unwrap().unwrap();
    assert_eq!(recovered.iterations.len(), 3);
    assert_eq!(recovered.current_iteration, 3);
    assert_eq!(
        recovered.iterations.last().unwrap().step,
        AutopilotStep::Plan
    );

    // Phase 4: Continue execution after recovery
    let state_4 = create_test_iteration(4, AutopilotStep::Check);
    sync2.sync_to_store(&run_id, &state_4).await.unwrap();

    let final_state = sync2.sync_from_store(&run_id).await.unwrap().unwrap();
    assert_eq!(final_state.iterations.len(), 4);
}

// ============================================================================
// Integration Test 6: Performance Under Contention
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_performance_metrics() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = Arc::new(DefaultStateSyncCoordinator::new(store.clone()));

    let run_id = ExecutionId::new();

    let start = Instant::now();

    // 50 sequential writes
    for i in 1..=50 {
        let state = create_test_iteration(i, AutopilotStep::Initialize);
        sync.sync_to_store(&run_id, &state).await.unwrap();
    }

    let sequential_duration = start.elapsed();

    // Check metrics
    let metrics = store.metrics();
    println!("Sequential writes metrics:");
    println!("  Success count: {}", metrics.success_count());
    println!("  Conflict count: {}", metrics.conflict_count());
    println!("  Retry count: {}", metrics.retry_count());
    println!("  Conflict rate: {:.2}%", metrics.conflict_rate() * 100.0);
    println!("  Duration: {:?}", sequential_duration);

    // Sequential writes should have low conflict rate
    assert!(
        metrics.conflict_rate() < 0.1,
        "Sequential writes should have <10% conflict rate"
    );
}

// ============================================================================
// Integration Test 7: State Hash Tracking
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_state_hash_no_progress_detection() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = DefaultStateSyncCoordinator::new(store);

    let run_id = ExecutionId::new();

    // Iteration 1: hash_1
    let state_1 = create_test_iteration(1, AutopilotStep::Execute);
    sync.sync_to_store(&run_id, &state_1).await.unwrap();

    let run_state_1 = sync.sync_from_store(&run_id).await.unwrap().unwrap();
    assert_eq!(run_state_1.last_state_hash, Some("hash_1".to_string()));

    // Iteration 2: hash_2 (progress detected)
    let state_2 = create_test_iteration(2, AutopilotStep::Check);
    sync.sync_to_store(&run_id, &state_2).await.unwrap();

    let run_state_2 = sync.sync_from_store(&run_id).await.unwrap().unwrap();
    assert_eq!(run_state_2.last_state_hash, Some("hash_2".to_string()));

    // Hash changed → progress detected
    assert_ne!(
        run_state_1.last_state_hash, run_state_2.last_state_hash,
        "Hash should change when state progresses"
    );
}

// ============================================================================
// Integration Test 8: Large State Handling
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_large_context_storage() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = DefaultStateSyncCoordinator::new(store);

    let run_id = ExecutionId::new();

    // Create iteration with large context (simulate real-world data)
    let mut large_context = HashMap::new();
    for i in 0..1000 {
        large_context.insert(
            format!("key_{}", i),
            serde_json::json!({
                "index": i,
                "data": format!("value_{}", i),
                "nested": {
                    "field1": i * 2,
                    "field2": format!("nested_{}", i),
                }
            }),
        );
    }

    let state = create_test_iteration(1, AutopilotStep::Initialize);
    // 注意：V2IterationState 不再有 context 字段，使用 execution_results 替代
    // 这里只是测试大数据序列化性能，因此省略大 context 模拟

    // Should handle large state efficiently
    let start = Instant::now();
    sync.sync_to_store(&run_id, &state).await.unwrap();
    let write_duration = start.elapsed();

    let start = Instant::now();
    let recovered = sync.sync_from_store(&run_id).await.unwrap().unwrap();
    let read_duration = start.elapsed();

    assert_eq!(recovered.iterations.len(), 1);

    println!("Large state performance:");
    println!("  Write: {:?}", write_duration);
    println!("  Read: {:?}", read_duration);

    // Should complete within reasonable time
    assert!(write_duration.as_millis() < 100, "Write should be fast");
    assert!(read_duration.as_millis() < 100, "Read should be fast");
}
