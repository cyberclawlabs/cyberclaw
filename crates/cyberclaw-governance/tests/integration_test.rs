//! Integration tests for cyberclaw-governance
//!
//! Tests the full governance workflow including policy evaluation,
//! decision making, and interaction between components.

use cyberclaw_core::capability::{CapabilityEffect, CapabilityRef, RiskLevel};
use cyberclaw_core::cluster::CapabilityPlacement;
use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::{ActorId, CapabilityId, ConnectorId, ExecutionId};
use cyberclaw_governance::decision::{GovernanceDecision, ReviewType};
use cyberclaw_governance::engine::{DefaultPolicyEngine, EvaluationContext, PolicyEngine};
use cyberclaw_governance::policy::{CapabilityPolicy, PolicyAction, PolicyRule};

fn create_test_capability(
    id: &str,
    risk: RiskLevel,
    effects: Vec<CapabilityEffect>,
) -> CapabilityRef {
    CapabilityRef {
        id: CapabilityId::from_string(id.to_string()).unwrap(),
        connector_id: ConnectorId::from_string("test-connector".to_string()).unwrap(),
        risk,
        effects,
        placement: Some(CapabilityPlacement::default()),
    }
}

fn create_test_actor(id: &str, actor_type: ActorType) -> ActorRef {
    ActorRef {
        id: ActorId::from_string(id.to_string()).unwrap(),
        actor_type,
        tenant_id: None,
        home_node_id: None,
        display_name: format!("Test Actor {}", id),
    }
}

fn create_test_context(capability: CapabilityRef, actor: ActorRef) -> EvaluationContext {
    EvaluationContext {
        capability,
        actor,
        execution_id: ExecutionId::new(),
        reason: Some("Integration test".to_string()),
    }
}

#[tokio::test]
async fn test_policy_engine_lifecycle() {
    // Create a default policy engine
    let engine = DefaultPolicyEngine::default();

    // Test low risk capability - should be allowed
    let low_risk_cap =
        create_test_capability("fs.read", RiskLevel::Low, vec![CapabilityEffect::Read]);
    let agent = create_test_actor("agent-1", ActorType::Agent);
    let context = create_test_context(low_risk_cap, agent.clone());

    let result = engine.evaluate_capability(context).await.unwrap();
    assert!(result.decision.is_allow());
    assert_eq!(result.evaluated_risk, RiskLevel::Low);
    assert!(!result.context_info.is_empty());

    // Test high risk capability - should require review
    let high_risk_cap =
        create_test_capability("db.write", RiskLevel::High, vec![CapabilityEffect::Write]);
    let context = create_test_context(high_risk_cap, agent.clone());

    let result = engine.evaluate_capability(context).await.unwrap();
    assert!(result.decision.is_review_required());
    assert_eq!(result.evaluated_risk, RiskLevel::High);

    // Test critical risk capability - should require review
    let critical_cap = create_test_capability(
        "db.delete",
        RiskLevel::Critical,
        vec![CapabilityEffect::Write],
    );
    let context = create_test_context(critical_cap, agent);

    let result = engine.evaluate_capability(context).await.unwrap();
    assert!(result.decision.is_review_required());
    assert_eq!(result.evaluated_risk, RiskLevel::Critical);
}

#[tokio::test]
async fn test_multiple_policies_evaluation() {
    // Create custom policy engine with higher threshold
    let engine = DefaultPolicyEngine::new(RiskLevel::High);

    // Test that medium risk is now allowed
    let medium_cap = create_test_capability(
        "api.call",
        RiskLevel::Medium,
        vec![CapabilityEffect::Network],
    );
    let agent = create_test_actor("agent-2", ActorType::Agent);
    let context = create_test_context(medium_cap, agent.clone());

    let result = engine.evaluate_capability(context).await.unwrap();
    assert!(result.decision.is_allow());
    assert_eq!(result.evaluated_risk, RiskLevel::Medium);

    // High risk equals threshold, so it's allowed (only > threshold requires review)
    let high_cap = create_test_capability(
        "system.execute",
        RiskLevel::High,
        vec![CapabilityEffect::Execute],
    );
    let context = create_test_context(high_cap, agent.clone());

    let result = engine.evaluate_capability(context).await.unwrap();
    assert!(result.decision.is_allow()); // High == High threshold, so allowed

    // But critical risk requires review (Critical > High)
    let critical_cap = create_test_capability(
        "system.shutdown",
        RiskLevel::Critical,
        vec![CapabilityEffect::Execute],
    );
    let context = create_test_context(critical_cap, agent);

    let result = engine.evaluate_capability(context).await.unwrap();
    assert!(result.decision.is_review_required());
}

