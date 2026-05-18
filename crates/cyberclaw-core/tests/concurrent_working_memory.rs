//! Concurrent safety tests for InMemoryWorkingMemory and WorkingMemoryRegistry
//!
//! This test suite validates thread-safety properties under concurrent access:
//! - Concurrent push operations: no entries lost, LRU eviction is consistent
//! - Concurrent checkpoint and rollback: no TOCTOU races
//! - LRU eviction correctness under racing writers
//! - Registry session isolation under concurrent access
//!
//! Uses std::thread (not tokio) because the WorkingMemory trait is sync,
//! not async. Tokio tests are used where needed for coordination.

use cyberclaw_core::ids::ExecutionId;
use cyberclaw_core::memory::working::{
    InMemoryWorkingMemory, WorkingMemory, WorkingMemoryConfig, WorkingMemoryRegistry,
};
use cyberclaw_core::memory_context::{WorkingEntryKind, WorkingMemoryEntry};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn make_entry(summary: &str) -> WorkingMemoryEntry {
    WorkingMemoryEntry {
        execution_id: None,
        kind: WorkingEntryKind::ToolResult,
        summary: summary.to_string(),
        artifact_refs: vec![],
        trace_id: None,
        encrypted: false,
    }
}

fn make_entry_with_exec(summary: &str, exec_id: &str) -> WorkingMemoryEntry {
    WorkingMemoryEntry {
        execution_id: Some(ExecutionId::from_string(exec_id.to_string()).unwrap()),
        kind: WorkingEntryKind::Goal,
        summary: summary.to_string(),
        artifact_refs: vec![],
        trace_id: None,
        encrypted: false,
    }
}

fn large_capacity_wm() -> InMemoryWorkingMemory {
    InMemoryWorkingMemory::new(WorkingMemoryConfig {
        capacity: 10_000,
        ttl_seconds: None,
    })
}

// ---------------------------------------------------------------------------
// Test 1: 8 concurrent threads each push 50 entries — no entry lost
//
// Behavior verified: All 400 pushes succeed; len() == 400 after all threads
// complete. No silent drops or deadlocks from concurrent write access.
// ---------------------------------------------------------------------------

