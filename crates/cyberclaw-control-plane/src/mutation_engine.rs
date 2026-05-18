//! Mutation Engine — plan-only orchestration for Skill variant mutation.
//!
//! Produces `MutationPlan` values that describe *what* Capability should be
//! invoked to mutate a parent `SkillVariant` into a child. The engine itself
//! performs **no side effects** and **never invokes an LLM directly**.
//! Dispatch is the orchestrator's responsibility, consistent with the
//! architectural constraint that "Connector is the only code-level capability
//! interface" (`CLAUDE.md` §3).
//!
//! The pattern mirrors `StagedVerificationGate::check()` in
//! [`crate::verification_gate`] — it defines the shape of work but does not
//! execute it.
//!
//! Borrowed defaults from Hermes / GEPA:
//! - `SizeLimit`: 15000 characters (Hermes SKILL.md ceiling).
//! - `GrowthLimit`: 20% (max relative growth per generation).
//! - Feedback channel (GEPA): previous evaluation failure text is passed
//!   back into the mutation prompt so the LLM can learn from mistakes.

use cyberclaw_core::capability::CapabilityRef;
use cyberclaw_core::ids::{ArtifactId, SkillId, VariantId};
use serde::{Deserialize, Serialize};

use crate::skill_archive::SkillVariant;

// ============================================================================
// Defaults
// ============================================================================

/// Default character ceiling for a mutated SKILL.md (Hermes-aligned).
pub const DEFAULT_SIZE_LIMIT_CHARS: u64 = 15_000;

/// Default max relative growth per mutation generation, as a fraction
/// (Hermes-aligned: 20% => 0.20).
pub const DEFAULT_GROWTH_LIMIT_FRACTION: f32 = 0.20;

// ============================================================================
// Mutation Output Kind
// ============================================================================

/// Shape the orchestrator should expect back from the dispatched capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationOutputKind {
    /// Unified diff of the SKILL.md file.
    SkillTextDiff,
    /// Complete new skill text (full replacement).
    FullReplacement,
    /// Structured JSON patch operations (RFC 6902 style).
    StructuredPatch,
}

// ============================================================================
// Constraint Kinds
// ============================================================================

/// Kinds of constraints that can shape a mutation.
///
/// The actual bound is carried in `MutationConstraint::value` as a
/// `serde_json::Value` so each kind can pick the representation that fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintKind {
    /// Maximum character count of the resulting skill text.
    /// `value` = integer number of characters.
    SizeLimit,
    /// Maximum relative growth compared to parent, as a fraction in [0, ∞).
    /// `value` = float (e.g. `0.20` for 20%).
    GrowthLimit,
    /// Require that the YAML frontmatter / section layout remain intact.
    /// `value` = boolean (`true` to enforce).
    StructurePreserved,
}

/// A single constraint attached to a mutation plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationConstraint {
    pub kind: ConstraintKind,
    pub value: serde_json::Value,
}

impl MutationConstraint {
    /// Build a default `SizeLimit` constraint (Hermes ceiling).
    pub fn default_size_limit() -> Self {
        Self {
            kind: ConstraintKind::SizeLimit,
            value: serde_json::Value::from(DEFAULT_SIZE_LIMIT_CHARS),
        }
    }

    /// Build a default `GrowthLimit` constraint (Hermes 20%).
    pub fn default_growth_limit() -> Self {
        Self {
            kind: ConstraintKind::GrowthLimit,
            value: serde_json::Value::from(DEFAULT_GROWTH_LIMIT_FRACTION as f64),
        }
    }

    /// Build a default `StructurePreserved` constraint.
    pub fn default_structure_preserved() -> Self {
        Self {
            kind: ConstraintKind::StructurePreserved,
            value: serde_json::Value::Bool(true),
        }
    }
}

// ============================================================================
// Mutation Input
// ============================================================================

/// Input material that will be threaded to the LLM capability via the
/// dispatched `ActionRequest`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationInput {
    /// Full parent SKILL.md text that the mutation is based on.
    pub parent_skill_text: String,
    /// GEPA-style feedback from the previous evaluation, if any.
    pub failure_feedback: Option<String>,
    /// Hard / soft constraints on the mutation.
    pub constraints: Vec<MutationConstraint>,
}

// ============================================================================
// Mutation Plan
// ============================================================================

