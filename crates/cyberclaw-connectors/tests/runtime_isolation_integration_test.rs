//! CRITICAL #4, #5, #6 集成测试：Runtime 隔离策略
//!
//! 本测试套件验证运行时隔离机制的正确性：
//! - CRITICAL #5: High/Critical capability 不能使用 Native runtime
//! - CRITICAL #4: Process runtime 未实现时必须 fail-fast
//! - CRITICAL #6: Native runtime 必须验证 capability contract
//! - HIGH #6: 验证实际执行的 runtime 与选择的一致

use cyberclaw_connectors::dispatcher::CapabilityDispatcher;
use cyberclaw_connectors::local::LocalConnector;
use cyberclaw_connectors::registry::ConnectorRegistry;
use cyberclaw_connectors::runtime::{RuntimeMode, RuntimeSelectionStrategy, RuntimeSelectorConfig};
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
            actual_runtime: Some(RuntimeMode::Native),
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
async fn test_high_risk_rejects_native_runtime() {
    // CRITICAL #5: High/Critical capability 不能使用 Native runtime

    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    let registry = Arc::new(ConnectorRegistry::new());
    let connector = TestConnector::new("test-high", RiskLevel::High, "test.high");
    registry.register(Arc::new(connector)).unwrap();

    // 使用默认配置（Risk-based selection, strict mode）
    let dispatcher = CapabilityDispatcher::new(registry.clone());

    let request = create_test_request("test-high", "test.high");
    let result = dispatcher.dispatch(request).await.unwrap();

    // 应该失败，因为 High risk 需要 Container runtime（未实现）
    assert_eq!(result.status, ConnectorExecutionStatus::Failed);
    assert!(result.error.is_some());

    let error_msg = result.error.unwrap();
    assert!(
        error_msg.contains("Container runtime not configured"),
        "Expected runtime isolation error, got: {}",
        error_msg
    );
}

#[tokio::test]
async fn test_process_runtime_fail_fast() {
    // CRITICAL #4: Process runtime 未实现时必须 fail-fast

    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    let registry = Arc::new(ConnectorRegistry::new());
    let connector = TestConnector::new("test-medium", RiskLevel::Medium, "test.medium");
    registry.register(Arc::new(connector)).unwrap();

    let dispatcher = CapabilityDispatcher::new(registry.clone());

    let request = create_test_request("test-medium", "test.medium");
    let result = dispatcher.dispatch(request).await.unwrap();

    // Medium risk 应该选择 Process runtime，但由于未实现应该 fail-fast
    assert_eq!(result.status, ConnectorExecutionStatus::Failed);
    assert!(result.error.is_some());

    let error_msg = result.error.unwrap();
    assert!(
        error_msg.contains("Process runtime not configured"),
        "Expected fail-fast error, got: {}",
        error_msg
    );
}

#[tokio::test]
async fn test_runtime_verification_mismatch() {
    // HIGH #6: 验证实际执行的 runtime 与选择的一致

    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    let registry = Arc::new(ConnectorRegistry::new());
    let connector = TestConnector::new("test-low", RiskLevel::Low, "test.low");
    registry.register(Arc::new(connector)).unwrap();

    let dispatcher = CapabilityDispatcher::new(registry.clone());

    let request = create_test_request("test-low", "test.low");
    let result = dispatcher.dispatch(request).await.unwrap();

    // Low risk 应该成功并使用 Native runtime
    assert_eq!(result.status, ConnectorExecutionStatus::Success);

    // 验证 actual_runtime 字段存在
    assert!(
        result.actual_runtime.is_some(),
        "actual_runtime field should be populated"
    );

    // 验证 actual_runtime 与预期一致（Low risk → Native）
    assert_eq!(
        result.actual_runtime.unwrap(),
        RuntimeMode::Native,
        "actual_runtime should match expected runtime for Low risk"
    );
}

#[tokio::test]
async fn test_native_runtime_with_local_connector() {
    // CRITICAL #6: Native runtime 必须验证 capability contract
    // 使用真实的 LocalConnector 来验证

    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    let workspace = create_test_workspace();
    let test_file = workspace.join("test.txt");
    std::fs::write(&test_file, "test content").unwrap();

    let registry = Arc::new(ConnectorRegistry::new());
    let local_connector = LocalConnector::new(workspace.clone());
    registry.register(Arc::new(local_connector)).unwrap();

    let dispatcher = CapabilityDispatcher::new(registry.clone());

    // Test Low risk: fs.read (应该使用 Native runtime 成功)
    let read_request = CapabilityExecutionRequest {
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
    };

    let result = dispatcher.dispatch(read_request).await.unwrap();
    assert_eq!(result.status, ConnectorExecutionStatus::Success);
    assert_eq!(result.actual_runtime, Some(RuntimeMode::Native));

    // 验证输出格式符合 contract
    assert!(result.output.is_object());
    assert!(result.output.get("content").is_some());

    // Cleanup
    std::fs::remove_dir_all(&workspace).ok();
}

