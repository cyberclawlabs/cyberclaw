//! Capability policy configuration system.

use crate::decision::{GovernanceDecision, ReviewType};
use cyberclaw_core::capability::{CapabilityEffect, CapabilityRef, RiskLevel};
use cyberclaw_core::identity::ActorRef;
use cyberclaw_core::ids::CapabilityId;
use serde::{Deserialize, Serialize};

/// Glob-style pattern for matching capability IDs.
///
/// Supports:
/// - `"fs.*"` matches `"fs.read"`, `"fs.write"` (single segment wildcard)
/// - `"*.read"` matches `"fs.read"`, `"db.read"`
/// - `"**"` matches everything
/// - `"fs.read"` exact match
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityPattern(String);

impl CapabilityPattern {
    /// Create a new capability pattern.
    pub fn new(pattern: &str) -> Self {
        Self(pattern.to_string())
    }

    /// Check if the pattern matches a capability ID string.
    pub fn matches(&self, capability_id: &str) -> bool {
        glob_match(&self.0, capability_id)
    }

    /// Get the pattern string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Simple glob matching without external dependencies.
///
/// Supports `*` (matches any chars except `.`) and `**` (matches everything).
fn glob_match(pattern: &str, input: &str) -> bool {
    if pattern == "**" {
        return true;
    }

    let pat_parts: Vec<&str> = pattern.split('.').collect();
    let inp_parts: Vec<&str> = input.split('.').collect();

    if pat_parts.len() != inp_parts.len() {
        return false;
    }

    pat_parts
        .iter()
        .zip(inp_parts.iter())
        .all(|(p, i)| *p == "*" || *p == *i)
}

/// Policy source layer indicating where a rule originates.
///
/// Higher-layer sources override lower-layer sources when rules conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicySource {
    /// Platform built-in default rules (lowest priority)
    SystemDefault = 0,
    /// Tenant-level policies
    TenantPolicy = 1,
    /// Project-level policies
    ProjectPolicy = 2,
    /// Session-level overrides (highest priority)
    SessionOverride = 3,
}

/// Trust level for skills, affecting policy enforcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillTrustLevel {
    /// Unverified skill (most restrictive)
    Installed,
    /// Platform-reviewed skill
    Verified,
    /// Organization-trusted skill
    Trusted,
}

impl SkillTrustLevel {
    /// Compute the effective action based on trust level and base policy action.
    ///
    /// - `Installed` + `RequireApproval` -> `Deny` (unverified skills cannot get approval)
    /// - All other combinations preserve the base action.
    pub fn effective_action(&self, base_action: &PolicyAction) -> PolicyAction {
        match (self, base_action) {
            (SkillTrustLevel::Installed, PolicyAction::RequireApproval) => PolicyAction::Deny,
            _ => base_action.clone(),
        }
    }
}

/// Policy action to take for matching capabilities
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAction {
    /// Allow the capability
    Allow,
    /// Deny the capability
    Deny,
    /// Require human review
    RequireHumanReview,
    /// Require approval
    RequireApproval,
    /// Require security review
    RequireSecurityReview,
    /// Escalate to higher authority
    RequireEscalation,
}

impl PolicyAction {
    /// Convert PolicyAction to GovernanceDecision with the given reason
    pub fn to_decision(&self, reason: impl Into<String>) -> GovernanceDecision {
        let reason = reason.into();
        match self {
            PolicyAction::Allow => GovernanceDecision::allow(reason),
            PolicyAction::Deny => GovernanceDecision::deny(reason),
            PolicyAction::RequireHumanReview => {
                GovernanceDecision::review_required(reason, ReviewType::Human)
            }
            PolicyAction::RequireApproval => {
                GovernanceDecision::review_required(reason, ReviewType::Approval)
            }
            PolicyAction::RequireSecurityReview => {
                GovernanceDecision::review_required(reason, ReviewType::Security)
            }
            PolicyAction::RequireEscalation => {
                GovernanceDecision::review_required(reason, ReviewType::Escalation)
            }
        }
    }
}

fn default_policy_source() -> PolicySource {
    PolicySource::SystemDefault
}