/// A pure description of a planned mutation.
///
/// The orchestrator is expected to:
/// 1. Look up `target_capability` in the connector registry.
/// 2. Build an `ActionRequest` whose `input` is derived from `input` and
///    `expected_output_schema`.
/// 3. Dispatch the request through the standard execution main-chain.
/// 4. Parse the returned artifact into a `MutationResult`.
///
/// `MutationEngine` deliberately does none of the above.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationPlan {
    /// The parent variant this plan derives from (None for root seeding).
    pub parent_variant_id: Option<VariantId>,
    /// Which Skill the mutation targets.
    pub skill_id: SkillId,
    /// Capability the orchestrator should dispatch (e.g. `llm.generate_diff`).
    pub target_capability: CapabilityRef,
    /// Materialised inputs for the dispatched capability.
    pub input: MutationInput,
    /// Expected output shape the orchestrator should parse.
    pub expected_output_schema: MutationOutputKind,
}

// ============================================================================
// Governance View
// ============================================================================

/// Project `MutationPlan` onto the governance [`cyberclaw_governance::MutationPlanView`]
/// trait so [`cyberclaw_governance::MutationPolicyGate`] can inspect plans
/// without a reverse crate dependency.
impl cyberclaw_governance::MutationPlanView for MutationPlan {
    fn target_capability_id(&self) -> &str {
        self.target_capability.id.as_str()
    }

    fn size_limit(&self) -> Option<usize> {
        self.input.constraints.iter().find_map(|c| {
            if c.kind == ConstraintKind::SizeLimit {
                c.value.as_u64().map(|v| v as usize)
            } else {
                None
            }
        })
    }

    fn growth_limit_pct(&self) -> Option<f32> {
        self.input.constraints.iter().find_map(|c| {
            if c.kind == ConstraintKind::GrowthLimit {
                // Constraint value is a fraction (e.g. 0.20); surface as a
                // percentage (20.0) for governance ceilings.
                c.value.as_f64().map(|v| (v * 100.0) as f32)
            } else {
                None
            }
        })
    }

    fn has_parent(&self) -> bool {
        self.parent_variant_id.is_some()
    }
}

// ============================================================================
// Mutation Result
// ============================================================================

/// Result shape the orchestrator builds *after* dispatching the plan.
///
/// `MutationEngine` does not produce this — it is included here purely as the
/// canonical type so callers and archivers share a vocabulary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationResult {
    /// Resulting child SKILL.md text.
    pub child_skill_text: String,
    /// Optional artifact id of the stored diff / patch.
    pub diff_artifact_id: Option<ArtifactId>,
    /// Model-provided reasoning, if captured.
    pub reasoning: Option<String>,
}

// ============================================================================
// Mutation Engine (planner)
// ============================================================================

/// Plans mutations. Pure, no side effects.
#[derive(Debug, Clone)]
pub struct MutationEngine {
    /// Capability the orchestrator should dispatch for mutation by default.
    default_capability: CapabilityRef,
    /// Constraints attached to every plan (callers can extend per-plan later).
    default_constraints: Vec<MutationConstraint>,
    /// Default output shape expected from the dispatched capability.
    default_output_kind: MutationOutputKind,
}

impl MutationEngine {
    /// Create a new engine using sensible Hermes-aligned defaults:
    /// `SizeLimit=15000` and `GrowthLimit=0.20`, output = `SkillTextDiff`.
    pub fn new(default_capability: CapabilityRef) -> Self {
        Self {
            default_capability,
            default_constraints: vec![
                MutationConstraint::default_size_limit(),
                MutationConstraint::default_growth_limit(),
            ],
            default_output_kind: MutationOutputKind::SkillTextDiff,
        }
    }

    /// Override the default constraint set.
    pub fn with_constraints(mut self, constraints: Vec<MutationConstraint>) -> Self {
        self.default_constraints = constraints;
        self
    }

    /// Override the default expected output shape.
    pub fn with_output_kind(mut self, kind: MutationOutputKind) -> Self {
        self.default_output_kind = kind;
        self
    }

    /// Read-only accessor for the default capability.
    pub fn default_capability(&self) -> &CapabilityRef {
        &self.default_capability
    }

    /// Read-only accessor for the default constraints.
    pub fn default_constraints(&self) -> &[MutationConstraint] {
        &self.default_constraints
    }

    /// Produce a `MutationPlan` for `parent`. Pure function.
    ///
    /// `parent_skill_text` is passed in rather than read from disk because
    /// `SkillVariant` only tracks an artifact id — the orchestrator / caller
    /// is responsible for fetching the actual text through the artifact store.
    pub fn plan(
        &self,
        parent: &SkillVariant,
        parent_skill_text: String,
        feedback: Option<String>,
    ) -> MutationPlan {
        MutationPlan {
            parent_variant_id: Some(parent.variant_id.clone()),
            skill_id: parent.skill_id.clone(),
            target_capability: self.default_capability.clone(),
            input: MutationInput {
                parent_skill_text,
                failure_feedback: feedback,
                constraints: self.default_constraints.clone(),
            },
            expected_output_schema: self.default_output_kind,
        }
    }

