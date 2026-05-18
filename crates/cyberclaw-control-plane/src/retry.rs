//! Exponential backoff retry logic for transient failures.
//!
//! Provides [`RetryConfig`] and the [`retry_with_backoff`] helper for wrapping
//! fallible async operations with configurable retry behaviour.

use cyberclaw_core::validation::{Validate, ValidationError, ValidationResult};
use cyberclaw_observability::metrics::recorders;
use tokio::time::{sleep, Duration};

/// Configuration for exponential-backoff retries.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum total number of attempts (including the first try).
    pub max_attempts: usize,
    /// Backoff delay before the second attempt.
    pub initial_backoff: Duration,
    /// Upper bound on the computed backoff.
    pub max_backoff: Duration,
    /// Multiplier applied to the backoff after each failure.
    pub backoff_multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(10),
            backoff_multiplier: 2.0,
        }
    }
}

impl Validate for RetryConfig {
    fn validate(&self) -> ValidationResult {
        if self.max_attempts == 0 {
            return Err(ValidationError::new("max_attempts must be > 0"));
        }
        if self.initial_backoff.is_zero() {
            return Err(ValidationError::new("initial_backoff must be > 0"));
        }
        if self.max_backoff < self.initial_backoff {
            return Err(ValidationError::new(format!(
                "max_backoff ({:?}) must be >= initial_backoff ({:?})",
                self.max_backoff, self.initial_backoff
            )));
        }
        if self.backoff_multiplier <= 1.0 {
            return Err(ValidationError::new(format!(
                "backoff_multiplier ({}) must be > 1.0",
                self.backoff_multiplier
            )));
        }
        Ok(())
    }
}

/// Execute `f` with exponential-backoff retries according to `config`.
///
/// On every transient failure (i.e., any failure that is *not* the last
/// allowed attempt), a warning is logged and the caller sleeps for the
/// current backoff duration before trying again.  The backoff is capped at
/// [`RetryConfig::max_backoff`].
///
/// Returns `Ok(T)` on the first successful attempt, or `Err(E)` after all
/// attempts have been exhausted.
pub async fn retry_with_backoff<F, Fut, T, E>(
    operation: &str,
    mut f: F,
    config: &RetryConfig,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let mut attempt = 0usize;
    let mut backoff = config.initial_backoff;

    loop {
        attempt += 1;
        match f().await {
            Ok(result) => return Ok(result),
            Err(e) if attempt >= config.max_attempts => {
                tracing::error!(
                    operation = operation,
                    attempt = attempt,
                    max_attempts = config.max_attempts,
                    error = %e,
                    "operation failed after all retry attempts"
                );
                recorders::record_retry_exhausted(operation);
                return Err(e);
            }
            Err(e) => {
                tracing::warn!(
                    operation = operation,
                    attempt = attempt,
                    backoff_ms = backoff.as_millis(),
                    error = %e,
                    "operation failed, retrying"
                );
                recorders::record_retry_attempt(operation);
                sleep(backoff).await;
                // Compute next backoff, capped at max.
                let next_secs = backoff.as_secs_f64() * config.backoff_multiplier;
                backoff = Duration::from_secs_f64(next_secs).min(config.max_backoff);
            }
        }
    }
}

// ─── Idempotency Key ─────────────────────────────────────────────────────────

/// A unique key that identifies a specific capability execution attempt.
///
/// Combining `execution_id`, `capability_id`, and `attempt` ensures that
/// retries of the same logical operation produce the same key for the same
/// attempt number, and a distinct key for each new attempt — preventing
/// duplicate execution while still allowing retries.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// Build a key from raw string components.
    ///
    /// Callers that work with typed IDs should prefer
    /// [`IdempotencyKey::from_parts`].
    pub fn new(execution_id: &str, capability_id: &str, attempt: usize) -> Self {
        Self(format!("{}:{}:{}", execution_id, capability_id, attempt))
    }

    /// Return the inner string representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ─── Backoff Strategies ───────────────────────────────────────────────────────

/// Extended backoff strategies supplementing the existing [`RetryConfig`].
#[derive(Debug, Clone)]
pub enum BackoffStrategy {
    /// Exponential backoff: delay doubles (or by `multiplier`) each attempt,
    /// capped at `max_ms`.  Optionally adds uniform random jitter of up to
    /// 50 % of the current delay to spread thundering-herd retries.
    ExponentialBackoff {
        base_ms: u64,
        max_ms: u64,
        jitter: bool,
    },
    /// Linear backoff: delay increases by `interval_ms` per attempt, capped
    /// at `max_ms`.
    LinearBackoff { interval_ms: u64, max_ms: u64 },
}

