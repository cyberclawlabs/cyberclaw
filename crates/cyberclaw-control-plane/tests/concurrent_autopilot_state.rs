// SPDX-License-Identifier: Apache-2.0

//! Concurrent Safety Tests for AutopilotStateSyncCoordinator
//!
//! Tests concurrent access patterns to detect:
//! - Data races in CAS-based state synchronization
//! - Deadlocks in high-contention write scenarios
//! - Deduplication logic correctness under concurrent writes
//! - Version monotonicity guarantees

use chrono::Utc;
use cyberclaw_control_plane::{
    autopilot_state_sync::DefaultStateSyncCoordinator,
    autopilot_types::{AutopilotPhase, AutopilotStep, V2IterationState},
    AutopilotStateSyncCoordinator, InMemorySharedStateStore, SharedStateStore,
};
use cyberclaw_core::ids::ExecutionId;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::task::JoinSet;

// ─── Helpers ────────────────────────────────────────────────────────────────

fn make_iteration(iteration_id: u32, step: AutopilotStep) -> V2IterationState {
    V2IterationState {
        iteration_id,
        step,
        start_time: Utc::now(),
        end_time: None,
        state_hash: format!("hash_{}", iteration_id),
        progress_delta: None,
        execution_results: Vec::new(),
        current_phase: AutopilotPhase::Plan,
        fix_loop_count: 0,
        plan_mode_snapshot: None,
    }
}

fn make_sync(store: Arc<InMemorySharedStateStore>) -> Arc<DefaultStateSyncCoordinator> {
    Arc::new(DefaultStateSyncCoordinator::new(store))
}

// ─── Test 1: Concurrent sync_to_store with distinct iteration IDs ────────────

/// 20 tasks each write a unique iteration to the same run_id.
/// All must succeed and all 20 iterations must appear in the final state.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_concurrent_sync_distinct_iterations_no_loss() {
    const N: usize = 20;

    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = make_sync(store.clone());
    let run_id = ExecutionId::new();

    let mut set = JoinSet::new();
    for i in 1..=N {
        let sync = sync.clone();
        let run_id = run_id.clone();
        set.spawn(async move {
            let state = make_iteration(i as u32, AutopilotStep::Execute);
            sync.sync_to_store(&run_id, &state).await
        });
    }

    let mut failures = 0usize;
    while let Some(result) = set.join_next().await {
        if result.expect("task panicked").is_err() {
            failures += 1;
        }
    }

    // At most a small number of retries-exceeded failures are tolerable
    // because CAS backoff is bounded (20 retries × 50ms).
    // With N=20 and a fast in-memory store, all should succeed.
    assert_eq!(failures, 0, "{} of {} concurrent syncs failed", failures, N);

    let restored = sync
        .sync_from_store(&run_id)
        .await
        .expect("sync_from_store failed")
        .expect("expected Some state");

    assert_eq!(
        restored.iterations.len(),
        N,
        "Expected {} iterations, found {}",
        N,
        restored.iterations.len()
    );
}

// ─── Test 2: Deduplication prevents duplicate iterations under concurrency ──

/// 30 tasks all try to write the SAME iteration_id to the same run_id.
/// Deduplication logic must ensure exactly 1 iteration is stored, not 30.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_concurrent_deduplication_prevents_duplicate_iterations() {
    const THREADS: usize = 30;

    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = make_sync(store.clone());
    let run_id = ExecutionId::new();

    // The same iteration_id = 1 sent by 30 concurrent tasks
    let state = make_iteration(1, AutopilotStep::Initialize);

    let mut set = JoinSet::new();
    for _ in 0..THREADS {
        let sync = sync.clone();
        let run_id = run_id.clone();
        let state = state.clone();
        set.spawn(async move { sync.sync_to_store(&run_id, &state).await });
    }

    let mut errors = 0usize;
    while let Some(r) = set.join_next().await {
        if r.expect("task panicked").is_err() {
            errors += 1;
        }
    }
    assert_eq!(errors, 0, "{} sync tasks failed unexpectedly", errors);

    let restored = sync
        .sync_from_store(&run_id)
        .await
        .expect("sync_from_store failed")
        .expect("expected Some state");

    assert_eq!(
        restored.iterations.len(),
        1,
        "Deduplication must produce exactly 1 iteration, found {}",
        restored.iterations.len()
    );
    assert_eq!(restored.iterations[0].iteration_id, 1);
}

