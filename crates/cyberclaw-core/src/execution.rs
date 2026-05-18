use crate::capability::RiskLevel;
use crate::ids::{CaseId, ExecutionId, LeaseId, NodeId, SkillId, TaskId, TraceId};
use crate::workspace::WorkspaceRef;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRef {
    pub id: crate::ids::AgentId,
    pub role: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionStatus {
    Pending,
    Running,
    WaitingReview,
    WaitingApproval,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

/// Execution mode determines the runtime behavior of an execution.
/// WARNING: Adding variants requires updating all Execution construction sites.
///
/// Serde uses snake_case so HTTP clients can send `"persistent"`, `"autopilot"`,
/// or `"normal"` (the formats emitted by JS/TS callers).  The previous
/// PascalCase wire format is no longer accepted — update any stored JSON that
/// used the old casing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    /// Standard single-shot execution.
    #[default]
    Normal,
    /// Autopilot: iterative goal-driven loop with governed steps.
    Autopilot,
    /// Persistent: story-driven long-running execution with checkpoints.
    /// Sprint D1: routed by `InMemoryExecutionService::execute()` to the
    /// optional `PersistentLoop` wired via `with_persistent_loop`. When the
    /// loop is not wired the execution fails with an explicit
    /// "PersistentLoop not wired" error. Sprint D3 implements the per-story
    /// dispatch internals.
    Persistent,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExecutionBudget {
    pub max_steps: Option<u32>,
    pub max_duration_ms: Option<u64>,
    pub max_tokens: Option<u32>,
    pub max_children: Option<u32>,
    /// Sprint 10 (gradual landing): accumulator updated after each LLM call.
    /// `#[serde(default)]` keeps backward compat with persisted budgets that
    /// pre-date this field. Compared against `max_tokens` via
    /// [`Self::is_token_exceeded`].
    #[serde(default)]
    pub tokens_used: u32,
}

impl ExecutionBudget {
    /// Sprint 10: record consumption from one LLM call (input + output token
    /// counts). Saturating to avoid overflow on extreme long-context calls.
    /// Returns `&mut Self` for builder-style chaining in tests / callers
    /// that own the budget directly.
    pub fn record_tokens(&mut self, input: u32, output: u32) -> &mut Self {
        self.tokens_used = self
            .tokens_used
            .saturating_add(input)
            .saturating_add(output);
        self
    }

    /// Sprint 10: returns `true` when `max_tokens` is set and `tokens_used`
    /// has reached or exceeded it. Always `false` when `max_tokens.is_none()`
    /// (no cap configured = unlimited).
    pub fn is_token_exceeded(&self) -> bool {
        self.max_tokens.is_some_and(|cap| self.tokens_used >= cap)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JoinStrategy {
    JoinAll,
    JoinAny,
    FanOutFanIn,
    MapReduce,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Execution {
    pub id: ExecutionId,
    pub root_execution_id: ExecutionId,
    pub parent_execution_id: Option<ExecutionId>,
    pub owner_node_id: Option<NodeId>,
    pub scheduled_node_id: Option<NodeId>,
    pub placement_group: Option<String>,
    pub lease_id: Option<LeaseId>,
    pub handoff_count: u32,
    pub case_id: Option<CaseId>,
    pub task_id: Option<TaskId>,
    pub agent: AgentRef,
    pub status: ExecutionStatus,
    pub join_strategy: Option<JoinStrategy>,
    pub budget: ExecutionBudget,
    pub workspace: Option<WorkspaceRef>,
    pub trace_id: TraceId,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Risk level of this execution, determined by the highest risk capability used
    pub risk_level: RiskLevel,
    /// Execution mode (Normal/Autopilot/Persistent). Set by the Resolver.
    #[serde(default)]
    pub execution_mode: ExecutionMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionTreeNode {
    pub execution: Execution,
    pub owner_node_id: Option<NodeId>,
    pub locality_hint: Option<String>,
    pub children: Vec<ExecutionId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExecutionMetrics {
    pub duration_ms: Option<u64>,
    pub steps: Option<u32>,
    pub tokens_in: Option<u32>,
    pub tokens_out: Option<u32>,
}

/// Sprint 44+1 — per-request runtime context the caller hands to the
/// agentic loop on creation. Carries overrides that should layer on top of
/// the agent's static manifest defaults (e.g. [`AgentConfig`] /
/// [`AgentSpec`] declared bindings).
///
/// Today only `runtime_skill_bindings` is wired: a chat handler can set
/// it from `req.skill_ids` to override the agent's `default_skills`. When
/// `None`, the loop falls back to the agent's defaults; when `Some`, the
/// runtime override takes precedence.
///
/// Backward-compatible: every field is `#[serde(default)]` so legacy
/// payloads without the field deserialize cleanly.
///
/// [`AgentConfig`]: ../../cyberclaw_agent_runtime/config/struct.AgentConfig.html
/// [`AgentSpec`]: crate::manifests::AgentSpec
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionContext {
    /// Optional per-request override of the agent's default skill bindings.
    ///
    /// `None` -> use the agent's manifest-declared defaults.
    /// `Some(vec![])` -> explicitly clear bindings for this request.
    /// `Some(non_empty)` -> override; the loop ignores the agent default.
    #[serde(default)]
    pub runtime_skill_bindings: Option<Vec<SkillId>>,
}

impl ExecutionContext {
    /// Construct an empty context (equivalent to `Default::default()`).
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder helper to attach runtime skill bindings.
    pub fn with_runtime_skill_bindings(mut self, bindings: Vec<SkillId>) -> Self {
        self.runtime_skill_bindings = Some(bindings);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_tokens_accumulates_input_and_output() {
        let mut budget = ExecutionBudget::default();
        budget.record_tokens(100, 50);
        budget.record_tokens(200, 75);
        assert_eq!(budget.tokens_used, 425);
    }

    #[test]
    fn record_tokens_saturates_on_overflow() {
        let mut budget = ExecutionBudget {
            tokens_used: u32::MAX - 5,
            ..Default::default()
        };
        budget.record_tokens(10, 10);
        assert_eq!(budget.tokens_used, u32::MAX);
    }

    #[test]
    fn is_token_exceeded_returns_false_when_no_cap() {
        let budget = ExecutionBudget {
            max_tokens: None,
            tokens_used: 1_000_000,
            ..Default::default()
        };
        assert!(!budget.is_token_exceeded());
    }

    #[test]
    fn is_token_exceeded_returns_false_when_under_cap() {
        let budget = ExecutionBudget {
            max_tokens: Some(1000),
            tokens_used: 999,
            ..Default::default()
        };
        assert!(!budget.is_token_exceeded());
    }

    #[test]
    fn is_token_exceeded_returns_true_at_or_above_cap() {
        let mut budget = ExecutionBudget {
            max_tokens: Some(1000),
            tokens_used: 1000,
            ..Default::default()
        };
        assert!(budget.is_token_exceeded());
        budget.tokens_used = 1500;
        assert!(budget.is_token_exceeded());
    }

    #[test]
    fn tokens_used_serde_default_for_legacy_payloads() {
        // Sprint 10 backward-compat: a JSON payload from before this field
        // existed must deserialize with tokens_used = 0.
        let legacy =
            r#"{"max_steps":null,"max_duration_ms":null,"max_tokens":1000,"max_children":null}"#;
        let budget: ExecutionBudget = serde_json::from_str(legacy).unwrap();
        assert_eq!(budget.tokens_used, 0);
        assert_eq!(budget.max_tokens, Some(1000));
    }

    // ---- ExecutionContext (Sprint 44+1) ----

    #[test]
    fn execution_context_default_has_no_runtime_bindings() {
        let ctx = ExecutionContext::default();
        assert!(ctx.runtime_skill_bindings.is_none());
    }

    #[test]
    fn execution_context_serde_default_for_empty_payload() {
        // Backward-compat: an empty JSON object must deserialize cleanly,
        // leaving runtime_skill_bindings = None.
        let ctx: ExecutionContext = serde_json::from_str("{}").unwrap();
        assert!(ctx.runtime_skill_bindings.is_none());
    }

    #[test]
    fn execution_context_with_runtime_skill_bindings_overrides_default() {
        let sk = SkillId::from_string("powerpoint".to_string()).expect("valid skill id");
        let ctx = ExecutionContext::new().with_runtime_skill_bindings(vec![sk.clone()]);
        let bindings = ctx.runtime_skill_bindings.as_ref().expect("set above");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].as_str(), "powerpoint");
    }

    #[test]
    fn execution_context_serde_roundtrip_carries_bindings() {
        let sk = SkillId::from_string("powerpoint".to_string()).expect("valid skill id");
        let ctx = ExecutionContext::new().with_runtime_skill_bindings(vec![sk]);
        let json = serde_json::to_string(&ctx).expect("serialize");
        let back: ExecutionContext = serde_json::from_str(&json).expect("deserialize");
        let bindings = back.runtime_skill_bindings.expect("present");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].as_str(), "powerpoint");
    }

    #[test]
    fn execution_context_explicit_empty_bindings_is_distinct_from_none() {
        // Sprint 44+1 contract: Some(vec![]) means "explicitly clear" — the
        // loop must not fall back to agent defaults; None means "no
        // override". This regression test pins that semantic.
        let ctx_none = ExecutionContext::default();
        let ctx_empty = ExecutionContext::new().with_runtime_skill_bindings(Vec::new());
        assert!(ctx_none.runtime_skill_bindings.is_none());
        assert_eq!(
            ctx_empty.runtime_skill_bindings.as_ref().map(|v| v.len()),
            Some(0)
        );
    }
}
