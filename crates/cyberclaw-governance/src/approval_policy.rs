//! Approval policy system for governance workflows.
//!
//! This module provides approval-based governance policies that require
//! explicit approval from authorized approvers before capabilities can execute.

use async_trait::async_trait;
use std::sync::Arc;

use cyberclaw_core::prelude::*;
use serde::{Deserialize, Serialize};

use crate::decision::{GovernanceDecision, ReviewType};
use crate::engine::{EvaluationContext, EvaluationResult, PolicyEngine};

/// Approval requirement for a capability
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalRequirement {
    /// No approval required
    None,

    /// Single approver required
    SingleApprover {
        /// Required approver role
        role: String,
    },

    /// Multiple approvers required
    MultipleApprovers {
        /// Required approver roles
        roles: Vec<String>,
        /// Minimum number of approvals needed
        min_approvals: usize,
    },

    /// Chain of command approval
    ChainOfCommand {
        /// Ordered list of roles that must approve
        role_chain: Vec<String>,
    },
}

/// Approval rule matching criteria
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRule {
    /// Rule name for debugging
    pub name: String,

    /// Priority (higher priority rules are evaluated first)
    pub priority: u32,

    /// Match specific capability ID pattern (glob syntax)
    pub capability_pattern: Option<String>,

    /// Match specific risk level (if None, matches all)
    pub risk_level: Option<RiskLevel>,

    /// Match specific effect (if None, matches all)
    pub effect: Option<CapabilityEffect>,

    /// Approval requirement for matching capabilities
    pub requirement: ApprovalRequirement,

    /// Reason for the approval requirement
    pub reason: String,

    /// Optional Iron Law behavioral reinforcement for this rule.
    /// When present, `generate_prompt_rules()` includes the Iron Law declaration,
    /// rationalization table, and red flags in the prompt output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iron_law: Option<IronLaw>,
}

impl ApprovalRule {
    /// Check if this rule matches the given capability
    pub fn matches(&self, capability: &CapabilityRef) -> bool {
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

        // Check capability_pattern match (simple glob matching)
        if let Some(ref pattern) = self.capability_pattern {
            if !self.matches_glob(pattern, capability.id.as_ref()) {
                return false;
            }
        }

        true
    }