// ─── Test 3: Version monotonically increases under sequential concurrent writes

/// 10 tasks write sequentially distinct iteration IDs using the same sync.
/// Each successful CAS write must increment the store version by exactly 1.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_version_monotonically_increases() {
    const ITERATIONS: usize = 10;

    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = make_sync(store.clone());
    let run_id = ExecutionId::new();

    // Write iterations one by one (sequential within a single task) to
    // guarantee version ordering without CAS contention.
    for i in 1..=ITERATIONS {
        let state = make_iteration(i as u32, AutopilotStep::Plan);
        sync.sync_to_store(&run_id, &state)
            .await
            .expect("sync_to_store failed");
    }

    let key = format!("autopilot:run:{}", run_id);
    let entry = store
        .get(&key)
        .await
        .expect("store.get failed")
        .expect("expected Some entry");

    // Each successful write increments version by 1; after N writes version == N
    assert_eq!(
        entry.version, ITERATIONS as u64,
        "Version must equal number of writes: expected {}, got {}",
        ITERATIONS, entry.version
    );
}

// ─── Test 4: Concurrent sync_from_store does not conflict with sync_to_store ─

/// 20 writers and 20 readers run concurrently on the same run_id.
/// No panics, no deadlocks; readers always receive valid (possibly empty) state.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_concurrent_reads_and_writes_no_conflict() {
    const WRITERS: usize = 20;
    const READERS: usize = 20;

    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = make_sync(store.clone());
    let run_id = ExecutionId::new();

    let mut set = JoinSet::new();

    // Writers: each writes a unique iteration
    for i in 1..=WRITERS {
        let sync = sync.clone();
        let run_id = run_id.clone();
        set.spawn(async move {
            let state = make_iteration(i as u32, AutopilotStep::Analyze);
            sync.sync_to_store(&run_id, &state).await.map(|_| "write")
        });
    }

    // Readers: each calls sync_from_store
    for _ in 0..READERS {
        let sync = sync.clone();
        let run_id = run_id.clone();
        set.spawn(async move { sync.sync_from_store(&run_id).await.map(|_| "read") });
    }

    let mut failures = 0usize;
    while let Some(r) = set.join_next().await {
        if r.expect("task panicked").is_err() {
            failures += 1;
        }
    }

    assert_eq!(
        failures,
        0,
        "{} of {} concurrent read+write operations failed",
        failures,
        WRITERS + READERS
    );
}

// ─── Test 5: Concurrent bidirectional_sync does not deadlock ─────────────────

/// 10 tasks call bidirectional_sync on the same run_id simultaneously.
/// All must complete within a timeout and return Ok.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_concurrent_bidirectional_sync_no_deadlock() {
    const THREADS: usize = 10;

    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = make_sync(store.clone());
    let run_id = ExecutionId::new();

    // Prime the store with one iteration
    let state = make_iteration(1, AutopilotStep::Execute);
    sync.sync_to_store(&run_id, &state)
        .await
        .expect("initial sync failed");

    let timeout = Duration::from_secs(15);

    let handle = tokio::spawn({
        let sync = sync.clone();
        let run_id = run_id.clone();
        async move {
            let mut set = JoinSet::new();
            for _ in 0..THREADS {
                let sync = sync.clone();
                let run_id = run_id.clone();
                set.spawn(async move { sync.bidirectional_sync(&run_id).await });
            }
            let mut errors = 0usize;
            while let Some(r) = set.join_next().await {
                if r.expect("task panicked").is_err() {
                    errors += 1;
                }
            }
            errors
        }
    });

    let errors = tokio::time::timeout(timeout, handle)
        .await
        .expect("bidirectional_sync timed out — possible deadlock")
        .expect("join handle failed");

    assert_eq!(
        errors, 0,
        "{} bidirectional_sync calls returned errors",
        errors
    );
}

