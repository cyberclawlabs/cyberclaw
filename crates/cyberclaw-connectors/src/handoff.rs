//! `HandoffConnector` — agent.handoff capability implementation.
//!
//! Sprint 21 T7. Wraps a [`HandoffSink`] (queue backend) and a
//! [`ToolOutputSanitizer`] (briefing hygiene) behind the [`Connector`] trait so
//! that the capability dispatch path (`gateway_impl`) can route `agent.handoff`
//! capability calls uniformly.
//!
//! # Circular-dep resolution
//!
//! `cyberclaw-control-plane` depends on `cyberclaw-connectors`, so we cannot
//! import `control-plane::HandoffQueue` here.  Instead we define the minimal
//! [`HandoffSink`] trait in this crate.  The control-plane can provide a thin
//! adapter `impl HandoffSink for Arc<dyn HandoffQueue>` when wiring the server.
//!
//! # Policy integration (S24)
//!
//! `execute_handoff` evaluates the `agent.handoff` capability via `PolicyEngine`
//! before enqueuing. Three branches:
//! - `Allow` → enqueue + return `{ status: "initiated" }`
//! - `ReviewRequired` → enqueue handoff (Initiated) + enqueue review via
//!   [`ReviewQueueSink`] + return `{ status: "pending_review", review_id }`
//! - `Deny` → return error with the policy reason
//!
//! # Agent-registry check
//!
//! When an [`AgentRegistry`] is wired via [`HandoffConnector::with_agent_registry`],
//! handoffs to unknown agents fail fast at enqueue time with a
//! [`HandoffError::TargetNotFound`] error.  Callers that do not wire a registry
//! (legacy / test mode) skip the check and preserve the previous v1 behaviour.
//!
//! ## Relationship to other "multi-agent" primitives in this workspace
//!
//! This module is **one of three** distinct multi-agent mechanisms. They
//! have overlapping keywords but solve different problems:
//!
//! | Primitive | Crate / File | Semantics |
//! |---|---|---|
//! | `SubAgentOrchestrator` | `cyberclaw-agent-runtime/src/sub_agent.rs` | Parent agent calls a child agent **as a tool**; child returns a result; parent resumes its own loop. Depth ≤ 3, max children 5. |
//! | `StageHandoffDocument` | `cyberclaw-control-plane/src/stage_handoff.rs` | Pipeline stage-to-stage paseo-style **markdown briefing** (≤ 30 lines) persisted to `<root>/<pipeline_id>/<from_stage>.md`. Linear stage hop, not live. |
//! | `HandoffConnector` | `cyberclaw-connectors/src/handoff.rs` | **Live conversational** transfer (Sprint 21): user is talking to agent_A; agent_A `agent.handoff` capability hands the conversation permanently to agent_B, with a briefing ≤ 2KB. |
//!
//! **This module is the `HandoffConnector` (live conversational peer-to-peer transfer).** If that
//! isn't what you want, see the matching row above.

use std::sync::Arc;

use anyhow::{anyhow, Context as _};
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use cyberclaw_core::agent::AgentRegistry;
use cyberclaw_core::artifact::ArtifactRef;
use cyberclaw_core::handoff::{
    HandoffError, HandoffRequest, HandoffStatus, HANDOFF_BRIEFING_MAX_BYTES,
};
use cyberclaw_core::ids::{AgentId, ConnectorId, HandoffId};
use cyberclaw_core::manifests::{CapabilityContract, ConnectorRuntime};
use cyberclaw_core::prelude::CapabilityEffect;
use cyberclaw_core::prelude::{CapabilityRef, RiskLevel};
use cyberclaw_governance::decision::GovernanceDecision;
use cyberclaw_governance::engine::{EvaluationContext, PolicyEngine};
use cyberclaw_governance::tool_output_sanitizer::ToolOutputSanitizer;

use crate::types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, Connector, ExecutionStatus,
};

// ---------------------------------------------------------------------------
// HandoffSink trait
// ---------------------------------------------------------------------------

