//! Per-provider LLM credential pool with cooldown-based rotation.
//!
//! Implements multi-key rotation for LLM API credentials. When a key is
//! exhausted (billing, rate-limited, auth failure), the pool marks it with
//! a cooldown and advances to the next available key.
//!
//! # Design
//!
//! - `CredentialPool` holds `Vec<PooledCredential>` protected by `Mutex`.
//! - `select()` picks the next key according to the `SelectionStrategy`.
//! - `rotate(reason)` marks the key that was last selected as unavailable
//!   for a reason-specific cooldown duration, then advances the index.
//! - If no key is available after rotation, `rotate()` returns `false`
//!   (pool exhausted) — caller should bubble up a terminal error.
//!
//! # Cooldown schedule
//!
//! | Reason           | Cooldown         |
//! |------------------|------------------|
//! | Billing          | 24 hours         |
//! | RateLimit        | 60 seconds       |
//! | QuotaExceeded    | 5 minutes        |
//! | AuthInvalid      | permanent (~52w) |
//! | AuthExpired      | permanent (~52w) |
//! | anything else    | no cooldown      |
//!
//! Permanent markers use `Utc::now() + 52 weeks` as the sentinel. Operators
//! can manually re-enable a key by clearing the cooldown in configuration.

use chrono::{DateTime, Duration, Utc};
use std::sync::Mutex;

use crate::failover_reason::LlmFailoverReason;

// ---------------------------------------------------------------------------
// PooledCredential
// ---------------------------------------------------------------------------

/// A single API key entry inside a `CredentialPool`.
#[derive(Debug, Clone)]
pub struct PooledCredential {
    /// The API key string.
    pub api_key: String,
    /// Advisory max-concurrent limit (not enforced in v1; reserved for future).
    pub max_concurrent: u32,
    /// Cumulative successful uses (incremented on `select()`).
    pub total_uses: u64,
    /// Cumulative errors recorded against this key.
    pub total_errors: u64,
    /// When set, this key is unavailable until the timestamp passes.
    pub cooldown_until: Option<DateTime<Utc>>,
}

impl PooledCredential {
    fn new(api_key: String) -> Self {
        Self {
            api_key,
            max_concurrent: 4,
            total_uses: 0,
            total_errors: 0,
            cooldown_until: None,
        }
    }

    /// Whether this credential is currently usable.
    pub fn is_available(&self, now: DateTime<Utc>) -> bool {
        self.cooldown_until.is_none_or(|t| now >= t)
    }
}

// ---------------------------------------------------------------------------
// SelectionStrategy
// ---------------------------------------------------------------------------

/// How `CredentialPool::select()` chooses among available keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionStrategy {
    /// Always use the first available key. Only advances on `rotate()`. (default)
    #[default]
    FillFirst,
    /// Cycle through keys in order, advancing on every `select()` call.
    RoundRobin,
    /// Pick an available key at random.
    Random,
    /// Pick the key with the lowest `total_uses` count.
    LeastUsed,
}

// ---------------------------------------------------------------------------
// CredentialStats — observable snapshot
// ---------------------------------------------------------------------------

/// Read-only snapshot of a single credential's state, for display / metrics.
#[derive(Debug, Clone)]
pub struct CredentialStats {
    /// Masked key (shows last 4 chars only).
    pub key_suffix: String,
    /// Cumulative successful uses.
    pub total_uses: u64,
    /// Cumulative errors.
    pub total_errors: u64,
    /// Whether the key is currently on cooldown.
    pub is_on_cooldown: bool,
    /// When the cooldown expires, if any.
    pub cooldown_until: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// CredentialPool
// ---------------------------------------------------------------------------

/// Inner mutable state protected by `Mutex`.
struct PoolState {
    entries: Vec<PooledCredential>,
    /// Index into `entries` that was most recently selected.
    current_index: usize,
}

/// A per-provider pool of API credentials with cooldown-based rotation.
///
/// # Thread safety
///
/// `CredentialPool` is `Send + Sync`. Internal state is protected by
/// `std::sync::Mutex` (short critical sections; no async needed).
///
/// # Example
///
/// ```rust
/// use cyberclaw_llm::credential_pool::{CredentialPool, SelectionStrategy};
///
/// let pool = CredentialPool::new("anthropic", vec!["sk-1".into(), "sk-2".into()], SelectionStrategy::FillFirst);
/// let key = pool.select();
/// assert!(key.is_some());
/// ```
pub struct CredentialPool {
    /// Provider name (e.g. "anthropic", "openai") — for display only.
    pub provider: String,
    strategy: SelectionStrategy,
    state: Mutex<PoolState>,
}

impl std::fmt::Debug for CredentialPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialPool")
            .field("provider", &self.provider)
            .field("strategy", &self.strategy)
            .finish_non_exhaustive()
    }
}

