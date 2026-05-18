//! Policy engine for capability evaluation and governance decisions.
//!
//! This module provides the core policy evaluation framework for determining
//! whether capabilities should be allowed to execute, denied, or require human review.

use async_trait::async_trait;
use cyberclaw_core::prelude::*;

use crate::decision::{GovernanceDecision, ReviewType};

// Re-export ActorType for test usage
#[cfg(test)]
use cyberclaw_core::identity::ActorType;

/// Context for policy evaluation.
///
/// Contains all information needed to make an informed governance decision
/// about a capability execution request.
#[derive(Debug, Clone)]
pub struct EvaluationContext {
    /// The capability being evaluated
    pub capability: CapabilityRef,

    /// The actor requesting capability execution
    pub actor: ActorRef,

    /// The execution ID this capability belongs to
    pub execution_id: ExecutionId,

    /// Optional reason provided for the capability request
    pub reason: Option<String>,
}

/// Result of policy evaluation with decision and metadata.
#[derive(Debug, Clone)]
pub struct EvaluationResult {
    /// The governance decision
    pub decision: GovernanceDecision,

    /// Risk level that influenced the decision
    pub evaluated_risk: RiskLevel,

    /// Additional context information
    pub context_info: Vec<String>,
}

/// Policy engine trait for capability evaluation.
///
/// Implementations of this trait define the governance rules and logic
/// for evaluating whether capabilities should be allowed to execute.
#[async_trait]
pub trait PolicyEngine: Send + Sync {
    /// Evaluate a capability request and return a governance decision.
    ///
    /// # Arguments
    ///
    /// * `context` - Evaluation context containing capability and actor information
    ///
    /// # Returns
    ///
    /// Returns an `EvaluationResult` containing the decision and rationale.
    ///
    /// # Errors
    ///
    /// Returns an error if evaluation fails due to invalid input or internal errors.
    async fn evaluate_capability(
        &self,
        context: EvaluationContext,
    ) -> anyhow::Result<EvaluationResult>;

    /// Sprint 29: read-only snapshot of the engine's declarative rule set, if any.
    ///
    /// Returns `None` for engines that don't expose declarative rules (e.g.
    /// [`DefaultPolicyEngine`] which is purely risk-based, [`NoopPolicyEngine`]).
    /// [`RuleBasedPolicyEngine`] overrides this with a `Some(RuleSet)` clone
    /// taken under the read lock — admin UIs can render or audit the live
    /// rules without needing trait-object downcasting.
    fn rules_snapshot(&self) -> Option<crate::rules::RuleSet> {
        None
    }

    /// Sprint 44 follow-up (commit d39bdf5): evaluate a request while
    /// respecting an explicit `bound_skill_allowed_capabilities` allow-list
    /// derived from the SKILL.md frontmatter of every skill currently bound
    /// to the calling agent.
    ///
    /// Behaviour:
    /// 1. If `bound_skill_allowed_capabilities` contains the requested
    ///    capability id, short-circuit to a Low-risk Allow without
    ///    consulting the underlying engine. The SKILL.md author has
    ///    already declared this trust at install time.
    /// 2. Otherwise (empty list or capability not present) delegate to the
    ///    normal [`PolicyEngine::evaluate_capability`] path — the original
    ///    risk- or rule-based gate is preserved.
    ///
    /// `None` for `bound_skill_allowed_capabilities` is equivalent to an
    /// empty slice. This default impl works for every PolicyEngine in the
    /// crate; implementations only need to override if they want a
    /// different short-circuit policy.
    async fn evaluate_capability_with_skill_bindings(
        &self,
        context: EvaluationContext,
        bound_skill_allowed_capabilities: Option<&[String]>,
    ) -> anyhow::Result<EvaluationResult> {
        if let Some(allowed) = bound_skill_allowed_capabilities {
            let cap_id = context.capability.id.as_ref();
            if allowed.iter().any(|c| c == cap_id) {
                return Ok(EvaluationResult {
                    decision: GovernanceDecision::allow(format!(
                        "skill-binding allow-list authorised {cap_id}"
                    )),
                    evaluated_risk: RiskLevel::Low,
                    context_info: vec![format!(
                        "bound_skill_allow_list: {} capabilities granted by skill bindings",
                        allowed.len()
                    )],
                });
            }
        }
        self.evaluate_capability(context).await
    }
}

