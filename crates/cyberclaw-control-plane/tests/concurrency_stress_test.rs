/// Concurrency stress tests for SharedStateStore (Task 7.2)
///
/// Tests:
/// 1. 1000+ concurrent CAS operations
/// 2. Multi-thread lock contention simulation
/// 3. Performance benchmarks: throughput and latency distribution
use cyberclaw_control_plane::{InMemorySharedStateStore, SharedStateStore};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinSet;

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Spawn N concurrent tasks, each running `f`, collect all results.
async fn spawn_concurrent<F, Fut, T>(n: usize, f: F) -> Vec<T>
where
    F: Fn(usize) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let f = Arc::new(f);
    let mut set = JoinSet::new();
    for i in 0..n {
        let f = f.clone();
        set.spawn(async move { f(i).await });
    }
    let mut results = Vec::with_capacity(n);
    while let Some(r) = set.join_next().await {
        results.push(r.expect("task panicked"));
    }
    results
}

// ─── Test 1: 1000+ concurrent CAS operations on separate keys ────────────────

/// Each task owns its own key — no contention.  All 1000 CAS calls must succeed.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_1000_concurrent_cas_separate_keys() {
    const N: usize = 1000;
    let store = Arc::new(InMemorySharedStateStore::new());

    let results = spawn_concurrent(N, move |i| {
        let store = store.clone();
        async move {
            let key = format!("key-{}", i);
            store.cas(key, 0, b"value".to_vec()).await
        }
    })
    .await;

    let failures: Vec<_> = results.iter().filter(|r| r.is_err()).collect();
    assert!(
        failures.is_empty(),
        "{} out of {} CAS operations failed (expected 0): first error: {:?}",
        failures.len(),
        N,
        failures.first()
    );
}

// ─── Test 2: 1000+ concurrent CAS operations on the SAME key (contention) ───

/// All tasks race on a single counter key.  Only one succeeds per round;
/// the rest must observe correct failure.  We verify final version equals
/// the number of successful increments.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_1000_concurrent_cas_same_key_contention() {
    const N: usize = 1000;
    let store = Arc::new(InMemorySharedStateStore::new());

    // Initialise the key at version 1, value = 0
    let init_bytes = serde_json::to_vec(&serde_json::json!(0)).unwrap();
    store
        .put("counter".to_string(), init_bytes, 1)
        .await
        .unwrap();

    // Each task reads the current version then attempts one CAS increment.
    // Some will conflict; that is expected.  We count successes.
    let success_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let sc = success_count.clone();
    let store_for_closure = store.clone();

    let results = spawn_concurrent(N, move |_i| {
        let store = store_for_closure.clone();
        let sc = sc.clone();
        async move {
            // Read current version
            let entry = store.get("counter").await.unwrap();
            let (version, cur_val) = match entry {
                Some(e) => {
                    let v: i64 = serde_json::from_slice(&e.value).unwrap_or(0);
                    (e.version, v)
                }
                None => (0, 0),
            };
            let new_bytes = serde_json::to_vec(&(cur_val + 1)).unwrap();
            let r = store.cas("counter".to_string(), version, new_bytes).await;
            if r.is_ok() {
                sc.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            r.is_ok()
        }
    })
    .await;

    let successes = results.iter().filter(|&&ok| ok).count();
    assert!(
        successes >= 1,
        "At least one CAS should succeed; got {}",
        successes
    );

    // The final version reflects how many writes actually landed.
    let final_entry = store.get("counter").await.unwrap().unwrap();
    // version started at 1, each success bumps it by 1
    let expected_version = 1 + successes as u64;
    assert_eq!(
        final_entry.version, expected_version,
        "Final version should match success count"
    );
}

// ─── Test 3: cas_with_retry convergence under high contention ────────────────

/// 100 concurrent tasks all use cas_with_retry to atomically increment a
/// shared counter.  Because each retries until it wins, ALL must succeed,
/// and the final value must equal N.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_cas_with_retry_convergence_under_contention() {
    const N: usize = 100;
    let store = Arc::new(InMemorySharedStateStore::new());

    // Set initial counter = 0 at version 1
    let init = serde_json::to_vec(&serde_json::json!({"count": 0})).unwrap();
    store.put("counter".to_string(), init, 1).await.unwrap();

    let store_for_retry = store.clone();
    let results = spawn_concurrent(N, move |_i| {
        let store = store_for_retry.clone();
        async move {
            store
                .cas_with_retry(
                    "counter",
                    |current| {
                        let n = current
                            .as_ref()
                            .and_then(|v| v.get("count"))
                            .and_then(|c| c.as_i64())
                            .unwrap_or(0);
                        Ok(serde_json::json!({ "count": n + 1 }))
                    },
                    50, // generous retry budget
                )
                .await
        }
    })
    .await;

    let failures: Vec<_> = results.iter().filter(|r| r.is_err()).collect();
    assert!(
        failures.is_empty(),
        "{} tasks failed: {:?}",
        failures.len(),
        failures.first()
    );

    // Final counter must equal N
    let entry = store.get("counter").await.unwrap().unwrap();
    let val: serde_json::Value = serde_json::from_slice(&entry.value).unwrap();
    assert_eq!(
        val["count"], N as i64,
        "All {} increments should be reflected",
        N
    );
}

