// Agent 6: SharedStateStore Autopilot Integration Tests
//
// Tests for CasConfig, Autopilot-specific methods, and integration scenarios

use cyberclaw_control_plane::shared_state_store::{
    CasConfig, InMemorySharedStateStore, SharedStateStore,
};
use std::sync::Arc;
use std::time::Duration;

// ============================================================================
// Unit Tests: CasConfig
// ============================================================================

#[test]
fn test_cas_config_for_autopilot() {
    let config = CasConfig::for_autopilot();
    assert_eq!(config.max_retries, 50, "Autopilot should use 50 retries");
    assert_eq!(
        config.base_backoff_ms, 50,
        "Autopilot should use 50ms base backoff"
    );
    assert_eq!(
        config.max_total_timeout_ms, 120000,
        "Autopilot should use 120s timeout"
    );
}

#[test]
fn test_cas_config_default() {
    let config = CasConfig::default_config();
    assert_eq!(config.max_retries, 10, "Default should use 10 retries");
    assert_eq!(
        config.base_backoff_ms, 10,
        "Default should use 10ms base backoff"
    );
    assert_eq!(
        config.max_total_timeout_ms, 30000,
        "Default should use 30s timeout"
    );
}

#[test]
fn test_cas_config_default_trait() {
    let config = CasConfig::default();
    assert_eq!(config.max_retries, CasConfig::default_config().max_retries);
    assert_eq!(
        config.base_backoff_ms,
        CasConfig::default_config().base_backoff_ms
    );
    assert_eq!(
        config.max_total_timeout_ms,
        CasConfig::default_config().max_total_timeout_ms
    );
}

