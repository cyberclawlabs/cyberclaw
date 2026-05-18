//! Property-based tests for Policy Consistency invariants.
//!
//! # Invariants Verified
//!
//! 1. **Determinism**: Identical inputs to evaluate_capability MUST always
//!    produce an identical decision (pure function - no randomness, no state).
//!
//! 2. **Deny Monotonicity**: Once a capability is denied, no subsequent
//!    evaluation with the same or higher risk level can produce Allow.
//!    Formally: if evaluate(ctx) = Deny, then for any ctx' with
//!    risk(ctx') >= risk(ctx), evaluate(ctx') != Allow.
//!
//! 3. **Risk Threshold Completeness**: For every RiskLevel, the DefaultPolicyEngine
//!    must produce a well-defined (non-panic) decision. No unhandled variant.
//!
//! 4. **Review Type Consistency**: ReviewRequired decisions for Critical risk
//!    MUST use Security review type; High risk MUST use Human review type.
//!
//! 5. **Allow Below Threshold**: For any risk level R that is <= threshold T,
//!    the decision MUST be Allow (not ReviewRequired or Deny).
//!
//! 6. **Review Above Threshold**: For any risk level R that is > threshold T,
//!    the decision MUST be ReviewRequired (the DefaultPolicyEngine never Denies).

use cyberclaw_core::capability::CapabilityRef;
use cyberclaw_core::capability::{CapabilityEffect, RiskLevel};
use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::{ActorId, CapabilityId, ConnectorId, ExecutionId};
use cyberclaw_governance::decision::ReviewType;
use cyberclaw_governance::engine::{DefaultPolicyEngine, EvaluationContext, PolicyEngine};
use proptest::prelude::*;

// ─── Helper builders ──────────────────────────────────────────────────────────

fn make_capability_ref(id: &str, risk: RiskLevel) -> CapabilityRef {
    CapabilityRef {
        id: CapabilityId::from_string(id.to_string()).unwrap_or_else(|_| panic!("invalid cap id")),
        connector_id: ConnectorId::from_string("test-connector".to_string())
            .unwrap_or_else(|_| panic!("invalid connector id")),
        risk,
        effects: vec![CapabilityEffect::Read],
        placement: None,
    }
}

fn make_actor(id: &str) -> ActorRef {
    ActorRef {
        id: ActorId::from_string(id.to_string()).unwrap_or_else(|_| ActorId::new()),
        actor_type: ActorType::Agent,
        tenant_id: None,
        home_node_id: None,
        display_name: format!("Test Actor {}", id),
    }
}

fn make_context(risk: RiskLevel) -> EvaluationContext {
    EvaluationContext {
        capability: make_capability_ref("test.capability", risk),
        actor: make_actor("test-actor"),
        execution_id: ExecutionId::new(),
        reason: Some("Property test evaluation".to_string()),
    }
}

// ─── Arbitrary generators ─────────────────────────────────────────────────────

fn arb_risk_level() -> impl Strategy<Value = RiskLevel> {
    prop_oneof![
        Just(RiskLevel::Low),
        Just(RiskLevel::Medium),
        Just(RiskLevel::High),
        Just(RiskLevel::Critical),
    ]
}

fn arb_threshold() -> impl Strategy<Value = RiskLevel> {
    arb_risk_level()
}

fn risk_rank(risk: &RiskLevel) -> u8 {
    match risk {
        RiskLevel::Low => 0,
        RiskLevel::Medium => 1,
        RiskLevel::High => 2,
        RiskLevel::Critical => 3,
    }
}

// ─── Property 1: Determinism ──────────────────────────────────────────────────