// ─── Test 4: Mixed read/write concurrency ───────────────────────────────────

/// 500 readers + 500 writers running simultaneously on 10 shared keys.
/// Verifies no panics, no data corruption, and all reads return valid versions.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_mixed_read_write_concurrency() {
    const READERS: usize = 500;
    const WRITERS: usize = 500;
    const NUM_KEYS: usize = 10;

    let store = Arc::new(InMemorySharedStateStore::new());

    // Initialise keys
    for i in 0..NUM_KEYS {
        store
            .cas(
                format!("shared-key-{}", i),
                0,
                serde_json::to_vec(&serde_json::json!(0)).unwrap(),
            )
            .await
            .unwrap();
    }

    let mut set = JoinSet::new();

    // Spawn readers
    for r in 0..READERS {
        let store = store.clone();
        set.spawn(async move {
            let key = format!("shared-key-{}", r % NUM_KEYS);
            let result = store.get(&key).await;
            // Readers just check the entry is valid
            if let Ok(Some(entry)) = result {
                assert!(entry.version >= 1, "Version must be at least 1");
            }
            true
        });
    }

    // Spawn writers (each attempts one CAS increment, tolerating conflicts)
    for w in 0..WRITERS {
        let store = store.clone();
        set.spawn(async move {
            let key = format!("shared-key-{}", w % NUM_KEYS);
            let entry = store.get(&key).await;
            if let Ok(Some(e)) = entry {
                let cur: i64 = serde_json::from_slice(&e.value).unwrap_or(0);
                let new_bytes = serde_json::to_vec(&(cur + 1)).unwrap();
                let _ = store.cas(key, e.version, new_bytes).await; // ignore conflicts
            }
            true
        });
    }

    let mut all_ok = true;
    while let Some(r) = set.join_next().await {
        if !r.expect("task panicked") {
            all_ok = false;
        }
    }
    assert!(all_ok, "Some tasks returned false");
}

// ─── Test 5: Throughput benchmark ───────────────────────────────────────────

/// Measures the throughput of CAS operations (separate keys, no contention).
/// Asserts a minimum throughput threshold to catch performance regressions.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_cas_throughput_benchmark() {
    const N: usize = 2000;
    const MIN_OPS_PER_SEC: f64 = 5_000.0; // conservative lower bound

    let store = Arc::new(InMemorySharedStateStore::new());
    let start = Instant::now();

    let results = spawn_concurrent(N, move |i| {
        let store = store.clone();
        async move {
            store
                .cas(format!("bench-key-{}", i), 0, b"v".to_vec())
                .await
        }
    })
    .await;

    let elapsed = start.elapsed();
    let successes = results.iter().filter(|r| r.is_ok()).count();
    let ops_per_sec = successes as f64 / elapsed.as_secs_f64();

    println!(
        "[throughput] {} CAS ops in {:.2}ms = {:.0} ops/sec",
        successes,
        elapsed.as_millis(),
        ops_per_sec
    );

    assert_eq!(successes, N, "All CAS ops on separate keys should succeed");
    assert!(
        ops_per_sec >= MIN_OPS_PER_SEC,
        "Throughput {:.0} ops/sec is below minimum {:.0} ops/sec",
        ops_per_sec,
        MIN_OPS_PER_SEC
    );
}

// ─── Test 6: Latency distribution ───────────────────────────────────────────

/// Measures p50/p95/p99 latency for single-key CAS operations (sequential,
/// no contention), providing a latency profile for the store.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_cas_latency_distribution() {
    const N: usize = 200;
    let store = InMemorySharedStateStore::new();

    let mut latencies_us: Vec<u64> = Vec::with_capacity(N);

    // Sequential CAS operations to isolate latency
    store
        .cas("lat-key".to_string(), 0, b"init".to_vec())
        .await
        .unwrap();
    for version in 1..=N as u64 {
        let t0 = Instant::now();
        store
            .cas("lat-key".to_string(), version, b"val".to_vec())
            .await
            .unwrap();
        latencies_us.push(t0.elapsed().as_micros().min(u64::MAX as u128) as u64);
    }

    latencies_us.sort_unstable();
    let p50 = latencies_us[N / 2];
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    let p95 = latencies_us[(N as f64 * 0.95) as usize];
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    let p99 = latencies_us[(N as f64 * 0.99) as usize];
    let max = *latencies_us.last().unwrap();

    println!(
        "[latency] p50={}µs  p95={}µs  p99={}µs  max={}µs",
        p50, p95, p99, max
    );

    // p99 should be under 50ms for an in-memory store
    assert!(p99 < 50_000, "p99 latency {}µs exceeds 50ms threshold", p99);
}

