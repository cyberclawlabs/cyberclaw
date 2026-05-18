//! 端到端集成测试：完整的 connector -> capability 执行链
//!
//! P0-5 任务：验证从 orchestrator 到 connector 的真实执行流程
//!
//! 覆盖范围：
//! - 完整链路：orchestrator -> resolver -> executor -> dispatcher -> connector
//! - 文件系统副作用验证（fs.write -> 文件真实创建）
//! - 状态转换：Pending -> Running -> Completed
//! - 清理逻辑：测试结束后删除临时文件

use cyberclaw_connectors::runtime::{RuntimeMode, RuntimeSelectorConfig};
use cyberclaw_connectors::{CapabilityDispatcher, ConnectorRegistry, LocalConnector};
use cyberclaw_control_plane::{
    ControlPlaneOrchestrator, ExecutionService, InMemoryEventBus, InMemoryExecutionService,
    InMemoryGatewayRouter, InMemoryLeaseManager, InMemoryMembershipService,
    InMemoryPlacementEngine, InMemoryRegistry, InMemoryResolver, InMemoryReviewQueue,
    InMemoryTaskManager, MembershipService, Registry,
};
use cyberclaw_core::capability::{CapabilityEffect, RiskLevel};
use cyberclaw_core::cluster::{
    CapabilityPlacement, MembershipState, NodeCapacity, NodeHealth, NodeRecord, NodeRole,
};
use cyberclaw_core::enums::Priority;
use cyberclaw_core::execution::ExecutionStatus;
use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::{ActorId, NodeId, TaskId};
#[allow(unused_imports)]
use cyberclaw_core::manifests::ConnectorRuntime;
use cyberclaw_core::manifests::{
    Artifacts, CapabilityContract, Compatibility, ConnectorSpec, Dependencies, PackageKind,
    PackageManifest, PackageSpec,
};
use cyberclaw_core::task::{Task, TaskInput, TaskKind, TriggerRef};
use cyberclaw_core::workspace::{WorkspaceMode, WorkspaceRef};
use cyberclaw_governance::engine::DefaultPolicyEngine;
use std::sync::Arc;

// ─────────────────────────────────────────────
// 辅助函数
// ─────────────────────────────────────────────

/// 创建测试 actor
fn make_test_actor(id: &str) -> ActorRef {
    ActorRef {
        id: ActorId::from_string(id.to_string()).unwrap(),
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: format!("Test User ({})", id),
    }
}

/// 构建 LocalConnector 的 PackageManifest，用于直接注册到 registry
///
/// 注意：为了避免依赖 ecosystem 文件系统扫描（及 apiVersion 校验），
/// 在测试中直接构造 PackageManifest 并插入 registry。
/// fs.write 在 manifest 中标记为 Low risk（测试专用），使其能绕过审批流程。
fn build_local_connector_manifest() -> PackageManifest {
    PackageManifest {
        api_version: "cyberclaw.io/v2".to_string(),
        kind: PackageKind::Connector,
        id: "local".to_string(),
        version: "0.1.0".to_string(),
        name: "LocalConnector".to_string(),
        display_name: Some("Local File System Connector (Test)".to_string()),
        summary: "Test connector for e2e verification".to_string(),
        owner: Some("test".to_string()),
        license: None,
        homepage: None,
        repository: None,
        tags: vec!["test".to_string()],
        compatibility: Compatibility {
            platform: "cyberclaw".to_string(),
            runtime: vec!["native".to_string()],
            os: vec![],
            arch: vec![],
        },
        dependencies: Dependencies::default(),
        artifacts: Artifacts::default(),
        config_schema: None,
        spec: PackageSpec::Connector(ConnectorSpec {
            subtype: "filesystem".to_string(),
            runtime: cyberclaw_core::manifests::ConnectorRuntime::Native,
            auth: None,
            capabilities: vec![
                CapabilityContract {
                    id: "fs.read".to_string(),
                    title: "Read File".to_string(),
                    description: Some("Read file content".to_string()),
                    input_schema: "FsReadInput".to_string(),
                    output_schema: "FsReadOutput".to_string(),
                    risk: RiskLevel::Low,
                    effects: vec![CapabilityEffect::Read],
                    placement: Some(CapabilityPlacement {
                        allowed_node_labels: vec!["worker".to_string()],
                        required_runtime: vec!["native".to_string()],
                        requires_local_secret: false,
                        network_zone: None,
                    }),
                    timeouts: Default::default(),
                },
                CapabilityContract {
                    id: "fs.write".to_string(),
                    title: "Write File".to_string(),
                    description: Some("Write file content".to_string()),
                    input_schema: "FsWriteInput".to_string(),
                    output_schema: "FsWriteOutput".to_string(),
                    // 测试用途：标记为 Low risk，绕过审批流程，直接验证执行链路
                    risk: RiskLevel::Low,
                    effects: vec![CapabilityEffect::Write],
                    placement: Some(CapabilityPlacement {
                        allowed_node_labels: vec!["worker".to_string()],
                        required_runtime: vec!["native".to_string()],
                        requires_local_secret: false,
                        network_zone: None,
                    }),
                    timeouts: Default::default(),
                },
            ],
        }),
    }
}