#[test]
fn test_concurrent_push_no_entries_lost() {
    const THREADS: usize = 8;
    const PUSHES_PER_THREAD: usize = 50;
    const TOTAL: usize = THREADS * PUSHES_PER_THREAD;

    let wm = Arc::new(large_capacity_wm());
    let handles: Vec<_> = (0..THREADS)
        .map(|t| {
            let wm = Arc::clone(&wm);
            thread::spawn(move || {
                for i in 0..PUSHES_PER_THREAD {
                    wm.push(make_entry(&format!("thread-{}-entry-{}", t, i)))
                        .expect("push must not fail under concurrent write");
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread must not panic");
    }

    let len = wm.len().unwrap();
    assert_eq!(
        len, TOTAL,
        "expected {} entries after concurrent pushes, found {}",
        TOTAL, len
    );
}

// ---------------------------------------------------------------------------
// Test 2: 16 threads × 100 pushes with capacity=800 — LRU evicts exactly
// the right number of entries
//
// Behavior verified: When total pushes (1600) exceed capacity (800), exactly
// 800 entries remain. No capacity violation. No panic from concurrent eviction.
// ---------------------------------------------------------------------------

#[test]
fn test_concurrent_push_lru_eviction_correct_under_racing_writers() {
    const CAPACITY: usize = 800;
    const THREADS: usize = 16;
    const PUSHES_PER_THREAD: usize = 100;
    const TOTAL_PUSHES: usize = THREADS * PUSHES_PER_THREAD; // 1600 > 800

    let wm = Arc::new(InMemoryWorkingMemory::with_capacity(CAPACITY));
    let handles: Vec<_> = (0..THREADS)
        .map(|t| {
            let wm = Arc::clone(&wm);
            thread::spawn(move || {
                for i in 0..PUSHES_PER_THREAD {
                    wm.push(make_entry(&format!("evict-t{}-i{}", t, i)))
                        .expect("push must succeed");
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread must not panic");
    }

    let len = wm.len().unwrap();
    assert_eq!(
        len, CAPACITY,
        "LRU eviction must maintain capacity={}: found {} entries after {} concurrent pushes",
        CAPACITY, len, TOTAL_PUSHES
    );
}

// ---------------------------------------------------------------------------
// Test 3: Concurrent checkpoint and subsequent rollback — TOCTOU safety
//
// Behavior verified: A checkpoint taken during concurrent writes captures a
// consistent state (no partial entries). Rolling back to the checkpoint
// restores exactly those entries.
//
// The race we test: writer threads are still running when checkpoint is taken.
// The checkpoint must capture a valid consistent snapshot (not a torn state).
//
// Two barriers are used to guarantee deterministic ordering:
//   barrier_before_cp: threads signal they have pushed their "before" entry;
//                      main thread proceeds to checkpoint.
//   barrier_after_cp:  main thread signals checkpoint is complete; threads
//                      then push their "after" entries.
// This ensures (a) checkpoint happens while threads are mid-flight and
// (b) the post-checkpoint pushes always run after checkpoint, so
// final_len is always > cp.entries.len() without any time dependency.
// ---------------------------------------------------------------------------

#[test]
fn test_checkpoint_during_concurrent_writes_captures_consistent_state() {
    const PRE_PUSH: usize = 20;
    const CONCURRENT_THREADS: usize = 4;
    const CONCURRENT_PUSHES: usize = 10;

    let wm = Arc::new(large_capacity_wm());

    // Phase 1: push a known set of entries sequentially
    for i in 0..PRE_PUSH {
        wm.push(make_entry(&format!("pre-{:03}", i))).unwrap();
    }
    assert_eq!(wm.len().unwrap(), PRE_PUSH);

    // Phase 2: take checkpoint WHILE concurrent writers are racing.
    // barrier_before_cp: threads wait here after pushing one "before" entry so
    //                    the main thread knows the store is non-empty before it
    //                    checkpoints.
    // barrier_after_cp:  threads wait here until the main thread has finished
    //                    the checkpoint call, then push their remaining entries.
    //                    This guarantees post-checkpoint pushes land *after*
    //                    cp is taken, making final_len > cp.entries.len()
    //                    unconditionally — no race, no sleep required.
    let barrier_before_cp = Arc::new(std::sync::Barrier::new(CONCURRENT_THREADS + 1));
    let barrier_after_cp = Arc::new(std::sync::Barrier::new(CONCURRENT_THREADS + 1));

    let handles: Vec<_> = (0..CONCURRENT_THREADS)
        .map(|t| {
            let wm = Arc::clone(&wm);
            let b_before = Arc::clone(&barrier_before_cp);
            let b_after = Arc::clone(&barrier_after_cp);
            thread::spawn(move || {
                // Push one entry before checkpoint is taken
                wm.push(make_entry(&format!("concurrent-before-cp-t{}", t)))
                    .unwrap();
                // Signal main thread: at least one entry is in the store
                b_before.wait();
                // Wait until main thread has taken the checkpoint
                b_after.wait();
                // Push remaining entries — these are guaranteed post-checkpoint
                for i in 1..CONCURRENT_PUSHES {
                    wm.push(make_entry(&format!("concurrent-after-cp-t{}-i{}", t, i)))
                        .unwrap();
                }
            })
        })
        .collect();

    // Wait until all concurrent threads have pushed their first entry
    barrier_before_cp.wait();

    // Take checkpoint — threads are paused at barrier_after_cp, so no new
    // writes can race against the checkpoint call itself.
    let cp = wm.checkpoint().unwrap();

    // Release threads to push their post-checkpoint entries
    barrier_after_cp.wait();

    // Checkpoint must have captured a valid consistent state:
    // PRE_PUSH + CONCURRENT_THREADS "before-cp" entries exactly.
    let expected_cp_len = PRE_PUSH + CONCURRENT_THREADS;
    assert_eq!(
        cp.entries.len(),
        expected_cp_len,
        "checkpoint must contain exactly {} entries (pre={} + concurrent_before={}); found {}",
        expected_cp_len,
        PRE_PUSH,
        CONCURRENT_THREADS,
        cp.entries.len()
    );

    // Let all concurrent threads finish their post-checkpoint pushes
    for h in handles {
        h.join().expect("thread must not panic");
    }

    // Post-checkpoint pushes: CONCURRENT_THREADS * (CONCURRENT_PUSHES - 1)
    let post_cp_pushes = CONCURRENT_THREADS * (CONCURRENT_PUSHES - 1);
    let final_len = wm.len().unwrap();
    assert_eq!(
        final_len,
        expected_cp_len + post_cp_pushes,
        "after all threads complete, store must have {} entries; found {}",
        expected_cp_len + post_cp_pushes,
        final_len
    );

    // Phase 3: rollback to checkpoint — must restore exactly cp.entries
    let cp_len = cp.entries.len();
    wm.rollback(&cp).unwrap();

    let after_rollback = wm.len().unwrap();
    assert_eq!(
        after_rollback, cp_len,
        "rollback must restore exactly {} checkpoint entries; found {}",
        cp_len, after_rollback
    );
}

// ---------------------------------------------------------------------------
// Test 4: Concurrent rollback safety — concurrent rollback calls must not
// corrupt state (last-write-wins semantics guaranteed by RwLock)
//
// Behavior verified: Multiple threads calling rollback simultaneously must
// not deadlock or corrupt the state. The final state must be one of the
// valid rollback states.
// ---------------------------------------------------------------------------

#[test]
fn test_concurrent_rollback_calls_do_not_deadlock() {
    let wm = Arc::new(large_capacity_wm());

    // Setup: push 10 entries
    for i in 0..10 {
        wm.push(make_entry(&format!("setup-{}", i))).unwrap();
    }
    let cp = wm.checkpoint().unwrap();

    // Add more entries after checkpoint
    for i in 0..5 {
        wm.push(make_entry(&format!("post-cp-{}", i))).unwrap();
    }

    // 4 threads all call rollback concurrently
    const THREADS: usize = 4;
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let wm = Arc::clone(&wm);
            let cp = cp.clone();
            thread::spawn(move || {
                wm.rollback(&cp).expect("rollback must not fail");
            })
        })
        .collect();

    // Must complete within 5 seconds (no deadlock)
    let timeout = Duration::from_secs(5);
    let wait_start = std::time::Instant::now();
    for h in handles {
        let elapsed = wait_start.elapsed();
        if elapsed >= timeout {
            panic!("concurrent rollback test timed out — possible deadlock");
        }
        h.join().expect("rollback thread must not panic");
    }

    // After all rollbacks, the store must be in a valid state
    // (exactly cp.entries.len() entries because last rollback wins)
    let len = wm.len().unwrap();
    assert_eq!(
        len,
        cp.entries.len(),
        "after concurrent rollbacks, must have exactly {} entries; found {}",
        cp.entries.len(),
        len
    );
}

// ---------------------------------------------------------------------------
// Test 5: Concurrent clear() calls — no deadlock, state is empty after
//
// Behavior verified: Multiple concurrent clear() calls must not deadlock.
// After all clears complete, the store must be empty.
// ---------------------------------------------------------------------------

#[test]
fn test_concurrent_clear_calls_no_deadlock() {
    let wm = Arc::new(large_capacity_wm());

    // Push 100 entries
    for i in 0..100 {
        wm.push(make_entry(&format!("clear-test-{}", i))).unwrap();
    }
    assert_eq!(wm.len().unwrap(), 100);

    const THREADS: usize = 8;
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let wm = Arc::clone(&wm);
            thread::spawn(move || {
                wm.clear().expect("clear must not fail");
            })
        })
        .collect();

    for h in handles {
        h.join().expect("clear thread must not panic");
    }

    // After all clears, the store must be empty
    assert_eq!(
        wm.len().unwrap(),
        0,
        "store must be empty after concurrent clears"
    );
}

// ---------------------------------------------------------------------------
// Test 6: Concurrent push + list_recent — readers must not see partial entries
//
// Behavior verified: list_recent() returns a coherent snapshot (no torn reads
// where only half of an entry is visible). Under concurrent write pressure,
// list_recent results must always have a valid length (0..=capacity).
// ---------------------------------------------------------------------------

#[test]
fn test_concurrent_push_and_list_recent_no_torn_reads() {
    const CAPACITY: usize = 100;
    const WRITER_THREADS: usize = 4;
    const READER_THREADS: usize = 4;
    const WRITES_PER_THREAD: usize = 30;

    let wm = Arc::new(InMemoryWorkingMemory::with_capacity(CAPACITY));

    let mut handles = Vec::new();

    // Spawn writers
    for t in 0..WRITER_THREADS {
        let wm = Arc::clone(&wm);
        handles.push(thread::spawn(move || {
            for i in 0..WRITES_PER_THREAD {
                wm.push(make_entry(&format!("rw-t{}-i{}", t, i))).unwrap();
            }
        }));
    }

    // Spawn readers (concurrently with writers)
    for _ in 0..READER_THREADS {
        let wm = Arc::clone(&wm);
        handles.push(thread::spawn(move || {
            for _ in 0..20 {
                let entries = wm.list_recent(50).unwrap();
                // list_recent must return a valid vec — never exceed capacity
                assert!(
                    entries.len() <= CAPACITY,
                    "list_recent returned {} entries, exceeding capacity {}",
                    entries.len(),
                    CAPACITY
                );
                // Each returned entry must have a non-empty summary (no torn read)
                for e in &entries {
                    assert!(
                        !e.summary.is_empty(),
                        "entry has empty summary — possible torn read"
                    );
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("thread must not panic");
    }
}

// ---------------------------------------------------------------------------
// Test 7: WorkingMemoryRegistry — concurrent session creation is safe
//
// Behavior verified: Multiple threads simultaneously calling registry.session()
// for the same session_id must always return the same Arc (idempotent creation).
// No duplicate sessions; no capacity violations.
// ---------------------------------------------------------------------------

#[test]
fn test_registry_concurrent_session_creation_is_idempotent() {
    use cyberclaw_core::ids::SessionId;

    let registry = Arc::new(WorkingMemoryRegistry::new(
        WorkingMemoryConfig {
            capacity: 50,
            ttl_seconds: None,
        },
        100, // max 100 sessions
    ));

    let session_id = SessionId::from_string("shared-session-id-001".to_string()).unwrap();

    // 10 threads all create the same session simultaneously
    const THREADS: usize = 10;
    let handles: Vec<_> = (0..THREADS)
        .map(|t| {
            let reg = Arc::clone(&registry);
            let sid = session_id.clone();
            thread::spawn(move || {
                let wm = reg.session(&sid).expect("session creation must succeed");
                // Push one entry to verify we can use the session
                wm.push(make_entry(&format!("session-push-t{}", t)))
                    .expect("push must succeed");
                wm.len().unwrap()
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread must not panic");
    }

    // Only one session should exist in the registry (idempotent creation)
    let count = registry.session_count().unwrap();
    assert_eq!(
        count, 1,
        "concurrent session creation must be idempotent: expected 1 session, found {}",
        count
    );

    // The session must have received all pushes from all threads
    let wm = registry.session(&session_id).unwrap();
    let len = wm.len().unwrap();
    assert_eq!(
        len, THREADS,
        "session must contain {} entries from {} concurrent pushers; found {}",
        THREADS, THREADS, len
    );
}

// ---------------------------------------------------------------------------
// Test 8: WorkingMemoryRegistry — concurrent multi-session operations
//
// Behavior verified: Multiple threads each creating and using their own session
// do not interfere with each other. Session isolation is preserved.
// ---------------------------------------------------------------------------

#[test]
fn test_registry_concurrent_isolated_sessions_no_cross_contamination() {
    use cyberclaw_core::ids::SessionId;

    const SESSIONS: usize = 20;
    const PUSHES_PER_SESSION: usize = 10;

    let registry = Arc::new(WorkingMemoryRegistry::new(
        WorkingMemoryConfig {
            capacity: 100,
            ttl_seconds: None,
        },
        SESSIONS + 5, // max sessions with headroom
    ));

    let handles: Vec<_> = (0..SESSIONS)
        .map(|s| {
            let reg = Arc::clone(&registry);
            thread::spawn(move || {
                let sid = SessionId::from_string(format!("isolated-session-{:04}", s)).unwrap();
                let wm = reg.session(&sid).expect("session must be created");

                // Push PUSHES_PER_SESSION entries into this session
                for i in 0..PUSHES_PER_SESSION {
                    wm.push(make_entry_with_exec(
                        &format!("session-{}-entry-{}", s, i),
                        &format!("exec-s{}-e{}", s, i),
                    ))
                    .expect("push must succeed");
                }

                // Verify isolation: only our entries are in this session
                let len = wm.len().unwrap();
                assert_eq!(
                    len, PUSHES_PER_SESSION,
                    "session {} must contain exactly {} entries; found {}",
                    s, PUSHES_PER_SESSION, len
                );
            })
        })
        .collect();

    for h in handles {
        h.join().expect("session thread must not panic");
    }

    // All sessions should be present
    let session_count = registry.session_count().unwrap();
    assert_eq!(
        session_count, SESSIONS,
        "expected {} sessions, found {}",
        SESSIONS, session_count
    );
}

// ---------------------------------------------------------------------------
// Test 9: Concurrent is_empty() checks are consistent with concurrent pushes
//
// Behavior verified: is_empty() never returns true after at least one push
// has completed. Tests the happens-before relationship between push and is_empty.
// ---------------------------------------------------------------------------

#[test]
fn test_is_empty_consistent_after_concurrent_push() {
    let wm = Arc::new(large_capacity_wm());

    // Push one entry sequentially first to establish a baseline
    wm.push(make_entry("baseline")).unwrap();
    assert!(!wm.is_empty().unwrap());

    // Now push 10 more concurrently — is_empty must always return false
    const THREADS: usize = 10;
    let handles: Vec<_> = (0..THREADS)
        .map(|t| {
            let wm = Arc::clone(&wm);
            thread::spawn(move || {
                wm.push(make_entry(&format!("concurrent-{}", t))).unwrap();
                // After our own push, definitely not empty
                let empty = wm.is_empty().unwrap();
                assert!(
                    !empty,
                    "is_empty must return false after at least one push has completed"
                );
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread must not panic");
    }
}

// ---------------------------------------------------------------------------
// Test 10: Stress test — 100 threads pushing for 2 seconds — no panic
//
// Behavior verified: No deadlock, no panic, no lock poisoning under sustained
// concurrent write pressure for 2 seconds.
// ---------------------------------------------------------------------------

#[test]
fn test_stress_100_threads_2_seconds_no_deadlock() {
    const THREADS: usize = 100;
    const CAPACITY: usize = 500;
    let run_duration = Duration::from_secs(2);

    let wm = Arc::new(InMemoryWorkingMemory::with_capacity(CAPACITY));
    let total_pushes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let start = std::time::Instant::now();

    let handles: Vec<_> = (0..THREADS)
        .map(|t| {
            let wm = Arc::clone(&wm);
            let counter = Arc::clone(&total_pushes);
            thread::spawn(move || {
                let mut i = 0usize;
                while start.elapsed() < run_duration {
                    wm.push(make_entry(&format!("stress-t{}-i{}", t, i)))
                        .expect("stress push must not fail");
                    counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    i += 1;
                }
                i
            })
        })
        .collect();

    // Join all threads; collect per-thread push counts
    let per_thread_counts: Vec<usize> = handles
        .into_iter()
        .map(|h| h.join().expect("stress thread must not panic"))
        .collect();

    let total = total_pushes.load(std::sync::atomic::Ordering::Relaxed);
    let thread_sum: usize = per_thread_counts.iter().sum();

    assert_eq!(
        total, thread_sum,
        "atomic counter {} must equal sum of per-thread counts {}",
        total, thread_sum
    );
    assert!(
        total > 0,
        "at least some pushes must have occurred during 2s stress window"
    );

    // Store must be at capacity (bounded by CAPACITY)
    let len = wm.len().unwrap();
    assert_eq!(
        len, CAPACITY,
        "after {} pushes with capacity {}, store must be at capacity; found {}",
        total, CAPACITY, len
    );

    println!(
        "[stress] {} threads for 2s: {} total pushes, {} stored (capacity={})",
        THREADS, total, len, CAPACITY
    );
}
