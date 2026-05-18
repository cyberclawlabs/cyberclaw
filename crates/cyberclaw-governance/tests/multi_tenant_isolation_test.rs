//! Multi-tenant isolation integration tests
//!
//! These tests verify that tenants are properly isolated from each other across:
//! - Quota management (tenant A's quota doesn't affect tenant B)
//! - Policy evaluation (tenant A's policies don't apply to tenant B)
//! - Metrics collection (tenant metrics are properly segregated)
//! - Cross-tenant access prevention

use cyberclaw_core::capability::{CapabilityEffect, CapabilityRef, RiskLevel};
use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::{ActorId, CapabilityId, ConnectorId, ExecutionId, TenantId};
use cyberclaw_governance::engine::{EvaluationContext, PolicyEngine};
use cyberclaw_governance::{
    CapabilityPolicy, PolicyAction, PolicyRule, QuotaPolicyEngine, TenantQuota, TenantQuotaManager,
};
use std::sync::Arc;

/// Helper function to create a test capability
fn create_test_capability(id: &str, risk: RiskLevel) -> CapabilityRef {
    CapabilityRef {
        id: CapabilityId::from_string(id.to_string()).unwrap(),
        connector_id: ConnectorId::from_string("test-connector".to_string()).unwrap(),
        risk,
        effects: vec![CapabilityEffect::Read],
        placement: None,
    }
}

/// Helper function to create a test actor with tenant_id
fn create_test_actor(actor_id: &str, tenant_id: Option<TenantId>) -> ActorRef {
    ActorRef {
        id: ActorId::from_string(actor_id.to_string()).unwrap(),
        actor_type: ActorType::Agent,
        tenant_id,
        home_node_id: None,
        display_name: actor_id.to_string(),
    }
}

#[tokio::test]
async fn test_tenant_quota_isolation() {
    // Test that tenant A reaching quota doesn't affect tenant B
    let default_tenant = TenantId::from_string("default".to_string()).unwrap();
    let manager = Arc::new(TenantQuotaManager::new(TenantQuota::unlimited(
        default_tenant,
    )));

    let tenant_a = TenantId::from_string("tenant-a".to_string()).unwrap();
    let tenant_b = TenantId::from_string("tenant-b".to_string()).unwrap();

    // Set quota for tenant A: max 2 concurrent executions
    let mut quota_a = TenantQuota::new(tenant_a.clone());
    quota_a.max_concurrent_executions = Some(2);
    quota_a.enabled = true;
    manager.set_quota(quota_a);

    // Set quota for tenant B: max 5 concurrent executions
    let mut quota_b = TenantQuota::new(tenant_b.clone());
    quota_b.max_concurrent_executions = Some(5);
    quota_b.enabled = true;
    manager.set_quota(quota_b);

    let engine = QuotaPolicyEngine::new(manager.clone());

    let capability = create_test_capability("test-cap", RiskLevel::Low);

    // Tenant A: first 2 executions should be allowed
    let actor_a1 = create_test_actor("actor-a1", Some(tenant_a.clone()));
    let actor_a2 = create_test_actor("actor-a2", Some(tenant_a.clone()));

    let context_a1 = EvaluationContext {
        capability: capability.clone(),
        actor: actor_a1,
        execution_id: ExecutionId::new(),
        reason: Some("Test execution 1".to_string()),
    };

    let result_a1 = engine.evaluate_capability(context_a1).await.unwrap();
    assert!(
        result_a1.decision.is_allow(),
        "Tenant A first execution should be allowed"
    );
    manager.start_execution(&tenant_a);

    let context_a2 = EvaluationContext {
        capability: capability.clone(),
        actor: actor_a2,
        execution_id: ExecutionId::new(),
        reason: Some("Test execution 2".to_string()),
    };

    let result_a2 = engine.evaluate_capability(context_a2).await.unwrap();
    assert!(
        result_a2.decision.is_allow(),
        "Tenant A second execution should be allowed"
    );
    manager.start_execution(&tenant_a);

    // Tenant A: third execution should be denied (quota exceeded)
    let actor_a3 = create_test_actor("actor-a3", Some(tenant_a.clone()));
    let context_a3 = EvaluationContext {
        capability: capability.clone(),
        actor: actor_a3,
        execution_id: ExecutionId::new(),
        reason: Some("Test execution 3".to_string()),
    };

    let result_a3 = engine.evaluate_capability(context_a3).await.unwrap();
    assert!(
        result_a3.decision.is_deny(),
        "Tenant A third execution should be denied (quota exceeded)"
    );

    // Tenant B: should still be able to execute (not affected by tenant A's quota)
    let actor_b1 = create_test_actor("actor-b1", Some(tenant_b.clone()));
    let actor_b2 = create_test_actor("actor-b2", Some(tenant_b.clone()));
    let actor_b3 = create_test_actor("actor-b3", Some(tenant_b.clone()));

    let context_b1 = EvaluationContext {
        capability: capability.clone(),
        actor: actor_b1,
        execution_id: ExecutionId::new(),
        reason: Some("Test execution B1".to_string()),
    };

    let result_b1 = engine.evaluate_capability(context_b1).await.unwrap();
    assert!(
        result_b1.decision.is_allow(),
        "Tenant B should not be affected by tenant A's quota"
    );

    let context_b2 = EvaluationContext {
        capability: capability.clone(),
        actor: actor_b2,
        execution_id: ExecutionId::new(),
        reason: Some("Test execution B2".to_string()),
    };

    let result_b2 = engine.evaluate_capability(context_b2).await.unwrap();
    assert!(
        result_b2.decision.is_allow(),
        "Tenant B execution 2 should succeed"
    );

    let context_b3 = EvaluationContext {
        capability: capability.clone(),
        actor: actor_b3,
        execution_id: ExecutionId::new(),
        reason: Some("Test execution B3".to_string()),
    };

    let result_b3 = engine.evaluate_capability(context_b3).await.unwrap();
    assert!(
        result_b3.decision.is_allow(),
        "Tenant B execution 3 should succeed"
    );
}

