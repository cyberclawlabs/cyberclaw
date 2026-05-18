//! Working memory implementation for CyberClaw.
//!
//! Provides a short-term, session-scoped memory cache with:
//! - LRU eviction (capacity-bounded dequeue)
//! - TTL expiry (time-to-live per entry)
//! - Session-level isolation (per-session stores keyed by SessionId)
//! - Thread-safe access via Arc<RwLock<...>>
//! - Full serde serialization support

use crate::errors::{MemoryError, MemoryResult};
use crate::ids::SessionId;
use crate::memory_context::{WorkingMemoryCheckpoint, WorkingMemoryEntry};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for a `SessionWorkingMemory` instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingMemoryConfig {
    /// Maximum number of entries retained (LRU eviction when exceeded).
    pub capacity: usize,
    /// Optional TTL in seconds.  `None` means entries never expire.
    pub ttl_seconds: Option<i64>,
}

impl Default for WorkingMemoryConfig {
    fn default() -> Self {
        Self {
            capacity: 200,
            ttl_seconds: Some(3600), // 1 hour default
        }
    }
}

// ---------------------------------------------------------------------------
// Timed entry wrapper
// ---------------------------------------------------------------------------

/// An entry stored inside the ring-buffer together with its insertion time.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimedEntry {
    pub entry: WorkingMemoryEntry,
    pub inserted_at: DateTime<Utc>,
}

impl TimedEntry {
    fn new(entry: WorkingMemoryEntry) -> Self {
        Self {
            entry,
            inserted_at: Utc::now(),
        }
    }

    /// Returns `true` when this entry has expired according to the given TTL.
    fn is_expired(&self, ttl: Duration) -> bool {
        Utc::now() - self.inserted_at > ttl
    }
}

// ---------------------------------------------------------------------------
// Core trait
// ---------------------------------------------------------------------------

/// Core trait for Working Memory operations (per-session).
///
/// Implementors must be `Send + Sync` to allow concurrent usage from
/// multiple tokio tasks.
pub trait WorkingMemory: Send + Sync {
    /// Append a new entry.  Evicts the oldest entry when capacity is reached.
    fn push(&self, entry: WorkingMemoryEntry) -> MemoryResult<()>;

    /// Return the `limit` most-recent (non-expired) entries in insertion order.
    fn list_recent(&self, limit: usize) -> MemoryResult<Vec<WorkingMemoryEntry>>;

    /// Snapshot the current state into a checkpoint.
    fn checkpoint(&self) -> MemoryResult<WorkingMemoryCheckpoint>;

    /// Restore state from a previously created checkpoint.
    fn rollback(&self, checkpoint: &WorkingMemoryCheckpoint) -> MemoryResult<()>;

    /// Remove all entries.
    fn clear(&self) -> MemoryResult<()>;

    /// Current number of live (non-expired) entries.
    fn len(&self) -> MemoryResult<usize>;

    /// Returns `true` when there are no live entries.
    fn is_empty(&self) -> MemoryResult<bool> {
        Ok(self.len()? == 0)
    }
}

// ---------------------------------------------------------------------------
// In-memory implementation
// ---------------------------------------------------------------------------

/// Single-session working memory backed by a `VecDeque` (ring buffer).
///
/// - **LRU eviction**: when `push()` exceeds capacity the front element is dropped.
/// - **TTL expiry**: expired entries are lazily pruned on every read/write.
/// - **Thread safety**: all mutable state lives behind `Arc<RwLock<...>>`.
#[derive(Debug, Clone)]
pub struct InMemoryWorkingMemory {
    state: Arc<RwLock<WorkingMemoryState>>,
    config: WorkingMemoryConfig,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkingMemoryState {
    entries: VecDeque<TimedEntry>,
}

impl WorkingMemoryState {
    fn new() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }

    /// Evict expired entries (lazy TTL purge).
    fn purge_expired(&mut self, ttl: Duration) {
        self.entries.retain(|e| !e.is_expired(ttl));
    }
}

impl InMemoryWorkingMemory {
    /// Create a new working memory with the given configuration.
    pub fn new(config: WorkingMemoryConfig) -> Self {
        Self {
            state: Arc::new(RwLock::new(WorkingMemoryState::new())),
            config,
        }
    }