impl BackoffStrategy {
    /// Compute the delay for the given (1-based) attempt number.
    ///
    /// `attempt` is the attempt that just failed (so `attempt = 1` is the
    /// first failure).  Jitter, when enabled, is deterministic in tests via
    /// a simple XOR fold of the attempt number — callers that require true
    /// randomness should apply their own jitter on top.
    pub fn delay_for(&self, attempt: usize) -> Duration {
        match self {
            BackoffStrategy::ExponentialBackoff {
                base_ms,
                max_ms,
                jitter,
            } => {
                let exp = (attempt as u32).saturating_sub(1);
                let base = (*base_ms as f64) * 2_f64.powi(exp as i32);
                let capped = base.min(*max_ms as f64) as u64;
                let delay_ms = if *jitter {
                    // Pseudo-random jitter: up to 50 % of capped delay.
                    let jitter_range = (capped / 2).max(1);
                    // XOR fold gives a stable value per attempt without
                    // pulling in a rand dependency.
                    let pseudo = (attempt as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
                    capped + (pseudo % jitter_range)
                } else {
                    capped
                };
                Duration::from_millis(delay_ms)
            }
            BackoffStrategy::LinearBackoff {
                interval_ms,
                max_ms,
            } => {
                let delay_ms = ((*interval_ms) * (attempt as u64)).min(*max_ms);
                Duration::from_millis(delay_ms)
            }
        }
    }
}

// ─── Reassignment Policy ──────────────────────────────────────────────────────

/// Determines which node should be targeted when retrying after a node-level
/// failure such as a connectivity error or unreachable host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReassignmentPolicy {
    /// Keep retrying on the same node.
    SameNode,
    /// Re-place the execution on any currently healthy node chosen by the
    /// scheduler.
    AnyHealthy,
    /// Try the supplied node IDs in priority order (first = highest priority).
    Failover(Vec<String>),
}

// ─── Retryability Classification ─────────────────────────────────────────────

/// Classification of an error with respect to retry eligibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryClass {
    /// Transient error — safe to retry (network timeout, node unreachable,
    /// temporary service unavailability, etc.).
    Retryable,
    /// Permanent error — retrying will not help (permission denied, bad
    /// arguments, business-logic rejection, etc.).
    NonRetryable,
    /// Classification is unknown; treated as non-retryable by default.
    Unknown,
}

impl RetryClass {
    /// Returns `true` if this class allows further retry attempts.
    pub fn is_retryable(self) -> bool {
        matches!(self, RetryClass::Retryable)
    }
}

/// Classify a string error message into a [`RetryClass`].
///
/// This is a heuristic based on common error substrings.  Production callers
/// should prefer matching on typed error variants where available and use this
/// function as a last resort.
pub fn classify_error(msg: &str) -> RetryClass {
    let lower = msg.to_ascii_lowercase();

    // Transient / infrastructure signals
    if lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("unreachable")
        || lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("temporarily unavailable")
        || lower.contains("service unavailable")
        || lower.contains("transient")
        || lower.contains("node unavailable")
        || lower.contains("network")
    {
        return RetryClass::Retryable;
    }

    // Permanent / business-logic signals
    if lower.contains("permission denied")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
        || lower.contains("invalid argument")
        || lower.contains("bad request")
        || lower.contains("not found")
        || lower.contains("already exists")
        || lower.contains("validation")
    {
        return RetryClass::NonRetryable;
    }

    RetryClass::Unknown
}

// ─── Retry Attempt Record ─────────────────────────────────────────────────────

/// A single recorded attempt within an [`ExecutionRetryContext`].
#[derive(Debug, Clone)]
pub struct RetryAttempt {
    /// 1-based attempt number.
    pub attempt: usize,
    /// Node identifier targeted during this attempt (free-form string so the
    /// type does not introduce a hard dependency on `cyberclaw_core` here).
    pub node_id: Option<String>,
    /// Wall-clock instant at which the attempt started.
    pub started_at: std::time::Instant,
    /// Whether the attempt succeeded.
    pub succeeded: bool,
    /// Error message if the attempt failed.
    pub error: Option<String>,
}

// ─── Execution Retry Context ──────────────────────────────────────────────────