proptest! {
    /// Evaluating the same capability risk with the same threshold MUST always
    /// produce the same decision variant.
    ///
    /// This verifies that DefaultPolicyEngine is a pure function with no hidden
    /// state or randomness.
    #[test]
    fn property_evaluation_is_deterministic(
        risk in arb_risk_level(),
        threshold in arb_threshold(),
    ) {
        let engine = DefaultPolicyEngine::new(threshold);

        let rt = tokio::runtime::Runtime::new().unwrap();

        let decision_a = rt.block_on(async {
            let ctx = make_context(risk);
            engine.evaluate_capability(ctx).await.unwrap().decision
        });

        let decision_b = rt.block_on(async {
            let ctx = make_context(risk);
            engine.evaluate_capability(ctx).await.unwrap().decision
        });

        // Both decisions must be the same variant
        prop_assert_eq!(
            std::mem::discriminant(&decision_a),
            std::mem::discriminant(&decision_b),
            "Same risk {:?} with threshold {:?} must produce same decision variant: {:?} vs {:?}",
            risk, threshold, decision_a, decision_b,
        );
    }
}

// ─── Property 2: Allow Below Or At Threshold ─────────────────────────────────

proptest! {
    /// For any risk level R where risk_rank(R) <= risk_rank(threshold),
    /// the decision MUST be Allow.
    ///
    /// This is the core security contract: capabilities within acceptable risk
    /// MUST be permitted.
    #[test]
    fn property_allow_when_risk_at_or_below_threshold(
        threshold in arb_threshold(),
    ) {
        // Test every risk level that is <= threshold
        let all_risks = [RiskLevel::Low, RiskLevel::Medium, RiskLevel::High, RiskLevel::Critical];
        let engine = DefaultPolicyEngine::new(threshold);
        let rt = tokio::runtime::Runtime::new().unwrap();

        for risk in &all_risks {
            if risk_rank(risk) <= risk_rank(&threshold) {
                let ctx = make_context(*risk);
                let result = rt.block_on(engine.evaluate_capability(ctx)).unwrap();

                prop_assert!(
                    result.decision.is_allow(),
                    "Risk {:?} (rank {}) <= threshold {:?} (rank {}) must produce Allow, got {:?}",
                    risk, risk_rank(risk),
                    threshold, risk_rank(&threshold),
                    result.decision,
                );
            }
        }
    }
}

// ─── Property 3: Review Required Above Threshold ──────────────────────────────

proptest! {
    /// For any risk level R where risk_rank(R) > risk_rank(threshold),
    /// the decision MUST be ReviewRequired (never Allow, never Deny).
    ///
    /// DefaultPolicyEngine's contract: above threshold = review, not deny.
    #[test]
    fn property_review_required_when_risk_above_threshold(
        threshold in arb_threshold(),
    ) {
        let all_risks = [RiskLevel::Low, RiskLevel::Medium, RiskLevel::High, RiskLevel::Critical];
        let engine = DefaultPolicyEngine::new(threshold);
        let rt = tokio::runtime::Runtime::new().unwrap();

        for risk in &all_risks {
            if risk_rank(risk) > risk_rank(&threshold) {
                let ctx = make_context(*risk);
                let result = rt.block_on(engine.evaluate_capability(ctx)).unwrap();

                prop_assert!(
                    result.decision.is_review_required(),
                    "Risk {:?} (rank {}) > threshold {:?} (rank {}) must produce ReviewRequired, got {:?}",
                    risk, risk_rank(risk),
                    threshold, risk_rank(&threshold),
                    result.decision,
                );

                // DefaultPolicyEngine must NEVER deny - it only allows or requires review
                prop_assert!(
                    !result.decision.is_deny(),
                    "DefaultPolicyEngine must NEVER produce Deny decisions",
                );
            }
        }
    }
}

// ─── Property 4: Critical Risk Always Uses Security Review Type ───────────────

proptest! {
    /// When the decision is ReviewRequired for a Critical risk capability,
    /// the review_type MUST be Security (not Human or Approval).
    ///
    /// This ensures Critical capabilities always get the highest scrutiny.
    #[test]
    fn property_critical_risk_always_uses_security_review_type(
        threshold in prop_oneof![
            Just(RiskLevel::Low),
            Just(RiskLevel::Medium),
            Just(RiskLevel::High),
        ], // Critical with any threshold below Critical
    ) {
        // Use a threshold that is strictly below Critical to ensure review is required
        if risk_rank(&threshold) < risk_rank(&RiskLevel::Critical) {
            let engine = DefaultPolicyEngine::new(threshold);
            let rt = tokio::runtime::Runtime::new().unwrap();

            let ctx = make_context(RiskLevel::Critical);
            let result = rt.block_on(engine.evaluate_capability(ctx)).unwrap();

            prop_assert!(
                result.decision.is_review_required(),
                "Critical risk must require review",
            );

            prop_assert_eq!(
                result.decision.review_type(),
                Some(&ReviewType::Security),
                "Critical risk MUST use Security review type, got {:?}",
                result.decision.review_type(),
            );
        }
    }
}