    /// Convenience constructor with default config.
    pub fn with_capacity(capacity: usize) -> Self {
        Self::new(WorkingMemoryConfig {
            capacity,
            ttl_seconds: None,
        })
    }

    fn ttl_duration(&self) -> Option<Duration> {
        self.config.ttl_seconds.map(Duration::seconds)
    }

    /// Acquire a write lock, returning a `MemoryError::LockPoisoned` on failure.
    fn write_state(&self) -> MemoryResult<std::sync::RwLockWriteGuard<'_, WorkingMemoryState>> {
        self.state
            .write()
            .map_err(|e| MemoryError::LockPoisoned(format!("write lock: {e}")))
    }

    /// Acquire a read lock, returning a `MemoryError::LockPoisoned` on failure.
    fn read_state(&self) -> MemoryResult<std::sync::RwLockReadGuard<'_, WorkingMemoryState>> {
        self.state
            .read()
            .map_err(|e| MemoryError::LockPoisoned(format!("read lock: {e}")))
    }
}

impl WorkingMemory for InMemoryWorkingMemory {
    fn push(&self, entry: WorkingMemoryEntry) -> MemoryResult<()> {
        let mut state = self.write_state()?;

        // Lazy TTL eviction before adding a new entry
        if let Some(ttl) = self.ttl_duration() {
            state.purge_expired(ttl);
        }

        // LRU eviction: drop front (oldest) when at capacity
        while state.entries.len() >= self.config.capacity {
            state.entries.pop_front();
        }

        state.entries.push_back(TimedEntry::new(entry));
        drop(state);
        Ok(())
    }

    fn list_recent(&self, limit: usize) -> MemoryResult<Vec<WorkingMemoryEntry>> {
        // Need write lock for lazy purge
        let mut state = self.write_state()?;
        if let Some(ttl) = self.ttl_duration() {
            state.purge_expired(ttl);
        }

        let entries: Vec<WorkingMemoryEntry> = state
            .entries
            .iter()
            .rev() // most-recent first
            .take(limit)
            .map(|te| te.entry.clone())
            .collect::<Vec<_>>()
            .into_iter()
            .rev() // restore insertion order
            .collect();
        drop(state);

        Ok(entries)
    }

    fn checkpoint(&self) -> MemoryResult<WorkingMemoryCheckpoint> {
        let entries: Vec<WorkingMemoryEntry> = {
            let state = self.read_state()?;
            state.entries.iter().map(|te| te.entry.clone()).collect()
        };

        Ok(WorkingMemoryCheckpoint {
            entries,
            checkpoint_id: Uuid::new_v4().to_string(),
        })
    }

    fn rollback(&self, checkpoint: &WorkingMemoryCheckpoint) -> MemoryResult<()> {
        let mut state = self.write_state()?;
        state.entries.clear();
        for entry in &checkpoint.entries {
            state.entries.push_back(TimedEntry::new(entry.clone()));
        }
        drop(state);
        Ok(())
    }

    fn clear(&self) -> MemoryResult<()> {
        {
            let mut state = self.write_state()?;
            state.entries.clear();
        }
        Ok(())
    }

    fn len(&self) -> MemoryResult<usize> {
        // Need write lock for lazy purge to get accurate count
        let mut state = self.write_state()?;
        if let Some(ttl) = self.ttl_duration() {
            state.purge_expired(ttl);
        }
        Ok(state.entries.len())
    }
}

// ---------------------------------------------------------------------------
// Session-scoped registry
// ---------------------------------------------------------------------------

/// A registry that maintains one `InMemoryWorkingMemory` per `SessionId`.
///
/// This provides the session-level isolation described in the Beta memory
/// architecture: each session gets its own bounded ring-buffer, and sessions
/// that have been explicitly evicted (or that have exceeded the max-sessions
/// limit) are cleaned up automatically.
#[derive(Debug, Clone)]
pub struct WorkingMemoryRegistry {
    sessions: Arc<RwLock<HashMap<String, Arc<InMemoryWorkingMemory>>>>,
    config: WorkingMemoryConfig,
    /// Maximum number of concurrent sessions retained.
    max_sessions: usize,
}

