use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

/// Maximum safe version before overflow concerns
/// When version exceeds this threshold, operations will be rejected
const MAX_SAFE_VERSION: u64 = u64::MAX - 1000;

/// Version warning threshold
/// When version exceeds this threshold, a warning should be logged
const VERSION_WARNING_THRESHOLD: u64 = u64::MAX - 10000;

/// CAS retry configuration
const CAS_DEFAULT_MAX_RETRIES: usize = 10;
const CAS_BASE_BACKOFF_MS: u64 = 10;
const CAS_MAX_TOTAL_TIMEOUT_MS: u64 = 30000; // Increased from 5000ms to support high-contention scenarios

/// CAS configuration for different scenarios
#[derive(Debug, Clone)]
pub struct CasConfig {
    pub max_retries: usize,
    pub base_backoff_ms: u64,
    pub max_total_timeout_ms: u64,
}

impl CasConfig {
    /// Autopilot 专用配置（长时运行场景）
    pub fn for_autopilot() -> Self {
        Self {
            max_retries: 50, // 增加重试次数以应对高并发场景
            base_backoff_ms: 50,
            max_total_timeout_ms: 120000, // 2 分钟
        }
    }

    /// 默认配置（短时操作）
    pub fn default_config() -> Self {
        Self {
            max_retries: CAS_DEFAULT_MAX_RETRIES,
            base_backoff_ms: CAS_BASE_BACKOFF_MS,
            max_total_timeout_ms: CAS_MAX_TOTAL_TIMEOUT_MS,
        }
    }
}

impl Default for CasConfig {
    fn default() -> Self {
        Self::default_config()
    }
}

/// Metrics for CAS operations (Task 7.4)
#[derive(Debug, Default)]
pub struct CasMetrics {
    /// Total number of CAS retry attempts
    pub cas_retry_count: AtomicU64,
    /// Total number of CAS conflict events
    pub cas_conflict_count: AtomicU64,
    /// Total number of successful CAS operations
    pub cas_success_count: AtomicU64,
    /// Total number of failed CAS operations (exhausted retries)
    pub cas_failure_count: AtomicU64,
    /// Total lock wait time in microseconds (histogram approximation via total)
    pub lock_wait_us_total: AtomicU64,
}

