use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum CircuitState {
    Closed { consecutive_failures: u32 },
    Open { opened_at: Instant },
    HalfOpen,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BreakerDecision {
    Allow,
    Deny { reason: String },
    Probe,
}

pub struct CircuitBreaker {
    failure_threshold: u32,
    cooldown: Duration,
    state: CircuitState,
}

impl CircuitBreaker {
    pub fn new(failure_threshold: u32, cooldown: Duration) -> Self {
        Self {
            failure_threshold,
            cooldown,
            state: CircuitState::Closed {
                consecutive_failures: 0,
            },
        }
    }

    pub fn check(&mut self) -> BreakerDecision {
        match &self.state {
            CircuitState::Closed { .. } => BreakerDecision::Allow,
            CircuitState::Open { opened_at } => {
                if opened_at.elapsed() >= self.cooldown {
                    tracing::info!("circuit breaker: Open -> HalfOpen (cooldown elapsed)");
                    self.state = CircuitState::HalfOpen;
                    BreakerDecision::Probe
                } else {
                    BreakerDecision::Deny {
                        reason: "circuit breaker is open".to_string(),
                    }
                }
            }
            CircuitState::HalfOpen => BreakerDecision::Probe,
        }
    }

    pub fn record_success(&mut self) {
        match &self.state {
            CircuitState::Closed { .. } => {
                self.state = CircuitState::Closed {
                    consecutive_failures: 0,
                };
            }
            CircuitState::HalfOpen => {
                tracing::info!("circuit breaker: HalfOpen -> Closed (probe succeeded)");
                self.state = CircuitState::Closed {
                    consecutive_failures: 0,
                };
            }
            CircuitState::Open { .. } => {
                tracing::warn!("circuit breaker: record_success called in Open state (unexpected)");
            }
        }
    }

    /// Returns `true` if this failure triggered the circuit to open.
    pub fn record_failure(&mut self) -> bool {
        match &self.state {
            CircuitState::Closed {
                consecutive_failures,
            } => {
                let new_failures = consecutive_failures + 1;
                if new_failures >= self.failure_threshold {
                    tracing::info!(
                        "circuit breaker: Closed -> Open (failures={}, threshold={})",
                        new_failures,
                        self.failure_threshold
                    );
                    self.state = CircuitState::Open {
                        opened_at: Instant::now(),
                    };
                    true
                } else {
                    self.state = CircuitState::Closed {
                        consecutive_failures: new_failures,
                    };
                    false
                }
            }
            CircuitState::HalfOpen => {
                tracing::info!("circuit breaker: HalfOpen -> Open (probe failed)");
                self.state = CircuitState::Open {
                    opened_at: Instant::now(),
                };
                true
            }
            CircuitState::Open { .. } => {
                tracing::warn!("circuit breaker: record_failure called in Open state (unexpected)");
                false
            }
        }
    }

    pub fn reset(&mut self) {
        tracing::info!("circuit breaker: forced reset -> Closed(0)");
        self.state = CircuitState::Closed {
            consecutive_failures: 0,
        };
    }

    pub fn state(&self) -> &CircuitState {
        &self.state
    }

    pub fn consecutive_failures(&self) -> u32 {
        match &self.state {
            CircuitState::Closed {
                consecutive_failures,
            } => *consecutive_failures,
            _ => 0,
        }
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new(3, Duration::from_secs(60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_default_is_closed_zero_failures() {
        let cb = CircuitBreaker::default();
        assert!(matches!(
            cb.state(),
            CircuitState::Closed {
                consecutive_failures: 0
            }
        ));
        assert_eq!(cb.consecutive_failures(), 0);
    }

    #[test]
    fn test_consecutive_successes_stay_closed_zero() {
        let mut cb = CircuitBreaker::default();
        cb.record_success();
        cb.record_success();
        cb.record_success();
        assert!(matches!(
            cb.state(),
            CircuitState::Closed {
                consecutive_failures: 0
            }
        ));
    }

    #[test]
    fn test_one_failure_closed_not_tripped() {
        let mut cb = CircuitBreaker::new(3, Duration::from_secs(60));
        let tripped = cb.record_failure();
        assert!(!tripped);
        assert_eq!(cb.consecutive_failures(), 1);
        assert!(matches!(
            cb.state(),
            CircuitState::Closed {
                consecutive_failures: 1
            }
        ));
    }

    #[test]
    fn test_three_failures_opens_circuit() {
        let mut cb = CircuitBreaker::new(3, Duration::from_secs(60));
        assert!(!cb.record_failure());
        assert!(!cb.record_failure());
        let tripped = cb.record_failure();
        assert!(tripped);
        assert!(matches!(cb.state(), CircuitState::Open { .. }));
    }

    #[test]
    fn test_open_check_returns_deny() {
        let mut cb = CircuitBreaker::new(3, Duration::from_secs(60));
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        let decision = cb.check();
        assert!(matches!(decision, BreakerDecision::Deny { .. }));
    }

    #[test]
    fn test_open_after_cooldown_returns_probe() {
        let mut cb = CircuitBreaker::new(1, Duration::from_millis(20));
        cb.record_failure();
        assert!(matches!(cb.check(), BreakerDecision::Deny { .. }));
        thread::sleep(Duration::from_millis(30));
        let decision = cb.check();
        assert_eq!(decision, BreakerDecision::Probe);
        assert!(matches!(cb.state(), CircuitState::HalfOpen));
    }

    #[test]
    fn test_halfopen_success_returns_closed() {
        let mut cb = CircuitBreaker::new(1, Duration::from_millis(20));
        cb.record_failure();
        thread::sleep(Duration::from_millis(30));
        cb.check(); // transitions to HalfOpen
        cb.record_success();
        assert!(matches!(
            cb.state(),
            CircuitState::Closed {
                consecutive_failures: 0
            }
        ));
        assert_eq!(cb.consecutive_failures(), 0);
    }

    #[test]
    fn test_halfopen_failure_returns_open() {
        let mut cb = CircuitBreaker::new(1, Duration::from_millis(20));
        cb.record_failure();
        thread::sleep(Duration::from_millis(30));
        cb.check(); // transitions to HalfOpen
        let tripped = cb.record_failure();
        assert!(tripped);
        assert!(matches!(cb.state(), CircuitState::Open { .. }));
    }

    #[test]
    fn test_reset_forces_closed_zero() {
        let mut cb = CircuitBreaker::new(3, Duration::from_secs(60));
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert!(matches!(cb.state(), CircuitState::Open { .. }));
        cb.reset();
        assert!(matches!(
            cb.state(),
            CircuitState::Closed {
                consecutive_failures: 0
            }
        ));
        assert_eq!(cb.consecutive_failures(), 0);
    }

    #[test]
    fn test_closed_check_returns_allow() {
        let mut cb = CircuitBreaker::default();
        assert_eq!(cb.check(), BreakerDecision::Allow);
    }
}
