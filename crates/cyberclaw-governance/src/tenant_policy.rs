//! Tenant boundary policy system for multi-tenant isolation.
//!
//! This module provides tenant isolation policies that enforce boundaries
//! between different tenants in a multi-tenant environment.

use async_trait::async_trait;
use std::sync::Arc;

use cyberclaw_core::prelude::*;
use serde::{Deserialize, Serialize};

use crate::decision::{GovernanceDecision, ReviewType};
use crate::engine::{EvaluationContext, EvaluationResult, PolicyEngine};

/// Tenant boundary enforcement mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryMode {
    /// Strict isolation: deny any cross-tenant access
    #[default]
    Strict,

    /// Review mode: require review for cross-tenant access
    Review,

    /// Permissive: allow cross-tenant with logging
    Permissive,

    /// Disabled: no boundary enforcement
    Disabled,
}

/// Tenant boundary rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantBoundaryRule {
    /// Rule name for debugging
    pub name: String,

    /// Priority (higher priority rules are evaluated first)
    pub priority: u32,

    /// Source tenant ID pattern (glob syntax, None matches all)
    pub source_tenant_pattern: Option<String>,

    /// Target tenant ID pattern (glob syntax, None matches all)
    pub target_tenant_pattern: Option<String>,

    /// Capability ID pattern (glob syntax, None matches all)
    pub capability_pattern: Option<String>,

    /// Boundary enforcement mode for matching requests
    pub mode: BoundaryMode,

    /// Reason for the boundary rule
    pub reason: String,
}

impl TenantBoundaryRule {
    /// Check if this rule matches the given context
    ///
    /// Note: Since CapabilityPlacement doesn't have tenant_id in the current architecture,
    /// target_tenant_pattern is used to match against network_zone as a proxy for
    /// tenant boundaries in multi-zone deployments.
    pub fn matches(
        &self,
        actor_tenant: Option<&TenantId>,
        _target_tenant: Option<&TenantId>,
        capability: &CapabilityRef,
    ) -> bool {
        // Check source tenant pattern
        if let Some(ref pattern) = self.source_tenant_pattern {
            match actor_tenant {
                Some(tenant_id) => {
                    if !Self::matches_glob(pattern, tenant_id.as_ref()) {
                        return false;
                    }
                }
                None => {
                    // Rule requires tenant, but actor has none
                    return false;
                }
            }
        }

        // Check target tenant pattern
        // Note: In current architecture, we use network_zone from placement as a proxy
        // for tenant boundaries. Future enhancement: add explicit tenant_id to CapabilityRef.
        if let Some(ref pattern) = self.target_tenant_pattern {
            if let Some(placement) = &capability.placement {
                if let Some(ref network_zone) = placement.network_zone {
                    if !Self::matches_glob(pattern, network_zone) {
                        return false;
                    }
                }
            }
            // If pattern is set but no network_zone, consider it a mismatch
            // unless the pattern is "*" (matches all)
            else if pattern != "*" {
                return false;
            }
        }

        // Check capability pattern
        if let Some(ref pattern) = self.capability_pattern {
            if !Self::matches_glob(pattern, capability.id.as_ref()) {
                return false;
            }
        }

        true
    }

    /// Simple glob pattern matching (* matches any sequence)
    fn matches_glob(pattern: &str, text: &str) -> bool {
        if pattern == "*" {
            return true;
        }

        // Split pattern by '*'
        let parts: Vec<&str> = pattern.split('*').collect();

        if parts.len() == 1 {
            // No wildcards, exact match
            return pattern == text;
        }

        let mut pos = 0;
        for (i, part) in parts.iter().enumerate() {
            if part.is_empty() {
                continue;
            }

            if i == 0 {
                // First part: must match start
                if !text.starts_with(part) {
                    return false;
                }
                pos = part.len();
            } else if i == parts.len() - 1 {
                // Last part: must match end
                if !text.ends_with(part) {
                    return false;
                }
            } else {
                // Middle part: must exist
                if let Some(idx) = text[pos..].find(part) {
                    pos += idx + part.len();
                } else {
                    return false;
                }
            }
        }

        true
    }