#[test]
fn test_tenant_policy_isolation() {
    // Test that tenant A's policies don't apply to tenant B
    let tenant_a = TenantId::from_string("tenant-a".to_string()).unwrap();
    let tenant_b = TenantId::from_string("tenant-b".to_string()).unwrap();

    // Create a policy with tenant-specific rules
    let mut policy = CapabilityPolicy::new(vec![], PolicyAction::Allow, "Default allow");

    // Tenant A rule: deny high-risk capabilities
    policy.add_rule(PolicyRule {
        name: "tenant-a-deny-high-risk".to_string(),
        priority: 100,
        capability_id: None,
        pattern: None,
        risk_level: Some(RiskLevel::High),
        effect: None,
        actor: None,
        tenant_id: Some(tenant_a.clone()),
        source: cyberclaw_governance::PolicySource::SystemDefault,
        action: PolicyAction::Deny,
        reason: "Tenant A policy: high risk capabilities are denied".to_string(),
    });

    // Tenant B rule: allow high-risk capabilities
    policy.add_rule(PolicyRule {
        name: "tenant-b-allow-high-risk".to_string(),
        priority: 100,
        capability_id: None,
        pattern: None,
        risk_level: Some(RiskLevel::High),
        effect: None,
        actor: None,
        tenant_id: Some(tenant_b.clone()),
        source: cyberclaw_governance::PolicySource::SystemDefault,
        action: PolicyAction::Allow,
        reason: "Tenant B policy: high risk capabilities are allowed".to_string(),
    });

    let high_risk_cap = create_test_capability("high-risk-cap", RiskLevel::High);

    // Tenant A actor should be denied
    let actor_a = create_test_actor("actor-a", Some(tenant_a));
    let decision_a = policy.evaluate(&high_risk_cap, &actor_a);
    assert!(
        decision_a.is_deny(),
        "Tenant A should be denied by tenant-specific policy"
    );
    assert!(decision_a
        .reason()
        .contains("Tenant A policy: high risk capabilities are denied"));

    // Tenant B actor should be allowed
    let actor_b = create_test_actor("actor-b", Some(tenant_b));
    let decision_b = policy.evaluate(&high_risk_cap, &actor_b);
    assert!(
        decision_b.is_allow(),
        "Tenant B should be allowed by tenant-specific policy"
    );
    assert!(decision_b
        .reason()
        .contains("Tenant B policy: high risk capabilities are allowed"));

    // Actor with no tenant should fall back to default policy
    let actor_no_tenant = create_test_actor("actor-no-tenant", None);
    let decision_no_tenant = policy.evaluate(&high_risk_cap, &actor_no_tenant);
    assert!(
        decision_no_tenant.is_allow(),
        "Actor with no tenant should use default policy (allow)"
    );
    assert!(decision_no_tenant.reason().contains("Default allow"));
}