impl CasMetrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn record_retry(&self) {
        self.cas_retry_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_conflict(&self) {
        self.cas_conflict_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_success(&self) {
        self.cas_success_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_failure(&self) {
        self.cas_failure_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_lock_wait_us(&self, us: u64) {
        self.lock_wait_us_total.fetch_add(us, Ordering::Relaxed);
    }

    /// Conflict rate: conflicts / (conflicts + successes), returns 0.0 if no operations
    pub fn conflict_rate(&self) -> f64 {
        let conflicts = self.cas_conflict_count.load(Ordering::Relaxed);
        let successes = self.cas_success_count.load(Ordering::Relaxed);
        let total = conflicts + successes;
        if total == 0 {
            0.0
        } else {
            // u64 → f64 精度损失是可接受的 (用于统计指标计算)
            #[allow(clippy::cast_precision_loss)]
            {
                conflicts as f64 / total as f64
            }
        }
    }

    pub fn retry_count(&self) -> u64 {
        self.cas_retry_count.load(Ordering::Relaxed)
    }

    pub fn conflict_count(&self) -> u64 {
        self.cas_conflict_count.load(Ordering::Relaxed)
    }

    pub fn success_count(&self) -> u64 {
        self.cas_success_count.load(Ordering::Relaxed)
    }

    pub fn failure_count(&self) -> u64 {
        self.cas_failure_count.load(Ordering::Relaxed)
    }

    pub fn lock_wait_us_total(&self) -> u64 {
        self.lock_wait_us_total.load(Ordering::Relaxed)
    }
}

/// Versioned state entry for CAS operations (Milestone C)
#[derive(Debug, Clone)]
pub struct StateEntry {
    pub key: String,
    pub value: Vec<u8>,
    pub version: u64,
    pub updated_at: DateTime<Utc>,
}

/// Shared state store trait with CAS support (Milestone C)
#[async_trait::async_trait]
pub trait SharedStateStore: Send + Sync {
    /// Get a state entry by key
    async fn get(&self, key: &str) -> anyhow::Result<Option<StateEntry>>;

    /// Put a state entry with a specific version (for internal use)
    async fn put(&self, key: String, value: Vec<u8>, version: u64) -> anyhow::Result<()>;

    /// Compare-and-Swap: Update value only if current version matches expected
    ///
    /// Returns the new version on success
    /// Returns error if version mismatch (optimistic lock failure)
    async fn cas(&self, key: String, expected_version: u64, value: Vec<u8>) -> anyhow::Result<u64>;

    /// Compare-and-Swap with custom configuration
    ///
    /// This is an async version that allows retry logic with custom CasConfig.
    /// Default implementation uses standard cas() without retry - implementors
    /// should override for advanced retry semantics.
    async fn cas_with_config(
        &self,
        key: String,
        expected_version: u64,
        value: Vec<u8>,
        config: &CasConfig,
    ) -> anyhow::Result<u64> {
        // Default implementation: attempt once using standard cas()
        let _ = config; // Suppress unused warning in default impl
        self.cas(key, expected_version, value).await
    }

    /// List all keys (for debugging/monitoring)
    async fn list_keys(&self) -> anyhow::Result<Vec<String>>;

    /// Delete a key (for cleanup)
    async fn delete(&self, key: &str) -> anyhow::Result<()>;

    // ============ Agent 6: Autopilot-specific methods ============

    /// Get state change history for a key (for no-progress detection)
    ///
    /// Returns up to `limit` historical StateEntry records in reverse chronological order.
    /// Default implementation returns empty vector - implementors should override to support history tracking.
    async fn get_state_history(
        &self,
        _key: &str,
        _limit: usize,
    ) -> anyhow::Result<Vec<StateEntry>> {
        Ok(Vec::new()) // Default: no history tracking
    }

    /// Batch query with key prefix (for retrieving all iterations of a run)
    ///
    /// Returns all StateEntry records whose keys start with `prefix`.
    /// Default implementation returns empty vector - implementors should override for prefix search.
    async fn get_with_prefix(&self, _prefix: &str) -> anyhow::Result<Vec<StateEntry>> {
        Ok(Vec::new()) // Default: no prefix query
    }

    /// Set a key with TTL (for iteration intermediate results)
    ///
    /// The entry will be automatically removed after `ttl_secs` seconds.
    /// Default implementation ignores TTL and uses standard put() - implementors should override for TTL support.
    async fn ttl_set(&self, key: String, value: Vec<u8>, _ttl_secs: u64) -> anyhow::Result<()> {
        self.put(key, value, 0).await // Default: no TTL, start at version 0
    }

    /// Watch for state changes on a key (for real-time awareness)
    ///
    /// Returns a receiver that will be notified when the key's value changes.
    /// Default implementation returns error - implementors should override for watch support.
    async fn watch(
        &self,
        _key: &str,
    ) -> anyhow::Result<tokio::sync::watch::Receiver<Option<StateEntry>>> {
        anyhow::bail!("Watch not supported by this implementation")
    }
}

/// In-memory implementation of SharedStateStore
#[derive(Clone)]
pub struct InMemorySharedStateStore {
    store: Arc<RwLock<BTreeMap<String, StateEntry>>>,
    metrics: Arc<CasMetrics>,
    // Agent 6: History tracking (key -> Vec<StateEntry> in reverse chronological order)
    version_history: Arc<RwLock<BTreeMap<String, Vec<StateEntry>>>>,
    // Agent 6: Watch channels (key -> sender)
    watchers: Arc<RwLock<BTreeMap<String, tokio::sync::watch::Sender<Option<StateEntry>>>>>,
}

impl InMemorySharedStateStore {
    pub fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(BTreeMap::new())),
            metrics: CasMetrics::new(),
            version_history: Arc::new(RwLock::new(BTreeMap::new())),
            watchers: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    /// Get a reference to the metrics for this store
    pub fn metrics(&self) -> Arc<CasMetrics> {
        self.metrics.clone()
    }

    // ============ Agent 6: Helper methods for history and watch ============

    /// Record a state entry in version history (async context)
    async fn record_history_async(&self, entry: &StateEntry) {
        let mut history = self.version_history.write().await;
        history
            .entry(entry.key.clone())
            .or_insert_with(Vec::new)
            .push(entry.clone());
    }

    /// Notify all watchers of a state change (async context)
    async fn notify_watchers_async(&self, entry: &StateEntry) {
        let watchers = self.watchers.read().await;
        if let Some(tx) = watchers.get(&entry.key) {
            let _ = tx.send(Some(entry.clone()));
        }
    }

    /// CAS with exponential backoff retry using CasConfig
    ///
    /// Retries the read-modify-write cycle up to `config.max_retries` times with
    /// exponential backoff (base * 2^attempt) capped at `config.max_total_timeout_ms`.
    ///
    /// The `update_fn` receives the current value (or None for new keys) and
    /// returns the new JSON value to store.
    ///
    /// # Deadlock Prevention
    /// - Maximum retry limit (`config.max_retries`) prevents infinite loops
    /// - Total timeout (`config.max_total_timeout_ms`) provides a hard upper bound
    /// - Lock is never held across `await` points (released before sleep)
    pub async fn cas_with_retry_config<F>(
        &self,
        key: &str,
        update_fn: F,
        config: &CasConfig,
    ) -> anyhow::Result<()>
    where
        F: Fn(Option<serde_json::Value>) -> anyhow::Result<serde_json::Value>,
    {
        let start = std::time::Instant::now();
        let max_total = std::time::Duration::from_millis(config.max_total_timeout_ms);

        let mut attempt = 0usize;

        loop {
            // Hard timeout guard (deadlock prevention)
            if start.elapsed() >= max_total {
                self.metrics.record_failure();
                anyhow::bail!(
                    "cas_with_retry timed out after {}ms for key '{}'",
                    config.max_total_timeout_ms,
                    key
                );
            }

            // --- READ phase (shared lock, released before await) ---
            let lock_start = std::time::Instant::now();
            let (current_version, current_value) = {
                let store = self.store.read().await;
                self.metrics.record_lock_wait_us(
                    lock_start.elapsed().as_micros().min(u64::MAX as u128) as u64,
                );

                if let Some(entry) = store.get(key) {
                    let json_val: serde_json::Value =
                        serde_json::from_slice(&entry.value).unwrap_or(serde_json::Value::Null);
                    (entry.version, Some(json_val))
                } else {
                    (0u64, None)
                }
            }; // read lock dropped here

            // Compute new value (outside of any lock)
            let new_value = update_fn(current_value)?;
            let new_bytes = serde_json::to_vec(&new_value)?;

            // --- WRITE phase (exclusive lock) ---
            let lock_start = std::time::Instant::now();
            let cas_result = {
                let mut store = self.store.write().await;
                self.metrics.record_lock_wait_us(
                    lock_start.elapsed().as_micros().min(u64::MAX as u128) as u64,
                );

                // Re-check version under write lock (TOCTOU prevention)
                let actual_version = store.get(key).map(|e| e.version).unwrap_or(0);

                if actual_version != current_version {
                    // Version changed between read and write — CAS conflict
                    Err(actual_version)
                } else {
                    // Version overflow protection
                    if current_version >= MAX_SAFE_VERSION {
                        return Err(anyhow::anyhow!(
                            "Version overflow prevention: key '{}' version {} exceeds maximum safe version {}",
                            key, current_version, MAX_SAFE_VERSION
                        ));
                    }

                    let new_version = current_version.checked_add(1).ok_or_else(|| {
                        anyhow::anyhow!(
                            "Version overflow: cannot increment version {} for key '{}'",
                            current_version,
                            key
                        )
                    })?;

                    if new_version >= VERSION_WARNING_THRESHOLD {
                        tracing::warn!(
                            key = key,
                            version = new_version,
                            max = u64::MAX,
                            "Key version is approaching maximum, consider resetting"
                        );
                    }

                    let entry = StateEntry {
                        key: key.to_string(),
                        value: new_bytes,
                        version: new_version,
                        updated_at: Utc::now(),
                    };
                    store.insert(key.to_string(), entry);
                    Ok(new_version)
                }
            }; // write lock dropped here

            match cas_result {
                Ok(_new_version) => {
                    self.metrics.record_success();
                    return Ok(());
                }
                Err(_actual_version) => {
                    self.metrics.record_conflict();

                    attempt += 1;
                    if attempt > config.max_retries {
                        self.metrics.record_failure();
                        anyhow::bail!(
                            "cas_with_retry exhausted {} retries for key '{}'",
                            config.max_retries,
                            key
                        );
                    }

                    self.metrics.record_retry();

                    // Linear backoff with jitter to reduce contention in high-concurrency scenarios
                    // Formula: base_backoff_ms * attempt + random(0, base_backoff_ms)
                    // This prevents thundering herd while keeping total wait time bounded
                    let linear_backoff = config.base_backoff_ms.saturating_mul(attempt as u64);
                    let jitter = rand::random::<u64>() % config.base_backoff_ms.max(1);
                    let backoff_ms = linear_backoff.saturating_add(jitter);
                    tokio::time::sleep(tokio::time::Duration::from_millis(backoff_ms)).await;
                }
            }
        }
    }

    /// CAS with exponential backoff retry (legacy interface, for backward compatibility)
    ///
    /// This method delegates to cas_with_retry_config with a custom config built from max_retries
    /// and default backoff/timeout settings.
    pub async fn cas_with_retry<F>(
        &self,
        key: &str,
        update_fn: F,
        max_retries: usize,
    ) -> anyhow::Result<()>
    where
        F: Fn(Option<serde_json::Value>) -> anyhow::Result<serde_json::Value>,
    {
        let config = CasConfig {
            max_retries,
            base_backoff_ms: CAS_BASE_BACKOFF_MS,
            max_total_timeout_ms: CAS_MAX_TOTAL_TIMEOUT_MS,
        };
        self.cas_with_retry_config(key, update_fn, &config).await
    }

    /// cas_with_retry using default max retries
    pub async fn cas_with_retry_default<F>(&self, key: &str, update_fn: F) -> anyhow::Result<()>
    where
        F: Fn(Option<serde_json::Value>) -> anyhow::Result<serde_json::Value>,
    {
        self.cas_with_retry(key, update_fn, CAS_DEFAULT_MAX_RETRIES)
            .await
    }
}

impl Default for InMemorySharedStateStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl SharedStateStore for InMemorySharedStateStore {
    async fn get(&self, key: &str) -> anyhow::Result<Option<StateEntry>> {
        let store = self.store.read().await;
        Ok(store.get(key).cloned())
    }

    async fn put(&self, key: String, value: Vec<u8>, version: u64) -> anyhow::Result<()> {
        let now = Utc::now();
        let entry = StateEntry {
            key: key.clone(),
            value,
            version,
            updated_at: now,
        };

        // Update main store
        let mut store = self.store.write().await;
        store.insert(key.clone(), entry.clone());
        drop(store); // Release lock before async operations

        // Agent 6: Record history and notify watchers
        self.record_history_async(&entry).await;
        self.notify_watchers_async(&entry).await;

        Ok(())
    }

    async fn cas(&self, key: String, expected_version: u64, value: Vec<u8>) -> anyhow::Result<u64> {
        let mut store = self.store.write().await;

        // Check current version
        let current_version = if let Some(entry) = store.get(&key) {
            entry.version
        } else {
            // Key doesn't exist, treat as version 0
            0
        };

        if current_version != expected_version {
            self.metrics.record_conflict();
            anyhow::bail!(
                "CAS failed for key '{}': expected version {}, found {}",
                key,
                expected_version,
                current_version
            );
        }

        // SECURITY: Check for version overflow
        if current_version >= MAX_SAFE_VERSION {
            anyhow::bail!(
                "Version overflow prevention: key '{}' version {} exceeds maximum safe version {}. \
                Consider resetting the key or using a different key.",
                key,
                current_version,
                MAX_SAFE_VERSION
            );
        }

        // Version matches, perform update with checked arithmetic
        let new_version = current_version.checked_add(1).ok_or_else(|| {
            anyhow::anyhow!(
                "Version overflow: cannot increment version {} for key '{}'",
                current_version,
                key
            )
        })?;

        // Log warning if approaching overflow (in production, this should use proper logging)
        if new_version >= VERSION_WARNING_THRESHOLD {
            eprintln!(
                "WARNING: Key '{}' version {} is approaching maximum ({}). \
                Consider resetting before overflow occurs.",
                key,
                new_version,
                u64::MAX
            );
        }

        let now = Utc::now();
        let entry = StateEntry {
            key: key.clone(),
            value,
            version: new_version,
            updated_at: now,
        };

        store.insert(key.clone(), entry.clone());
        drop(store); // Release lock before async operations

        self.metrics.record_success();

        // Agent 6: Record history and notify watchers (async)
        let entry_clone = entry.clone();
        let store_clone = self.clone();
        tokio::spawn(async move {
            store_clone.record_history_async(&entry_clone).await;
            store_clone.notify_watchers_async(&entry_clone).await;
        });

        Ok(new_version)
    }

    async fn cas_with_config(
        &self,
        key: String,
        expected_version: u64,
        value: Vec<u8>,
        config: &CasConfig,
    ) -> anyhow::Result<u64> {
        // For InMemorySharedStateStore, we use a retry loop with exponential backoff
        let start = std::time::Instant::now();
        let max_total = std::time::Duration::from_millis(config.max_total_timeout_ms);

        let mut attempt = 0usize;

        loop {
            // Hard timeout guard
            if start.elapsed() >= max_total {
                self.metrics.record_failure();
                anyhow::bail!(
                    "cas_with_config timed out after {}ms for key '{}'",
                    config.max_total_timeout_ms,
                    key
                );
            }

            // Attempt CAS
            let result = self.cas(key.clone(), expected_version, value.clone()).await;

            match result {
                Ok(new_version) => return Ok(new_version),
                Err(e) => {
                    // Check if it's a version mismatch (retryable) or other error (non-retryable)
                    let error_msg = e.to_string();
                    if !error_msg.contains("CAS failed") {
                        // Non-retryable error (e.g., version overflow)
                        return Err(e);
                    }

                    attempt += 1;
                    if attempt > config.max_retries {
                        self.metrics.record_failure();
                        return Err(anyhow::anyhow!(
                            "cas_with_config exhausted {} retries for key '{}': {}",
                            config.max_retries,
                            key,
                            e
                        ));
                    }

                    self.metrics.record_retry();

                    // Re-read current version for next attempt
                    let current_entry = self.get(&key).await?;
                    let current_version = current_entry.as_ref().map(|e| e.version).unwrap_or(0);

                    // If expected_version hasn't changed, we're stuck - bail out early
                    if current_version == expected_version {
                        // This shouldn't happen but guards against infinite loops
                        return Err(anyhow::anyhow!(
                            "cas_with_config detected stuck state at version {} for key '{}'",
                            expected_version,
                            key
                        ));
                    }

                    // Linear backoff with jitter to reduce contention in high-concurrency scenarios
                    // Formula: base_backoff_ms * attempt + random(0, base_backoff_ms)
                    // This prevents thundering herd while keeping total wait time bounded
                    let linear_backoff = config.base_backoff_ms.saturating_mul(attempt as u64);
                    let jitter = rand::random::<u64>() % config.base_backoff_ms.max(1);
                    let backoff_ms = linear_backoff.saturating_add(jitter);
                    tokio::time::sleep(tokio::time::Duration::from_millis(backoff_ms)).await;
                }
            }
        }
    }

    async fn list_keys(&self) -> anyhow::Result<Vec<String>> {
        let store = self.store.read().await;
        Ok(store.keys().cloned().collect())
    }

    async fn delete(&self, key: &str) -> anyhow::Result<()> {
        let mut store = self.store.write().await;
        store.remove(key);
        Ok(())
    }

    // ============ Agent 6: Autopilot-specific method implementations ============

    async fn get_state_history(&self, key: &str, limit: usize) -> anyhow::Result<Vec<StateEntry>> {
        let history = self.version_history.read().await;

        Ok(history
            .get(key)
            .map(|entries| entries.iter().take(limit).cloned().collect())
            .unwrap_or_default())
    }

    async fn get_with_prefix(&self, prefix: &str) -> anyhow::Result<Vec<StateEntry>> {
        let store = self.store.read().await;

        Ok(store
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(_, v)| v.clone())
            .collect())
    }

    async fn ttl_set(&self, key: String, value: Vec<u8>, ttl_secs: u64) -> anyhow::Result<()> {
        // Insert the key normally
        self.put(key.clone(), value, 0).await?;

        // Spawn background task to delete after TTL
        let store_clone = self.store.clone();
        let key_clone = key.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(ttl_secs)).await;
            let mut store = store_clone.write().await;
            store.remove(&key_clone);
        });

        Ok(())
    }

    async fn watch(
        &self,
        key: &str,
    ) -> anyhow::Result<tokio::sync::watch::Receiver<Option<StateEntry>>> {
        let mut watchers = self.watchers.write().await;

        // Get or create watcher for this key
        let receiver = if let Some(tx) = watchers.get(key) {
            tx.subscribe()
        } else {
            // Create new watch channel
            let current_value = self.get(key).await?;
            let (tx, rx) = tokio::sync::watch::channel(current_value);
            watchers.insert(key.to_string(), tx);
            rx
        };

        Ok(receiver)
    }
}