    /// Create a decision based on this rule's mode
    pub fn to_decision(
        &self,
        actor_tenant: Option<&TenantId>,
        target_tenant: Option<&TenantId>,
    ) -> GovernanceDecision {
        let tenant_info = match (actor_tenant, target_tenant) {
            (Some(source), Some(target)) => {
                format!("source tenant: {}, target tenant: {}", source, target)
            }
            (Some(source), None) => format!("source tenant: {}, no target tenant", source),
            (None, Some(target)) => format!("no source tenant, target tenant: {}", target),
            (None, None) => "no tenant information".to_string(),
        };

        let reason = format!("{} ({})", self.reason, tenant_info);

        match self.mode {
            BoundaryMode::Strict => GovernanceDecision::deny(reason),
            BoundaryMode::Review => {
                GovernanceDecision::review_required(reason, ReviewType::Security)
            }
            BoundaryMode::Permissive => GovernanceDecision::allow(reason),
            BoundaryMode::Disabled => {
                GovernanceDecision::allow("Tenant boundary checking disabled")
            }
        }
    }
}

/// Tenant boundary policy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantBoundaryPolicy {
    /// Boundary rules sorted by priority (highest first)
    rules: Vec<TenantBoundaryRule>,

    /// Default boundary mode when no rule matches
    default_mode: BoundaryMode,

    /// Default reason when using default mode
    default_reason: String,
}

impl TenantBoundaryPolicy {
    /// Create a new tenant boundary policy
    pub fn new(
        mut rules: Vec<TenantBoundaryRule>,
        default_mode: BoundaryMode,
        default_reason: impl Into<String>,
    ) -> Self {
        // Sort rules by priority (highest first)
        rules.sort_by_key(|b| std::cmp::Reverse(b.priority));
        Self {
            rules,
            default_mode,
            default_reason: default_reason.into(),
        }
    }

    /// Create a default tenant boundary policy with strict isolation
    ///
    /// Note: This policy assumes cross-tenant access detection happens at the
    /// evaluate() level (checking if source_tenant != target_tenant).
    /// Rules here provide additional fine-grained control based on capability types.
    pub fn default_policy() -> Self {
        let rules = vec![
            // Deny cross-tenant data access (when cross-tenant detected)
            TenantBoundaryRule {
                name: "deny-cross-tenant-data".to_string(),
                priority: 90,
                source_tenant_pattern: None, // Matches any source
                target_tenant_pattern: None, // Matches any target
                capability_pattern: Some("data.*".to_string()),
                mode: BoundaryMode::Strict,
                reason: "Cross-tenant data access denied".to_string(),
            },
            // Review cross-tenant network access (when cross-tenant detected)
            TenantBoundaryRule {
                name: "review-cross-tenant-network".to_string(),
                priority: 80,
                source_tenant_pattern: None, // Matches any source
                target_tenant_pattern: None, // Matches any target
                capability_pattern: Some("network.*".to_string()),
                mode: BoundaryMode::Review,
                reason: "Cross-tenant network access requires review".to_string(),
            },
        ];

        Self::new(
            rules,
            BoundaryMode::Review,
            "Default: cross-tenant access requires review",
        )
    }

    /// Create a permissive policy (useful for single-tenant deployments)
    pub fn permissive_policy() -> Self {
        Self::new(
            vec![],
            BoundaryMode::Permissive,
            "Permissive mode: all cross-tenant access allowed",
        )
    }

    /// Evaluate the tenant boundary policy
    ///
    /// Note: In the current architecture, target_tenant is derived from actor context
    /// or other metadata. CapabilityPlacement uses network_zone as a proxy for
    /// tenant boundaries.
    pub fn evaluate(
        &self,
        actor_tenant: Option<&TenantId>,
        target_tenant: Option<&TenantId>,
        capability: &CapabilityRef,
    ) -> GovernanceDecision {
        // If both are None or both are Some with same value, allow (same-tenant or no-tenant)
        match (actor_tenant, target_tenant) {
            (None, None) => {
                return GovernanceDecision::allow("No tenant isolation required");
            }
            (Some(source), Some(target)) if source == target => {
                return GovernanceDecision::allow("Same-tenant access");
            }
            _ => {} // Continue to rule evaluation
        }

        // Find the first matching rule
        for rule in &self.rules {
            if rule.matches(actor_tenant, target_tenant, capability) {
                return rule.to_decision(actor_tenant, target_tenant);
            }
        }

        // No rule matched, use default mode
        let reason = format!(
            "{} (source: {:?}, target: {:?})",
            self.default_reason, actor_tenant, target_tenant
        );

        match self.default_mode {
            BoundaryMode::Strict => GovernanceDecision::deny(reason),
            BoundaryMode::Review => {
                GovernanceDecision::review_required(reason, ReviewType::Security)
            }
            BoundaryMode::Permissive => GovernanceDecision::allow(reason),
            BoundaryMode::Disabled => {
                GovernanceDecision::allow("Tenant boundary checking disabled")
            }
        }
    }