    /// Produce a plan for a *root* variant (no parent).
    ///
    /// Used when seeding a Skill archive — the "parent text" is whatever
    /// initial SKILL.md the caller wants to mutate, and the resulting plan
    /// carries `parent_variant_id = None`.
    pub fn plan_root(
        &self,
        skill_id: SkillId,
        parent_skill_text: String,
        feedback: Option<String>,
    ) -> MutationPlan {
        MutationPlan {
            parent_variant_id: None,
            skill_id,
            target_capability: self.default_capability.clone(),
            input: MutationInput {
                parent_skill_text,
                failure_feedback: feedback,
                constraints: self.default_constraints.clone(),
            },
            expected_output_schema: self.default_output_kind,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::capability::{CapabilityEffect, RiskLevel};
    use cyberclaw_core::ids::{CapabilityId, ConnectorId};

    use crate::skill_evolution::CaseTrack;

    // -- helpers --

    fn mk_cap(id: &str) -> CapabilityRef {
        CapabilityRef {
            id: CapabilityId::from_string(id.to_string()).expect("cap id"),
            connector_id: ConnectorId::from_string("llm".to_string()).expect("conn id"),
            risk: RiskLevel::Medium,
            effects: vec![CapabilityEffect::Execute],
            placement: None,
        }
    }

    fn mk_variant() -> SkillVariant {
        SkillVariant {
            variant_id: VariantId::new(),
            parent_variant_id: None,
            skill_id: SkillId::new(),
            score: 0.75,
            child_count: 0,
            track: CaseTrack::Success,
            patch_artifact_id: None,
        }
    }

    // -- Tests --

    #[test]
    fn test_plan_creation_without_feedback() {
        let engine = MutationEngine::new(mk_cap("llm.generate_diff"));
        let parent = mk_variant();
        let plan = engine.plan(&parent, "# Hello".to_string(), None);

        assert_eq!(plan.parent_variant_id, Some(parent.variant_id.clone()));
        assert_eq!(plan.skill_id, parent.skill_id);
        assert_eq!(plan.input.parent_skill_text, "# Hello");
        assert!(plan.input.failure_feedback.is_none());
        // Two default constraints (size + growth).
        assert_eq!(plan.input.constraints.len(), 2);
    }

    #[test]
    fn test_plan_creation_with_feedback() {
        let engine = MutationEngine::new(mk_cap("llm.generate_diff"));
        let parent = mk_variant();
        let plan = engine.plan(
            &parent,
            "body".to_string(),
            Some("prev run failed: timeout".to_string()),
        );

        assert_eq!(
            plan.input.failure_feedback.as_deref(),
            Some("prev run failed: timeout")
        );
    }

    #[test]
    fn test_constraint_propagation_with_constraints_override() {
        let custom = vec![MutationConstraint {
            kind: ConstraintKind::SizeLimit,
            value: serde_json::Value::from(2_000u64),
        }];
        let engine =
            MutationEngine::new(mk_cap("llm.generate_diff")).with_constraints(custom.clone());
        let plan = engine.plan(&mk_variant(), "x".to_string(), None);

        assert_eq!(plan.input.constraints.len(), 1);
        assert_eq!(plan.input.constraints[0].kind, ConstraintKind::SizeLimit);
        assert_eq!(plan.input.constraints[0].value, serde_json::json!(2_000u64));
    }

    #[test]
    fn test_with_constraints_replaces_defaults() {
        let engine = MutationEngine::new(mk_cap("llm.generate_diff"));
        assert_eq!(engine.default_constraints().len(), 2);

        let engine = engine.with_constraints(vec![]);
        assert!(engine.default_constraints().is_empty());
    }

    #[test]
    fn test_serde_roundtrip_mutation_plan() {
        let engine = MutationEngine::new(mk_cap("llm.generate_diff"));
        let plan = engine.plan(&mk_variant(), "text".to_string(), Some("fb".to_string()));

        let json = serde_json::to_string(&plan).expect("serialize");
        let deser: MutationPlan = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deser.input.parent_skill_text, "text");
        assert_eq!(deser.input.failure_feedback.as_deref(), Some("fb"));
        assert_eq!(deser.input.constraints.len(), plan.input.constraints.len());
        assert_eq!(deser.expected_output_schema, plan.expected_output_schema);
    }

    #[test]
    fn test_multi_constraint_plan() {
        let constraints = vec![
            MutationConstraint::default_size_limit(),
            MutationConstraint::default_growth_limit(),
            MutationConstraint::default_structure_preserved(),
        ];
        let engine = MutationEngine::new(mk_cap("llm.generate_diff")).with_constraints(constraints);
        let plan = engine.plan(&mk_variant(), "body".to_string(), None);

        assert_eq!(plan.input.constraints.len(), 3);
        let kinds: Vec<ConstraintKind> = plan.input.constraints.iter().map(|c| c.kind).collect();
        assert!(kinds.contains(&ConstraintKind::SizeLimit));
        assert!(kinds.contains(&ConstraintKind::GrowthLimit));
        assert!(kinds.contains(&ConstraintKind::StructurePreserved));
    }

    #[test]
    fn test_plan_root_has_no_parent() {
        let engine = MutationEngine::new(mk_cap("llm.generate_diff"));
        let skill_id = SkillId::new();
        let plan = engine.plan_root(skill_id.clone(), "# root seed".to_string(), None);

        assert!(plan.parent_variant_id.is_none());
        assert_eq!(plan.skill_id, skill_id);
        assert_eq!(plan.input.parent_skill_text, "# root seed");
        // Default constraints still apply.
        assert_eq!(plan.input.constraints.len(), 2);
    }

    #[test]
    fn test_plan_input_preserves_parent_skill_text() {
        let engine = MutationEngine::new(mk_cap("llm.generate_diff"));
        let parent = mk_variant();
        let body = "---\nname: foo\n---\n# Step 1\n- do thing\n";
        let plan = engine.plan(&parent, body.to_string(), None);

        assert_eq!(plan.input.parent_skill_text, body);
    }

    #[test]
    fn test_plan_respects_target_capability_override() {
        let cap_a = mk_cap("llm.generate_diff");
        let cap_b = mk_cap("llm.rewrite_full");
        let engine_a = MutationEngine::new(cap_a.clone());
        let engine_b = MutationEngine::new(cap_b.clone());

        let parent = mk_variant();
        let plan_a = engine_a.plan(&parent, "x".to_string(), None);
        let plan_b = engine_b.plan(&parent, "x".to_string(), None);

        assert_eq!(plan_a.target_capability.id.as_str(), "llm.generate_diff");
        assert_eq!(plan_b.target_capability.id.as_str(), "llm.rewrite_full");
    }

    #[test]
    fn test_mutation_output_kind_all_variants_serialize() {
        for kind in [
            MutationOutputKind::SkillTextDiff,
            MutationOutputKind::FullReplacement,
            MutationOutputKind::StructuredPatch,
        ] {
            let json = serde_json::to_string(&kind).expect("serialize variant");
            let deser: MutationOutputKind =
                serde_json::from_str(&json).expect("deserialize variant");
            assert_eq!(deser, kind, "roundtrip failed for {:?}", kind);
        }
    }

    #[test]
    fn test_constraint_kind_size_vs_growth_differentiate() {
        let size = MutationConstraint::default_size_limit();
        let growth = MutationConstraint::default_growth_limit();

        assert_ne!(size.kind, growth.kind);
        assert_eq!(size.kind, ConstraintKind::SizeLimit);
        assert_eq!(growth.kind, ConstraintKind::GrowthLimit);

        // Their serialized forms should also differ.
        let size_json = serde_json::to_string(&size).expect("ser size");
        let growth_json = serde_json::to_string(&growth).expect("ser growth");
        assert_ne!(size_json, growth_json);
        assert!(size_json.contains("size_limit"));
        assert!(growth_json.contains("growth_limit"));
    }

    #[test]
    fn test_with_output_kind_override() {
        let engine = MutationEngine::new(mk_cap("llm.generate_diff"))
            .with_output_kind(MutationOutputKind::FullReplacement);
        let plan = engine.plan(&mk_variant(), "body".to_string(), None);
        assert_eq!(
            plan.expected_output_schema,
            MutationOutputKind::FullReplacement
        );
    }

    #[test]
    fn test_mutation_result_serde_roundtrip() {
        let result = MutationResult {
            child_skill_text: "# new".to_string(),
            diff_artifact_id: Some(ArtifactId::new()),
            reasoning: Some("improved clarity".to_string()),
        };
        let json = serde_json::to_string(&result).expect("serialize");
        let deser: MutationResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.child_skill_text, "# new");
        assert_eq!(deser.reasoning.as_deref(), Some("improved clarity"));
        assert!(deser.diff_artifact_id.is_some());
    }
}
