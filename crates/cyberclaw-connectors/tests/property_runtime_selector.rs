//! Property-based tests for Runtime Selector invariants.
//!
//! # Invariants Verified
//!
//! 1. **Risk-to-Runtime Monotonicity**: Higher risk levels MUST select at least
//!    as secure a runtime as lower risk levels (Native < Process < Container).
//!
//! 2. **Strict Mode Safety Guarantee**: When strict_mode=true, Critical risk
//!    capabilities MUST ALWAYS select Container runtime regardless of strategy.
//!
//! 3. **Override Determinism**: A capability with an explicit override MUST
//!    always return that override regardless of the risk level.
//!
//! 4. **AlwaysContainer Completeness**: With AlwaysContainer strategy,
//!    ALL capabilities regardless of risk MUST use Container.
//!
//! 5. **Risk-Based Selection Completeness**: All 4 RiskLevel variants produce
//!    a valid RuntimeMode; no variant is left unhandled (exhaustiveness).

use cyberclaw_connectors::runtime::{
    RuntimeMode, RuntimeSelectionStrategy, RuntimeSelector, RuntimeSelectorConfig,
};
use cyberclaw_core::capability::{CapabilityEffect, RiskLevel};
use cyberclaw_core::manifests::{CapabilityContract, CapabilityTimeouts};
use proptest::prelude::*;
use std::collections::HashMap;

// ─── Arbitrary generators ─────────────────────────────────────────────────────

/// Produce an arbitrary RiskLevel
fn arb_risk_level() -> impl Strategy<Value = RiskLevel> {
    prop_oneof![
        Just(RiskLevel::Low),
        Just(RiskLevel::Medium),
        Just(RiskLevel::High),
        Just(RiskLevel::Critical),
    ]
}

/// Produce an arbitrary RuntimeSelectionStrategy
fn arb_strategy() -> impl Strategy<Value = RuntimeSelectionStrategy> {
    prop_oneof![
        Just(RuntimeSelectionStrategy::AlwaysNative),
        Just(RuntimeSelectionStrategy::AlwaysProcess),
        Just(RuntimeSelectionStrategy::AlwaysContainer),
        Just(RuntimeSelectionStrategy::RiskBased),
    ]
}

/// Build a CapabilityContract with the given id and risk level
fn make_capability(id: &str, risk: RiskLevel) -> CapabilityContract {
    CapabilityContract {
        id: id.to_string(),
        title: format!("Test Capability {}", id),
        description: None,
        input_schema: "{}".to_string(),
        output_schema: "{}".to_string(),
        risk,
        effects: vec![CapabilityEffect::Read],
        placement: None,
        timeouts: CapabilityTimeouts {
            request_ms: Some(5000),
        },
    }
}

/// Numeric security rank: Native=0, Process=1, Container=2
fn security_rank(mode: RuntimeMode) -> u8 {
    match mode {
        RuntimeMode::Native => 0,
        RuntimeMode::Process => 1,
        RuntimeMode::Container => 2,
    }
}

/// Numeric risk rank: Low=0, Medium=1, High=2, Critical=3
fn risk_rank(risk: &RiskLevel) -> u8 {
    match risk {
        RiskLevel::Low => 0,
        RiskLevel::Medium => 1,
        RiskLevel::High => 2,
        RiskLevel::Critical => 3,
    }
}

// ─── Property 1: Risk-Based Selection Monotonicity ───────────────────────────

proptest! {
    /// For RiskBased strategy, the selected runtime security level must be
    /// monotonically non-decreasing with respect to the risk level.
    ///
    /// Formally: risk_rank(r1) <= risk_rank(r2) =>
    ///           security_rank(select(r1)) <= security_rank(select(r2))
    #[test]
    fn property_risk_based_selection_is_monotonic(
        risk1 in arb_risk_level(),
        risk2 in arb_risk_level(),
    ) {
        let config = RuntimeSelectorConfig {
            default_strategy: RuntimeSelectionStrategy::RiskBased,
            capability_overrides: HashMap::new(),
            strict_mode: false, // Disable strict mode to test pure risk mapping
        };
        let selector = RuntimeSelector::new(config);

        let cap1 = make_capability("cap.a", risk1);
        let cap2 = make_capability("cap.b", risk2);

        let mode1 = selector.select_runtime(&cap1);
        let mode2 = selector.select_runtime(&cap2);

        // If risk1 <= risk2, then security_rank(mode1) <= security_rank(mode2)
        if risk_rank(&risk1) <= risk_rank(&risk2) {
            prop_assert!(
                security_rank(mode1) <= security_rank(mode2),
                "Risk-based selection must be monotonic: risk {:?}(rank {}) -> runtime {:?}(rank {}) \
                 must be <= risk {:?}(rank {}) -> runtime {:?}(rank {})",
                risk1, risk_rank(&risk1), mode1, security_rank(mode1),
                risk2, risk_rank(&risk2), mode2, security_rank(mode2),
            );
        }
    }
}

// ─── Property 2: Strict Mode Safety Guarantee ────────────────────────────────

