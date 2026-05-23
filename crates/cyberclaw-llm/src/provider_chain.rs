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
use crate::credential_pool::CredentialPool;
use crate::error::{LlmError, LlmResult};
use crate::failover_reason::{classify_llm_error, LlmFailoverReason};
use crate::types::{ChatChunk, ChatRequest, ChatResponse};
use async_trait::async_trait;
use futures::stream::Stream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// LlmErrorKind — backward-compatible 2-way classifier
// ---------------------------------------------------------------------------

/// Coarse classification of LLM errors for retry decisions.
///
/// This type is kept for backward compatibility.  New code should use
/// [`LlmFailoverReason`] and [`classify_llm_error`] for semantic recovery hints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmErrorKind {
    /// Transient errors that may succeed on retry (429, 500, 503, timeout).
    Transient,
    /// Non-transient errors that should not be retried (400, 401, 403).
    NonTransient,
}

/// Convert a semantic [`LlmFailoverReason`] to the coarse [`LlmErrorKind`].
///
/// Reasons where `should_retry()` is true map to `Transient`; all others map
/// to `NonTransient`.
impl From<LlmFailoverReason> for LlmErrorKind {
    fn from(reason: LlmFailoverReason) -> Self {
        if reason.should_retry() {
            LlmErrorKind::Transient
        } else {
            LlmErrorKind::NonTransient
        }
    }
}

/// Classify an [`LlmError`] into a coarse [`LlmErrorKind`].
///
/// Delegates to [`classify_llm_error`] internally and converts via
/// `From<LlmFailoverReason>`.  Prefer using [`classify_llm_error`] directly
/// when you need the full semantic reason.
pub fn classify_error(err: &LlmError) -> LlmErrorKind {
    let (status, body) = match err {
        LlmError::Timeout => return LlmErrorKind::Transient,
        LlmError::HttpError(_) => return LlmErrorKind::Transient,
        LlmError::ApiError { status, message } => (Some(*status), message.as_str()),
        _ => (None, ""),
    };
    LlmErrorKind::from(classify_llm_error(status, body))
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
///
/// Optionally integrates with a [`CredentialPool`]: when an error's
/// [`LlmFailoverReason::should_rotate_credential()`] is true the pool's
/// next available key is retrieved via `select()` and injected into the
/// request's `api_key` override field before the next attempt.  If the
/// pool is exhausted (`rotate()` returns `false`) the error is returned
/// immediately as terminal.
pub struct RetryProvider {
    inner: Arc<dyn LlmClient>,
    config: RetryConfig,
    /// Optional credential pool for multi-key rotation.
    credential_pool: Option<Arc<CredentialPool>>,
}

impl RetryProvider {
    /// Create a new retry provider wrapping `inner`.
    pub fn new(inner: Arc<dyn LlmClient>, config: RetryConfig) -> Self {
        Self {
            inner,
            config,
            credential_pool: None,
        }
    }

    /// Attach a credential pool.  When set, billing / auth errors trigger
    /// key rotation instead of immediate failure.
    pub fn with_credential_pool(mut self, pool: Arc<CredentialPool>) -> Self {
        self.credential_pool = Some(pool);
        self
    }

    /// Compute delay for the given attempt (0-indexed).
    fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let delay = self
            .config
            .base_delay
            .saturating_mul(2u32.saturating_pow(attempt));
        std::cmp::min(delay, self.config.max_delay)
    }

    /// Try to rotate the credential pool on a rotation-triggering error.
    ///
    /// Returns `Some(true)` if rotation succeeded and a new key is available.
    /// Returns `Some(false)` if the pool is exhausted (caller must surface terminal error).
    /// Returns `None` if no pool is attached (caller should apply standard retry logic).
    fn try_rotate_pool(&self, reason: LlmFailoverReason) -> Option<bool> {
        if let Some(pool) = &self.credential_pool {
            if reason.should_rotate_credential() {
                let still_available = pool.rotate(reason);
                if !still_available {
                    tracing::warn!(
                        provider = %pool.provider,
                        "credential pool exhausted — no fresh keys available"
                    );
                }
                return Some(still_available);
            }
        }
        // No pool attached, or reason doesn't call for credential rotation
        None
    }

    /// Inject the current pool key into the request's `api_key_override` field,
    /// if a pool is present.
    fn inject_pool_key(&self, mut request: ChatRequest) -> ChatRequest {
        if let Some(pool) = &self.credential_pool {
            if let Some(key) = pool.select() {
                request.api_key_override = Some(key);
            }
        }
        request
    }
}

