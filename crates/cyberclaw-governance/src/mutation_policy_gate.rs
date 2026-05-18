//! MutationPolicyGate — vets MutationPlan instances before dispatch.
//!
//! The self-evolution loop (see `crates/cyberclaw-control-plane/src/mutation_engine.rs`)
//! emits `MutationPlan` values that describe *what* capability should be
//! dispatched to mutate a skill variant. Governance must be able to veto such
//! plans before any connector is invoked.
//!
//! Inspection rules:
//! 1. `target_capability` must be on an allowlist (deny by default).
//! 2. `MutationConstraint::SizeLimit` must not exceed a hard platform ceiling.
//! 3. `MutationConstraint::GrowthLimit` must not exceed a hard platform ceiling.
//! 4. If `require_parent_for_mutation` is set, plans without a
//!    `parent_variant_id` are denied (root seeding must go through an
//!    explicit elevated path).
//!
//! The gate is a **pure function** — `check()` has no side effects. It does
//! not read disk, emit metrics, or invoke capabilities. This mirrors the
//! shape of [`crate::dangerous_capability_filter::DangerousCapabilityFilter`]
//! and keeps governance composable.
//!
//! To avoid a dependency cycle (governance must not depend on control-plane),
//! the gate works against a [`MutationPlanView`] trait that projects only the
//! fields the gate needs. Control-plane's concrete `MutationPlan` implements
//! the trait adjacent to its definition.

use serde::{Deserialize, Serialize};

// ============================================================================
// Defaults
// ============================================================================

/// Default hard ceiling on the `SizeLimit` constraint value, in characters.
pub const DEFAULT_MAX_ALLOWED_SIZE: usize = 30_000;

/// Default hard ceiling on the `GrowthLimit` constraint value, as a percentage.
pub const DEFAULT_MAX_ALLOWED_GROWTH_PCT: f32 = 50.0;

// ============================================================================
// Plan View Trait
// ============================================================================

/// Read-only projection of a mutation plan for governance inspection.
///
/// Concrete mutation plans (e.g. `cyberclaw_control_plane::mutation_engine::MutationPlan`)
/// implement this trait so governance can evaluate them without a direct
/// type dependency on the control-plane crate.
pub trait MutationPlanView {
    /// Capability identifier the plan targets (e.g. `"llm.generate_diff"`).
    fn target_capability_id(&self) -> &str;

    /// The `SizeLimit` constraint value, in characters, if present.
    fn size_limit(&self) -> Option<usize>;

    /// The `GrowthLimit` constraint value, expressed as a percentage
    /// (e.g. `20.0` for 20%), if present.
    fn growth_limit_pct(&self) -> Option<f32>;

    /// Whether the plan has a parent variant (i.e. is a mutation rather than
    /// a root seed).
    fn has_parent(&self) -> bool;
}

// ============================================================================
// Policy Verdict
// ============================================================================

/// The outcome of a [`MutationPolicyGate::check`] call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PolicyVerdict {
    /// The plan is permitted to dispatch.
    Allow,
    /// The plan is rejected.
    Deny {
        /// Human-readable explanation for the denial.
        reason: String,
        /// Stable rule identifier (e.g. `"MP001"`).
        rule_id: String,
    },
}

// ============================================================================
// Rule IDs
// ============================================================================

/// Rule ID: target capability not on the allowlist.
pub const RULE_CAPABILITY_NOT_ALLOWED: &str = "MP001";
/// Rule ID: SizeLimit constraint exceeds the platform ceiling.
pub const RULE_SIZE_LIMIT_EXCEEDED: &str = "MP002";
/// Rule ID: GrowthLimit constraint exceeds the platform ceiling.
pub const RULE_GROWTH_LIMIT_EXCEEDED: &str = "MP003";
/// Rule ID: plan has no parent but one is required.
pub const RULE_MISSING_PARENT: &str = "MP004";

// ============================================================================
// Gate
// ============================================================================

/// Policy gate that vets mutation plans before dispatch.
#[derive(Debug, Clone)]
pub struct MutationPolicyGate {
    /// Capability ids the gate will permit. Plan must target exactly one.
    allowed_capabilities: Vec<String>,
    /// Hard ceiling on `SizeLimit` constraint value (default 30_000 chars).
    max_allowed_size: usize,
    /// Hard ceiling on `GrowthLimit` percentage (default 50%).
    max_allowed_growth_pct: f32,
    /// If `true`, plans without a `parent_variant_id` are denied.
    require_parent_for_mutation: bool,
}