#[tokio::test]
async fn test_policy_conflict_resolution() {
    // Create a policy with conflicting rules to test priority resolution
    let policy = CapabilityPolicy::default();

    // Test that high priority rules take precedence
    let critical_cap = create_test_capability(
        "critical.operation",
        RiskLevel::Critical,
        vec![CapabilityEffect::Write],
    );
    let agent = create_test_actor("agent-3", ActorType::Agent);

    let decision = policy.evaluate(&critical_cap, &agent);

    // Critical risk should require approval (not just review)
    assert!(decision.is_review_required());
    assert_eq!(decision.review_type(), Some(&ReviewType::Approval));
}

#[test]
fn test_policy_rule_matching_logic() {
    // Create a complex policy rule
    let rule = PolicyRule {
        name: "complex-rule".to_string(),
        priority: 50,
        capability_id: Some(CapabilityId::from_string("specific.cap".to_string()).unwrap()),
        pattern: None,
        risk_level: Some(RiskLevel::High),
        effect: Some(CapabilityEffect::Write),
        actor: None,
        tenant_id: None,
        source: cyberclaw_governance::PolicySource::SystemDefault,
        action: PolicyAction::RequireSecurityReview,
        reason: "High risk write operation".to_string(),
    };

    // Test exact match
    let matching_cap = create_test_capability(
        "specific.cap",
        RiskLevel::High,
        vec![CapabilityEffect::Write],
    );
    let agent = create_test_actor("agent-4", ActorType::Agent);
    assert!(rule.matches(&matching_cap, &agent));

    // Test non-matching capability ID
    let wrong_id_cap =
        create_test_capability("other.cap", RiskLevel::High, vec![CapabilityEffect::Write]);
    assert!(!rule.matches(&wrong_id_cap, &agent));

    // Test non-matching risk level
    let wrong_risk_cap = create_test_capability(
        "specific.cap",
        RiskLevel::Low,
        vec![CapabilityEffect::Write],
    );
    assert!(!rule.matches(&wrong_risk_cap, &agent));

    // Test non-matching effect
    let wrong_effect_cap = create_test_capability(
        "specific.cap",
        RiskLevel::High,
        vec![CapabilityEffect::Read],
    );
    assert!(!rule.matches(&wrong_effect_cap, &agent));
}

#[test]
fn test_policy_priority_ordering_comprehensive() {
    // Create a policy with multiple rules of different priorities
    let mut policy = CapabilityPolicy::new(vec![], PolicyAction::Deny, "Default deny for safety");

    // Add rules in random priority order
    policy.add_rule(PolicyRule {
        name: "medium-priority".to_string(),
        priority: 50,
        capability_id: None,
        pattern: None,
        risk_level: Some(RiskLevel::Medium),
        effect: None,
        actor: None,
        tenant_id: None,
        source: cyberclaw_governance::PolicySource::SystemDefault,
        action: PolicyAction::Allow,
        reason: "Medium priority allows medium risk".to_string(),
    });

    policy.add_rule(PolicyRule {
        name: "high-priority".to_string(),
        priority: 100,
        capability_id: None,
        pattern: None,
        risk_level: Some(RiskLevel::Medium),
        effect: None,
        actor: None,
        tenant_id: None,
        source: cyberclaw_governance::PolicySource::SystemDefault,
        action: PolicyAction::Deny,
        reason: "High priority denies medium risk".to_string(),
    });

    policy.add_rule(PolicyRule {
        name: "low-priority".to_string(),
        priority: 10,
        capability_id: None,
        pattern: None,
        risk_level: Some(RiskLevel::Medium),
        effect: None,
        actor: None,
        tenant_id: None,
        source: cyberclaw_governance::PolicySource::SystemDefault,
        action: PolicyAction::RequireHumanReview,
        reason: "Low priority requires review".to_string(),
    });

    // Evaluate with medium risk capability
    let cap = create_test_capability("test.cap", RiskLevel::Medium, vec![]);
    let agent = create_test_actor("agent-5", ActorType::Agent);

    let decision = policy.evaluate(&cap, &agent);

    // Should match highest priority rule (100) which denies
    assert!(decision.is_deny());
    assert_eq!(decision.reason(), "High priority denies medium risk");
}

