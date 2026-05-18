//! 测试辅助函数

use cyberclaw_connectors::types::CapabilityExecutionRequest;
use cyberclaw_core::prelude::*;

#[allow(dead_code)]
pub fn create_test_request(
    execution_id: &str,
    connector_id: ConnectorId,
    capability_id: &str,
    input: serde_json::Value,
) -> CapabilityExecutionRequest {
    CapabilityExecutionRequest {
        execution_id: ExecutionId::from_string(execution_id.to_string())
            .expect("Valid execution ID"),
        trace_id: format!("trace-{}", execution_id),
        actor: ActorRef {
            id: ActorId::from_string("test-actor".to_string()).expect("Valid actor ID"),
            actor_type: ActorType::System,
            tenant_id: None,
            home_node_id: None,
            display_name: "test-actor".to_string(),
        },
        workspace: WorkspaceRef {
            id: WorkspaceId::from_string("test-workspace".to_string()).expect("Valid workspace ID"),
            mode: WorkspaceMode::Ephemeral,
            materialization_mode: None,
            home_node_id: None,
            backing_store: None,
            root: "/tmp/test-workspace".to_string(),
            writable_roots: vec![],
        },
        connector_id,
        capability_id: CapabilityId::from_string(capability_id.to_string())
            .expect("Valid capability ID"),
        input,
    }
}