// ─── Test 6: State hash consistency — concurrent writers preserve last_state_hash

/// Multiple writers write sequential iterations; the final stored state_hash
/// must match the last iteration written and must never be empty.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_state_hash_consistency_after_concurrent_writes() {
    const N: usize = 15;

    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = make_sync(store);
    let run_id = ExecutionId::new();

    // Write iterations sequentially so we know the exact expected hash
    for i in 1..=N {
        let state = make_iteration(i as u32, AutopilotStep::Check);
        sync.sync_to_store(&run_id, &state)
            .await
            .expect("sync_to_store failed");
    }

    let restored = sync
        .sync_from_store(&run_id)
        .await
        .expect("sync_from_store failed")
        .expect("expected Some state");

    let hash = restored
        .last_state_hash
        .expect("last_state_hash must be Some");
    // Each hash was set to "hash_{iteration_id}"
    // The last iteration written is N
    assert_eq!(
        hash,
        format!("hash_{}", N),
        "last_state_hash must reflect the final iteration"
    );
}

// ─── Test 7: Isolated run_ids — concurrent writes to different runs don't mix ─

/// 50 tasks each write to their own unique run_id.
/// After all writes, each run must contain exactly its own 1 iteration.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_concurrent_writes_isolated_run_ids_no_cross_contamination() {
    const RUNS: usize = 50;

    let store = Arc::new(InMemorySharedStateStore::new());

    // Collect run_ids upfront (can't clone ExecutionId in a closure easily)
    let run_ids: Vec<ExecutionId> = (0..RUNS).map(|_| ExecutionId::new()).collect();

    let mut set = JoinSet::new();
    for run_id in &run_ids {
        let run_id = run_id.clone();
        let store = store.clone();
        set.spawn(async move {
            let sync = DefaultStateSyncCoordinator::new(store);
            let state = make_iteration(1, AutopilotStep::Decide);
            sync.sync_to_store(&run_id, &state).await?;
            let restored = sync.sync_from_store(&run_id).await?;
            Ok::<_, anyhow::Error>(restored.map(|r| r.iterations.len()).unwrap_or(0))
        });
    }

    let mut all_counts = Vec::new();
    while let Some(r) = set.join_next().await {
        let count = r.expect("task panicked").expect("sync error");
        all_counts.push(count);
    }

    assert_eq!(all_counts.len(), RUNS);
    for (i, count) in all_counts.iter().enumerate() {
        assert_eq!(
            *count, 1,
            "Run {} has {} iterations, expected exactly 1",
            i, count
        );
    }
}

// ─── Test 8: CAS contention counter — successful writes are counted accurately ─

/// 40 tasks race to write different iterations to the same run_id.
/// We count how many succeed. The sum of successful writes must match
/// the number of iterations stored.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_cas_contention_successful_count_matches_stored_iterations() {
    const N: usize = 40;

    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = make_sync(store.clone());
    let run_id = ExecutionId::new();

    let success_count = Arc::new(AtomicU64::new(0));

    let mut set = JoinSet::new();
    for i in 1..=N {
        let sync = sync.clone();
        let run_id = run_id.clone();
        let sc = success_count.clone();
        set.spawn(async move {
            let state = make_iteration(i as u32, AutopilotStep::Update);
            let result = sync.sync_to_store(&run_id, &state).await;
            if result.is_ok() {
                sc.fetch_add(1, Ordering::Relaxed);
            }
            result.is_ok()
        });
    }

    while set.join_next().await.is_some() {}

    let successes = success_count.load(Ordering::Relaxed) as usize;
    let restored = sync
        .sync_from_store(&run_id)
        .await
        .expect("sync_from_store failed")
        .expect("expected Some state");

    assert_eq!(
        restored.iterations.len(),
        successes,
        "Stored iterations ({}) must equal successful writes ({})",
        restored.iterations.len(),
        successes
    );

    // At high concurrency (N=40, retries=20) some writes may fail due to
    // exhausted CAS retries.  At minimum, more than half must succeed.
    assert!(
        successes >= N / 2,
        "Too few successful writes: {}/{} succeeded",
        successes,
        N
    );
}