/// 装配完整的 ControlPlaneOrchestrator，带有 capability dispatcher
///
/// 关键设计决策：
/// 1. 不使用 EcosystemScanner，因为 local connector manifest 使用旧的 apiVersion (v1alpha1)
///    会被 ManifestLoader.validate_manifest() 拒绝
/// 2. 直接构建 PackageManifest 并注册到 registry，确保 local connector 的 capabilities 可用
/// 3. fs.write 在测试 manifest 中标记为 Low risk，使其能绕过审批流程直接执行
///
/// 返回：
/// - orchestrator：主编排器
/// - execution_service：用于查询执行状态
/// - connector_registry：connector 注册表
async fn build_e2e_orchestrator_with_dispatcher(
    workspace: std::path::PathBuf,
) -> (
    ControlPlaneOrchestrator,
    Arc<InMemoryExecutionService>,
    Arc<ConnectorRegistry>,
) {
    // 1. 初始化 registry
    let registry = Arc::new(InMemoryRegistry::new());

    // 2. 直接注册 local connector manifest（不通过 EcosystemScanner）
    //    这样可以确保 fs.write 等 capabilities 出现在 available_capabilities 中
    let local_manifest = build_local_connector_manifest();
    let local_record = cyberclaw_control_plane::PackageRecord {
        id: "local".to_string(),
        kind: PackageKind::Connector,
        latest_version: "0.1.0".to_string(),
        installed_versions: vec!["0.1.0".to_string()],
        active_version: Some("0.1.0".to_string()),
        source: cyberclaw_control_plane::PackageSource::LocalPath(
            "/test/local-connector".to_string(),
        ),
        state: cyberclaw_control_plane::RegistryState::Active,
        available_nodes: vec![],
        runtime_requirements: vec!["native".to_string()],
        manifest: local_manifest,
    };
    registry.upsert(local_record).await.unwrap();

    // 3. 初始化 connector registry + LocalConnector
    let connector_registry = Arc::new(ConnectorRegistry::new());
    let local_connector = Arc::new(LocalConnector::new(workspace.clone()));
    connector_registry.register(local_connector).unwrap();

    // 4. 创建 capability dispatcher (测试配置: 强制 fs.write 使用 Native runtime)
    //
    // P2 阶段限制：Process runtime 尚未实现，但 fs.write 在生产环境中标记为 Medium risk
    // (要求 Process isolation)。为了测试执行链路，我们使用 capability_overrides
    // 强制 fs.write 在测试中使用 Native runtime。
    //
    // 这是临时的测试配置，不影响生产代码。一旦 Process runtime 实现（P3），
    // 应该移除此 override，让 fs.write 使用正常的 Medium risk → Process 映射。
    let mut runtime_config = RuntimeSelectorConfig::default();
    runtime_config
        .capability_overrides
        .insert("fs.write".to_string(), RuntimeMode::Native);
    let dispatcher = Arc::new(CapabilityDispatcher::with_runtime_config(
        connector_registry.clone(),
        runtime_config,
    ));

    // 5. 创建 execution service，挂载 dispatcher
    let execution_service =
        Arc::new(InMemoryExecutionService::new().with_capability_dispatcher(dispatcher));

    // 6. 装配其余 control plane 组件
    let gateway = Arc::new(InMemoryGatewayRouter::new());
    let resolver = Arc::new(InMemoryResolver::new(registry.clone()));
    let review_queue = Arc::new(InMemoryReviewQueue::new(None));
    let task_manager = Arc::new(InMemoryTaskManager::new());
    let placement_engine = Arc::new(InMemoryPlacementEngine::new());
    let lease_manager = Arc::new(InMemoryLeaseManager::default());
    let membership_service = Arc::new(InMemoryMembershipService::default());
    let event_bus = Arc::new(InMemoryEventBus::default());

    // 7. 注册一个活跃节点（必须，否则 orchestrator 无法提交执行）
    let node_id = NodeId::from_string("e2e-test-node".to_string()).unwrap();
    let node = NodeRecord {
        id: node_id.clone(),
        role: NodeRole::Worker,
        labels: vec!["worker".to_string()],
        health: NodeHealth::Healthy,
        capacity: NodeCapacity {
            max_executions: Some(10),
            max_cpu_millis: None,
            max_memory_mb: None,
        },
        current_executions: 0,
        region: Some("us-east-1".to_string()),
        zone: Some("us-east-1a".to_string()),
        membership_state: MembershipState::Joining,
        last_heartbeat_at: chrono::Utc::now(),
    };
    membership_service.join(node).unwrap();
    // 发送心跳，将节点从 Joining 状态升级为 Active 状态
    membership_service.heartbeat(&node_id).unwrap();

    // 8. 创建 policy engine
    let policy_engine = Arc::new(DefaultPolicyEngine::default());

    let security_event_store: Arc<dyn cyberclaw_observability::SecurityEventStore> =
        Arc::new(cyberclaw_observability::InMemorySecurityEventStore::new());

    // 创建 PluginRegistry 实例
    let plugin_registry = Arc::new(cyberclaw_plugin_runtime::PluginRegistry::new(
        std::path::PathBuf::from("/tmp/test_plugins"),
    ));

    // 9. 装配 orchestrator
    let orchestrator = ControlPlaneOrchestrator::new(
        gateway,
        resolver,
        registry,
        review_queue,
        task_manager,
        execution_service.clone(),
        placement_engine,
        lease_manager,
        membership_service,
        event_bus,
        policy_engine,
        plugin_registry,
        security_event_store,
    );

    (orchestrator, execution_service, connector_registry)
}

