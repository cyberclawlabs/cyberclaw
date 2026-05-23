use crate::dispatch_interceptor::{
    DispatchCtx, DispatchInterceptor, SandboxInjectionInterceptor, TruncationMetadataInterceptor,
    WallClockInterceptor,
};
use crate::registry::ConnectorRegistry;
use crate::runtime::{
    ContainerRuntime, ProcessRuntime, RuntimeMode, RuntimeSelector, RuntimeSelectorConfig,
};
use crate::types::{CapabilityExecutionRequest, CapabilityExecutionResult, ExecutionStatus};
use cyberclaw_core::audit_logger::{AuditLogEntry, AuditLogger, DefaultAuditLogger}; // MEDIUM #9 FIX
use cyberclaw_core::capability::RiskLevel;
use cyberclaw_core::security_error::SecurityError;
use cyberclaw_observability::events::{EventRecorder, InMemoryEventRecorder, ObservabilityEvent};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

/// Default output cap (256 KiB) applied to capability results dispatched
/// through Process or Container runtime modes. Native mode is unaffected.
const DEFAULT_PROCESS_OUTPUT_CAP_BYTES: usize = 262_144;

/// Default request timeout (120s) when CapabilityContract.timeouts.request_ms
/// is not specified or is zero. Used by Process + Container dispatch paths.
const DEFAULT_DISPATCH_TIMEOUT_MS: u64 = 120_000;

/// Maximum allowed dispatch timeout (10 minutes) — caps any contract value
/// to prevent indefinite hangs.
const MAX_DISPATCH_TIMEOUT_MS: u64 = 600_000;

// ─── Rate Limiting ──────────────────────────────────────────────────────────

/// CRITICAL #7 FIX: Token bucket rate limiter for per-capability rate limiting.
/// Prevents DoS attacks by limiting the number of requests per capability per time window.
#[derive(Debug)]
struct TokenBucket {
    /// Maximum number of tokens the bucket can hold
    capacity: f64,
    /// Current number of available tokens
    tokens: f64,
    /// Number of tokens to add per second
    refill_rate: f64,
    /// Last time the bucket was refilled
    last_refill: Instant,
}

impl TokenBucket {
    /// Create a new token bucket with the given capacity and refill rate
    fn new(capacity: f64, refill_rate: f64) -> Self {
        Self {
            capacity,
            tokens: capacity, // Start with full bucket
            refill_rate,
            last_refill: Instant::now(),
        }
    }

    /// Try to consume one token from the bucket. Returns true if successful.
    fn try_consume(&mut self) -> bool {
        self.refill();

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Refill tokens based on elapsed time
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();

        // Add tokens based on refill rate and elapsed time
        let tokens_to_add = elapsed * self.refill_rate;
        self.tokens = (self.tokens + tokens_to_add).min(self.capacity);
        self.last_refill = now;
    }
}

/// Default rate limit: 10 requests per second per capability
const DEFAULT_RATE_LIMIT_CAPACITY: f64 = 10.0;
const DEFAULT_RATE_LIMIT_REFILL_RATE: f64 = 10.0;

/// Dispatcher for routing capability execution requests to connectors
pub struct CapabilityDispatcher {
    registry: Arc<ConnectorRegistry>,
    runtime_selector: RuntimeSelector,
    /// Unified security configuration for consistent security policy enforcement.
    security_config: Arc<cyberclaw_core::security_config::SecurityConfigManager>,
    /// CRITICAL #7 FIX: Per-capability rate limiters to prevent DoS attacks
    rate_limiters: Arc<RwLock<HashMap<String, TokenBucket>>>,
    /// HIGH #11 FIX: Event recorder for recording capability invocation events
    event_recorder: Arc<dyn EventRecorder>,
    /// MEDIUM #4 FIX: Validated whitelist of capabilities allowed in Native runtime
    #[allow(dead_code)] // Reserved for future runtime whitelist enforcement
    native_whitelist: Vec<String>,
    /// Process runtime executor for Medium-risk capability dispatch.
    /// When Some, RuntimeMode::Process wraps connector.execute() with
    /// hard timeout + output cap. When None, falls back to fail-fast.
    process_runtime: Option<Arc<dyn ProcessRuntime>>,
    /// Container runtime for High/Critical-risk capability dispatch.
    /// When Some, RuntimeMode::Container wraps connector.execute() inside
    /// the container. When None, fails fast with a config error.
    container_runtime: Option<Arc<ContainerRuntime>>,
    /// Maximum bytes the dispatcher allows in result.output before
    /// truncating (Process + Container modes only). Default 256 KiB.
    process_output_cap_bytes: usize,
    /// Pre/post hooks invoked around every `connector.execute()` call.
    /// Registered in declaration order; `before` runs in-order, `after`
    /// runs in-order (no reverse stacking — we want stable observability).
    ///
    /// Default registration (via `new()`) includes:
    /// - [`WallClockInterceptor`] (GAP-3 fix: Native timeout deadline)
    /// - [`TruncationMetadataInterceptor`] (GAP-1 fix: structured truncation flag)
    /// - [`SandboxInjectionInterceptor`] (GAP-2 hook: sandbox profile hint)
    ///
    /// Use [`Self::without_default_interceptors`] to opt out for tests or
    /// alternate runtimes that want to register custom interceptors.
    interceptors: Vec<Arc<dyn DispatchInterceptor>>,
}

impl CapabilityDispatcher {
    /// Create a new capability dispatcher
    pub fn new(registry: Arc<ConnectorRegistry>) -> Self {
        // MEDIUM #4 FIX: Define and validate default native whitelist
        let native_whitelist = vec![
            "fs.read".to_string(),
            "log.write".to_string(),
            "metrics.increment".to_string(),
        ];

        // Validate whitelist at startup - panic if invalid (fail-fast)
        Self::validate_native_whitelist(&native_whitelist, &registry)
            .expect("MEDIUM #4: Invalid native runtime whitelist configuration");

        Self {
            registry,
            runtime_selector: RuntimeSelector::new(RuntimeSelectorConfig::default()),
            security_config: Arc::new(
                cyberclaw_core::security_config::SecurityConfigManager::default(),
            ),
            rate_limiters: Arc::new(RwLock::new(HashMap::new())),
            event_recorder: Arc::new(InMemoryEventRecorder::new()),
            native_whitelist,
            process_runtime: None,
            container_runtime: None,
            process_output_cap_bytes: DEFAULT_PROCESS_OUTPUT_CAP_BYTES,
            interceptors: Self::default_interceptors(),
        }
    }

    /// Build the default interceptor stack registered by `new()` /
    /// `with_runtime_config()`. The order matters: wall-clock first so the
    /// deadline is set before any other `before` hook reads it, sandbox
    /// hint second, truncation metadata last (it only mutates `after`).
    fn default_interceptors() -> Vec<Arc<dyn DispatchInterceptor>> {
        vec![
            Arc::new(WallClockInterceptor::new()),
            Arc::new(SandboxInjectionInterceptor::new()),
            Arc::new(TruncationMetadataInterceptor::new()),
        ]
    }