#[tokio::test]
async fn test_critical_risk_container_fail_fast() {
    // CRITICAL #5: Critical risk 必须使用 Container runtime

    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    let registry = Arc::new(ConnectorRegistry::new());
    let connector = TestConnector::new("test-critical", RiskLevel::Critical, "test.critical");
    registry.register(Arc::new(connector)).unwrap();

    let dispatcher = CapabilityDispatcher::new(registry.clone());

    let request = create_test_request("test-critical", "test.critical");
    let result = dispatcher.dispatch(request).await.unwrap();

    // 应该失败，因为 Critical risk 需要 Container runtime（未实现）
    assert_eq!(result.status, ConnectorExecutionStatus::Failed);
    assert!(result.error.is_some());

    let error_msg = result.error.unwrap();
    assert!(
        error_msg.contains("Container runtime not configured"),
        "Expected container runtime error for Critical risk, got: {}",
        error_msg
    );
}

#[tokio::test]
async fn test_always_native_strategy_bypasses_isolation() {
    // 验证 AlwaysNative 策略在 RuntimeSelector 层面选择 Native runtime，
    // 但 dispatcher 的 CRITICAL #5 安全检查仍会拒绝 High/Critical risk 能力
    // 通过 Native runtime 执行，即使 AlwaysNative 策略开启也不例外。
    //
    // 若需完全绕过隔离，应使用 Low/Medium risk 能力配合 AlwaysNative 策略。

    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    let registry = Arc::new(ConnectorRegistry::new());
    let connector = TestConnector::new("test-high", RiskLevel::High, "test.high");
    registry.register(Arc::new(connector)).unwrap();

    // 使用 AlwaysNative 策略（非 strict mode）
    let custom_config = RuntimeSelectorConfig {
        default_strategy: RuntimeSelectionStrategy::AlwaysNative,
        capability_overrides: std::collections::HashMap::new(),
        strict_mode: false,
    };

    let dispatcher = CapabilityDispatcher::with_runtime_config(registry.clone(), custom_config);

    let request = create_test_request("test-high", "test.high");
    let result = dispatcher.dispatch(request).await.unwrap();

    // CRITICAL #5 FIX: 即使 AlwaysNative 策略选择了 Native runtime，
    // dispatcher 仍然拒绝 High risk 能力使用 Native runtime
    assert_eq!(result.status, ConnectorExecutionStatus::Failed);
    assert!(result.error.is_some());
    let error_msg = result.error.unwrap();
    assert!(
        error_msg.contains("Native runtime not allowed"),
        "Expected CRITICAL #5 rejection for High risk with Native runtime, got: {}",
        error_msg
    );
}

#[tokio::test]
async fn test_runtime_isolation_with_different_risk_levels() {
    // 综合测试：验证不同风险等级的隔离策略

    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    let registry = Arc::new(ConnectorRegistry::new());

    // 注册不同风险等级的 connectors
    let low_connector = TestConnector::new("test-low", RiskLevel::Low, "test.low");
    let medium_connector = TestConnector::new("test-medium", RiskLevel::Medium, "test.medium");
    let high_connector = TestConnector::new("test-high", RiskLevel::High, "test.high");
    let critical_connector =
        TestConnector::new("test-critical", RiskLevel::Critical, "test.critical");

    registry.register(Arc::new(low_connector)).unwrap();
    registry.register(Arc::new(medium_connector)).unwrap();
    registry.register(Arc::new(high_connector)).unwrap();
    registry.register(Arc::new(critical_connector)).unwrap();

    let dispatcher = CapabilityDispatcher::new(registry.clone());

    // Low risk → Native → Success
    let low_result = dispatcher
        .dispatch(create_test_request("test-low", "test.low"))
        .await
        .unwrap();
    assert_eq!(low_result.status, ConnectorExecutionStatus::Success);
    assert_eq!(low_result.actual_runtime, Some(RuntimeMode::Native));

    // Medium risk → Process → Fail (not implemented)
    let medium_result = dispatcher
        .dispatch(create_test_request("test-medium", "test.medium"))
        .await
        .unwrap();
    assert_eq!(medium_result.status, ConnectorExecutionStatus::Failed);
    assert!(medium_result
        .error
        .unwrap()
        .contains("Process runtime not configured"));

    // High risk → Container → Fail (not implemented)
    let high_result = dispatcher
        .dispatch(create_test_request("test-high", "test.high"))
        .await
        .unwrap();
    assert_eq!(high_result.status, ConnectorExecutionStatus::Failed);
    assert!(high_result
        .error
        .unwrap()
        .contains("Container runtime not configured"));

    // Critical risk → Container → Fail (not implemented)
    let critical_result = dispatcher
        .dispatch(create_test_request("test-critical", "test.critical"))
        .await
        .unwrap();
    assert_eq!(critical_result.status, ConnectorExecutionStatus::Failed);
    assert!(critical_result
        .error
        .unwrap()
        .contains("Container runtime not configured"));
}