// ─────────────────────────────────────────────
// E2E 测试：fs.write 写文件并验证副作用
// ─────────────────────────────────────────────

/// 端到端测试：验证 orchestrator 完整链路（ingress -> resolve -> plan -> submit）
///
/// 测试流程：
/// 1. 装配完整的 orchestrator（registry 中已注册 local connector，fs.write = Low risk）
/// 2. 创建带有显式 fs.write capability 请求的 Task
/// 3. 通过 orchestrator.process_ingress() 触发完整解析和提交流程
/// 4. 验证 orchestrator 正确完成了 resolve -> plan -> placement -> lease -> submit 链路
/// 5. 验证 execution 记录已创建且状态为 Pending
///
/// 注意：orchestrator.process_ingress() 的职责是：
///   - 接收请求 → 规范化 → 解析（resolver.plan）→ 评估风险 → 提交（placement + lease + submit）
///   - orchestrator 不直接调用 execute()，execute() 由工作节点调用
///   - 文件系统副作用验证由 test_e2e_direct_capability_dispatch_fs_write 覆盖
#[tokio::test(flavor = "multi_thread")]
async fn test_e2e_fs_write_execution() {
    // 步骤 0：设置临时工作区
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace_path = temp_dir.path().to_path_buf();

    // 装配 orchestrator（带 capability dispatcher，registry 中已注册 local connector）
    let (orchestrator, execution_service, _connector_registry) =
        build_e2e_orchestrator_with_dispatcher(workspace_path.clone()).await;

    // 步骤 1：创建工作区引用
    let workspace_ref = WorkspaceRef {
        id: cyberclaw_core::ids::WorkspaceId::from_string("e2e-test-workspace".to_string())
            .unwrap(),
        mode: WorkspaceMode::Isolated,
        materialization_mode: None,
        home_node_id: None,
        backing_store: None,
        root: workspace_path.to_string_lossy().to_string(),
        writable_roots: vec![workspace_path.to_string_lossy().to_string()],
    };

    // 步骤 2：创建带有显式 fs.write capability 请求的 Task
    //
    // TaskInput.payload schema：
    // { "capability": "fs.write", "params": { "path": "test.txt", "content": "hello", "create_dirs": false } }
    //
    // 测试设计：
    // - local connector 在 build_local_connector_manifest() 中以 Low risk 注册
    // - Low risk 不触发审批流程，直接走 submit 路径
    // - orchestrator 将完成: ingress -> gateway.normalize -> task_manager.create_task
    //   -> resolver.plan -> evaluate_risk -> placement_engine.place -> lease_manager.acquire
    //   -> execution_service.submit -> return (submitted=true, review_id=None)
    let actor = make_test_actor("e2e-test-user");
    let task = Task {
        id: TaskId::new(),
        case_id: None,
        title: "Write test file".to_string(),
        summary: "Write hello to test.txt".to_string(),
        kind: TaskKind::Execution,
        priority: Priority::Low,
        requested_by: actor.clone(),
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "e2e-test".to_string(),
        },
        input: TaskInput {
            payload: serde_json::json!({
                "capability": "fs.write",
                "params": {
                    "path": "test.txt",
                    "content": "hello",
                    "create_dirs": false
                }
            }),
        },
        desired_outputs: vec![],
        // P0 fix compatibility: Allow path needs explicit execution trigger
        labels: vec![
            "e2e".to_string(),
            "test".to_string(),
            "allow-empty-actions".to_string(),
        ],
        preferred_agent_id: None,
    };

    // 步骤 3：通过 orchestrator 触发完整解析和提交流程
    let ingress_request = cyberclaw_control_plane::IngressRequest {
        actor: actor.clone(),
        session: None,
        workspace: Some(workspace_ref),
        task,
    };

    let result = orchestrator.process_ingress(ingress_request).await;
    assert!(
        result.is_ok(),
        "orchestrator.process_ingress() 应该成功，但得到错误: {:?}",
        result.err()
    );

    let result = result.unwrap();

    // 步骤 4：验证 orchestrator 完成了完整的提交链路
    //
    // 因为：
    // - local connector 注册了 fs.write，risk = Low
    // - Low risk -> evaluate_risk() 返回 RiskLevel::Low
    // - RiskLevel::Low < RiskLevel::Medium -> 跳过审批门控
    // - 直接执行 submit_execution(): placement -> lease -> execution_service.submit()
    // - 结果：submitted = true, review_id = None
    assert!(
        result.submitted,
        "fs.write 是 Low risk，应该直接提交而不需要审批，但 submitted = false"
    );
    assert!(
        result.review_id.is_none(),
        "Low risk 任务不应触发审批流程，但 review_id = {:?}",
        result.review_id
    );

    // 步骤 5：验证 execution 记录已在 execution_service 中创建
    let execution_id = result.execution_id;
    let exec = execution_service.get(&execution_id).await.unwrap();
    assert!(
        exec.is_some(),
        "orchestrator 提交后，execution_service 中应该存在对应的 execution 记录"
    );

    // 步骤 6：验证 execution 状态为 Completed
    // P0 修复后：Allow path 在 process_ingress 中自动执行，无需手动调用 execute()
    // 配置了 CapabilityDispatcher 时 fs.write 直接执行完成，状态为 Completed。
    let exec = execution_service.get(&execution_id).await.unwrap().unwrap();
    assert!(
        matches!(exec.status, ExecutionStatus::Completed),
        "execute 完成后，execution 应处于 Completed 状态，实际状态: {:?}",
        exec.status
    );

    // 步骤 7：清理（temp_dir 会在 drop 时自动清理）
    assert!(workspace_path.exists(), "工作区路径在测试结束前应该存在");
}