    /// Add a new rule to the policy
    pub fn add_rule(&mut self, rule: TenantBoundaryRule) {
        self.rules.push(rule);
        // Re-sort by priority
        self.rules.sort_by_key(|b| std::cmp::Reverse(b.priority));
    }

    /// Get all rules
    pub fn rules(&self) -> &[TenantBoundaryRule] {
        &self.rules
    }
}

impl Default for TenantBoundaryPolicy {
    fn default() -> Self {
        Self::default_policy()
    }
}

/// Tenant boundary policy engine wrapper
///
/// Wraps a TenantBoundaryPolicy as a PolicyEngine for use in CompositePolicyEngine
#[derive(Clone)]
pub struct TenantBoundaryPolicyEngine {
    policy: Arc<TenantBoundaryPolicy>,
}

impl TenantBoundaryPolicyEngine {
    /// Create a new tenant boundary policy engine
    pub fn new(policy: Arc<TenantBoundaryPolicy>) -> Self {
        Self { policy }
    }

    /// Create with default policy
    pub fn default_engine() -> Self {
        Self::new(Arc::new(TenantBoundaryPolicy::default()))
    }

    /// Create with permissive policy
    pub fn permissive_engine() -> Self {
        Self::new(Arc::new(TenantBoundaryPolicy::permissive_policy()))
    }
}

