//! # MiddlewarePipeline
//!
//! Unified middleware pipeline for CyberClaw execution paths.
//! All cross-cutting concerns (policy, audit, tracing, hooks) are modeled as
//! composable middleware that execute in priority order around the core action.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use cyberclaw_core::capability::{CapabilityEffect, RiskLevel};
use cyberclaw_core::identity::ActorRef;

// ─── Request / Response ──────────────────────────────────────────────────────

/// Middleware request carrying execution context through the pipeline.
#[derive(Debug, Clone)]
pub struct MiddlewareRequest {
    /// The capability being invoked.
    pub capability_id: String,
    /// The connector that will execute the capability.
    pub connector_id: String,
    /// Arbitrary JSON input payload.
    pub input: serde_json::Value,
    /// The actor initiating the request.
    pub actor: ActorRef,
    /// Free-form metadata propagated across middleware.
    pub metadata: HashMap<String, String>,
    /// Optional risk level from the capability contract.
    /// When `None`, PolicyMiddleware falls back to `RiskLevel::High` (fail-safe default).
    pub risk_level: Option<RiskLevel>,
    /// Capability effects from the capability contract.
    /// Defaults to empty (PolicyMiddleware uses `vec![]` as fallback).
    pub effects: Vec<CapabilityEffect>,
}

/// Middleware response returned after pipeline execution.
#[derive(Debug, Clone)]
pub struct MiddlewareResponse {
    /// Output payload from the core action or final middleware.
    pub output: serde_json::Value,
    /// Metadata accumulated during pipeline traversal.
    pub metadata: HashMap<String, String>,
}

// ─── Error ───────────────────────────────────────────────────────────────────

/// Error type returned by middleware.
#[derive(Debug, thiserror::Error)]
pub enum MiddlewareError {
    /// A middleware rejected the request (e.g. policy denied).
    #[error("middleware '{middleware}' rejected: {reason}")]
    Rejected { middleware: String, reason: String },
    /// An internal error occurred inside a middleware.
    #[error("middleware '{middleware}' internal error: {source}")]
    Internal {
        middleware: String,
        #[source]
        source: anyhow::Error,
    },
}

// ─── MiddlewareNext trait ────────────────────────────────────────────────────

/// Trait representing the next handler in the middleware chain.
#[async_trait]
pub trait MiddlewareNext: Send + Sync {
    /// Forward the request to the next middleware or terminal handler.
    async fn run(&self, request: MiddlewareRequest) -> Result<MiddlewareResponse, MiddlewareError>;
}

// ─── Middleware trait ────────────────────────────────────────────────────────

/// A single middleware in the pipeline.
///
/// Middleware are sorted by `priority()` (lower = earlier). Each middleware
/// can inspect/modify the request, call `next.run()` to continue the chain,
/// and inspect/modify the response.
#[async_trait]
pub trait Middleware: Send + Sync {
    /// Human-readable name used for logging and diagnostics.
    fn name(&self) -> &str;

    /// Execution priority. Lower values execute first (0 = highest priority).
    fn priority(&self) -> u32;

    /// Process the request. Call `next.run(request)` to pass control downstream.
    async fn process(
        &self,
        request: MiddlewareRequest,
        next: &dyn MiddlewareNext,
    ) -> Result<MiddlewareResponse, MiddlewareError>;
}

// ─── MiddlewarePipeline ──────────────────────────────────────────────────────

/// Ordered pipeline of middleware that wraps every execution path.
///
/// Middleware are maintained in ascending priority order. When `execute` is
/// called, they run from lowest priority value to highest, forming a nested
/// call chain around the terminal pass-through handler.
pub struct MiddlewarePipeline {
    middlewares: Vec<Arc<dyn Middleware>>,
}

impl Default for MiddlewarePipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl MiddlewarePipeline {
    /// Create an empty pipeline.
    pub fn new() -> Self {
        Self {
            middlewares: Vec::new(),
        }
    }

    /// Add a middleware and re-sort by priority.
    pub fn add(&mut self, middleware: Arc<dyn Middleware>) {
        self.middlewares.push(middleware);
        self.middlewares.sort_by_key(|m| m.priority());
    }

    /// Execute the pipeline. If no middleware is registered the request is
    /// passed through to a terminal handler that echoes the input as output.
    pub async fn execute(
        &self,
        request: MiddlewareRequest,
    ) -> Result<MiddlewareResponse, MiddlewareError> {
        let chain = ChainRunner::new(&self.middlewares);
        chain.run(request).await
    }
}