impl WorkingMemoryRegistry {
    /// Create a new registry with given per-session config and session cap.
    pub fn new(config: WorkingMemoryConfig, max_sessions: usize) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            config,
            max_sessions,
        }
    }

    /// Retrieve or lazily create the working memory for a session.
    pub fn session(&self, session_id: &SessionId) -> MemoryResult<Arc<InMemoryWorkingMemory>> {
        let key = session_id.as_str().to_owned();

        // Fast path: session already exists
        {
            let sessions = self
                .sessions
                .read()
                .map_err(|e| MemoryError::LockPoisoned(format!("registry read lock: {e}")))?;
            if let Some(wm) = sessions.get(&key) {
                return Ok(Arc::clone(wm));
            }
        }

        // Slow path: create session, enforce max_sessions
        let mut sessions = self
            .sessions
            .write()
            .map_err(|e| MemoryError::LockPoisoned(format!("registry write lock: {e}")))?;

        // Double-checked locking
        if let Some(wm) = sessions.get(&key) {
            return Ok(Arc::clone(wm));
        }

        if sessions.len() >= self.max_sessions {
            return Err(MemoryError::CapacityExceeded {
                max: self.max_sessions,
                current: sessions.len(),
            });
        }

        let wm = Arc::new(InMemoryWorkingMemory::new(self.config.clone()));
        sessions.insert(key, Arc::clone(&wm));
        drop(sessions);
        Ok(wm)
    }

    /// Explicitly evict (drop) a session's working memory.
    pub fn evict_session(&self, session_id: &SessionId) -> MemoryResult<bool> {
        let mut sessions = self
            .sessions
            .write()
            .map_err(|e| MemoryError::LockPoisoned(format!("registry write lock: {e}")))?;
        Ok(sessions.remove(session_id.as_str()).is_some())
    }

    /// Return the current number of active sessions.
    pub fn session_count(&self) -> MemoryResult<usize> {
        let sessions = self
            .sessions
            .read()
            .map_err(|e| MemoryError::LockPoisoned(format!("registry read lock: {e}")))?;
        Ok(sessions.len())
    }
}

// ---------------------------------------------------------------------------
// Legacy stub kept for backward-compat with mod.rs exports
// ---------------------------------------------------------------------------

/// Lightweight wrapper re-exported as `WorkingMemory` in `mod.rs`.
///
/// Wraps an `InMemoryWorkingMemory` to provide the same surface as the old
/// stub while giving the full feature set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingMemoryManager {
    capacity: usize,
    #[serde(skip)]
    inner: Option<Arc<InMemoryWorkingMemory>>,
}

impl WorkingMemoryManager {
    /// Create a manager backed by an `InMemoryWorkingMemory` with the given capacity.
    pub fn new(capacity: usize) -> Self {
        let config = WorkingMemoryConfig {
            capacity,
            ttl_seconds: None,
        };
        Self {
            capacity,
            inner: Some(Arc::new(InMemoryWorkingMemory::new(config))),
        }
    }

    fn memory(&self) -> MemoryResult<&Arc<InMemoryWorkingMemory>> {
        self.inner
            .as_ref()
            .ok_or_else(|| MemoryError::OperationFailed("manager not initialised".into()))
    }

    /// Push a new entry.
    pub fn push(&self, entry: WorkingMemoryEntry) -> MemoryResult<()> {
        self.memory()?.push(entry)
    }

    /// List the `limit` most-recent entries.
    pub fn list_recent(&self, limit: usize) -> MemoryResult<Vec<WorkingMemoryEntry>> {
        self.memory()?.list_recent(limit)
    }

    /// Create a checkpoint.
    pub fn checkpoint(&self) -> MemoryResult<WorkingMemoryCheckpoint> {
        self.memory()?.checkpoint()
    }

    /// Rollback to a checkpoint.
    pub fn rollback(&self, cp: &WorkingMemoryCheckpoint) -> MemoryResult<()> {
        self.memory()?.rollback(cp)
    }

    /// Clear all entries.
    pub fn clear(&self) -> MemoryResult<()> {
        self.memory()?.clear()
    }

    /// Return current entry count.
    pub fn len(&self) -> MemoryResult<usize> {
        self.memory()?.len()
    }