#[async_trait]
impl PolicyEngine for TenantBoundaryPolicyEngine {
    async fn evaluate_capability(
        &self,
        context: EvaluationContext,
    ) -> anyhow::Result<EvaluationResult> {
        let actor_tenant = context.actor.tenant_id.as_ref();

        // In the current architecture, EvaluationContext does not carry a target tenant.
        // CapabilityRef uses network_zone from CapabilityPlacement as a proxy for tenant
        // boundaries in multi-zone deployments. The TenantBoundaryPolicy.evaluate() method
        // handles None target_tenant by skipping same-tenant checks and proceeding to rule
        // evaluation, where target_tenant_pattern matches against network_zone instead.
        // To enable explicit cross-tenant checks, add target_tenant to EvaluationContext
        // or CapabilityRef and thread it through the call chain.
        let target_tenant: Option<&TenantId> = None;

        let decision = self
            .policy
            .evaluate(actor_tenant, target_tenant, &context.capability);

        let mut context_info = vec![
            format!(
                "Tenant boundary evaluated for actor: {} (tenant: {:?})",
                context.actor.id, context.actor.tenant_id
            ),
            format!("Capability: {}", context.capability.id),
        ];

        if let Some(placement) = &context.capability.placement {
            if let Some(ref network_zone) = placement.network_zone {
                context_info.push(format!("Network zone: {}", network_zone));
            }
        }

        Ok(EvaluationResult {
            decision,
            evaluated_risk: context.capability.risk,
            context_info,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_capability(id: &str, network_zone: Option<String>) -> CapabilityRef {
        CapabilityRef {
            id: CapabilityId::from_string(id.to_string()).unwrap(),
            connector_id: ConnectorId::from_string("test-connector".to_string()).unwrap(),
            risk: RiskLevel::Medium,
            effects: vec![CapabilityEffect::Read],
            placement: network_zone.map(|zone| CapabilityPlacement {
                allowed_node_labels: vec![],
                required_runtime: vec![],
                requires_local_secret: false,
                network_zone: Some(zone),
            }),
        }
    }

    #[test]
    fn test_boundary_mode_serialization() {
        let mode = BoundaryMode::Strict;
        let json = serde_json::to_string(&mode).unwrap();
        let deserialized: BoundaryMode = serde_json::from_str(&json).unwrap();
        assert_eq!(mode, deserialized);
    }

    #[test]
    fn test_same_tenant_access() {
        let policy = TenantBoundaryPolicy::default();
        let tenant_a = TenantId::from_string("tenant-a".to_string()).unwrap();
        let capability = create_test_capability("test", None);

        let decision = policy.evaluate(Some(&tenant_a), Some(&tenant_a), &capability);
        assert!(decision.is_allow());
        assert!(decision.reason().contains("Same-tenant"));
    }

    #[test]
    fn test_no_tenant_access() {
        let policy = TenantBoundaryPolicy::default();
        let capability = create_test_capability("test", None);

        let decision = policy.evaluate(None, None, &capability);
        assert!(decision.is_allow());
        assert!(decision.reason().contains("No tenant isolation"));
    }

    #[test]
    fn test_cross_tenant_data_access_denied() {
        let policy = TenantBoundaryPolicy::default();
        let tenant_a = TenantId::from_string("tenant-a".to_string()).unwrap();
        let tenant_b = TenantId::from_string("tenant-b".to_string()).unwrap();
        let capability = create_test_capability("data.read", Some("zone-b".to_string()));

        let decision = policy.evaluate(Some(&tenant_a), Some(&tenant_b), &capability);
        assert!(decision.is_deny());
        assert!(decision.reason().contains("Cross-tenant data access"));
    }

    #[test]
    fn test_cross_tenant_network_review() {
        let policy = TenantBoundaryPolicy::default();
        let tenant_a = TenantId::from_string("tenant-a".to_string()).unwrap();
        let tenant_b = TenantId::from_string("tenant-b".to_string()).unwrap();
        let capability = create_test_capability("network.fetch", Some("zone-b".to_string()));

        let decision = policy.evaluate(Some(&tenant_a), Some(&tenant_b), &capability);
        assert!(decision.is_review_required());
        assert_eq!(decision.review_type(), Some(&ReviewType::Security));
    }

    #[test]
    fn test_permissive_policy() {
        let policy = TenantBoundaryPolicy::permissive_policy();
        let tenant_a = TenantId::from_string("tenant-a".to_string()).unwrap();
        let tenant_b = TenantId::from_string("tenant-b".to_string()).unwrap();
        let capability = create_test_capability("data.write", Some("zone-b".to_string()));

        let decision = policy.evaluate(Some(&tenant_a), Some(&tenant_b), &capability);
        assert!(decision.is_allow());
    }

    #[test]
    fn test_rule_priority() {
        let mut policy = TenantBoundaryPolicy::new(vec![], BoundaryMode::Strict, "default deny");

        let tenant_a = TenantId::from_string("tenant-a".to_string()).unwrap();
        let tenant_b = TenantId::from_string("tenant-b".to_string()).unwrap();

        // Add low priority deny rule
        policy.add_rule(TenantBoundaryRule {
            name: "low-priority-deny".to_string(),
            priority: 10,
            source_tenant_pattern: Some("tenant-a".to_string()),
            target_tenant_pattern: Some("zone-*".to_string()), // Match network zone pattern
            capability_pattern: None,
            mode: BoundaryMode::Strict,
            reason: "low priority denies".to_string(),
        });

        // Add high priority allow rule
        policy.add_rule(TenantBoundaryRule {
            name: "high-priority-allow".to_string(),
            priority: 100,
            source_tenant_pattern: Some("tenant-a".to_string()),
            target_tenant_pattern: Some("zone-*".to_string()), // Match network zone pattern
            capability_pattern: None,
            mode: BoundaryMode::Permissive,
            reason: "high priority allows".to_string(),
        });

        let capability = create_test_capability("test", Some("zone-b".to_string()));
        let decision = policy.evaluate(Some(&tenant_a), Some(&tenant_b), &capability);

        // Should use high priority rule (allow)
        assert!(decision.is_allow());
        assert!(decision.reason().contains("high priority"));
    }

    #[tokio::test]
    async fn test_tenant_boundary_engine() {
        let policy = Arc::new(TenantBoundaryPolicy::permissive_policy());
        let engine = TenantBoundaryPolicyEngine::new(policy);

        let tenant_a = TenantId::from_string("tenant-a".to_string()).unwrap();

        let capability = create_test_capability("data.read", Some("zone-a".to_string()));
        let actor = ActorRef {
            id: ActorId::from_string("test-actor".to_string()).unwrap(),
            actor_type: ActorType::Agent,
            tenant_id: Some(tenant_a),
            home_node_id: None,
            display_name: "Test Actor".to_string(),
        };

        let context = EvaluationContext {
            capability,
            actor,
            execution_id: ExecutionId::new(),
            reason: Some("Test".to_string()),
        };

        let result = engine.evaluate_capability(context).await.unwrap();
        // Using permissive policy, should allow
        assert!(result.decision.is_allow());
    }
}
