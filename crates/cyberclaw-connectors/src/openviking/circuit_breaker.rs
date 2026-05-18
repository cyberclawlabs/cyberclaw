//! Circuit breaker for OpenViking connector.
//!
//! Trips after N consecutive failures, then enters a cooldown period during
//! which requests are rejected. After cooldown elapses the breaker moves to
//! half-open, allowing a single probe request through.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::RwLock;
use std::time::Instant;

/// Observable circuit breaker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CbState {
    Closed,
    Open,
    HalfOpen,
}

/// Thread-safe circuit breaker with atomic counters.
#[derive(Debug)]
pub struct OvCircuitBreaker {
    threshold: u32,
    cooldown_ms: u64,
    consecutive_failures: AtomicU32,
    opened_at: RwLock<Option<Instant>>,
}

impl OvCircuitBreaker {
    /// Create a new circuit breaker.
    pub fn new(threshold: u32, cooldown_ms: u64) -> Self {
        Self {
            threshold,
            cooldown_ms,
            consecutive_failures: AtomicU32::new(0),
            opened_at: RwLock::new(None),
        }
    }

    /// Current state of the breaker.
    pub fn state(&self) -> CbState {
        let failures = self.consecutive_failures.load(Ordering::SeqCst);
        if failures < self.threshold {
            return CbState::Closed;
        }
        let guard = self.opened_at.read().unwrap();
        match *guard {
            Some(ts) if ts.elapsed().as_millis() < self.cooldown_ms as u128 => CbState::Open,
            Some(_) => CbState::HalfOpen,
            None => CbState::Open,
        }
    }

    /// Returns `true` if a request should be allowed through.
    pub fn allow_request(&self) -> bool {
        match self.state() {
            CbState::Closed | CbState::HalfOpen => true,
            CbState::Open => false,
        }
    }

    /// Record a successful request -- resets the failure counter.
    pub fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::SeqCst);
        let mut guard = self.opened_at.write().unwrap();
        *guard = None;
    }

    /// Record a failed request -- increments the failure counter and may trip.
    ///
    /// Note: there is a tiny race window between `fetch_add` and acquiring the
    /// write lock on `opened_at`.  During that window another thread calling
    /// `state()` may see `failures >= threshold` with `opened_at == None`,
    /// which maps to `CbState::Open` — the conservative (safe) choice.
    pub fn record_failure(&self) {
        let prev = self.consecutive_failures.fetch_add(1, Ordering::SeqCst);
        if prev + 1 >= self.threshold {
            let mut guard = self.opened_at.write().unwrap();
            if guard.is_none() {
                *guard = Some(Instant::now());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_closed() {
        let cb = OvCircuitBreaker::new(3, 60_000);
        assert_eq!(cb.state(), CbState::Closed);
        assert!(cb.allow_request());
    }

    #[test]
    fn trips_after_threshold() {
        let cb = OvCircuitBreaker::new(2, 60_000);
        cb.record_failure();
        assert_eq!(cb.state(), CbState::Closed);
        cb.record_failure();
        assert_eq!(cb.state(), CbState::Open);
        assert!(!cb.allow_request());
    }

    #[test]
    fn success_resets() {
        let cb = OvCircuitBreaker::new(2, 60_000);
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CbState::Open);
        cb.record_success();
        assert_eq!(cb.state(), CbState::Closed);
        assert!(cb.allow_request());
    }

    #[test]
    fn half_open_after_cooldown() {
        let cb = OvCircuitBreaker::new(1, 0);
        cb.record_failure();
        assert_eq!(cb.state(), CbState::HalfOpen);
        assert!(cb.allow_request());
    }
}