/// Minimal queue interface used by [`HandoffConnector`].
///
/// Defined here to avoid a circular dependency with `cyberclaw-control-plane`.
/// The control-plane crate provides an adapter that forwards calls to
/// `InMemoryHandoffQueue::enqueue`.
#[async_trait]
pub trait HandoffSink: Send + Sync + std::fmt::Debug {
    /// Enqueue a validated [`HandoffRequest`].
    async fn enqueue(&self, req: HandoffRequest) -> Result<(), HandoffError>;
}

// ---------------------------------------------------------------------------
// ReviewQueueSink trait
// ---------------------------------------------------------------------------

/// Trait for capability dispatchers that need to enqueue review requests.
///
/// Mirror of [`HandoffSink`] cross-crate inversion (S21 T7) and
/// [`HandoffCompletionSink`] (S22 T2): the actual `ReviewQueue` lives in
/// `cyberclaw-control-plane`, but `HandoffConnector` (this crate) doesn't
/// depend on control-plane. Implementors (typically server's `AppState`
/// adapter) wrap a `ReviewQueue` and call `ReviewRequest::for_handoff(...)`.
#[async_trait::async_trait]
pub trait ReviewQueueSink: Send + Sync + std::fmt::Debug {
    /// Create + enqueue a review for a handoff request. Returns Ok on success.
    /// Implementors:
    ///   1. Build a `ReviewRequest::for_handoff(review_id, handoff_id, ...)`.
    ///   2. Call the underlying `ReviewQueue::enqueue(req)`.
    async fn enqueue_review_for_handoff(
        &self,
        review_id: cyberclaw_core::ids::ReviewId,
        handoff_id: HandoffId,
        title: String,
        summary: String,
        requested_by: cyberclaw_core::identity::ActorRef,
    ) -> anyhow::Result<()>;
}

/// No-op implementation for tests / unopinionated callers.
/// Logs a warning when invoked but doesn't actually enqueue.
#[derive(Debug)]
pub struct NoopReviewQueueSink;