#[tokio::test]
async fn test_end_to_end_governance_workflow() {
    // Simulate a complete governance workflow from request to decision

    // Step 1: Create actor (representing an AI agent)
    let agent = create_test_actor("security-scanner", ActorType::Agent);

    // Step 2: Create capability request (file system read - low risk)
    let read_cap = create_test_capability("fs.read", RiskLevel::Low, vec![CapabilityEffect::Read]);

    // Step 3: Evaluate with default policy engine
    let engine = DefaultPolicyEngine::default();
    let context = create_test_context(read_cap.clone(), agent.clone());
    let result = engine.evaluate_capability(context).await.unwrap();

    // Step 4: Verify low risk is allowed
    assert!(result.decision.is_allow());
    assert!(result.decision.reason().contains("acceptable threshold"));

    // Step 5: Try a high-risk operation (database write)
    let write_cap =
        create_test_capability("db.write", RiskLevel::High, vec![CapabilityEffect::Write]);
    let context = create_test_context(write_cap, agent.clone());
    let result = engine.evaluate_capability(context).await.unwrap();

    // Step 6: Verify high risk requires review
    assert!(result.decision.is_review_required());
    assert!(result.decision.reason().contains("exceeds threshold"));

    // Step 7: Test with custom policy
    let policy = CapabilityPolicy::default();
    let critical_cap = create_test_capability(
        "system.shutdown",
        RiskLevel::Critical,
        vec![CapabilityEffect::Execute],
    );
    let decision = policy.evaluate(&critical_cap, &agent);

    // Step 8: Verify critical requires approval
    assert!(decision.is_review_required());
    assert_eq!(decision.review_type(), Some(&ReviewType::Approval));
}

#[test]
fn test_governance_decision_helpers() {
    // Test all decision types and their helper methods
    let allow = GovernanceDecision::allow("Safe operation");
    assert!(allow.is_allow());
    assert!(!allow.is_deny());
    assert!(!allow.is_review_required());
    assert_eq!(allow.reason(), "Safe operation");
    assert!(allow.review_type().is_none());

    let deny = GovernanceDecision::deny("Unsafe operation");
    assert!(!deny.is_allow());
    assert!(deny.is_deny());
    assert!(!deny.is_review_required());
    assert_eq!(deny.reason(), "Unsafe operation");
    assert!(deny.review_type().is_none());

    let review = GovernanceDecision::review_required("Needs approval", ReviewType::Approval);
    assert!(!review.is_allow());
    assert!(!review.is_deny());
    assert!(review.is_review_required());
    assert_eq!(review.reason(), "Needs approval");
    assert_eq!(review.review_type(), Some(&ReviewType::Approval));
}

#[test]
fn test_policy_action_conversions() {
    // Test all PolicyAction to GovernanceDecision conversions
    let actions = vec![
        (PolicyAction::Allow, "allowed"),
        (PolicyAction::Deny, "denied"),
        (PolicyAction::RequireHumanReview, "human review"),
        (PolicyAction::RequireApproval, "approval"),
        (PolicyAction::RequireSecurityReview, "security review"),
        (PolicyAction::RequireEscalation, "escalation"),
    ];

    for (action, reason) in actions {
        let decision = action.to_decision(reason);
        assert_eq!(decision.reason(), reason);

        match action {
            PolicyAction::Allow => assert!(decision.is_allow()),
            PolicyAction::Deny => assert!(decision.is_deny()),
            _ => {
                assert!(decision.is_review_required());
                assert!(decision.review_type().is_some());
            }
        }
    }
}

// ============================================================================
// PersistentPolicyEngine Integration Tests
// ============================================================================

use cyberclaw_governance::PersistentPolicyEngine;
use cyberclaw_store::memory::InMemoryStateStore;
use std::sync::Arc;

#[tokio::test]
async fn test_persistent_engine_basic_lifecycle() {
    // Create a persistent policy engine with in-memory store
    let store = Arc::new(InMemoryStateStore::new());
    let engine = PersistentPolicyEngine::with_default_policies(store.clone())
        .await
        .unwrap();

    // Add a policy rule
    let rule = PolicyRule {
        name: "allow-low-risk".to_string(),
        priority: 10,
        capability_id: None,
        pattern: None,
        risk_level: Some(RiskLevel::Low),
        effect: None,
        actor: None,
        tenant_id: None,
        source: cyberclaw_governance::PolicySource::SystemDefault,
        action: PolicyAction::Allow,
        reason: "Low risk is always safe".to_string(),
    };

    engine.add_policy(rule).await.unwrap();

    // Evaluate a low-risk capability
    let cap = create_test_capability("test.read", RiskLevel::Low, vec![CapabilityEffect::Read]);
    let actor = create_test_actor("test-agent", ActorType::Agent);
    let context = create_test_context(cap, actor);

    let result = engine.evaluate_capability(context).await.unwrap();
    assert!(result.decision.is_allow());
    assert_eq!(result.evaluated_risk, RiskLevel::Low);
}

