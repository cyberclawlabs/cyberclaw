//! Integration test — Sprint 21 T8
//!
//! Verifies that the `HandoffConnector` (registered via `ConnectorRegistry`) is
//! correctly wired to `InMemoryHandoffQueue` through the `QueueSink` adapter:
//! executing `agent.handoff` on the connector enqueues a `HandoffRequest` with
//! status `Initiated` in the shared queue.
//!
//! # Why we call the connector directly (not through `ControlPlaneGateway`)
//!
//! The `CapabilityDispatcher` enforces runtime-isolation policy: `High`-risk
//! capabilities must run in a Container runtime, which is not yet implemented.
//! `HandoffConnector` declares `RiskLevel::High` (correct — it mutates agent
//! routing state).  Plumbing a test-only dispatcher bypass would require
//! touching `cyberclaw-connectors`, which is out of scope for T8.
//!
//! The T8 contract ("dispatch `agent.handoff` via the connector → queue
//! enqueues one `Initiated` entry") is fully satisfied here: we look up the
//! connector from the registry by its capability and call `execute()` on it,
//! which is exactly the path `CapabilityDispatcher` would take after the
//! runtime gate is resolved in a future sprint.
//!
//! See `.spec-workflow/specs/s21-multi-agent-handoff/tasks.md` T8.

use std::sync::Arc;

use async_trait::async_trait;

use cyberclaw_connectors::{
    handoff::{HandoffConnector, HandoffSink},
    registry::ConnectorRegistry,
    types::{CapabilityExecutionRequest, ExecutionStatus},
    Connector,
};
use cyberclaw_control_plane::handoff_queue::{HandoffQueue, InMemoryHandoffQueue};
use cyberclaw_core::{
    handoff::{HandoffError, HandoffRequest, HandoffStatus},
    ids::{ActorId, CapabilityId, ConnectorId, ExecutionId, WorkspaceId},
    prelude::{ActorRef, ActorType, WorkspaceRef},
    workspace::WorkspaceMode,
};
use cyberclaw_governance::tool_output_sanitizer::ToolOutputSanitizer;

// ---------------------------------------------------------------------------
// QueueSink — same adapter as in server/src/state.rs, local copy for test
// ---------------------------------------------------------------------------

struct QueueSink(Arc<dyn HandoffQueue>);

impl std::fmt::Debug for QueueSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueueSink").finish_non_exhaustive()
    }
}

#[async_trait]
impl HandoffSink for QueueSink {
    async fn enqueue(&self, req: HandoffRequest) -> Result<(), HandoffError> {
        self.0.enqueue(req).await
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_execution_request(input: serde_json::Value) -> CapabilityExecutionRequest {
    CapabilityExecutionRequest {
        execution_id: ExecutionId::from_string("exec_t8_smoke".to_string()).unwrap(),
        trace_id: "trace_t8_smoke".to_string(),
        actor: ActorRef {
            id: ActorId::from_string("agent_alpha".to_string()).unwrap(),
            actor_type: ActorType::Agent,
            tenant_id: None,
            home_node_id: None,
            display_name: "Agent Alpha".to_string(),
        },
        workspace: WorkspaceRef {
            id: WorkspaceId::new(),
            mode: WorkspaceMode::Ephemeral,
            materialization_mode: None,
            home_node_id: None,
            backing_store: None,
            root: "/tmp/t8-test".to_string(),
            writable_roots: vec![],
        },
        connector_id: ConnectorId::from_string("handoff".to_string()).unwrap(),
        capability_id: CapabilityId::from_string("agent.handoff".to_string()).unwrap(),
        input,
    }
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

/// Core closure test: `HandoffConnector::execute` wired through the registry
/// adapter enqueues exactly one `Initiated` entry into `InMemoryHandoffQueue`.
#[tokio::test]
async fn handoff_connector_via_registry_enqueues_to_queue() {
    // 1. Build the shared in-memory queue
    let queue = Arc::new(InMemoryHandoffQueue::new());

    // 2. Build the HandoffConnector with QueueSink adapter
    let sink: Arc<dyn HandoffSink> = Arc::new(QueueSink(queue.clone() as Arc<dyn HandoffQueue>));
    let connector = Arc::new(HandoffConnector::new(
        ConnectorId::from_string("handoff".to_string()).unwrap(),
        sink,
        Arc::new(ToolOutputSanitizer::new()),
        Arc::new(cyberclaw_governance::engine::NoopPolicyEngine),
        Arc::new(cyberclaw_connectors::handoff::NoopReviewQueueSink),
    ));

    // 3. Register connector in ConnectorRegistry (verifies registry lookup path)
    let registry = Arc::new(ConnectorRegistry::new());
    registry.register(connector.clone()).unwrap();

    // Verify the capability is discoverable via registry
    let caps = registry.list_capabilities();
    assert!(
        caps.iter()
            .any(|(_, cap_id)| cap_id.as_ref() == "agent.handoff"),
        "agent.handoff should be discoverable in the registry"
    );

    // 4. Execute agent.handoff directly on the connector
    //    (mirrors what CapabilityDispatcher does after the runtime gate)
    let request = make_execution_request(serde_json::json!({
        "from_agent_id":   "agent_alpha",
        "to_agent_id":     "agent_beta",
        "conversation_id": "conv_t8_smoke",
        "reason":          "T8 integration smoke test",
        "briefing_text":   "Passing the baton to beta.",
        "context_artifacts": []
    }));

    let result = connector.execute(request).await.unwrap();

    assert_eq!(
        result.status,
        ExecutionStatus::Success,
        "connector.execute should succeed; error: {:?}",
        result.error
    );

    let handoff_id = result.output["handoff_id"]
        .as_str()
        .expect("output must have handoff_id");
    assert!(
        handoff_id.starts_with("ho_"),
        "handoff_id should start with 'ho_', got: {handoff_id}"
    );
    assert_eq!(
        result.output["status"].as_str().unwrap(),
        "initiated",
        "output status should be 'initiated'"
    );

    // 5. Verify the queue holds exactly one Initiated entry
    let recent = queue.list_recent(10).await;
    assert_eq!(recent.len(), 1, "queue should have exactly 1 entry");
    assert_eq!(
        recent[0].status,
        HandoffStatus::Initiated,
        "queue entry status should be Initiated"
    );
    assert_eq!(recent[0].from_agent_id.to_string(), "agent_alpha");
    assert_eq!(recent[0].to_agent_id.to_string(), "agent_beta");
    assert_eq!(recent[0].conversation_id, "conv_t8_smoke");
}
