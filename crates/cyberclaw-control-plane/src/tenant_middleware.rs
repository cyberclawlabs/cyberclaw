//! Tenant boundary middleware for multi-tenant isolation.
//!
//! Integrates [`TenantBoundaryPolicy`] from `cyberclaw-governance` into the
//! [`MiddlewarePipeline`], ensuring that tenant A's agents cannot access
//! tenant B's resources.
//!
//! Unlike the [`TenantBoundaryPolicyEngine`] wrapper (which loses the target
//! tenant due to [`EvaluationContext`] limitations), this middleware calls
//! [`TenantBoundaryPolicy::evaluate`] directly with both actor and target
//! tenant IDs extracted from the middleware request.

use async_trait::async_trait;
use std::sync::Arc;

use cyberclaw_core::prelude::*;
use cyberclaw_governance::tenant_policy::TenantBoundaryPolicy;

use crate::middleware_pipeline::{
    Middleware, MiddlewareError, MiddlewareNext, MiddlewareRequest, MiddlewareResponse,
};

/// Context describing the tenant boundaries of a request.
#[derive(Debug, Clone)]
pub struct TenantContext {
    /// Tenant ID of the actor making the request.
    pub actor_tenant_id: Option<String>,
    /// Tenant ID of the target resource.
    pub target_tenant_id: Option<String>,
    /// The capability being invoked.
    pub capability_id: String,
    /// The connector that will execute the capability.
    pub connector_id: String,
}

/// Middleware that enforces tenant boundary isolation.
///
/// Runs at priority 5 (after tracing at 0 but before policy at 10).
/// System actors (no `tenant_id`) are allowed through unconditionally.
/// Cross-tenant requests are evaluated against [`TenantBoundaryPolicy`].
pub struct TenantMiddleware {
    boundary_policy: Arc<TenantBoundaryPolicy>,
}

impl TenantMiddleware {
    /// Create a new `TenantMiddleware` wrapping the given policy.
    pub fn new(boundary_policy: Arc<TenantBoundaryPolicy>) -> Self {
        Self { boundary_policy }
    }

    /// Create with the default strict boundary policy.
    pub fn with_default_policy() -> Self {
        Self::new(Arc::new(TenantBoundaryPolicy::default()))
    }

    /// Create with a permissive boundary policy (useful for single-tenant deployments).
    pub fn with_permissive_policy() -> Self {
        Self::new(Arc::new(TenantBoundaryPolicy::permissive_policy()))
    }

    /// Extract a [`TenantContext`] from the middleware request.
    fn extract_context(request: &MiddlewareRequest) -> TenantContext {
        let actor_tenant_id = request.actor.tenant_id.as_ref().map(|t| t.to_string());
        let target_tenant_id = request.metadata.get("target_tenant_id").cloned();

        TenantContext {
            actor_tenant_id,
            target_tenant_id,
            capability_id: request.capability_id.clone(),
            connector_id: request.connector_id.clone(),
        }
    }
}

#[async_trait]
impl Middleware for TenantMiddleware {
    fn name(&self) -> &str {
        "tenant-boundary"
    }

    fn priority(&self) -> u32 {
        5
    }