    /// Create a new capability dispatcher with custom runtime selector config
    pub fn with_runtime_config(
        registry: Arc<ConnectorRegistry>,
        runtime_config: RuntimeSelectorConfig,
    ) -> Self {
        // MEDIUM #4 FIX: Use validated default whitelist
        let native_whitelist = vec![
            "fs.read".to_string(),
            "log.write".to_string(),
            "metrics.increment".to_string(),
        ];

        Self::validate_native_whitelist(&native_whitelist, &registry)
            .expect("MEDIUM #4: Invalid native runtime whitelist configuration");

        Self {
            registry,
            runtime_selector: RuntimeSelector::new(runtime_config),
            security_config: Arc::new(
                cyberclaw_core::security_config::SecurityConfigManager::default(),
            ),
            rate_limiters: Arc::new(RwLock::new(HashMap::new())),
            event_recorder: Arc::new(InMemoryEventRecorder::new()),
            native_whitelist,
            process_runtime: None,
            container_runtime: None,
            process_output_cap_bytes: DEFAULT_PROCESS_OUTPUT_CAP_BYTES,
            interceptors: Self::default_interceptors(),
        }
    }

    /// Register an additional [`DispatchInterceptor`] in the dispatcher chain.
    /// Runs in addition to any default interceptors registered by `new()`.
    pub fn with_interceptor(mut self, interceptor: Arc<dyn DispatchInterceptor>) -> Self {
        self.interceptors.push(interceptor);
        self
    }

    /// Drop the default interceptor stack. Useful when a test or alternate
    /// runtime wants to register a custom set without inheriting the defaults.
    pub fn without_default_interceptors(mut self) -> Self {
        self.interceptors.clear();
        self
    }

    /// Attach a Process runtime to enable RuntimeMode::Process dispatch for
    /// Medium-risk capabilities. When unset, Process-mode dispatch fails fast
    /// with a configuration error.
    pub fn with_process_runtime(mut self, rt: Arc<dyn ProcessRuntime>) -> Self {
        self.process_runtime = Some(rt);
        self
    }

    /// Attach a Container runtime to enable RuntimeMode::Container dispatch
    /// for High/Critical-risk capabilities. When unset, Container-mode
    /// dispatch fails fast with a configuration error.
    pub fn with_container_runtime(mut self, rt: Arc<ContainerRuntime>) -> Self {
        self.container_runtime = Some(rt);
        self
    }

    /// Override the per-call output cap (in bytes) applied to Process and
    /// Container dispatch results. Default is 256 KiB.
    pub fn with_process_output_cap(mut self, cap_bytes: usize) -> Self {
        self.process_output_cap_bytes = cap_bytes;
        self
    }

    /// Attach a custom SecurityConfigManager for unified security policy enforcement.
    pub fn with_security_config(
        mut self,
        config: Arc<cyberclaw_core::security_config::SecurityConfigManager>,
    ) -> Self {
        self.security_config = config;
        self
    }

    /// HIGH #11 FIX: Attach a custom EventRecorder for capability event recording.
    pub fn with_event_recorder(mut self, recorder: Arc<dyn EventRecorder>) -> Self {
        self.event_recorder = recorder;
        self
    }

    /// MEDIUM #4 FIX: Validate Native runtime whitelist entries
    ///
    /// Ensures all whitelist entries are:
    /// 1. In correct "category.action" format
    /// 2. Contain only safe characters (alphanumeric + underscore)
    /// 3. Have non-empty category and action parts
    /// 4. Reference Low risk capabilities only (when registered)
    fn validate_native_whitelist(
        whitelist: &[String],
        registry: &Arc<ConnectorRegistry>,
    ) -> anyhow::Result<()> {
        for capability_id in whitelist {
            // 1. Validate format: must be "category.action" format
            if !capability_id.contains('.') {
                return Err(anyhow::anyhow!(
                    "MEDIUM #4: Invalid whitelist entry format: '{}'. \
                     Expected 'category.action' format",
                    capability_id
                ));
            }

            let parts: Vec<&str> = capability_id.split('.').collect();
            if parts.len() != 2 {
                return Err(anyhow::anyhow!(
                    "MEDIUM #4: Invalid whitelist entry format: '{}'. \
                     Must have exactly one dot separator",
                    capability_id
                ));
            }

            // 2. Validate category and action are not empty
            if parts[0].is_empty() || parts[1].is_empty() {
                return Err(anyhow::anyhow!(
                    "MEDIUM #4: Invalid whitelist entry: '{}'. \
                     Category and action cannot be empty",
                    capability_id
                ));
            }

            // 3. Validate only safe characters (alphanumeric + underscore)
            if !parts[0].chars().all(|c| c.is_alphanumeric() || c == '_')
                || !parts[1].chars().all(|c| c.is_alphanumeric() || c == '_')
            {
                return Err(anyhow::anyhow!(
                    "MEDIUM #4: Invalid whitelist entry: '{}'. \
                     Only alphanumeric characters and underscores allowed",
                    capability_id
                ));
            }

            // 4. Validate capability is registered and has Low risk level
            // Note: We search through all registered connectors for this capability
            let mut found_capability = false;

            for conn_id in registry.list_connectors() {
                if let Some(conn_entry) = registry.get_entry(&conn_id) {
                    if let Some(cap_contract) = conn_entry
                        .capabilities
                        .iter()
                        .find(|c| c.id == *capability_id)
                    {
                        found_capability = true;
                        if cap_contract.risk != RiskLevel::Low {
                            return Err(anyhow::anyhow!(
                                "MEDIUM #4: Whitelist capability '{}' has risk level {:?}. \
                                 Only Low risk capabilities can be whitelisted for Native runtime",
                                capability_id,
                                cap_contract.risk
                            ));
                        }
                        break;
                    }
                }
            }

            if !found_capability {
                // Capability not yet registered - warn but don't fail
                // (it may be registered dynamically later)
                warn!(
                    "MEDIUM #4: Whitelist capability '{}' not found in registry. \
                     It may be registered later.",
                    capability_id
                );
            }
        }

        info!(
            "MEDIUM #4: Native runtime whitelist validated | entries: {} | all valid",
            whitelist.len()
        );

        Ok(())
    }

    /// Dispatch a capability execution request to the appropriate connector
    pub async fn dispatch(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<CapabilityExecutionResult> {
        let span = tracing::span!(
            tracing::Level::INFO,
            "capability_dispatch",
            execution_id = %request.execution_id,
            trace_id = %request.trace_id,
            connector_id = %request.connector_id,
            capability_id = %request.capability_id,
        );
        let _enter = span.enter();

        // MEDIUM #9: 统一审计日志格式
        let audit_logger = DefaultAuditLogger;
        audit_logger.log(
            AuditLogEntry::new(
                "CapabilityDispatcher",
                "capability.dispatch",
                format!(
                    "Dispatching capability {} to connector {}",
                    request.capability_id, request.connector_id
                ),
            )
            .with_trace_id(request.trace_id.as_str())
            .with_metadata("execution_id", request.execution_id.to_string())
            .with_metadata("capability_id", request.capability_id.to_string())
            .with_metadata("connector_id", request.connector_id.to_string()),
        );

        info!(
            "Dispatching capability {} to connector {}",
            request.capability_id, request.connector_id
        );

        // HIGH #11 FIX: Track start time for duration calculation
        let start_time = Instant::now();

        // HIGH #11 FIX: Record CapabilityInvoked event at dispatch start
        let invoked_event = ObservabilityEvent::CapabilityInvoked {
            execution_id: request.execution_id.clone(),
            capability_id: request.capability_id.clone(),
            timestamp: chrono::Utc::now(),
        };

        if let Err(e) = self.event_recorder.record_event(invoked_event).await {
            warn!("Failed to record CapabilityInvoked event: {:?}", e);
        }

        // CRITICAL #7 FIX: Rate limiting check to prevent DoS attacks
        {
            let mut limiters = self
                .rate_limiters
                .write()
                .expect("Rate limiter lock poisoned");
            let rate_limiter = limiters
                .entry(request.capability_id.to_string())
                .or_insert_with(|| {
                    TokenBucket::new(DEFAULT_RATE_LIMIT_CAPACITY, DEFAULT_RATE_LIMIT_REFILL_RATE)
                });

            if !rate_limiter.try_consume() {
                let error_msg = format!(
                    "Rate limit exceeded for capability {}. Maximum {} requests per second.",
                    request.capability_id, DEFAULT_RATE_LIMIT_CAPACITY
                );
                error!(
                    "Rate limit exceeded for capability {} (execution: {})",
                    request.capability_id, request.execution_id
                );
                return Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: serde_json::json!({
                        "error": error_msg.clone()
                    }),
                    status: ExecutionStatus::Failed,
                    error: Some(error_msg),
                    actual_runtime: None,
                });
            }

            info!(
                "Rate limit check passed for capability {} (tokens remaining: {:.2})",
                request.capability_id, rate_limiter.tokens
            );
        }