/// Policy rule for matching capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    /// Rule name for debugging
    pub name: String,
    /// Priority (higher priority rules are evaluated first)
    pub priority: u32,
    /// Match specific capability ID (if None, matches all).
    /// When both `capability_id` and `pattern` are set, `pattern` takes precedence.
    pub capability_id: Option<CapabilityId>,
    /// Glob pattern for matching capability IDs (e.g. "fs.*", "*.read", "**").
    /// Takes precedence over `capability_id` when set.
    #[serde(default)]
    pub pattern: Option<CapabilityPattern>,
    /// Match specific risk level (if None, matches all)
    pub risk_level: Option<RiskLevel>,
    /// Match specific effect (if None, matches all)
    pub effect: Option<CapabilityEffect>,
    /// Match specific actor (if None, matches all)
    pub actor: Option<ActorRef>,
    /// Match specific tenant (if None, applies to all tenants or system-wide)
    pub tenant_id: Option<cyberclaw_core::ids::TenantId>,
    /// Source layer for this rule (default: SystemDefault)
    #[serde(default = "default_policy_source")]
    pub source: PolicySource,
    /// Action to take when rule matches
    pub action: PolicyAction,
    /// Reason for the policy
    pub reason: String,
}

impl PolicyRule {
    /// Check if this rule matches the given capability and actor
    pub fn matches(&self, capability: &CapabilityRef, actor: &ActorRef) -> bool {
        // Check tenant_id match (tenant isolation)
        // If rule has tenant_id, actor must have matching tenant_id
        // If rule has no tenant_id, it's a system-wide policy
        if let Some(ref rule_tenant_id) = self.tenant_id {
            match &actor.tenant_id {
                Some(actor_tenant_id) if actor_tenant_id == rule_tenant_id => {
                    // Tenant matches, continue checking
                }
                _ => {
                    // Tenant doesn't match or actor has no tenant
                    return false;
                }
            }
        }

        // Check capability match: pattern takes precedence over capability_id
        if let Some(ref pat) = self.pattern {
            if !pat.matches(capability.id.as_str()) {
                return false;
            }
        } else if let Some(ref id) = self.capability_id {
            if id != &capability.id {
                return false;
            }
        }

        // Check risk_level match
        if let Some(ref risk) = self.risk_level {
            if risk != &capability.risk {
                return false;
            }
        }

        // Check effect match
        if let Some(ref effect) = self.effect {
            let matches = capability.effects.iter().any(|e| match (e, effect) {
                (CapabilityEffect::Read, CapabilityEffect::Read) => true,
                (CapabilityEffect::Write, CapabilityEffect::Write) => true,
                (CapabilityEffect::Execute, CapabilityEffect::Execute) => true,
                (CapabilityEffect::Network, CapabilityEffect::Network) => true,
                (CapabilityEffect::Ticket, CapabilityEffect::Ticket) => true,
                (CapabilityEffect::Notification, CapabilityEffect::Notification) => true,
                (CapabilityEffect::Custom(a), CapabilityEffect::Custom(b)) => a == b,
                _ => false,
            });
            if !matches {
                return false;
            }
        }

        // Check actor match
        if let Some(ref policy_actor) = self.actor {
            if policy_actor.id != actor.id {
                return false;
            }
        }

        true
    }

    /// Create a decision for this rule
    pub fn to_decision(&self) -> GovernanceDecision {
        self.action.to_decision(&self.reason)
    }
}

/// Capability policy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityPolicy {
    /// Policy rules sorted by priority (highest first)
    rules: Vec<PolicyRule>,
    /// Default action when no rule matches
    default_action: PolicyAction,
    /// Default reason when using default action
    default_reason: String,
}

impl CapabilityPolicy {
    /// Create a new policy with the given rules and default action
    pub fn new(
        mut rules: Vec<PolicyRule>,
        default_action: PolicyAction,
        default_reason: impl Into<String>,
    ) -> Self {
        // Sort rules by priority (highest first)
        rules.sort_by_key(|b| std::cmp::Reverse(b.priority));
        Self {
            rules,
            default_action,
            default_reason: default_reason.into(),
        }
    }