// ─── Internal chain runner ───────────────────────────────────────────────────

/// Recursive chain runner that walks through the middleware slice.
struct ChainRunner<'a> {
    remaining: &'a [Arc<dyn Middleware>],
}

impl<'a> ChainRunner<'a> {
    fn new(middlewares: &'a [Arc<dyn Middleware>]) -> Self {
        Self {
            remaining: middlewares,
        }
    }
}

#[async_trait]
impl MiddlewareNext for ChainRunner<'_> {
    async fn run(&self, request: MiddlewareRequest) -> Result<MiddlewareResponse, MiddlewareError> {
        match self.remaining.split_first() {
            Some((current, rest)) => {
                let next = ChainRunner { remaining: rest };
                current.process(request, &next).await
            }
            None => {
                // Terminal: pass-through handler echoes input as output.
                Ok(MiddlewareResponse {
                    output: request.input.clone(),
                    metadata: request.metadata.clone(),
                })
            }
        }
    }
}

// ─── Built-in middleware skeletons ───────────────────────────────────────────

/// Distributed tracing middleware. Creates spans around the execution.
pub struct TraceMiddleware;

#[async_trait]
impl Middleware for TraceMiddleware {
    fn name(&self) -> &str {
        "trace"
    }

    fn priority(&self) -> u32 {
        0
    }

    async fn process(
        &self,
        request: MiddlewareRequest,
        next: &dyn MiddlewareNext,
    ) -> Result<MiddlewareResponse, MiddlewareError> {
        use tracing::{info_span, Instrument};

        let span = info_span!(
            "middleware_execution",
            capability_id = %request.capability_id,
            connector_id = %request.connector_id,
            actor = %request.actor.display_name,
        );

        next.run(request).instrument(span).await
    }
}

/// Policy evaluation middleware. Checks permissions via PolicyEngine.
pub struct PolicyMiddleware {
    policy_engine: Option<Arc<dyn cyberclaw_governance::engine::PolicyEngine>>,
    deny_when_no_engine: bool,
}

impl PolicyMiddleware {
    /// Create with an optional policy engine. When `None`, deny all (fail-safe).
    pub fn new(policy_engine: Option<Arc<dyn cyberclaw_governance::engine::PolicyEngine>>) -> Self {
        Self {
            policy_engine,
            deny_when_no_engine: true,
        }
    }

    /// Create a permissive middleware that passes through when no engine is configured.
    /// Only for testing and development.
    pub fn permissive() -> Self {
        Self {
            policy_engine: None,
            deny_when_no_engine: false,
        }
    }
}

#[async_trait]
impl Middleware for PolicyMiddleware {
    fn name(&self) -> &str {
        "policy"
    }

    fn priority(&self) -> u32 {
        10
    }

    async fn process(
        &self,
        request: MiddlewareRequest,
        next: &dyn MiddlewareNext,
    ) -> Result<MiddlewareResponse, MiddlewareError> {
        if let Some(ref engine) = self.policy_engine {
            use cyberclaw_core::capability::CapabilityRef;
            use cyberclaw_core::ids::{CapabilityId, ConnectorId};
            use cyberclaw_governance::engine::EvaluationContext;
            use cyberclaw_governance::GovernanceDecision;

            let capability_id =
                CapabilityId::from_string(request.capability_id.clone()).map_err(|e| {
                    MiddlewareError::Internal {
                        middleware: self.name().to_string(),
                        source: anyhow::anyhow!("invalid capability_id: {}", e),
                    }
                })?;
            let connector_id =
                ConnectorId::from_string(request.connector_id.clone()).map_err(|e| {
                    MiddlewareError::Internal {
                        middleware: self.name().to_string(),
                        source: anyhow::anyhow!("invalid connector_id: {}", e),
                    }
                })?;

            let eval_context = EvaluationContext {
                capability: CapabilityRef {
                    id: capability_id,
                    connector_id,
                    risk: request.risk_level.unwrap_or(RiskLevel::High),
                    effects: request.effects.clone(),
                    placement: None,
                },
                actor: request.actor.clone(),
                execution_id: cyberclaw_core::ids::ExecutionId::new(),
                reason: request.metadata.get("reason").cloned(),
            };

            let eval_result = engine
                .evaluate_capability(eval_context)
                .await
                .map_err(|e| MiddlewareError::Internal {
                    middleware: self.name().to_string(),
                    source: e,
                })?;

            match eval_result.decision {
                GovernanceDecision::Allow { .. } => {
                    tracing::info!(capability_id = %request.capability_id, "policy: allowed");
                }
                GovernanceDecision::Deny { reason } => {
                    tracing::warn!(capability_id = %request.capability_id, %reason, "policy: denied");
                    return Err(MiddlewareError::Rejected {
                        middleware: self.name().to_string(),
                        reason,
                    });
                }
                GovernanceDecision::ReviewRequired { reason, .. } => {
                    tracing::warn!(capability_id = %request.capability_id, %reason, "policy: review required");
                    return Err(MiddlewareError::Rejected {
                        middleware: self.name().to_string(),
                        reason: format!("review required: {}", reason),
                    });
                }
            }
        } else if self.deny_when_no_engine {
            return Err(MiddlewareError::Rejected {
                middleware: self.name().to_string(),
                reason: "no policy engine configured (deny-by-default)".to_string(),
            });
        }

        next.run(request).await
    }
}

