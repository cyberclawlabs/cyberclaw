use crate::capability::RiskLevel;
use crate::identity::{ActorRef, ActorType};
use crate::ids::{
    ApprovalStepId, CapabilityId, CaseId, ExecutionId, HandoffId, LeaseId, NodeId,
    PolicyDecisionId, ReviewId, TenantId, TraceId, WorkspaceId,
};
use serde::{Deserialize, Serialize};

/// Discriminator for what entity a [`ReviewRequest`] is targeting.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReviewTarget {
    Execution { execution_id: ExecutionId },
    Handoff { handoff_id: HandoffId },
}

impl ReviewTarget {
    pub fn execution_id(&self) -> Option<&ExecutionId> {
        match self {
            Self::Execution { execution_id } => Some(execution_id),
            Self::Handoff { .. } => None,
        }
    }

    pub fn handoff_id(&self) -> Option<&HandoffId> {
        match self {
            Self::Handoff { handoff_id } => Some(handoff_id),
            Self::Execution { .. } => None,
        }
    }
}

pub fn default_target_execution() -> ReviewTarget {
    ReviewTarget::Execution {
        execution_id: ExecutionId::from_string("__legacy__".to_string())
            .expect("sentinel literal valid"),
    }
}

impl Default for ReviewTarget {
    fn default() -> Self {
        ReviewTarget::Execution {
            execution_id: ExecutionId::from_string("__legacy__".to_string())
                .expect("legacy sentinel valid"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewStatus {
    Pending,
    Approved,
    Rejected,
    Cancelled,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReviewKind {
    HumanReview,
    Approval,
    Escalation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewRequest {
    pub id: ReviewId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<ExecutionId>,
    pub case_id: Option<CaseId>,
    pub owner_node_id: Option<NodeId>,
    pub lease_id: Option<LeaseId>,
    pub title: String,
    pub summary: String,
    pub requested_by: ActorRef,
    pub status: ReviewStatus,
    pub review_kind: ReviewKind,
    pub trace_id: TraceId,
    /// 审核请求创建时间（UTC）
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Discriminated target of this review. Defaults to Execution variant for
    /// legacy records that pre-date this field.
    #[serde(default)]
    pub target: ReviewTarget,
}

impl ReviewRequest {
    /// Build a review whose target is an Execution (existing behavior).
    #[allow(clippy::too_many_arguments)]
    pub fn for_execution(
        id: ReviewId,
        execution_id: ExecutionId,
        case_id: Option<CaseId>,
        title: String,
        summary: String,
        requested_by: ActorRef,
        review_kind: ReviewKind,
        trace_id: TraceId,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> Self {
        Self {
            id,
            execution_id: Some(execution_id.clone()),
            case_id,
            owner_node_id: None,
            lease_id: None,
            title,
            summary,
            requested_by,
            status: ReviewStatus::Pending,
            review_kind,
            target: ReviewTarget::Execution { execution_id },
            trace_id,
            created_at,
        }
    }

    /// Build a review whose target is a Handoff (Sprint 22 NEW).
    /// `execution_id` is `None` — Handoff reviews are not tied to an Execution.
    #[allow(clippy::too_many_arguments)]
    pub fn for_handoff(
        id: ReviewId,
        handoff_id: HandoffId,
        case_id: Option<CaseId>,
        title: String,
        summary: String,
        requested_by: ActorRef,
        review_kind: ReviewKind,
        trace_id: TraceId,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> Self {
        Self {
            id,
            execution_id: None,
            case_id,
            owner_node_id: None,
            lease_id: None,
            title,
            summary,
            requested_by,
            status: ReviewStatus::Pending,
            review_kind,
            target: ReviewTarget::Handoff { handoff_id },
            trace_id,
            created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Decision {
    Allow,
    Review,
    ApprovalRequired,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub id: PolicyDecisionId,
    pub execution_id: ExecutionId,
    pub capability_id: CapabilityId,
    pub actor: ActorRef,
    pub decision: Decision,
    pub risk: RiskLevel,
    pub reasons: Vec<String>,
    pub approval_step_id: Option<ApprovalStepId>,
    pub trace_id: TraceId,
}

/// Types of review processes
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewType {
    /// Requires human review and approval
    HumanReview,
    /// Automatic review with configurable rules
    AutoReview,
    /// Peer review by another agent or service
    PeerReview,
}

/// Multi-dimensional review policy configuration
///
/// Supports tenant/workspace/actor isolation and capability pattern matching.
/// Dimensions are evaluated in AND logic (all matching conditions must pass).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewPolicy {
    /// Optional tenant-level policy isolation
    pub tenant_id: Option<TenantId>,

    /// Optional workspace-level policy isolation
    pub workspace_id: Option<WorkspaceId>,

    /// Optional actor type filtering
    pub actor_type: Option<ActorType>,

    /// Optional capability ID pattern (supports wildcards with glob-style matching)
    /// Examples: "github/*", "slack/send-message", "*"
    pub capability_pattern: Option<String>,

    /// Required review type when policy matches
    pub review_type: ReviewType,

    /// Optional minimum risk level threshold
    pub min_risk_level: Option<RiskLevel>,
}

impl ReviewPolicy {
    /// Evaluate if this policy requires review for the given context
    ///
    /// Returns Some(ReviewType) if review is required, None otherwise.
    ///
    /// # Arguments
    /// * `tenant_id` - Tenant context (None for system/global operations)
    /// * `workspace_id` - Workspace context (None for tenant-level operations)
    /// * `actor` - The actor requesting the action
    /// * `capability_id` - The capability being invoked
    /// * `risk_level` - Risk level of the capability
    pub fn evaluate(
        &self,
        tenant_id: Option<&TenantId>,
        workspace_id: Option<&WorkspaceId>,
        actor: &ActorRef,
        capability_id: &CapabilityId,
        risk_level: &RiskLevel,
    ) -> Option<ReviewType> {
        // Check tenant_id dimension
        if let Some(policy_tenant_id) = &self.tenant_id {
            match tenant_id {
                Some(ctx_tenant_id) if ctx_tenant_id == policy_tenant_id => {}
                _ => return None, // No match
            }
        }

        // Check workspace_id dimension
        if let Some(policy_workspace_id) = &self.workspace_id {
            match workspace_id {
                Some(ctx_workspace_id) if ctx_workspace_id == policy_workspace_id => {}
                _ => return None, // No match
            }
        }

        // Check actor_type dimension
        if let Some(policy_actor_type) = &self.actor_type {
            if !self.matches_actor_type(&actor.actor_type, policy_actor_type) {
                return None; // No match
            }
        }

        // Check capability_pattern dimension
        if let Some(pattern) = &self.capability_pattern {
            if !self.matches_capability(capability_id.as_str(), pattern) {
                return None; // No match
            }
        }

        // Check risk level threshold
        if let Some(min_risk) = &self.min_risk_level {
            if risk_level < min_risk {
                return None; // Risk too low, no review required
            }
        }

        // All conditions match, review required
        Some(self.review_type.clone())
    }

    /// Match actor type (exact match)
    fn matches_actor_type(&self, actor_type: &ActorType, policy_type: &ActorType) -> bool {
        actor_type == policy_type
    }

    /// Match capability ID against pattern (glob-style matching)
    ///
    /// Supports:
    /// - Exact match: "github/create-issue"
    /// - Wildcard suffix: "github/*" matches "github/create-issue"
    /// - Full wildcard: "*" matches everything
    fn matches_capability(&self, capability_id: &str, pattern: &str) -> bool {
        if pattern == "*" {
            return true; // Match all
        }

        if let Some(prefix) = pattern.strip_suffix("/*") {
            // Prefix match: "github/*" matches "github/create-issue"
            return capability_id.starts_with(prefix);
        }

        // Exact match
        capability_id == pattern
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ActorId;

    fn create_test_actor(actor_type: ActorType, tenant_id: Option<TenantId>) -> ActorRef {
        ActorRef {
            id: crate::ids::ActorId::new(),
            actor_type,
            tenant_id,
            home_node_id: None,
            display_name: "Test Actor".to_string(),
        }
    }

    #[test]
    fn test_review_type_equality() {
        assert_eq!(ReviewType::HumanReview, ReviewType::HumanReview);
        assert_eq!(ReviewType::AutoReview, ReviewType::AutoReview);
        assert_eq!(ReviewType::PeerReview, ReviewType::PeerReview);
        assert_ne!(ReviewType::HumanReview, ReviewType::AutoReview);
    }

    #[test]
    fn test_policy_no_conditions_always_matches() {
        let policy = ReviewPolicy {
            tenant_id: None,
            workspace_id: None,
            actor_type: None,
            capability_pattern: None,
            review_type: ReviewType::HumanReview,
            min_risk_level: None,
        };

        let actor = create_test_actor(ActorType::Agent, None);
        let capability_id = CapabilityId::from_string("github/create-issue".to_string()).unwrap();

        let result = policy.evaluate(None, None, &actor, &capability_id, &RiskLevel::Low);
        assert_eq!(result, Some(ReviewType::HumanReview));
    }

    #[test]
    fn test_policy_tenant_id_match() {
        let tenant_id = TenantId::from_string("tenant-123".to_string()).unwrap();
        let policy = ReviewPolicy {
            tenant_id: Some(tenant_id.clone()),
            workspace_id: None,
            actor_type: None,
            capability_pattern: None,
            review_type: ReviewType::AutoReview,
            min_risk_level: None,
        };

        let actor = create_test_actor(ActorType::Agent, Some(tenant_id.clone()));
        let capability_id = CapabilityId::from_string("slack/send-message".to_string()).unwrap();

        // Should match with correct tenant_id
        let result = policy.evaluate(
            Some(&tenant_id),
            None,
            &actor,
            &capability_id,
            &RiskLevel::Medium,
        );
        assert_eq!(result, Some(ReviewType::AutoReview));

        // Should NOT match with different tenant_id
        let other_tenant = TenantId::from_string("tenant-456".to_string()).unwrap();
        let result = policy.evaluate(
            Some(&other_tenant),
            None,
            &actor,
            &capability_id,
            &RiskLevel::Medium,
        );
        assert_eq!(result, None);

        // Should NOT match with None tenant_id
        let result = policy.evaluate(None, None, &actor, &capability_id, &RiskLevel::Medium);
        assert_eq!(result, None);
    }

    #[test]
    fn test_policy_workspace_id_match() {
        let workspace_id = WorkspaceId::from_string("workspace-abc".to_string()).unwrap();
        let policy = ReviewPolicy {
            tenant_id: None,
            workspace_id: Some(workspace_id.clone()),
            actor_type: None,
            capability_pattern: None,
            review_type: ReviewType::PeerReview,
            min_risk_level: None,
        };

        let actor = create_test_actor(ActorType::Agent, None);
        let capability_id = CapabilityId::from_string("jira/create-ticket".to_string()).unwrap();

        // Should match with correct workspace_id
        let result = policy.evaluate(
            None,
            Some(&workspace_id),
            &actor,
            &capability_id,
            &RiskLevel::High,
        );
        assert_eq!(result, Some(ReviewType::PeerReview));

        // Should NOT match with different workspace_id
        let other_workspace = WorkspaceId::from_string("workspace-xyz".to_string()).unwrap();
        let result = policy.evaluate(
            None,
            Some(&other_workspace),
            &actor,
            &capability_id,
            &RiskLevel::High,
        );
        assert_eq!(result, None);
    }

    #[test]
    fn test_policy_actor_type_match() {
        let policy = ReviewPolicy {
            tenant_id: None,
            workspace_id: None,
            actor_type: Some(ActorType::Agent),
            capability_pattern: None,
            review_type: ReviewType::HumanReview,
            min_risk_level: None,
        };

        let capability_id = CapabilityId::from_string("database/write".to_string()).unwrap();

        // Should match with Agent actor
        let agent = create_test_actor(ActorType::Agent, None);
        let result = policy.evaluate(None, None, &agent, &capability_id, &RiskLevel::Critical);
        assert_eq!(result, Some(ReviewType::HumanReview));

        // Should NOT match with Human actor
        let human = create_test_actor(ActorType::Human, None);
        let result = policy.evaluate(None, None, &human, &capability_id, &RiskLevel::Critical);
        assert_eq!(result, None);

        // Should NOT match with System actor
        let system = create_test_actor(ActorType::System, None);
        let result = policy.evaluate(None, None, &system, &capability_id, &RiskLevel::Critical);
        assert_eq!(result, None);
    }

    #[test]
    fn test_policy_capability_pattern_exact_match() {
        let policy = ReviewPolicy {
            tenant_id: None,
            workspace_id: None,
            actor_type: None,
            capability_pattern: Some("github/create-issue".to_string()),
            review_type: ReviewType::AutoReview,
            min_risk_level: None,
        };

        let actor = create_test_actor(ActorType::Agent, None);

        // Exact match should succeed
        let capability_id = CapabilityId::from_string("github/create-issue".to_string()).unwrap();
        let result = policy.evaluate(None, None, &actor, &capability_id, &RiskLevel::Low);
        assert_eq!(result, Some(ReviewType::AutoReview));

        // Different capability should NOT match
        let other_capability = CapabilityId::from_string("github/close-issue".to_string()).unwrap();
        let result = policy.evaluate(None, None, &actor, &other_capability, &RiskLevel::Low);
        assert_eq!(result, None);
    }

    #[test]
    fn test_policy_capability_pattern_wildcard() {
        let policy = ReviewPolicy {
            tenant_id: None,
            workspace_id: None,
            actor_type: None,
            capability_pattern: Some("github/*".to_string()),
            review_type: ReviewType::PeerReview,
            min_risk_level: None,
        };

        let actor = create_test_actor(ActorType::Agent, None);

        // Should match any github capability
        let cap1 = CapabilityId::from_string("github/create-issue".to_string()).unwrap();
        assert_eq!(
            policy.evaluate(None, None, &actor, &cap1, &RiskLevel::Medium),
            Some(ReviewType::PeerReview)
        );

        let cap2 = CapabilityId::from_string("github/create-pr".to_string()).unwrap();
        assert_eq!(
            policy.evaluate(None, None, &actor, &cap2, &RiskLevel::Medium),
            Some(ReviewType::PeerReview)
        );

        // Should NOT match non-github capabilities
        let slack_cap = CapabilityId::from_string("slack/send-message".to_string()).unwrap();
        assert_eq!(
            policy.evaluate(None, None, &actor, &slack_cap, &RiskLevel::Medium),
            None
        );
    }

    #[test]
    fn test_policy_capability_pattern_full_wildcard() {
        let policy = ReviewPolicy {
            tenant_id: None,
            workspace_id: None,
            actor_type: None,
            capability_pattern: Some("*".to_string()),
            review_type: ReviewType::HumanReview,
            min_risk_level: None,
        };

        let actor = create_test_actor(ActorType::Agent, None);

        // Should match all capabilities
        let cap1 = CapabilityId::from_string("github/create-issue".to_string()).unwrap();
        let cap2 = CapabilityId::from_string("slack/send-message".to_string()).unwrap();
        let cap3 = CapabilityId::from_string("database/write".to_string()).unwrap();

        assert_eq!(
            policy.evaluate(None, None, &actor, &cap1, &RiskLevel::Low),
            Some(ReviewType::HumanReview)
        );
        assert_eq!(
            policy.evaluate(None, None, &actor, &cap2, &RiskLevel::Medium),
            Some(ReviewType::HumanReview)
        );
        assert_eq!(
            policy.evaluate(None, None, &actor, &cap3, &RiskLevel::High),
            Some(ReviewType::HumanReview)
        );
    }

    #[test]
    fn test_policy_risk_level_threshold() {
        let policy = ReviewPolicy {
            tenant_id: None,
            workspace_id: None,
            actor_type: None,
            capability_pattern: None,
            review_type: ReviewType::HumanReview,
            min_risk_level: Some(RiskLevel::High),
        };

        let actor = create_test_actor(ActorType::Agent, None);
        let capability_id = CapabilityId::from_string("database/write".to_string()).unwrap();

        // Critical should match (>= High)
        let result = policy.evaluate(None, None, &actor, &capability_id, &RiskLevel::Critical);
        assert_eq!(result, Some(ReviewType::HumanReview));

        // High should match (== High)
        let result = policy.evaluate(None, None, &actor, &capability_id, &RiskLevel::High);
        assert_eq!(result, Some(ReviewType::HumanReview));

        // Medium should NOT match (< High)
        let result = policy.evaluate(None, None, &actor, &capability_id, &RiskLevel::Medium);
        assert_eq!(result, None);

        // Low should NOT match (< High)
        let result = policy.evaluate(None, None, &actor, &capability_id, &RiskLevel::Low);
        assert_eq!(result, None);
    }

    #[test]
    fn test_policy_multi_dimension_all_match() {
        // Complex policy with multiple dimensions
        let tenant_id = TenantId::from_string("tenant-prod".to_string()).unwrap();
        let workspace_id = WorkspaceId::from_string("workspace-main".to_string()).unwrap();

        let policy = ReviewPolicy {
            tenant_id: Some(tenant_id.clone()),
            workspace_id: Some(workspace_id.clone()),
            actor_type: Some(ActorType::Agent),
            capability_pattern: Some("database/*".to_string()),
            review_type: ReviewType::HumanReview,
            min_risk_level: Some(RiskLevel::High),
        };

        let actor = create_test_actor(ActorType::Agent, Some(tenant_id.clone()));
        let capability_id = CapabilityId::from_string("database/delete".to_string()).unwrap();

        // All dimensions match
        let result = policy.evaluate(
            Some(&tenant_id),
            Some(&workspace_id),
            &actor,
            &capability_id,
            &RiskLevel::Critical,
        );
        assert_eq!(result, Some(ReviewType::HumanReview));
    }

    #[test]
    fn test_policy_multi_dimension_partial_match_fails() {
        let tenant_id = TenantId::from_string("tenant-prod".to_string()).unwrap();
        let workspace_id = WorkspaceId::from_string("workspace-main".to_string()).unwrap();

        let policy = ReviewPolicy {
            tenant_id: Some(tenant_id.clone()),
            workspace_id: Some(workspace_id.clone()),
            actor_type: Some(ActorType::Agent),
            capability_pattern: Some("database/*".to_string()),
            review_type: ReviewType::HumanReview,
            min_risk_level: Some(RiskLevel::High),
        };

        let actor = create_test_actor(ActorType::Agent, Some(tenant_id.clone()));
        let capability_id = CapabilityId::from_string("database/delete".to_string()).unwrap();

        // Wrong workspace_id - should fail
        let other_workspace = WorkspaceId::from_string("workspace-dev".to_string()).unwrap();
        let result = policy.evaluate(
            Some(&tenant_id),
            Some(&other_workspace),
            &actor,
            &capability_id,
            &RiskLevel::Critical,
        );
        assert_eq!(result, None);

        // Wrong actor_type - should fail
        let human = create_test_actor(ActorType::Human, Some(tenant_id.clone()));
        let result = policy.evaluate(
            Some(&tenant_id),
            Some(&workspace_id),
            &human,
            &capability_id,
            &RiskLevel::Critical,
        );
        assert_eq!(result, None);

        // Wrong capability pattern - should fail
        let slack_cap = CapabilityId::from_string("slack/send-message".to_string()).unwrap();
        let result = policy.evaluate(
            Some(&tenant_id),
            Some(&workspace_id),
            &actor,
            &slack_cap,
            &RiskLevel::Critical,
        );
        assert_eq!(result, None);

        // Risk level too low - should fail
        let result = policy.evaluate(
            Some(&tenant_id),
            Some(&workspace_id),
            &actor,
            &capability_id,
            &RiskLevel::Medium,
        );
        assert_eq!(result, None);
    }

    #[test]
    fn review_target_execution_serde_roundtrip() {
        let t = ReviewTarget::Execution {
            execution_id: ExecutionId::from_string("exec_1".to_string()).unwrap(),
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: ReviewTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
        assert!(json.contains("\"execution\""));
    }

    #[test]
    fn review_target_handoff_serde_roundtrip() {
        let t = ReviewTarget::Handoff {
            handoff_id: HandoffId::from_string("ho_1".to_string()).unwrap(),
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: ReviewTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
        assert!(json.contains("\"handoff\""));
    }

    #[test]
    fn review_request_for_handoff_constructor_sets_none_and_target() {
        let req = ReviewRequest::for_handoff(
            ReviewId::from_string("rev_1".to_string()).unwrap(),
            HandoffId::from_string("ho_42".to_string()).unwrap(),
            None,
            "Test".into(),
            "Summary".into(),
            ActorRef {
                id: ActorId::new(),
                actor_type: ActorType::Agent,
                tenant_id: None,
                home_node_id: None,
                display_name: "test-agent".to_string(),
            },
            ReviewKind::HumanReview,
            TraceId::from_string("trace_1".to_string()).unwrap(),
            chrono::Utc::now(),
        );
        assert_eq!(req.execution_id, None);
        assert!(matches!(req.target, ReviewTarget::Handoff { .. }));
        assert_eq!(req.target.handoff_id().unwrap().as_str(), "ho_42");
    }

    #[test]
    fn review_request_for_execution_sets_some_execution_id() {
        let exec_id = ExecutionId::from_string("exec_42".to_string()).unwrap();
        let req = ReviewRequest::for_execution(
            ReviewId::from_string("rev_2".to_string()).unwrap(),
            exec_id.clone(),
            None,
            "Test exec review".into(),
            "Summary".into(),
            ActorRef {
                id: ActorId::new(),
                actor_type: ActorType::Agent,
                tenant_id: None,
                home_node_id: None,
                display_name: "test-agent".to_string(),
            },
            ReviewKind::HumanReview,
            TraceId::from_string("trace_2".to_string()).unwrap(),
            chrono::Utc::now(),
        );
        assert_eq!(req.execution_id, Some(exec_id.clone()));
        assert_eq!(req.target.execution_id(), Some(&exec_id));
    }

    #[test]
    fn review_request_deserialize_legacy_no_target_field() {
        // JSON WITHOUT target field — must default to Execution variant via
        // Default impl for ReviewTarget.
        let json = serde_json::json!({
            "id": "rev_legacy",
            "execution_id": "exec_old",
            "case_id": null,
            "owner_node_id": null,
            "lease_id": null,
            "title": "old review",
            "summary": "legacy record",
            "requested_by": {
                "id": "actor_1",
                "actor_type": "Agent",
                "tenant_id": null,
                "home_node_id": null,
                "display_name": "legacy-agent"
            },
            "status": "Pending",
            "review_kind": "HumanReview",
            "trace_id": "trace_old",
            "created_at": "2026-01-01T00:00:00Z"
        })
        .to_string();
        let req: ReviewRequest = serde_json::from_str(&json).expect("legacy deserializes");
        assert!(matches!(req.target, ReviewTarget::Execution { .. }));
    }
}