// ---------------------------------------------------------------------------
// FIX 5: Generic SharedStateStore with versioned entries and atomic CAS ops
// ---------------------------------------------------------------------------

/// Error type for generic SharedStateStore operations.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("Key not found")]
    KeyNotFound,

    #[error("Version mismatch: expected {expected}, found {actual}")]
    VersionMismatch { expected: u64, actual: u64 },

    #[error("Update conflict: {0}")]
    UpdateConflict(String),
}

/// A versioned value wrapper that records who updated it and when.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionedEntry<V> {
    pub value: V,
    pub version: u64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub updated_by: String,
}

/// Generic shared-state store with optimistic-locking (CAS) semantics.
///
/// All mutation methods hold an exclusive write-lock for the duration of the
/// mutation, so operations are atomic even under concurrent access.
///
/// The optional `locks` map provides per-key `Mutex` fences for callers that
/// need to serialise high-contention updates outside of the RwLock.
pub struct GenericSharedStateStore<K, V> {
    data: Arc<RwLock<HashMap<K, VersionedEntry<V>>>>,
    /// Per-key mutexes for high-contention callers.
    locks: Arc<RwLock<HashMap<K, Arc<Mutex<()>>>>>,
}

impl<K, V> GenericSharedStateStore<K, V>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// Creates a new, empty store.
    pub fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
            locks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    // ------------------------------------------------------------------
    // Read helpers
    // ------------------------------------------------------------------

    /// Returns a clone of the versioned entry for `key`, or `None`.
    pub async fn get(&self, key: &K) -> Option<VersionedEntry<V>> {
        let store = self.data.read().await;
        store.get(key).cloned()
    }

    /// Returns the current version of `key`, or `None` if absent.
    pub async fn get_version(&self, key: &K) -> Option<u64> {
        let store = self.data.read().await;
        store.get(key).map(|e| e.version)
    }

    // ------------------------------------------------------------------
    // Write operations (atomic under exclusive write-lock)
    // ------------------------------------------------------------------

    /// Atomically updates a value using optimistic locking (compare-and-swap).
    ///
    /// Succeeds only when the stored version equals `expected_version`.
    /// Returns the new version on success.
    pub async fn update(
        &self,
        key: K,
        value: V,
        expected_version: u64,
        updated_by: String,
    ) -> Result<u64, StateError> {
        let mut store = self.data.write().await;

        // Read current version first (drops immutable borrow before insert).
        let current_version = store.get(&key).map(|entry| entry.version);

        match current_version {
            Some(v) if v == expected_version => {
                let new_version = expected_version + 1;
                store.insert(
                    key,
                    VersionedEntry {
                        value,
                        version: new_version,
                        updated_at: chrono::Utc::now(),
                        updated_by,
                    },
                );
                Ok(new_version)
            }
            Some(actual) => Err(StateError::VersionMismatch {
                expected: expected_version,
                actual,
            }),
            None => Err(StateError::KeyNotFound),
        }
    }

    /// Atomically inserts or updates a value (upsert).
    ///
    /// If the key already exists, the version is incremented.
    /// If the key is new, version starts at 1.
    pub async fn upsert(&self, key: K, value: V, updated_by: String) -> Result<u64, StateError> {
        let mut store = self.data.write().await;

        // Read existing version before taking mutable borrow for insert.
        let new_version = store.get(&key).map(|e| e.version + 1).unwrap_or(1);

        store.insert(
            key,
            VersionedEntry {
                value,
                version: new_version,
                updated_at: chrono::Utc::now(),
                updated_by,
            },
        );

        Ok(new_version)
    }

    /// Atomically transforms an existing value in-place.
    ///
    /// `transform` receives the current value and returns the replacement.
    /// Returns `StateError::KeyNotFound` if the key does not exist.
    pub async fn update_with<F>(
        &self,
        key: K,
        updated_by: String,
        transform: F,
    ) -> Result<u64, StateError>
    where
        F: FnOnce(&V) -> Result<V, StateError>,
    {
        let mut store = self.data.write().await;

        // Extract what we need before the mutable borrow for insert.
        let (current_value, current_version) = match store.get(&key) {
            Some(entry) => (entry.value.clone(), entry.version),
            None => return Err(StateError::KeyNotFound),
        };

        let new_value = transform(&current_value)?;
        let new_version = current_version + 1;

        store.insert(
            key,
            VersionedEntry {
                value: new_value,
                version: new_version,
                updated_at: chrono::Utc::now(),
                updated_by,
            },
        );

        Ok(new_version)
    }

    /// CAS update guarded by a per-key `Mutex` for high-contention scenarios.
    ///
    /// Acquires a per-key lock before delegating to [`Self::update`], ensuring
    /// that concurrent callers for the *same* key are fully serialised even
    /// outside of the store's internal `RwLock`.
    pub async fn update_with_lock(
        &self,
        key: K,
        value: V,
        expected_version: u64,
        updated_by: String,
    ) -> Result<u64, StateError> {
        // Get or create the per-key Mutex.
        let key_lock = {
            let mut locks = self.locks.write().await;
            locks
                .entry(key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };

        // Serialise access for this key.
        let _guard = key_lock.lock().await;

        self.update(key, value, expected_version, updated_by).await
    }
}