/// Audit logging middleware. Records execution audit events.
pub struct AuditMiddleware;

#[async_trait]
impl Middleware for AuditMiddleware {
    fn name(&self) -> &str {
        "audit"
    }

    fn priority(&self) -> u32 {
        20
    }

    async fn process(
        &self,
        mut request: MiddlewareRequest,
        next: &dyn MiddlewareNext,
    ) -> Result<MiddlewareResponse, MiddlewareError> {
        let start = std::time::Instant::now();
        let capability_id = request.capability_id.clone();
        let connector_id = request.connector_id.clone();
        let actor_name = request.actor.display_name.clone();

        tracing::info!(
            audit = "pre_execution",
            %capability_id,
            %connector_id,
            actor = %actor_name,
            "audit: capability execution starting"
        );

        request.metadata.insert(
            "audit_started_at".to_string(),
            chrono::Utc::now().to_rfc3339(),
        );

        let result = next.run(request).await;
        let elapsed_ms = start.elapsed().as_millis();

        match &result {
            Ok(_) => {
                tracing::info!(
                    audit = "post_execution",
                    %capability_id,
                    %connector_id,
                    actor = %actor_name,
                    elapsed_ms = elapsed_ms,
                    status = "success",
                    "audit: capability execution completed"
                );
            }
            Err(e) => {
                tracing::warn!(
                    audit = "post_execution",
                    %capability_id,
                    %connector_id,
                    actor = %actor_name,
                    elapsed_ms = elapsed_ms,
                    status = "failed",
                    error = %e,
                    "audit: capability execution failed"
                );
            }
        }

        result
    }
}

/// W3C Trace Context propagation middleware.
///
/// Extracts `traceparent` / `tracestate` from incoming request metadata,
/// creates a child span for the current operation, and injects updated trace
/// headers into the response metadata for downstream services.
///
/// Priority **1** — runs immediately after the base `TraceMiddleware` (0) and
/// before tenant/policy middleware (5+).
pub struct TracePropagationMiddleware;

#[async_trait]
impl Middleware for TracePropagationMiddleware {
    fn name(&self) -> &str {
        "trace-propagation"
    }

    fn priority(&self) -> u32 {
        1
    }

    async fn process(
        &self,
        mut request: MiddlewareRequest,
        next: &dyn MiddlewareNext,
    ) -> Result<MiddlewareResponse, MiddlewareError> {
        use cyberclaw_observability::distributed::{
            extract_trace_context, inject_trace_context, TraceContext, TracePropagator,
        };

        let propagator = TracePropagator::new();

        // Extract trace context from incoming request metadata (if present).
        let trace_ctx = extract_trace_context(&request.metadata);

        // Determine the working context: either derive a child from incoming, or start fresh.
        let working_ctx = match trace_ctx {
            Some(parent) => {
                // Create a child span linked to the parent trace.
                let child_span = propagator.create_child_span(&parent, &request.capability_id);
                let child_headers = inject_trace_context(&child_span);
                // Merge child trace headers into request metadata for downstream.
                for (k, v) in &child_headers {
                    request.metadata.insert(k.clone(), v.clone());
                }
                // Also stash parent_span_id for correlation
                request
                    .metadata
                    .insert("parent_span_id".to_string(), parent.span_id.clone());
                child_span
            }
            None => {
                // No incoming trace — start a new root context.
                let root = TraceContext::new_root();
                let child_span = propagator.create_child_span(&root, &request.capability_id);
                let child_headers = inject_trace_context(&child_span);
                for (k, v) in &child_headers {
                    request.metadata.insert(k.clone(), v.clone());
                }
                child_span
            }
        };

        // Forward to the next middleware.
        let mut response = next.run(request).await?;

        // Inject trace headers into the response metadata so callers can correlate.
        let response_headers = inject_trace_context(&working_ctx);
        for (k, v) in response_headers {
            response.metadata.insert(k, v);
        }

        Ok(response)
    }
}

