//! Policy engine and governance framework for CyberClaw.
//!
//! This crate provides the core governance capabilities for evaluating
//! and controlling capability execution based on risk assessment and
//! organizational policies.
//!
//! # Features
//!
//! - **Risk-based evaluation**: Automatic capability assessment based on risk levels
//! - **Configurable policies**: Customizable policy engines with threshold configuration
//! - **Review workflow integration**: Support for human-in-the-loop review requirements
//! - **Audit trail**: Detailed reasoning for all governance decisions
//!
//! # Examples
//!
//! ```
//! use cyberclaw_governance::engine::{DefaultPolicyEngine, PolicyEngine, EvaluationContext};
//! use cyberclaw_core::prelude::*;
//!
//! # async fn example() -> anyhow::Result<()> {
//! // Create a policy engine
//! let engine = DefaultPolicyEngine::default();
//!
//! // Create evaluation context
//! let context = EvaluationContext {
//!     capability: CapabilityRef {
//!         id: CapabilityId::from_string("fs.read".to_string()).unwrap(),
//!         connector_id: ConnectorId::from_string("local-fs".to_string()).unwrap(),
//!         risk: RiskLevel::Low,
//!         effects: vec![CapabilityEffect::Read],
//!         placement: None,
//!     },
//!     actor: ActorRef {
//!         id: ActorId::from_string("agent-1".to_string()).unwrap(),
//!         actor_type: ActorType::Agent,
//!         tenant_id: None,
//!         home_node_id: None,
//!         display_name: "Agent 1".to_string(),
//!     },
//!     execution_id: ExecutionId::new(),
//!     reason: Some("Read configuration file".to_string()),
//! };
//!
//! // Evaluate capability
//! let result = engine.evaluate_capability(context).await?;
//! println!("Decision: {:?}", result.decision);
//! # Ok(())
//! # }
//! ```

pub mod command_rewrite_registry;
pub mod dangerous_capability_filter;
pub mod driving_safety;
pub mod input_validator;
pub mod mutation_policy_gate;
pub mod prompt_injection_guard;

pub mod approval_policy;
pub mod composite_engine;
pub mod credentials;
pub mod decision;
pub mod engine;
pub mod evolution;
pub mod leak_detector;
pub mod persistent_engine;
pub mod policy;
pub mod rules;
pub mod secret_scanner;
pub mod smart_approval;
pub mod tenant_policy;
pub mod tenant_quota;
pub mod tool_output_sanitizer;
pub mod tool_permission_matcher;

pub use approval_policy::{
    ApprovalPolicy, ApprovalPolicyEngine, ApprovalRequirement, ApprovalRule,
};
pub use command_rewrite_registry::{
    CommandPermissionChecker, CommandPermissionError, CommandPermissionRule,
    CommandPermissionVerdict, PatternType, RuleVerdict,
};
pub use composite_engine::{
    CombinationStrategy, CompositePolicyEngine, CompositePolicyEngineBuilder,
    CompositePolicyEngineConfig,
};
pub use credentials::{
    ConnectorCredentialProxy, Credential, CredentialContext, CredentialError, CredentialInfo,
    CredentialVault, EnvVarVault, SecretString, VaultBackend,
};
pub use dangerous_capability_filter::{
    CapabilityException, DangerSeverity, DangerousCapabilityFilter, DangerousRule, FilterDecision,
};
pub use decision::{GovernanceDecision, ReviewType};
pub use driving_safety::{ConfirmationState, DrivingSafetyConfig, DrivingSafetyPlugin};
pub use engine::{
    DefaultPolicyEngine, EvaluationContext, EvaluationResult, NoopPolicyEngine, PolicyEngine,
    RuleBasedPolicyEngine,
};
pub use evolution::{
    EvolutionThresholds, GovernanceSignal, GovernanceSignalCollector, MetaGovernancePolicy,
    PolicyEvolutionEngine, PolicySuggestion, SignalDecision, SuggestionType,
};
pub use input_validator::{InputValidator, ValidationError, ValidationErrorCode, ValidationResult};
pub use leak_detector::{
    LeakAction, LeakDetector, LeakMatch, LeakPattern, LeakScanResult, LeakSeverity,
};
pub use mutation_policy_gate::{
    MutationPlanView, MutationPolicyGate, PolicyVerdict, DEFAULT_MAX_ALLOWED_GROWTH_PCT,
    DEFAULT_MAX_ALLOWED_SIZE,
};
pub use persistent_engine::{PersistentPolicyEngine, PolicyConditions};
pub use policy::{
    CapabilityPattern, CapabilityPolicy, PolicyAction, PolicyRule, PolicySource, SkillTrustLevel,
};
pub use prompt_injection_guard::{InjectionWarning, PromptInjectionGuard, SanitizedOutput};
pub use rules::{Rule, RuleKind, RuleSet};
pub use secret_scanner::{
    SecretFinding, SecretPattern, SecretScanResult, SecretScanner, SecretSeverity,
};
pub use smart_approval::{ApprovalDecision, DangerousPattern, SmartApproval};
pub use tenant_policy::{
    BoundaryMode, TenantBoundaryPolicy, TenantBoundaryPolicyEngine, TenantBoundaryRule,
};
pub use tenant_quota::{
    QuotaCheckResult, QuotaPolicyEngine, TenantQuota, TenantQuotaManager, TenantUsageSnapshot,
};
pub use tool_output_sanitizer::{
    SanitizationSeverity, SanitizationWarning, SanitizedToolOutput, ToolOutputSanitizer,
    WarningCategory,
};
pub use tool_permission_matcher::{
    PermissionCheckResult, PermissionDecision, ToolPermissionConfig, ToolPermissionMatcher,
    ToolPermissionRule,
};