// ─────────────────────────────────────────────
// E2E 测试：直接通过 ExecutionService + dispatcher 执行 fs.write
// ─────────────────────────────────────────────

/// 端到端测试：直接通过 ExecutionService + CapabilityDispatcher 验证 fs.write 链路
///
/// 这个测试绕过 orchestrator，直接测试：
/// InMemoryExecutionService -> CapabilityDispatcher -> LocalConnector -> fs.write
///
/// 测试流程：
/// 1. 创建带有 dispatcher 的 ExecutionService
/// 2. 提交包含 fs.write action 的 ExecutionPlan
/// 3. 执行
/// 4. 验证文件被创建
/// 5. 验证最终状态为 Completed
#[tokio::test]
async fn test_e2e_direct_capability_dispatch_fs_write() {
    use cyberclaw_connectors::types::{ExecutionStatus as ConnectorStatus, FsWriteInput};
    use cyberclaw_core::ids::{ExecutionId, TraceId};

    // 步骤 0：设置临时工作区
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace_path = temp_dir.path().to_path_buf();
    let test_file_path = "e2e_direct_test.txt";
    let test_file = workspace_path.join(test_file_path);
    let expected_content = "e2e direct test content";

    // 步骤 1：创建工作区引用
    let workspace = WorkspaceRef {
        id: cyberclaw_core::ids::WorkspaceId::from_string("direct-test-workspace".to_string())
            .unwrap(),
        mode: WorkspaceMode::Isolated,
        materialization_mode: None,
        home_node_id: None,
        backing_store: None,
        root: workspace_path.to_string_lossy().to_string(),
        writable_roots: vec![workspace_path.to_string_lossy().to_string()],
    };

    // 步骤 2：初始化 connector registry + LocalConnector
    let connector_registry = Arc::new(ConnectorRegistry::new());
    let local_connector = Arc::new(LocalConnector::new(workspace_path.clone()));
    connector_registry.register(local_connector).unwrap();

    // 步骤 3：创建 capability dispatcher (P2 runtime override: fs.write → Native)
    let mut runtime_config = RuntimeSelectorConfig::default();
    runtime_config
        .capability_overrides
        .insert("fs.write".to_string(), RuntimeMode::Native);
    let dispatcher = Arc::new(CapabilityDispatcher::with_runtime_config(
        connector_registry.clone(),
        runtime_config,
    ));

    // 步骤 4：创建 ExecutionService，挂载 dispatcher
    let execution_service =
        Arc::new(InMemoryExecutionService::new().with_capability_dispatcher(dispatcher.clone()));

    // 步骤 5：创建包含 fs.write action 的 ExecutionPlan
    // 注意：execution_id 由 submit_plan() 内部生成，此处不需要预先创建
    let connector_id = cyberclaw_core::ids::ConnectorId::from_string("local".to_string()).unwrap();
    let capability_id =
        cyberclaw_core::ids::CapabilityId::from_string("fs.write".to_string()).unwrap();
    let agent_id =
        cyberclaw_core::ids::AgentId::from_string("cyberclaw/master-agent".to_string()).unwrap();

    let write_input = FsWriteInput {
        path: test_file_path.to_string(),
        content: expected_content.to_string(),
        create_dirs: false,
    };

    let plan = cyberclaw_control_plane::ExecutionPlan {
        resolution: cyberclaw_control_plane::Resolution {
            agent: agent_id.clone(),
            skills: vec![],
            workflow: None,
            connectors: vec![connector_id.clone()],
            capabilities: vec![capability_id.clone()],
            reasons: vec!["e2e test".to_string()],
        },
        actions: vec![cyberclaw_control_plane::PlannedAction {
            connector_id: connector_id.clone(),
            capability: capability_id.clone(),
            input: serde_json::to_value(write_input).unwrap(),
            reason: "Write test file for e2e verification".to_string(),
        }],
        review_required: false,
        max_fix_loops: cyberclaw_control_plane::default_max_fix_loops(),
        expected_outcomes: vec![],
    };

    // 步骤 6：提交 execution plan（存储计划）
    let submitted_id = execution_service.submit_plan(plan).await;
    assert!(
        submitted_id.is_ok(),
        "submit_plan 应该成功，但得到错误: {:?}",
        submitted_id.err()
    );
    let submitted_id = submitted_id.unwrap();

    // 步骤 7：验证初始状态为 Pending
    let exec = execution_service.get(&submitted_id).await.unwrap().unwrap();
    assert!(
        matches!(exec.status, ExecutionStatus::Pending),
        "初始状态应为 Pending，实际: {:?}",
        exec.status
    );

    // 步骤 8：执行（触发 dispatcher -> LocalConnector -> fs.write）
    // 注意：submit_plan 存储计划，但 execute 需要找到该计划
    // submit_plan 使用新的 execution_id，不是我们指定的 execution_id
    let exec_result = execution_service.execute(&submitted_id).await;
    assert!(
        exec_result.is_ok(),
        "execute() 应该成功完成 fs.write，但得到错误: {:?}",
        exec_result.err()
    );

    // 步骤 9：验证状态转换为 Completed
    let exec = execution_service.get(&submitted_id).await.unwrap().unwrap();
    assert!(
        matches!(exec.status, ExecutionStatus::Completed),
        "执行后状态应为 Completed，实际: {:?}",
        exec.status
    );
    assert!(exec.started_at.is_some(), "started_at 应该被设置");
    assert!(exec.finished_at.is_some(), "finished_at 应该被设置");

    // 步骤 10：验证文件系统副作用
    assert!(
        test_file.exists(),
        "fs.write 执行后，文件 {:?} 应该存在于工作区",
        test_file
    );

    let actual_content = std::fs::read_to_string(&test_file).unwrap();
    assert_eq!(
        actual_content, expected_content,
        "文件内容应该是 '{}'，实际内容: '{}'",
        expected_content, actual_content
    );

    // 步骤 11：验证 connector 直接执行（辅助验证）
    let actor = make_test_actor("verification-actor");
    let verify_request = cyberclaw_connectors::CapabilityExecutionRequest {
        execution_id: ExecutionId::new(),
        trace_id: TraceId::new().to_string(),
        actor,
        workspace: workspace.clone(),
        connector_id: connector_id.clone(),
        capability_id: cyberclaw_core::ids::CapabilityId::from_string("fs.read".to_string())
            .unwrap(),
        input: serde_json::json!({ "path": test_file_path }),
    };

    let verify_result = dispatcher.dispatch(verify_request).await;
    assert!(
        verify_result.is_ok(),
        "fs.read 验证应该成功，但得到错误: {:?}",
        verify_result.err()
    );

    let verify_result = verify_result.unwrap();
    assert!(
        matches!(verify_result.status, ConnectorStatus::Success),
        "fs.read 应该返回 Success 状态"
    );

    let read_content = verify_result.output["content"].as_str().unwrap_or("");
    assert_eq!(
        read_content, expected_content,
        "通过 fs.read 读取的内容应与写入内容一致"
    );

    // 步骤 12：清理（temp_dir drop 时自动清理，此处显式验证）
    drop(temp_dir);
}