    /// Create a default policy that allows low risk and requires review for higher risk
    pub fn default_policy() -> Self {
        let rules = vec![
            PolicyRule {
                name: "deny-critical-without-approval".to_string(),
                priority: 100,
                capability_id: None,
                pattern: None,
                risk_level: Some(RiskLevel::Critical),
                effect: None,
                actor: None,
                tenant_id: None,
                source: PolicySource::SystemDefault,
                action: PolicyAction::RequireApproval,
                reason: "Critical risk capabilities require approval".to_string(),
            },
            PolicyRule {
                name: "review-high-risk".to_string(),
                priority: 90,
                capability_id: None,
                pattern: None,
                risk_level: Some(RiskLevel::High),
                effect: None,
                actor: None,
                tenant_id: None,
                source: PolicySource::SystemDefault,
                action: PolicyAction::RequireHumanReview,
                reason: "High risk capabilities require human review".to_string(),
            },
            PolicyRule {
                name: "allow-low-medium".to_string(),
                priority: 10,
                capability_id: None,
                pattern: None,
                risk_level: Some(RiskLevel::Low),
                effect: None,
                actor: None,
                tenant_id: None,
                source: PolicySource::SystemDefault,
                action: PolicyAction::Allow,
                reason: "Low risk capabilities are allowed".to_string(),
            },
            PolicyRule {
                name: "allow-medium".to_string(),
                priority: 20,
                capability_id: None,
                pattern: None,
                risk_level: Some(RiskLevel::Medium),
                effect: None,
                actor: None,
                tenant_id: None,
                source: PolicySource::SystemDefault,
                action: PolicyAction::Allow,
                reason: "Medium risk capabilities are allowed".to_string(),
            },
        ];

        Self::new(
            rules,
            PolicyAction::RequireHumanReview,
            "No matching rule, default to human review",
        )
    }

    /// Evaluate the policy for a capability and actor
    pub fn evaluate(&self, capability: &CapabilityRef, actor: &ActorRef) -> GovernanceDecision {
        // Find the first matching rule
        for rule in &self.rules {
            if rule.matches(capability, actor) {
                return rule.to_decision();
            }
        }

        // No rule matched, use default action
        self.default_action.to_decision(&self.default_reason)
    }

    /// Add a new rule to the policy
    pub fn add_rule(&mut self, rule: PolicyRule) {
        self.rules.push(rule);
        // Re-sort by priority
        self.rules.sort_by_key(|b| std::cmp::Reverse(b.priority));
    }

    /// Get all rules
    pub fn rules(&self) -> &[PolicyRule] {
        &self.rules
    }
}

