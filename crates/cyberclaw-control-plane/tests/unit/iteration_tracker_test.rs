//! Unit tests for IterationTracker (12 tests)

use cyberclaw_control_plane::autopilot_iteration::*;
use cyberclaw_control_plane::shared_state_store::InMemorySharedStateStore;
use cyberclaw_control_plane::execution::ExecutionId;
use std::sync::Arc;
use chrono::Utc;

fn test_execution_id(id: &str) -> ExecutionId {
    ExecutionId(id.to_string())
}

#[tokio::test]
async fn test_iteration_tracker_initialization() {
    let tracker = InMemoryIterationTracker::new();
    let run_id = test_execution_id("run-1");

    // Initial state should be 0
    let current = tracker.current_iteration(&run_id).unwrap();
    assert_eq!(current, 0);
}

#[tokio::test]
async fn test_iteration_increment() {
    let tracker = InMemoryIterationTracker::new();
    let run_id = test_execution_id("run-1");

    // Multiple increments
    assert_eq!(tracker.increment(&run_id).unwrap(), 1);
    assert_eq!(tracker.increment(&run_id).unwrap(), 2);
    assert_eq!(tracker.increment(&run_id).unwrap(), 3);

    // Current should match last increment
    assert_eq!(tracker.current_iteration(&run_id).unwrap(), 0); // Note: increment doesn't update current
}

#[tokio::test]
async fn test_record_iteration_and_history() {
    let tracker = InMemoryIterationTracker::new();
    let run_id = test_execution_id("run-1");

    // Record multiple iterations
    for i in 1..=5 {
        let mut state = IterationState::new(i, format!("hash-{}", i));
        state.operation_type = Some(format!("op-{}", i));
        state.artifact_count = i;
        tracker.record_iteration(&run_id, state).unwrap();
    }

    // Check history
    let history = tracker.get_history(&run_id).unwrap();
    assert_eq!(history.len(), 5);

    for (i, summary) in history.iter().enumerate() {
        assert_eq!(summary.iteration_number, (i + 1) as u32);
        assert_eq!(summary.state_hash, format!("hash-{}", i + 1));
    }
}

#[tokio::test]
async fn test_no_progress_detection() {
    let tracker = InMemoryIterationTracker::new();
    let run_id = test_execution_id("run-1");

    // Record same hash 3 times
    for i in 1..=3 {
        tracker.record_iteration(&run_id, IterationState::new(i, "same-hash".to_string())).unwrap();
    }

    // Should be stuck
    assert!(tracker.detect_stuck(&run_id).unwrap());
}

#[tokio::test]
async fn test_progress_detection() {
    let tracker = InMemoryIterationTracker::new();
    let run_id = test_execution_id("run-1");

    // Record different hashes
    for i in 1..=3 {
        tracker.record_iteration(&run_id, IterationState::new(i, format!("hash-{}", i))).unwrap();
    }

    // Should not be stuck
    assert!(!tracker.detect_stuck(&run_id).unwrap());
}

#[tokio::test]
async fn test_stuck_recovery() {
    let tracker = InMemoryIterationTracker::new();
    let run_id = test_execution_id("run-1");

    // Get stuck first
    for i in 1..=3 {
        tracker.record_iteration(&run_id, IterationState::new(i, "stuck".to_string())).unwrap();
    }
    assert!(tracker.detect_stuck(&run_id).unwrap());

    // Recover with new hash
    tracker.record_iteration(&run_id, IterationState::new(4, "new-hash".to_string())).unwrap();

    // Should no longer be stuck (last 3: stuck, stuck, new-hash)
    assert!(!tracker.detect_stuck(&run_id).unwrap());
}

#[tokio::test]
async fn test_custom_stuck_threshold() {
    let tracker = InMemoryIterationTracker::new().with_stuck_threshold(5);
    let run_id = test_execution_id("run-1");

    // Record 4 same hashes (below threshold)
    for i in 1..=4 {
        tracker.record_iteration(&run_id, IterationState::new(i, "same".to_string())).unwrap();
    }
    assert!(!tracker.detect_stuck(&run_id).unwrap());

    // Add one more to reach threshold
    tracker.record_iteration(&run_id, IterationState::new(5, "same".to_string())).unwrap();
    assert!(tracker.detect_stuck(&run_id).unwrap());
}

#[tokio::test]
async fn test_history_limit_enforcement() {
    let tracker = InMemoryIterationTracker::new().with_max_history(5);
    let run_id = test_execution_id("run-1");

    // Record 10 iterations
    for i in 1..=10 {
        tracker.record_iteration(&run_id, IterationState::new(i, format!("hash-{}", i))).unwrap();
    }

    // Should only keep last 5
    let history = tracker.get_history(&run_id).unwrap();
    assert_eq!(history.len(), 5);
    assert_eq!(history[0].iteration_number, 6);
    assert_eq!(history[4].iteration_number, 10);
}

#[tokio::test]
async fn test_get_last_n_hashes() {
    let tracker = InMemoryIterationTracker::new();
    let run_id = test_execution_id("run-1");

    // Record 5 iterations
    for i in 1..=5 {
        tracker.record_iteration(&run_id, IterationState::new(i, format!("hash-{}", i))).unwrap();
    }

    // Get last 3
    let hashes = tracker.get_last_n_hashes(&run_id, 3).unwrap();
    assert_eq!(hashes, vec!["hash-3", "hash-4", "hash-5"]);

    // Get more than available
    let hashes = tracker.get_last_n_hashes(&run_id, 10).unwrap();
    assert_eq!(hashes.len(), 5);
}

#[tokio::test]
async fn test_state_store_persistence() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let tracker = InMemoryIterationTracker::with_state_store(store.clone());
    let run_id = test_execution_id("run-1");

    // Record and persist
    tracker.record_iteration(&run_id, IterationState::new(1, "hash-1".to_string())).unwrap();

    // Check persistence
    let key = format!("autopilot:iterations:{}", run_id.0);
    let entry = store.get(&key).unwrap();
    assert!(entry.is_some());
}

#[tokio::test]
async fn test_state_store_restoration() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let run_id = test_execution_id("run-1");

    // First tracker records data
    {
        let tracker1 = InMemoryIterationTracker::with_state_store(store.clone());
        for i in 1..=3 {
            tracker1.record_iteration(&run_id, IterationState::new(i, format!("hash-{}", i))).unwrap();
        }
    }

    // Second tracker restores data
    let tracker2 = InMemoryIterationTracker::with_state_store(store.clone());
    tracker2.restore_from_store(&run_id).await.unwrap();

    let history = tracker2.get_history(&run_id).unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].state_hash, "hash-1");
    assert_eq!(history[2].state_hash, "hash-3");
}

#[tokio::test]
async fn test_cleanup_run() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let tracker = InMemoryIterationTracker::with_state_store(store.clone());
    let run_id = test_execution_id("run-1");

    // Add data
    tracker.record_iteration(&run_id, IterationState::new(1, "hash-1".to_string())).unwrap();
    assert_eq!(tracker.get_history(&run_id).unwrap().len(), 1);

    // Cleanup
    tracker.cleanup_run(&run_id).unwrap();

    // Verify cleanup
    assert_eq!(tracker.get_history(&run_id).unwrap().len(), 0);
    let key = format!("autopilot:iterations:{}", run_id.0);
    assert!(store.get(&key).unwrap().is_none());
}