impl MutationPolicyGate {
    /// Construct a gate with the supplied capability allowlist and the
    /// default size / growth ceilings.
    pub fn new(allowed_capabilities: Vec<String>) -> Self {
        Self {
            allowed_capabilities,
            max_allowed_size: DEFAULT_MAX_ALLOWED_SIZE,
            max_allowed_growth_pct: DEFAULT_MAX_ALLOWED_GROWTH_PCT,
            require_parent_for_mutation: false,
        }
    }

    /// Override the hard ceiling on `SizeLimit`.
    pub fn with_max_size(mut self, max: usize) -> Self {
        self.max_allowed_size = max;
        self
    }

    /// Override the hard ceiling on `GrowthLimit` (as a percentage).
    pub fn with_max_growth_pct(mut self, max: f32) -> Self {
        self.max_allowed_growth_pct = max;
        self
    }

    /// Toggle whether plans without a parent variant are rejected.
    pub fn require_parent(mut self, required: bool) -> Self {
        self.require_parent_for_mutation = required;
        self
    }

    /// Inspect a mutation plan. Returns [`PolicyVerdict::Allow`] or
    /// [`PolicyVerdict::Deny`] with an explanatory reason and rule id.
    ///
    /// Pure function — no side effects.
    pub fn check<P: MutationPlanView + ?Sized>(&self, plan: &P) -> PolicyVerdict {
        // Rule MP001: target capability must be on the allowlist.
        let cap_id = plan.target_capability_id();
        if !self.allowed_capabilities.iter().any(|c| c == cap_id) {
            return PolicyVerdict::Deny {
                reason: format!(
                    "target capability '{}' is not on the mutation allowlist",
                    cap_id
                ),
                rule_id: RULE_CAPABILITY_NOT_ALLOWED.to_string(),
            };
        }

        // Rule MP002: SizeLimit constraint must not exceed the ceiling.
        if let Some(size) = plan.size_limit() {
            if size > self.max_allowed_size {
                return PolicyVerdict::Deny {
                    reason: format!(
                        "SizeLimit {} exceeds platform ceiling {}",
                        size, self.max_allowed_size
                    ),
                    rule_id: RULE_SIZE_LIMIT_EXCEEDED.to_string(),
                };
            }
        }

        // Rule MP003: GrowthLimit constraint must not exceed the ceiling.
        if let Some(growth) = plan.growth_limit_pct() {
            if growth > self.max_allowed_growth_pct {
                return PolicyVerdict::Deny {
                    reason: format!(
                        "GrowthLimit {:.2}% exceeds platform ceiling {:.2}%",
                        growth, self.max_allowed_growth_pct
                    ),
                    rule_id: RULE_GROWTH_LIMIT_EXCEEDED.to_string(),
                };
            }
        }

        // Rule MP004: require parent if configured.
        if self.require_parent_for_mutation && !plan.has_parent() {
            return PolicyVerdict::Deny {
                reason: "plan has no parent_variant_id; root seeding requires an elevated actor"
                    .to_string(),
                rule_id: RULE_MISSING_PARENT.to_string(),
            };
        }

        PolicyVerdict::Allow
    }

    /// Read-only accessor for the allowlist.
    pub fn allowed_capabilities(&self) -> &[String] {
        &self.allowed_capabilities
    }

    /// Read-only accessor for the size ceiling.
    pub fn max_allowed_size(&self) -> usize {
        self.max_allowed_size
    }

    /// Read-only accessor for the growth ceiling.
    pub fn max_allowed_growth_pct(&self) -> f32 {
        self.max_allowed_growth_pct
    }