#[async_trait::async_trait]
impl ReviewQueueSink for NoopReviewQueueSink {
    async fn enqueue_review_for_handoff(
        &self,
        _review_id: cyberclaw_core::ids::ReviewId,
        _handoff_id: HandoffId,
        _title: String,
        _summary: String,
        _requested_by: cyberclaw_core::identity::ActorRef,
    ) -> anyhow::Result<()> {
        tracing::warn!(
            target: "cyberclaw::handoff",
            "NoopReviewQueueSink: review enqueue called but no real sink configured (S24 wire-up missing?)"
        );
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Input schema (parsed from CapabilityExecutionRequest.input)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct HandoffInput {
    /// Agent ID that is requesting the handoff (the sender).
    from_agent_id: String,
    /// Target agent ID (the receiver).
    to_agent_id: String,
    /// Conversation this handoff belongs to.
    conversation_id: String,
    /// Human-readable one-line reason (≤ 200 chars, no newlines).
    reason: String,
    /// Optional briefing text for the receiving agent (≤ 2 KB).
    #[serde(default)]
    briefing_text: String,
    /// Optional artifact references to pass along.
    #[serde(default)]
    context_artifacts: Vec<ArtifactRef>,
}

// ---------------------------------------------------------------------------
// HandoffOutput (serialized into CapabilityExecutionResult.output)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct HandoffOutput {
    handoff_id: String,
    status: HandoffStatus,
}

// ---------------------------------------------------------------------------
// HandoffConnector
// ---------------------------------------------------------------------------

/// Connector that exposes the `agent.handoff` capability.
///
/// Dispatch logic (S24):
/// 1. Parse + validate input
/// 2. Sanitize briefing
/// 3. Build `HandoffRequest` (status = Initiated)
/// 4. Evaluate via `PolicyEngine` — three branches:
///    - `Allow` → enqueue + `{ status: "initiated" }`
///    - `ReviewRequired` → enqueue handoff + enqueue review + `{ status: "pending_review", review_id }`
///    - `Deny` → return error with policy reason
pub struct HandoffConnector {
    id: ConnectorId,
    sink: Arc<dyn HandoffSink>,
    sanitizer: Arc<ToolOutputSanitizer>,
    /// Optional agent registry for existence checks. When `None`, the check is
    /// skipped (backward-compatible / legacy mode). Wire via
    /// [`HandoffConnector::with_agent_registry`].
    agent_registry: Option<Arc<dyn AgentRegistry>>,
    /// Policy engine for capability evaluation (S24).
    policy_engine: Arc<dyn PolicyEngine>,
    /// Review queue sink for enqueuing human-review requests (S24).
    review_sink: Arc<dyn ReviewQueueSink>,
    /// Cached capability list (built once at construction).
    capabilities: Vec<CapabilityContract>,
}

impl std::fmt::Debug for HandoffConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HandoffConnector")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl HandoffConnector {
    /// Construct a new connector.
    ///
    /// The agent-registry check is **not** wired by default (backward-compatible
    /// mode). Use [`HandoffConnector::with_agent_registry`] to enable it.
    ///
    /// # Arguments
    ///
    /// - `id` — connector identifier
    /// - `sink` — queue backend for enqueuing handoff requests
    /// - `sanitizer` — briefing hygiene (credential redaction + injection detection)
    /// - `policy_engine` — capability governance evaluation (S24)
    /// - `review_sink` — review queue backend for ReviewRequired path (S24)
    pub fn new(
        id: ConnectorId,
        sink: Arc<dyn HandoffSink>,
        sanitizer: Arc<ToolOutputSanitizer>,
        policy_engine: Arc<dyn PolicyEngine>,
        review_sink: Arc<dyn ReviewQueueSink>,
    ) -> Self {
        let capabilities = vec![CapabilityContract {
            id: "agent.handoff".to_string(),
            title: "Agent Handoff".to_string(),
            description: Some(
                "Transfer conversation control from the current agent to another agent."
                    .to_string(),
            ),
            input_schema: r#"{"type":"object","required":["from_agent_id","to_agent_id","conversation_id","reason"],"properties":{"from_agent_id":{"type":"string"},"to_agent_id":{"type":"string"},"conversation_id":{"type":"string"},"reason":{"type":"string","maxLength":200},"briefing_text":{"type":"string","maxLength":2048},"context_artifacts":{"type":"array"}}}"#.to_string(),
            output_schema: r#"{"type":"object","required":["handoff_id","status"],"properties":{"handoff_id":{"type":"string"},"status":{"type":"string"}}}"#.to_string(),
            risk: RiskLevel::High,
            effects: vec![CapabilityEffect::Write, CapabilityEffect::Notification],
            placement: None,
            timeouts: Default::default(),
        }];

        Self {
            id,
            sink,
            sanitizer,
            agent_registry: None,
            policy_engine,
            review_sink,
            capabilities,
        }
    }

    /// Wire an [`AgentRegistry`] so that handoffs to unknown agents fail fast
    /// at enqueue time.
    ///
    /// When no registry is wired (the default), the existence check is skipped.
    pub fn with_agent_registry(mut self, registry: Arc<dyn AgentRegistry>) -> Self {
        self.agent_registry = Some(registry);
        self
    }
}

#[async_trait]
impl Connector for HandoffConnector {
    fn id(&self) -> &ConnectorId {
        &self.id
    }

    fn runtime(&self) -> ConnectorRuntime {
        ConnectorRuntime::Native
    }

    fn capabilities(&self) -> Vec<CapabilityContract> {
        self.capabilities.clone()
    }