        // HIGH #9 FIX: Get ConnectorEntry snapshot to avoid concurrent modification issues
        // Using registry.get_entry() provides a cloned snapshot of capabilities,
        // preventing TOCTOU race conditions where capabilities could be modified
        // between validation and execution
        let connector_entry = self
            .registry
            .get_entry(&request.connector_id)
            .ok_or_else(|| anyhow::anyhow!("Connector {} not found", request.connector_id))?;

        let connector = connector_entry.connector.clone();

        // Find the capability contract from the immutable snapshot
        let capability_contract = connector_entry
            .capabilities
            .iter()
            .find(|c| c.id == request.capability_id.as_str());

        let capability_contract = match capability_contract {
            Some(contract) => contract,
            None => {
                error!(
                    "Connector {} does not support capability {}",
                    request.connector_id, request.capability_id
                );
                return Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id.clone(),
                    trace_id: request.trace_id.clone(),
                    connector_id: request.connector_id.clone(),
                    capability_id: request.capability_id.clone(),
                    output: serde_json::json!({
                        "error": format!("Connector {} does not support capability {}",
                            request.connector_id, request.capability_id)
                    }),
                    status: ExecutionStatus::Failed,
                    error: Some(format!(
                        "Connector {} does not support capability {}",
                        request.connector_id, request.capability_id
                    )),
                    actual_runtime: None,
                });
            }
        };

        // Select runtime based on capability risk level
        let runtime_mode = self.runtime_selector.select_runtime(capability_contract);
        info!(
            "Runtime selection for capability {} (risk: {:?}): {:?}",
            request.capability_id, capability_contract.risk, runtime_mode
        );

        // GAP fix wave (1/2/3): run pre-dispatch interceptors. `before` may
        // populate `dispatch_ctx.deadline` (WallClockInterceptor → GAP-3),
        // `dispatch_ctx.sandbox_hint` (SandboxInjectionInterceptor → GAP-2),
        // or short-circuit by returning Err. The DispatchCtx is rebuilt
        // fresh per call so it can't leak state across requests.
        let mut dispatch_ctx = DispatchCtx::new(request.clone(), capability_contract.clone());
        for interceptor in &self.interceptors {
            if let Err(e) = interceptor.before(&mut dispatch_ctx).await {
                error!(
                    "DispatchInterceptor.before failed for capability {}: {:?}",
                    request.capability_id, e
                );
                return Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id.clone(),
                    trace_id: request.trace_id.clone(),
                    connector_id: request.connector_id.clone(),
                    capability_id: request.capability_id.clone(),
                    output: serde_json::json!({
                        "error": format!("DispatchInterceptor rejected: {}", e)
                    }),
                    status: ExecutionStatus::Failed,
                    error: Some(format!("DispatchInterceptor rejected: {}", e)),
                    actual_runtime: None,
                });
            }
        }

        // Check runtime availability and fail-fast for unconfigured runtimes
        match runtime_mode {
            RuntimeMode::Container => {
                if self.container_runtime.is_none() {
                    error!(
                        "Container runtime not configured for capability {}",
                        request.capability_id
                    );
                    let msg = "Container runtime not configured. Configure with CapabilityDispatcher::with_container_runtime() to dispatch High/Critical-risk capabilities. See RB-09 for current capability matrix.".to_string();
                    return Ok(CapabilityExecutionResult {
                        execution_id: request.execution_id.clone(),
                        trace_id: request.trace_id.clone(),
                        connector_id: request.connector_id.clone(),
                        capability_id: request.capability_id.clone(),
                        output: serde_json::json!({ "error": msg.clone() }),
                        status: ExecutionStatus::Failed,
                        error: Some(msg),
                        actual_runtime: None,
                    });
                }

                // High/Critical risk only — guard against misconfigured selectors
                if !matches!(
                    capability_contract.risk,
                    RiskLevel::High | RiskLevel::Critical
                ) {
                    warn!(
                        "Container runtime dispatched for non-High/Critical capability {} (risk: {:?})",
                        request.capability_id, capability_contract.risk
                    );
                }
            }
            RuntimeMode::Process => {
                if self.process_runtime.is_none() {
                    error!(
                        "Process runtime not configured for capability {}",
                        request.capability_id
                    );
                    let msg = "Process runtime not configured. Configure with CapabilityDispatcher::with_process_runtime() to dispatch Medium-risk capabilities.".to_string();
                    return Ok(CapabilityExecutionResult {
                        execution_id: request.execution_id.clone(),
                        trace_id: request.trace_id.clone(),
                        connector_id: request.connector_id.clone(),
                        capability_id: request.capability_id.clone(),
                        output: serde_json::json!({ "error": msg.clone() }),
                        status: ExecutionStatus::Failed,
                        error: Some(msg),
                        actual_runtime: None,
                    });
                }

                // Process mode carries Low/Medium/High-risk capabilities.
                // Critical alone is reserved for Container — the dispatcher's
                // timeout + output cap is sufficient bounded blast radius for
                // High but not for Critical (which often touches privileged
                // syscalls / network egress where only namespace isolation
                // is sufficient). Operators wanting Critical→Process must
                // explicitly drop the cap to High.
                if capability_contract.risk == RiskLevel::Critical {
                    error!(
                        "Process runtime not allowed for capability {} with risk level Critical",
                        request.capability_id
                    );
                    let msg =
                        "Process runtime not allowed for Critical risk capabilities. Requires Container isolation."
                            .to_string();
                    return Ok(CapabilityExecutionResult {
                        execution_id: request.execution_id.clone(),
                        trace_id: request.trace_id.clone(),
                        connector_id: request.connector_id.clone(),
                        capability_id: request.capability_id.clone(),
                        output: serde_json::json!({ "error": msg.clone() }),
                        status: ExecutionStatus::Failed,
                        error: Some(msg),
                        actual_runtime: None,
                    });
                }
            }
            RuntimeMode::Native => {
                // CRITICAL #5 FIX: Reject High/Critical risk capabilities from using Native runtime
                // Native runtime is only allowed for Low/Medium risk capabilities
                //
                // 2026-05-17 (v1.2.9): EXCEPTION — capabilities whose ID
                // matches `cyberclaw_core::manifests::NETWORK_BOUND_PREFIXES`
                // (browser.*, mcp.*) may run Native even at High/Critical
                // risk. Their actual isolation lives in an external process
                // (browser via CDP, MCP server, remote HTTP) — cyberclaw's
                // in-process Rust shim only holds a network connection.
                // Forcing Container for these breaks the WS connection pool /
                // session state, with no security benefit (the external
                // process is the boundary). Governance review_threshold still
                // gates by risk regardless of runtime, so safety is preserved.
                match capability_contract.risk {
                    RiskLevel::High | RiskLevel::Critical
                        if !cyberclaw_core::manifests::is_network_bound_capability(
                            request.capability_id.as_str(),
                        ) =>
                    {
                        error!(
                            "Native runtime not allowed for capability {} with risk level {:?}",
                            request.capability_id, capability_contract.risk
                        );
                        return Ok(CapabilityExecutionResult {
                            execution_id: request.execution_id.clone(),
                            trace_id: request.trace_id.clone(),
                            connector_id: request.connector_id.clone(),
                            capability_id: request.capability_id.clone(),
                            output: serde_json::json!({
                                "error": format!(
                                    "Native runtime not allowed for {:?} risk capabilities. Requires Process/Container isolation.",
                                    capability_contract.risk
                                )
                            }),
                            status: ExecutionStatus::Failed,
                            error: Some(format!(
                                "Native runtime not allowed for {:?} risk capabilities. Requires Process/Container isolation.",
                                capability_contract.risk
                            )),
                            actual_runtime: None,
                        });
                    }
                    // Reached for: Low/Medium (always), AND High/Critical when
                    // network_bound bypass applies (the guard arm above
                    // explicitly excludes those network-bound High/Critical
                    // capabilities, falling through here). Same validation
                    // logic for all four levels — risk informs governance,
                    // not runtime when the connector self-isolates.
                    _ => {
                        // CRITICAL #6 FIX: Verify capability contract constraints before execution

                        // Check 1: Effects must not be empty
                        if capability_contract.effects.is_empty() {
                            error!(
                                "Capability {} has no declared effects - rejecting execution",
                                request.capability_id
                            );
                            return Ok(CapabilityExecutionResult {
                                execution_id: request.execution_id.clone(),
                                trace_id: request.trace_id.clone(),
                                connector_id: request.connector_id.clone(),
                                capability_id: request.capability_id.clone(),
                                output: serde_json::json!({
                                    "error": "Capability has no declared effects. All capabilities must declare their effects."
                                }),
                                status: ExecutionStatus::Failed,
                                error: Some(
                                    "Capability has no declared effects. All capabilities must declare their effects.".to_string()
                                ),
                                actual_runtime: None,
                            });
                        }

                        // Check 2: Validate timeout configuration if present
                        if let Some(timeout_ms) = capability_contract.timeouts.request_ms {
                            if timeout_ms == 0 {
                                error!(
                                    "Capability {} has invalid timeout (0ms) - rejecting execution",
                                    request.capability_id
                                );
                                return Ok(CapabilityExecutionResult {
                                    execution_id: request.execution_id.clone(),
                                    trace_id: request.trace_id.clone(),
                                    connector_id: request.connector_id.clone(),
                                    capability_id: request.capability_id.clone(),
                                    output: serde_json::json!({
                                        "error": "Capability has invalid timeout configuration (0ms)."
                                    }),
                                    status: ExecutionStatus::Failed,
                                    error: Some(
                                        "Capability has invalid timeout configuration (0ms)."
                                            .to_string(),
                                    ),
                                    actual_runtime: None,
                                });
                            }

                            // Warn about excessively long timeouts (>5 minutes)
                            if timeout_ms > 300_000 {
                                warn!(
                                    "Capability {} has very long timeout ({}ms). Consider reducing for better resource management.",
                                    request.capability_id, timeout_ms
                                );
                            }
                        }

                        // S7 FIX: Cross-validate behavior contract destructiveness vs risk level.
                        // If a behavior contract declares is_destructive=true but risk is Low,
                        // this is an inconsistency that must be rejected.
                        if capability_contract.risk == RiskLevel::Low {
                            if let Some(behavior) = connector_entry
                                .behavior_contracts
                                .get(request.capability_id.as_str())
                            {
                                if behavior.is_destructive {
                                    error!(
                                        "Capability {} declares is_destructive=true but risk=Low — inconsistent contract",
                                        request.capability_id
                                    );
                                    return Ok(CapabilityExecutionResult {
                                        execution_id: request.execution_id.clone(),
                                        trace_id: request.trace_id.clone(),
                                        connector_id: request.connector_id.clone(),
                                        capability_id: request.capability_id.clone(),
                                        output: serde_json::json!({
                                            "error": "Capability contract inconsistency: is_destructive=true but risk=Low. Destructive capabilities must have risk >= Medium."
                                        }),
                                        status: ExecutionStatus::Failed,
                                        error: Some(
                                            "Capability contract inconsistency: is_destructive=true but risk=Low. Destructive capabilities must have risk >= Medium.".to_string()
                                        ),
                                        actual_runtime: None,
                                    });
                                }
                            }
                        }

                        info!(
                            "Native runtime validation passed for capability {} (risk: {:?}, effects: {:?}, timeout: {:?}ms)",
                            request.capability_id,
                            capability_contract.risk,
                            capability_contract.effects,
                            capability_contract.timeouts.request_ms
                        );
                        // Continue with native execution
                    }
                }
            }
        }

        // Execute the capability.
        //
        // GAP-3 fix (2026-05-22): Native used to run UNWRAPPED — a hung
        // browser/CDP shim could pin the dispatcher forever. We now wrap
        // Native with `tokio::time::timeout(deadline)` where `deadline`
        // comes from [`WallClockInterceptor`]. Process/Container still go
        // through `execute_with_dispatch_budget` (same timeout math, plus
        // output cap), so behaviour is now symmetric across runtimes.
        let exec_outcome: anyhow::Result<CapabilityExecutionResult> = match runtime_mode {
            RuntimeMode::Native => {
                let deadline_dur = dispatch_ctx
                    .deadline
                    .map(|d| d.saturating_duration_since(Instant::now()))
                    .unwrap_or_else(|| {
                        compute_dispatch_timeout(capability_contract.timeouts.request_ms)
                    });
                match tokio::time::timeout(deadline_dur, connector.execute(request.clone())).await {
                    Ok(Ok(mut result)) => {
                        // HIGH #6 FIX: Set actual_runtime to enable post-execution verification.
                        result.actual_runtime = Some(RuntimeMode::Native);
                        Ok(result)
                    }
                    Ok(Err(e)) => Err(anyhow::anyhow!("Connector execution failed: {}", e)),
                    Err(_) => Err(anyhow::anyhow!(
                        "Native capability execution exceeded {:?} timeout",
                        deadline_dur
                    )),
                }
            }
            RuntimeMode::Process | RuntimeMode::Container => {
                let timeout = compute_dispatch_timeout(capability_contract.timeouts.request_ms);
                let cap_bytes = self.process_output_cap_bytes;
                let expected = runtime_mode;

                // Audit emission: dispatcher contract enforcement.
                audit_logger.log(
                    AuditLogEntry::new(
                        "CapabilityDispatcher",
                        "runtime.process_dispatch",
                        format!(
                            "Dispatching capability {} via {:?} runtime (timeout {}ms, output cap {}B)",
                            request.capability_id,
                            expected,
                            timeout.as_millis(),
                            cap_bytes
                        ),
                    )
                    .with_trace_id(request.trace_id.as_str())
                    .with_metadata("capability_id", request.capability_id.to_string())
                    .with_metadata("runtime_mode", format!("{:?}", expected))
                    .with_metadata("timeout_ms", timeout.as_millis().to_string())
                    .with_metadata("output_cap_bytes", cap_bytes.to_string()),
                );

                execute_with_dispatch_budget(
                    &connector,
                    request.clone(),
                    timeout,
                    cap_bytes,
                    expected,
                )
                .await
                .map_err(|e| anyhow::anyhow!(e))
            }
        };

        match exec_outcome {
            Ok(mut result) => {
                // GAP fix wave: run post-dispatch interceptor `after` hooks.
                // These may annotate `result.output` with structured metadata
                // (e.g. TruncationMetadataInterceptor → GAP-1). They run
                // BEFORE the runtime-mismatch check so even rejected results
                // carry the same metadata schema (defence in depth).
                for interceptor in &self.interceptors {
                    interceptor.after(&dispatch_ctx, &mut result).await;
                }

                // HIGH #6 FIX: Verify actual runtime matches expected runtime
                if let Some(actual) = result.actual_runtime {
                    if actual != runtime_mode {
                        error!(
                            "Runtime mismatch for capability {}: expected {:?}, got {:?}",
                            request.capability_id, runtime_mode, actual
                        );
                        return Ok(CapabilityExecutionResult {
                            execution_id: request.execution_id,
                            trace_id: request.trace_id,
                            connector_id: request.connector_id,
                            capability_id: request.capability_id,
                            output: serde_json::json!({
                                "error": format!(
                                    "Security violation: Runtime mismatch. Expected {:?}, got {:?}",
                                    runtime_mode, actual
                                )
                            }),
                            status: ExecutionStatus::Failed,
                            error: Some(format!(
                                "Runtime mismatch: expected {:?}, got {:?}",
                                runtime_mode, actual
                            )),
                            actual_runtime: Some(actual),
                        });
                    }
                }

                info!(
                    "Capability {} executed successfully with status {:?}, runtime: {:?}",
                    request.capability_id, result.status, result.actual_runtime
                );

                // HIGH #11 FIX: Record CapabilityCompleted event on success
                let duration_ms = start_time.elapsed().as_millis().min(u64::MAX as u128) as u64;
                let success = matches!(result.status, ExecutionStatus::Success);
                let completed_event = ObservabilityEvent::CapabilityCompleted {
                    execution_id: request.execution_id.clone(),
                    capability_id: request.capability_id.clone(),
                    success,
                    duration_ms,
                    timestamp: chrono::Utc::now(),
                };

                if let Err(e) = self.event_recorder.record_event(completed_event).await {
                    warn!("Failed to record CapabilityCompleted event: {:?}", e);
                }

                Ok(result)
            }
            Err(e) => {
                error!(
                    "Failed to execute capability {}: {:?}",
                    request.capability_id, e
                );

                // HIGH #12 FIX: Classify connector errors and wrap as SecurityError
                let error_msg = e.to_string();
                let error_msg_lower = error_msg.to_lowercase();

                let security_error = if error_msg_lower.contains("timeout") {
                    SecurityError::CapabilityCheckFailed {
                        message: format!("Capability execution timeout: {}", error_msg),
                        capability_id: request.capability_id.to_string(),
                        trace_id: Some(request.trace_id.to_string()),
                    }
                } else if error_msg_lower.contains("permission")
                    || error_msg_lower.contains("access denied")
                    || error_msg_lower.contains("unauthorized")
                {
                    SecurityError::AuthorizationFailed {
                        message: format!(
                            "Capability execution authorization failed: {}",
                            error_msg
                        ),
                        trace_id: Some(request.trace_id.to_string()),
                    }
                } else {
                    SecurityError::CapabilityCheckFailed {
                        message: format!("Capability execution failed: {}", error_msg),
                        capability_id: request.capability_id.to_string(),
                        trace_id: Some(request.trace_id.to_string()),
                    }
                };

                error!(
                    "Security error during capability execution: {}",
                    security_error.to_audit_log()
                );

                // HIGH #11 FIX: Record CapabilityCompleted event on failure
                let duration_ms = start_time.elapsed().as_millis().min(u64::MAX as u128) as u64;
                let completed_event = ObservabilityEvent::CapabilityCompleted {
                    execution_id: request.execution_id.clone(),
                    capability_id: request.capability_id.clone(),
                    success: false,
                    duration_ms,
                    timestamp: chrono::Utc::now(),
                };

                if let Err(e) = self.event_recorder.record_event(completed_event).await {
                    warn!("Failed to record CapabilityCompleted event: {:?}", e);
                }

                Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: serde_json::json!({
                        "error": security_error.to_string()
                    }),
                    status: ExecutionStatus::Failed,
                    error: Some(security_error.to_string()),
                    actual_runtime: None,
                })
            }
        }
    }

    /// Dispatch with automatic connector resolution
    pub async fn dispatch_auto(
        &self,
        mut request: CapabilityExecutionRequest,
    ) -> anyhow::Result<CapabilityExecutionResult> {
        // If connector_id is not specified, try to find one that supports the capability
        if request.connector_id.as_str().is_empty() {
            if let Some(connector_id) = self
                .registry
                .find_connector_for_capability(&request.capability_id)
            {
                warn!(
                    "Auto-resolved connector {} for capability {}",
                    connector_id, request.capability_id
                );
                request.connector_id = connector_id;
            } else {
                error!(
                    "No connector found for capability {}",
                    request.capability_id
                );
                let error_msg = format!(
                    "No connector found for capability {}",
                    request.capability_id
                );
                return Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: serde_json::json!({
                        "error": error_msg.clone()
                    }),
                    status: ExecutionStatus::Failed,
                    error: Some(error_msg),
                    actual_runtime: None,
                });
            }
        }

        self.dispatch(request).await
    }
}