#[tokio::test]
async fn test_persistent_engine_reload_across_instances() {
    // Create first engine instance and add policies
    let store = Arc::new(InMemoryStateStore::new());
    let engine1 = PersistentPolicyEngine::new(store.clone(), PolicyAction::Deny, "Default deny")
        .await
        .unwrap();

    // Add multiple policy rules
    let rules = vec![
        PolicyRule {
            name: "allow-read".to_string(),
            priority: 10,
            capability_id: None,
            pattern: None,
            risk_level: None,
            effect: Some(CapabilityEffect::Read),
            actor: None,
            tenant_id: None,
            source: cyberclaw_governance::PolicySource::SystemDefault,
            action: PolicyAction::Allow,
            reason: "Reads are safe".to_string(),
        },
        PolicyRule {
            name: "deny-critical".to_string(),
            priority: 20,
            capability_id: None,
            pattern: None,
            risk_level: Some(RiskLevel::Critical),
            effect: None,
            actor: None,
            tenant_id: None,
            source: cyberclaw_governance::PolicySource::SystemDefault,
            action: PolicyAction::Deny,
            reason: "Critical is too risky".to_string(),
        },
    ];

    for rule in rules {
        engine1.add_policy(rule).await.unwrap();
    }

    // Create second engine instance from same store (simulates restart)
    let engine2 = PersistentPolicyEngine::new(store.clone(), PolicyAction::Deny, "Default deny")
        .await
        .unwrap();

    // Verify policies were loaded
    let loaded_rules = engine2.get_rules().await;
    assert_eq!(loaded_rules.len(), 2);
    assert!(loaded_rules.iter().any(|r| r.name == "allow-read"));
    assert!(loaded_rules.iter().any(|r| r.name == "deny-critical"));

    // Verify evaluation works correctly with loaded policies
    let read_cap =
        create_test_capability("test.read", RiskLevel::Low, vec![CapabilityEffect::Read]);
    let actor = create_test_actor("test-agent", ActorType::Agent);
    let context = create_test_context(read_cap, actor);

    let result = engine2.evaluate_capability(context).await.unwrap();
    assert!(result.decision.is_allow());
}

#[tokio::test]
async fn test_persistent_engine_activate_deactivate() {
    let store = Arc::new(InMemoryStateStore::new());
    let engine = PersistentPolicyEngine::new(
        store.clone(),
        PolicyAction::RequireHumanReview,
        "Default review",
    )
    .await
    .unwrap();

    // Add a policy rule
    let rule = PolicyRule {
        name: "allow-medium-risk".to_string(),
        priority: 10,
        capability_id: None,
        pattern: None,
        risk_level: Some(RiskLevel::Medium),
        effect: None,
        actor: None,
        tenant_id: None,
        source: cyberclaw_governance::PolicySource::SystemDefault,
        action: PolicyAction::Allow,
        reason: "Medium risk allowed".to_string(),
    };

    engine.add_policy(rule).await.unwrap();

    // Test with active policy
    let cap = create_test_capability("test.op", RiskLevel::Medium, vec![]);
    let actor = create_test_actor("test-agent", ActorType::Agent);
    let context1 = create_test_context(cap.clone(), actor.clone());

    let result = engine.evaluate_capability(context1).await.unwrap();
    assert!(result.decision.is_allow());

    // Deactivate the policy
    engine.deactivate_policy("allow-medium-risk").await.unwrap();

    // Test with deactivated policy (should fall back to default)
    let context2 = create_test_context(cap, actor);
    let result = engine.evaluate_capability(context2).await.unwrap();
    assert!(result.decision.is_review_required());

    // Reactivate the policy
    engine.activate_policy("allow-medium-risk").await.unwrap();

    // Test with reactivated policy
    let cap2 = create_test_capability("test.op2", RiskLevel::Medium, vec![]);
    let actor2 = create_test_actor("test-agent2", ActorType::Agent);
    let context3 = create_test_context(cap2, actor2);

    let result = engine.evaluate_capability(context3).await.unwrap();
    assert!(result.decision.is_allow());
}

