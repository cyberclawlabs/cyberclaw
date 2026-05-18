//! Provider decorator chain for retry, failover, and circuit breaker patterns.
//!
//! Implements the decorator pattern where each provider wraps another provider,
//! sharing the same `LlmClient` trait. Decorators can be composed to build
//! resilient LLM call chains.
//!
//! # Example
//!
//! ```rust,no_run
//! use cyberclaw_llm::provider_chain::*;
//! use std::sync::Arc;
//!
//! // Build a chain: circuit_breaker -> retry -> inner_provider
//! // let chain = build_provider_chain(primary, vec![secondary, fallback]);
//! ```

use crate::client::LlmClient;
use crate::error::{LlmError, LlmResult};
use crate::types::{ChatChunk, ChatRequest, ChatResponse};
use async_trait::async_trait;
use futures::stream::Stream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// LlmErrorKind — classifies errors as transient (retryable) or non-transient
// ---------------------------------------------------------------------------

/// Classification of LLM errors for retry decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmErrorKind {
    /// Transient errors that may succeed on retry (429, 500, 503, timeout).
    Transient,
    /// Non-transient errors that should not be retried (400, 401, 403).
    NonTransient,
}

/// Classify an `LlmError` into a retry-relevant kind.
pub fn classify_error(err: &LlmError) -> LlmErrorKind {
    match err {
        LlmError::Timeout => LlmErrorKind::Transient,
        LlmError::ApiError { status, .. } => match *status {
            429 | 500 | 502 | 503 | 504 => LlmErrorKind::Transient,
            400 | 401 | 403 | 404 => LlmErrorKind::NonTransient,
            _ => LlmErrorKind::NonTransient,
        },
        LlmError::HttpError(_) => LlmErrorKind::Transient,
        _ => LlmErrorKind::NonTransient,
    }
}

// ---------------------------------------------------------------------------
// RetryProvider
// ---------------------------------------------------------------------------

/// Configuration for retry behavior.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (not counting the initial attempt).
    pub max_retries: u32,
    /// Base delay for exponential backoff.
    pub base_delay: Duration,
    /// Maximum delay cap.
    pub max_delay: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
        }
    }
}

/// Decorator that retries transient failures with exponential backoff.
pub struct RetryProvider {
    inner: Arc<dyn LlmClient>,
    config: RetryConfig,
}

impl RetryProvider {
    /// Create a new retry provider wrapping `inner`.
    pub fn new(inner: Arc<dyn LlmClient>, config: RetryConfig) -> Self {
        Self { inner, config }
    }

    /// Compute delay for the given attempt (0-indexed).
    fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let delay = self
            .config
            .base_delay
            .saturating_mul(2u32.saturating_pow(attempt));
        std::cmp::min(delay, self.config.max_delay)
    }
}

#[async_trait]
impl LlmClient for RetryProvider {
    async fn chat_completion(&self, request: ChatRequest) -> LlmResult<ChatResponse> {
        let mut last_err: Option<LlmError> = None;

        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                if let Some(ref e) = last_err {
                    if classify_error(e) == LlmErrorKind::NonTransient {
                        return Err(last_err.unwrap());
                    }
                }
                let delay = self.delay_for_attempt(attempt - 1);
                tracing::warn!(
                    attempt = attempt,
                    delay_ms = delay.as_millis() as u64,
                    "retrying chat_completion"
                );
                tokio::time::sleep(delay).await;
            }

            match self.inner.chat_completion(request.clone()).await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| LlmError::Internal("retry exhausted".to_string())))
    }

    async fn chat_completion_stream(
        &self,
        request: ChatRequest,
    ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>> {
        // Streaming is not retried mid-stream; only the initial connection is retried.
        let mut last_err: Option<LlmError> = None;

        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                if let Some(ref e) = last_err {
                    if classify_error(e) == LlmErrorKind::NonTransient {
                        return Err(last_err.unwrap());
                    }
                }
                let delay = self.delay_for_attempt(attempt - 1);
                tracing::warn!(
                    attempt = attempt,
                    delay_ms = delay.as_millis() as u64,
                    "retrying chat_completion_stream"
                );
                tokio::time::sleep(delay).await;
            }

            match self.inner.chat_completion_stream(request.clone()).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| LlmError::Internal("retry exhausted".to_string())))
    }

    fn provider(&self) -> &str {
        "retry"
    }

    async fn validate_connection(&self) -> LlmResult<()> {
        self.inner.validate_connection().await
    }
}

