use async_trait::async_trait;
use dashmap::DashMap;
use lru::LruCache;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::Hash;
use std::num::NonZeroUsize;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tracing::{debug, trace};

/// Cache entry with metadata
#[derive(Debug, Clone)]
pub struct CacheEntry<V> {
    pub value: V,
    pub created_at: Instant,
    pub expires_at: Option<Instant>,
    pub access_count: u64,
}

impl<V> CacheEntry<V> {
    pub fn new(value: V, ttl: Option<Duration>) -> Self {
        let now = Instant::now();
        Self {
            value,
            created_at: now,
            expires_at: ttl.map(|d| now + d),
            access_count: 0,
        }
    }

    pub fn is_expired(&self) -> bool {
        self.expires_at
            .map(|exp| Instant::now() > exp)
            .unwrap_or(false)
    }
}

/// Cache layer trait
#[async_trait]
pub trait CacheLayer<K, V>: Send + Sync
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Clone + Send + Sync,
{
    /// Get value from cache
    async fn get(&self, key: &K) -> Option<V>;

    /// Put value into cache
    async fn put(&self, key: K, value: V, ttl: Option<Duration>);

    /// Remove value from cache
    async fn remove(&self, key: &K) -> Option<V>;

    /// Clear all cached values
    async fn clear(&self);

    /// Get cache statistics
    async fn stats(&self) -> CacheStats;
}

/// L1 Cache - In-memory LRU cache (hot data)
pub struct L1Cache<K, V>
where
    K: Hash + Eq + Clone,
    V: Clone,
{
    cache: Arc<RwLock<LruCache<K, CacheEntry<V>>>>,
    max_size: usize,
    stats: Arc<CacheStats>,
}

impl<K, V> L1Cache<K, V>
where
    K: Hash + Eq + Clone,
    V: Clone,
{
    pub fn new(max_size: usize) -> Self {
        let cache = LruCache::new(NonZeroUsize::new(max_size).unwrap_or(NonZeroUsize::new(100).unwrap()));
        Self {
            cache: Arc::new(RwLock::new(cache)),
            max_size,
            stats: Arc::new(CacheStats::default()),
        }
    }
}