/// Tracks the full retry history for a single logical execution and provides
/// decision helpers for callers implementing retry loops.
#[derive(Debug)]
pub struct ExecutionRetryContext {
    /// Maximum number of attempts (including the first try).
    pub max_attempts: usize,
    /// Backoff strategy used to compute inter-attempt delays.
    pub backoff: BackoffStrategy,
    /// Reassignment policy applied when a node-level failure is detected.
    pub reassignment: ReassignmentPolicy,
    /// Recorded history of every attempt made so far.
    pub attempts: Vec<RetryAttempt>,
}

impl ExecutionRetryContext {
    /// Create a new context with the given limits and strategies.
    pub fn new(
        max_attempts: usize,
        backoff: BackoffStrategy,
        reassignment: ReassignmentPolicy,
    ) -> Self {
        Self {
            max_attempts,
            backoff,
            reassignment,
            attempts: Vec::new(),
        }
    }

    /// Record the outcome of one attempt.
    ///
    /// `node_id` is `None` when no node has been assigned yet.
    pub fn record_attempt(
        &mut self,
        node_id: Option<String>,
        started_at: std::time::Instant,
        succeeded: bool,
        error: Option<String>,
    ) {
        let attempt = self.attempts.len() + 1;
        self.attempts.push(RetryAttempt {
            attempt,
            node_id,
            started_at,
            succeeded,
            error,
        });
    }

    /// Return the number of attempts recorded so far.
    pub fn attempt_count(&self) -> usize {
        self.attempts.len()
    }

    /// Decide whether another attempt should be made.
    ///
    /// Returns `false` if:
    /// - The error is classified as [`RetryClass::NonRetryable`], or
    /// - The maximum attempt count has been reached or exceeded.
    pub fn should_retry(&self, error_msg: &str) -> bool {
        if self.attempts.len() >= self.max_attempts {
            return false;
        }
        match classify_error(error_msg) {
            RetryClass::NonRetryable => false,
            RetryClass::Retryable | RetryClass::Unknown => {
                // Unknown defaults to retrying here because the caller already
                // checked the attempt ceiling above.  Callers that want
                // Unknown → no-retry can check `classify_error` themselves.
                true
            }
        }
    }

    /// Compute the delay to wait before the next attempt.
    ///
    /// Uses `attempts.len() + 1` so that the delay escalates correctly:
    /// after 0 recorded attempts the first retry gets `delay_for(1)`,
    /// after 1 recorded attempt the second retry gets `delay_for(2)`, etc.
    pub fn next_delay(&self) -> Duration {
        self.backoff.delay_for(self.attempts.len() + 1)
    }

    /// Build an [`IdempotencyKey`] for the next attempt that will be made.
    pub fn idempotency_key(&self, execution_id: &str, capability_id: &str) -> IdempotencyKey {
        IdempotencyKey::new(execution_id, capability_id, self.attempts.len() + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Returns a closure that fails the first `fail_times` calls, then succeeds.
    fn flaky_op(
        fail_times: usize,
        call_count: Arc<AtomicUsize>,
    ) -> impl FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<u32, String>>>>
    {
        move || {
            let count = call_count.clone();
            let n = count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if n < fail_times {
                    Err(format!("transient error on attempt {}", n + 1))
                } else {
                    Ok(42)
                }
            })
        }
    }

