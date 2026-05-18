//! Persistent policy engine with StateStore integration.
//!
//! This module provides a [`PolicyEngine`] implementation that persists policies to a
//! [`StateStore`] backend (PostgreSQL or in-memory), combining fast in-memory evaluation
//! with durable persistence.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────┐
//! │  PersistentPolicyEngine             │
//! │  ┌───────────┐      ┌────────────┐ │
//! │  │ In-Memory │◄────►│ StateStore │ │
//! │  │  Cache    │      │  Backend   │ │
//! │  │(RwLock)   │      │(PostgreSQL)│ │
//! │  └───────────┘      └────────────┘ │
//! └─────────────────────────────────────┘
//!         ▲                    │
//!         │ evaluate()         │ add_policy()
//!         │ (read-only)        │ reload_policies()
//!         │                    ▼ (write-through)
//! ```
//!
//! ### Key Design Patterns
//!
//! 1. **Load-on-Init**: On creation, all active policies are loaded from StateStore into memory
//! 2. **Write-Through Caching**: New policies are first written to StateStore, then cached
//! 3. **Read-Optimized**: Policy evaluation uses read-only lock for high concurrency
//! 4. **Bidirectional Conversion**: Automatic conversion between domain types and storage records
//!
//! ## Usage Examples
//!
//! ### Basic Setup with In-Memory Store
//!
//! ```rust
//! use std::sync::Arc;
//! use cyberclaw_store::memory::InMemoryStateStore;
//! use cyberclaw_governance::persistent_engine::PersistentPolicyEngine;
//! use cyberclaw_governance::policy::PolicyAction;
//!
//! # async fn example() -> anyhow::Result<()> {
//! // Create in-memory store
//! let store = Arc::new(InMemoryStateStore::new());
//!
//! // Create engine with default policies
//! let engine = PersistentPolicyEngine::with_default_policies(store).await?;
//!
//! // Or create with custom default action
//! // let engine = PersistentPolicyEngine::new(
//! //     store,
//! //     PolicyAction::Deny,
//! //     "Default deny for security"
//! // ).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ### Adding Policy Rules
//!
//! ```rust
//! # use std::sync::Arc;
//! # use cyberclaw_store::memory::InMemoryStateStore;
//! # use cyberclaw_governance::persistent_engine::PersistentPolicyEngine;
//! # use cyberclaw_governance::policy::{PolicyAction, PolicyRule};
//! # use cyberclaw_core::capability::RiskLevel;
//! # async fn example() -> anyhow::Result<()> {
//! # let store = Arc::new(InMemoryStateStore::new());
//! # let engine = PersistentPolicyEngine::with_default_policies(store).await?;
//! // Allow low-risk capabilities automatically
//! let rule = PolicyRule {
//!     name: "allow-low-risk".to_string(),
//!     priority: 10,
//!     capability_id: None,
//!     pattern: None,
//!     risk_level: Some(RiskLevel::Low),
//!     effect: None,
//!     actor: None,
//!     tenant_id: None,
//!     source: cyberclaw_governance::PolicySource::SystemDefault,
//!     action: PolicyAction::Allow,
//!     reason: "Low risk capabilities are automatically allowed".to_string(),
//! };
//!
//! engine.add_policy(rule).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ### Policy Evaluation
//!
//! ```rust
//! # use std::sync::Arc;
//! # use cyberclaw_store::memory::InMemoryStateStore;
//! # use cyberclaw_governance::persistent_engine::PersistentPolicyEngine;
//! # use cyberclaw_governance::engine::{PolicyEngine, EvaluationContext};
//! # use cyberclaw_core::capability::{CapabilityRef, CapabilityEffect, RiskLevel};
//! # use cyberclaw_core::identity::{ActorRef, ActorType};
//! # use cyberclaw_core::ids::{CapabilityId, ConnectorId, ActorId, ExecutionId};
//! # async fn example() -> anyhow::Result<()> {
//! # let store = Arc::new(InMemoryStateStore::new());
//! # let engine = PersistentPolicyEngine::with_default_policies(store).await?;
//! // Build evaluation context
//! let context = EvaluationContext {
//!     capability: CapabilityRef {
//!         id: CapabilityId::from_string("file.read".to_string())?,
//!         connector_id: ConnectorId::from_string("fs-connector".to_string())?,
//!         risk: RiskLevel::Low,
//!         effects: vec![CapabilityEffect::Read],
//!         placement: None,
//!     },
//!     actor: ActorRef {
//!         id: ActorId::from_string("user-123".to_string())?,
//!         actor_type: ActorType::Human,
//!         tenant_id: None,
//!         home_node_id: None,
//!         display_name: "Alice".to_string(),
//!     },
//!     execution_id: ExecutionId::new(),
//!     reason: Some("Read config file".to_string()),
//! };
//!
//! // Evaluate capability
//! let result = engine.evaluate_capability(context).await?;
//!
//! if result.decision.is_allow() {
//!     println!("✅ Capability allowed");
//! } else if result.decision.is_review_required() {
//!     println!("⏸️ Requires human review");
//! } else {
//!     println!("❌ Capability denied");
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ### Policy Management Operations
//!
//! ```rust
//! # use std::sync::Arc;
//! # use cyberclaw_store::memory::InMemoryStateStore;
//! # use cyberclaw_governance::persistent_engine::PersistentPolicyEngine;
//! # async fn example() -> anyhow::Result<()> {
//! # let store = Arc::new(InMemoryStateStore::new());
//! # let engine = PersistentPolicyEngine::with_default_policies(store).await?;
//! // Deactivate a policy (keeps in store but excludes from evaluation)
//! engine.deactivate_policy("old-policy").await?;
//!
//! // Reactivate a policy
//! engine.activate_policy("old-policy").await?;
//!
//! // Reload all policies from store (useful after external changes)
//! engine.reload_policies().await?;
//!
//! // Get all current rules (read-only)
//! let rules = engine.get_rules().await;
//! println!("Loaded {} policy rules", rules.len());
//! # Ok(())
//! # }
//! ```
//!
//! ## Storage Schema
//!
//! Policies are stored using [`PolicyRecord`] with JSON-encoded conditions:
//!
//! ```sql
//! CREATE TABLE policies (
//!     id UUID PRIMARY KEY,
//!     name TEXT UNIQUE NOT NULL,
//!     effect TEXT NOT NULL,  -- "Allow", "Deny", "RequireHumanReview", etc.
//!     conditions JSONB NOT NULL,  -- { priority, capability_id, risk_level, effect, actor_id }
//!     active BOOLEAN NOT NULL DEFAULT true,
//!     created_at TIMESTAMPTZ NOT NULL,
//!     updated_at TIMESTAMPTZ NOT NULL
//! );
//! ```
//!
//! ### PolicyConditions JSON Schema
//!
//! ```json
//! {
//!   "priority": 100,
//!   "capability_id": "file.write",       // optional
//!   "risk_level": "High",                // optional: Low | Medium | High | Critical
//!   "effect": "Write",                   // optional: Read | Write | Execute | Network | etc.
//!   "actor_id": "user-123"               // optional
//! }
//! ```
//!
//! ## Conversion Patterns
//!
//! The engine automatically converts between:
//!
//! - **Domain Type**: [`PolicyRule`] (in-memory, type-safe)
//! - **Storage Type**: [`PolicyRecord`] (persistent, JSON-based)
//!
//! Conversion methods:
//!
//! - `record_to_rule()`: Parse JSON conditions → Typed PolicyRule
//! - `rule_to_record()`: Serialize PolicyRule → JSON PolicyRecord
//!
//! ## Caching Strategy
//!
//! 1. **Initialization**: Load all active policies from StateStore on `new()`
//! 2. **Add Policy**: Write to StateStore → Update in-memory cache
//! 3. **Deactivate/Activate**: Update StateStore → Reload all policies
//! 4. **Reload**: Fetch from StateStore → Replace in-memory cache
//! 5. **Evaluation**: Read-only lock on in-memory cache (no disk I/O)
//!
//! This design ensures:
//! - **Fast evaluation** (in-memory reads)
//! - **Durability** (all writes persisted)
//! - **Consistency** (write-through updates)
//! - **High concurrency** (read-write lock)

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use cyberclaw_core::capability::{CapabilityEffect, RiskLevel};
use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::{ActorId, CapabilityId};
use cyberclaw_store::state_store::{PolicyRecord, StateStore};