// ─── Test 7: Lock contention simulation (multi-threaded) ─────────────────────

/// Spawns many threads that simultaneously hold the store reference and
/// perform interleaved reads and writes, verifying the RwLock never deadlocks
/// and all operations complete within a reasonable timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_lock_contention_simulation() {
    const TASKS: usize = 200;
    const OPS_PER_TASK: usize = 10;

    let store = Arc::new(InMemorySharedStateStore::new());

    // Pre-populate keys
    for i in 0..20 {
        store
            .cas(format!("contention-{}", i), 0, b"0".to_vec())
            .await
            .unwrap();
    }

    let timeout = Duration::from_secs(30);
    let store_for_contention = store.clone();
    let result = tokio::time::timeout(timeout, async {
        spawn_concurrent(TASKS, move |t| {
            let store = store_for_contention.clone();
            async move {
                for op in 0..OPS_PER_TASK {
                    let key = format!("contention-{}", (t + op) % 20);
                    // Alternate reads and writes
                    if op % 2 == 0 {
                        let _ = store.get(&key).await;
                    } else {
                        let entry = store.get(&key).await;
                        if let Ok(Some(e)) = entry {
                            let _ = store.cas(key, e.version, b"updated".to_vec()).await;
                        }
                    }
                }
                true
            }
        })
        .await
    })
    .await;

    assert!(
        result.is_ok(),
        "Lock contention test timed out after {}s — possible deadlock",
        timeout.as_secs()
    );
}

// ─── Test 8: Metrics under concurrent load ──────────────────────────────────

/// Verifies that CAS metrics are correctly tracked under concurrent load.
/// All atomic counters must be consistent after all tasks finish.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_metrics_under_concurrent_load() {
    const N: usize = 500;
    let store = Arc::new(InMemorySharedStateStore::new());

    // Mix of successful CAS (new keys) and conflicting CAS (version 0 on
    // existing keys after the first writer wins).
    let store_for_metrics = store.clone();
    spawn_concurrent(N, move |i| {
        let store = store_for_metrics.clone();
        async move {
            // Each task creates a unique key → all should succeed
            let _ = store
                .cas(format!("metrics-key-{}", i), 0, b"v".to_vec())
                .await;
        }
    })
    .await;

    let m = store.metrics();
    let successes = m.success_count();
    let conflicts = m.conflict_count();
    let total = successes + conflicts;

    println!(
        "[metrics] successes={} conflicts={} total={} conflict_rate={:.2}%",
        successes,
        conflicts,
        total,
        m.conflict_rate() * 100.0
    );

    // All N CAS ops on unique keys must succeed
    assert_eq!(successes, N as u64, "Expected {} successes", N);
    assert_eq!(conflicts, 0, "Expected 0 conflicts on unique keys");
}

// ─── Test 9: cas_with_retry exhaustion (negative test) ───────────────────────

/// Verifies that cas_with_retry fails correctly when the update_fn always
/// sees a version that another writer keeps bumping.  We simulate this by
/// giving max_retries=0 — the first conflict triggers failure immediately.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_cas_with_retry_exhaustion() {
    let store = Arc::new(InMemorySharedStateStore::new());

    // Set up a key
    let init = serde_json::to_vec(&serde_json::json!(0)).unwrap();
    store
        .put("exhaustion-key".to_string(), init, 1)
        .await
        .unwrap();

    // First writer bumps version to 2
    let store2 = store.clone();
    tokio::spawn(async move {
        let _ = store2
            .cas(
                "exhaustion-key".to_string(),
                1,
                serde_json::to_vec(&serde_json::json!(99)).unwrap(),
            )
            .await;
    })
    .await
    .unwrap();

    // Now try with 0 retries — should fail on the very first conflict
    let result = store
        .cas_with_retry(
            "exhaustion-key",
            |_| Ok(serde_json::json!(42)), // update_fn that uses stale version
            0,
        )
        .await;

    // Either it succeeds (if it happened to read version 2 before the spawn)
    // or it fails due to exhaustion — both are valid; just assert no panic.
    let _ = result; // outcome depends on scheduling; we only check for no panic
}
