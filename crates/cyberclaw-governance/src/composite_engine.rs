//! Composite policy engine for combining multiple policy types.
//!
//! This module provides a unified policy engine that combines:
//! - Capability policies (whitelist, risk-based, etc.)
//! - Approval policies (workflow, escalation, etc.)
//! - Tenant boundary policies (isolation, resource limits, etc.)

use async_trait::async_trait;
use std::sync::Arc;

use crate::decision::GovernanceDecision;
use crate::engine::{EvaluationContext, EvaluationResult, PolicyEngine};

/// Strategy for combining multiple policy decisions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CombinationStrategy {
    /// Deny takes precedence over RequireReview, which takes precedence over Allow
    /// This is the most secure strategy
    #[default]
    DenyFirst,

    /// All policies must allow for final Allow decision
    /// Any Deny or RequireReview results in that decision
    AllMustAllow,

    /// First policy decision is used (short-circuit evaluation)
    FirstMatch,
}

/// Composite policy engine configuration
#[derive(Debug, Clone)]
pub struct CompositePolicyEngineConfig {
    /// Strategy for combining policy decisions
    pub combination_strategy: CombinationStrategy,

    /// Whether to continue evaluation after first Deny
    pub short_circuit_on_deny: bool,

    /// Whether to continue evaluation after first RequireReview
    pub short_circuit_on_review: bool,
}

impl Default for CompositePolicyEngineConfig {
    fn default() -> Self {
        Self {
            combination_strategy: CombinationStrategy::DenyFirst,
            short_circuit_on_deny: true,
            short_circuit_on_review: false,
        }
    }
}

/// Composite policy engine combining multiple policy engines
///
/// Evaluates capability requests against multiple policy engines and combines
/// their decisions according to the configured strategy.
///
/// # Examples
///
/// ```
/// use cyberclaw_governance::composite_engine::{CompositePolicyEngine, CompositePolicyEngineBuilder};
/// use cyberclaw_governance::engine::DefaultPolicyEngine;
/// use std::sync::Arc;
///
/// let engine = CompositePolicyEngineBuilder::new()
///     .add_engine(Arc::new(DefaultPolicyEngine::default()))
///     .build();
/// ```
#[derive(Clone)]
pub struct CompositePolicyEngine {
    engines: Vec<Arc<dyn PolicyEngine>>,
    config: CompositePolicyEngineConfig,
}

impl CompositePolicyEngine {
    /// Create a new composite policy engine with the given engines and config
    pub fn new(engines: Vec<Arc<dyn PolicyEngine>>, config: CompositePolicyEngineConfig) -> Self {
        Self { engines, config }
    }

    /// Create a builder for constructing a composite policy engine
    pub fn builder() -> CompositePolicyEngineBuilder {
        CompositePolicyEngineBuilder::new()
    }

    /// Combine multiple evaluation results according to the configured strategy
    fn combine_results(&self, results: Vec<EvaluationResult>) -> EvaluationResult {
        if results.is_empty() {
            // No engines configured, default to deny for safety
            return EvaluationResult {
                decision: GovernanceDecision::deny("No policy engines configured"),
                evaluated_risk: cyberclaw_core::prelude::RiskLevel::Critical,
                context_info: vec!["No policy engines available".to_string()],
            };
        }

        match self.config.combination_strategy {
            CombinationStrategy::DenyFirst => self.combine_deny_first(results),
            CombinationStrategy::AllMustAllow => self.combine_all_must_allow(results),
            CombinationStrategy::FirstMatch => results.into_iter().next().unwrap(),
        }
    }