    async fn process(
        &self,
        request: MiddlewareRequest,
        next: &dyn MiddlewareNext,
    ) -> Result<MiddlewareResponse, MiddlewareError> {
        let ctx = Self::extract_context(&request);

        // System actors (no tenant_id) are always allowed through.
        if ctx.actor_tenant_id.is_none() {
            return next.run(request).await;
        }

        // Build typed IDs for policy evaluation.
        let actor_tenant = ctx
            .actor_tenant_id
            .as_ref()
            .and_then(|id| TenantId::from_string(id.clone()).ok());
        let target_tenant = ctx
            .target_tenant_id
            .as_ref()
            .and_then(|id| TenantId::from_string(id.clone()).ok());

        // If there is no target tenant information, allow through for backward
        // compatibility — callers that do not set `target_tenant_id` metadata
        // are assumed to be same-tenant or single-tenant deployments.
        if target_tenant.is_none() {
            return next.run(request).await;
        }

        // Build a minimal CapabilityRef for policy evaluation.
        let capability = CapabilityRef {
            id: CapabilityId::from_string(ctx.capability_id.clone())
                .unwrap_or_else(|_| CapabilityId::from_string("unknown".to_string()).unwrap()),
            connector_id: ConnectorId::from_string(ctx.connector_id.clone())
                .unwrap_or_else(|_| ConnectorId::from_string("unknown".to_string()).unwrap()),
            risk: RiskLevel::Medium,
            effects: vec![CapabilityEffect::Read],
            placement: None,
        };

        // Evaluate directly against TenantBoundaryPolicy so both actor and
        // target tenant IDs are threaded through (the PolicyEngine wrapper
        // loses the target tenant).
        let decision = self.boundary_policy.evaluate(
            actor_tenant.as_ref(),
            target_tenant.as_ref(),
            &capability,
        );

        if decision.is_deny() {
            return Err(MiddlewareError::Rejected {
                middleware: self.name().to_string(),
                reason: format!(
                    "Cross-tenant access denied: actor tenant {:?} -> target tenant {:?} ({})",
                    ctx.actor_tenant_id,
                    ctx.target_tenant_id,
                    decision.reason()
                ),
            });
        }

        if decision.is_review_required() {
            return Err(MiddlewareError::Rejected {
                middleware: self.name().to_string(),
                reason: format!(
                    "Cross-tenant access requires review: actor tenant {:?} -> target tenant {:?} ({})",
                    ctx.actor_tenant_id,
                    ctx.target_tenant_id,
                    decision.reason()
                ),
            });
        }

        // Allowed — continue the chain.
        next.run(request).await
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware_pipeline::MiddlewarePipeline;
    use cyberclaw_core::identity::{ActorRef, ActorType};
    use cyberclaw_core::ids::ActorId;
    use std::collections::HashMap;

    /// Build a request with the given actor tenant and target tenant metadata.
    fn make_request(actor_tenant: Option<&str>, target_tenant: Option<&str>) -> MiddlewareRequest {
        let tenant_id = actor_tenant.map(|t| TenantId::from_string(t.to_string()).unwrap());

        let mut metadata = HashMap::new();
        if let Some(target) = target_tenant {
            metadata.insert("target_tenant_id".to_string(), target.to_string());
        }

        MiddlewareRequest {
            capability_id: "data.read".to_string(),
            connector_id: "test-connector".to_string(),
            input: serde_json::json!({"test": true}),
            actor: ActorRef {
                id: ActorId::new(),
                actor_type: ActorType::Agent,
                tenant_id,
                home_node_id: None,
                display_name: "test-actor".to_string(),
            },
            metadata,
            risk_level: None,
            effects: vec![],
        }
    }

    #[tokio::test]
    async fn system_actor_passes_through() {
        let middleware = TenantMiddleware::with_default_policy();
        let mut pipeline = MiddlewarePipeline::new();
        pipeline.add(Arc::new(middleware));

        // System actor: no tenant_id, should always pass.
        let request = make_request(None, Some("tenant-b"));
        let result = pipeline.execute(request).await;
        assert!(result.is_ok(), "system actor should pass through");
    }

    #[tokio::test]
    async fn same_tenant_access_allowed() {
        let middleware = TenantMiddleware::with_default_policy();
        let mut pipeline = MiddlewarePipeline::new();
        pipeline.add(Arc::new(middleware));

        // Same tenant on both sides should be allowed.
        let request = make_request(Some("tenant-a"), Some("tenant-a"));
        let result = pipeline.execute(request).await;
        assert!(result.is_ok(), "same-tenant access should be allowed");
    }

    #[tokio::test]
    async fn cross_tenant_access_denied() {
        let middleware = TenantMiddleware::with_default_policy();
        let mut pipeline = MiddlewarePipeline::new();
        pipeline.add(Arc::new(middleware));

        // Cross-tenant data access should be denied by default policy.
        let request = make_request(Some("tenant-a"), Some("tenant-b"));
        let result = pipeline.execute(request).await;
        assert!(result.is_err(), "cross-tenant access should be denied");

        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("tenant-boundary"),
            "error should identify middleware: {err_msg}"
        );
    }

    #[tokio::test]
    async fn missing_target_tenant_defaults_to_allow() {
        let middleware = TenantMiddleware::with_default_policy();
        let mut pipeline = MiddlewarePipeline::new();
        pipeline.add(Arc::new(middleware));

        // No target_tenant_id in metadata — backward-compat allows.
        let request = make_request(Some("tenant-a"), None);
        let result = pipeline.execute(request).await;
        assert!(
            result.is_ok(),
            "missing target tenant should default to allow for backward compat"
        );
    }

    #[tokio::test]
    async fn priority_is_correct() {
        let middleware = TenantMiddleware::with_default_policy();
        assert_eq!(middleware.priority(), 5);
        assert_eq!(middleware.name(), "tenant-boundary");
    }

    #[tokio::test]
    async fn permissive_policy_allows_cross_tenant() {
        let middleware = TenantMiddleware::with_permissive_policy();
        let mut pipeline = MiddlewarePipeline::new();
        pipeline.add(Arc::new(middleware));

        // Even cross-tenant should pass with permissive policy.
        let request = make_request(Some("tenant-a"), Some("tenant-b"));
        let result = pipeline.execute(request).await;
        assert!(
            result.is_ok(),
            "permissive policy should allow cross-tenant access"
        );
    }

    #[tokio::test]
    async fn tenant_context_extraction() {
        let request = make_request(Some("tenant-x"), Some("tenant-y"));
        let ctx = TenantMiddleware::extract_context(&request);

        assert_eq!(ctx.actor_tenant_id.as_deref(), Some("tenant-x"));
        assert_eq!(ctx.target_tenant_id.as_deref(), Some("tenant-y"));
        assert_eq!(ctx.capability_id, "data.read");
        assert_eq!(ctx.connector_id, "test-connector");
    }
}