// ─────────────────────────────────────────────
// E2E 测试：状态转换完整验证（Pending -> Running -> Completed）
// ─────────────────────────────────────────────

/// 端到端测试：验证执行状态从 Pending 到 Completed 的完整转换
///
/// 专注于验证状态机的正确性：
/// - 初始状态：Pending
/// - 执行中：Running
/// - 执行完成：Completed
/// - 时间戳正确设置
#[tokio::test]
async fn test_e2e_execution_status_transitions() {
    use cyberclaw_connectors::types::FsWriteInput;

    // 步骤 0：设置临时工作区
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace_path = temp_dir.path().to_path_buf();

    // 步骤 1：初始化 connector + dispatcher + execution service
    let connector_registry = Arc::new(ConnectorRegistry::new());
    let local_connector = Arc::new(LocalConnector::new(workspace_path.clone()));
    connector_registry.register(local_connector).unwrap();

    // P2 runtime override: fs.write → Native (Process runtime not yet implemented)
    let mut runtime_config = RuntimeSelectorConfig::default();
    runtime_config
        .capability_overrides
        .insert("fs.write".to_string(), RuntimeMode::Native);
    let dispatcher = Arc::new(CapabilityDispatcher::with_runtime_config(
        connector_registry,
        runtime_config,
    ));
    let execution_service =
        Arc::new(InMemoryExecutionService::new().with_capability_dispatcher(dispatcher));

    // 步骤 2：创建包含 fs.write action 的 ExecutionPlan
    let connector_id = cyberclaw_core::ids::ConnectorId::from_string("local".to_string()).unwrap();
    let capability_id =
        cyberclaw_core::ids::CapabilityId::from_string("fs.write".to_string()).unwrap();
    let agent_id =
        cyberclaw_core::ids::AgentId::from_string("cyberclaw/master-agent".to_string()).unwrap();

    let write_input = FsWriteInput {
        path: "status_test.txt".to_string(),
        content: "status transition test".to_string(),
        create_dirs: false,
    };

    let plan = cyberclaw_control_plane::ExecutionPlan {
        resolution: cyberclaw_control_plane::Resolution {
            agent: agent_id,
            skills: vec![],
            workflow: None,
            connectors: vec![connector_id.clone()],
            capabilities: vec![capability_id.clone()],
            reasons: vec!["status transition test".to_string()],
        },
        actions: vec![cyberclaw_control_plane::PlannedAction {
            connector_id,
            capability: capability_id,
            input: serde_json::to_value(write_input).unwrap(),
            reason: "Status transition test".to_string(),
        }],
        review_required: false,
        max_fix_loops: cyberclaw_control_plane::default_max_fix_loops(),
        expected_outcomes: vec![],
    };

    // 步骤 3：提交 plan
    let execution_id = execution_service.submit_plan(plan).await.unwrap();

    // 步骤 4：验证初始状态
    let before_time = chrono::Utc::now();
    let exec = execution_service.get(&execution_id).await.unwrap().unwrap();
    assert!(
        matches!(exec.status, ExecutionStatus::Pending),
        "提交后状态应为 Pending，实际: {:?}",
        exec.status
    );
    assert!(exec.started_at.is_none(), "未执行前 started_at 应为 None");
    assert!(exec.finished_at.is_none(), "未执行前 finished_at 应为 None");

    // 步骤 5：执行
    execution_service.execute(&execution_id).await.unwrap();
    let after_time = chrono::Utc::now();

    // 步骤 6：验证最终状态和时间戳
    let exec = execution_service.get(&execution_id).await.unwrap().unwrap();
    assert!(
        matches!(exec.status, ExecutionStatus::Completed),
        "执行后状态应为 Completed，实际: {:?}",
        exec.status
    );

    assert!(
        exec.started_at.is_some(),
        "Completed 后 started_at 应该被设置"
    );
    assert!(
        exec.finished_at.is_some(),
        "Completed 后 finished_at 应该被设置"
    );

    // 验证时间戳在合理范围内
    let started_at = exec.started_at.unwrap();
    let finished_at = exec.finished_at.unwrap();
    assert!(
        started_at >= before_time && started_at <= after_time,
        "started_at ({}) 应该在执行时间范围内 [{}, {}]",
        started_at,
        before_time,
        after_time
    );
    assert!(
        finished_at >= started_at,
        "finished_at ({}) 应该不早于 started_at ({})",
        finished_at,
        started_at
    );

    // 步骤 7：验证文件副作用
    let status_file = workspace_path.join("status_test.txt");
    assert!(status_file.exists(), "状态转换测试文件应该存在");

    // 清理
    drop(temp_dir);
}