impl<K, V> Default for GenericSharedStateStore<K, V>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod generic_store_tests {
    use super::*;

    type Store = GenericSharedStateStore<String, String>;

    #[tokio::test]
    async fn test_upsert_new_key_starts_at_version_1() {
        let store = Store::new();
        let v = store
            .upsert("k".to_string(), "v1".to_string(), "alice".to_string())
            .await
            .unwrap();
        assert_eq!(v, 1);
        let entry = store.get(&"k".to_string()).await.unwrap();
        assert_eq!(entry.version, 1);
        assert_eq!(entry.value, "v1");
    }

    #[tokio::test]
    async fn test_upsert_increments_version() {
        let store = Store::new();
        store
            .upsert("k".to_string(), "v1".to_string(), "alice".to_string())
            .await
            .unwrap();
        let v2 = store
            .upsert("k".to_string(), "v2".to_string(), "bob".to_string())
            .await
            .unwrap();
        assert_eq!(v2, 2);
    }

    #[tokio::test]
    async fn test_update_success() {
        let store = Store::new();
        store
            .upsert("k".to_string(), "v1".to_string(), "alice".to_string())
            .await
            .unwrap();

        let new_v = store
            .update("k".to_string(), "v2".to_string(), 1, "bob".to_string())
            .await
            .unwrap();
        assert_eq!(new_v, 2);

        let entry = store.get(&"k".to_string()).await.unwrap();
        assert_eq!(entry.value, "v2");
        assert_eq!(entry.updated_by, "bob");
    }