// ─── Property 5: High Risk Always Uses Human Review Type ─────────────────────

proptest! {
    /// When the decision is ReviewRequired for a High risk capability,
    /// the review_type MUST be Human (not Security or Approval).
    ///
    /// High risk capabilities need human oversight but not full security review.
    #[test]
    fn property_high_risk_uses_human_review_type(
        threshold in prop_oneof![
            Just(RiskLevel::Low),
            Just(RiskLevel::Medium),
        ], // Only thresholds strictly below High
    ) {
        let engine = DefaultPolicyEngine::new(threshold);
        let rt = tokio::runtime::Runtime::new().unwrap();

        let ctx = make_context(RiskLevel::High);
        let result = rt.block_on(engine.evaluate_capability(ctx)).unwrap();

        prop_assert!(
            result.decision.is_review_required(),
            "High risk must require review",
        );

        prop_assert_eq!(
            result.decision.review_type(),
            Some(&ReviewType::Human),
            "High risk MUST use Human review type, got {:?}",
            result.decision.review_type(),
        );
    }
}

// ─── Property 6: Evaluated Risk Matches Input Risk ────────────────────────────

proptest! {
    /// The EvaluationResult.evaluated_risk MUST always match the input
    /// capability's risk level. The engine must not silently promote or
    /// demote risks.
    #[test]
    fn property_evaluated_risk_matches_input(
        risk in arb_risk_level(),
        threshold in arb_threshold(),
    ) {
        let engine = DefaultPolicyEngine::new(threshold);
        let rt = tokio::runtime::Runtime::new().unwrap();

        let ctx = make_context(risk);
        let result = rt.block_on(engine.evaluate_capability(ctx)).unwrap();

        let evaluated = result.evaluated_risk;
        prop_assert_eq!(
            evaluated,
            risk,
            "evaluated_risk must match input risk {:?}",
            risk,
        );
    }
}

// ─── Property 7: Decision Is Total Function (No Panics) ───────────────────────

proptest! {
    /// evaluate_capability must complete without panicking for ANY combination
    /// of risk level and threshold.
    ///
    /// This verifies that all match arms in the engine are exhaustive and
    /// the function is a total function over its domain.
    #[test]
    fn property_evaluation_is_total_no_panics(
        risk in arb_risk_level(),
        threshold in arb_threshold(),
    ) {
        let engine = DefaultPolicyEngine::new(threshold);
        let rt = tokio::runtime::Runtime::new().unwrap();

        // Must not panic
        let result = rt.block_on(engine.evaluate_capability(make_context(risk)));

        prop_assert!(
            result.is_ok(),
            "evaluate_capability must succeed for risk {:?}, got error: {:?}",
            risk, result.err(),
        );

        let eval = result.unwrap();

        // Decision must be one of the three valid variants
        let is_valid = eval.decision.is_allow()
            || eval.decision.is_deny()
            || eval.decision.is_review_required();

        prop_assert!(
            is_valid,
            "Decision must be Allow, Deny, or ReviewRequired",
        );
    }
}

// ─── Property 8: Context Info Is Always Non-Empty ─────────────────────────────

proptest! {
    /// context_info in EvaluationResult must always contain at least one
    /// entry (the capability identifier). The audit trail must never be empty.
    #[test]
    fn property_context_info_is_always_populated(
        risk in arb_risk_level(),
        threshold in arb_threshold(),
    ) {
        let engine = DefaultPolicyEngine::new(threshold);
        let rt = tokio::runtime::Runtime::new().unwrap();

        let result = rt.block_on(engine.evaluate_capability(make_context(risk))).unwrap();

        prop_assert!(
            !result.context_info.is_empty(),
            "context_info must never be empty - audit trail requires at least capability info",
        );
    }
}
