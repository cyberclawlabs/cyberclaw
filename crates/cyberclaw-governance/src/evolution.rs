//! Self-learning governance: policy evolution based on usage patterns.
//!
//! This module provides signal collection and analysis to suggest policy
//! changes (promotions, demotions, new rules) based on observed governance
//! decisions over time.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

use cyberclaw_core::capability::RiskLevel;

use crate::policy::PolicyAction;

// ---------------------------------------------------------------------------
// Signal types
// ---------------------------------------------------------------------------

/// The observed outcome of a governance decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalDecision {
    /// The capability was approved by policy.
    Approved,
    /// The capability was rejected by policy.
    Rejected,
    /// The capability was rejected by policy but a human overrode the decision.
    OverriddenByUser,
}

/// A single governance observation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceSignal {
    /// The capability that was evaluated.
    pub capability_id: String,
    /// The actor who requested the capability.
    pub actor_id: String,
    /// The decision that was made.
    pub decision: SignalDecision,
    /// When the decision was made.
    pub timestamp: DateTime<Utc>,
    /// Risk level of the capability at evaluation time.
    pub risk_level: RiskLevel,
}

// ---------------------------------------------------------------------------
// Signal collector
// ---------------------------------------------------------------------------

/// Thread-safe accumulator for governance signals.
#[derive(Debug, Clone)]
pub struct GovernanceSignalCollector {
    signals: Arc<RwLock<Vec<GovernanceSignal>>>,
}