/// Default policy engine implementation.
///
/// Implements risk-based evaluation:
/// - Low risk → Allow
/// - Medium/High/Critical risk → ReviewRequired
///
/// This implementation can be configured with custom risk thresholds.
#[derive(Debug, Clone)]
pub struct DefaultPolicyEngine {
    /// Risk threshold above which review is required (default: Low)
    pub review_threshold: RiskLevel,
}

impl Default for DefaultPolicyEngine {
    fn default() -> Self {
        Self {
            review_threshold: RiskLevel::Low,
        }
    }
}

impl DefaultPolicyEngine {
    /// Create a new default policy engine with custom review threshold.
    ///
    /// # Arguments
    ///
    /// * `review_threshold` - Risk level at or above which review is required
    ///
    /// # Examples
    ///
    /// ```
    /// use cyberclaw_governance::engine::DefaultPolicyEngine;
    /// use cyberclaw_core::prelude::RiskLevel;
    ///
    /// let engine = DefaultPolicyEngine::new(RiskLevel::Medium);
    /// ```
    pub fn new(review_threshold: RiskLevel) -> Self {
        Self { review_threshold }
    }
}

#[async_trait]
impl PolicyEngine for DefaultPolicyEngine {
    async fn evaluate_capability(
        &self,
        context: EvaluationContext,
    ) -> anyhow::Result<EvaluationResult> {
        let capability_risk = context.capability.risk;
        let mut context_info = Vec::new();

        // Add context information
        context_info.push(format!(
            "Capability: {} from connector {}",
            context.capability.id, context.capability.connector_id
        ));

        context_info.push(format!(
            "Requested by actor: {} ({})",
            context.actor.id, context.actor.display_name
        ));

        if !context.capability.effects.is_empty() {
            context_info.push(format!(
                "Capability effects: {:?}",
                context.capability.effects
            ));
        }

        if let Some(reason) = &context.reason {
            context_info.push(format!("Request reason: {}", reason));
        }

        // Risk-based evaluation logic
        let decision = if capability_risk > self.review_threshold {
            let reason = format!(
                "Capability risk level {:?} exceeds threshold {:?}",
                capability_risk, self.review_threshold
            );

            // Determine review type based on risk level
            let review_type = match capability_risk {
                RiskLevel::Critical => ReviewType::Security,
                RiskLevel::High => ReviewType::Human,
                _ => ReviewType::Approval,
            };

            GovernanceDecision::review_required(reason, review_type)
        } else {
            let reason = format!(
                "Capability risk level {:?} is within acceptable threshold",
                capability_risk
            );
            GovernanceDecision::allow(reason)
        };

        Ok(EvaluationResult {
            decision,
            evaluated_risk: capability_risk,
            context_info,
        })
    }
}

// ---------------------------------------------------------------------------
// NoopPolicyEngine
// ---------------------------------------------------------------------------

/// Test/Noop implementation of [`PolicyEngine`] that always returns Allow.
///
/// Useful for unit tests where policy evaluation is not the focus, or for
/// cases where the caller wants to bypass governance entirely (e.g.,
/// system-internal capability dispatches).
#[derive(Debug, Default, Clone)]
pub struct NoopPolicyEngine;