/// Hook dispatcher middleware. Integrates with the existing HookDispatcher.
pub struct HookMiddleware;

#[async_trait]
impl Middleware for HookMiddleware {
    fn name(&self) -> &str {
        "hook"
    }

    fn priority(&self) -> u32 {
        30
    }

    async fn process(
        &self,
        mut request: MiddlewareRequest,
        next: &dyn MiddlewareNext,
    ) -> Result<MiddlewareResponse, MiddlewareError> {
        tracing::debug!(
            hook = "pre_execution",
            capability_id = %request.capability_id,
            connector_id = %request.connector_id,
            "hook: dispatching pre-execution"
        );
        request
            .metadata
            .insert("hook_pre_dispatched".to_string(), "true".to_string());

        let mut response = next.run(request).await?;

        tracing::debug!(
            hook = "post_execution",
            capability_id = response
                .metadata
                .get("capability_id")
                .map(|s| s.as_str())
                .unwrap_or("unknown"),
            "hook: dispatching post-execution"
        );
        response
            .metadata
            .insert("hook_post_dispatched".to_string(), "true".to_string());

        Ok(response)
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::identity::ActorType;
    use cyberclaw_core::ids::ActorId;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Helper: build a minimal MiddlewareRequest for testing.
    fn test_request() -> MiddlewareRequest {
        MiddlewareRequest {
            capability_id: "cap-1".into(),
            connector_id: "conn-1".into(),
            input: serde_json::json!({"key": "value"}),
            actor: ActorRef {
                id: ActorId::new(),
                actor_type: ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "test-actor".to_string(),
            },
            metadata: HashMap::new(),
            risk_level: None,
            effects: vec![],
        }
    }

    /// A test middleware that records its execution order via a shared counter.
    struct OrderRecorder {
        label: String,
        prio: u32,
        counter: Arc<AtomicU32>,
        /// The order value assigned when this middleware executes.
        observed: Arc<AtomicU32>,
    }

    #[async_trait]
    impl Middleware for OrderRecorder {
        fn name(&self) -> &str {
            &self.label
        }

        fn priority(&self) -> u32 {
            self.prio
        }

        async fn process(
            &self,
            mut request: MiddlewareRequest,
            next: &dyn MiddlewareNext,
        ) -> Result<MiddlewareResponse, MiddlewareError> {
            let seq = self.counter.fetch_add(1, Ordering::SeqCst);
            self.observed.store(seq, Ordering::SeqCst);
            request.metadata.insert(self.label.clone(), seq.to_string());
            next.run(request).await
        }
    }

    #[tokio::test]
    async fn middlewares_execute_in_priority_order() {
        let counter = Arc::new(AtomicU32::new(0));
        let obs_a = Arc::new(AtomicU32::new(u32::MAX));
        let obs_b = Arc::new(AtomicU32::new(u32::MAX));
        let obs_c = Arc::new(AtomicU32::new(u32::MAX));

        let mut pipeline = MiddlewarePipeline::new();
        // Add in non-sorted order to verify sorting
        pipeline.add(Arc::new(OrderRecorder {
            label: "B".into(),
            prio: 20,
            counter: counter.clone(),
            observed: obs_b.clone(),
        }));
        pipeline.add(Arc::new(OrderRecorder {
            label: "C".into(),
            prio: 30,
            counter: counter.clone(),
            observed: obs_c.clone(),
        }));
        pipeline.add(Arc::new(OrderRecorder {
            label: "A".into(),
            prio: 10,
            counter: counter.clone(),
            observed: obs_a.clone(),
        }));

        let resp = pipeline.execute(test_request()).await.unwrap();

        assert_eq!(
            obs_a.load(Ordering::SeqCst),
            0,
            "A (prio=10) should run first"
        );
        assert_eq!(
            obs_b.load(Ordering::SeqCst),
            1,
            "B (prio=20) should run second"
        );
        assert_eq!(
            obs_c.load(Ordering::SeqCst),
            2,
            "C (prio=30) should run third"
        );

        // Metadata should contain all three labels
        assert_eq!(resp.metadata.get("A"), Some(&"0".to_string()));
        assert_eq!(resp.metadata.get("B"), Some(&"1".to_string()));
        assert_eq!(resp.metadata.get("C"), Some(&"2".to_string()));
    }

    #[tokio::test]
    async fn chain_passes_request_and_response_through() {
        struct MetaAdder;

        #[async_trait]
        impl Middleware for MetaAdder {
            fn name(&self) -> &str {
                "meta-adder"
            }
            fn priority(&self) -> u32 {
                0
            }
            async fn process(
                &self,
                mut request: MiddlewareRequest,
                next: &dyn MiddlewareNext,
            ) -> Result<MiddlewareResponse, MiddlewareError> {
                request.metadata.insert("added".into(), "yes".into());
                let mut resp = next.run(request).await?;
                resp.metadata.insert("post".into(), "done".into());
                Ok(resp)
            }
        }

        let mut pipeline = MiddlewarePipeline::new();
        pipeline.add(Arc::new(MetaAdder));

        let resp = pipeline.execute(test_request()).await.unwrap();
        assert_eq!(resp.metadata.get("added"), Some(&"yes".to_string()));
        assert_eq!(resp.metadata.get("post"), Some(&"done".to_string()));
        assert_eq!(resp.output, serde_json::json!({"key": "value"}));
    }

    #[tokio::test]
    async fn middleware_error_short_circuits_chain() {
        struct Blocker;

        #[async_trait]
        impl Middleware for Blocker {
            fn name(&self) -> &str {
                "blocker"
            }
            fn priority(&self) -> u32 {
                5
            }
            async fn process(
                &self,
                _request: MiddlewareRequest,
                _next: &dyn MiddlewareNext,
            ) -> Result<MiddlewareResponse, MiddlewareError> {
                Err(MiddlewareError::Rejected {
                    middleware: "blocker".into(),
                    reason: "denied by policy".into(),
                })
            }
        }

        let reached = Arc::new(AtomicU32::new(0));
        struct Unreachable(Arc<AtomicU32>);

        #[async_trait]
        impl Middleware for Unreachable {
            fn name(&self) -> &str {
                "unreachable"
            }
            fn priority(&self) -> u32 {
                10
            }
            async fn process(
                &self,
                request: MiddlewareRequest,
                next: &dyn MiddlewareNext,
            ) -> Result<MiddlewareResponse, MiddlewareError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                next.run(request).await
            }
        }

        let mut pipeline = MiddlewarePipeline::new();
        pipeline.add(Arc::new(Blocker));
        pipeline.add(Arc::new(Unreachable(reached.clone())));

        let result = pipeline.execute(test_request()).await;
        assert!(result.is_err());
        assert_eq!(
            reached.load(Ordering::SeqCst),
            0,
            "downstream middleware must not execute"
        );

        let err = result.unwrap_err();
        assert!(err.to_string().contains("denied by policy"));
    }