#[async_trait]
impl<K, V> CacheLayer<K, V> for L1Cache<K, V>
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Clone + Send + Sync,
{
    async fn get(&self, key: &K) -> Option<V> {
        let mut cache = self.cache.write().unwrap();

        if let Some(entry) = cache.get_mut(key) {
            if entry.is_expired() {
                cache.pop(key);
                self.stats.misses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return None;
            }

            entry.access_count += 1;
            self.stats.hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Some(entry.value.clone())
        } else {
            self.stats.misses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            None
        }
    }

    async fn put(&self, key: K, value: V, ttl: Option<Duration>) {
        let mut cache = self.cache.write().unwrap();
        cache.put(key, CacheEntry::new(value, ttl));
        self.stats.puts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    async fn remove(&self, key: &K) -> Option<V> {
        let mut cache = self.cache.write().unwrap();
        cache.pop(key).map(|entry| {
            self.stats.removals.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            entry.value
        })
    }

    async fn clear(&self) {
        let mut cache = self.cache.write().unwrap();
        cache.clear();
        self.stats.clears.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    async fn stats(&self) -> CacheStats {
        self.stats.clone()
    }
}

/// L2 Cache - Concurrent hash map (warm data)
pub struct L2Cache<K, V>
where
    K: Hash + Eq + Clone,
    V: Clone,
{
    cache: Arc<DashMap<K, CacheEntry<V>>>,
    max_size: usize,
    stats: Arc<CacheStats>,
}

impl<K, V> L2Cache<K, V>
where
    K: Hash + Eq + Clone,
    V: Clone,
{
    pub fn new(max_size: usize) -> Self {
        Self {
            cache: Arc::new(DashMap::new()),
            max_size,
            stats: Arc::new(CacheStats::default()),
        }
    }

    /// Evict expired entries
    pub async fn evict_expired(&self) {
        let mut to_remove = Vec::new();

        for entry in self.cache.iter() {
            if entry.value().is_expired() {
                to_remove.push(entry.key().clone());
            }
        }

        for key in to_remove {
            self.cache.remove(&key);
            self.stats.evictions.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

#[async_trait]
impl<K, V> CacheLayer<K, V> for L2Cache<K, V>
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Clone + Send + Sync,
{
    async fn get(&self, key: &K) -> Option<V> {
        if let Some(mut entry) = self.cache.get_mut(key) {
            if entry.is_expired() {
                drop(entry);
                self.cache.remove(key);
                self.stats.misses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return None;
            }

            entry.access_count += 1;
            self.stats.hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Some(entry.value.clone())
        } else {
            self.stats.misses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            None
        }
    }

    async fn put(&self, key: K, value: V, ttl: Option<Duration>) {
        // Simple size limit check
        if self.cache.len() >= self.max_size {
            // Evict some old entries (simplified)
            self.evict_expired().await;
        }

        self.cache.insert(key, CacheEntry::new(value, ttl));
        self.stats.puts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    async fn remove(&self, key: &K) -> Option<V> {
        self.cache.remove(key).map(|(_, entry)| {
            self.stats.removals.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            entry.value
        })
    }

    async fn clear(&self) {
        self.cache.clear();
        self.stats.clears.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    async fn stats(&self) -> CacheStats {
        self.stats.clone()
    }
}

/// Multi-layer cache implementation
pub struct MultiLayerCache<K, V>
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Clone + Send + Sync,
{
    l1: Arc<L1Cache<K, V>>,
    l2: Arc<L2Cache<K, V>>,
    write_through: bool,
}

impl<K, V> MultiLayerCache<K, V>
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Clone + Send + Sync,
{
    pub fn new(l1_size: usize, l2_size: usize, write_through: bool) -> Self {
        Self {
            l1: Arc::new(L1Cache::new(l1_size)),
            l2: Arc::new(L2Cache::new(l2_size)),
            write_through,
        }
    }

    /// Get value checking all layers
    pub async fn get(&self, key: &K) -> Option<V> {
        // Check L1 first
        if let Some(value) = self.l1.get(key).await {
            trace!("L1 cache hit");
            return Some(value);
        }

        // Check L2
        if let Some(value) = self.l2.get(key).await {
            trace!("L2 cache hit, promoting to L1");
            // Promote to L1
            self.l1.put(key.clone(), value.clone(), None).await;
            return Some(value);
        }

        trace!("Cache miss on all layers");
        None
    }

    /// Put value into cache layers
    pub async fn put(&self, key: K, value: V, ttl: Option<Duration>) {
        if self.write_through {
            // Write to both layers
            self.l1.put(key.clone(), value.clone(), ttl).await;
            self.l2.put(key, value, ttl).await;
        } else {
            // Write to L1 only
            self.l1.put(key, value, ttl).await;
        }
    }

    /// Get combined statistics
    pub async fn stats(&self) -> MultiLayerStats {
        MultiLayerStats {
            l1_stats: self.l1.stats().await,
            l2_stats: self.l2.stats().await,
        }
    }

    /// Start background eviction task
    pub fn start_eviction_task(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));

            loop {
                interval.tick().await;
                self.l2.evict_expired().await;
            }
        });
    }
}

/// Cache statistics
#[derive(Debug, Default, Clone)]
pub struct CacheStats {
    pub hits: std::sync::atomic::AtomicU64,
    pub misses: std::sync::atomic::AtomicU64,
    pub puts: std::sync::atomic::AtomicU64,
    pub removals: std::sync::atomic::AtomicU64,
    pub evictions: std::sync::atomic::AtomicU64,
    pub clears: std::sync::atomic::AtomicU64,
}

impl CacheStats {
    pub fn hit_ratio(&self) -> f64 {
        let hits = self.hits.load(std::sync::atomic::Ordering::Relaxed) as f64;
        let misses = self.misses.load(std::sync::atomic::Ordering::Relaxed) as f64;

        if hits + misses == 0.0 {
            0.0
        } else {
            hits / (hits + misses)
        }
    }
}

/// Multi-layer cache statistics
#[derive(Debug, Clone)]
pub struct MultiLayerStats {
    pub l1_stats: CacheStats,
    pub l2_stats: CacheStats,
}

/// Cache configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    pub l1_size: usize,
    pub l2_size: usize,
    pub default_ttl: Duration,
    pub write_through: bool,
    pub eviction_interval: Duration,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            l1_size: 1000,
            l2_size: 10000,
            default_ttl: Duration::from_secs(300),
            write_through: false,
            eviction_interval: Duration::from_secs(60),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_multi_layer_cache() {
        let cache = MultiLayerCache::new(10, 100, false);

        // Test put and get
        cache.put("key1".to_string(), "value1".to_string(), None).await;
        assert_eq!(cache.get(&"key1".to_string()).await, Some("value1".to_string()));

        // Test miss
        assert_eq!(cache.get(&"key2".to_string()).await, None);
    }

    #[tokio::test]
    async fn test_cache_expiration() {
        let cache = L1Cache::<String, String>::new(10);

        // Put with TTL
        cache.put("key1".to_string(), "value1".to_string(), Some(Duration::from_millis(100))).await;

        // Should exist immediately
        assert!(cache.get(&"key1".to_string()).await.is_some());

        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Should be expired
        assert!(cache.get(&"key1".to_string()).await.is_none());
    }
}