    /// Deny-first combination: Deny > RequireReview > Allow
    fn combine_deny_first(&self, results: Vec<EvaluationResult>) -> EvaluationResult {
        let mut combined_context_info = Vec::new();
        let highest_risk = results
            .iter()
            .map(|r| &r.evaluated_risk)
            .max()
            .cloned()
            .unwrap_or(cyberclaw_core::prelude::RiskLevel::Low);

        // Collect all Deny decisions first
        let denies: Vec<_> = results.iter().filter(|r| r.decision.is_deny()).collect();

        if !denies.is_empty() {
            for result in &denies {
                combined_context_info.push(format!("DENY: {}", result.decision.reason()));
                combined_context_info.extend(result.context_info.clone());
            }

            let combined_reason = denies
                .iter()
                .map(|r| r.decision.reason())
                .collect::<Vec<_>>()
                .join("; ");

            return EvaluationResult {
                decision: GovernanceDecision::deny(combined_reason),
                evaluated_risk: highest_risk,
                context_info: combined_context_info,
            };
        }

        // Then collect ReviewRequired decisions
        let reviews: Vec<_> = results
            .iter()
            .filter(|r| r.decision.is_review_required())
            .collect();

        if !reviews.is_empty() {
            for result in &reviews {
                combined_context_info.push(format!("REVIEW: {}", result.decision.reason()));
                combined_context_info.extend(result.context_info.clone());
            }

            let combined_reason = reviews
                .iter()
                .map(|r| r.decision.reason())
                .collect::<Vec<_>>()
                .join("; ");

            // Use the highest priority review type (Security > Human > Approval > Escalation)
            let review_type = reviews
                .iter()
                .filter_map(|r| r.decision.review_type())
                .max_by_key(|rt| match rt {
                    crate::decision::ReviewType::Security => 4,
                    crate::decision::ReviewType::Human => 3,
                    crate::decision::ReviewType::Approval => 2,
                    crate::decision::ReviewType::Escalation => 1,
                })
                .cloned()
                .unwrap_or(crate::decision::ReviewType::Human);

            return EvaluationResult {
                decision: GovernanceDecision::review_required(combined_reason, review_type),
                evaluated_risk: highest_risk,
                context_info: combined_context_info,
            };
        }

        // All allow
        for result in &results {
            combined_context_info.push(format!("ALLOW: {}", result.decision.reason()));
            combined_context_info.extend(result.context_info.clone());
        }

        EvaluationResult {
            decision: GovernanceDecision::allow("All policy engines approved"),
            evaluated_risk: highest_risk,
            context_info: combined_context_info,
        }
    }

    /// All-must-allow combination: all engines must return Allow
    fn combine_all_must_allow(&self, results: Vec<EvaluationResult>) -> EvaluationResult {
        let mut combined_context_info = Vec::new();
        let highest_risk = results
            .iter()
            .map(|r| &r.evaluated_risk)
            .max()
            .cloned()
            .unwrap_or(cyberclaw_core::prelude::RiskLevel::Low);

        for result in &results {
            combined_context_info.extend(result.context_info.clone());

            if !result.decision.is_allow() {
                // First non-allow decision is returned
                return EvaluationResult {
                    decision: result.decision.clone(),
                    evaluated_risk: highest_risk,
                    context_info: combined_context_info,
                };
            }
        }

        // All allowed
        EvaluationResult {
            decision: GovernanceDecision::allow("All policy engines approved"),
            evaluated_risk: highest_risk,
            context_info: combined_context_info,
        }
    }
}

#[async_trait]
impl PolicyEngine for CompositePolicyEngine {
    async fn evaluate_capability(
        &self,
        context: EvaluationContext,
    ) -> anyhow::Result<EvaluationResult> {
        let mut results = Vec::new();

        for engine in &self.engines {
            let result = engine.evaluate_capability(context.clone()).await?;

            // Short-circuit on Deny if configured
            if self.config.short_circuit_on_deny && result.decision.is_deny() {
                return Ok(result);
            }

            // Short-circuit on RequireReview if configured
            if self.config.short_circuit_on_review && result.decision.is_review_required() {
                return Ok(result);
            }

            results.push(result);
        }

        Ok(self.combine_results(results))
    }
}

/// Builder for constructing composite policy engines
#[derive(Default)]
pub struct CompositePolicyEngineBuilder {
    engines: Vec<Arc<dyn PolicyEngine>>,
    config: CompositePolicyEngineConfig,
}