    /// Returns `true` when empty.
    pub fn is_empty(&self) -> MemoryResult<bool> {
        Ok(self.len()? == 0)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{ExecutionId, TraceId};
    use crate::memory_context::{WorkingEntryKind, WorkingMemoryEntry};

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

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

    fn make_entry_with_exec(summary: &str, exec: &str) -> WorkingMemoryEntry {
        WorkingMemoryEntry {
            execution_id: Some(ExecutionId::from_string(exec.to_string()).unwrap()),
            kind: WorkingEntryKind::Goal,
            summary: summary.to_string(),
            artifact_refs: vec![],
            trace_id: Some(TraceId::from_string("trace-001".to_string()).unwrap()),
            encrypted: false,
        }
    }

    fn default_wm() -> InMemoryWorkingMemory {
        InMemoryWorkingMemory::new(WorkingMemoryConfig {
            capacity: 10,
            ttl_seconds: None,
        })
    }

    // -----------------------------------------------------------------------
    // WorkingMemoryConfig
    // -----------------------------------------------------------------------

    #[test]
    fn test_config_default_values() {
        let cfg = WorkingMemoryConfig::default();
        assert_eq!(cfg.capacity, 200);
        assert_eq!(cfg.ttl_seconds, Some(3600));
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let cfg = WorkingMemoryConfig {
            capacity: 50,
            ttl_seconds: Some(600),
        };
        let json = serde_json::to_string(&cfg).expect("serialize config");
        let back: WorkingMemoryConfig = serde_json::from_str(&json).expect("deserialize config");
        assert_eq!(back.capacity, 50);
        assert_eq!(back.ttl_seconds, Some(600));
    }

    #[test]
    fn test_config_no_ttl_serde_roundtrip() {
        let cfg = WorkingMemoryConfig {
            capacity: 100,
            ttl_seconds: None,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: WorkingMemoryConfig = serde_json::from_str(&json).unwrap();
        assert!(back.ttl_seconds.is_none());
    }

    // -----------------------------------------------------------------------
    // push + list_recent
    // -----------------------------------------------------------------------

    #[test]
    fn test_push_and_list_recent_basic() {
        let wm = default_wm();
        wm.push(make_entry("entry-1")).unwrap();
        wm.push(make_entry("entry-2")).unwrap();
        wm.push(make_entry("entry-3")).unwrap();

        let recent = wm.list_recent(5).unwrap();
        assert_eq!(recent.len(), 3);
        // list_recent returns in insertion order
        assert_eq!(recent[0].summary, "entry-1");
        assert_eq!(recent[2].summary, "entry-3");
    }

    #[test]
    fn test_list_recent_respects_limit() {
        let wm = default_wm();
        for i in 0..8 {
            wm.push(make_entry(&format!("entry-{i}"))).unwrap();
        }
        let recent = wm.list_recent(3).unwrap();
        assert_eq!(recent.len(), 3);
        // Most recent 3 are entry-5, entry-6, entry-7
        assert_eq!(recent[0].summary, "entry-5");
        assert_eq!(recent[2].summary, "entry-7");
    }

    #[test]
    fn test_list_recent_empty_returns_empty() {
        let wm = default_wm();
        let recent = wm.list_recent(10).unwrap();
        assert!(recent.is_empty());
    }

    #[test]
    fn test_list_recent_limit_zero_returns_empty() {
        let wm = default_wm();
        wm.push(make_entry("x")).unwrap();
        let recent = wm.list_recent(0).unwrap();
        assert!(recent.is_empty());
    }

    // -----------------------------------------------------------------------
    // LRU eviction
    // -----------------------------------------------------------------------

    #[test]
    fn test_lru_eviction_at_capacity() {
        let wm = InMemoryWorkingMemory::with_capacity(3);
        wm.push(make_entry("first")).unwrap();
        wm.push(make_entry("second")).unwrap();
        wm.push(make_entry("third")).unwrap();
        // Pushing a 4th entry should evict "first"
        wm.push(make_entry("fourth")).unwrap();

        let entries = wm.list_recent(10).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].summary, "second");
        assert_eq!(entries[2].summary, "fourth");
    }