    #[tokio::test]
    async fn test_update_version_mismatch() {
        let store = Store::new();
        store
            .upsert("k".to_string(), "v1".to_string(), "alice".to_string())
            .await
            .unwrap();

        let err = store
            .update("k".to_string(), "v2".to_string(), 0, "bob".to_string())
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            StateError::VersionMismatch {
                expected: 0,
                actual: 1
            }
        ));
    }

    #[tokio::test]
    async fn test_update_key_not_found() {
        let store = Store::new();
        let err = store
            .update(
                "missing".to_string(),
                "v".to_string(),
                0,
                "alice".to_string(),
            )
            .await
            .unwrap_err();

        assert!(matches!(err, StateError::KeyNotFound));
    }

    #[tokio::test]
    async fn test_update_with_transform() {
        let store = Store::new();
        store
            .upsert("k".to_string(), "hello".to_string(), "alice".to_string())
            .await
            .unwrap();

        let v = store
            .update_with("k".to_string(), "alice".to_string(), |current| {
                Ok(format!("{} world", current))
            })
            .await
            .unwrap();

        assert_eq!(v, 2);
        let entry = store.get(&"k".to_string()).await.unwrap();
        assert_eq!(entry.value, "hello world");
    }

    #[tokio::test]
    async fn test_update_with_key_not_found() {
        let store = Store::new();
        let err = store
            .update_with("missing".to_string(), "alice".to_string(), |_| {
                Ok("x".to_string())
            })
            .await
            .unwrap_err();
        assert!(matches!(err, StateError::KeyNotFound));
    }

    #[tokio::test]
    async fn test_get_version() {
        let store = Store::new();
        assert!(store.get_version(&"k".to_string()).await.is_none());

        store
            .upsert("k".to_string(), "v".to_string(), "alice".to_string())
            .await
            .unwrap();
        assert_eq!(store.get_version(&"k".to_string()).await, Some(1));
    }

    #[tokio::test]
    async fn test_update_with_lock_success() {
        let store = Store::new();
        store
            .upsert("k".to_string(), "v1".to_string(), "alice".to_string())
            .await
            .unwrap();

        let v = store
            .update_with_lock("k".to_string(), "v2".to_string(), 1, "bob".to_string())
            .await
            .unwrap();
        assert_eq!(v, 2);
    }

    #[tokio::test]
    async fn test_update_with_lock_version_mismatch() {
        let store = Store::new();
        store
            .upsert("k".to_string(), "v1".to_string(), "alice".to_string())
            .await
            .unwrap();

        let err = store
            .update_with_lock("k".to_string(), "v2".to_string(), 99, "bob".to_string())
            .await
            .unwrap_err();
        assert!(matches!(err, StateError::VersionMismatch { .. }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_cas_new_key() {
        let store = InMemorySharedStateStore::new();
        let key = "test-key".to_string();
        let value = b"test-value".to_vec();

        // CAS with version 0 should succeed for new key
        let new_version = store.cas(key.clone(), 0, value.clone()).await.unwrap();
        assert_eq!(new_version, 1);

        // Verify value stored
        let entry = store.get(&key).await.unwrap().unwrap();
        assert_eq!(entry.value, value);
        assert_eq!(entry.version, 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_cas_update_success() {
        let store = InMemorySharedStateStore::new();
        let key = "test-key".to_string();

        // Initial insert
        store.cas(key.clone(), 0, b"v1".to_vec()).await.unwrap();

        // Update with correct version
        let new_version = store.cas(key.clone(), 1, b"v2".to_vec()).await.unwrap();
        assert_eq!(new_version, 2);

        // Verify updated value
        let entry = store.get(&key).await.unwrap().unwrap();
        assert_eq!(entry.value, b"v2".to_vec());
        assert_eq!(entry.version, 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_cas_version_mismatch() {
        let store = InMemorySharedStateStore::new();
        let key = "test-key".to_string();

        // Initial insert
        store.cas(key.clone(), 0, b"v1".to_vec()).await.unwrap();

        // Attempt update with wrong version
        let result = store.cas(key.clone(), 0, b"v2".to_vec()).await;
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("CAS failed"));
        assert!(error_msg.contains("expected version 0, found 1"));

        // Verify original value unchanged
        let entry = store.get(&key).await.unwrap().unwrap();
        assert_eq!(entry.value, b"v1".to_vec());
        assert_eq!(entry.version, 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_cas_concurrent_updates() {
        let store = InMemorySharedStateStore::new();
        let key = "counter".to_string();

        // Initial value
        store.cas(key.clone(), 0, b"0".to_vec()).await.unwrap();

        // Simulate concurrent update attempt
        let entry1 = store.get(&key).await.unwrap().unwrap();
        let entry2 = store.get(&key).await.unwrap().unwrap();

        // First update succeeds
        let result1 = store.cas(key.clone(), entry1.version, b"1".to_vec()).await;
        assert!(result1.is_ok());
        assert_eq!(result1.unwrap(), 2);

        // Second update fails (stale version)
        let result2 = store.cas(key.clone(), entry2.version, b"2".to_vec()).await;
        assert!(result2.is_err());

        // Retry with fresh version
        let entry3 = store.get(&key).await.unwrap().unwrap();
        let result3 = store.cas(key.clone(), entry3.version, b"2".to_vec()).await;
        assert!(result3.is_ok());
        assert_eq!(result3.unwrap(), 3);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_list_keys() {
        let store = InMemorySharedStateStore::new();

        store
            .cas("key1".to_string(), 0, b"v1".to_vec())
            .await
            .unwrap();
        store
            .cas("key2".to_string(), 0, b"v2".to_vec())
            .await
            .unwrap();
        store
            .cas("key3".to_string(), 0, b"v3".to_vec())
            .await
            .unwrap();

        let keys = store.list_keys().await.unwrap();
        assert_eq!(keys.len(), 3);
        assert!(keys.contains(&"key1".to_string()));
        assert!(keys.contains(&"key2".to_string()));
        assert!(keys.contains(&"key3".to_string()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_delete() {
        let store = InMemorySharedStateStore::new();
        let key = "test-key".to_string();

        store.cas(key.clone(), 0, b"value".to_vec()).await.unwrap();
        assert!(store.get(&key).await.unwrap().is_some());

        store.delete(&key).await.unwrap();
        assert!(store.get(&key).await.unwrap().is_none());

        // After deletion, can create new entry starting at version 1
        let new_version = store.cas(key.clone(), 0, b"new".to_vec()).await.unwrap();
        assert_eq!(new_version, 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_get_nonexistent() {
        let store = InMemorySharedStateStore::new();
        let result = store.get("nonexistent").await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    // Security Test: Version Overflow Protection

    #[tokio::test(flavor = "multi_thread")]
    async fn test_version_overflow_prevention() {
        let store = InMemorySharedStateStore::new();
        let key = "overflow-test".to_string();

        // Manually set a key with version near MAX_SAFE_VERSION
        store
            .put(key.clone(), b"value".to_vec(), MAX_SAFE_VERSION)
            .await
            .unwrap();

        // Attempt to CAS should fail (version >= MAX_SAFE_VERSION)
        let result = store
            .cas(key.clone(), MAX_SAFE_VERSION, b"new-value".to_vec())
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Version overflow prevention"),
            "Expected overflow prevention error, got: {}",
            err_msg
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_version_checked_arithmetic() {
        let store = InMemorySharedStateStore::new();
        let key = "arithmetic-test".to_string();

        // Set version just below MAX_SAFE_VERSION
        store
            .put(key.clone(), b"value".to_vec(), MAX_SAFE_VERSION - 1)
            .await
            .unwrap();

        // This should succeed (last safe increment)
        let result = store
            .cas(key.clone(), MAX_SAFE_VERSION - 1, b"new-value".to_vec())
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), MAX_SAFE_VERSION);

        // Next increment should fail
        let result2 = store
            .cas(key.clone(), MAX_SAFE_VERSION, b"newer-value".to_vec())
            .await;
        assert!(result2.is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_version_near_max_u64() {
        let store = InMemorySharedStateStore::new();
        let key = "max-test".to_string();

        // Set version to u64::MAX - 1
        store
            .put(key.clone(), b"value".to_vec(), u64::MAX - 1)
            .await
            .unwrap();

        // Attempt to increment should fail (version >= MAX_SAFE_VERSION)
        let result = store
            .cas(key.clone(), u64::MAX - 1, b"new-value".to_vec())
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Version overflow"),
            "Expected overflow error, got: {}",
            err_msg
        );
    }

    // Task 7.1: cas_with_retry tests

    #[tokio::test(flavor = "multi_thread")]
    async fn test_cas_with_retry_new_key() {
        let store = InMemorySharedStateStore::new();

        store
            .cas_with_retry("counter", |_| Ok(serde_json::json!(0)), 3)
            .await
            .unwrap();

        let entry = store.get("counter").await.unwrap().unwrap();
        assert_eq!(entry.version, 1);
        let val: serde_json::Value = serde_json::from_slice(&entry.value).unwrap();
        assert_eq!(val, serde_json::json!(0));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_cas_with_retry_increment() {
        let store = InMemorySharedStateStore::new();

        // Set initial value
        let initial = serde_json::to_vec(&serde_json::json!({"count": 0})).unwrap();
        store.put("counter".to_string(), initial, 1).await.unwrap();

        // Increment via cas_with_retry
        store
            .cas_with_retry(
                "counter",
                |current| {
                    let count = current
                        .and_then(|v| v.get("count").and_then(|c| c.as_i64()))
                        .unwrap_or(0);
                    Ok(serde_json::json!({"count": count + 1}))
                },
                5,
            )
            .await
            .unwrap();

        let entry = store.get("counter").await.unwrap().unwrap();
        let val: serde_json::Value = serde_json::from_slice(&entry.value).unwrap();
        assert_eq!(val["count"], 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_cas_with_retry_metrics_recorded() {
        let store = InMemorySharedStateStore::new();

        store
            .cas_with_retry("key", |_| Ok(serde_json::json!("hello")), 3)
            .await
            .unwrap();

        let m = store.metrics();
        assert_eq!(m.success_count(), 1);
        assert_eq!(m.failure_count(), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_cas_with_retry_update_fn_error_propagates() {
        let store = InMemorySharedStateStore::new();

        let result = store
            .cas_with_retry(
                "key",
                |_| Err(anyhow::anyhow!("intentional error from update_fn")),
                3,
            )
            .await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("intentional error"));
    }

    // Task 7.4: metrics tests

    #[tokio::test(flavor = "multi_thread")]
    async fn test_metrics_conflict_rate_zero_initially() {
        let store = InMemorySharedStateStore::new();
        assert_eq!(store.metrics().conflict_rate(), 0.0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_metrics_conflict_rate_after_cas() {
        let store = InMemorySharedStateStore::new();

        // One successful CAS
        store.cas("k".to_string(), 0, b"v".to_vec()).await.unwrap();
        // One conflicting CAS (wrong version)
        let _ = store.cas("k".to_string(), 0, b"v2".to_vec()).await;

        let m = store.metrics();
        assert_eq!(m.success_count(), 1);
        assert_eq!(m.conflict_count(), 1);
        // conflict_rate = 1 / (1 + 1) = 0.5
        assert!((m.conflict_rate() - 0.5).abs() < 1e-9);
    }
}