    /// Simple glob pattern matching (* matches any sequence)
    fn matches_glob(&self, pattern: &str, text: &str) -> bool {
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

    /// Create a decision for this rule's requirement
    pub fn to_decision(&self) -> GovernanceDecision {
        match &self.requirement {
            ApprovalRequirement::None => GovernanceDecision::allow(&self.reason),
            ApprovalRequirement::SingleApprover { role } => GovernanceDecision::review_required(
                format!("{}: requires approval from {}", self.reason, role),
                ReviewType::Approval,
            ),
            ApprovalRequirement::MultipleApprovers {
                roles,
                min_approvals,
            } => GovernanceDecision::review_required(
                format!(
                    "{}: requires {} approvals from roles: {}",
                    self.reason,
                    min_approvals,
                    roles.join(", ")
                ),
                ReviewType::Approval,
            ),
            ApprovalRequirement::ChainOfCommand { role_chain } => {
                GovernanceDecision::review_required(
                    format!(
                        "{}: requires chain-of-command approval: {}",
                        self.reason,
                        role_chain.join(" -> ")
                    ),
                    ReviewType::Escalation,
                )
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Iron Law + Anti-Rationalization (Behavioral Reinforcement)
// ---------------------------------------------------------------------------

/// A single entry in the rationalization prevention table.
///
/// Maps a common excuse to its factual reality, based on the superpowers
/// Anti-Rationalization pattern (Cialdini, 2021; Meincke et al., 2025).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RationalizationEntry {
    /// The excuse an agent might generate.
    pub excuse: String,
    /// The factual reality that counters the excuse.
    pub reality: String,
}

/// Iron Law declaration for a governance rule.
///
/// Three-layer behavioral defense:
/// 1. **Iron Law** — an absolute, non-negotiable rule statement
/// 2. **Rationalization Table** — excuse↔reality pairs that block common evasions
/// 3. **Red Flags** — self-check signals that indicate the agent is about to violate
///
/// Based on superpowers' verification-before-completion pattern, which improved
/// LLM rule compliance from ~33% to ~72% in empirical testing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IronLaw {
    /// The absolute rule declaration (e.g., "NO DESTRUCTIVE OPERATIONS WITHOUT APPROVAL").
    pub declaration: String,
    /// Excuse↔reality pairs that counter common rationalizations.
    pub rationalization_table: Vec<RationalizationEntry>,
    /// Self-check signals indicating imminent violation.
    pub red_flags: Vec<String>,
}

impl IronLaw {
    /// Render this Iron Law as a prompt-injectable text block.
    pub fn render(&self) -> String {
        let mut parts = Vec::new();

        // Iron Law declaration (Authority principle — imperative, non-negotiable)
        parts.push(format!("IRON LAW: {}", self.declaration));
        parts.push(
            "Violating the letter of this rule is violating the spirit of this rule.".to_string(),
        );

        // Rationalization table
        if !self.rationalization_table.is_empty() {
            parts.push(String::new());
            parts.push("Rationalization Prevention:".to_string());
            parts.push("| Excuse | Reality |".to_string());
            parts.push("|--------|---------|".to_string());
            for entry in &self.rationalization_table {
                parts.push(format!("| \"{}\" | {} |", entry.excuse, entry.reality));
            }
        }

        // Red flags
        if !self.red_flags.is_empty() {
            parts.push(String::new());
            parts.push("Red Flags — STOP if you notice:".to_string());
            for flag in &self.red_flags {
                parts.push(format!("- {}", flag));
            }
        }

        parts.join("\n")
    }
}

/// Approval policy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalPolicy {
    /// Approval rules sorted by priority (highest first)
    rules: Vec<ApprovalRule>,

    /// Default requirement when no rule matches
    default_requirement: ApprovalRequirement,

    /// Default reason when using default requirement
    default_reason: String,
}

impl ApprovalPolicy {
    /// Create a new approval policy with the given rules and default
    pub fn new(
        mut rules: Vec<ApprovalRule>,
        default_requirement: ApprovalRequirement,
        default_reason: impl Into<String>,
    ) -> Self {
        // Sort rules by priority (highest first)
        rules.sort_by_key(|b| std::cmp::Reverse(b.priority));
        Self {
            rules,
            default_requirement,
            default_reason: default_reason.into(),
        }
    }

    /// Create a default approval policy
    ///
    /// - Critical risk: Chain of command approval
    /// - High risk: Single approver (security-admin)
    /// - Medium/Low: No approval required
    pub fn default_policy() -> Self {
        let rules = vec![
            ApprovalRule {
                name: "critical-chain-of-command".to_string(),
                priority: 100,
                capability_pattern: None,
                risk_level: Some(RiskLevel::Critical),
                effect: None,
                requirement: ApprovalRequirement::ChainOfCommand {
                    role_chain: vec!["security-admin".to_string(), "platform-admin".to_string()],
                },
                reason: "Critical risk capabilities require chain-of-command approval".to_string(),
                iron_law: Some(IronLaw {
                    declaration: "NO CRITICAL CAPABILITY EXECUTION WITHOUT CHAIN-OF-COMMAND APPROVAL".to_string(),
                    rationalization_table: vec![
                        RationalizationEntry {
                            excuse: "It's urgent, I'll get approval after".to_string(),
                            reality: "Urgency does not override safety. Request expedited approval.".to_string(),
                        },
                        RationalizationEntry {
                            excuse: "The previous similar action was approved".to_string(),
                            reality: "Each critical action requires independent approval. No precedent exemptions.".to_string(),
                        },
                        RationalizationEntry {
                            excuse: "I'm confident this is safe".to_string(),
                            reality: "Confidence is not authorization. The approval chain exists for oversight.".to_string(),
                        },
                    ],
                    red_flags: vec![
                        "About to execute a critical capability without approval evidence".to_string(),
                        "Reasoning about why this particular case is an exception".to_string(),
                        "Planning to request forgiveness rather than permission".to_string(),
                    ],
                }),
            },
            ApprovalRule {
                name: "high-risk-approval".to_string(),
                priority: 90,
                capability_pattern: None,
                risk_level: Some(RiskLevel::High),
                effect: None,
                requirement: ApprovalRequirement::SingleApprover {
                    role: "security-admin".to_string(),
                },
                reason: "High risk capabilities require security admin approval".to_string(),
                iron_law: Some(IronLaw {
                    declaration: "NO HIGH-RISK EXECUTION WITHOUT SECURITY ADMIN APPROVAL".to_string(),
                    rationalization_table: vec![
                        RationalizationEntry {
                            excuse: "This is a read-only high-risk action".to_string(),
                            reality: "Risk level is determined by policy, not by perceived impact. Get approval.".to_string(),
                        },
                        RationalizationEntry {
                            excuse: "The security admin is unavailable".to_string(),
                            reality: "Unavailability means wait, not bypass. Escalate if truly urgent.".to_string(),
                        },
                    ],
                    red_flags: vec![
                        "Attempting to reclassify risk level to avoid approval".to_string(),
                        "Using words like 'probably safe' or 'should be fine'".to_string(),
                    ],
                }),
            },
            ApprovalRule {
                name: "write-capabilities-approval".to_string(),
                priority: 50,
                capability_pattern: None,
                risk_level: None,
                effect: Some(CapabilityEffect::Write),
                requirement: ApprovalRequirement::SingleApprover {
                    role: "data-admin".to_string(),
                },
                reason: "Write capabilities require data admin approval".to_string(),
                iron_law: None,
            },
        ];

        Self::new(
            rules,
            ApprovalRequirement::None,
            "No approval required for low-risk capabilities",
        )
    }

    /// Evaluate the approval policy for a capability
    pub fn evaluate(&self, capability: &CapabilityRef) -> GovernanceDecision {
        // Find the first matching rule
        for rule in &self.rules {
            if rule.matches(capability) {
                return rule.to_decision();
            }
        }

        // No rule matched, use default requirement
        match &self.default_requirement {
            ApprovalRequirement::None => GovernanceDecision::allow(&self.default_reason),
            requirement => {
                let temp_rule = ApprovalRule {
                    name: "default".to_string(),
                    priority: 0,
                    capability_pattern: None,
                    risk_level: None,
                    effect: None,
                    requirement: requirement.clone(),
                    reason: self.default_reason.clone(),
                    iron_law: None,
                };
                temp_rule.to_decision()
            }
        }
    }

    /// Add a new rule to the policy
    pub fn add_rule(&mut self, rule: ApprovalRule) {
        self.rules.push(rule);
        // Re-sort by priority
        self.rules.sort_by_key(|b| std::cmp::Reverse(b.priority));
    }

    /// Get all rules
    pub fn rules(&self) -> &[ApprovalRule] {
        &self.rules
    }

    /// Generate prompt-injectable rules from this policy's Iron Laws.
    ///
    /// Returns a list of rule strings suitable for injection into
    /// `PromptAssembler::set_rules()`. Each string contains the Iron Law
    /// declaration, rationalization table, and red flags for rules that
    /// have Iron Law behavioral reinforcement configured.
    pub fn generate_prompt_rules(&self) -> Vec<String> {
        self.rules
            .iter()
            .filter_map(|rule| {
                rule.iron_law
                    .as_ref()
                    .map(|law| format!("[Policy: {}]\n{}", rule.name, law.render()))
            })
            .collect()
    }
}

impl Default for ApprovalPolicy {
    fn default() -> Self {
        Self::default_policy()
    }
}

/// Approval policy engine wrapper
///
/// Wraps an ApprovalPolicy as a PolicyEngine for use in CompositePolicyEngine
#[derive(Clone)]
pub struct ApprovalPolicyEngine {
    policy: Arc<ApprovalPolicy>,
}

impl ApprovalPolicyEngine {
    /// Create a new approval policy engine
    pub fn new(policy: Arc<ApprovalPolicy>) -> Self {
        Self { policy }
    }

    /// Create with default policy
    pub fn default_engine() -> Self {
        Self::new(Arc::new(ApprovalPolicy::default()))
    }
}

#[async_trait]
impl PolicyEngine for ApprovalPolicyEngine {
    async fn evaluate_capability(
        &self,
        context: EvaluationContext,
    ) -> anyhow::Result<EvaluationResult> {
        let decision = self.policy.evaluate(&context.capability);

        let mut context_info = vec![
            format!("Approval policy evaluated for: {}", context.capability.id),
            format!("Risk level: {:?}", context.capability.risk),
        ];

        if !context.capability.effects.is_empty() {
            context_info.push(format!("Effects: {:?}", context.capability.effects));
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

    #[test]
    fn test_approval_requirement_serialization() {
        let req = ApprovalRequirement::SingleApprover {
            role: "admin".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let deserialized: ApprovalRequirement = serde_json::from_str(&json).unwrap();
        assert_eq!(req, deserialized);
    }

    #[test]
    fn test_glob_matching() {
        let rule = ApprovalRule {
            name: "test".to_string(),
            priority: 10,
            capability_pattern: Some("fs.*".to_string()),
            risk_level: None,
            effect: None,
            requirement: ApprovalRequirement::None,
            reason: "test".to_string(),
            iron_law: None,
        };

        assert!(rule.matches_glob("fs.*", "fs.read"));
        assert!(rule.matches_glob("fs.*", "fs.write"));
        assert!(!rule.matches_glob("fs.*", "network.fetch"));
        assert!(rule.matches_glob("*", "anything"));
        assert!(rule.matches_glob("*.read", "fs.read"));
        assert!(rule.matches_glob("*.read", "network.read"));
        assert!(!rule.matches_glob("*.read", "fs.write"));
    }

    #[test]
    fn test_approval_rule_matches() {
        let rule = ApprovalRule {
            name: "high-risk-rule".to_string(),
            priority: 10,
            capability_pattern: None,
            risk_level: Some(RiskLevel::High),
            effect: None,
            requirement: ApprovalRequirement::SingleApprover {
                role: "admin".to_string(),
            },
            reason: "test".to_string(),
            iron_law: None,
        };

        let high_cap = create_test_capability("test", RiskLevel::High, vec![]);
        let low_cap = create_test_capability("test", RiskLevel::Low, vec![]);

        assert!(rule.matches(&high_cap));
        assert!(!rule.matches(&low_cap));
    }

    #[test]
    fn test_default_approval_policy_critical() {
        let policy = ApprovalPolicy::default();
        let capability = create_test_capability("test", RiskLevel::Critical, vec![]);

        let decision = policy.evaluate(&capability);
        assert!(decision.is_review_required());
        assert_eq!(decision.review_type(), Some(&ReviewType::Escalation));
        assert!(decision.reason().contains("chain-of-command"));
    }

    #[test]
    fn test_default_approval_policy_high() {
        let policy = ApprovalPolicy::default();
        let capability = create_test_capability("test", RiskLevel::High, vec![]);

        let decision = policy.evaluate(&capability);
        assert!(decision.is_review_required());
        assert_eq!(decision.review_type(), Some(&ReviewType::Approval));
        assert!(decision.reason().contains("security admin"));
    }

    #[test]
    fn test_default_approval_policy_write() {
        let policy = ApprovalPolicy::default();
        let capability =
            create_test_capability("test", RiskLevel::Low, vec![CapabilityEffect::Write]);

        let decision = policy.evaluate(&capability);
        assert!(decision.is_review_required());
        assert!(decision.reason().contains("data admin"));
    }

    #[test]
    fn test_default_approval_policy_low() {
        let policy = ApprovalPolicy::default();
        let capability = create_test_capability("test", RiskLevel::Low, vec![]);

        let decision = policy.evaluate(&capability);
        assert!(decision.is_allow());
    }

    #[test]
    fn test_approval_policy_priority() {
        let mut policy = ApprovalPolicy::new(vec![], ApprovalRequirement::None, "default allow");

        // Add low priority rule
        policy.add_rule(ApprovalRule {
            name: "low-priority".to_string(),
            priority: 10,
            capability_pattern: None,
            risk_level: Some(RiskLevel::High),
            effect: None,
            requirement: ApprovalRequirement::None,
            reason: "low priority allows".to_string(),
            iron_law: None,
        });

        // Add high priority rule
        policy.add_rule(ApprovalRule {
            name: "high-priority".to_string(),
            priority: 100,
            capability_pattern: None,
            risk_level: Some(RiskLevel::High),
            effect: None,
            requirement: ApprovalRequirement::SingleApprover {
                role: "admin".to_string(),
            },
            reason: "high priority requires approval".to_string(),
            iron_law: None,
        });

        let capability = create_test_capability("test", RiskLevel::High, vec![]);
        let decision = policy.evaluate(&capability);

        // Should use high priority rule (require approval)
        assert!(decision.is_review_required());
        assert!(decision.reason().contains("high priority"));
    }

    #[tokio::test]
    async fn test_approval_policy_engine() {
        let policy = Arc::new(ApprovalPolicy::default());
        let engine = ApprovalPolicyEngine::new(policy);

        let capability = create_test_capability("test", RiskLevel::Critical, vec![]);
        let actor = ActorRef {
            id: ActorId::from_string("test-actor".to_string()).unwrap(),
            actor_type: ActorType::Agent,
            tenant_id: None,
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
        assert!(result.decision.is_review_required());
        assert_eq!(result.evaluated_risk, RiskLevel::Critical);
    }

    // -- Iron Law tests -----------------------------------------------------

    #[test]
    fn test_iron_law_render_full() {
        let law = IronLaw {
            declaration: "NO DESTRUCTIVE OPERATIONS WITHOUT APPROVAL".to_string(),
            rationalization_table: vec![
                RationalizationEntry {
                    excuse: "It's urgent".to_string(),
                    reality: "Urgency does not override safety.".to_string(),
                },
                RationalizationEntry {
                    excuse: "I'm confident this is safe".to_string(),
                    reality: "Confidence is not authorization.".to_string(),
                },
            ],
            red_flags: vec![
                "About to execute without approval evidence".to_string(),
                "Reasoning about why this case is an exception".to_string(),
            ],
        };

        let rendered = law.render();

        // Iron Law declaration
        assert!(rendered.contains("IRON LAW: NO DESTRUCTIVE OPERATIONS WITHOUT APPROVAL"));
        assert!(rendered.contains("Violating the letter of this rule is violating the spirit"));

        // Rationalization table
        assert!(rendered.contains("Rationalization Prevention:"));
        assert!(rendered.contains("| Excuse | Reality |"));
        assert!(rendered.contains("\"It's urgent\""));
        assert!(rendered.contains("Urgency does not override safety."));
        assert!(rendered.contains("\"I'm confident this is safe\""));
        assert!(rendered.contains("Confidence is not authorization."));

        // Red flags
        assert!(rendered.contains("Red Flags"));
        assert!(rendered.contains("- About to execute without approval evidence"));
        assert!(rendered.contains("- Reasoning about why this case is an exception"));
    }

    #[test]
    fn test_iron_law_render_minimal() {
        let law = IronLaw {
            declaration: "ALWAYS VERIFY BEFORE CLAIMING DONE".to_string(),
            rationalization_table: vec![],
            red_flags: vec![],
        };

        let rendered = law.render();
        assert!(rendered.contains("IRON LAW: ALWAYS VERIFY"));
        assert!(!rendered.contains("Rationalization Prevention:"));
        assert!(!rendered.contains("Red Flags"));
    }

    #[test]
    fn test_default_policy_has_iron_laws() {
        let policy = ApprovalPolicy::default();

        // Critical and High rules should have Iron Laws
        let critical_rule = policy
            .rules()
            .iter()
            .find(|r| r.name == "critical-chain-of-command");
        assert!(critical_rule.is_some());
        assert!(critical_rule.unwrap().iron_law.is_some());

        let high_rule = policy
            .rules()
            .iter()
            .find(|r| r.name == "high-risk-approval");
        assert!(high_rule.is_some());
        assert!(high_rule.unwrap().iron_law.is_some());

        // Write rule should not have an Iron Law
        let write_rule = policy
            .rules()
            .iter()
            .find(|r| r.name == "write-capabilities-approval");
        assert!(write_rule.is_some());
        assert!(write_rule.unwrap().iron_law.is_none());
    }

    #[test]
    fn test_generate_prompt_rules() {
        let policy = ApprovalPolicy::default();
        let rules = policy.generate_prompt_rules();

        // Should generate rules for critical and high (both have iron_law)
        assert_eq!(rules.len(), 2);

        // Critical rule
        assert!(rules[0].contains("[Policy: critical-chain-of-command]"));
        assert!(rules[0].contains("IRON LAW:"));
        assert!(rules[0].contains("CHAIN-OF-COMMAND"));
        assert!(rules[0].contains("Rationalization Prevention:"));

        // High rule
        assert!(rules[1].contains("[Policy: high-risk-approval]"));
        assert!(rules[1].contains("IRON LAW:"));
        assert!(rules[1].contains("SECURITY ADMIN"));
    }

    #[test]
    fn test_generate_prompt_rules_empty_for_no_iron_laws() {
        let policy = ApprovalPolicy::new(
            vec![ApprovalRule {
                name: "simple-rule".to_string(),
                priority: 10,
                capability_pattern: None,
                risk_level: None,
                effect: None,
                requirement: ApprovalRequirement::None,
                reason: "no iron law".to_string(),
                iron_law: None,
            }],
            ApprovalRequirement::None,
            "default",
        );

        let rules = policy.generate_prompt_rules();
        assert!(rules.is_empty());
    }

    #[test]
    fn test_iron_law_serialization() {
        let law = IronLaw {
            declaration: "TEST LAW".to_string(),
            rationalization_table: vec![RationalizationEntry {
                excuse: "excuse".to_string(),
                reality: "reality".to_string(),
            }],
            red_flags: vec!["flag".to_string()],
        };

        let json = serde_json::to_string(&law).unwrap();
        let deserialized: IronLaw = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.declaration, "TEST LAW");
        assert_eq!(deserialized.rationalization_table.len(), 1);
        assert_eq!(deserialized.red_flags.len(), 1);
    }
}