#[tokio::test]
async fn test_persistent_engine_priority_ordering() {
    let store = Arc::new(InMemoryStateStore::new());
    let engine = PersistentPolicyEngine::new(store.clone(), PolicyAction::Deny, "Default deny")
        .await
        .unwrap();

    // Add rules with different priorities
    let low_priority = PolicyRule {
        name: "low-priority-allow".to_string(),
        priority: 10,
        capability_id: None,
        pattern: None,
        risk_level: Some(RiskLevel::High),
        effect: None,
        actor: None,
        tenant_id: None,
        source: cyberclaw_governance::PolicySource::SystemDefault,
        action: PolicyAction::Allow,
        reason: "Low priority allows".to_string(),
    };

    let high_priority = PolicyRule {
        name: "high-priority-deny".to_string(),
        priority: 100,
        capability_id: None,
        pattern: None,
        risk_level: Some(RiskLevel::High),
        effect: None,
        actor: None,
        tenant_id: None,
        source: cyberclaw_governance::PolicySource::SystemDefault,
        action: PolicyAction::Deny,
        reason: "High priority denies".to_string(),
    };

    // Add in reverse priority order
    engine.add_policy(low_priority).await.unwrap();
    engine.add_policy(high_priority).await.unwrap();

    // Evaluate - should match high priority (deny)
    let cap = create_test_capability("test.high", RiskLevel::High, vec![]);
    let actor = create_test_actor("test-agent", ActorType::Agent);
    let context = create_test_context(cap, actor);

    let result = engine.evaluate_capability(context).await.unwrap();
    assert!(result.decision.is_deny());
    assert_eq!(result.decision.reason(), "High priority denies");
}

#[tokio::test]
async fn test_persistent_engine_complex_matching() {
    let store = Arc::new(InMemoryStateStore::new());
    let engine = PersistentPolicyEngine::new(
        store.clone(),
        PolicyAction::RequireHumanReview,
        "Default review",
    )
    .await
    .unwrap();

    // Add a specific rule that matches capability ID + risk + effect
    let specific_rule = PolicyRule {
        name: "specific-allow".to_string(),
        priority: 50,
        capability_id: Some(CapabilityId::from_string("db.read".to_string()).unwrap()),
        pattern: None,
        risk_level: Some(RiskLevel::Medium),
        effect: Some(CapabilityEffect::Read),
        actor: None,
        tenant_id: None,
        source: cyberclaw_governance::PolicySource::SystemDefault,
        action: PolicyAction::Allow,
        reason: "Specific DB read allowed".to_string(),
    };

    engine.add_policy(specific_rule).await.unwrap();

    // Test exact match - should allow
    let matching_cap =
        create_test_capability("db.read", RiskLevel::Medium, vec![CapabilityEffect::Read]);
    let actor = create_test_actor("test-agent", ActorType::Agent);
    let context1 = create_test_context(matching_cap, actor.clone());

    let result = engine.evaluate_capability(context1).await.unwrap();
    assert!(result.decision.is_allow());

    // Test non-matching capability ID - should require review (default)
    let non_matching_cap =
        create_test_capability("db.write", RiskLevel::Medium, vec![CapabilityEffect::Read]);
    let context2 = create_test_context(non_matching_cap, actor.clone());

    let result = engine.evaluate_capability(context2).await.unwrap();
    assert!(result.decision.is_review_required());

    // Test non-matching effect - should require review (default)
    let wrong_effect_cap =
        create_test_capability("db.read", RiskLevel::Medium, vec![CapabilityEffect::Write]);
    let context3 = create_test_context(wrong_effect_cap, actor);

    let result = engine.evaluate_capability(context3).await.unwrap();
    assert!(result.decision.is_review_required());
}

#[tokio::test]
async fn test_persistent_engine_concurrent_evaluation() {
    // Test that concurrent evaluations work correctly
    let store = Arc::new(InMemoryStateStore::new());
    let engine = Arc::new(
        PersistentPolicyEngine::new(store.clone(), PolicyAction::Deny, "Default deny")
            .await
            .unwrap(),
    );

    // Add a policy
    let rule = PolicyRule {
        name: "allow-low".to_string(),
        priority: 10,
        capability_id: None,
        pattern: None,
        risk_level: Some(RiskLevel::Low),
        effect: None,
        actor: None,
        tenant_id: None,
        source: cyberclaw_governance::PolicySource::SystemDefault,
        action: PolicyAction::Allow,
        reason: "Low risk allowed".to_string(),
    };

    engine.add_policy(rule).await.unwrap();

    // Spawn multiple concurrent evaluations
    let mut handles = vec![];
    for i in 0..10 {
        let engine_clone = engine.clone();
        let handle = tokio::spawn(async move {
            let cap = create_test_capability(
                &format!("test.cap{}", i),
                RiskLevel::Low,
                vec![CapabilityEffect::Read],
            );
            let actor = create_test_actor(&format!("agent-{}", i), ActorType::Agent);
            let context = create_test_context(cap, actor);

            engine_clone.evaluate_capability(context).await.unwrap()
        });
        handles.push(handle);
    }

    // Wait for all evaluations to complete
    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.decision.is_allow());
    }
}