#[test]
fn test_cross_tenant_policy_leak_prevention() {
    // Test that a tenant-specific policy doesn't accidentally match another tenant
    let tenant_a = TenantId::from_string("tenant-a".to_string()).unwrap();
    let tenant_b = TenantId::from_string("tenant-b".to_string()).unwrap();

    let mut policy = CapabilityPolicy::new(
        vec![],
        PolicyAction::RequireHumanReview,
        "Default review required",
    );

    // Tenant A has a very permissive rule (allow all low risk)
    policy.add_rule(PolicyRule {
        name: "tenant-a-allow-all-low".to_string(),
        priority: 50,
        capability_id: None,
        pattern: None,
        risk_level: Some(RiskLevel::Low),
        effect: None,
        actor: None,
        tenant_id: Some(tenant_a.clone()),
        source: cyberclaw_governance::PolicySource::SystemDefault,
        action: PolicyAction::Allow,
        reason: "Tenant A: allow all low risk".to_string(),
    });

    let low_risk_cap = create_test_capability("low-risk-cap", RiskLevel::Low);

    // Tenant A should match the permissive rule
    let actor_a = create_test_actor("actor-a", Some(tenant_a));
    let decision_a = policy.evaluate(&low_risk_cap, &actor_a);
    assert!(
        decision_a.is_allow(),
        "Tenant A should match its tenant-specific rule"
    );

    // Tenant B should NOT match tenant A's rule (should fall back to default)
    let actor_b = create_test_actor("actor-b", Some(tenant_b));
    let decision_b = policy.evaluate(&low_risk_cap, &actor_b);
    assert!(
        decision_b.is_review_required(),
        "Tenant B should NOT match tenant A's rule, should use default"
    );
    assert!(decision_b.reason().contains("Default review required"));
}

#[test]
fn test_system_wide_policy_applies_to_all_tenants() {
    // Test that policies with no tenant_id apply to all tenants
    let tenant_a = TenantId::from_string("tenant-a".to_string()).unwrap();
    let tenant_b = TenantId::from_string("tenant-b".to_string()).unwrap();

    let policy = CapabilityPolicy::new(
        vec![
            // System-wide rule: deny critical risk for everyone
            PolicyRule {
                name: "system-wide-deny-critical".to_string(),
                priority: 200,
                capability_id: None,
                pattern: None,
                risk_level: Some(RiskLevel::Critical),
                effect: None,
                actor: None,
                tenant_id: None, // System-wide (applies to all tenants)
                source: cyberclaw_governance::PolicySource::SystemDefault,
                action: PolicyAction::Deny,
                reason: "System-wide policy: critical risk capabilities are denied".to_string(),
            },
        ],
        PolicyAction::Allow,
        "Default allow",
    );

    let critical_cap = create_test_capability("critical-cap", RiskLevel::Critical);

    // Both tenant A and tenant B should be affected by the system-wide rule
    let actor_a = create_test_actor("actor-a", Some(tenant_a));
    let decision_a = policy.evaluate(&critical_cap, &actor_a);
    assert!(
        decision_a.is_deny(),
        "Tenant A should be denied by system-wide policy"
    );
    assert!(decision_a
        .reason()
        .contains("System-wide policy: critical risk capabilities are denied"));

    let actor_b = create_test_actor("actor-b", Some(tenant_b));
    let decision_b = policy.evaluate(&critical_cap, &actor_b);
    assert!(
        decision_b.is_deny(),
        "Tenant B should be denied by system-wide policy"
    );
    assert!(decision_b
        .reason()
        .contains("System-wide policy: critical risk capabilities are denied"));

    // Actor with no tenant should also be affected
    let actor_no_tenant = create_test_actor("actor-no-tenant", None);
    let decision_no_tenant = policy.evaluate(&critical_cap, &actor_no_tenant);
    assert!(
        decision_no_tenant.is_deny(),
        "System actor should be denied by system-wide policy"
    );
}