#[async_trait]
impl LlmClient for RetryProvider {
    async fn chat_completion(&self, request: ChatRequest) -> LlmResult<ChatResponse> {
        let mut last_err: Option<LlmError> = None;
        // Inject pool key into initial request if pool is present
        let mut current_request = self.inject_pool_key(request);

        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                if let Some(ref e) = last_err {
                    let (status, body) = match e {
                        LlmError::ApiError { status, message } => (Some(*status), message.as_str()),
                        _ => (None, ""),
                    };
                    let reason = classify_llm_error(status, body);

                    match self.try_rotate_pool(reason) {
                        Some(true) => {
                            // Pool rotated; inject new key and retry
                            current_request = self.inject_pool_key(current_request.clone());
                        }
                        Some(false) => {
                            // Pool exhausted — surface as terminal error immediately
                            return Err(LlmError::Internal(
                                "credential pool exhausted — all keys on cooldown".to_string(),
                            ));
                        }
                        None => {
                            // No pool attached — apply standard non-transient check
                            if classify_error(e) == LlmErrorKind::NonTransient {
                                return Err(last_err.unwrap());
                            }
                        }
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

            match self.inner.chat_completion(current_request.clone()).await {
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
        let mut current_request = self.inject_pool_key(request);

        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                if let Some(ref e) = last_err {
                    let (status, body) = match e {
                        LlmError::ApiError { status, message } => (Some(*status), message.as_str()),
                        _ => (None, ""),
                    };
                    let reason = classify_llm_error(status, body);

                    match self.try_rotate_pool(reason) {
                        Some(true) => {
                            current_request = self.inject_pool_key(current_request.clone());
                        }
                        Some(false) => {
                            return Err(LlmError::Internal(
                                "credential pool exhausted — all keys on cooldown".to_string(),
                            ));
                        }
                        None => {
                            if classify_error(e) == LlmErrorKind::NonTransient {
                                return Err(last_err.unwrap());
                            }
                        }
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

            match self
                .inner
                .chat_completion_stream(current_request.clone())
                .await
            {
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
            rate_limit: None,
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

    // -----------------------------------------------------------------------
    // RetryProvider + CredentialPool integration tests
    // -----------------------------------------------------------------------

    use crate::credential_pool::{CredentialPool, SelectionStrategy};

    /// Mock that fails with a billing error on the first N calls, then succeeds.
    /// Also records the api_key_override it received so tests can assert rotation.
    struct PoolMockProvider {
        name: String,
        billing_fail_count: AtomicU32,
        call_count: AtomicU32,
        received_keys: std::sync::Mutex<Vec<Option<String>>>,
    }

    impl PoolMockProvider {
        fn new_billing(name: &str, fail_n: u32) -> Self {
            Self {
                name: name.to_string(),
                billing_fail_count: AtomicU32::new(fail_n),
                call_count: AtomicU32::new(0),
                received_keys: std::sync::Mutex::new(vec![]),
            }
        }

        fn calls(&self) -> u32 {
            self.call_count.load(Ordering::SeqCst)
        }

        fn received_keys(&self) -> Vec<Option<String>> {
            self.received_keys.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl LlmClient for PoolMockProvider {
        async fn chat_completion(&self, request: ChatRequest) -> LlmResult<ChatResponse> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            self.received_keys
                .lock()
                .unwrap()
                .push(request.api_key_override.clone());
            let remaining = self.billing_fail_count.load(Ordering::SeqCst);
            if remaining > 0 {
                self.billing_fail_count.fetch_sub(1, Ordering::SeqCst);
                return Err(LlmError::ApiError {
                    status: 429,
                    // Body triggers Billing classification via pattern match
                    message: "insufficient credits for this request".to_string(),
                });
            }
            Ok(dummy_response())
        }

        async fn chat_completion_stream(
            &self,
            _request: ChatRequest,
        ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>> {
            Ok(Box::new(futures::stream::empty()))
        }

        fn provider(&self) -> &str {
            &self.name
        }

        async fn validate_connection(&self) -> LlmResult<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_retry_provider_rotates_on_billing_error() {
        // Pool with 2 keys; first call will fail with billing error, triggering rotation.
        let pool = Arc::new(CredentialPool::new(
            "anthropic",
            vec!["key-a".to_string(), "key-b".to_string()],
            SelectionStrategy::FillFirst,
        ));
        let mock = Arc::new(PoolMockProvider::new_billing("pool-mock", 1));

        let retry = RetryProvider::new(
            mock.clone(),
            RetryConfig {
                max_retries: 3,
                base_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(10),
            },
        )
        .with_credential_pool(pool.clone());

        let result = retry.chat_completion(dummy_request()).await;
        assert!(result.is_ok(), "should succeed after rotating to key-b");
        assert_eq!(mock.calls(), 2, "expected 1 billing failure + 1 success");

        // After rotation, key-a should be on cooldown
        assert_eq!(pool.available_count(), 1);
        let keys = mock.received_keys();
        // First call gets key-a (or initial select), second gets whatever pool.select() gives
        assert_eq!(keys.len(), 2);
    }

    #[tokio::test]
    async fn test_retry_provider_falls_back_on_pool_exhaustion() {
        // Pool with only 1 key; billing error exhausts the pool.
        let pool = Arc::new(CredentialPool::new(
            "anthropic",
            vec!["key-only".to_string()],
            SelectionStrategy::FillFirst,
        ));
        let mock = Arc::new(PoolMockProvider::new_billing("exhaust-mock", 10));

        let retry = RetryProvider::new(
            mock.clone(),
            RetryConfig {
                max_retries: 3,
                base_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(10),
            },
        )
        .with_credential_pool(pool.clone());

        let result = retry.chat_completion(dummy_request()).await;
        assert!(result.is_err(), "should fail when pool exhausted");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("pool exhausted") || err_msg.contains("credential"),
            "error should mention pool exhaustion, got: {err_msg}"
        );
        // Only 1 call: initial attempt fails, rotation exhausts pool, no more attempts
        assert_eq!(mock.calls(), 1);
        assert_eq!(pool.available_count(), 0);
    }

    #[tokio::test]
    async fn test_retry_provider_single_key_mode_unchanged() {
        // No credential pool attached — retry behavior is unchanged.
        let mock = Arc::new(MockProvider::with_failures("no-pool", 2, 500));
        let retry = RetryProvider::new(
            mock.clone(),
            RetryConfig {
                max_retries: 3,
                base_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(10),
            },
        );
        // No .with_credential_pool() call

        let result = retry.chat_completion(dummy_request()).await;
        assert!(result.is_ok(), "transient failures should still be retried");
        assert_eq!(mock.calls(), 3); // 1 initial + 2 retries
    }
}
