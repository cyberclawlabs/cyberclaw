//! Integration tests for RuntimeSelector integration in CapabilityDispatcher
//!
//! Verifies that runtime selection works correctly in the production dispatch flow,
//! including fail-fast behavior for unimplemented runtimes.

use cyberclaw_connectors::dispatcher::CapabilityDispatcher;
use cyberclaw_connectors::local::LocalConnector;
use cyberclaw_connectors::registry::ConnectorRegistry;
use cyberclaw_connectors::runtime::RuntimeSelectorConfig;
use cyberclaw_connectors::types::{
    CapabilityExecutionRequest, Connector, ExecutionStatus as ConnectorExecutionStatus,
};
use cyberclaw_core::capability::{CapabilityEffect, RiskLevel};
use cyberclaw_core::manifests::{CapabilityContract, CapabilityTimeouts, ConnectorRuntime};
use cyberclaw_core::prelude::*;
use std::sync::Arc;

/// Helper: Create a test workspace
fn create_test_workspace() -> std::path::PathBuf {
    let temp_dir = std::env::temp_dir().join(format!("cyberclaw_test_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).unwrap();
    temp_dir
}

/// Helper: Create a test connector with custom risk level capabilities
#[derive(Debug)]
struct TestConnector {
    id: ConnectorId,
    capabilities: Vec<CapabilityContract>,
}

impl TestConnector {
    fn new(id: &str, risk_level: RiskLevel, capability_id: &str) -> Self {
        let id = ConnectorId::from_string(id.to_string()).unwrap();
        let capabilities = vec![CapabilityContract {
            id: capability_id.to_string(),
            title: format!("Test {} Capability", capability_id),
            description: Some(format!("Test capability with {:?} risk", risk_level)),
            input_schema: "TestInput".to_string(),
            output_schema: "TestOutput".to_string(),
            risk: risk_level,
            effects: vec![CapabilityEffect::Read],
            placement: None,
            timeouts: CapabilityTimeouts {
                request_ms: Some(5000),
            },
        }];

        Self { id, capabilities }
    }
}

#[async_trait::async_trait]
impl Connector for TestConnector {
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
    ) -> anyhow::Result<cyberclaw_connectors::types::CapabilityExecutionResult> {
        // Echo back the input as output
        Ok(cyberclaw_connectors::types::CapabilityExecutionResult {
            execution_id: request.execution_id,
            trace_id: request.trace_id,
            connector_id: request.connector_id,
            capability_id: request.capability_id,
            output: request.input.clone(),
            status: ConnectorExecutionStatus::Success,
            error: None,
            actual_runtime: Some(cyberclaw_connectors::runtime::RuntimeMode::Native),
        })
    }
}

/// Helper: Create a basic execution request
fn create_test_request(connector_id: &str, capability_id: &str) -> CapabilityExecutionRequest {
    let workspace = create_test_workspace();

    CapabilityExecutionRequest {
        execution_id: ExecutionId::new(),
        trace_id: uuid::Uuid::new_v4().to_string(),
        actor: ActorRef {
            id: ActorId::from_string("test-actor".to_string()).unwrap(),
            actor_type: cyberclaw_core::identity::ActorType::Human,
            tenant_id: None,
            home_node_id: None,
            display_name: "Test Actor".to_string(),
        },
        workspace: WorkspaceRef {
            id: WorkspaceId::from_string("test-workspace".to_string()).unwrap(),
            mode: cyberclaw_core::workspace::WorkspaceMode::Isolated,
            materialization_mode: Some(
                cyberclaw_core::workspace::WorkspaceMaterializationMode::LocalEphemeral,
            ),
            home_node_id: None,
            backing_store: None,
            root: workspace.to_string_lossy().to_string(),
            writable_roots: vec![workspace.to_string_lossy().to_string()],
        },
        connector_id: ConnectorId::from_string(connector_id.to_string()).unwrap(),
        capability_id: CapabilityId::from_string(capability_id.to_string()).unwrap(),
        input: serde_json::json!({
            "test": "data"
        }),
    }
}

#[tokio::test]
async fn test_low_risk_uses_native() {
    // Initialize test logger
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    // 1. Create registry and register Low risk connector
    let registry = Arc::new(ConnectorRegistry::new());
    let connector = TestConnector::new("test-low", RiskLevel::Low, "test.low");
    registry.register(Arc::new(connector)).unwrap();

    // 2. Create dispatcher with default config (risk-based selection)
    let dispatcher = CapabilityDispatcher::new(registry.clone());

    // 3. Dispatch request
    let request = create_test_request("test-low", "test.low");
    let result = dispatcher.dispatch(request).await.unwrap();

    // 4. Verify success (Low risk should use Native runtime and succeed)
    assert_eq!(result.status, ConnectorExecutionStatus::Success);
    assert!(result.error.is_none());
}

#[tokio::test]
async fn test_medium_risk_routes_to_process() {
    // Initialize test logger
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    // 1. Create registry and register Medium risk connector
    let registry = Arc::new(ConnectorRegistry::new());
    let connector = TestConnector::new("test-medium", RiskLevel::Medium, "test.medium");
    registry.register(Arc::new(connector)).unwrap();

    // 2. Create dispatcher
    let dispatcher = CapabilityDispatcher::new(registry.clone());

    // 3. Dispatch request
    let request = create_test_request("test-medium", "test.medium");
    let result = dispatcher.dispatch(request).await.unwrap();

    // 4. Verify: Medium risk routes to Process runtime.
    // CRITICAL #4 FIX: Process runtime is not yet implemented, so dispatcher
    // must fail-fast instead of silently falling back to Native runtime.
    assert_eq!(result.status, ConnectorExecutionStatus::Failed);
    assert!(result.error.is_some());
    let error_msg = result.error.unwrap();
    assert!(
        error_msg.contains("Process runtime not configured"),
        "Expected fail-fast error for Medium risk capability, got: {}",
        error_msg
    );
}

#[tokio::test]
async fn test_high_risk_container_fail_fast() {
    // Initialize test logger
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    // 1. Create registry and register High risk connector
    let registry = Arc::new(ConnectorRegistry::new());
    let connector = TestConnector::new("test-high", RiskLevel::High, "test.high");
    registry.register(Arc::new(connector)).unwrap();

    // 2. Create dispatcher
    let dispatcher = CapabilityDispatcher::new(registry.clone());

    // 3. Dispatch request
    let request = create_test_request("test-high", "test.high");
    let result = dispatcher.dispatch(request).await.unwrap();

    // 4. Assert: Returns error with "Container runtime not configured"
    assert_eq!(result.status, ConnectorExecutionStatus::Failed);
    assert!(result.error.is_some());

    let error_msg = result.error.unwrap();
    assert!(
        error_msg.contains("Container runtime not configured"),
        "Expected fail-fast error message, got: {}",
        error_msg
    );
    assert!(
        error_msg.contains("High/Critical-risk") || error_msg.contains("High/Critical risk"),
        "Expected clear error message mentioning risk level, got: {}",
        error_msg
    );
}

#[tokio::test]
async fn test_critical_risk_container_fail_fast() {
    // Initialize test logger
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    // 1. Create registry and register Critical risk connector
    let registry = Arc::new(ConnectorRegistry::new());
    let connector = TestConnector::new("test-critical", RiskLevel::Critical, "test.critical");
    registry.register(Arc::new(connector)).unwrap();

    // 2. Create dispatcher
    let dispatcher = CapabilityDispatcher::new(registry.clone());

    // 3. Dispatch request
    let request = create_test_request("test-critical", "test.critical");
    let result = dispatcher.dispatch(request).await.unwrap();

    // 4. Assert: Returns error with "Container runtime not configured"
    assert_eq!(result.status, ConnectorExecutionStatus::Failed);
    assert!(result.error.is_some());

    let error_msg = result.error.unwrap();
    assert!(
        error_msg.contains("Container runtime not configured"),
        "Expected fail-fast error message, got: {}",
        error_msg
    );
}

#[tokio::test]
async fn test_runtime_selection_with_real_local_connector() {
    // Initialize test logger
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    let workspace = create_test_workspace();

    // 1. Create registry and register LocalConnector
    let registry = Arc::new(ConnectorRegistry::new());
    let local_connector = LocalConnector::new(workspace.clone());
    registry.register(Arc::new(local_connector)).unwrap();

    // 2. Create dispatcher
    let dispatcher = CapabilityDispatcher::new(registry.clone());

    // Test Low risk: fs.read (should succeed with Native runtime)
    let read_request = {
        let test_file = workspace.join("test.txt");
        std::fs::write(&test_file, "test content").unwrap();

        CapabilityExecutionRequest {
            execution_id: ExecutionId::new(),
            trace_id: uuid::Uuid::new_v4().to_string(),
            actor: ActorRef {
                id: ActorId::from_string("test-actor".to_string()).unwrap(),
                actor_type: cyberclaw_core::identity::ActorType::Human,
                tenant_id: None,
                home_node_id: None,
                display_name: "Test Actor".to_string(),
            },
            workspace: WorkspaceRef {
                id: WorkspaceId::from_string("test-workspace".to_string()).unwrap(),
                mode: cyberclaw_core::workspace::WorkspaceMode::Isolated,
                materialization_mode: Some(
                    cyberclaw_core::workspace::WorkspaceMaterializationMode::LocalEphemeral,
                ),
                home_node_id: None,
                backing_store: None,
                root: workspace.to_string_lossy().to_string(),
                writable_roots: vec![workspace.to_string_lossy().to_string()],
            },
            connector_id: ConnectorId::from_string("local".to_string()).unwrap(),
            capability_id: CapabilityId::from_string("fs.read".to_string()).unwrap(),
            input: serde_json::json!({
                "path": test_file.to_string_lossy().to_string()
            }),
        }
    };

    let read_result = dispatcher.dispatch(read_request).await.unwrap();
    assert_eq!(read_result.status, ConnectorExecutionStatus::Success);
    assert!(read_result.error.is_none());

    // Test Medium risk: fs.write (should route to Process runtime, fail-fast if not implemented)
    // CRITICAL #4 FIX: Medium risk → Process runtime → fail-fast (not fallback to native)
    let write_request = CapabilityExecutionRequest {
        execution_id: ExecutionId::new(),
        trace_id: uuid::Uuid::new_v4().to_string(),
        actor: ActorRef {
            id: ActorId::from_string("test-actor".to_string()).unwrap(),
            actor_type: cyberclaw_core::identity::ActorType::Human,
            tenant_id: None,
            home_node_id: None,
            display_name: "Test Actor".to_string(),
        },
        workspace: WorkspaceRef {
            id: WorkspaceId::from_string("test-workspace".to_string()).unwrap(),
            mode: cyberclaw_core::workspace::WorkspaceMode::Isolated,
            materialization_mode: Some(
                cyberclaw_core::workspace::WorkspaceMaterializationMode::LocalEphemeral,
            ),
            home_node_id: None,
            backing_store: None,
            root: workspace.to_string_lossy().to_string(),
            writable_roots: vec![workspace.to_string_lossy().to_string()],
        },
        connector_id: ConnectorId::from_string("local".to_string()).unwrap(),
        capability_id: CapabilityId::from_string("fs.write".to_string()).unwrap(),
        input: serde_json::json!({
            "path": workspace.join("write_test.txt").to_string_lossy().to_string(),
            "content": "test write content"
        }),
    };

    let write_result = dispatcher.dispatch(write_request).await.unwrap();
    // CRITICAL #4 FIX: Medium risk → Process runtime → fail-fast
    assert_eq!(write_result.status, ConnectorExecutionStatus::Failed);
    assert!(write_result.error.is_some());
    let write_error = write_result.error.unwrap();
    assert!(
        write_error.contains("Process runtime not configured"),
        "Expected fail-fast error for Medium risk fs.write, got: {}",
        write_error
    );

    // Test High risk: cmd.exec (should fail-fast with Container runtime not available)
    let exec_request = CapabilityExecutionRequest {
        execution_id: ExecutionId::new(),
        trace_id: uuid::Uuid::new_v4().to_string(),
        actor: ActorRef {
            id: ActorId::from_string("test-actor".to_string()).unwrap(),
            actor_type: cyberclaw_core::identity::ActorType::Human,
            tenant_id: None,
            home_node_id: None,
            display_name: "Test Actor".to_string(),
        },
        workspace: WorkspaceRef {
            id: WorkspaceId::from_string("test-workspace".to_string()).unwrap(),
            mode: cyberclaw_core::workspace::WorkspaceMode::Isolated,
            materialization_mode: Some(
                cyberclaw_core::workspace::WorkspaceMaterializationMode::LocalEphemeral,
            ),
            home_node_id: None,
            backing_store: None,
            root: workspace.to_string_lossy().to_string(),
            writable_roots: vec![workspace.to_string_lossy().to_string()],
        },
        connector_id: ConnectorId::from_string("local".to_string()).unwrap(),
        capability_id: CapabilityId::from_string("cmd.exec".to_string()).unwrap(),
        input: serde_json::json!({
            "command": "echo test"
        }),
    };

    let exec_result = dispatcher.dispatch(exec_request).await.unwrap();
    assert_eq!(exec_result.status, ConnectorExecutionStatus::Failed);
    assert!(exec_result.error.is_some());

    let error_msg = exec_result.error.unwrap();
    assert!(
        error_msg.contains("Container runtime not configured"),
        "Expected fail-fast error for High risk capability, got: {}",
        error_msg
    );

    // Cleanup
    std::fs::remove_dir_all(&workspace).ok();
}

#[tokio::test]
async fn test_error_message_is_clear_and_actionable() {
    // Initialize test logger
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    // Create registry and register High risk connector
    let registry = Arc::new(ConnectorRegistry::new());
    let connector = TestConnector::new("test-high", RiskLevel::High, "test.high");
    registry.register(Arc::new(connector)).unwrap();

    let dispatcher = CapabilityDispatcher::new(registry.clone());
    let request = create_test_request("test-high", "test.high");
    let result = dispatcher.dispatch(request).await.unwrap();

    // Verify error message structure
    assert_eq!(result.status, ConnectorExecutionStatus::Failed);
    let error = result.error.unwrap();

    // Check for key elements in error message:
    // 1. Mentions "Container runtime"
    assert!(
        error.contains("Container runtime"),
        "Error should mention Container runtime: {}",
        error
    );

    // 2. Indicates the runtime is not configured / unavailable
    assert!(
        error.contains("not configured")
            || error.contains("not yet available")
            || error.contains("not implemented"),
        "Error should indicate unavailability: {}",
        error
    );

    // 3. Mentions risk level or security context
    assert!(
        error.contains("High") || error.contains("Critical") || error.contains("risk"),
        "Error should mention risk level: {}",
        error
    );

    // 4. Check JSON output field also contains clear error.
    // Use containment instead of equality so the assertion survives
    // operator-friendly elaboration of the message (e.g. adding the
    // builder method name and runbook ref).
    let output_error = result.output.get("error").and_then(|e| e.as_str());
    assert!(output_error.is_some(), "Output should contain error field");
    let oe = output_error.unwrap();
    assert!(
        oe.contains("Container runtime not configured"),
        "output.error should report container runtime missing: {}",
        oe
    );
    assert!(
        oe.contains("High") || oe.contains("Critical") || oe.contains("risk"),
        "output.error should mention risk level: {}",
        oe
    );
}

#[tokio::test]
async fn test_runtime_selector_is_in_production_path() {
    // This test verifies that RuntimeSelector is actually called in the production
    // dispatcher path, not just in unit tests.
    //
    // We verify this by:
    // 1. Checking different risk levels produce different runtime selection behavior
    // 2. Confirming the dispatcher's runtime selection logic affects execution

    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    let registry = Arc::new(ConnectorRegistry::new());

    // Register connectors with different risk levels
    let low_connector = TestConnector::new("test-low", RiskLevel::Low, "test.low");
    let high_connector = TestConnector::new("test-high", RiskLevel::High, "test.high");

    registry.register(Arc::new(low_connector)).unwrap();
    registry.register(Arc::new(high_connector)).unwrap();

    let dispatcher = CapabilityDispatcher::new(registry.clone());

    // Low risk should succeed
    let low_request = create_test_request("test-low", "test.low");
    let low_result = dispatcher.dispatch(low_request).await.unwrap();
    assert_eq!(low_result.status, ConnectorExecutionStatus::Success);

    // High risk should fail-fast
    let high_request = create_test_request("test-high", "test.high");
    let high_result = dispatcher.dispatch(high_request).await.unwrap();
    assert_eq!(high_result.status, ConnectorExecutionStatus::Failed);
    assert!(high_result
        .error
        .unwrap()
        .contains("Container runtime not configured"));

    // This confirms RuntimeSelector is in the production path, because:
    // - Low risk → Native → Success
    // - High risk → Container → Fail-fast (runtime not implemented)
    // If RuntimeSelector wasn't called, both would have the same behavior
}

#[tokio::test]
async fn test_custom_runtime_selector_config() {
    // Test that dispatcher can use custom RuntimeSelectorConfig

    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    let registry = Arc::new(ConnectorRegistry::new());
    let connector = TestConnector::new("test-medium", RiskLevel::Medium, "test.medium");
    registry.register(Arc::new(connector)).unwrap();

    // Create dispatcher with custom config (force Native for all)
    let custom_config = RuntimeSelectorConfig {
        default_strategy: cyberclaw_connectors::runtime::RuntimeSelectionStrategy::AlwaysNative,
        capability_overrides: std::collections::HashMap::new(),
        strict_mode: false,
    };

    let dispatcher = CapabilityDispatcher::with_runtime_config(registry.clone(), custom_config);

    // Medium risk should succeed with AlwaysNative strategy
    let request = create_test_request("test-medium", "test.medium");
    let result = dispatcher.dispatch(request).await.unwrap();

    assert_eq!(result.status, ConnectorExecutionStatus::Success);
    assert!(result.error.is_none());
}