use crate::engine::{EvaluationContext, EvaluationResult, PolicyEngine};
use crate::policy::{CapabilityPolicy, PolicyAction, PolicyRule};

/// Policy conditions stored in PolicyRecord.conditions (JSON)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyConditions {
    pub priority: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk_level: Option<String>, // "Low", "Medium", "High", "Critical"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect: Option<String>, // "Read", "Write", "Execute", "Delete", "Admin"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,
}

/// Persistent policy engine that loads policies from StateStore
pub struct PersistentPolicyEngine {
    /// StateStore backend
    store: Arc<dyn StateStore>,
    /// In-memory policy cache
    policy: Arc<RwLock<CapabilityPolicy>>,
    /// Default action when no policy matches
    default_action: PolicyAction,
    /// Default reason for default action
    default_reason: String,
}

impl PersistentPolicyEngine {
    /// Create a new persistent policy engine
    ///
    /// # Arguments
    ///
    /// * `store` - StateStore backend (PostgreSQL or in-memory)
    /// * `default_action` - Default action when no rule matches
    /// * `default_reason` - Default reason for default action
    pub async fn new(
        store: Arc<dyn StateStore>,
        default_action: PolicyAction,
        default_reason: impl Into<String>,
    ) -> Result<Self> {
        let default_reason = default_reason.into();

        // Load initial policies from store
        let policy_records = store
            .list_policies(true) // Load only active policies
            .await
            .context("Failed to load policies from store")?;

        // Convert PolicyRecords to PolicyRules
        let mut rules = Vec::new();
        for record in policy_records {
            match Self::record_to_rule(&record) {
                Ok(rule) => rules.push(rule),
                Err(e) => {
                    // Log warning but continue loading other policies
                    eprintln!("Warning: Failed to load policy '{}': {}", record.name, e);
                }
            }
        }

        let policy = Arc::new(RwLock::new(CapabilityPolicy::new(
            rules,
            default_action.clone(),
            default_reason.clone(),
        )));

        Ok(Self {
            store,
            policy,
            default_action,
            default_reason,
        })
    }