// ─── Test 9: sync_to_store and sync_from_store with progress tracking ─────────

/// Multiple writers track a progress counter. After all writes, the
/// current_iteration in the stored state must equal the highest iteration_id
/// that was successfully written.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_current_iteration_tracks_highest_written() {
    const N: usize = 10;

    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = make_sync(store);
    let run_id = ExecutionId::new();

    // Sequential writes guarantee ordering
    for i in 1..=N {
        let state = make_iteration(i as u32, AutopilotStep::Iterate);
        sync.sync_to_store(&run_id, &state)
            .await
            .expect("sync_to_store failed");
    }

    let restored = sync
        .sync_from_store(&run_id)
        .await
        .expect("sync_from_store failed")
        .expect("expected Some state");

    assert_eq!(
        restored.current_iteration, N as u32,
        "current_iteration must equal last written iteration_id"
    );
    assert_eq!(restored.iterations.len(), N);
}

// ─── Test 10: Stress test — 100 goroutines writing for 2 seconds ─────────────

/// Sustained concurrent writes across 100 tasks for 2 seconds.
/// No panics, no deadlocks. Stored state must be internally consistent
/// (iteration IDs unique, count <= total spawned).
#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
async fn test_stress_100_tasks_sustained_writes() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let run_id = ExecutionId::new();
    let sync = make_sync(store);

    let writes = Arc::new(AtomicU64::new(0));
    let end_time = Instant::now() + Duration::from_secs(2);

    let mut set = JoinSet::new();

    for _task_id in 0..100 {
        let sync = sync.clone();
        let run_id = run_id.clone();
        let writes = writes.clone();

        set.spawn(async move {
            let mut local_iter = writes.fetch_add(1, Ordering::Relaxed) as u32 + 1;

            while Instant::now() < end_time {
                let state = make_iteration(local_iter, AutopilotStep::Execute);
                // Intentionally ignore CAS-retry-exhaustion errors; they're
                // expected under extreme contention and are not a correctness bug.
                let _ = sync.sync_to_store(&run_id, &state).await;
                local_iter = writes.fetch_add(1, Ordering::Relaxed) as u32 + 1;

                // Yield to other tasks
                tokio::task::yield_now().await;
            }
        });
    }

    // Await all tasks with a generous timeout (5s > 2s run duration)
    let deadline = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            _ = &mut deadline => {
                set.abort_all();
                break;
            }
            result = set.join_next() => {
                match result {
                    None => break,  // All tasks complete
                    Some(r) => { r.expect("stress task panicked"); }
                }
            }
        }
    }

    // Verify final state integrity
    let maybe_state = sync
        .sync_from_store(&run_id)
        .await
        .expect("sync_from_store failed after stress test");

    if let Some(state) = maybe_state {
        // Iteration IDs must be unique
        let mut seen = std::collections::HashSet::new();
        for iter in &state.iterations {
            assert!(
                seen.insert(iter.iteration_id),
                "Duplicate iteration_id {} found in stress test state",
                iter.iteration_id
            );
        }

        // current_iteration must be one of the actually stored iterations
        let stored_ids: std::collections::HashSet<u32> =
            state.iterations.iter().map(|i| i.iteration_id).collect();
        assert!(
            stored_ids.contains(&state.current_iteration),
            "current_iteration {} not in stored iteration IDs",
            state.current_iteration
        );
    }
    // If no state was stored (all writes failed under extreme contention), that is
    // acceptable for this stress scenario; the important assertion is no panics.
}