    #[tokio::test]
    async fn test_success_on_first_attempt() {
        let calls = Arc::new(AtomicUsize::new(0));
        let config = RetryConfig {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(10),
            backoff_multiplier: 2.0,
        };
        let result = retry_with_backoff("test_op", flaky_op(0, calls.clone()), &config).await;
        assert_eq!(result.unwrap(), 42);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_transient_failure() {
        let calls = Arc::new(AtomicUsize::new(0));
        let config = RetryConfig {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(10),
            backoff_multiplier: 2.0,
        };
        // Fails twice, succeeds on 3rd attempt.
        let result = retry_with_backoff("test_op", flaky_op(2, calls.clone()), &config).await;
        assert_eq!(result.unwrap(), 42);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_exhausts_retries_and_returns_error() {
        let calls = Arc::new(AtomicUsize::new(0));
        let config = RetryConfig {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(10),
            backoff_multiplier: 2.0,
        };
        // Always fails.
        let result =
            retry_with_backoff("test_op", flaky_op(usize::MAX, calls.clone()), &config).await;
        assert!(result.is_err());
        // Should have attempted exactly max_attempts times.
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_backoff_capped_at_max() {
        // Verify that the retry loop terminates correctly even with very short
        // delays; correctness of the cap is tested by exhausting a long retry
        // chain without hanging.
        let calls = Arc::new(AtomicUsize::new(0));
        let config = RetryConfig {
            max_attempts: 5,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(2),
            backoff_multiplier: 100.0, // would exceed max without cap
        };
        let result =
            retry_with_backoff("test_op", flaky_op(usize::MAX, calls.clone()), &config).await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn test_single_attempt_no_retry() {
        let calls = Arc::new(AtomicUsize::new(0));
        let config = RetryConfig {
            max_attempts: 1,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(10),
            backoff_multiplier: 2.0,
        };
        let result =
            retry_with_backoff("test_op", flaky_op(usize::MAX, calls.clone()), &config).await;
        assert!(result.is_err());
        // With max_attempts = 1, no retry should occur.
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_retry_config_validation_valid() {
        let config = RetryConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_retry_config_validation_zero_attempts() {
        let config = RetryConfig {
            max_attempts: 0,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(10),
            backoff_multiplier: 2.0,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("max_attempts"));
    }

    #[test]
    fn test_retry_config_validation_zero_initial_backoff() {
        let config = RetryConfig {
            max_attempts: 3,
            initial_backoff: Duration::ZERO,
            max_backoff: Duration::from_secs(10),
            backoff_multiplier: 2.0,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("initial_backoff"));
    }

    #[test]
    fn test_retry_config_validation_max_less_than_initial() {
        let config = RetryConfig {
            max_attempts: 3,
            initial_backoff: Duration::from_secs(10),
            max_backoff: Duration::from_millis(100),
            backoff_multiplier: 2.0,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("max_backoff"));
    }

    #[test]
    fn test_retry_config_validation_invalid_multiplier() {
        let config = RetryConfig {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(10),
            backoff_multiplier: 1.0,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("backoff_multiplier"));
    }

    // ─── Enhanced retry tests ─────────────────────────────────────────────────

    /// Idempotency keys are unique per attempt and reproducible.
    #[test]
    fn test_idempotency_key_unique_and_reproducible() {
        let key1 = IdempotencyKey::new("exec-1", "cap-A", 1);
        let key2 = IdempotencyKey::new("exec-1", "cap-A", 2);
        let key1_again = IdempotencyKey::new("exec-1", "cap-A", 1);

        // Different attempts produce different keys.
        assert_ne!(key1, key2);
        // Same inputs reproduce the same key.
        assert_eq!(key1, key1_again);
        // Different execution IDs produce different keys even at the same attempt.
        let key3 = IdempotencyKey::new("exec-2", "cap-A", 1);
        assert_ne!(key1, key3);
        // String representation includes all three components.
        assert_eq!(key1.as_str(), "exec-1:cap-A:1");
    }

    /// ExponentialBackoff doubles the delay each attempt and caps at max_ms.
    #[test]
    fn test_exponential_backoff_calculation() {
        let strat = BackoffStrategy::ExponentialBackoff {
            base_ms: 100,
            max_ms: 800,
            jitter: false,
        };
        // attempt 1 → 100 ms (base * 2^0)
        assert_eq!(strat.delay_for(1), Duration::from_millis(100));
        // attempt 2 → 200 ms (base * 2^1)
        assert_eq!(strat.delay_for(2), Duration::from_millis(200));
        // attempt 3 → 400 ms
        assert_eq!(strat.delay_for(3), Duration::from_millis(400));
        // attempt 4 → 800 ms (capped)
        assert_eq!(strat.delay_for(4), Duration::from_millis(800));
        // attempt 5 → still 800 ms (cap holds)
        assert_eq!(strat.delay_for(5), Duration::from_millis(800));
    }

    /// LinearBackoff increments by interval_ms and caps at max_ms.
    #[test]
    fn test_linear_backoff_calculation() {
        let strat = BackoffStrategy::LinearBackoff {
            interval_ms: 50,
            max_ms: 150,
        };
        assert_eq!(strat.delay_for(1), Duration::from_millis(50));
        assert_eq!(strat.delay_for(2), Duration::from_millis(100));
        assert_eq!(strat.delay_for(3), Duration::from_millis(150));
        // Capped at max_ms.
        assert_eq!(strat.delay_for(4), Duration::from_millis(150));
    }

    /// Error classification: retryable vs non-retryable vs unknown.
    #[test]
    fn test_error_classification() {
        assert_eq!(
            classify_error("connection timeout after 30s"),
            RetryClass::Retryable
        );
        assert_eq!(classify_error("node unavailable"), RetryClass::Retryable);
        assert_eq!(
            classify_error("permission denied for user"),
            RetryClass::NonRetryable
        );
        assert_eq!(
            classify_error("invalid argument: missing field"),
            RetryClass::NonRetryable
        );
        assert_eq!(
            classify_error("something weird happened"),
            RetryClass::Unknown
        );

        // is_retryable helper
        assert!(RetryClass::Retryable.is_retryable());
        assert!(!RetryClass::NonRetryable.is_retryable());
        assert!(!RetryClass::Unknown.is_retryable());
    }

    /// Retry history is recorded correctly.
    #[test]
    fn test_retry_history_recording() {
        let mut ctx = ExecutionRetryContext::new(
            3,
            BackoffStrategy::ExponentialBackoff {
                base_ms: 100,
                max_ms: 1000,
                jitter: false,
            },
            ReassignmentPolicy::SameNode,
        );

        assert_eq!(ctx.attempt_count(), 0);

        ctx.record_attempt(
            Some("node-1".into()),
            std::time::Instant::now(),
            false,
            Some("timeout".into()),
        );
        assert_eq!(ctx.attempt_count(), 1);
        assert_eq!(ctx.attempts[0].attempt, 1);
        assert!(!ctx.attempts[0].succeeded);
        assert_eq!(ctx.attempts[0].node_id.as_deref(), Some("node-1"));

        ctx.record_attempt(Some("node-1".into()), std::time::Instant::now(), true, None);
        assert_eq!(ctx.attempt_count(), 2);
        assert!(ctx.attempts[1].succeeded);
    }

    /// Max retry count is enforced: should_retry returns false once exhausted.
    #[test]
    fn test_max_retry_limit_enforced() {
        let mut ctx = ExecutionRetryContext::new(
            2,
            BackoffStrategy::LinearBackoff {
                interval_ms: 10,
                max_ms: 100,
            },
            ReassignmentPolicy::AnyHealthy,
        );

        // No attempts yet — retryable error should allow retry.
        assert!(ctx.should_retry("connection timeout"));

        ctx.record_attempt(
            None,
            std::time::Instant::now(),
            false,
            Some("timeout".into()),
        );
        // 1 attempt recorded, max is 2 — still can retry.
        assert!(ctx.should_retry("connection timeout"));

        ctx.record_attempt(
            None,
            std::time::Instant::now(),
            false,
            Some("timeout".into()),
        );
        // 2 attempts recorded, at max — no more retries.
        assert!(!ctx.should_retry("connection timeout"));
    }

    /// Non-retryable error stops retrying immediately regardless of count.
    #[test]
    fn test_non_retryable_error_stops_retry() {
        let ctx = ExecutionRetryContext::new(
            5,
            BackoffStrategy::ExponentialBackoff {
                base_ms: 50,
                max_ms: 500,
                jitter: false,
            },
            ReassignmentPolicy::Failover(vec!["node-2".into(), "node-3".into()]),
        );

        // Permission denied is NonRetryable — must not retry even with attempts left.
        assert!(!ctx.should_retry("permission denied for agent"));
    }

    /// next_delay returns the correct delay based on attempt count.
    #[test]
    fn test_next_delay_advances_with_attempts() {
        let mut ctx = ExecutionRetryContext::new(
            5,
            BackoffStrategy::ExponentialBackoff {
                base_ms: 100,
                max_ms: 10_000,
                jitter: false,
            },
            ReassignmentPolicy::SameNode,
        );

        // 0 attempts recorded → delay for attempt 1 = 100 ms
        assert_eq!(ctx.next_delay(), Duration::from_millis(100));

        ctx.record_attempt(None, std::time::Instant::now(), false, None);
        // 1 attempt recorded → delay for attempt 2 = 200 ms
        assert_eq!(ctx.next_delay(), Duration::from_millis(200));
    }

    /// idempotency_key from context advances with attempt count.
    #[test]
    fn test_idempotency_key_from_context() {
        let mut ctx = ExecutionRetryContext::new(
            3,
            BackoffStrategy::LinearBackoff {
                interval_ms: 10,
                max_ms: 100,
            },
            ReassignmentPolicy::SameNode,
        );

        let key_before = ctx.idempotency_key("exec-42", "cap-X");
        assert_eq!(key_before.as_str(), "exec-42:cap-X:1");

        ctx.record_attempt(None, std::time::Instant::now(), false, None);
        let key_after = ctx.idempotency_key("exec-42", "cap-X");
        assert_eq!(key_after.as_str(), "exec-42:cap-X:2");

        assert_ne!(key_before, key_after);
    }
}