// ============================================================================
// Unit Tests: cas_with_config
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_cas_with_config_basic() {
    let store = InMemorySharedStateStore::new();
    let config = CasConfig::default_config();

    // Initial insert
    let result = store
        .cas_with_config("key1".to_string(), 0, b"value1".to_vec(), &config)
        .await;
    assert!(result.is_ok(), "First CAS should succeed");

    // Verify
    let entry = store.get("key1").await.unwrap().unwrap();
    assert_eq!(entry.value, b"value1");
    assert_eq!(entry.version, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_cas_with_config_version_conflict() {
    let store = InMemorySharedStateStore::new();
    let config = CasConfig::default_config();

    // Insert initial value
    store
        .cas_with_config("key2".to_string(), 0, b"v1".to_vec(), &config)
        .await
        .unwrap();

    // Update with correct version
    store
        .cas_with_config("key2".to_string(), 1, b"v2".to_vec(), &config)
        .await
        .unwrap();

    // Try to update with stale version (should fail)
    let result = store
        .cas_with_config("key2".to_string(), 1, b"v3".to_vec(), &config)
        .await;
    assert!(
        result.is_err(),
        "CAS with stale version should fail after retries"
    );
}

// Test split into two deterministic tests to eliminate TOCTOU race condition flakiness.
//
// Original test was flaky because:
// 1. TOCTOU: each task did GET (read version) then CAS (write) as separate non-atomic steps.
//    If tasks ran sequentially (e.g. under low concurrency scheduler), every task would read
//    the latest version and succeed => failures == 0 => assertion panic.
// 2. The assert!(failures > 0) assumed concurrent conflict, but sequential scheduling made
//    all 10 tasks succeed without any conflict.
//
// Fix: split into two tests with deterministic conflict conditions:
//   - test_cas_with_config_stale_version_always_fails: use a known stale version => always fails
//   - test_cas_with_config_concurrent_consistency: verify exactly one winner from concurrent
//     tasks that all use the SAME stale version (forcing deterministic conflict).

#[tokio::test(flavor = "multi_thread")]
async fn test_cas_with_config_stale_version_always_fails() {
    // Verify: CAS with a known-stale expected_version fails deterministically (no race condition).
    let store = InMemorySharedStateStore::new();
    let config = CasConfig {
        max_retries: 3,
        base_backoff_ms: 1,
        max_total_timeout_ms: 500,
    };

    // Insert initial value -> version becomes 1
    store
        .cas_with_config("key3a".to_string(), 0, b"v0".to_vec(), &config)
        .await
        .unwrap();

    // Update to version 2
    store
        .cas_with_config("key3a".to_string(), 1, b"v1".to_vec(), &config)
        .await
        .unwrap();

    // CAS with stale version 1 (current is 2): must always fail
    let result = store
        .cas_with_config("key3a".to_string(), 1, b"stale".to_vec(), &config)
        .await;
    assert!(
        result.is_err(),
        "CAS with stale version must always fail; got Ok instead"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_cas_with_config_concurrent_consistency() {
    // Verify concurrent correctness: when N tasks all attempt CAS with the SAME stale version,
    // exactly one wins and the others fail. This is deterministic regardless of scheduling order.
    let store = Arc::new(InMemorySharedStateStore::new());
    let config = CasConfig {
        max_retries: 1, // Low retries: stuck-state will be detected quickly
        base_backoff_ms: 1,
        max_total_timeout_ms: 500,
    };

    // Insert initial value -> version becomes 1
    store
        .cas_with_config("key3b".to_string(), 0, b"v0".to_vec(), &config)
        .await
        .unwrap();

    // Capture the CURRENT version BEFORE spawning tasks.
    // All tasks will use this same stale_version, guaranteeing:
    //   - exactly 1 task succeeds (the first to acquire the lock)
    //   - all remaining 9 tasks fail with version mismatch
    // This eliminates TOCTOU: no per-task GET needed.
    let stale_version = store
        .get("key3b")
        .await
        .unwrap()
        .expect("key3b must exist")
        .version;

    let handles: Vec<_> = (1..=10u32)
        .map(|i| {
            let store = store.clone();
            let config = config.clone();
            tokio::spawn(async move {
                let value = format!("v{}", i).into_bytes();
                // All tasks use the SAME stale_version -- only one can win
                store
                    .cas_with_config("key3b".to_string(), stale_version, value, &config)
                    .await
            })
        })
        .collect();

    let mut successes = 0u32;
    let mut failures = 0u32;
    for handle in handles {
        match handle.await.unwrap() {
            Ok(_) => successes += 1,
            Err(_) => failures += 1,
        }
    }

    // Deterministic: exactly 1 succeeds, 9 fail (version mismatch after first write)
    assert_eq!(
        successes, 1,
        "Exactly one CAS should succeed when all use the same stale version; got {} successes",
        successes
    );
    assert_eq!(
        failures, 9,
        "All other 9 CAS operations should fail with version mismatch; got {} failures",
        failures
    );

    // Final state: version must be stale_version + 1
    let final_entry = store.get("key3b").await.unwrap().unwrap();
    assert_eq!(
        final_entry.version,
        stale_version + 1,
        "Final version must be stale_version+1 after exactly one successful write"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_cas_with_config_timeout_control() {
    let store = InMemorySharedStateStore::new();

    // Config with very short timeout
    let short_timeout_config = CasConfig {
        max_retries: 100, // High retries, but short timeout
        base_backoff_ms: 100,
        max_total_timeout_ms: 50, // Only 50ms total
    };

    // Insert initial value
    store
        .cas_with_config("key4".to_string(), 0, b"v1".to_vec(), &short_timeout_config)
        .await
        .unwrap();

    // Force a conflict by using wrong version - should timeout quickly
    let start = std::time::Instant::now();
    let result = store
        .cas_with_config(
            "key4".to_string(),
            0, // Wrong version
            b"v2".to_vec(),
            &short_timeout_config,
        )
        .await;
    let elapsed = start.elapsed();

    assert!(result.is_err(), "Should fail due to wrong version");
    assert!(
        elapsed.as_millis() < 1000,
        "Should timeout within 1s (actual: {}ms)",
        elapsed.as_millis()
    );
}

// ============================================================================
// Unit Tests: get_state_history
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_get_state_history_empty() {
    let store = InMemorySharedStateStore::new();
    let history = store.get_state_history("nonexistent", 10).await.unwrap();
    assert!(
        history.is_empty(),
        "History for nonexistent key should be empty"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_state_history_tracking() {
    let store = InMemorySharedStateStore::new();

    // Insert and update multiple times
    store
        .put("key5".to_string(), b"v1".to_vec(), 1)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;

    store
        .put("key5".to_string(), b"v2".to_vec(), 2)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;

    store
        .put("key5".to_string(), b"v3".to_vec(), 3)
        .await
        .unwrap();

    // Retrieve history
    let history = store.get_state_history("key5", 10).await.unwrap();
    assert_eq!(history.len(), 3, "Should track all 3 updates");

    // Check ordering (should be in insertion order)
    assert_eq!(history[0].version, 1);
    assert_eq!(history[1].version, 2);
    assert_eq!(history[2].version, 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_state_history_limit() {
    let store = InMemorySharedStateStore::new();

    // Insert 5 updates
    for i in 1..=5 {
        store
            .put("key6".to_string(), format!("v{}", i).into_bytes(), i)
            .await
            .unwrap();
    }

    // Request only 3
    let history = store.get_state_history("key6", 3).await.unwrap();
    assert_eq!(history.len(), 3, "Should respect limit");
    assert_eq!(history[0].version, 1);
    assert_eq!(history[1].version, 2);
    assert_eq!(history[2].version, 3);
}

// ============================================================================
// Unit Tests: get_with_prefix
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_get_with_prefix_empty() {
    let store = InMemorySharedStateStore::new();
    let entries = store.get_with_prefix("autopilot:").await.unwrap();
    assert!(entries.is_empty(), "No entries should match");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_with_prefix_matching() {
    let store = InMemorySharedStateStore::new();

    // Insert keys with different prefixes
    store
        .put("autopilot:run:001".to_string(), b"data1".to_vec(), 1)
        .await
        .unwrap();
    store
        .put("autopilot:run:002".to_string(), b"data2".to_vec(), 1)
        .await
        .unwrap();
    store
        .put("other:key:001".to_string(), b"data3".to_vec(), 1)
        .await
        .unwrap();

    // Query with prefix
    let entries = store.get_with_prefix("autopilot:").await.unwrap();
    assert_eq!(entries.len(), 2, "Should match 2 autopilot keys");

    let keys: Vec<&str> = entries.iter().map(|e| e.key.as_str()).collect();
    assert!(keys.contains(&"autopilot:run:001"));
    assert!(keys.contains(&"autopilot:run:002"));
    assert!(!keys.contains(&"other:key:001"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_with_prefix_exact_match() {
    let store = InMemorySharedStateStore::new();

    store
        .put("exact".to_string(), b"data".to_vec(), 1)
        .await
        .unwrap();
    store
        .put("exact:sub".to_string(), b"data2".to_vec(), 1)
        .await
        .unwrap();

    let entries = store.get_with_prefix("exact").await.unwrap();
    assert_eq!(
        entries.len(),
        2,
        "Should match both 'exact' and 'exact:sub'"
    );
}

// ============================================================================
// Unit Tests: ttl_set
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_ttl_set_basic() {
    let store = InMemorySharedStateStore::new();

    // Set with short TTL
    store
        .ttl_set("temp1".to_string(), b"tempvalue".to_vec(), 1)
        .await
        .unwrap();

    // Immediately readable
    let entry = store.get("temp1").await.unwrap();
    assert!(entry.is_some(), "Should exist immediately after ttl_set");
    assert_eq!(entry.unwrap().value, b"tempvalue");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_ttl_set_expiration() {
    let store = InMemorySharedStateStore::new();

    // Set with 1-second TTL
    store
        .ttl_set("temp2".to_string(), b"expires".to_vec(), 1)
        .await
        .unwrap();

    // Verify exists
    assert!(store.get("temp2").await.unwrap().is_some());

    // Wait for expiration
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Should be gone
    let entry = store.get("temp2").await.unwrap();
    assert!(
        entry.is_none(),
        "Key should be auto-deleted after TTL expires"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_ttl_set_multiple_keys() {
    let store = InMemorySharedStateStore::new();

    // Set multiple keys with different TTLs
    store
        .ttl_set("temp3".to_string(), b"short".to_vec(), 1)
        .await
        .unwrap();
    store
        .ttl_set("temp4".to_string(), b"long".to_vec(), 10)
        .await
        .unwrap();

    // Wait for first to expire
    tokio::time::sleep(Duration::from_secs(2)).await;

    assert!(
        store.get("temp3").await.unwrap().is_none(),
        "temp3 should be deleted"
    );
    assert!(
        store.get("temp4").await.unwrap().is_some(),
        "temp4 should still exist"
    );
}

// ============================================================================
// Unit Tests: watch
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_watch_basic() {
    let store = InMemorySharedStateStore::new();

    // Create watch before any data exists
    let mut rx = store.watch("watch1").await.unwrap();

    // Initial value should be None
    assert!(rx.borrow().is_none(), "Initial watch value should be None");

    // Insert data
    store
        .put("watch1".to_string(), b"data1".to_vec(), 1)
        .await
        .unwrap();

    // Wait for notification
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Should receive update
    let latest = rx.borrow_and_update();
    assert!(latest.is_some(), "Watch should receive update");
    assert_eq!(latest.as_ref().unwrap().value, b"data1");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_watch_multiple_updates() {
    let store = InMemorySharedStateStore::new();

    let rx = store.watch("watch2").await.unwrap();

    // Send multiple updates
    store
        .put("watch2".to_string(), b"v1".to_vec(), 1)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;

    store
        .put("watch2".to_string(), b"v2".to_vec(), 2)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;

    store
        .put("watch2".to_string(), b"v3".to_vec(), 3)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Latest value should be v3
    let latest = rx.borrow();
    assert!(latest.is_some());
    assert_eq!(latest.as_ref().unwrap().value, b"v3");
    assert_eq!(latest.as_ref().unwrap().version, 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_watch_multiple_watchers() {
    let store = InMemorySharedStateStore::new();

    // Create two watchers for same key
    let rx1 = store.watch("watch3").await.unwrap();
    let rx2 = store.watch("watch3").await.unwrap();

    // Update the key
    store
        .put("watch3".to_string(), b"shared".to_vec(), 1)
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Both should receive update
    assert!(
        rx1.borrow().is_some(),
        "First watcher should receive update"
    );
    assert!(
        rx2.borrow().is_some(),
        "Second watcher should receive update"
    );
}

// ============================================================================
// Integration Tests: Autopilot Scenarios
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_autopilot_iteration_tracking_workflow() {
    let store = InMemorySharedStateStore::new();
    let config = CasConfig::for_autopilot();

    // Simulate autopilot iterations
    let run_id = "autopilot-run-001";
    let base_key = format!("autopilot:run:{}", run_id);

    for iteration in 1..=5 {
        let key = format!("{}:iter:{}", base_key, iteration);
        let data = format!("iteration {} state", iteration).into_bytes();

        // Use CAS for safe concurrent writes
        store.cas_with_config(key, 0, data, &config).await.unwrap();
    }

    // Retrieve all iterations using prefix query
    let iterations = store
        .get_with_prefix(&format!("{}:iter:", base_key))
        .await
        .unwrap();
    assert_eq!(iterations.len(), 5, "Should track all 5 iterations");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_autopilot_no_progress_detection() {
    let store = InMemorySharedStateStore::new();

    // Simulate multiple iterations with state tracking
    let state_key = "autopilot:state:002";

    for i in 1..=3 {
        let state_hash = if i < 3 {
            format!("hash_{}", i)
        } else {
            "hash_2".to_string() // Same as previous - no progress
        };

        store
            .put(
                state_key.to_string(),
                state_hash.as_bytes().to_vec(),
                i as u64,
            )
            .await
            .unwrap();
    }

    // Get history and detect no-progress
    let history = store.get_state_history(state_key, 10).await.unwrap();
    assert_eq!(history.len(), 3);

    // Check last two entries
    let last = &history[2];
    let second_last = &history[1];
    assert_eq!(
        last.value, second_last.value,
        "No progress: state hash unchanged"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_autopilot_temporary_intermediate_results() {
    let store = InMemorySharedStateStore::new();

    // Store intermediate result with TTL
    let temp_key = "autopilot:temp:iter:5:analysis";
    store
        .ttl_set(temp_key.to_string(), b"intermediate data".to_vec(), 2)
        .await
        .unwrap();

    // Verify exists
    assert!(store.get(temp_key).await.unwrap().is_some());

    // Wait for cleanup
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Should be auto-cleaned
    assert!(
        store.get(temp_key).await.unwrap().is_none(),
        "Temporary data should be cleaned up"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_autopilot_realtime_monitoring() {
    let store = InMemorySharedStateStore::new();

    // Setup watch for monitoring
    let monitor_key = "autopilot:run:003:status";
    let monitor_rx = store.watch(monitor_key).await.unwrap();

    // Simulate status updates
    tokio::spawn({
        let store = store.clone();
        let key = monitor_key.to_string();
        async move {
            for status in ["running", "paused", "resumed", "completed"] {
                tokio::time::sleep(Duration::from_millis(50)).await;
                store
                    .put(key.clone(), status.as_bytes().to_vec(), 0)
                    .await
                    .unwrap();
            }
        }
    });

    // Monitor should receive real-time updates
    tokio::time::sleep(Duration::from_millis(250)).await;

    let latest = monitor_rx.borrow();
    assert!(latest.is_some());
    // Should have received at least "completed" or a later status
    let status = String::from_utf8(latest.as_ref().unwrap().value.clone()).unwrap();
    assert!(
        ["running", "paused", "resumed", "completed"].contains(&status.as_str()),
        "Should receive valid status update"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_autopilot_high_concurrency_cas() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let config = CasConfig::for_autopilot();

    // Initialize counter
    let counter_key = "autopilot:counter";
    store
        .put(counter_key.to_string(), 0u64.to_le_bytes().to_vec(), 1)
        .await
        .unwrap();

    // Spawn 20 concurrent incrementers
    let handles: Vec<_> = (0..20)
        .map(|_| {
            let store = store.clone();
            let config = config.clone();
            tokio::spawn(async move {
                for _ in 0..5 {
                    loop {
                        let current = store.get(counter_key).await.unwrap().unwrap();
                        let value = u64::from_le_bytes(current.value.try_into().unwrap());
                        let new_value = value + 1;

                        match store
                            .cas_with_config(
                                counter_key.to_string(),
                                current.version,
                                new_value.to_le_bytes().to_vec(),
                                &config,
                            )
                            .await
                        {
                            Ok(_) => break,     // Success, move to next increment
                            Err(_) => continue, // Retry on conflict
                        }
                    }
                }
            })
        })
        .collect();

    // Wait for all to complete
    for handle in handles {
        handle.await.unwrap();
    }

    // Final count should be 20 * 5 = 100
    let final_entry = store.get(counter_key).await.unwrap().unwrap();
    let final_value = u64::from_le_bytes(final_entry.value.try_into().unwrap());
    assert_eq!(final_value, 100, "All concurrent increments should succeed");
}