    #[tokio::test]
    async fn empty_pipeline_passes_through() {
        let pipeline = MiddlewarePipeline::new();
        let req = test_request();
        let expected_input = req.input.clone();

        let resp = pipeline.execute(req).await.unwrap();
        assert_eq!(resp.output, expected_input);
    }

    #[tokio::test]
    async fn builtin_middlewares_have_correct_priorities() {
        assert_eq!(TraceMiddleware.priority(), 0);
        assert_eq!(TracePropagationMiddleware.priority(), 1);
        assert_eq!(PolicyMiddleware::permissive().priority(), 10);
        assert_eq!(AuditMiddleware.priority(), 20);
        assert_eq!(HookMiddleware.priority(), 30);
    }

    #[tokio::test]
    async fn builtin_middlewares_pass_through() {
        let mut pipeline = MiddlewarePipeline::new();
        pipeline.add(Arc::new(TraceMiddleware));
        pipeline.add(Arc::new(TracePropagationMiddleware));
        pipeline.add(Arc::new(PolicyMiddleware::permissive()));
        pipeline.add(Arc::new(AuditMiddleware));
        pipeline.add(Arc::new(HookMiddleware));

        let req = test_request();
        let expected = req.input.clone();
        let resp = pipeline.execute(req).await.unwrap();
        assert_eq!(resp.output, expected);
    }