#[async_trait]
impl PolicyEngine for NoopPolicyEngine {
    async fn evaluate_capability(
        &self,
        context: EvaluationContext,
    ) -> anyhow::Result<EvaluationResult> {
        Ok(EvaluationResult {
            decision: GovernanceDecision::allow("noop policy engine"),
            evaluated_risk: context.capability.risk,
            context_info: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// RuleBasedPolicyEngine
// ---------------------------------------------------------------------------

/// Rule-based policy engine that evaluates declarative agent×capability rules.
///
/// Sprint 27: evaluates a [`crate::rules::RuleSet`] first (Deny wins, then Allow),
/// falling back to [`DefaultPolicyEngine`] risk-based logic when no rule matches.
///
/// Sprint 28: rules live behind `Arc<RwLock<RuleSet>>` so a background watcher
/// (started via [`Self::start_hot_reload`]) can swap them in without restarting
/// the server.
#[derive(Debug, Clone)]
pub struct RuleBasedPolicyEngine {
    pub rules: std::sync::Arc<std::sync::RwLock<crate::rules::RuleSet>>,
    pub fallback: DefaultPolicyEngine,
}

impl RuleBasedPolicyEngine {
    /// Create a new rule-based engine with the given rule set and fallback engine.
    pub fn new(rules: crate::rules::RuleSet, fallback: DefaultPolicyEngine) -> Self {
        Self {
            rules: std::sync::Arc::new(std::sync::RwLock::new(rules)),
            fallback,
        }
    }

    /// Sprint 28: spawn a background tokio task that re-reads `path` whenever
    /// its mtime changes (polled every `interval`). Replaces the rule set
    /// in-place via the shared `Arc<RwLock<_>>`. The task uses a `Weak`
    /// reference so it auto-exits when the engine is dropped — no shutdown
    /// channel needed. Returns the [`tokio::task::JoinHandle`] so the caller
    /// can `abort()` it if it wants to retire the watcher early.
    pub fn start_hot_reload(
        &self,
        path: std::path::PathBuf,
        interval: std::time::Duration,
    ) -> tokio::task::JoinHandle<()> {
        let rules_weak = std::sync::Arc::downgrade(&self.rules);
        tokio::spawn(async move {
            let mut last_mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            let mut ticker = tokio::time::interval(interval);
            // First tick fires immediately; skip it so we don't reload on the same mtime.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let Some(rules) = rules_weak.upgrade() else {
                    tracing::debug!(
                        path = %path.display(),
                        "S28: rule set hot-reload watcher exiting (engine dropped)"
                    );
                    return;
                };
                let current_mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                if current_mtime != last_mtime {
                    let new_rules = crate::rules::RuleSet::from_yaml_path(&path);
                    if let Ok(mut guard) = rules.write() {
                        let new_count = new_rules.rules.len();
                        *guard = new_rules;
                        tracing::info!(
                            path = %path.display(),
                            rule_count = new_count,
                            "S28: rule set hot-reloaded"
                        );
                    }
                    last_mtime = current_mtime;
                }
            }
        })
    }
}

#[async_trait]
impl PolicyEngine for RuleBasedPolicyEngine {
    async fn evaluate_capability(
        &self,
        context: EvaluationContext,
    ) -> anyhow::Result<EvaluationResult> {
        let agent_id = context.actor.id.as_ref();
        let capability_id = context.capability.id.as_ref();

        // Acquire the read lock briefly, clone the matched rule out, drop the lock
        // before the (potentially) async fallback path. Cloning a Rule is cheap —
        // 3 short Strings + an i32 + an enum.
        let matched: Option<crate::rules::Rule> = {
            let guard = self
                .rules
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.evaluate(agent_id, capability_id).cloned()
        };

        if let Some(rule) = matched {
            let reason = rule
                .reason
                .clone()
                .unwrap_or_else(|| format!("matched rule: kind={:?}", rule.kind));

            let decision = match rule.kind {
                crate::rules::RuleKind::Deny => crate::decision::GovernanceDecision::deny(reason),
                crate::rules::RuleKind::Allow => crate::decision::GovernanceDecision::allow(reason),
            };

            return Ok(EvaluationResult {
                decision,
                evaluated_risk: context.capability.risk,
                context_info: vec!["rule_based: matched".to_string()],
            });
        }

        // No rule matched → fallback to risk-based default
        self.fallback.evaluate_capability(context).await
    }

    fn rules_snapshot(&self) -> Option<crate::rules::RuleSet> {
        // Take a read-lock and clone — RuleSet is small (typically <100 rules),
        // and the snapshot is for read-only display, not the eval hot path.
        let guard = self
            .rules
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Some(guard.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    async fn test_low_risk_capability_allowed() {
        let engine = DefaultPolicyEngine::default();
        let capability = create_test_capability(RiskLevel::Low);
        let context = create_test_context(capability);

        let result = engine.evaluate_capability(context).await.unwrap();

        assert!(result.decision.is_allow());
        assert_eq!(result.evaluated_risk, RiskLevel::Low);
        assert!(!result.context_info.is_empty());
    }

    #[tokio::test]
    async fn test_medium_risk_requires_review() {
        let engine = DefaultPolicyEngine::default();
        let capability = create_test_capability(RiskLevel::Medium);
        let context = create_test_context(capability);

        let result = engine.evaluate_capability(context).await.unwrap();

        assert!(result.decision.is_review_required());
        assert_eq!(result.evaluated_risk, RiskLevel::Medium);
        assert_eq!(result.decision.review_type(), Some(&ReviewType::Approval));
    }

    #[tokio::test]
    async fn test_high_risk_requires_review() {
        let engine = DefaultPolicyEngine::default();
        let capability = create_test_capability(RiskLevel::High);
        let context = create_test_context(capability);

        let result = engine.evaluate_capability(context).await.unwrap();

        assert!(result.decision.is_review_required());
        assert_eq!(result.evaluated_risk, RiskLevel::High);
        assert_eq!(result.decision.review_type(), Some(&ReviewType::Human));
    }

    #[tokio::test]
    async fn test_critical_risk_requires_review() {
        let engine = DefaultPolicyEngine::default();
        let capability = create_test_capability(RiskLevel::Critical);
        let context = create_test_context(capability);

        let result = engine.evaluate_capability(context).await.unwrap();

        assert!(result.decision.is_review_required());
        assert_eq!(result.evaluated_risk, RiskLevel::Critical);
        assert_eq!(result.decision.review_type(), Some(&ReviewType::Security));
    }

    #[tokio::test]
    async fn test_custom_threshold() {
        // Set threshold to Medium, so Medium risk should be allowed
        let engine = DefaultPolicyEngine::new(RiskLevel::Medium);
        let capability = create_test_capability(RiskLevel::Medium);
        let context = create_test_context(capability);

        let result = engine.evaluate_capability(context).await.unwrap();

        assert!(result.decision.is_allow());
        assert_eq!(result.evaluated_risk, RiskLevel::Medium);
    }

    /// F-2 (commit d39bdf5 follow-up): when the requested capability id is
    /// in the bound-skill allow-list, the engine short-circuits to Allow
    /// even if the underlying default policy would have demanded review.
    #[tokio::test]
    async fn skill_binding_allow_list_short_circuits_to_allow() {
        let engine = DefaultPolicyEngine::default();
        // High risk would normally trigger ReviewType::Human under the
        // default threshold (Low). The allow-list must override that.
        let mut capability = create_test_capability(RiskLevel::High);
        capability.id =
            CapabilityId::from_string("connector:browser:navigate".to_string()).unwrap();
        let context = create_test_context(capability);

        let allow_list = vec![
            "connector:fs:read".to_string(),
            "connector:browser:navigate".to_string(),
        ];
        let result = engine
            .evaluate_capability_with_skill_bindings(context, Some(&allow_list))
            .await
            .unwrap();

        assert!(
            result.decision.is_allow(),
            "matching cap id in allow-list must be Allow"
        );
        assert_eq!(result.evaluated_risk, RiskLevel::Low);
    }

    /// F-2: a capability that is NOT in the allow-list falls through to
    /// the original gate; high-risk caps still require review.
    #[tokio::test]
    async fn skill_binding_allow_list_falls_through_when_not_listed() {
        let engine = DefaultPolicyEngine::default();
        let mut capability = create_test_capability(RiskLevel::High);
        capability.id = CapabilityId::from_string("connector:net:exfil".to_string()).unwrap();
        let context = create_test_context(capability);

        let allow_list = vec!["connector:fs:read".to_string()];
        let result = engine
            .evaluate_capability_with_skill_bindings(context, Some(&allow_list))
            .await
            .unwrap();

        assert!(
            result.decision.is_review_required(),
            "non-allow-listed High cap must still require review"
        );
    }

    /// F-2: `None` for the allow-list parameter is equivalent to no
    /// short-circuit — original gate runs unchanged.
    #[tokio::test]
    async fn skill_binding_none_preserves_original_gate() {
        let engine = DefaultPolicyEngine::default();
        let capability = create_test_capability(RiskLevel::Medium);
        let context = create_test_context(capability);

        let result = engine
            .evaluate_capability_with_skill_bindings(context, None)
            .await
            .unwrap();

        assert!(
            result.decision.is_review_required(),
            "None allow-list must not short-circuit; original Medium-risk gate stands"
        );
    }

    #[tokio::test]
    async fn test_evaluation_includes_context() {
        let engine = DefaultPolicyEngine::default();
        let capability = create_test_capability(RiskLevel::Low);
        let context = create_test_context(capability);

        let result = engine.evaluate_capability(context).await.unwrap();

        // Verify context_info includes expected information
        let info_str = result.context_info.join(" ");
        assert!(info_str.contains("test.capability"));
        assert!(info_str.contains("test-connector"));
        assert!(info_str.contains("test-actor"));
        assert!(info_str.contains("Test evaluation"));
    }

    #[tokio::test]
    async fn hot_reload_swaps_rules_when_yaml_file_changes() {
        // Sprint 28: write yaml v1, build engine + watcher, evaluate (deny path),
        // overwrite with yaml v2 (now allows the same agent), wait for the watcher
        // to pick up the new mtime, evaluate again (allow path).
        use std::time::Duration;
        use tokio::time::sleep;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rules.yaml");

        // v1: deny bad-agent on any capability.
        std::fs::write(
            &path,
            r#"
rules:
  - kind: deny
    agent_id: bad-agent
    capability_id: null
    reason: "v1-deny"
"#,
        )
        .unwrap();

        let rules = crate::rules::RuleSet::from_yaml_path(&path);
        let engine = RuleBasedPolicyEngine::new(rules, DefaultPolicyEngine::default());

        // Poll every 50ms — fast enough for the test, well above filesystem mtime
        // resolution (1s on some filesystems but APFS is finer).
        let watcher = engine.start_hot_reload(path.clone(), Duration::from_millis(50));

        // First evaluate uses v1 — deny.
        let ctx = {
            let cap = create_test_capability(RiskLevel::Low);
            let mut actor = create_test_actor();
            actor.id = ActorId::from_string("bad-agent".to_string()).unwrap();
            EvaluationContext {
                capability: cap,
                actor,
                execution_id: ExecutionId::new(),
                reason: None,
            }
        };
        let r1 = engine.evaluate_capability(ctx.clone()).await.unwrap();
        assert!(r1.decision.is_deny(), "v1 should deny bad-agent");

        // Wait long enough that the next mtime tick is distinguishable.
        // APFS mtime granularity is 1ns but std::fs::metadata may round to ms;
        // sleep 1.1s to be safe across platforms (Linux ext4 is 1ns too).
        sleep(Duration::from_millis(1100)).await;

        // v2: allow bad-agent (overwrites v1).
        std::fs::write(
            &path,
            r#"
rules:
  - kind: allow
    agent_id: bad-agent
    capability_id: null
    reason: "v2-allow"
"#,
        )
        .unwrap();

        // Wait for watcher to observe new mtime + re-read + swap. 50ms ticks ×
        // some slack: 500ms is plenty.
        sleep(Duration::from_millis(500)).await;

        let r2 = engine.evaluate_capability(ctx).await.unwrap();
        assert!(
            r2.decision.is_allow(),
            "v2 should allow bad-agent after hot-reload"
        );

        watcher.abort();
    }

    #[tokio::test]
    async fn hot_reload_watcher_exits_when_engine_dropped() {
        // Verify the Weak-reference shutdown pattern — drop the engine, watcher
        // should exit on its next tick.
        use std::time::Duration;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rules.yaml");
        std::fs::write(&path, "rules: []\n").unwrap();

        let rules = crate::rules::RuleSet::from_yaml_path(&path);
        let engine = RuleBasedPolicyEngine::new(rules, DefaultPolicyEngine::default());
        let watcher = engine.start_hot_reload(path.clone(), Duration::from_millis(20));

        drop(engine);

        // Watcher's next tick should observe Weak::upgrade() == None and return.
        let result = tokio::time::timeout(Duration::from_secs(2), watcher).await;
        assert!(
            result.is_ok(),
            "watcher should self-terminate within 2s after engine drop, got: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn rule_based_engine_uses_rules_then_falls_back_to_default() {
        use crate::rules::RuleSet;

        let yaml = r#"
rules:
  - kind: deny
    agent_id: bad-agent
    capability_id: null
    reason: "blocked"
"#;
        let rules: RuleSet = serde_yaml::from_str(yaml).unwrap();
        let engine =
            crate::engine::RuleBasedPolicyEngine::new(rules, DefaultPolicyEngine::default());

        // Deny rule path: bad-agent should be denied for any capability
        let ctx = {
            let mut cap = create_test_capability(RiskLevel::Low);
            cap.id = CapabilityId::from_string("any.cap".to_string()).unwrap();
            let mut actor = create_test_actor();
            actor.id = ActorId::from_string("bad-agent".to_string()).unwrap();
            EvaluationContext {
                capability: cap,
                actor,
                execution_id: ExecutionId::new(),
                reason: None,
            }
        };
        let result = engine.evaluate_capability(ctx).await.unwrap();
        assert!(result.decision.is_deny());

        // Fallback to default: good-agent + Low risk → Allow
        let ctx2 = {
            let cap = create_test_capability(RiskLevel::Low);
            let actor = create_test_actor(); // test-actor, not bad-agent
            EvaluationContext {
                capability: cap,
                actor,
                execution_id: ExecutionId::new(),
                reason: None,
            }
        };
        let result2 = engine.evaluate_capability(ctx2).await.unwrap();
        assert!(result2.decision.is_allow());
    }
}