// ---------------------------------------------------------------------------
// FailoverProvider
// ---------------------------------------------------------------------------

/// Decorator that fails over to backup providers on error.
pub struct FailoverProvider {
    providers: Vec<Arc<dyn LlmClient>>,
}

impl FailoverProvider {
    /// Create a failover provider. The first provider is primary; the rest are backups.
    ///
    /// # Panics
    ///
    /// Panics if `providers` is empty.
    pub fn new(providers: Vec<Arc<dyn LlmClient>>) -> Self {
        assert!(!providers.is_empty(), "at least one provider is required");
        Self { providers }
    }
}

#[async_trait]
impl LlmClient for FailoverProvider {
    async fn chat_completion(&self, request: ChatRequest) -> LlmResult<ChatResponse> {
        let mut last_err: Option<LlmError> = None;

        for (i, provider) in self.providers.iter().enumerate() {
            match provider.chat_completion(request.clone()).await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    tracing::warn!(
                        provider_index = i,
                        provider = provider.provider(),
                        error = %e,
                        "failover: provider failed, trying next"
                    );
                    last_err = Some(e);
                }
            }
        }

        Err(last_err
            .unwrap_or_else(|| LlmError::Internal("all failover providers exhausted".to_string())))
    }

    async fn chat_completion_stream(
        &self,
        request: ChatRequest,
    ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>> {
        let mut last_err: Option<LlmError> = None;

        for (i, provider) in self.providers.iter().enumerate() {
            match provider.chat_completion_stream(request.clone()).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    tracing::warn!(
                        provider_index = i,
                        provider = provider.provider(),
                        error = %e,
                        "failover: provider stream failed, trying next"
                    );
                    last_err = Some(e);
                }
            }
        }

        Err(last_err
            .unwrap_or_else(|| LlmError::Internal("all failover providers exhausted".to_string())))
    }

    fn provider(&self) -> &str {
        "failover"
    }

    async fn validate_connection(&self) -> LlmResult<()> {
        // Validate the primary provider only.
        if let Some(primary) = self.providers.first() {
            primary.validate_connection().await
        } else {
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// CircuitBreakerProvider
// ---------------------------------------------------------------------------

/// State of the circuit breaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation — requests pass through.
    Closed,
    /// Circuit is open — requests are rejected immediately.
    Open,
    /// Trial state — allows a single probe request.
    HalfOpen,
}

/// Configuration for the circuit breaker.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the circuit.
    pub failure_threshold: u32,
    /// Duration after which the circuit transitions from Open to HalfOpen.
    pub half_open_after: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            half_open_after: Duration::from_secs(30),
        }
    }
}

/// Internal mutable state for the circuit breaker.
struct CircuitBreakerState {
    state: CircuitState,
    consecutive_failures: u32,
    last_failure_time: Option<Instant>,
}

/// Decorator that implements the circuit breaker pattern.
///
/// After `failure_threshold` consecutive failures the circuit opens and all
/// requests are rejected immediately with a descriptive error.  After
/// `half_open_after` elapses, a single probe request is allowed through.
/// If it succeeds the circuit closes; otherwise it reopens.
pub struct CircuitBreakerProvider {
    inner: Arc<dyn LlmClient>,
    config: CircuitBreakerConfig,
    state: Mutex<CircuitBreakerState>,
    /// Total number of times the circuit has been tripped (observable metric).
    pub trip_count: AtomicU64,
}

impl CircuitBreakerProvider {
    /// Create a new circuit breaker wrapping `inner`.
    pub fn new(inner: Arc<dyn LlmClient>, config: CircuitBreakerConfig) -> Self {
        Self {
            inner,
            config,
            state: Mutex::new(CircuitBreakerState {
                state: CircuitState::Closed,
                consecutive_failures: 0,
                last_failure_time: None,
            }),
            trip_count: AtomicU64::new(0),
        }
    }

    /// Return the current circuit state.
    pub async fn circuit_state(&self) -> CircuitState {
        let guard = self.state.lock().await;
        guard.state
    }

    /// Check whether a request should be allowed and update state transitions.
    async fn check_state(&self) -> LlmResult<()> {
        let mut guard = self.state.lock().await;

        match guard.state {
            CircuitState::Closed => Ok(()),
            CircuitState::Open => {
                if let Some(last) = guard.last_failure_time {
                    if last.elapsed() >= self.config.half_open_after {
                        tracing::info!("circuit breaker transitioning to half-open");
                        guard.state = CircuitState::HalfOpen;
                        return Ok(());
                    }
                }
                Err(LlmError::Internal(
                    "circuit breaker is open — requests are blocked".to_string(),
                ))
            }
            CircuitState::HalfOpen => Ok(()),
        }
    }