#[test]
fn test_tenant_priority_over_system_policy() {
    // Test that tenant-specific policies with higher priority override system-wide policies
    let tenant_a = TenantId::from_string("tenant-a".to_string()).unwrap();
    let tenant_b = TenantId::from_string("tenant-b".to_string()).unwrap();

    let policy = CapabilityPolicy::new(
        vec![
            // System-wide rule: require review for high risk (priority 50)
            PolicyRule {
                name: "system-review-high-risk".to_string(),
                priority: 50,
                capability_id: None,
                pattern: None,
                risk_level: Some(RiskLevel::High),
                effect: None,
                actor: None,
                tenant_id: None,
                source: cyberclaw_governance::PolicySource::SystemDefault,
                action: PolicyAction::RequireHumanReview,
                reason: "System policy: high risk requires review".to_string(),
            },
            // Tenant A override: allow high risk (priority 100)
            PolicyRule {
                name: "tenant-a-allow-high-risk".to_string(),
                priority: 100,
                capability_id: None,
                pattern: None,
                risk_level: Some(RiskLevel::High),
                effect: None,
                actor: None,
                tenant_id: Some(tenant_a.clone()),
                source: cyberclaw_governance::PolicySource::SystemDefault,
                action: PolicyAction::Allow,
                reason: "Tenant A override: high risk allowed".to_string(),
            },
        ],
        PolicyAction::Deny,
        "Default deny",
    );

    let high_risk_cap = create_test_capability("high-risk-cap", RiskLevel::High);

    // Tenant A should match the higher-priority tenant-specific rule (allow)
    let actor_a = create_test_actor("actor-a", Some(tenant_a));
    let decision_a = policy.evaluate(&high_risk_cap, &actor_a);
    assert!(
        decision_a.is_allow(),
        "Tenant A should use tenant-specific higher-priority rule"
    );
    assert!(decision_a
        .reason()
        .contains("Tenant A override: high risk allowed"));

    // Tenant B should match the system-wide rule (review required)
    let actor_b = create_test_actor("actor-b", Some(tenant_b));
    let decision_b = policy.evaluate(&high_risk_cap, &actor_b);
    assert!(
        decision_b.is_review_required(),
        "Tenant B should match system-wide rule"
    );
    assert!(decision_b
        .reason()
        .contains("System policy: high risk requires review"));
}