    async fn execute(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<CapabilityExecutionResult> {
        if request.capability_id.as_ref() != "agent.handoff" {
            let cap_id_str = request.capability_id.to_string();
            return Ok(CapabilityExecutionResult {
                execution_id: request.execution_id,
                trace_id: request.trace_id,
                connector_id: self.id.clone(),
                capability_id: request.capability_id,
                output: serde_json::Value::Null,
                status: ExecutionStatus::Failed,
                error: Some(format!("Unknown capability: {cap_id_str}")),
                actual_runtime: Some(crate::runtime::RuntimeMode::Native),
            });
        }

        self.execute_handoff(request).await
    }
}

impl HandoffConnector {
    async fn execute_handoff(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<CapabilityExecutionResult> {
        // 1. Parse input
        let input: HandoffInput = serde_json::from_value(request.input.clone())
            .context("Invalid input for agent.handoff")?;

        // 2. Agent-existence check via AgentRegistry (when wired).
        //    Parse to AgentId first so we can call exists(); empty string also fails parse.
        let to_agent_id_parsed = AgentId::from_string(input.to_agent_id.clone())
            .with_context(|| format!("Invalid to_agent_id: {}", input.to_agent_id))?;
        if let Some(registry) = &self.agent_registry {
            if !registry.exists(&to_agent_id_parsed).await {
                return Err(anyhow!(HandoffError::TargetNotFound(
                    input.to_agent_id.clone()
                )));
            }
        }

        // 3. Sanitize briefing_text (credential redaction + injection detection)
        let sanitized = self
            .sanitizer
            .sanitize("agent.handoff", &input.briefing_text);
        let clean_briefing = sanitized.content;

        // 4. Guard: sanitized briefing must still fit within the byte cap
        //    (sanitizer may expand text slightly via [REDACTED] markers).
        if clean_briefing.len() > HANDOFF_BRIEFING_MAX_BYTES {
            return Err(anyhow!(HandoffError::Validation(format!(
                "briefing_text exceeds {} bytes after sanitization (got {})",
                HANDOFF_BRIEFING_MAX_BYTES,
                clean_briefing.len()
            ))));
        }

        // 5. Build HandoffRequest
        let from_agent_id = AgentId::from_string(input.from_agent_id.clone())
            .with_context(|| format!("Invalid from_agent_id: {}", input.from_agent_id))?;
        // to_agent_id already parsed and existence-checked in step 2.
        let to_agent_id = to_agent_id_parsed;
        let handoff_id = HandoffId::from_string(format!("ho_{}", Uuid::new_v4().simple()))
            .context("Failed to generate handoff ID")?;

        let req = HandoffRequest::new(
            handoff_id,
            from_agent_id,
            to_agent_id,
            input.conversation_id,
            input.reason,
            clean_briefing,
            input.context_artifacts,
            request.execution_id.clone().into(),
            Utc::now(),
        );

        // 6. Validate invariants (self-handoff, reason length, TTL, etc.)
        req.validate().map_err(anyhow::Error::from)?;

        // 7. PolicyEngine evaluation (S24)
        let eval_ctx = EvaluationContext {
            capability: CapabilityRef {
                id: cyberclaw_core::ids::CapabilityId::from_string("agent.handoff".to_string())
                    .expect("static capability id is always valid"),
                connector_id: self.id.clone(),
                risk: RiskLevel::High,
                effects: vec![CapabilityEffect::Write, CapabilityEffect::Notification],
                placement: None,
            },
            actor: request.actor.clone(),
            execution_id: request.execution_id.clone(),
            reason: Some(req.reason.clone()),
        };
        let eval = self
            .policy_engine
            .evaluate_capability(eval_ctx)
            .await
            .map_err(|e| anyhow!("policy evaluation failed: {e}"))?;

        let handoff_id_str = req.handoff_id.to_string();
        let to_agent_id_str = req.to_agent_id.to_string();

        match eval.decision {
            GovernanceDecision::Allow { .. } => {
                // 8a. Allow path: enqueue and return initiated status
                self.sink.enqueue(req).await.map_err(anyhow::Error::from)?;

                let output = serde_json::to_value(HandoffOutput {
                    handoff_id: handoff_id_str,
                    status: HandoffStatus::Initiated,
                })
                .context("Failed to serialize handoff output")?;

                Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: self.id.clone(),
                    capability_id: request.capability_id,
                    output,
                    status: ExecutionStatus::Success,
                    error: None,
                    actual_runtime: Some(crate::runtime::RuntimeMode::Native),
                })
            }
            GovernanceDecision::ReviewRequired { reason, .. } => {
                // 8b. ReviewRequired path: enqueue handoff (Initiated) AND review
                self.sink.enqueue(req).await.map_err(anyhow::Error::from)?;

                let review_id = cyberclaw_core::ids::ReviewId::new();
                self.review_sink
                    .enqueue_review_for_handoff(
                        review_id.clone(),
                        HandoffId::from_string(handoff_id_str.clone())
                            .context("Failed to reconstruct handoff ID for review")?,
                        format!("Handoff to {} requires approval", to_agent_id_str),
                        reason,
                        request.actor.clone(),
                    )
                    .await
                    .map_err(|e| anyhow!("review enqueue failed: {e}"))?;

                let output = serde_json::json!({
                    "handoff_id": handoff_id_str,
                    "status": "pending_review",
                    "review_id": review_id.to_string(),
                });

                Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: self.id.clone(),
                    capability_id: request.capability_id,
                    output,
                    status: ExecutionStatus::Success,
                    error: None,
                    actual_runtime: Some(crate::runtime::RuntimeMode::Native),
                })
            }
            GovernanceDecision::Deny { reason } => {
                // 8c. Deny path: return error with policy reason
                Err(anyhow!("handoff denied by policy: {}", reason))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::ids::{CapabilityId, ExecutionId, ReviewId, WorkspaceId};
    use cyberclaw_core::prelude::{ActorRef, ActorType, WorkspaceRef};
    use cyberclaw_governance::engine::{EvaluationResult, NoopPolicyEngine};

    // ── Mock handoff sink ────────────────────────────────────────────────────

    #[derive(Debug, Default)]
    struct MockSink {
        captures: std::sync::Mutex<Vec<HandoffRequest>>,
        fail: bool,
    }

    impl MockSink {
        fn capturing() -> Arc<Self> {
            Arc::new(Self::default())
        }

        #[allow(dead_code)]
        fn failing() -> Arc<Self> {
            Arc::new(Self {
                fail: true,
                ..Default::default()
            })
        }

        fn captured(&self) -> Vec<HandoffRequest> {
            self.captures.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl HandoffSink for MockSink {
        async fn enqueue(&self, req: HandoffRequest) -> Result<(), HandoffError> {
            if self.fail {
                return Err(HandoffError::Internal("mock failure".to_string()));
            }
            self.captures.lock().unwrap().push(req);
            Ok(())
        }
    }

    // ── Mock review sink ─────────────────────────────────────────────────────

    #[derive(Debug, Default)]
    struct MockReviewSink {
        captures: std::sync::Mutex<Vec<(ReviewId, HandoffId)>>,
    }

    impl MockReviewSink {
        fn capturing() -> Arc<Self> {
            Arc::new(Self::default())
        }

        fn captured(&self) -> Vec<(ReviewId, HandoffId)> {
            self.captures.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ReviewQueueSink for MockReviewSink {
        async fn enqueue_review_for_handoff(
            &self,
            review_id: ReviewId,
            handoff_id: HandoffId,
            _title: String,
            _summary: String,
            _requested_by: cyberclaw_core::identity::ActorRef,
        ) -> anyhow::Result<()> {
            self.captures.lock().unwrap().push((review_id, handoff_id));
            Ok(())
        }
    }

    // ── Mock policy engines ──────────────────────────────────────────────────

    #[derive(Debug)]
    struct MockPolicyEngine {
        decision: GovernanceDecision,
    }

    #[async_trait]
    impl PolicyEngine for MockPolicyEngine {
        async fn evaluate_capability(
            &self,
            context: EvaluationContext,
        ) -> anyhow::Result<EvaluationResult> {
            Ok(EvaluationResult {
                decision: self.decision.clone(),
                evaluated_risk: context.capability.risk,
                context_info: vec![],
            })
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn make_connector(sink: Arc<dyn HandoffSink>) -> HandoffConnector {
        HandoffConnector::new(
            ConnectorId::from_string("handoff-connector".to_string()).unwrap(),
            sink,
            Arc::new(ToolOutputSanitizer::new()),
            Arc::new(NoopPolicyEngine),
            Arc::new(NoopReviewQueueSink),
        )
    }

    fn make_request(input: serde_json::Value) -> CapabilityExecutionRequest {
        CapabilityExecutionRequest {
            execution_id: ExecutionId::from_string("exec_01".to_string()).unwrap(),
            trace_id: "trace_01".to_string(),
            actor: ActorRef {
                id: cyberclaw_core::ids::ActorId::from_string("actor_a".to_string()).unwrap(),
                actor_type: ActorType::Agent,
                tenant_id: None,
                home_node_id: None,
                display_name: "Agent A".to_string(),
            },
            workspace: WorkspaceRef {
                id: WorkspaceId::from_string("ws_01".to_string()).unwrap(),
                mode: cyberclaw_core::workspace::WorkspaceMode::Ephemeral,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: "/tmp/ws".to_string(),
                writable_roots: vec![],
            },
            connector_id: ConnectorId::from_string("handoff-connector".to_string()).unwrap(),
            capability_id: CapabilityId::from_string("agent.handoff".to_string()).unwrap(),
            input,
        }
    }

    fn valid_input() -> serde_json::Value {
        serde_json::json!({
            "from_agent_id": "agent_a",
            "to_agent_id": "agent_b",
            "conversation_id": "conv_01",
            "reason": "B specializes in backend tasks",
            "briefing_text": "The user wants schema review. PRD at /docs/prd.md",
            "context_artifacts": []
        })
    }

    // ── Test 1: happy path ────────────────────────────────────────────────────

    #[tokio::test]
    async fn execute_happy_path_enqueues_valid_request() {
        let sink = MockSink::capturing();
        let connector = make_connector(sink.clone());
        let req = make_request(valid_input());

        let result = connector.execute(req).await.unwrap();
        assert_eq!(result.status, ExecutionStatus::Success);
        assert!(result.error.is_none());

        let output: serde_json::Value = result.output;
        assert_eq!(output["status"], "initiated");
        assert!(output["handoff_id"].as_str().unwrap().starts_with("ho_"));

        let captured = sink.captured();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].status, HandoffStatus::Initiated);
        assert_eq!(captured[0].from_agent_id.to_string(), "agent_a");
        assert_eq!(captured[0].to_agent_id.to_string(), "agent_b");
    }

    // ── Test 2: briefing sanitization ────────────────────────────────────────

    #[tokio::test]
    async fn execute_sanitizes_briefing() {
        let sink = MockSink::capturing();
        let connector = make_connector(sink.clone());

        let mut input = valid_input();
        // Use a value that matches the openai_api_key pattern: sk- + ≥20 alphanumeric chars
        input["briefing_text"] = serde_json::Value::String(
            "context: sk-ABCDEFGHIJ1234567890XYZ let me explain".to_string(),
        );

        let req = make_request(input);
        let result = connector.execute(req).await.unwrap();
        assert_eq!(result.status, ExecutionStatus::Success);

        let captured = sink.captured();
        assert_eq!(captured.len(), 1);
        // Credential should be redacted
        assert!(
            !captured[0]
                .briefing_text
                .contains("sk-ABCDEFGHIJ1234567890XYZ"),
            "briefing should not contain raw secret; got: {}",
            captured[0].briefing_text
        );
        assert!(
            captured[0].briefing_text.contains("[REDACTED]"),
            "briefing should contain [REDACTED]; got: {}",
            captured[0].briefing_text
        );
    }

    // ── Test 3: self-handoff rejected ────────────────────────────────────────

    #[tokio::test]
    async fn execute_validates_input_rejects_self_handoff() {
        let sink = MockSink::capturing();
        let connector = make_connector(sink.clone());

        let mut input = valid_input();
        input["to_agent_id"] = serde_json::Value::String("agent_a".to_string());

        let req = make_request(input);
        let err = connector.execute(req).await.unwrap_err();
        assert!(
            err.to_string().contains("self-handoff"),
            "expected self-handoff error, got: {err}"
        );
        assert!(sink.captured().is_empty());
    }

    // ── Test 4: empty reason rejected ────────────────────────────────────────

    #[tokio::test]
    async fn execute_validates_input_rejects_empty_reason() {
        let sink = MockSink::capturing();
        let connector = make_connector(sink.clone());

        let mut input = valid_input();
        input["reason"] = serde_json::Value::String("   ".to_string());

        let req = make_request(input);
        let err = connector.execute(req).await.unwrap_err();
        assert!(
            err.to_string().contains("reason"),
            "expected reason validation error, got: {err}"
        );
        assert!(sink.captured().is_empty());
    }

    // ── Test 5: oversized briefing rejected ──────────────────────────────────

    #[tokio::test]
    async fn execute_rejects_oversized_briefing() {
        let sink = MockSink::capturing();
        let connector = make_connector(sink.clone());

        let mut input = valid_input();
        input["briefing_text"] =
            serde_json::Value::String("x".repeat(HANDOFF_BRIEFING_MAX_BYTES + 1));

        let req = make_request(input);
        let err = connector.execute(req).await.unwrap_err();
        assert!(
            err.to_string().contains("briefing_text"),
            "expected briefing_text validation error, got: {err}"
        );
        assert!(sink.captured().is_empty());
    }

    // ── Test 6: capabilities returns agent.handoff ────────────────────────────

    #[test]
    fn capabilities_returns_agent_handoff() {
        let connector = make_connector(MockSink::capturing());
        let caps = connector.capabilities();
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].id, "agent.handoff");
    }

    // ── Test 7: connector id is stable ───────────────────────────────────────

    #[test]
    fn connector_id_is_stable() {
        let connector = make_connector(MockSink::capturing());
        assert_eq!(connector.id().to_string(), "handoff-connector");
    }

    // ── Test 8: runtime matches expected variant ──────────────────────────────

    #[test]
    fn runtime_matches_expected_variant() {
        let connector = make_connector(MockSink::capturing());
        assert_eq!(connector.runtime(), ConnectorRuntime::Native);
    }

    // ── Test 9 (S24): Allow decision enqueues sink, status=initiated ──────────

    #[tokio::test]
    async fn execute_with_allow_decision_calls_sink() {
        let sink = MockSink::capturing();
        let review_sink = MockReviewSink::capturing();
        let connector = HandoffConnector::new(
            ConnectorId::from_string("handoff-connector".to_string()).unwrap(),
            sink.clone(),
            Arc::new(ToolOutputSanitizer::new()),
            Arc::new(NoopPolicyEngine), // always Allow
            review_sink.clone(),
        );

        let result = connector
            .execute(make_request(valid_input()))
            .await
            .unwrap();
        assert_eq!(result.status, ExecutionStatus::Success);
        assert_eq!(result.output["status"], "initiated");
        assert!(result.output["handoff_id"]
            .as_str()
            .unwrap()
            .starts_with("ho_"));

        // sink received exactly 1 handoff
        assert_eq!(sink.captured().len(), 1);
        // review sink received nothing
        assert_eq!(review_sink.captured().len(), 0);
    }

    // ── Test 10 (S24): ReviewRequired enqueues both sinks, status=pending_review

    #[tokio::test]
    async fn execute_with_review_required_calls_review_sink() {
        use cyberclaw_governance::decision::ReviewType;

        let sink = MockSink::capturing();
        let review_sink = MockReviewSink::capturing();
        let connector = HandoffConnector::new(
            ConnectorId::from_string("handoff-connector".to_string()).unwrap(),
            sink.clone(),
            Arc::new(ToolOutputSanitizer::new()),
            Arc::new(MockPolicyEngine {
                decision: GovernanceDecision::review_required(
                    "High risk capability",
                    ReviewType::Human,
                ),
            }),
            review_sink.clone(),
        );

        let result = connector
            .execute(make_request(valid_input()))
            .await
            .unwrap();
        assert_eq!(result.status, ExecutionStatus::Success);
        assert_eq!(result.output["status"], "pending_review");
        // review_id field must be present and non-empty
        assert!(result.output["review_id"]
            .as_str()
            .map(|s| !s.is_empty())
            .unwrap_or(false));
        // handoff_id still present
        assert!(result.output["handoff_id"]
            .as_str()
            .unwrap()
            .starts_with("ho_"));

        // handoff sink enqueued (stays in Initiated)
        assert_eq!(sink.captured().len(), 1);
        // review sink also received exactly 1 entry
        let reviews = review_sink.captured();
        assert_eq!(reviews.len(), 1);
        // review_id in output matches what was passed to review sink
        let output_review_id = result.output["review_id"].as_str().unwrap();
        assert_eq!(reviews[0].0.to_string(), output_review_id);
    }

    // ── Test 11 (S24): Deny returns error with rationale ─────────────────────

    #[tokio::test]
    async fn execute_with_deny_returns_error() {
        let sink = MockSink::capturing();
        let review_sink = MockReviewSink::capturing();
        let connector = HandoffConnector::new(
            ConnectorId::from_string("handoff-connector".to_string()).unwrap(),
            sink.clone(),
            Arc::new(ToolOutputSanitizer::new()),
            Arc::new(MockPolicyEngine {
                decision: GovernanceDecision::deny("agent.handoff not permitted for this actor"),
            }),
            review_sink.clone(),
        );

        let err = connector
            .execute(make_request(valid_input()))
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("handoff denied by policy"),
            "expected denial message, got: {msg}"
        );
        assert!(
            msg.contains("agent.handoff not permitted for this actor"),
            "expected rationale in error, got: {msg}"
        );

        // Neither sink should have been called
        assert_eq!(sink.captured().len(), 0);
        assert_eq!(review_sink.captured().len(), 0);
    }

    // ── Mock agent registry ──────────────────────────────────────────────────

    #[derive(Debug)]
    struct MockAgentRegistry {
        exists: bool,
    }

    #[async_trait]
    impl AgentRegistry for MockAgentRegistry {
        async fn exists(&self, _id: &cyberclaw_core::ids::AgentId) -> bool {
            self.exists
        }
    }

    // ── Test 12: registry says agent missing → reject ────────────────────────

    #[tokio::test]
    async fn handoff_rejects_when_registry_says_agent_missing() {
        let sink = MockSink::capturing();
        let connector = HandoffConnector::new(
            ConnectorId::from_string("handoff-connector".to_string()).unwrap(),
            sink.clone(),
            Arc::new(ToolOutputSanitizer::new()),
            Arc::new(NoopPolicyEngine),
            Arc::new(NoopReviewQueueSink),
        )
        .with_agent_registry(Arc::new(MockAgentRegistry { exists: false }));

        let err = connector
            .execute(make_request(valid_input()))
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("agent_b"),
            "error should mention the unknown agent id, got: {err}"
        );
        assert!(sink.captured().is_empty(), "sink must not be called");
    }

    // ── Test 13: registry confirms agent exists → accept ─────────────────────

    #[tokio::test]
    async fn handoff_accepts_when_registry_confirms_agent() {
        let sink = MockSink::capturing();
        let connector = HandoffConnector::new(
            ConnectorId::from_string("handoff-connector".to_string()).unwrap(),
            sink.clone(),
            Arc::new(ToolOutputSanitizer::new()),
            Arc::new(NoopPolicyEngine),
            Arc::new(NoopReviewQueueSink),
        )
        .with_agent_registry(Arc::new(MockAgentRegistry { exists: true }));

        let result = connector
            .execute(make_request(valid_input()))
            .await
            .unwrap();
        assert_eq!(result.status, ExecutionStatus::Success);
        assert_eq!(sink.captured().len(), 1);
    }

    // ── Test 14: no registry configured → skip check (backward compat) ───────

    #[tokio::test]
    async fn handoff_skips_check_when_no_registry_configured() {
        let sink = MockSink::capturing();
        // Connector built without with_agent_registry() — no check performed
        let connector = make_connector(sink.clone());

        let result = connector
            .execute(make_request(valid_input()))
            .await
            .unwrap();
        assert_eq!(result.status, ExecutionStatus::Success);
        assert_eq!(sink.captured().len(), 1);
    }
}