    #[tokio::test]
    async fn trace_propagation_middleware_injects_headers() {
        let mut pipeline = MiddlewarePipeline::new();
        pipeline.add(Arc::new(TracePropagationMiddleware));

        let req = test_request();
        let resp = pipeline.execute(req).await.unwrap();

        // The middleware should have injected traceparent into the response metadata.
        assert!(
            resp.metadata.contains_key("traceparent"),
            "response metadata must contain traceparent header"
        );
        let tp = resp.metadata.get("traceparent").unwrap();
        let parts: Vec<&str> = tp.split('-').collect();
        assert_eq!(parts.len(), 4, "traceparent must have 4 parts");
        assert_eq!(parts[0], "00", "version must be 00");
    }

    #[tokio::test]
    async fn test_policy_middleware_denies_without_engine() {
        // PolicyMiddleware::new(None) 应在无策略引擎时 deny-by-default
        let middleware = PolicyMiddleware::new(None);
        let mut pipeline = MiddlewarePipeline::new();
        pipeline.add(Arc::new(middleware));

        let result = pipeline.execute(test_request()).await;

        assert!(result.is_err(), "无策略引擎时应拒绝请求");
        match result.unwrap_err() {
            MiddlewareError::Rejected { middleware, reason } => {
                assert_eq!(middleware, "policy", "拒绝方应为 policy 中间件");
                assert!(
                    reason.contains("no policy engine") || reason.contains("deny-by-default"),
                    "拒绝原因应说明无策略引擎，实际: {}",
                    reason
                );
            }
            e => panic!("预期 Rejected 错误，得到: {}", e),
        }
    }

    #[tokio::test]
    async fn test_policy_middleware_permissive_allows() {
        // PolicyMiddleware::permissive() 在无策略引擎时应放行请求
        let middleware = PolicyMiddleware::permissive();
        let mut pipeline = MiddlewarePipeline::new();
        pipeline.add(Arc::new(middleware));

        let req = test_request();
        let expected_output = req.input.clone();

        let result = pipeline.execute(req).await;

        assert!(result.is_ok(), "permissive 模式应允许请求通过");
        let resp = result.unwrap();
        assert_eq!(
            resp.output, expected_output,
            "permissive 模式应透传 input 作为 output"
        );
    }

    #[tokio::test]
    async fn trace_propagation_middleware_continues_incoming_trace() {
        let mut pipeline = MiddlewarePipeline::new();
        pipeline.add(Arc::new(TracePropagationMiddleware));

        let mut req = test_request();
        // Inject an incoming traceparent
        let incoming_trace_id = "aaaabbbbccccdddd1111222233334444";
        let incoming_span_id = "1122334455667788";
        req.metadata.insert(
            "traceparent".to_string(),
            format!("00-{}-{}-01", incoming_trace_id, incoming_span_id),
        );

        let resp = pipeline.execute(req).await.unwrap();

        let tp = resp.metadata.get("traceparent").unwrap();
        let parts: Vec<&str> = tp.split('-').collect();
        // The trace_id should be preserved from the incoming header
        assert_eq!(parts[1], incoming_trace_id, "trace_id must be preserved");
        // The span_id should be a NEW child, not the incoming span_id
        assert_ne!(
            parts[2], incoming_span_id,
            "span_id must be a new child span"
        );
        // Parent span should be recorded
        assert_eq!(
            resp.metadata.get("parent_span_id").map(String::as_str),
            Some(incoming_span_id),
            "parent_span_id must reference the incoming span"
        );
    }

    #[tokio::test]
    async fn s1_middleware_request_carries_risk_and_effects() {
        use cyberclaw_core::capability::{CapabilityEffect, RiskLevel};

        // Verify that MiddlewareRequest can carry risk_level and effects
        let mut req = test_request();
        req.risk_level = Some(RiskLevel::Critical);
        req.effects = vec![CapabilityEffect::Execute, CapabilityEffect::Network];

        assert_eq!(req.risk_level, Some(RiskLevel::Critical));
        assert_eq!(req.effects.len(), 2);
    }

    #[tokio::test]
    async fn s1_middleware_request_defaults_to_none_risk() {
        // Verify default behavior: risk_level=None, effects=empty
        let req = test_request();
        assert_eq!(req.risk_level, None);
        assert!(req.effects.is_empty());
    }
}