    /// Record a successful call.
    async fn record_success(&self) {
        let mut guard = self.state.lock().await;
        guard.consecutive_failures = 0;
        guard.state = CircuitState::Closed;
    }

    /// Record a failed call and potentially trip the breaker.
    async fn record_failure(&self) {
        let mut guard = self.state.lock().await;
        guard.consecutive_failures += 1;
        guard.last_failure_time = Some(Instant::now());

        if guard.consecutive_failures >= self.config.failure_threshold {
            if guard.state != CircuitState::Open {
                tracing::warn!(
                    failures = guard.consecutive_failures,
                    "circuit breaker tripped to open"
                );
                self.trip_count.fetch_add(1, Ordering::Relaxed);
            }
            guard.state = CircuitState::Open;
        }
    }
}

#[async_trait]
impl LlmClient for CircuitBreakerProvider {
    async fn chat_completion(&self, request: ChatRequest) -> LlmResult<ChatResponse> {
        self.check_state().await?;

        match self.inner.chat_completion(request).await {
            Ok(resp) => {
                self.record_success().await;
                Ok(resp)
            }
            Err(e) => {
                self.record_failure().await;
                Err(e)
            }
        }
    }

    async fn chat_completion_stream(
        &self,
        request: ChatRequest,
    ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>> {
        self.check_state().await?;

        match self.inner.chat_completion_stream(request).await {
            Ok(stream) => {
                self.record_success().await;
                Ok(stream)
            }
            Err(e) => {
                self.record_failure().await;
                Err(e)
            }
        }
    }

    fn provider(&self) -> &str {
        "circuit_breaker"
    }

    async fn validate_connection(&self) -> LlmResult<()> {
        self.inner.validate_connection().await
    }
}

// ---------------------------------------------------------------------------
// ModelDegradationChain
// ---------------------------------------------------------------------------

/// Three-tier model degradation chain: primary -> secondary -> fallback.
///
/// Unlike `FailoverProvider` (which is generic over N providers), this struct
/// gives each tier an explicit semantic name for configuration and logging.
pub struct ModelDegradationChain {
    primary: Arc<dyn LlmClient>,
    secondary: Arc<dyn LlmClient>,
    fallback: Arc<dyn LlmClient>,
}

impl ModelDegradationChain {
    /// Create a degradation chain with three tiers.
    pub fn new(
        primary: Arc<dyn LlmClient>,
        secondary: Arc<dyn LlmClient>,
        fallback: Arc<dyn LlmClient>,
    ) -> Self {
        Self {
            primary,
            secondary,
            fallback,
        }
    }
}

#[async_trait]
impl LlmClient for ModelDegradationChain {
    async fn chat_completion(&self, request: ChatRequest) -> LlmResult<ChatResponse> {
        match self.primary.chat_completion(request.clone()).await {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                tracing::warn!(tier = "primary", error = %e, "degradation: primary failed");
            }
        }

        match self.secondary.chat_completion(request.clone()).await {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                tracing::warn!(tier = "secondary", error = %e, "degradation: secondary failed");
            }
        }

        self.fallback.chat_completion(request).await
    }

    async fn chat_completion_stream(
        &self,
        request: ChatRequest,
    ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>> {
        match self.primary.chat_completion_stream(request.clone()).await {
            Ok(stream) => return Ok(stream),
            Err(e) => {
                tracing::warn!(tier = "primary", error = %e, "degradation: primary stream failed");
            }
        }

        match self.secondary.chat_completion_stream(request.clone()).await {
            Ok(stream) => return Ok(stream),
            Err(e) => {
                tracing::warn!(
                    tier = "secondary",
                    error = %e,
                    "degradation: secondary stream failed"
                );
            }
        }

        self.fallback.chat_completion_stream(request).await
    }

    fn provider(&self) -> &str {
        "degradation_chain"
    }

    async fn validate_connection(&self) -> LlmResult<()> {
        self.primary.validate_connection().await
    }
}

// ---------------------------------------------------------------------------
// build_provider_chain
// ---------------------------------------------------------------------------