impl Default for CapabilityPolicy {
    fn default() -> Self {
        Self::default_policy()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::identity::ActorType;
    use cyberclaw_core::ids::{ActorId, ConnectorId};

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
            placement: None,
        }
    }

    fn create_test_actor(id: &str) -> ActorRef {
        ActorRef {
            id: ActorId::from_string(id.to_string()).unwrap(),
            actor_type: ActorType::Agent,
            tenant_id: None,
            home_node_id: None,
            display_name: id.to_string(),
        }
    }

    fn make_rule(name: &str, action: PolicyAction) -> PolicyRule {
        PolicyRule {
            name: name.to_string(),
            priority: 10,
            capability_id: None,
            pattern: None,
            risk_level: None,
            effect: None,
            actor: None,
            tenant_id: None,
            source: PolicySource::SystemDefault,
            action,
            reason: name.to_string(),
        }
    }

    #[test]
    fn test_policy_action_to_decision() {
        let action = PolicyAction::Allow;
        let decision = action.to_decision("test reason");
        assert!(decision.is_allow());
        assert_eq!(decision.reason(), "test reason");
    }

    #[test]
    fn test_policy_rule_matches_capability_id() {
        let mut rule = make_rule("test", PolicyAction::Allow);
        rule.capability_id = Some(CapabilityId::from_string("test-cap".to_string()).unwrap());

        let matching_cap = create_test_capability("test-cap", RiskLevel::Low, vec![]);
        let non_matching_cap = create_test_capability("other-cap", RiskLevel::Low, vec![]);
        let actor = create_test_actor("test-agent");

        assert!(rule.matches(&matching_cap, &actor));
        assert!(!rule.matches(&non_matching_cap, &actor));
    }

    #[test]
    fn test_policy_rule_matches_risk_level() {
        let mut rule = make_rule("test", PolicyAction::Deny);
        rule.risk_level = Some(RiskLevel::High);

        let high_risk_cap = create_test_capability("cap1", RiskLevel::High, vec![]);
        let low_risk_cap = create_test_capability("cap2", RiskLevel::Low, vec![]);
        let actor = create_test_actor("test-agent");

        assert!(rule.matches(&high_risk_cap, &actor));
        assert!(!rule.matches(&low_risk_cap, &actor));
    }

    #[test]
    fn test_policy_rule_matches_effect() {
        let mut rule = make_rule("test", PolicyAction::RequireApproval);
        rule.effect = Some(CapabilityEffect::Write);

        let write_cap = create_test_capability(
            "cap1",
            RiskLevel::Low,
            vec![CapabilityEffect::Read, CapabilityEffect::Write],
        );
        let read_cap = create_test_capability("cap2", RiskLevel::Low, vec![CapabilityEffect::Read]);
        let actor = create_test_actor("test-agent");

        assert!(rule.matches(&write_cap, &actor));
        assert!(!rule.matches(&read_cap, &actor));
    }

    #[test]
    fn test_default_policy_low_risk() {
        let policy = CapabilityPolicy::default();
        let capability = create_test_capability("test", RiskLevel::Low, vec![]);
        let actor = create_test_actor("agent");

        let decision = policy.evaluate(&capability, &actor);
        assert!(decision.is_allow());
    }

    #[test]
    fn test_default_policy_high_risk() {
        let policy = CapabilityPolicy::default();
        let capability = create_test_capability("test", RiskLevel::High, vec![]);
        let actor = create_test_actor("agent");

        let decision = policy.evaluate(&capability, &actor);
        assert!(decision.is_review_required());
        assert_eq!(decision.review_type(), Some(&ReviewType::Human));
    }

    #[test]
    fn test_default_policy_critical_risk() {
        let policy = CapabilityPolicy::default();
        let capability = create_test_capability("test", RiskLevel::Critical, vec![]);
        let actor = create_test_actor("agent");

        let decision = policy.evaluate(&capability, &actor);
        assert!(decision.is_review_required());
        assert_eq!(decision.review_type(), Some(&ReviewType::Approval));
    }

    #[test]
    fn test_policy_priority_ordering() {
        let mut policy = CapabilityPolicy::new(vec![], PolicyAction::Deny, "default deny");

        let mut low = make_rule("low-priority", PolicyAction::Allow);
        low.priority = 10;
        low.risk_level = Some(RiskLevel::High);
        low.reason = "low priority allows".to_string();
        policy.add_rule(low);

        let mut high = make_rule("high-priority", PolicyAction::Deny);
        high.priority = 100;
        high.risk_level = Some(RiskLevel::High);
        high.reason = "high priority denies".to_string();
        policy.add_rule(high);

        let capability = create_test_capability("test", RiskLevel::High, vec![]);
        let actor = create_test_actor("agent");

        let decision = policy.evaluate(&capability, &actor);
        assert!(decision.is_deny());
        assert_eq!(decision.reason(), "high priority denies");
    }

    #[test]
    fn test_policy_no_match_uses_default() {
        let mut rule = make_rule("specific-cap", PolicyAction::Allow);
        rule.capability_id = Some(CapabilityId::from_string("specific".to_string()).unwrap());
        rule.reason = "specific cap allowed".to_string();

        let policy = CapabilityPolicy::new(vec![rule], PolicyAction::Deny, "default deny all");

        let capability = create_test_capability("other", RiskLevel::Low, vec![]);
        let actor = create_test_actor("agent");

        let decision = policy.evaluate(&capability, &actor);
        assert!(decision.is_deny());
        assert_eq!(decision.reason(), "default deny all");
    }

    // --- CapabilityPattern glob tests ---

    #[test]
    fn test_glob_prefix_wildcard() {
        let pat = CapabilityPattern::new("fs.*");
        assert!(pat.matches("fs.read"));
        assert!(pat.matches("fs.write"));
        assert!(!pat.matches("db.read"));
        assert!(!pat.matches("fs.sub.deep"));
    }

    #[test]
    fn test_glob_suffix_wildcard() {
        let pat = CapabilityPattern::new("*.read");
        assert!(pat.matches("fs.read"));
        assert!(pat.matches("db.read"));
        assert!(!pat.matches("fs.write"));
    }

    #[test]
    fn test_glob_double_star_matches_everything() {
        let pat = CapabilityPattern::new("**");
        assert!(pat.matches("fs.read"));
        assert!(pat.matches("db.write"));
        assert!(pat.matches("anything"));
        assert!(pat.matches("a.b.c.d"));
    }

    #[test]
    fn test_glob_exact_match() {
        let pat = CapabilityPattern::new("fs.read");
        assert!(pat.matches("fs.read"));
        assert!(!pat.matches("fs.write"));
        assert!(!pat.matches("db.read"));
    }

    #[test]
    fn test_glob_multi_segment() {
        let pat = CapabilityPattern::new("fs.*.local");
        assert!(pat.matches("fs.read.local"));
        assert!(pat.matches("fs.write.local"));
        assert!(!pat.matches("fs.read.remote"));
        assert!(!pat.matches("fs.read"));
    }

    #[test]
    fn test_pattern_in_policy_rule() {
        let mut rule = make_rule("fs-all", PolicyAction::Deny);
        rule.pattern = Some(CapabilityPattern::new("fs.*"));

        let fs_read = create_test_capability("fs.read", RiskLevel::Low, vec![]);
        let db_read = create_test_capability("db.read", RiskLevel::Low, vec![]);
        let actor = create_test_actor("agent");

        assert!(rule.matches(&fs_read, &actor));
        assert!(!rule.matches(&db_read, &actor));
    }

    #[test]
    fn test_pattern_takes_precedence_over_capability_id() {
        let mut rule = make_rule("precedence", PolicyAction::Allow);
        rule.capability_id = Some(CapabilityId::from_string("fs.read".to_string()).unwrap());
        rule.pattern = Some(CapabilityPattern::new("db.*"));

        let fs_read = create_test_capability("fs.read", RiskLevel::Low, vec![]);
        let db_write = create_test_capability("db.write", RiskLevel::Low, vec![]);
        let actor = create_test_actor("agent");

        // Pattern "db.*" should be used, not capability_id "fs.read"
        assert!(!rule.matches(&fs_read, &actor));
        assert!(rule.matches(&db_write, &actor));
    }

    // --- PolicySource priority tests ---

    #[test]
    fn test_policy_source_ordering() {
        assert!(PolicySource::SessionOverride > PolicySource::ProjectPolicy);
        assert!(PolicySource::ProjectPolicy > PolicySource::TenantPolicy);
        assert!(PolicySource::TenantPolicy > PolicySource::SystemDefault);
    }

    #[test]
    fn test_policy_source_session_overrides_system() {
        let mut system_rule = make_rule("system-deny", PolicyAction::Deny);
        system_rule.source = PolicySource::SystemDefault;
        system_rule.priority = 100;

        let mut session_rule = make_rule("session-allow", PolicyAction::Allow);
        session_rule.source = PolicySource::SessionOverride;
        session_rule.priority = 1;

        // Sort by source descending, then priority descending
        let mut rules = [system_rule, session_rule];
        rules.sort_by(|a, b| {
            b.source
                .cmp(&a.source)
                .then_with(|| b.priority.cmp(&a.priority))
        });

        assert_eq!(rules[0].name, "session-allow");
        assert_eq!(rules[1].name, "system-deny");
    }

    // --- SkillTrustLevel tests ---

    #[test]
    fn test_installed_require_approval_becomes_deny() {
        let action = SkillTrustLevel::Installed.effective_action(&PolicyAction::RequireApproval);
        assert_eq!(action, PolicyAction::Deny);
    }

    #[test]
    fn test_installed_allow_stays_allow() {
        let action = SkillTrustLevel::Installed.effective_action(&PolicyAction::Allow);
        assert_eq!(action, PolicyAction::Allow);
    }

    #[test]
    fn test_verified_require_approval_stays() {
        let action = SkillTrustLevel::Verified.effective_action(&PolicyAction::RequireApproval);
        assert_eq!(action, PolicyAction::RequireApproval);
    }

    #[test]
    fn test_trusted_require_approval_stays() {
        let action = SkillTrustLevel::Trusted.effective_action(&PolicyAction::RequireApproval);
        assert_eq!(action, PolicyAction::RequireApproval);
    }

    #[test]
    fn test_trusted_allow_stays_allow() {
        let action = SkillTrustLevel::Trusted.effective_action(&PolicyAction::Allow);
        assert_eq!(action, PolicyAction::Allow);
    }

    #[test]
    fn test_installed_deny_stays_deny() {
        let action = SkillTrustLevel::Installed.effective_action(&PolicyAction::Deny);
        assert_eq!(action, PolicyAction::Deny);
    }
}