// ─────────────────────────────────────────────
// E2E 测试：验证 fs.write + fs.read 完整读写链路
// ─────────────────────────────────────────────

/// 端到端测试：验证写入后读取的完整链路一致性
///
/// 通过两次 capability 执行验证数据完整性：
/// 1. fs.write 写入内容
/// 2. fs.read 读取内容
/// 3. 验证读写内容一致
#[tokio::test]
async fn test_e2e_fs_write_then_read_consistency() {
    use cyberclaw_connectors::types::{
        ExecutionStatus as ConnectorStatus, FsReadInput, FsWriteInput,
    };
    use cyberclaw_core::ids::{ExecutionId, TraceId};

    // 设置临时工作区
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace_path = temp_dir.path().to_path_buf();

    let workspace = WorkspaceRef {
        id: cyberclaw_core::ids::WorkspaceId::from_string("consistency-test".to_string()).unwrap(),
        mode: WorkspaceMode::Isolated,
        materialization_mode: None,
        home_node_id: None,
        backing_store: None,
        root: workspace_path.to_string_lossy().to_string(),
        writable_roots: vec![workspace_path.to_string_lossy().to_string()],
    };

    let actor = make_test_actor("consistency-actor");

    // 初始化 dispatcher (P2 runtime override: fs.write → Native)
    let connector_registry = Arc::new(ConnectorRegistry::new());
    let local_connector = Arc::new(LocalConnector::new(workspace_path.clone()));
    connector_registry.register(local_connector).unwrap();
    let mut runtime_config = RuntimeSelectorConfig::default();
    runtime_config
        .capability_overrides
        .insert("fs.write".to_string(), RuntimeMode::Native);
    let dispatcher = CapabilityDispatcher::with_runtime_config(connector_registry, runtime_config);

    let connector_id = cyberclaw_core::ids::ConnectorId::from_string("local".to_string()).unwrap();
    let test_path = "consistency_test.txt";
    let test_content = "Line 1: Hello from e2e test\nLine 2: Verifying read-write consistency\nLine 3: End of test";

    // 执行 fs.write
    let write_request = cyberclaw_connectors::CapabilityExecutionRequest {
        execution_id: ExecutionId::new(),
        trace_id: TraceId::new().to_string(),
        actor: actor.clone(),
        workspace: workspace.clone(),
        connector_id: connector_id.clone(),
        capability_id: cyberclaw_core::ids::CapabilityId::from_string("fs.write".to_string())
            .unwrap(),
        input: serde_json::to_value(FsWriteInput {
            path: test_path.to_string(),
            content: test_content.to_string(),
            create_dirs: false,
        })
        .unwrap(),
    };

    let write_result = dispatcher.dispatch(write_request).await.unwrap();
    assert!(
        matches!(write_result.status, ConnectorStatus::Success),
        "fs.write 应该成功，但得到: {:?}",
        write_result.error
    );

    // 验证文件存在
    let file_path = workspace_path.join(test_path);
    assert!(file_path.exists(), "fs.write 后文件应该存在");

    // 执行 fs.read
    let read_request = cyberclaw_connectors::CapabilityExecutionRequest {
        execution_id: ExecutionId::new(),
        trace_id: TraceId::new().to_string(),
        actor: actor.clone(),
        workspace: workspace.clone(),
        connector_id: connector_id.clone(),
        capability_id: cyberclaw_core::ids::CapabilityId::from_string("fs.read".to_string())
            .unwrap(),
        input: serde_json::to_value(FsReadInput {
            path: test_path.to_string(),
            offset: None,
            limit: None,
        })
        .unwrap(),
    };

    let read_result = dispatcher.dispatch(read_request).await.unwrap();
    assert!(
        matches!(read_result.status, ConnectorStatus::Success),
        "fs.read 应该成功，但得到: {:?}",
        read_result.error
    );

    // 验证读写一致性
    let read_content = read_result.output["content"].as_str().unwrap_or("");
    assert_eq!(
        read_content, test_content,
        "fs.read 读取内容应与 fs.write 写入内容完全一致"
    );

    // 清理
    drop(temp_dir);
}