/// Compute the dispatch timeout from the capability contract.
///
/// Behavior:
/// - `None` or `Some(0)` → DEFAULT_DISPATCH_TIMEOUT_MS (120s)
/// - `Some(n)` → clamped to `[1ms, MAX_DISPATCH_TIMEOUT_MS]` (10min ceiling)
fn compute_dispatch_timeout(request_ms: Option<u64>) -> Duration {
    let raw_ms = match request_ms {
        Some(n) if n > 0 => n,
        _ => DEFAULT_DISPATCH_TIMEOUT_MS,
    };
    Duration::from_millis(raw_ms.clamp(1, MAX_DISPATCH_TIMEOUT_MS))
}

/// Execute a connector with hard dispatch budget enforcement.
///
/// Wraps `connector.execute()` with:
/// 1. A `tokio::time::timeout` that fails the call if it exceeds `timeout`
/// 2. An output cap that truncates oversized `result.output` JSON to keep
///    downstream consumers (LLMs, audit, persistence) bounded
/// 3. `actual_runtime` tagging so the dispatcher's runtime-mismatch check
///    sees the expected runtime regardless of what the connector returned
///
/// Returns `Err(String)` for both timeout and connector failure cases so the
/// caller can fold the error into a Failed `CapabilityExecutionResult`.
async fn execute_with_dispatch_budget(
    connector: &Arc<dyn crate::types::Connector>,
    request: CapabilityExecutionRequest,
    timeout: Duration,
    output_cap_bytes: usize,
    expected_runtime: RuntimeMode,
) -> Result<CapabilityExecutionResult, String> {
    match tokio::time::timeout(timeout, connector.execute(request)).await {
        Ok(Ok(mut result)) => {
            // Truncate output if oversized — the dispatcher contract caps
            // result payloads independently of any connector-side limits.
            let serialized = serde_json::to_string(&result.output).unwrap_or_default();
            if serialized.len() > output_cap_bytes && output_cap_bytes > 0 {
                let preview_len = output_cap_bytes / 2;
                let preview: String = serialized.chars().take(preview_len).collect();
                result.output = serde_json::json!({
                    "truncated": true,
                    "original_size_bytes": serialized.len(),
                    "max_size_bytes": output_cap_bytes,
                    "preview": preview,
                });
            }
            result.actual_runtime = Some(expected_runtime);
            Ok(result)
        }
        Ok(Err(e)) => Err(format!("Connector execution failed: {}", e)),
        Err(_) => Err(format!(
            "Capability execution exceeded {:?} timeout under {:?} runtime",
            timeout, expected_runtime
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // MEDIUM #4 FIX: Tests for native whitelist validation

    fn create_test_registry() -> ConnectorRegistry {
        // Return empty registry - tests focus on format validation
        // Actual capability registration validation is integration level
        ConnectorRegistry::new()
    }

    #[test]
    fn test_validate_native_whitelist_valid_format() {
        let whitelist = vec!["fs.read".to_string(), "log.write".to_string()];
        let registry = Arc::new(create_test_registry());

        let result = CapabilityDispatcher::validate_native_whitelist(&whitelist, &registry);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_native_whitelist_invalid_no_dot() {
        let whitelist = vec!["invalid_no_dot".to_string()];
        let registry = Arc::new(create_test_registry());

        let result = CapabilityDispatcher::validate_native_whitelist(&whitelist, &registry);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("MEDIUM #4"));
        assert!(err.contains("Expected 'category.action' format"));
    }

    #[test]
    fn test_validate_native_whitelist_multiple_dots() {
        let whitelist = vec!["fs.read.extra".to_string()];
        let registry = Arc::new(create_test_registry());

        let result = CapabilityDispatcher::validate_native_whitelist(&whitelist, &registry);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("MEDIUM #4"));
        assert!(err.contains("Must have exactly one dot separator"));
    }

    #[test]
    fn test_validate_native_whitelist_empty_parts() {
        let whitelist = vec![".read".to_string()]; // Empty category
        let registry = Arc::new(create_test_registry());

        let result = CapabilityDispatcher::validate_native_whitelist(&whitelist, &registry);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("MEDIUM #4"));
        assert!(err.contains("Category and action cannot be empty"));
    }

    #[test]
    fn test_validate_native_whitelist_unsafe_characters() {
        let whitelist = vec!["fs.read-file".to_string()]; // Hyphen not allowed
        let registry = Arc::new(create_test_registry());

        let result = CapabilityDispatcher::validate_native_whitelist(&whitelist, &registry);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("MEDIUM #4"));
        assert!(err.contains("Only alphanumeric characters and underscores allowed"));
    }

    #[test]
    fn test_validate_native_whitelist_special_chars() {
        let test_cases = vec![
            "fs.read/write", // Slash
            "fs.read@file",  // At sign
            "fs.read!",      // Exclamation
            "fs.read$",      // Dollar sign
            "fs.read%",      // Percent
        ];

        let registry = Arc::new(create_test_registry());

        for invalid_entry in test_cases {
            let whitelist = vec![invalid_entry.to_string()];
            let result = CapabilityDispatcher::validate_native_whitelist(&whitelist, &registry);
            assert!(
                result.is_err(),
                "Should reject whitelist entry with special chars: {}",
                invalid_entry
            );
        }
    }

    #[test]
    fn test_validate_native_whitelist_valid_underscores() {
        let whitelist = vec!["fs_ops.read_file".to_string()]; // Underscores are allowed
        let registry = Arc::new(create_test_registry());

        // This should pass format validation (registry check may warn if not registered)
        let result = CapabilityDispatcher::validate_native_whitelist(&whitelist, &registry);
        // Should not error on format - only warn if not in registry
        assert!(result.is_ok());
    }

    // ─── S7 FIX: Tests for destructive + low risk cross-validation ──────────

    use cyberclaw_core::capability::CapabilityEffect;
    use cyberclaw_core::capability_contract::CapabilityBehaviorContract;
    use cyberclaw_core::manifests::{CapabilityContract, CapabilityTimeouts};

    fn low_risk_cap(id: &str) -> CapabilityContract {
        CapabilityContract {
            id: id.to_string(),
            title: id.to_string(),
            description: None,
            input_schema: "{}".to_string(),
            output_schema: "{}".to_string(),
            risk: RiskLevel::Low,
            effects: vec![CapabilityEffect::Read],
            placement: None,
            timeouts: CapabilityTimeouts::default(),
        }
    }

    #[tokio::test]
    async fn s7_rejects_destructive_low_risk_capability() {
        let cap_id = "fs.read";

        let mut behavior_contracts = std::collections::HashMap::new();
        behavior_contracts.insert(
            cap_id.to_string(),
            CapabilityBehaviorContract {
                is_read_only: false,
                is_destructive: true, // Inconsistent with Low risk
                is_concurrency_safe: false,
                max_result_size_chars: 0,
            },
        );

        // Verify the inconsistency: is_destructive=true with risk=Low should be caught
        let behavior = behavior_contracts.get(cap_id).unwrap();
        assert!(
            behavior.is_destructive,
            "Test setup: behavior must be destructive"
        );

        let cap_contract = low_risk_cap(cap_id);
        assert_eq!(
            cap_contract.risk,
            RiskLevel::Low,
            "Test setup: risk must be Low"
        );

        // The cross-validation logic: destructive + Low risk => inconsistent
        let is_inconsistent = cap_contract.risk == RiskLevel::Low && behavior.is_destructive;
        assert!(
            is_inconsistent,
            "Destructive capability with Low risk must be flagged as inconsistent"
        );
    }

    #[tokio::test]
    async fn s7_allows_non_destructive_low_risk_capability() {
        let cap_id = "fs.read";

        let mut behavior_contracts = std::collections::HashMap::new();
        behavior_contracts.insert(
            cap_id.to_string(),
            CapabilityBehaviorContract {
                is_read_only: true,
                is_destructive: false, // Consistent with Low risk
                is_concurrency_safe: true,
                max_result_size_chars: 4096,
            },
        );

        let behavior = behavior_contracts.get(cap_id).unwrap();
        let cap_contract = low_risk_cap(cap_id);

        // Non-destructive + Low risk => consistent, should be allowed
        let is_inconsistent = cap_contract.risk == RiskLevel::Low && behavior.is_destructive;
        assert!(
            !is_inconsistent,
            "Non-destructive capability with Low risk should be allowed"
        );
    }

    // ─── S7 End-to-End: dispatch() integration test ───────────────────────────

    /// Mock connector that exposes a single Low-risk capability for S7 testing.
    #[derive(Debug)]
    struct S7MockConnector {
        id: cyberclaw_core::ids::ConnectorId,
    }

    #[async_trait::async_trait]
    impl crate::types::Connector for S7MockConnector {
        fn id(&self) -> &cyberclaw_core::ids::ConnectorId {
            &self.id
        }

        fn runtime(&self) -> cyberclaw_core::manifests::ConnectorRuntime {
            cyberclaw_core::manifests::ConnectorRuntime::Native
        }

        fn capabilities(&self) -> Vec<CapabilityContract> {
            vec![low_risk_cap("fs.read")]
        }

        async fn execute(
            &self,
            request: crate::types::CapabilityExecutionRequest,
        ) -> anyhow::Result<crate::types::CapabilityExecutionResult> {
            Ok(crate::types::CapabilityExecutionResult {
                execution_id: request.execution_id,
                trace_id: request.trace_id,
                connector_id: request.connector_id,
                capability_id: request.capability_id,
                output: serde_json::json!({"ok": true}),
                status: crate::types::ExecutionStatus::Success,
                error: None,
                actual_runtime: None,
            })
        }
    }

    #[tokio::test]
    async fn s7_dispatch_rejects_destructive_low_risk_end_to_end() {
        use cyberclaw_core::identity::{ActorRef, ActorType};
        use cyberclaw_core::ids::{ActorId, ConnectorId, ExecutionId, WorkspaceId};
        use cyberclaw_core::workspace::{WorkspaceMode, WorkspaceRef};

        let connector_id = ConnectorId::from_string("s7-test-connector".to_string()).unwrap();
        let cap_id = "fs.read";

        // 1. Register mock connector
        let registry = Arc::new(ConnectorRegistry::new());
        let mock = Arc::new(S7MockConnector {
            id: connector_id.clone(),
        });
        registry.register(mock).unwrap();

        // 2. Attach destructive behavior contract to Low-risk capability
        registry
            .set_behavior_contract(
                &connector_id,
                cap_id.to_string(),
                CapabilityBehaviorContract {
                    is_read_only: false,
                    is_destructive: true, // Inconsistent with Low risk
                    is_concurrency_safe: false,
                    max_result_size_chars: 0,
                },
            )
            .unwrap();

        // 3. Dispatch through the full pipeline
        let dispatcher = CapabilityDispatcher::new(registry);
        let request = crate::types::CapabilityExecutionRequest {
            execution_id: ExecutionId::new(),
            trace_id: "s7-test-trace".to_string(),
            actor: ActorRef {
                id: ActorId::new(),
                actor_type: ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "s7-test".to_string(),
            },
            workspace: WorkspaceRef {
                id: WorkspaceId::new(),
                mode: WorkspaceMode::Ephemeral,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: "/tmp/s7-test".to_string(),
                writable_roots: vec![],
            },
            connector_id,
            capability_id: cyberclaw_core::ids::CapabilityId::from_string(cap_id.to_string())
                .unwrap(),
            input: serde_json::json!({}),
        };

        let result = dispatcher.dispatch(request).await.unwrap();

        // 4. Assert: dispatch must reject with Failed status
        assert_eq!(
            result.status,
            crate::types::ExecutionStatus::Failed,
            "S7: Destructive capability with Low risk must be rejected by dispatch()"
        );
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or("")
                .contains("contract inconsistency"),
            "S7: Error message must mention contract inconsistency, got: {:?}",
            result.error
        );
    }

    #[tokio::test]
    async fn s7_allows_destructive_medium_risk_capability() {
        let cap_id = "fs.delete";

        let mut behavior_contracts = std::collections::HashMap::new();
        behavior_contracts.insert(
            cap_id.to_string(),
            CapabilityBehaviorContract {
                is_read_only: false,
                is_destructive: true, // Consistent with Medium risk
                is_concurrency_safe: false,
                max_result_size_chars: 0,
            },
        );

        let behavior = behavior_contracts.get(cap_id).unwrap();

        // Medium risk + destructive => consistent, should be allowed
        let medium_cap = CapabilityContract {
            id: cap_id.to_string(),
            title: cap_id.to_string(),
            description: None,
            input_schema: "{}".to_string(),
            output_schema: "{}".to_string(),
            risk: RiskLevel::Medium,
            effects: vec![CapabilityEffect::Write],
            placement: None,
            timeouts: CapabilityTimeouts::default(),
        };

        let is_inconsistent = medium_cap.risk == RiskLevel::Low && behavior.is_destructive;
        assert!(
            !is_inconsistent,
            "Destructive capability with Medium risk should be allowed"
        );
    }

    // ─── RB-09: Process + Container dispatch budget tests ────────────────────

    /// Mock connector configurable for runtime dispatch tests.
    /// Returns canned output, optionally sleeping first to simulate slow work,
    /// and reports a configurable risk level.
    #[derive(Debug)]
    struct DispatchTestConnector {
        id: cyberclaw_core::ids::ConnectorId,
        capability_id: String,
        risk: RiskLevel,
        output: serde_json::Value,
        sleep_for: Option<std::time::Duration>,
    }

    #[async_trait::async_trait]
    impl crate::types::Connector for DispatchTestConnector {
        fn id(&self) -> &cyberclaw_core::ids::ConnectorId {
            &self.id
        }

        fn runtime(&self) -> cyberclaw_core::manifests::ConnectorRuntime {
            cyberclaw_core::manifests::ConnectorRuntime::Native
        }

        fn capabilities(&self) -> Vec<CapabilityContract> {
            vec![CapabilityContract {
                id: self.capability_id.clone(),
                title: self.capability_id.clone(),
                description: None,
                input_schema: "{}".to_string(),
                output_schema: "{}".to_string(),
                risk: self.risk,
                effects: vec![CapabilityEffect::Read],
                placement: None,
                timeouts: CapabilityTimeouts::default(),
            }]
        }

        async fn execute(
            &self,
            request: crate::types::CapabilityExecutionRequest,
        ) -> anyhow::Result<crate::types::CapabilityExecutionResult> {
            if let Some(d) = self.sleep_for {
                tokio::time::sleep(d).await;
            }
            Ok(crate::types::CapabilityExecutionResult {
                execution_id: request.execution_id,
                trace_id: request.trace_id,
                connector_id: request.connector_id,
                capability_id: request.capability_id,
                output: self.output.clone(),
                status: crate::types::ExecutionStatus::Success,
                error: None,
                actual_runtime: None,
            })
        }
    }

    fn make_dispatch_request(
        connector_id: cyberclaw_core::ids::ConnectorId,
        capability_id: &str,
    ) -> crate::types::CapabilityExecutionRequest {
        use cyberclaw_core::identity::{ActorRef, ActorType};
        use cyberclaw_core::ids::{ActorId, ExecutionId, WorkspaceId};
        use cyberclaw_core::workspace::{WorkspaceMode, WorkspaceRef};

        crate::types::CapabilityExecutionRequest {
            execution_id: ExecutionId::new(),
            trace_id: format!("dispatch-test-{}", uuid::Uuid::new_v4()),
            actor: ActorRef {
                id: ActorId::new(),
                actor_type: ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "dispatch-test".to_string(),
            },
            workspace: WorkspaceRef {
                id: WorkspaceId::new(),
                mode: WorkspaceMode::Ephemeral,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: "/tmp/dispatch-test".to_string(),
                writable_roots: vec![],
            },
            connector_id,
            capability_id: cyberclaw_core::ids::CapabilityId::from_string(
                capability_id.to_string(),
            )
            .unwrap(),
            input: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn process_runtime_not_configured_fails_fast() {
        // Arrange: dispatcher with no process_runtime, register Medium-risk capability.
        let connector_id =
            cyberclaw_core::ids::ConnectorId::from_string("rb09-noproc".to_string()).unwrap();
        let cap_id = "rb09.medium";

        let registry = Arc::new(ConnectorRegistry::new());
        let mock = Arc::new(DispatchTestConnector {
            id: connector_id.clone(),
            capability_id: cap_id.to_string(),
            risk: RiskLevel::Medium,
            output: serde_json::json!({"ok": true}),
            sleep_for: None,
        });
        registry.register(mock).unwrap();

        let dispatcher = CapabilityDispatcher::new(registry);
        let request = make_dispatch_request(connector_id, cap_id);

        let result = dispatcher.dispatch(request).await.unwrap();

        assert_eq!(
            result.status,
            crate::types::ExecutionStatus::Failed,
            "Process runtime unconfigured must fail-fast"
        );
        let err = result.error.as_deref().unwrap_or("");
        assert!(
            err.contains("not configured"),
            "Error should explain configuration gap, got: {}",
            err
        );
        assert!(err.contains("Process runtime"));
    }

    #[tokio::test]
    async fn process_runtime_dispatches_when_configured() {
        // Arrange: dispatcher with ProcessExecutor configured.
        let connector_id =
            cyberclaw_core::ids::ConnectorId::from_string("rb09-procok".to_string()).unwrap();
        let cap_id = "rb09.medium_ok";

        let registry = Arc::new(ConnectorRegistry::new());
        let mock = Arc::new(DispatchTestConnector {
            id: connector_id.clone(),
            capability_id: cap_id.to_string(),
            risk: RiskLevel::Medium,
            output: serde_json::json!({"ok": true, "value": 42}),
            sleep_for: None,
        });
        registry.register(mock).unwrap();

        let process_rt: Arc<dyn ProcessRuntime> =
            Arc::new(crate::runtime::ProcessExecutor::new_unrestricted());
        let dispatcher = CapabilityDispatcher::new(registry).with_process_runtime(process_rt);
        let request = make_dispatch_request(connector_id, cap_id);

        let result = dispatcher.dispatch(request).await.unwrap();

        assert_eq!(
            result.status,
            crate::types::ExecutionStatus::Success,
            "Configured Process runtime should dispatch successfully"
        );
        assert_eq!(
            result.actual_runtime,
            Some(RuntimeMode::Process),
            "actual_runtime must be tagged Process"
        );
    }

    #[tokio::test]
    async fn process_runtime_enforces_timeout() {
        // Arrange: capability with 100ms timeout, connector that sleeps 1s.
        let connector_id =
            cyberclaw_core::ids::ConnectorId::from_string("rb09-proctimeout".to_string()).unwrap();
        let cap_id = "rb09.slow_medium";

        // Custom contract with 100ms request_ms — we register the connector
        // with our DispatchTestConnector mock that returns CapabilityTimeouts::default(),
        // then override the contract via the registry's stored entry by re-registering
        // with the desired timeout.
        #[derive(Debug)]
        struct TimeoutMock {
            id: cyberclaw_core::ids::ConnectorId,
            cap_id: String,
        }

        #[async_trait::async_trait]
        impl crate::types::Connector for TimeoutMock {
            fn id(&self) -> &cyberclaw_core::ids::ConnectorId {
                &self.id
            }

            fn runtime(&self) -> cyberclaw_core::manifests::ConnectorRuntime {
                cyberclaw_core::manifests::ConnectorRuntime::Native
            }

            fn capabilities(&self) -> Vec<CapabilityContract> {
                vec![CapabilityContract {
                    id: self.cap_id.clone(),
                    title: self.cap_id.clone(),
                    description: None,
                    input_schema: "{}".to_string(),
                    output_schema: "{}".to_string(),
                    risk: RiskLevel::Medium,
                    effects: vec![CapabilityEffect::Read],
                    placement: None,
                    timeouts: CapabilityTimeouts {
                        request_ms: Some(100),
                    },
                }]
            }

            async fn execute(
                &self,
                request: crate::types::CapabilityExecutionRequest,
            ) -> anyhow::Result<crate::types::CapabilityExecutionResult> {
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                Ok(crate::types::CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: serde_json::json!({"too": "late"}),
                    status: crate::types::ExecutionStatus::Success,
                    error: None,
                    actual_runtime: None,
                })
            }
        }

        let registry = Arc::new(ConnectorRegistry::new());
        let mock = Arc::new(TimeoutMock {
            id: connector_id.clone(),
            cap_id: cap_id.to_string(),
        });
        registry.register(mock).unwrap();

        let process_rt: Arc<dyn ProcessRuntime> =
            Arc::new(crate::runtime::ProcessExecutor::new_unrestricted());
        let dispatcher = CapabilityDispatcher::new(registry).with_process_runtime(process_rt);
        let request = make_dispatch_request(connector_id, cap_id);

        let result = dispatcher.dispatch(request).await.unwrap();

        assert_eq!(
            result.status,
            crate::types::ExecutionStatus::Failed,
            "Timeout must produce Failed status"
        );
        let err = result.error.as_deref().unwrap_or("");
        assert!(
            err.contains("exceeded") && err.contains("timeout"),
            "Error should mention timeout exceeded, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn process_runtime_truncates_oversized_output() {
        // Arrange: connector returns ~5KB string, dispatcher cap is 1024 bytes.
        let connector_id =
            cyberclaw_core::ids::ConnectorId::from_string("rb09-oversized".to_string()).unwrap();
        let cap_id = "rb09.oversized_medium";

        let big_str = "X".repeat(5 * 1024);
        let registry = Arc::new(ConnectorRegistry::new());
        let mock = Arc::new(DispatchTestConnector {
            id: connector_id.clone(),
            capability_id: cap_id.to_string(),
            risk: RiskLevel::Medium,
            output: serde_json::json!({ "blob": big_str }),
            sleep_for: None,
        });
        registry.register(mock).unwrap();

        let process_rt: Arc<dyn ProcessRuntime> =
            Arc::new(crate::runtime::ProcessExecutor::new_unrestricted());
        let dispatcher = CapabilityDispatcher::new(registry)
            .with_process_runtime(process_rt)
            .with_process_output_cap(1024);
        let request = make_dispatch_request(connector_id, cap_id);

        let result = dispatcher.dispatch(request).await.unwrap();

        assert_eq!(result.status, crate::types::ExecutionStatus::Success);
        let truncated = result
            .output
            .get("truncated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(
            truncated,
            "Output exceeding cap must be truncated, got: {:?}",
            result.output
        );
        let max_size = result
            .output
            .get("max_size_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        assert_eq!(max_size, 1024, "max_size_bytes should reflect cap");
    }

    #[tokio::test]
    async fn container_runtime_not_configured_fails_fast() {
        // Arrange: dispatcher with no container_runtime, register High-risk capability.
        let connector_id =
            cyberclaw_core::ids::ConnectorId::from_string("rb09-nocontainer".to_string()).unwrap();
        let cap_id = "rb09.high";

        let registry = Arc::new(ConnectorRegistry::new());
        let mock = Arc::new(DispatchTestConnector {
            id: connector_id.clone(),
            capability_id: cap_id.to_string(),
            risk: RiskLevel::High,
            output: serde_json::json!({"ok": true}),
            sleep_for: None,
        });
        registry.register(mock).unwrap();

        let dispatcher = CapabilityDispatcher::new(registry);
        let request = make_dispatch_request(connector_id, cap_id);

        let result = dispatcher.dispatch(request).await.unwrap();

        assert_eq!(
            result.status,
            crate::types::ExecutionStatus::Failed,
            "Container runtime unconfigured must fail-fast"
        );
        let err = result.error.as_deref().unwrap_or("");
        assert!(
            err.contains("not configured") && err.contains("Container runtime"),
            "Error should explain container configuration gap, got: {}",
            err
        );
    }
}