#[tokio::test]
async fn test_multiple_tenants_concurrent_quota_usage() {
    // Test that multiple tenants can independently use their quotas concurrently
    let default_tenant = TenantId::from_string("default".to_string()).unwrap();
    let manager = Arc::new(TenantQuotaManager::new(TenantQuota::unlimited(
        default_tenant,
    )));

    let tenant_a = TenantId::from_string("tenant-a".to_string()).unwrap();
    let tenant_b = TenantId::from_string("tenant-b".to_string()).unwrap();
    let tenant_c = TenantId::from_string("tenant-c".to_string()).unwrap();

    // Set different quotas for each tenant
    let mut quota_a = TenantQuota::new(tenant_a.clone());
    quota_a.max_concurrent_executions = Some(1);
    quota_a.enabled = true;
    manager.set_quota(quota_a);

    let mut quota_b = TenantQuota::new(tenant_b.clone());
    quota_b.max_concurrent_executions = Some(2);
    quota_b.enabled = true;
    manager.set_quota(quota_b);

    let mut quota_c = TenantQuota::new(tenant_c.clone());
    quota_c.max_concurrent_executions = Some(3);
    quota_c.enabled = true;
    manager.set_quota(quota_c);

    let engine = QuotaPolicyEngine::new(manager.clone());
    let capability = create_test_capability("test-cap", RiskLevel::Low);

    // Tenant A: 1 allowed, 2nd denied
    let actor_a1 = create_test_actor("actor-a1", Some(tenant_a.clone()));
    let actor_a2 = create_test_actor("actor-a2", Some(tenant_a.clone()));

    let context_a1 = EvaluationContext {
        capability: capability.clone(),
        actor: actor_a1,
        execution_id: ExecutionId::new(),
        reason: Some("Test A1".to_string()),
    };

    assert!(engine
        .evaluate_capability(context_a1)
        .await
        .unwrap()
        .decision
        .is_allow());
    manager.start_execution(&tenant_a);

    let context_a2 = EvaluationContext {
        capability: capability.clone(),
        actor: actor_a2,
        execution_id: ExecutionId::new(),
        reason: Some("Test A2".to_string()),
    };

    assert!(engine
        .evaluate_capability(context_a2)
        .await
        .unwrap()
        .decision
        .is_deny());

    // Tenant B: 2 allowed, 3rd denied
    let actor_b1 = create_test_actor("actor-b1", Some(tenant_b.clone()));
    let actor_b2 = create_test_actor("actor-b2", Some(tenant_b.clone()));
    let actor_b3 = create_test_actor("actor-b3", Some(tenant_b.clone()));

    let context_b1 = EvaluationContext {
        capability: capability.clone(),
        actor: actor_b1,
        execution_id: ExecutionId::new(),
        reason: Some("Test B1".to_string()),
    };

    assert!(engine
        .evaluate_capability(context_b1)
        .await
        .unwrap()
        .decision
        .is_allow());
    manager.start_execution(&tenant_b);

    let context_b2 = EvaluationContext {
        capability: capability.clone(),
        actor: actor_b2,
        execution_id: ExecutionId::new(),
        reason: Some("Test B2".to_string()),
    };

    assert!(engine
        .evaluate_capability(context_b2)
        .await
        .unwrap()
        .decision
        .is_allow());
    manager.start_execution(&tenant_b);

    let context_b3 = EvaluationContext {
        capability: capability.clone(),
        actor: actor_b3,
        execution_id: ExecutionId::new(),
        reason: Some("Test B3".to_string()),
    };

    assert!(engine
        .evaluate_capability(context_b3)
        .await
        .unwrap()
        .decision
        .is_deny());

    // Tenant C: 3 allowed, 4th denied
    let actor_c1 = create_test_actor("actor-c1", Some(tenant_c.clone()));
    let actor_c2 = create_test_actor("actor-c2", Some(tenant_c.clone()));
    let actor_c3 = create_test_actor("actor-c3", Some(tenant_c.clone()));
    let actor_c4 = create_test_actor("actor-c4", Some(tenant_c.clone()));

    let context_c1 = EvaluationContext {
        capability: capability.clone(),
        actor: actor_c1,
        execution_id: ExecutionId::new(),
        reason: Some("Test C1".to_string()),
    };

    assert!(engine
        .evaluate_capability(context_c1)
        .await
        .unwrap()
        .decision
        .is_allow());
    manager.start_execution(&tenant_c);

    let context_c2 = EvaluationContext {
        capability: capability.clone(),
        actor: actor_c2,
        execution_id: ExecutionId::new(),
        reason: Some("Test C2".to_string()),
    };

    assert!(engine
        .evaluate_capability(context_c2)
        .await
        .unwrap()
        .decision
        .is_allow());
    manager.start_execution(&tenant_c);

    let context_c3 = EvaluationContext {
        capability: capability.clone(),
        actor: actor_c3,
        execution_id: ExecutionId::new(),
        reason: Some("Test C3".to_string()),
    };

    assert!(engine
        .evaluate_capability(context_c3)
        .await
        .unwrap()
        .decision
        .is_allow());
    manager.start_execution(&tenant_c);

    let context_c4 = EvaluationContext {
        capability: capability.clone(),
        actor: actor_c4,
        execution_id: ExecutionId::new(),
        reason: Some("Test C4".to_string()),
    };

    assert!(engine
        .evaluate_capability(context_c4)
        .await
        .unwrap()
        .decision
        .is_deny());
}