    /// Create a persistent policy engine with default policies
    pub async fn with_default_policies(store: Arc<dyn StateStore>) -> Result<Self> {
        Self::new(
            store,
            PolicyAction::RequireHumanReview,
            "No matching policy, default to human review",
        )
        .await
    }

    /// Convert PolicyRecord to PolicyRule
    fn record_to_rule(record: &PolicyRecord) -> Result<PolicyRule> {
        // Parse conditions from JSON
        let conditions: PolicyConditions = serde_json::from_value(record.conditions.clone())
            .context("Failed to parse policy conditions")?;

        // Parse PolicyAction from effect string
        let action = Self::parse_action(&record.effect)
            .with_context(|| format!("Invalid policy effect: {}", record.effect))?;

        // Parse optional capability_id
        let capability_id = if let Some(ref cap_id_str) = conditions.capability_id {
            Some(
                CapabilityId::from_string(cap_id_str.clone())
                    .context("Invalid capability_id in policy")?,
            )
        } else {
            None
        };

        // Parse optional risk_level
        let risk_level = if let Some(ref risk_str) = conditions.risk_level {
            Some(Self::parse_risk_level(risk_str)?)
        } else {
            None
        };

        // Parse optional effect
        let effect = if let Some(ref effect_str) = conditions.effect {
            Some(Self::parse_capability_effect(effect_str)?)
        } else {
            None
        };

        // Parse optional actor
        let actor = if let Some(ref actor_id_str) = conditions.actor_id {
            Some(ActorRef {
                id: ActorId::from_string(actor_id_str.clone())
                    .context("Invalid actor_id in policy")?,
                actor_type: ActorType::Agent, // Default to Agent
                tenant_id: None,
                home_node_id: None,
                display_name: actor_id_str.clone(),
            })
        } else {
            None
        };

        // Parse optional tenant_id
        let tenant_id = if let Some(ref tenant_id_str) = conditions.tenant_id {
            Some(
                cyberclaw_core::ids::TenantId::from_string(tenant_id_str.clone())
                    .context("Invalid tenant_id in policy")?,
            )
        } else {
            None
        };

        Ok(PolicyRule {
            name: record.name.clone(),
            priority: conditions.priority,
            capability_id,
            pattern: None,
            risk_level,
            effect,
            actor,
            tenant_id,
            source: crate::policy::PolicySource::SystemDefault,
            action,
            reason: format!("Policy: {}", record.name),
        })
    }