impl Default for GovernanceSignalCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl GovernanceSignalCollector {
    /// Create an empty collector.
    pub fn new() -> Self {
        Self {
            signals: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Record a new governance signal.
    pub fn record_signal(&self, signal: GovernanceSignal) {
        self.signals.write().unwrap().push(signal);
    }

    /// Return all signals recorded since `since`.
    pub fn get_signals_since(&self, since: DateTime<Utc>) -> Vec<GovernanceSignal> {
        self.signals
            .read()
            .unwrap()
            .iter()
            .filter(|s| s.timestamp >= since)
            .cloned()
            .collect()
    }

    /// Approval rate for a given capability (0.0..=1.0). Returns 0.0 when no signals exist.
    pub fn approval_rate(&self, capability_id: &str) -> f64 {
        let guard = self.signals.read().unwrap();
        let matching: Vec<&GovernanceSignal> = guard
            .iter()
            .filter(|s| s.capability_id == capability_id)
            .collect();
        if matching.is_empty() {
            return 0.0;
        }
        let approved = matching
            .iter()
            .filter(|s| s.decision == SignalDecision::Approved)
            .count();
        approved as f64 / matching.len() as f64
    }

    /// Override rate for a given capability (0.0..=1.0). Returns 0.0 when no signals exist.
    pub fn override_rate(&self, capability_id: &str) -> f64 {
        let guard = self.signals.read().unwrap();
        let matching: Vec<&GovernanceSignal> = guard
            .iter()
            .filter(|s| s.capability_id == capability_id)
            .collect();
        if matching.is_empty() {
            return 0.0;
        }
        let overridden = matching
            .iter()
            .filter(|s| s.decision == SignalDecision::OverriddenByUser)
            .count();
        overridden as f64 / matching.len() as f64
    }

    /// Total signal count for a capability.
    pub fn signal_count(&self, capability_id: &str) -> usize {
        self.signals
            .read()
            .unwrap()
            .iter()
            .filter(|s| s.capability_id == capability_id)
            .count()
    }

    /// Return distinct capability IDs present in collected signals.
    pub fn capability_ids(&self) -> Vec<String> {
        let guard = self.signals.read().unwrap();
        let mut ids: Vec<String> = guard.iter().map(|s| s.capability_id.clone()).collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// Return the highest risk level observed for a capability, if any.
    pub fn max_risk_level(&self, capability_id: &str) -> Option<RiskLevel> {
        self.signals
            .read()
            .unwrap()
            .iter()
            .filter(|s| s.capability_id == capability_id)
            .map(|s| s.risk_level)
            .max()
    }
}

// ---------------------------------------------------------------------------
// Suggestion types
// ---------------------------------------------------------------------------

/// The kind of policy change being suggested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionType {
    /// Relax policy (e.g. require-approval -> allow).
    Promote,
    /// Tighten policy (e.g. allow -> deny).
    Demote,
    /// Introduce a brand-new rule.
    NewRule,
}

/// A suggested change to governance policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicySuggestion {
    /// Unique identifier for this suggestion.
    pub id: String,
    /// Kind of change.
    pub suggestion_type: SuggestionType,
    /// The capability pattern this suggestion applies to.
    pub capability_pattern: String,
    /// The current policy action (if a rule already exists).
    pub current_action: Option<PolicyAction>,
    /// The suggested new action.
    pub suggested_action: PolicyAction,
    /// Confidence score (0.0..=1.0).
    pub confidence: f64,
    /// Human-readable rationale.
    pub reason: String,
    /// When this suggestion was created.
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Meta-governance
// ---------------------------------------------------------------------------

/// Hard constraints that limit what the evolution engine may suggest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaGovernancePolicy {
    /// Suggestions below this confidence are filtered out.
    pub minimum_confidence: f64,
    /// Minimum time between suggestions for the same capability pattern.
    pub cooldown_hours: i64,
    /// Maximum number of promotions the engine may emit per analysis run.
    pub max_promotions_per_day: usize,
    /// Capability patterns that must never be auto-promoted.
    pub critical_patterns: Vec<String>,
}

impl Default for MetaGovernancePolicy {
    fn default() -> Self {
        Self {
            minimum_confidence: 0.7,
            cooldown_hours: 72,
            max_promotions_per_day: 5,
            critical_patterns: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Evolution engine
// ---------------------------------------------------------------------------

/// Configurable thresholds for the evolution engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionThresholds {
    /// Approval rate above which promotion is suggested.
    pub promote_approval_rate: f64,
    /// Minimum signals required to consider promotion.
    pub promote_min_signals: usize,
    /// Rejection rate above which demotion is suggested.
    pub demote_rejection_rate: f64,
    /// Minimum signals required to consider demotion.
    pub demote_min_signals: usize,
}

impl Default for EvolutionThresholds {
    fn default() -> Self {
        Self {
            promote_approval_rate: 0.95,
            promote_min_signals: 100,
            demote_rejection_rate: 0.5,
            demote_min_signals: 50,
        }
    }
}

/// Analyses governance signals and produces policy change suggestions.
#[derive(Debug, Clone, Default)]
pub struct PolicyEvolutionEngine {
    /// Evaluation thresholds.
    pub thresholds: EvolutionThresholds,
    /// Meta-governance hard limits.
    pub meta: MetaGovernancePolicy,
    /// Timestamps of previously emitted suggestions keyed by capability pattern.
    previous_suggestions: Vec<(String, DateTime<Utc>)>,
}

impl PolicyEvolutionEngine {
    /// Create an engine with custom thresholds and meta-governance.
    pub fn new(thresholds: EvolutionThresholds, meta: MetaGovernancePolicy) -> Self {
        Self {
            thresholds,
            meta,
            previous_suggestions: Vec::new(),
        }
    }

    /// Record that a suggestion was emitted (for cooldown tracking).
    pub fn record_suggestion(&mut self, capability_pattern: &str, when: DateTime<Utc>) {
        self.previous_suggestions
            .push((capability_pattern.to_string(), when));
    }

    /// Analyze collected signals and return policy suggestions.
    ///
    /// Applies meta-governance constraints (confidence filter, cooldown,
    /// max promotions, critical-capability protection).
    pub fn analyze(&self, collector: &GovernanceSignalCollector) -> Vec<PolicySuggestion> {
        let now = Utc::now();
        let mut raw: Vec<PolicySuggestion> = Vec::new();

        for cap_id in collector.capability_ids() {
            let count = collector.signal_count(&cap_id);
            let approval = collector.approval_rate(&cap_id);
            let rejection = 1.0 - approval - collector.override_rate(&cap_id);

            // --- Promotion check ---
            if count >= self.thresholds.promote_min_signals
                && approval > self.thresholds.promote_approval_rate
            {
                let confidence = approval; // directly proportional
                raw.push(PolicySuggestion {
                    id: format!("promote-{}", cap_id),
                    suggestion_type: SuggestionType::Promote,
                    capability_pattern: cap_id.clone(),
                    current_action: Some(PolicyAction::RequireApproval),
                    suggested_action: PolicyAction::Allow,
                    confidence,
                    reason: format!(
                        "Approval rate {:.1}% over {} signals suggests auto-approve",
                        approval * 100.0,
                        count,
                    ),
                    created_at: now,
                });
            }

            // --- Demotion check ---
            if count >= self.thresholds.demote_min_signals
                && rejection > self.thresholds.demote_rejection_rate
            {
                let confidence = rejection;
                raw.push(PolicySuggestion {
                    id: format!("demote-{}", cap_id),
                    suggestion_type: SuggestionType::Demote,
                    capability_pattern: cap_id.clone(),
                    current_action: Some(PolicyAction::Allow),
                    suggested_action: PolicyAction::Deny,
                    confidence,
                    reason: format!(
                        "Rejection rate {:.1}% over {} signals suggests blocking",
                        rejection * 100.0,
                        count,
                    ),
                    created_at: now,
                });
            }
        }

        // --- Meta-governance filters ---
        self.apply_meta_governance(raw, now)
    }

    fn apply_meta_governance(
        &self,
        mut suggestions: Vec<PolicySuggestion>,
        now: DateTime<Utc>,
    ) -> Vec<PolicySuggestion> {
        // 1. Minimum confidence filter
        suggestions.retain(|s| s.confidence >= self.meta.minimum_confidence);

        // 2. Critical capability protection: never promote critical patterns
        suggestions.retain(|s| {
            if s.suggestion_type == SuggestionType::Promote {
                !self.meta.critical_patterns.contains(&s.capability_pattern)
            } else {
                true
            }
        });

        // 3. Cooldown: skip if we already suggested for this pattern within cooldown window
        let cooldown = Duration::hours(self.meta.cooldown_hours);
        suggestions.retain(|s| {
            !self.previous_suggestions.iter().any(|(pat, ts)| {
                *pat == s.capability_pattern && now.signed_duration_since(*ts) < cooldown
            })
        });

        // 4. Max promotions per day
        let mut promo_count = 0usize;
        suggestions.retain(|s| {
            if s.suggestion_type == SuggestionType::Promote {
                if promo_count >= self.meta.max_promotions_per_day {
                    return false;
                }
                promo_count += 1;
            }
            true
        });

        suggestions
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn make_signal(
        capability_id: &str,
        decision: SignalDecision,
        risk: RiskLevel,
        timestamp: DateTime<Utc>,
    ) -> GovernanceSignal {
        GovernanceSignal {
            capability_id: capability_id.to_string(),
            actor_id: "actor-1".to_string(),
            decision,
            timestamp,
            risk_level: risk,
        }
    }

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    // -----------------------------------------------------------------------
    // 1. Signal collection and retrieval
    // -----------------------------------------------------------------------

    #[test]
    fn test_signal_collection_basic() {
        let collector = GovernanceSignalCollector::new();
        let ts = now();
        collector.record_signal(make_signal(
            "cap.a",
            SignalDecision::Approved,
            RiskLevel::Low,
            ts,
        ));
        collector.record_signal(make_signal(
            "cap.b",
            SignalDecision::Rejected,
            RiskLevel::High,
            ts,
        ));

        assert_eq!(collector.signal_count("cap.a"), 1);
        assert_eq!(collector.signal_count("cap.b"), 1);
        assert_eq!(collector.signal_count("cap.c"), 0);
    }

    #[test]
    fn test_signals_since_filter() {
        let collector = GovernanceSignalCollector::new();
        let old = now() - Duration::hours(48);
        let recent = now() - Duration::hours(1);

        collector.record_signal(make_signal(
            "cap.a",
            SignalDecision::Approved,
            RiskLevel::Low,
            old,
        ));
        collector.record_signal(make_signal(
            "cap.a",
            SignalDecision::Approved,
            RiskLevel::Low,
            recent,
        ));

        let since_24h = now() - Duration::hours(24);
        let filtered = collector.get_signals_since(since_24h);
        assert_eq!(filtered.len(), 1);
    }

    // -----------------------------------------------------------------------
    // 2. Approval / override rate calculation
    // -----------------------------------------------------------------------

    #[test]
    fn test_approval_rate_all_approved() {
        let collector = GovernanceSignalCollector::new();
        let ts = now();
        for _ in 0..10 {
            collector.record_signal(make_signal(
                "cap.a",
                SignalDecision::Approved,
                RiskLevel::Low,
                ts,
            ));
        }
        let rate = collector.approval_rate("cap.a");
        assert!((rate - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_approval_rate_mixed() {
        let collector = GovernanceSignalCollector::new();
        let ts = now();
        for _ in 0..7 {
            collector.record_signal(make_signal(
                "cap.a",
                SignalDecision::Approved,
                RiskLevel::Low,
                ts,
            ));
        }
        for _ in 0..3 {
            collector.record_signal(make_signal(
                "cap.a",
                SignalDecision::Rejected,
                RiskLevel::Low,
                ts,
            ));
        }
        let rate = collector.approval_rate("cap.a");
        assert!((rate - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn test_override_rate() {
        let collector = GovernanceSignalCollector::new();
        let ts = now();
        for _ in 0..8 {
            collector.record_signal(make_signal(
                "cap.a",
                SignalDecision::Approved,
                RiskLevel::Low,
                ts,
            ));
        }
        for _ in 0..2 {
            collector.record_signal(make_signal(
                "cap.a",
                SignalDecision::OverriddenByUser,
                RiskLevel::Low,
                ts,
            ));
        }
        let rate = collector.override_rate("cap.a");
        assert!((rate - 0.2).abs() < f64::EPSILON);
    }

    #[test]
    fn test_rates_empty_capability() {
        let collector = GovernanceSignalCollector::new();
        assert!((collector.approval_rate("none")).abs() < f64::EPSILON);
        assert!((collector.override_rate("none")).abs() < f64::EPSILON);
    }

    // -----------------------------------------------------------------------
    // 3. Promotion suggestion generation
    // -----------------------------------------------------------------------

    #[test]
    fn test_promotion_suggested_on_high_approval() {
        let collector = GovernanceSignalCollector::new();
        let ts = now();
        // 100 approved, 2 rejected => approval 100/102 ≈ 0.98
        for _ in 0..100 {
            collector.record_signal(make_signal(
                "cap.safe",
                SignalDecision::Approved,
                RiskLevel::Low,
                ts,
            ));
        }
        for _ in 0..2 {
            collector.record_signal(make_signal(
                "cap.safe",
                SignalDecision::Rejected,
                RiskLevel::Low,
                ts,
            ));
        }

        let engine = PolicyEvolutionEngine::default();
        let suggestions = engine.analyze(&collector);
        assert!(
            suggestions
                .iter()
                .any(|s| s.suggestion_type == SuggestionType::Promote
                    && s.capability_pattern == "cap.safe"),
            "Expected a promotion suggestion for cap.safe"
        );
    }

    // -----------------------------------------------------------------------
    // 4. Demotion suggestion generation
    // -----------------------------------------------------------------------

    #[test]
    fn test_demotion_suggested_on_high_rejection() {
        let collector = GovernanceSignalCollector::new();
        let ts = now();
        // 40 rejected, 10 approved => rejection = 40/50 = 0.8
        for _ in 0..40 {
            collector.record_signal(make_signal(
                "cap.risky",
                SignalDecision::Rejected,
                RiskLevel::High,
                ts,
            ));
        }
        for _ in 0..10 {
            collector.record_signal(make_signal(
                "cap.risky",
                SignalDecision::Approved,
                RiskLevel::High,
                ts,
            ));
        }

        let engine = PolicyEvolutionEngine::default();
        let suggestions = engine.analyze(&collector);
        assert!(
            suggestions
                .iter()
                .any(|s| s.suggestion_type == SuggestionType::Demote
                    && s.capability_pattern == "cap.risky"),
            "Expected a demotion suggestion for cap.risky"
        );
    }

    // -----------------------------------------------------------------------
    // 5. Meta-governance: confidence filter
    // -----------------------------------------------------------------------

    #[test]
    fn test_meta_confidence_filter() {
        let collector = GovernanceSignalCollector::new();
        let ts = now();
        // 55 rejected, 45 approved => rejection = 0.55, approval = 0.45
        // demotion confidence = 0.55 which is < 0.7 minimum
        for _ in 0..55 {
            collector.record_signal(make_signal(
                "cap.edge",
                SignalDecision::Rejected,
                RiskLevel::Medium,
                ts,
            ));
        }
        for _ in 0..45 {
            collector.record_signal(make_signal(
                "cap.edge",
                SignalDecision::Approved,
                RiskLevel::Medium,
                ts,
            ));
        }

        let engine = PolicyEvolutionEngine::default();
        let suggestions = engine.analyze(&collector);
        // Should be filtered out because confidence 0.55 < 0.7
        assert!(
            !suggestions
                .iter()
                .any(|s| s.capability_pattern == "cap.edge"),
            "Low-confidence suggestion should be filtered by meta-governance"
        );
    }

    // -----------------------------------------------------------------------
    // 6. Cooldown enforcement
    // -----------------------------------------------------------------------

    #[test]
    fn test_cooldown_blocks_repeated_suggestion() {
        let collector = GovernanceSignalCollector::new();
        let ts = now();
        for _ in 0..120 {
            collector.record_signal(make_signal(
                "cap.repeat",
                SignalDecision::Approved,
                RiskLevel::Low,
                ts,
            ));
        }

        let mut engine = PolicyEvolutionEngine::default();
        // First analysis should produce a suggestion
        let first = engine.analyze(&collector);
        assert!(
            first.iter().any(|s| s.capability_pattern == "cap.repeat"),
            "First analysis should produce a suggestion"
        );

        // Record that we emitted the suggestion recently
        engine.record_suggestion("cap.repeat", now());

        // Second analysis within cooldown should suppress it
        let second = engine.analyze(&collector);
        assert!(
            !second.iter().any(|s| s.capability_pattern == "cap.repeat"),
            "Cooldown should suppress repeated suggestion"
        );
    }

    // -----------------------------------------------------------------------
    // 7. Critical capability protection
    // -----------------------------------------------------------------------

    #[test]
    fn test_critical_capability_never_promoted() {
        let collector = GovernanceSignalCollector::new();
        let ts = now();
        // Even with perfect approval rate, critical caps should not be promoted
        for _ in 0..200 {
            collector.record_signal(make_signal(
                "admin.delete",
                SignalDecision::Approved,
                RiskLevel::Critical,
                ts,
            ));
        }

        let meta = MetaGovernancePolicy {
            critical_patterns: vec!["admin.delete".to_string()],
            ..Default::default()
        };
        let engine = PolicyEvolutionEngine::new(EvolutionThresholds::default(), meta);
        let suggestions = engine.analyze(&collector);
        assert!(
            !suggestions
                .iter()
                .any(|s| s.suggestion_type == SuggestionType::Promote
                    && s.capability_pattern == "admin.delete"),
            "Critical capability must never be auto-promoted"
        );
    }

    // -----------------------------------------------------------------------
    // Additional coverage
    // -----------------------------------------------------------------------

    #[test]
    fn test_max_promotions_per_day_limit() {
        let collector = GovernanceSignalCollector::new();
        let ts = now();
        // Create 7 capabilities all qualifying for promotion
        for i in 0..7 {
            let cap = format!("cap.promo{}", i);
            for _ in 0..110 {
                collector.record_signal(make_signal(
                    &cap,
                    SignalDecision::Approved,
                    RiskLevel::Low,
                    ts,
                ));
            }
        }

        let engine = PolicyEvolutionEngine::default(); // max_promotions_per_day = 5
        let suggestions = engine.analyze(&collector);
        let promo_count = suggestions
            .iter()
            .filter(|s| s.suggestion_type == SuggestionType::Promote)
            .count();
        assert!(
            promo_count <= 5,
            "Should not exceed max_promotions_per_day (got {})",
            promo_count
        );
    }

    #[test]
    fn test_no_suggestion_below_min_signals() {
        let collector = GovernanceSignalCollector::new();
        let ts = now();
        // Only 10 signals — below both promote (100) and demote (50) thresholds
        for _ in 0..10 {
            collector.record_signal(make_signal(
                "cap.few",
                SignalDecision::Approved,
                RiskLevel::Low,
                ts,
            ));
        }

        let engine = PolicyEvolutionEngine::default();
        let suggestions = engine.analyze(&collector);
        assert!(
            suggestions.is_empty(),
            "No suggestions expected when signal count is below threshold"
        );
    }

    #[test]
    fn test_capability_ids_deduplication() {
        let collector = GovernanceSignalCollector::new();
        let ts = now();
        collector.record_signal(make_signal(
            "cap.a",
            SignalDecision::Approved,
            RiskLevel::Low,
            ts,
        ));
        collector.record_signal(make_signal(
            "cap.a",
            SignalDecision::Rejected,
            RiskLevel::Low,
            ts,
        ));
        collector.record_signal(make_signal(
            "cap.b",
            SignalDecision::Approved,
            RiskLevel::Low,
            ts,
        ));

        let ids = collector.capability_ids();
        assert_eq!(ids, vec!["cap.a", "cap.b"]);
    }

    #[test]
    fn test_thread_safety_clone() {
        // Verify the collector can be cloned (Arc-based sharing)
        let collector = GovernanceSignalCollector::new();
        let ts = now();
        collector.record_signal(make_signal(
            "cap.a",
            SignalDecision::Approved,
            RiskLevel::Low,
            ts,
        ));

        let clone = collector.clone();
        assert_eq!(clone.signal_count("cap.a"), 1);

        // Writing through clone is visible in original
        clone.record_signal(make_signal(
            "cap.b",
            SignalDecision::Rejected,
            RiskLevel::High,
            ts,
        ));
        assert_eq!(collector.signal_count("cap.b"), 1);
    }
}