    /// Read-only accessor for the require-parent flag.
    pub fn requires_parent(&self) -> bool {
        self.require_parent_for_mutation
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal mock implementing [`MutationPlanView`] for unit testing.
    struct MockPlan {
        capability_id: String,
        size_limit: Option<usize>,
        growth_pct: Option<f32>,
        has_parent: bool,
    }

    impl MockPlan {
        fn new(capability_id: &str) -> Self {
            Self {
                capability_id: capability_id.to_string(),
                size_limit: None,
                growth_pct: None,
                has_parent: true,
            }
        }

        fn with_size(mut self, size: usize) -> Self {
            self.size_limit = Some(size);
            self
        }

        fn with_growth(mut self, pct: f32) -> Self {
            self.growth_pct = Some(pct);
            self
        }

        fn without_parent(mut self) -> Self {
            self.has_parent = false;
            self
        }
    }

    impl MutationPlanView for MockPlan {
        fn target_capability_id(&self) -> &str {
            &self.capability_id
        }
        fn size_limit(&self) -> Option<usize> {
            self.size_limit
        }
        fn growth_limit_pct(&self) -> Option<f32> {
            self.growth_pct
        }
        fn has_parent(&self) -> bool {
            self.has_parent
        }
    }

    // -- allowlist --

    #[test]
    fn test_default_gate_denies_unknown_capability() {
        let gate = MutationPolicyGate::new(vec!["llm.generate_diff".to_string()]);
        let plan = MockPlan::new("shell.exec");
        match gate.check(&plan) {
            PolicyVerdict::Deny { rule_id, .. } => {
                assert_eq!(rule_id, RULE_CAPABILITY_NOT_ALLOWED);
            }
            other => panic!("expected Deny, got {:?}", other),
        }
    }

    #[test]
    fn test_allowlist_accepts_allowed_capability() {
        let gate = MutationPolicyGate::new(vec!["llm.generate_diff".to_string()]);
        let plan = MockPlan::new("llm.generate_diff");
        assert_eq!(gate.check(&plan), PolicyVerdict::Allow);
    }

    #[test]
    fn test_empty_allowlist_denies_all() {
        let gate = MutationPolicyGate::new(vec![]);
        let plan = MockPlan::new("llm.generate_diff");
        match gate.check(&plan) {
            PolicyVerdict::Deny { rule_id, .. } => {
                assert_eq!(rule_id, RULE_CAPABILITY_NOT_ALLOWED);
            }
            other => panic!("expected Deny, got {:?}", other),
        }
    }

    // -- size ceiling --

    #[test]
    fn test_size_limit_within_ceiling_allows() {
        let gate = MutationPolicyGate::new(vec!["llm.generate_diff".to_string()]);
        let plan = MockPlan::new("llm.generate_diff").with_size(10_000);
        assert_eq!(gate.check(&plan), PolicyVerdict::Allow);
    }

    #[test]
    fn test_size_limit_exceeding_ceiling_denies() {
        let gate = MutationPolicyGate::new(vec!["llm.generate_diff".to_string()]);
        let plan = MockPlan::new("llm.generate_diff").with_size(100_000);
        match gate.check(&plan) {
            PolicyVerdict::Deny { rule_id, reason } => {
                assert_eq!(rule_id, RULE_SIZE_LIMIT_EXCEEDED);
                assert!(reason.contains("100000"));
            }
            other => panic!("expected Deny, got {:?}", other),
        }
    }

    #[test]
    fn test_size_limit_at_ceiling_allows() {
        let gate = MutationPolicyGate::new(vec!["llm.generate_diff".to_string()]);
        let plan = MockPlan::new("llm.generate_diff").with_size(DEFAULT_MAX_ALLOWED_SIZE);
        assert_eq!(gate.check(&plan), PolicyVerdict::Allow);
    }

    #[test]
    fn test_with_max_size_override() {
        let gate =
            MutationPolicyGate::new(vec!["llm.generate_diff".to_string()]).with_max_size(5_000);
        let plan = MockPlan::new("llm.generate_diff").with_size(6_000);
        match gate.check(&plan) {
            PolicyVerdict::Deny { rule_id, .. } => {
                assert_eq!(rule_id, RULE_SIZE_LIMIT_EXCEEDED);
            }
            other => panic!("expected Deny, got {:?}", other),
        }
    }

    // -- growth ceiling --

    #[test]
    fn test_growth_limit_within_ceiling_allows() {
        let gate = MutationPolicyGate::new(vec!["llm.generate_diff".to_string()]);
        let plan = MockPlan::new("llm.generate_diff").with_growth(20.0);
        assert_eq!(gate.check(&plan), PolicyVerdict::Allow);
    }

    #[test]
    fn test_growth_limit_exceeding_ceiling_denies() {
        let gate = MutationPolicyGate::new(vec!["llm.generate_diff".to_string()]);
        let plan = MockPlan::new("llm.generate_diff").with_growth(75.0);
        match gate.check(&plan) {
            PolicyVerdict::Deny { rule_id, .. } => {
                assert_eq!(rule_id, RULE_GROWTH_LIMIT_EXCEEDED);
            }
            other => panic!("expected Deny, got {:?}", other),
        }
    }

    #[test]
    fn test_with_max_growth_pct_override() {
        let gate = MutationPolicyGate::new(vec!["llm.generate_diff".to_string()])
            .with_max_growth_pct(10.0);
        let plan = MockPlan::new("llm.generate_diff").with_growth(15.0);
        match gate.check(&plan) {
            PolicyVerdict::Deny { rule_id, .. } => {
                assert_eq!(rule_id, RULE_GROWTH_LIMIT_EXCEEDED);
            }
            other => panic!("expected Deny, got {:?}", other),
        }
    }

    // -- parent requirement --

    #[test]
    fn test_require_parent_denies_plan_without_parent() {
        let gate =
            MutationPolicyGate::new(vec!["llm.generate_diff".to_string()]).require_parent(true);
        let plan = MockPlan::new("llm.generate_diff").without_parent();
        match gate.check(&plan) {
            PolicyVerdict::Deny { rule_id, .. } => {
                assert_eq!(rule_id, RULE_MISSING_PARENT);
            }
            other => panic!("expected Deny, got {:?}", other),
        }
    }

    #[test]
    fn test_require_parent_disabled_allows_plan_without_parent() {
        let gate =
            MutationPolicyGate::new(vec!["llm.generate_diff".to_string()]).require_parent(false);
        let plan = MockPlan::new("llm.generate_diff").without_parent();
        assert_eq!(gate.check(&plan), PolicyVerdict::Allow);
    }

    #[test]
    fn test_require_parent_allows_plan_with_parent() {
        let gate =
            MutationPolicyGate::new(vec!["llm.generate_diff".to_string()]).require_parent(true);
        let plan = MockPlan::new("llm.generate_diff"); // has_parent = true by default
        assert_eq!(gate.check(&plan), PolicyVerdict::Allow);
    }

    // -- multi-constraint / ordering --

    #[test]
    fn test_multiple_constraints_one_over_ceiling_denies_with_matching_rule_id() {
        // Size is fine, growth is over — should deny with the growth rule id,
        // because size is checked first and passes.
        let gate = MutationPolicyGate::new(vec!["llm.generate_diff".to_string()]);
        let plan = MockPlan::new("llm.generate_diff")
            .with_size(5_000)
            .with_growth(80.0);
        match gate.check(&plan) {
            PolicyVerdict::Deny { rule_id, .. } => {
                assert_eq!(rule_id, RULE_GROWTH_LIMIT_EXCEEDED);
            }
            other => panic!("expected Deny, got {:?}", other),
        }
    }

    #[test]
    fn test_allowlist_checked_before_size() {
        // Capability is not allowed AND size is too big. Capability check is
        // first, so we should see MP001, not MP002.
        let gate = MutationPolicyGate::new(vec!["llm.generate_diff".to_string()]);
        let plan = MockPlan::new("shell.exec").with_size(1_000_000);
        match gate.check(&plan) {
            PolicyVerdict::Deny { rule_id, .. } => {
                assert_eq!(rule_id, RULE_CAPABILITY_NOT_ALLOWED);
            }
            other => panic!("expected Deny, got {:?}", other),
        }
    }

    // -- serde --

    #[test]
    fn test_policy_verdict_serde_roundtrip_allow() {
        let verdict = PolicyVerdict::Allow;
        let json = serde_json::to_string(&verdict).expect("serialize");
        let back: PolicyVerdict = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, PolicyVerdict::Allow);
    }

    #[test]
    fn test_policy_verdict_serde_roundtrip_deny() {
        let verdict = PolicyVerdict::Deny {
            reason: "size too big".to_string(),
            rule_id: RULE_SIZE_LIMIT_EXCEEDED.to_string(),
        };
        let json = serde_json::to_string(&verdict).expect("serialize");
        let back: PolicyVerdict = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, verdict);
    }

    // -- defaults --

    #[test]
    fn test_default_max_allowed_size_is_30000() {
        let gate = MutationPolicyGate::new(vec!["x".to_string()]);
        assert_eq!(gate.max_allowed_size(), 30_000);
        assert_eq!(DEFAULT_MAX_ALLOWED_SIZE, 30_000);
    }

    #[test]
    fn test_default_max_allowed_growth_pct_is_50() {
        let gate = MutationPolicyGate::new(vec!["x".to_string()]);
        assert!((gate.max_allowed_growth_pct() - 50.0).abs() < f32::EPSILON);
        assert!((DEFAULT_MAX_ALLOWED_GROWTH_PCT - 50.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_default_require_parent_is_false() {
        let gate = MutationPolicyGate::new(vec!["x".to_string()]);
        assert!(!gate.requires_parent());
    }
}