proptest! {
    /// When strict_mode=true, Critical risk capabilities MUST ALWAYS use
    /// Container runtime regardless of the default strategy.
    ///
    /// This is the core security invariant: no strategy override can weaken
    /// the isolation requirement for Critical capabilities when strict mode
    /// is enabled.
    #[test]
    fn property_strict_mode_always_uses_container_for_critical(
        strategy in arb_strategy(),
    ) {
        let config = RuntimeSelectorConfig {
            default_strategy: strategy,
            capability_overrides: HashMap::new(),
            strict_mode: true,
        };
        let selector = RuntimeSelector::new(config);
        let critical_cap = make_capability("pkg.install", RiskLevel::Critical);

        let mode = selector.select_runtime(&critical_cap);

        prop_assert_eq!(
            mode,
            RuntimeMode::Container,
            "Strict mode MUST enforce Container for Critical capability regardless of strategy {:?}",
            strategy,
        );
    }
}

// ─── Property 3: Override Determinism ────────────────────────────────────────

proptest! {
    /// A capability with an explicit override in the config MUST always
    /// return that override regardless of the risk level or strategy.
    ///
    /// This tests that overrides take strict priority over all other logic.
    #[test]
    fn property_capability_override_is_deterministic(
        risk in arb_risk_level(),
        strategy in arb_strategy(),
        override_mode in prop_oneof![
            Just(RuntimeMode::Native),
            Just(RuntimeMode::Process),
            Just(RuntimeMode::Container),
        ],
    ) {
        let cap_id = "override.test.capability";
        let mut overrides = HashMap::new();
        overrides.insert(cap_id.to_string(), override_mode);

        let config = RuntimeSelectorConfig {
            default_strategy: strategy,
            capability_overrides: overrides,
            strict_mode: false, // Disable strict mode so override is respected even for Native
        };
        let selector = RuntimeSelector::new(config);
        let cap = make_capability(cap_id, risk);

        let selected = selector.select_runtime(&cap);

        prop_assert_eq!(
            selected,
            override_mode,
            "Override MUST be returned deterministically for cap '{}' with risk {:?} and strategy {:?}",
            cap_id, risk, strategy,
        );
    }
}

// ─── Property 4: AlwaysContainer Completeness ────────────────────────────────

proptest! {
    /// With AlwaysContainer strategy and strict_mode=false, ALL capabilities
    /// regardless of risk level MUST select Container.
    ///
    /// This ensures the strategy works as a global override when needed.
    #[test]
    fn property_always_container_strategy_selects_container_for_all_risks(
        risk in arb_risk_level(),
        cap_id in "[a-z]{3,8}\\.[a-z]{3,8}",
    ) {
        let config = RuntimeSelectorConfig {
            default_strategy: RuntimeSelectionStrategy::AlwaysContainer,
            capability_overrides: HashMap::new(),
            strict_mode: false,
        };
        let selector = RuntimeSelector::new(config);
        let cap = make_capability(&cap_id, risk);

        let mode = selector.select_runtime(&cap);

        prop_assert_eq!(
            mode,
            RuntimeMode::Container,
            "AlwaysContainer strategy MUST select Container for any risk level {:?}",
            risk,
        );
    }
}

// ─── Property 5: Risk-Based Exhaustiveness ───────────────────────────────────

proptest! {
    /// Every RiskLevel variant produces a valid RuntimeMode in RiskBased
    /// selection. No panic, no unhandled variant.
    ///
    /// This verifies the match arms in `RuntimeMode::from_risk_level` are
    /// exhaustive and produce results within the valid RuntimeMode set.
    #[test]
    fn property_risk_based_selection_is_total_function(
        risk in arb_risk_level(),
    ) {
        let config = RuntimeSelectorConfig {
            default_strategy: RuntimeSelectionStrategy::RiskBased,
            capability_overrides: HashMap::new(),
            strict_mode: false,
        };
        let selector = RuntimeSelector::new(config);
        let cap = make_capability("exhaustive.test", risk);

        // select_runtime must not panic for any RiskLevel
        let mode = selector.select_runtime(&cap);

        // The result must be one of the three valid modes
        let valid_modes = [RuntimeMode::Native, RuntimeMode::Process, RuntimeMode::Container];
        prop_assert!(
            valid_modes.contains(&mode),
            "select_runtime returned an invalid mode {:?} for risk {:?}",
            mode, risk,
        );
    }
}

// ─── Property 6: Non-Overridden Capability Is Not Affected By Overrides ──────

proptest! {
    /// Adding an override for capability A must not affect the selection
    /// for a different capability B (no cross-contamination of overrides).
    #[test]
    fn property_overrides_do_not_contaminate_other_capabilities(
        risk in arb_risk_level(),
        override_mode in prop_oneof![
            Just(RuntimeMode::Native),
            Just(RuntimeMode::Process),
            Just(RuntimeMode::Container),
        ],
    ) {
        let mut overrides = HashMap::new();
        overrides.insert("cap.with.override".to_string(), override_mode);

        let config = RuntimeSelectorConfig {
            default_strategy: RuntimeSelectionStrategy::RiskBased,
            capability_overrides: overrides,
            strict_mode: false,
        };
        let selector = RuntimeSelector::new(config);

        // Cap WITHOUT override: should get risk-based selection
        let cap_no_override = make_capability("cap.without.override", risk);
        let mode_no_override = selector.select_runtime(&cap_no_override);

        // Cap WITH override: should get override
        let cap_with_override = make_capability("cap.with.override", risk);
        let mode_with_override = selector.select_runtime(&cap_with_override);

        // The override should equal override_mode
        prop_assert_eq!(mode_with_override, override_mode);

        // The non-override cap should get risk-based result (not necessarily override_mode)
        let expected_no_override = RuntimeMode::from_risk_level(risk);
        prop_assert_eq!(
            mode_no_override,
            expected_no_override,
            "Cap without override should get risk-based selection for risk {:?}",
            risk,
        );
    }
}