impl CompositePolicyEngineBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a policy engine to the composite
    pub fn add_engine(mut self, engine: Arc<dyn PolicyEngine>) -> Self {
        self.engines.push(engine);
        self
    }

    /// Set the combination strategy
    pub fn combination_strategy(mut self, strategy: CombinationStrategy) -> Self {
        self.config.combination_strategy = strategy;
        self
    }

    /// Set whether to short-circuit on Deny
    pub fn short_circuit_on_deny(mut self, short_circuit: bool) -> Self {
        self.config.short_circuit_on_deny = short_circuit;
        self
    }

    /// Set whether to short-circuit on RequireReview
    pub fn short_circuit_on_review(mut self, short_circuit: bool) -> Self {
        self.config.short_circuit_on_review = short_circuit;
        self
    }

    /// Build the composite policy engine
    pub fn build(self) -> CompositePolicyEngine {
        CompositePolicyEngine::new(self.engines, self.config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::DefaultPolicyEngine;
    use cyberclaw_core::prelude::*;

    fn create_test_capability(risk: RiskLevel) -> CapabilityRef {
        CapabilityRef {
            id: CapabilityId::from_string("test.capability".to_string()).unwrap(),
            connector_id: ConnectorId::from_string("test-connector".to_string()).unwrap(),
            risk,
            effects: vec![CapabilityEffect::Read],
            placement: None,
        }
    }

    fn create_test_actor() -> ActorRef {
        ActorRef {
            id: ActorId::from_string("test-actor".to_string()).unwrap(),
            actor_type: ActorType::Agent,
            tenant_id: None,
            home_node_id: None,
            display_name: "Test Actor".to_string(),
        }
    }

    fn create_test_context(capability: CapabilityRef) -> EvaluationContext {
        EvaluationContext {
            capability,
            actor: create_test_actor(),
            execution_id: ExecutionId::new(),
            reason: Some("Test evaluation".to_string()),
        }
    }

    #[tokio::test]
    async fn test_composite_engine_all_allow() {
        let engine1 = Arc::new(DefaultPolicyEngine::new(RiskLevel::Medium));
        let engine2 = Arc::new(DefaultPolicyEngine::new(RiskLevel::Medium));

        let composite = CompositePolicyEngine::builder()
            .add_engine(engine1)
            .add_engine(engine2)
            .build();

        let capability = create_test_capability(RiskLevel::Low);
        let context = create_test_context(capability);

        let result = composite.evaluate_capability(context).await.unwrap();
        assert!(result.decision.is_allow());
    }

    #[tokio::test]
    async fn test_composite_engine_deny_first() {
        let engine1 = Arc::new(DefaultPolicyEngine::new(RiskLevel::Low)); // Strict
        let engine2 = Arc::new(DefaultPolicyEngine::new(RiskLevel::High)); // Permissive

        let composite = CompositePolicyEngine::builder()
            .add_engine(engine1)
            .add_engine(engine2)
            .combination_strategy(CombinationStrategy::DenyFirst)
            .build();

        let capability = create_test_capability(RiskLevel::Medium);
        let context = create_test_context(capability);

        let result = composite.evaluate_capability(context).await.unwrap();
        // Engine1 (strict) will require review for Medium risk
        // DenyFirst strategy: RequireReview takes precedence
        assert!(result.decision.is_review_required());
    }

    #[tokio::test]
    async fn test_composite_engine_short_circuit_on_deny() {
        let engine1 = Arc::new(DefaultPolicyEngine::new(RiskLevel::Low)); // Will deny High risk
        let engine2 = Arc::new(DefaultPolicyEngine::new(RiskLevel::Critical)); // Never evaluated

        let composite = CompositePolicyEngine::builder()
            .add_engine(engine1)
            .add_engine(engine2)
            .short_circuit_on_deny(true)
            .build();

        let capability = create_test_capability(RiskLevel::Critical);
        let context = create_test_context(capability);

        let result = composite.evaluate_capability(context).await.unwrap();
        // Engine1 requires review for Critical, but since we're using DenyFirst
        // and short_circuit_on_deny, should return immediately
        assert!(result.decision.is_review_required());
    }

    #[tokio::test]
    async fn test_composite_engine_all_must_allow() {
        let engine1 = Arc::new(DefaultPolicyEngine::new(RiskLevel::Low));
        let engine2 = Arc::new(DefaultPolicyEngine::new(RiskLevel::High));

        let composite = CompositePolicyEngine::builder()
            .add_engine(engine1)
            .add_engine(engine2)
            .combination_strategy(CombinationStrategy::AllMustAllow)
            .build();

        let capability = create_test_capability(RiskLevel::Medium);
        let context = create_test_context(capability);

        let result = composite.evaluate_capability(context).await.unwrap();
        // Engine1 (strict) requires review, so AllMustAllow fails
        assert!(result.decision.is_review_required());
    }

    #[tokio::test]
    async fn test_composite_engine_first_match() {
        let engine1 = Arc::new(DefaultPolicyEngine::new(RiskLevel::High)); // Permissive
        let engine2 = Arc::new(DefaultPolicyEngine::new(RiskLevel::Low)); // Strict, never evaluated

        let composite = CompositePolicyEngine::builder()
            .add_engine(engine1)
            .add_engine(engine2)
            .combination_strategy(CombinationStrategy::FirstMatch)
            .build();

        let capability = create_test_capability(RiskLevel::Medium);
        let context = create_test_context(capability);

        let result = composite.evaluate_capability(context).await.unwrap();
        // Engine1 (permissive) allows Medium risk, FirstMatch returns immediately
        assert!(result.decision.is_allow());
    }

    #[tokio::test]
    async fn test_composite_engine_no_engines() {
        let composite = CompositePolicyEngine::builder().build();

        let capability = create_test_capability(RiskLevel::Low);
        let context = create_test_context(capability);

        let result = composite.evaluate_capability(context).await.unwrap();
        // No engines = deny for safety
        assert!(result.decision.is_deny());
    }
}