    /// Convert PolicyRule to PolicyRecord
    fn rule_to_record(rule: &PolicyRule) -> Result<PolicyRecord> {
        let conditions = PolicyConditions {
            priority: rule.priority,
            capability_id: rule.capability_id.as_ref().map(|id| id.to_string()),
            risk_level: rule.risk_level.as_ref().map(Self::format_risk_level),
            effect: rule.effect.as_ref().map(Self::format_capability_effect),
            actor_id: rule.actor.as_ref().map(|a| a.id.to_string()),
            tenant_id: rule.tenant_id.as_ref().map(|tid| tid.to_string()),
        };

        let conditions_json =
            serde_json::to_value(&conditions).context("Failed to serialize policy conditions")?;

        Ok(PolicyRecord {
            id: Uuid::new_v4(),
            name: rule.name.clone(),
            effect: Self::format_action(&rule.action),
            conditions: conditions_json,
            active: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
    }

    /// Parse PolicyAction from string
    fn parse_action(s: &str) -> Result<PolicyAction> {
        match s {
            "Allow" => Ok(PolicyAction::Allow),
            "Deny" => Ok(PolicyAction::Deny),
            "RequireHumanReview" => Ok(PolicyAction::RequireHumanReview),
            "RequireApproval" => Ok(PolicyAction::RequireApproval),
            "RequireSecurityReview" => Ok(PolicyAction::RequireSecurityReview),
            "RequireEscalation" => Ok(PolicyAction::RequireEscalation),
            _ => Err(anyhow::anyhow!("Unknown policy action: {}", s)),
        }
    }

    /// Format PolicyAction to string
    fn format_action(action: &PolicyAction) -> String {
        match action {
            PolicyAction::Allow => "Allow".to_string(),
            PolicyAction::Deny => "Deny".to_string(),
            PolicyAction::RequireHumanReview => "RequireHumanReview".to_string(),
            PolicyAction::RequireApproval => "RequireApproval".to_string(),
            PolicyAction::RequireSecurityReview => "RequireSecurityReview".to_string(),
            PolicyAction::RequireEscalation => "RequireEscalation".to_string(),
        }
    }

    /// Parse RiskLevel from string
    fn parse_risk_level(s: &str) -> Result<RiskLevel> {
        match s {
            "Low" => Ok(RiskLevel::Low),
            "Medium" => Ok(RiskLevel::Medium),
            "High" => Ok(RiskLevel::High),
            "Critical" => Ok(RiskLevel::Critical),
            _ => Err(anyhow::anyhow!("Unknown risk level: {}", s)),
        }
    }

    /// Format RiskLevel to string
    fn format_risk_level(level: &RiskLevel) -> String {
        match level {
            RiskLevel::Low => "Low".to_string(),
            RiskLevel::Medium => "Medium".to_string(),
            RiskLevel::High => "High".to_string(),
            RiskLevel::Critical => "Critical".to_string(),
        }
    }

    /// Parse CapabilityEffect from string
    fn parse_capability_effect(s: &str) -> Result<CapabilityEffect> {
        match s {
            "Read" => Ok(CapabilityEffect::Read),
            "Write" => Ok(CapabilityEffect::Write),
            "Execute" => Ok(CapabilityEffect::Execute),
            "Network" => Ok(CapabilityEffect::Network),
            "Ticket" => Ok(CapabilityEffect::Ticket),
            "Notification" => Ok(CapabilityEffect::Notification),
            other => Ok(CapabilityEffect::Custom(other.to_string())),
        }
    }

    /// Format CapabilityEffect to string
    fn format_capability_effect(effect: &CapabilityEffect) -> String {
        match effect {
            CapabilityEffect::Read => "Read".to_string(),
            CapabilityEffect::Write => "Write".to_string(),
            CapabilityEffect::Execute => "Execute".to_string(),
            CapabilityEffect::Network => "Network".to_string(),
            CapabilityEffect::Ticket => "Ticket".to_string(),
            CapabilityEffect::Notification => "Notification".to_string(),
            CapabilityEffect::Custom(s) => s.clone(),
        }
    }

    /// Add a new policy rule
    pub async fn add_policy(&self, rule: PolicyRule) -> Result<()> {
        // Convert rule to record
        let record = Self::rule_to_record(&rule)?;

        // Save to store
        self.store
            .save_policy(record)
            .await
            .context("Failed to save policy to store")?;

        // Update in-memory policy
        let mut policy = self.policy.write().await;
        policy.add_rule(rule);

        Ok(())
    }

    /// Reload all policies from store
    pub async fn reload_policies(&self) -> Result<()> {
        // Load policies from store
        let policy_records = self
            .store
            .list_policies(true) // Load only active policies
            .await
            .context("Failed to load policies from store")?;

        // Convert PolicyRecords to PolicyRules
        let mut rules = Vec::new();
        for record in policy_records {
            match Self::record_to_rule(&record) {
                Ok(rule) => rules.push(rule),
                Err(e) => {
                    eprintln!("Warning: Failed to load policy '{}': {}", record.name, e);
                }
            }
        }

        // Replace in-memory policy
        let mut policy = self.policy.write().await;
        *policy = CapabilityPolicy::new(
            rules,
            self.default_action.clone(),
            self.default_reason.clone(),
        );

        Ok(())
    }

    /// Activate a policy by name
    pub async fn activate_policy(&self, name: &str) -> Result<()> {
        self.store
            .update_policy(name, true)
            .await
            .context("Failed to activate policy")?;

        // Reload policies to reflect changes
        self.reload_policies().await
    }

    /// Deactivate a policy by name
    pub async fn deactivate_policy(&self, name: &str) -> Result<()> {
        self.store
            .update_policy(name, false)
            .await
            .context("Failed to deactivate policy")?;

        // Reload policies to reflect changes
        self.reload_policies().await
    }

    /// Get all policy rules (read-only)
    pub async fn get_rules(&self) -> Vec<PolicyRule> {
        let policy = self.policy.read().await;
        policy.rules().to_vec()
    }
}

#[async_trait]
impl PolicyEngine for PersistentPolicyEngine {
    async fn evaluate_capability(&self, context: EvaluationContext) -> Result<EvaluationResult> {
        // Read the current policy
        let policy = self.policy.read().await;

        // Evaluate using CapabilityPolicy
        let decision = policy.evaluate(&context.capability, &context.actor);

        // Build context information
        let context_info = vec![
            format!("Capability: {}", context.capability.id),
            format!("Connector: {}", context.capability.connector_id),
            format!("Risk: {:?}", context.capability.risk),
            format!("Actor: {}", context.actor.id),
            format!("Execution: {}", context.execution_id),
        ];

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
    use cyberclaw_core::capability::CapabilityRef;
    use cyberclaw_core::ids::{ConnectorId, ExecutionId};
    use cyberclaw_store::memory::InMemoryStateStore;

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
    async fn test_conversion_round_trip() {
        let rule = PolicyRule {
            name: "test-rule".to_string(),
            priority: 100,
            capability_id: Some(CapabilityId::from_string("test.cap".to_string()).unwrap()),
            pattern: None,
            risk_level: Some(RiskLevel::High),
            effect: Some(CapabilityEffect::Write),
            actor: None,
            tenant_id: None,
            source: crate::policy::PolicySource::SystemDefault,
            action: PolicyAction::RequireApproval,
            reason: "Test rule".to_string(),
        };

        // Convert rule to record
        let record = PersistentPolicyEngine::rule_to_record(&rule).unwrap();

        // Convert record back to rule
        let converted_rule = PersistentPolicyEngine::record_to_rule(&record).unwrap();

        // Verify fields match
        assert_eq!(rule.name, converted_rule.name);
        assert_eq!(rule.priority, converted_rule.priority);
        assert_eq!(rule.capability_id, converted_rule.capability_id);
        assert_eq!(rule.risk_level, converted_rule.risk_level);
        assert_eq!(rule.effect, converted_rule.effect);
    }

    #[tokio::test]
    async fn test_persistent_engine_creation() {
        let store = Arc::new(InMemoryStateStore::new());
        let engine = PersistentPolicyEngine::new(store, PolicyAction::Deny, "Default deny")
            .await
            .unwrap();

        assert_eq!(engine.default_action, PolicyAction::Deny);
    }

    #[tokio::test]
    async fn test_add_and_evaluate_policy() {
        let store = Arc::new(InMemoryStateStore::new());
        let engine = PersistentPolicyEngine::new(store, PolicyAction::Deny, "Default deny")
            .await
            .unwrap();

        // Add a policy rule
        let rule = PolicyRule {
            name: "allow-low-risk".to_string(),
            priority: 10,
            capability_id: None,
            pattern: None,
            risk_level: Some(RiskLevel::Low),
            effect: None,
            actor: None,
            tenant_id: None,
            source: crate::policy::PolicySource::SystemDefault,
            action: PolicyAction::Allow,
            reason: "Low risk is allowed".to_string(),
        };

        engine.add_policy(rule).await.unwrap();

        // Evaluate low risk capability
        let capability = create_test_capability(RiskLevel::Low);
        let context = create_test_context(capability);

        let result = engine.evaluate_capability(context).await.unwrap();
        assert!(result.decision.is_allow());
    }

    #[tokio::test]
    async fn test_reload_policies() {
        let store = Arc::new(InMemoryStateStore::new());
        let engine = PersistentPolicyEngine::new(store.clone(), PolicyAction::Deny, "Default deny")
            .await
            .unwrap();

        // Add a policy
        let rule = PolicyRule {
            name: "test-policy".to_string(),
            priority: 10,
            capability_id: None,
            pattern: None,
            risk_level: Some(RiskLevel::Medium),
            effect: None,
            actor: None,
            tenant_id: None,
            source: crate::policy::PolicySource::SystemDefault,
            action: PolicyAction::Allow,
            reason: "Test".to_string(),
        };

        engine.add_policy(rule.clone()).await.unwrap();

        // Create a new engine instance (simulates restart)
        let engine2 = PersistentPolicyEngine::new(store, PolicyAction::Deny, "Default deny")
            .await
            .unwrap();

        // Verify policy was loaded
        let rules = engine2.get_rules().await;
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].name, "test-policy");
    }
}