    #[test]
    fn test_lru_eviction_exact_boundary() {
        let wm = InMemoryWorkingMemory::with_capacity(1);
        wm.push(make_entry("old")).unwrap();
        wm.push(make_entry("new")).unwrap();

        let entries = wm.list_recent(5).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].summary, "new");
    }

    #[test]
    fn test_len_reflects_current_count() {
        let wm = default_wm();
        assert_eq!(wm.len().unwrap(), 0);
        wm.push(make_entry("a")).unwrap();
        assert_eq!(wm.len().unwrap(), 1);
        wm.push(make_entry("b")).unwrap();
        assert_eq!(wm.len().unwrap(), 2);
    }

    #[test]
    fn test_is_empty_true_on_new_instance() {
        let wm = default_wm();
        assert!(wm.is_empty().unwrap());
    }

    #[test]
    fn test_is_empty_false_after_push() {
        let wm = default_wm();
        wm.push(make_entry("x")).unwrap();
        assert!(!wm.is_empty().unwrap());
    }

    // -----------------------------------------------------------------------
    // TTL expiry
    // -----------------------------------------------------------------------

    #[test]
    fn test_ttl_non_expired_entries_kept() {
        // TTL of 1 hour – entries created now must not expire
        let wm = InMemoryWorkingMemory::new(WorkingMemoryConfig {
            capacity: 10,
            ttl_seconds: Some(3600),
        });
        wm.push(make_entry("fresh")).unwrap();
        let entries = wm.list_recent(10).unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_ttl_expired_entries_purged_on_list() {
        // Insert a pre-aged entry by manipulating the state directly
        let wm = InMemoryWorkingMemory::new(WorkingMemoryConfig {
            capacity: 10,
            ttl_seconds: Some(1), // 1 second TTL
        });

        // Manually insert an entry with a timestamp in the past
        {
            let mut state = wm.state.write().unwrap();
            state.entries.push_back(TimedEntry {
                entry: make_entry("expired"),
                // 2 seconds ago = already expired
                inserted_at: Utc::now() - Duration::seconds(2),
            });
        }

        let entries = wm.list_recent(10).unwrap();
        assert!(entries.is_empty(), "Expired entry must be purged on list");
    }

    #[test]
    fn test_ttl_len_excludes_expired() {
        let wm = InMemoryWorkingMemory::new(WorkingMemoryConfig {
            capacity: 10,
            ttl_seconds: Some(1),
        });

        {
            let mut state = wm.state.write().unwrap();
            state.entries.push_back(TimedEntry {
                entry: make_entry("expired"),
                inserted_at: Utc::now() - Duration::seconds(5),
            });
            state.entries.push_back(TimedEntry {
                entry: make_entry("alive"),
                inserted_at: Utc::now(),
            });
        }

        assert_eq!(
            wm.len().unwrap(),
            1,
            "Only the live entry should be counted"
        );
    }

    #[test]
    fn test_no_ttl_entries_never_expire() {
        let wm = InMemoryWorkingMemory::new(WorkingMemoryConfig {
            capacity: 10,
            ttl_seconds: None,
        });
        // Manually insert an old entry
        {
            let mut state = wm.state.write().unwrap();
            state.entries.push_back(TimedEntry {
                entry: make_entry("ancient"),
                inserted_at: Utc::now() - Duration::days(365),
            });
        }
        // No TTL → should still be present
        assert_eq!(wm.len().unwrap(), 1);
        let entries = wm.list_recent(5).unwrap();
        assert_eq!(entries.len(), 1);
    }

    // -----------------------------------------------------------------------
    // clear
    // -----------------------------------------------------------------------

    #[test]
    fn test_clear_removes_all_entries() {
        let wm = default_wm();
        for i in 0..5 {
            wm.push(make_entry(&format!("entry-{i}"))).unwrap();
        }
        assert_eq!(wm.len().unwrap(), 5);
        wm.clear().unwrap();
        assert_eq!(wm.len().unwrap(), 0);
        assert!(wm.list_recent(10).unwrap().is_empty());
    }

    // -----------------------------------------------------------------------
    // checkpoint + rollback
    // -----------------------------------------------------------------------

    #[test]
    fn test_checkpoint_captures_current_state() {
        let wm = default_wm();
        wm.push(make_entry("a")).unwrap();
        wm.push(make_entry("b")).unwrap();

        let cp = wm.checkpoint().unwrap();
        assert_eq!(cp.entries.len(), 2);
        assert!(!cp.checkpoint_id.is_empty());
        assert_eq!(cp.entries[0].summary, "a");
        assert_eq!(cp.entries[1].summary, "b");
    }

    #[test]
    fn test_rollback_restores_state() {
        let wm = default_wm();
        wm.push(make_entry("before-1")).unwrap();
        wm.push(make_entry("before-2")).unwrap();

        let cp = wm.checkpoint().unwrap();

        // Mutate state after checkpoint
        wm.push(make_entry("after-1")).unwrap();
        wm.clear().unwrap();
        wm.push(make_entry("different")).unwrap();

        // Rollback
        wm.rollback(&cp).unwrap();

        let entries = wm.list_recent(10).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].summary, "before-1");
        assert_eq!(entries[1].summary, "before-2");
    }

    #[test]
    fn test_checkpoint_ids_are_unique() {
        let wm = default_wm();
        wm.push(make_entry("x")).unwrap();
        let cp1 = wm.checkpoint().unwrap();
        let cp2 = wm.checkpoint().unwrap();
        assert_ne!(cp1.checkpoint_id, cp2.checkpoint_id);
    }

    #[test]
    fn test_rollback_to_empty_checkpoint() {
        let wm = default_wm();
        // Checkpoint while empty
        let cp = wm.checkpoint().unwrap();

        wm.push(make_entry("after")).unwrap();
        assert_eq!(wm.len().unwrap(), 1);

        wm.rollback(&cp).unwrap();
        assert_eq!(wm.len().unwrap(), 0);
    }

    #[test]
    fn test_checkpoint_serde_roundtrip() {
        let wm = default_wm();
        wm.push(make_entry_with_exec("exec-entry", "exec-123"))
            .unwrap();
        let cp = wm.checkpoint().unwrap();

        let json = serde_json::to_string(&cp).expect("checkpoint must serialize");
        let restored: WorkingMemoryCheckpoint =
            serde_json::from_str(&json).expect("checkpoint must deserialize");

        assert_eq!(restored.checkpoint_id, cp.checkpoint_id);
        assert_eq!(restored.entries.len(), 1);
        assert_eq!(restored.entries[0].summary, "exec-entry");
    }

    // -----------------------------------------------------------------------
    // Thread-safety: concurrent writes from multiple threads
    // -----------------------------------------------------------------------

    #[test]
    fn test_concurrent_push_thread_safety() {
        use std::sync::Arc;
        use std::thread;

        let wm = Arc::new(InMemoryWorkingMemory::new(WorkingMemoryConfig {
            capacity: 1000,
            ttl_seconds: None,
        }));

        let handles: Vec<_> = (0..8)
            .map(|i| {
                let wm_clone = Arc::clone(&wm);
                thread::spawn(move || {
                    for j in 0..20 {
                        wm_clone
                            .push(make_entry(&format!("thread-{i}-entry-{j}")))
                            .expect("push must succeed");
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().expect("thread must not panic");
        }

        // 8 threads × 20 entries = 160 entries (capacity=1000, no eviction)
        assert_eq!(wm.len().unwrap(), 160);
    }

    // -----------------------------------------------------------------------
    // WorkingMemoryRegistry
    // -----------------------------------------------------------------------

    fn make_session_id(s: &str) -> SessionId {
        SessionId::from_string(s.to_string()).unwrap()
    }

    #[test]
    fn test_registry_creates_session_on_demand() {
        let registry = WorkingMemoryRegistry::new(WorkingMemoryConfig::default(), 10);
        let sid = make_session_id("session-alpha");

        let wm = registry.session(&sid).unwrap();
        wm.push(make_entry("hello")).unwrap();
        assert_eq!(wm.len().unwrap(), 1);
    }

    #[test]
    fn test_registry_returns_same_session_instance() {
        let registry = WorkingMemoryRegistry::new(WorkingMemoryConfig::default(), 10);
        let sid = make_session_id("session-beta");

        let wm1 = registry.session(&sid).unwrap();
        wm1.push(make_entry("first")).unwrap();

        let wm2 = registry.session(&sid).unwrap();
        // wm2 points to the same underlying storage
        assert_eq!(wm2.len().unwrap(), 1);
    }

    #[test]
    fn test_registry_session_isolation() {
        let registry = WorkingMemoryRegistry::new(WorkingMemoryConfig::default(), 10);
        let s1 = make_session_id("session-s1");
        let s2 = make_session_id("session-s2");

        let wm1 = registry.session(&s1).unwrap();
        let wm2 = registry.session(&s2).unwrap();

        wm1.push(make_entry("only in s1")).unwrap();
        assert_eq!(wm1.len().unwrap(), 1);
        assert_eq!(wm2.len().unwrap(), 0);
    }

    #[test]
    fn test_registry_evict_session() {
        let registry = WorkingMemoryRegistry::new(WorkingMemoryConfig::default(), 10);
        let sid = make_session_id("session-to-evict");

        registry.session(&sid).unwrap();
        assert_eq!(registry.session_count().unwrap(), 1);

        let evicted = registry.evict_session(&sid).unwrap();
        assert!(evicted);
        assert_eq!(registry.session_count().unwrap(), 0);
    }

    #[test]
    fn test_registry_evict_nonexistent_returns_false() {
        let registry = WorkingMemoryRegistry::new(WorkingMemoryConfig::default(), 10);
        let sid = make_session_id("ghost-session");
        let evicted = registry.evict_session(&sid).unwrap();
        assert!(!evicted);
    }

    #[test]
    fn test_registry_max_sessions_enforced() {
        let registry = WorkingMemoryRegistry::new(WorkingMemoryConfig::default(), 2);
        registry.session(&make_session_id("session-s1")).unwrap();
        registry.session(&make_session_id("session-s2")).unwrap();

        // Third session must fail
        let result = registry.session(&make_session_id("session-s3"));
        assert!(
            matches!(
                result,
                Err(MemoryError::CapacityExceeded { max: 2, current: 2 })
            ),
            "expected CapacityExceeded, got {:?}",
            result
        );
    }

    #[test]
    fn test_registry_session_count() {
        let registry = WorkingMemoryRegistry::new(WorkingMemoryConfig::default(), 10);
        assert_eq!(registry.session_count().unwrap(), 0);

        registry.session(&make_session_id("s1")).unwrap();
        assert_eq!(registry.session_count().unwrap(), 1);

        registry.session(&make_session_id("s2")).unwrap();
        assert_eq!(registry.session_count().unwrap(), 2);
    }

    // -----------------------------------------------------------------------
    // WorkingMemoryManager (legacy compatibility layer)
    // -----------------------------------------------------------------------

    #[test]
    fn test_manager_push_and_list() {
        let mgr = WorkingMemoryManager::new(50);
        mgr.push(make_entry("m1")).unwrap();
        mgr.push(make_entry("m2")).unwrap();

        let recent = mgr.list_recent(5).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].summary, "m1");
        assert_eq!(recent[1].summary, "m2");
    }

    #[test]
    fn test_manager_clear() {
        let mgr = WorkingMemoryManager::new(50);
        mgr.push(make_entry("entry")).unwrap();
        mgr.clear().unwrap();
        assert!(mgr.is_empty().unwrap());
    }

    #[test]
    fn test_manager_checkpoint_rollback() {
        let mgr = WorkingMemoryManager::new(50);
        mgr.push(make_entry("before")).unwrap();
        let cp = mgr.checkpoint().unwrap();

        mgr.push(make_entry("after")).unwrap();
        mgr.rollback(&cp).unwrap();

        let entries = mgr.list_recent(10).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].summary, "before");
    }

    #[test]
    fn test_manager_len_and_is_empty() {
        let mgr = WorkingMemoryManager::new(5);
        assert!(mgr.is_empty().unwrap());
        mgr.push(make_entry("x")).unwrap();
        assert_eq!(mgr.len().unwrap(), 1);
        assert!(!mgr.is_empty().unwrap());
    }

    #[test]
    fn test_manager_serde_roundtrip() {
        let mgr = WorkingMemoryManager::new(100);
        let json = serde_json::to_string(&mgr).expect("manager must serialize");
        let restored: WorkingMemoryManager =
            serde_json::from_str(&json).expect("manager must deserialize");
        assert_eq!(restored.capacity, 100);
        // inner is skipped in serde; operations on restored manager will still work
        // because inner is None → any method call returns OperationFailed
        let result = restored.push(make_entry("x"));
        assert!(result.is_err());
    }
}