/// Build a standard provider chain: `CircuitBreaker -> Retry -> Failover`.
///
/// The `primary` provider is wrapped with retry and circuit breaker.
/// If `backups` are provided they form a failover chain together with the
/// wrapped primary.
///
/// Returns an `Arc<dyn LlmClient>` ready for use.
pub fn build_provider_chain(
    primary: Arc<dyn LlmClient>,
    backups: Vec<Arc<dyn LlmClient>>,
) -> Arc<dyn LlmClient> {
    // Layer 1: wrap primary with retry
    let retried: Arc<dyn LlmClient> = Arc::new(RetryProvider::new(primary, RetryConfig::default()));

    // Layer 2: wrap with circuit breaker
    let breaker: Arc<dyn LlmClient> = Arc::new(CircuitBreakerProvider::new(
        retried,
        CircuitBreakerConfig::default(),
    ));

    // Layer 3: if backups exist, wrap in failover
    if backups.is_empty() {
        breaker
    } else {
        let mut providers = vec![breaker];
        providers.extend(backups);
        Arc::new(FailoverProvider::new(providers))
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    // -----------------------------------------------------------------------
    // Mock provider
    // -----------------------------------------------------------------------

    /// A configurable mock LlmClient for testing decorator behavior.
    struct MockProvider {
        name: String,
        /// Number of calls that will fail before succeeding.
        fail_count: AtomicU32,
        /// The status code to return on failure.
        fail_status: u16,
        /// Total number of calls received.
        call_count: AtomicU32,
    }

    impl MockProvider {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                fail_count: AtomicU32::new(0),
                fail_status: 500,
                call_count: AtomicU32::new(0),
            }
        }

        fn with_failures(name: &str, fail_n: u32, status: u16) -> Self {
            Self {
                name: name.to_string(),
                fail_count: AtomicU32::new(fail_n),
                fail_status: status,
                call_count: AtomicU32::new(0),
            }
        }

        fn calls(&self) -> u32 {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    fn dummy_request() -> ChatRequest {
        ChatRequest {
            model: "test-model".to_string(),
            messages: vec![crate::types::Message::user("hello")],
            ..Default::default()
        }
    }

    fn dummy_response() -> ChatResponse {
        ChatResponse {
            id: "resp-1".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "test-model".to_string(),
            choices: vec![crate::types::Choice {
                index: 0,
                message: crate::types::Message::assistant("hi"),
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        }
    }

    #[async_trait]
    impl LlmClient for MockProvider {
        async fn chat_completion(&self, _request: ChatRequest) -> LlmResult<ChatResponse> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            let remaining = self.fail_count.load(Ordering::SeqCst);
            if remaining > 0 {
                self.fail_count.fetch_sub(1, Ordering::SeqCst);
                return Err(LlmError::ApiError {
                    status: self.fail_status,
                    message: format!("{} transient failure", self.name),
                });
            }
            Ok(dummy_response())
        }

        async fn chat_completion_stream(
            &self,
            _request: ChatRequest,
        ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            let remaining = self.fail_count.load(Ordering::SeqCst);
            if remaining > 0 {
                self.fail_count.fetch_sub(1, Ordering::SeqCst);
                return Err(LlmError::ApiError {
                    status: self.fail_status,
                    message: format!("{} transient failure", self.name),
                });
            }
            Ok(Box::new(futures::stream::empty()))
        }

        fn provider(&self) -> &str {
            &self.name
        }

        async fn validate_connection(&self) -> LlmResult<()> {
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // classify_error tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_classify_transient_errors() {
        assert_eq!(classify_error(&LlmError::Timeout), LlmErrorKind::Transient);
        for status in [429, 500, 502, 503, 504] {
            let err = LlmError::ApiError {
                status,
                message: "err".into(),
            };
            assert_eq!(classify_error(&err), LlmErrorKind::Transient);
        }
    }

    #[test]
    fn test_classify_non_transient_errors() {
        for status in [400, 401, 403, 404] {
            let err = LlmError::ApiError {
                status,
                message: "err".into(),
            };
            assert_eq!(classify_error(&err), LlmErrorKind::NonTransient);
        }
        assert_eq!(
            classify_error(&LlmError::ConfigError("bad".into())),
            LlmErrorKind::NonTransient
        );
    }

    // -----------------------------------------------------------------------
    // RetryProvider tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_retry_succeeds_after_transient_failures() {
        let mock = Arc::new(MockProvider::with_failures("retry-test", 2, 500));
        let retry = RetryProvider::new(
            mock.clone(),
            RetryConfig {
                max_retries: 3,
                base_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(10),
            },
        );

        let result = retry.chat_completion(dummy_request()).await;
        assert!(result.is_ok());
        assert_eq!(mock.calls(), 3); // 1 initial + 2 retries
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let mock = Arc::new(MockProvider::with_failures("exhaust", 10, 500));
        let retry = RetryProvider::new(
            mock.clone(),
            RetryConfig {
                max_retries: 2,
                base_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(10),
            },
        );

        let result = retry.chat_completion(dummy_request()).await;
        assert!(result.is_err());
        assert_eq!(mock.calls(), 3); // 1 initial + 2 retries
    }

    #[tokio::test]
    async fn test_retry_skips_non_transient() {
        let mock = Arc::new(MockProvider::with_failures("non-trans", 5, 401));
        let retry = RetryProvider::new(
            mock.clone(),
            RetryConfig {
                max_retries: 3,
                base_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(10),
            },
        );

        let result = retry.chat_completion(dummy_request()).await;
        assert!(result.is_err());
        // Should stop after first failure + one retry that checks error kind
        assert_eq!(mock.calls(), 1);
    }

    #[tokio::test]
    async fn test_retry_immediate_success() {
        let mock = Arc::new(MockProvider::new("ok"));
        let retry = RetryProvider::new(mock.clone(), RetryConfig::default());

        let result = retry.chat_completion(dummy_request()).await;
        assert!(result.is_ok());
        assert_eq!(mock.calls(), 1);
    }

    // -----------------------------------------------------------------------
    // FailoverProvider tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_failover_uses_backup_on_primary_failure() {
        let primary = Arc::new(MockProvider::with_failures("primary", 100, 500));
        let backup = Arc::new(MockProvider::new("backup"));

        let failover = FailoverProvider::new(vec![primary.clone(), backup.clone()]);

        let result = failover.chat_completion(dummy_request()).await;
        assert!(result.is_ok());
        assert_eq!(primary.calls(), 1);
        assert_eq!(backup.calls(), 1);
    }

    #[tokio::test]
    async fn test_failover_all_fail() {
        let p1 = Arc::new(MockProvider::with_failures("p1", 100, 500));
        let p2 = Arc::new(MockProvider::with_failures("p2", 100, 500));

        let failover = FailoverProvider::new(vec![p1.clone(), p2.clone()]);

        let result = failover.chat_completion(dummy_request()).await;
        assert!(result.is_err());
        assert_eq!(p1.calls(), 1);
        assert_eq!(p2.calls(), 1);
    }

    // -----------------------------------------------------------------------
    // CircuitBreakerProvider tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_circuit_breaker_opens_after_threshold() {
        let mock = Arc::new(MockProvider::with_failures("cb-test", 100, 500));
        let cb = CircuitBreakerProvider::new(
            mock.clone(),
            CircuitBreakerConfig {
                failure_threshold: 3,
                half_open_after: Duration::from_secs(60),
            },
        );

        // Trigger 3 failures to trip the breaker
        for _ in 0..3 {
            let _ = cb.chat_completion(dummy_request()).await;
        }

        assert_eq!(cb.circuit_state().await, CircuitState::Open);
        assert_eq!(cb.trip_count.load(Ordering::Relaxed), 1);

        // Next call should be rejected without reaching the inner provider
        let result = cb.chat_completion(dummy_request()).await;
        assert!(result.is_err());
        assert_eq!(mock.calls(), 3); // no additional call
    }

    #[tokio::test]
    async fn test_circuit_breaker_half_open_recovery() {
        let mock = Arc::new(MockProvider::with_failures("cb-recover", 3, 500));
        let cb = CircuitBreakerProvider::new(
            mock.clone(),
            CircuitBreakerConfig {
                failure_threshold: 3,
                half_open_after: Duration::from_millis(10),
            },
        );

        // Trip the breaker
        for _ in 0..3 {
            let _ = cb.chat_completion(dummy_request()).await;
        }
        assert_eq!(cb.circuit_state().await, CircuitState::Open);

        // Wait for half-open transition
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Next call should go through (half-open probe) and succeed
        // because mock has exhausted its 3 failures
        let result = cb.chat_completion(dummy_request()).await;
        assert!(result.is_ok());
        assert_eq!(cb.circuit_state().await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn test_circuit_breaker_success_resets() {
        let mock = Arc::new(MockProvider::with_failures("cb-reset", 2, 500));
        let cb = CircuitBreakerProvider::new(
            mock.clone(),
            CircuitBreakerConfig {
                failure_threshold: 5,
                half_open_after: Duration::from_secs(60),
            },
        );

        // 2 failures then success
        let _ = cb.chat_completion(dummy_request()).await;
        let _ = cb.chat_completion(dummy_request()).await;
        let result = cb.chat_completion(dummy_request()).await;

        assert!(result.is_ok());
        assert_eq!(cb.circuit_state().await, CircuitState::Closed);
    }

    // -----------------------------------------------------------------------
    // ModelDegradationChain tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_degradation_uses_primary() {
        let primary = Arc::new(MockProvider::new("primary"));
        let secondary = Arc::new(MockProvider::new("secondary"));
        let fallback = Arc::new(MockProvider::new("fallback"));

        let chain =
            ModelDegradationChain::new(primary.clone(), secondary.clone(), fallback.clone());
        let result = chain.chat_completion(dummy_request()).await;

        assert!(result.is_ok());
        assert_eq!(primary.calls(), 1);
        assert_eq!(secondary.calls(), 0);
        assert_eq!(fallback.calls(), 0);
    }

    #[tokio::test]
    async fn test_degradation_falls_to_secondary() {
        let primary = Arc::new(MockProvider::with_failures("primary", 100, 500));
        let secondary = Arc::new(MockProvider::new("secondary"));
        let fallback = Arc::new(MockProvider::new("fallback"));

        let chain =
            ModelDegradationChain::new(primary.clone(), secondary.clone(), fallback.clone());
        let result = chain.chat_completion(dummy_request()).await;

        assert!(result.is_ok());
        assert_eq!(primary.calls(), 1);
        assert_eq!(secondary.calls(), 1);
        assert_eq!(fallback.calls(), 0);
    }

    #[tokio::test]
    async fn test_degradation_falls_to_fallback() {
        let primary = Arc::new(MockProvider::with_failures("primary", 100, 500));
        let secondary = Arc::new(MockProvider::with_failures("secondary", 100, 500));
        let fallback = Arc::new(MockProvider::new("fallback"));

        let chain =
            ModelDegradationChain::new(primary.clone(), secondary.clone(), fallback.clone());
        let result = chain.chat_completion(dummy_request()).await;

        assert!(result.is_ok());
        assert_eq!(primary.calls(), 1);
        assert_eq!(secondary.calls(), 1);
        assert_eq!(fallback.calls(), 1);
    }

    #[tokio::test]
    async fn test_degradation_all_fail() {
        let primary = Arc::new(MockProvider::with_failures("primary", 100, 500));
        let secondary = Arc::new(MockProvider::with_failures("secondary", 100, 500));
        let fallback = Arc::new(MockProvider::with_failures("fallback", 100, 500));

        let chain =
            ModelDegradationChain::new(primary.clone(), secondary.clone(), fallback.clone());
        let result = chain.chat_completion(dummy_request()).await;

        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // build_provider_chain tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_build_chain_no_backups() {
        let mock = Arc::new(MockProvider::new("primary"));
        let chain = build_provider_chain(mock, vec![]);
        let result = chain.chat_completion(dummy_request()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_build_chain_with_backups() {
        let primary = Arc::new(MockProvider::with_failures("primary", 100, 500));
        let backup = Arc::new(MockProvider::new("backup"));
        let chain = build_provider_chain(primary, vec![backup]);

        // The primary is wrapped with retry+circuit_breaker and will fail,
        // then failover should pick up the backup.
        let result = chain.chat_completion(dummy_request()).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // Delay calculation test
    // -----------------------------------------------------------------------

    #[test]
    fn test_delay_calculation() {
        let retry = RetryProvider::new(
            Arc::new(MockProvider::new("x")),
            RetryConfig {
                max_retries: 5,
                base_delay: Duration::from_secs(1),
                max_delay: Duration::from_secs(30),
            },
        );

        assert_eq!(retry.delay_for_attempt(0), Duration::from_secs(1));
        assert_eq!(retry.delay_for_attempt(1), Duration::from_secs(2));
        assert_eq!(retry.delay_for_attempt(2), Duration::from_secs(4));
        assert_eq!(retry.delay_for_attempt(3), Duration::from_secs(8));
        assert_eq!(retry.delay_for_attempt(4), Duration::from_secs(16));
        assert_eq!(retry.delay_for_attempt(5), Duration::from_secs(30)); // capped
    }
}