impl CredentialPool {
    /// Create a new pool for the given provider.
    ///
    /// `keys` must be non-empty if you intend to call `select()`.
    pub fn new(
        provider: impl Into<String>,
        keys: Vec<String>,
        strategy: SelectionStrategy,
    ) -> Self {
        let entries = keys.into_iter().map(PooledCredential::new).collect();
        Self {
            provider: provider.into(),
            strategy,
            state: Mutex::new(PoolState {
                entries,
                current_index: 0,
            }),
        }
    }

    /// Select the best available key according to the pool's strategy.
    ///
    /// Increments `total_uses` on the chosen entry.
    /// Returns `None` if all keys are on cooldown.
    pub fn select(&self) -> Option<String> {
        let now = Utc::now();
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());

        let n = guard.entries.len();
        if n == 0 {
            return None;
        }

        let chosen_idx = match self.strategy {
            SelectionStrategy::FillFirst => {
                // First available from the beginning
                guard
                    .entries
                    .iter()
                    .enumerate()
                    .find(|(_, e)| e.is_available(now))
                    .map(|(i, _)| i)?
            }

            SelectionStrategy::RoundRobin => {
                // Start from current_index + 1, wrap around
                let start = (guard.current_index + 1) % n;
                (0..n)
                    .map(|offset| (start + offset) % n)
                    .find(|&i| guard.entries[i].is_available(now))?
            }

            SelectionStrategy::Random => {
                // Simple LCG-based pseudo-random to avoid pulling in rand
                let seed = (Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64)
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let start = (seed as usize) % n;
                (0..n)
                    .map(|offset| (start + offset) % n)
                    .find(|&i| guard.entries[i].is_available(now))?
            }

            SelectionStrategy::LeastUsed => {
                // Available key with the lowest total_uses
                guard
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.is_available(now))
                    .min_by_key(|(_, e)| e.total_uses)
                    .map(|(i, _)| i)?
            }
        };

        guard.current_index = chosen_idx;
        guard.entries[chosen_idx].total_uses += 1;
        Some(guard.entries[chosen_idx].api_key.clone())
    }

    /// Record an error against the key at `current_index` and apply a
    /// reason-appropriate cooldown.
    ///
    /// Returns `true` if at least one key is still available after rotation,
    /// or `false` if the pool is fully exhausted (caller should surface a
    /// terminal error).
    pub fn rotate(&self, reason: LlmFailoverReason) -> bool {
        let cooldown = cooldown_for(reason);
        let now = Utc::now();
        let expiry = match cooldown {
            Some(duration) => now + duration,
            // No auto-recovery: use a ~52-week sentinel
            None => now + Duration::weeks(52),
        };

        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());

        // Read index into a local before the mutable borrow via get_mut
        let idx = guard.current_index;

        // Mark the current key as exhausted
        if let Some(entry) = guard.entries.get_mut(idx) {
            entry.total_errors += 1;
            entry.cooldown_until = Some(expiry);
            tracing::warn!(
                provider = %self.provider,
                key_suffix = %mask_key(&entry.api_key),
                reason = ?reason,
                cooldown_until = %expiry,
                "credential pool: key placed on cooldown"
            );
        }

        // Check whether any key is still available
        guard.entries.iter().any(|e| e.is_available(now))
    }

    /// Number of currently available (not on cooldown) keys.
    pub fn available_count(&self) -> usize {
        let now = Utc::now();
        let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.entries.iter().filter(|e| e.is_available(now)).count()
    }

    /// Total number of keys in the pool (regardless of cooldown).
    pub fn total_count(&self) -> usize {
        let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.entries.len()
    }

    /// Observable snapshot of each credential's state (for /usage display).
    pub fn stats(&self) -> Vec<CredentialStats> {
        let now = Utc::now();
        let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .entries
            .iter()
            .map(|e| CredentialStats {
                key_suffix: mask_key(&e.api_key),
                total_uses: e.total_uses,
                total_errors: e.total_errors,
                is_on_cooldown: !e.is_available(now),
                cooldown_until: e.cooldown_until,
            })
            .collect()
    }

    /// Format the pool stats as a human-readable string for `/usage` display.
    ///
    /// Returns `None` if the pool has zero or one key (no rotation info worth
    /// showing).
    pub fn format_usage(&self) -> Option<String> {
        let total = self.total_count();
        if total <= 1 {
            return None;
        }

        let stats = self.stats();
        let available = stats.iter().filter(|s| !s.is_on_cooldown).count();
        let on_cooldown = total - available;

        let mut lines = vec![format!("Credential pool ({}):", self.provider)];
        lines.push(format!("  Total keys: {}", total));

        if on_cooldown > 0 {
            lines.push(format!(
                "  Available: {} ({} in cooldown)",
                available, on_cooldown
            ));
            for s in stats.iter().filter(|s| s.is_on_cooldown) {
                if let Some(until) = s.cooldown_until {
                    lines.push(format!(
                        "    ...{} cooldown until {}",
                        s.key_suffix,
                        until.format("%Y-%m-%dT%H:%MZ")
                    ));
                }
            }
        } else {
            lines.push(format!("  Available: {}", available));
        }

        let total_rotations: u64 = stats.iter().map(|s| s.total_errors).sum();
        if total_rotations > 0 {
            lines.push(format!("  Total rotations: {}", total_rotations));
        }

        Some(lines.join("\n"))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute the cooldown duration for a given failover reason.
///
/// Returns `Some(duration)` for auto-recoverable conditions.
/// Returns `None` for permanent / operator-must-fix conditions — callers
/// should apply the ~52-week sentinel.
fn cooldown_for(reason: LlmFailoverReason) -> Option<Duration> {
    match reason {
        LlmFailoverReason::Billing => Some(Duration::hours(24)),
        LlmFailoverReason::RateLimit => Some(Duration::seconds(60)),
        LlmFailoverReason::QuotaExceeded => Some(Duration::minutes(5)),
        // Operator must manually re-enable — no auto-recovery
        LlmFailoverReason::AuthInvalid | LlmFailoverReason::AuthExpired => None,
        // All other reasons: no cooldown (caller decides whether to rotate)
        _ => None,
    }
}

/// Return the last 4 characters of a key for display purposes.
fn mask_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 4 {
        "****".to_string()
    } else {
        format!("...{}", &key[key.len().saturating_sub(4)..])
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pool(keys: Vec<&str>, strategy: SelectionStrategy) -> CredentialPool {
        CredentialPool::new(
            "test-provider",
            keys.into_iter().map(|s| s.to_string()).collect(),
            strategy,
        )
    }

    // ── select() strategy tests ────────────────────────────────────────────

    #[test]
    fn test_fill_first_returns_first_available() {
        let pool = make_pool(
            vec!["key-a", "key-b", "key-c"],
            SelectionStrategy::FillFirst,
        );
        // Should always pick the first key until it's on cooldown
        let key1 = pool.select().unwrap();
        assert_eq!(key1, "key-a");
        let key2 = pool.select().unwrap();
        assert_eq!(key2, "key-a");
    }

    #[test]
    fn test_round_robin_advances_index() {
        let pool = make_pool(
            vec!["key-a", "key-b", "key-c"],
            SelectionStrategy::RoundRobin,
        );
        // RoundRobin starts from (current_index + 1) % n; current starts at 0
        let k1 = pool.select().unwrap(); // picks index 1 -> key-b
        let k2 = pool.select().unwrap(); // picks index 2 -> key-c
        let k3 = pool.select().unwrap(); // wraps to index 0 -> key-a
        assert_ne!(k1, k2);
        assert_ne!(k2, k3);
        // All three keys should appear across three calls
        let keys: std::collections::HashSet<_> = [k1, k2, k3].into_iter().collect();
        assert_eq!(keys.len(), 3);
    }

    #[test]
    fn test_least_used_picks_lowest_count() {
        let pool = make_pool(
            vec!["key-a", "key-b", "key-c"],
            SelectionStrategy::LeastUsed,
        );
        // All start at 0 uses; first pick is deterministic (stable min)
        let k1 = pool.select().unwrap(); // key-a (0 uses, first in tie)
                                         // After k1 has 1 use, next pick should be key-b (0 uses)
        let k2 = pool.select().unwrap();
        let k3 = pool.select().unwrap();
        // Each should be different due to least-used rotation
        let keys: std::collections::HashSet<_> = [k1, k2, k3].into_iter().collect();
        assert_eq!(keys.len(), 3);
    }

    #[test]
    fn test_select_skips_cooldown_keys() {
        let pool = make_pool(vec!["key-a", "key-b"], SelectionStrategy::FillFirst);

        // Put key-a on a 24h cooldown via billing rotation
        pool.select(); // selects key-a, sets current_index=0
        pool.rotate(LlmFailoverReason::Billing);

        // Now FillFirst should skip key-a and return key-b
        let next = pool.select().unwrap();
        assert_eq!(next, "key-b");
    }

    // ── rotate() cooldown tests ────────────────────────────────────────────

    #[test]
    fn test_rotate_billing_sets_24h_cooldown() {
        let pool = make_pool(vec!["key-a", "key-b"], SelectionStrategy::FillFirst);
        pool.select(); // selects key-a

        pool.rotate(LlmFailoverReason::Billing);

        let guard = pool.state.lock().unwrap();
        let entry = &guard.entries[0]; // key-a is at index 0
        let until = entry.cooldown_until.unwrap();
        let expected_min = Utc::now() + Duration::hours(23);
        let expected_max = Utc::now() + Duration::hours(25);
        assert!(
            until > expected_min && until < expected_max,
            "billing cooldown should be ~24h, got {until}"
        );
    }

    #[test]
    fn test_rotate_rate_limit_sets_60s_cooldown() {
        let pool = make_pool(vec!["key-a", "key-b"], SelectionStrategy::FillFirst);
        pool.select(); // selects key-a

        pool.rotate(LlmFailoverReason::RateLimit);

        let guard = pool.state.lock().unwrap();
        let entry = &guard.entries[0];
        let until = entry.cooldown_until.unwrap();
        let expected_min = Utc::now() + Duration::seconds(55);
        let expected_max = Utc::now() + Duration::seconds(65);
        assert!(
            until > expected_min && until < expected_max,
            "rate-limit cooldown should be ~60s, got {until}"
        );
    }

    #[test]
    fn test_rotate_auth_invalid_permanent_cooldown() {
        let pool = make_pool(vec!["key-a", "key-b"], SelectionStrategy::FillFirst);
        pool.select(); // selects key-a

        pool.rotate(LlmFailoverReason::AuthInvalid);

        let guard = pool.state.lock().unwrap();
        let entry = &guard.entries[0];
        let until = entry.cooldown_until.unwrap();
        // Permanent: should be at least 51 weeks in the future
        let min_permanent = Utc::now() + Duration::weeks(51);
        assert!(
            until > min_permanent,
            "auth-invalid should set permanent (~52w) cooldown, got {until}"
        );
    }

    #[test]
    fn test_rotate_returns_false_when_pool_exhausted() {
        let pool = make_pool(vec!["key-a"], SelectionStrategy::FillFirst);
        pool.select(); // selects key-a, current_index = 0

        // Rotating the only key should exhaust the pool
        let still_available = pool.rotate(LlmFailoverReason::Billing);
        assert!(
            !still_available,
            "single-key pool should report exhausted after rotation"
        );
    }

    #[test]
    fn test_rotate_returns_true_when_fresh_key_available() {
        let pool = make_pool(vec!["key-a", "key-b"], SelectionStrategy::FillFirst);
        pool.select(); // selects key-a

        let still_available = pool.rotate(LlmFailoverReason::Billing);
        assert!(
            still_available,
            "pool with a fresh key-b should return true after rotating key-a"
        );
        assert_eq!(pool.available_count(), 1);
    }

    // ── format_usage display ───────────────────────────────────────────────

    #[test]
    fn test_format_usage_hidden_for_single_key() {
        let pool = make_pool(vec!["key-a"], SelectionStrategy::FillFirst);
        assert!(
            pool.format_usage().is_none(),
            "single-key pool should produce no usage block"
        );
    }

    #[test]
    fn test_format_usage_shows_pool_stats() {
        let pool = make_pool(vec!["key-a", "key-b"], SelectionStrategy::FillFirst);
        pool.select();
        pool.rotate(LlmFailoverReason::RateLimit);

        let text = pool.format_usage().unwrap();
        assert!(text.contains("Credential pool (test-provider)"));
        assert!(text.contains("Total keys: 2"));
        assert!(text.contains("1 in cooldown"));
    }
}
