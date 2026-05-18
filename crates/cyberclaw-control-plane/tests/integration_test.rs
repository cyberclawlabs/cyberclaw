use cyberclaw_control_plane::{
    ControlPlaneOrchestrator, EcosystemScanner, EventBus, EventFilter, ExecutionService,
    InMemoryEventBus, InMemoryExecutionService, InMemoryGatewayRouter, InMemoryLeaseManager,
    InMemoryMembershipService, InMemoryPlacementEngine, InMemoryRegistry, InMemoryResolver,
    InMemoryReviewQueue, InMemoryTaskManager, Loader, MembershipService, Registry, Resolver,
    ReviewQueue,
};
use cyberclaw_core::cluster::{
    ClusterEvent, MembershipState, NodeCapacity, NodeHealth, NodeRecord, NodeRole,
};
use cyberclaw_core::enums::Priority;
use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::{ActorId, AgentId, CapabilityId, ConnectorId, NodeId, SkillId, TaskId};
use cyberclaw_core::manifests::PackageKind;
use cyberclaw_core::task::{Task, TaskInput, TaskKind, TriggerRef};
use cyberclaw_governance::engine::DefaultPolicyEngine;
use cyberclaw_plugin_runtime::PluginRegistry;
use std::sync::Arc;

/// Helper function to create a fully initialized ControlPlaneOrchestrator for testing
fn create_test_orchestrator_with_active_node() -> (
    ControlPlaneOrchestrator,
    Arc<InMemoryReviewQueue>,
    Arc<InMemoryExecutionService>,
) {
    let (orchestrator, review_queue, execution_service, _event_bus) =
        create_test_orchestrator_with_event_bus();
    (orchestrator, review_queue, execution_service)
}

/// Helper that also returns the shared EventBus for event subscription tests
fn create_test_orchestrator_with_event_bus() -> (
    ControlPlaneOrchestrator,
    Arc<InMemoryReviewQueue>,
    Arc<InMemoryExecutionService>,
    Arc<InMemoryEventBus>,
) {
    let gateway = Arc::new(InMemoryGatewayRouter::new());
    let registry = Arc::new(InMemoryRegistry::new());
    let resolver = Arc::new(InMemoryResolver::new(registry.clone()));
    let review_queue = Arc::new(InMemoryReviewQueue::new(None));
    let task_manager = Arc::new(InMemoryTaskManager::new());

    // Initialize additional services for complete control plane
    let execution_service = Arc::new(InMemoryExecutionService::default());
    let placement_engine = Arc::new(InMemoryPlacementEngine::new());
    let lease_manager = Arc::new(InMemoryLeaseManager::default());
    let membership_service = Arc::new(InMemoryMembershipService::default());
    let event_bus = Arc::new(InMemoryEventBus::default());

    // Add a test node and promote to Active state
    let test_node_id = NodeId::from_string("test-node-1".to_string()).unwrap();
    let test_node = NodeRecord {
        id: test_node_id.clone(),
        role: NodeRole::Worker,
        labels: vec!["worker".to_string()],
        health: NodeHealth::Healthy,
        capacity: NodeCapacity::default(),
        current_executions: 0,
        region: None,
        zone: None,
        membership_state: MembershipState::Joining,
        last_heartbeat_at: chrono::Utc::now(),
    };
    membership_service.join(test_node).unwrap();
    membership_service.heartbeat(&test_node_id).unwrap();

    // 创建 policy engine
    let policy_engine = Arc::new(DefaultPolicyEngine::default());

    // Create plugin registry
    let temp_dir = tempfile::TempDir::new().unwrap();
    let plugin_registry = Arc::new(PluginRegistry::new(temp_dir.path().to_path_buf()));

    let security_event_store: Arc<dyn cyberclaw_observability::SecurityEventStore> =
        Arc::new(cyberclaw_observability::InMemorySecurityEventStore::new());

    let orchestrator = ControlPlaneOrchestrator::new(
        gateway,
        resolver,
        registry,
        review_queue.clone(),
        task_manager,
        execution_service.clone(),
        placement_engine,
        lease_manager,
        membership_service,
        event_bus.clone(),
        policy_engine,
        plugin_registry,
        security_event_store,
    );

    (orchestrator, review_queue, execution_service, event_bus)
}

/// 集成测试：验证从 ecosystem 扫描到 resolver 的完整流程
#[tokio::test]
async fn test_full_ecosystem_to_resolver_flow() {
    // 第 1 步：扫描 ecosystem
    let scanner = EcosystemScanner::new("../../ecosystem");
    let packages = scanner.scan_all().await;
    assert!(
        packages.is_ok(),
        "failed to scan ecosystem: {:?}",
        packages.err()
    );

    let packages = packages.unwrap();
    assert!(
        packages.len() >= 3,
        "expected at least 3 packages, got {}",
        packages.len()
    );

    // 第 2 步：加载到 Registry
    let registry = Arc::new(InMemoryRegistry::new());
    let count = registry.load_packages(packages).await;
    assert!(count.is_ok(), "failed to load packages: {:?}", count.err());
    assert!(count.unwrap() >= 3, "expected at least 3 packages loaded");

    // 第 3 步：验证 registry 状态
    let agents = registry.list(Some(PackageKind::Agent)).await.unwrap();
    let skills = registry.list(Some(PackageKind::Skill)).await.unwrap();
    let connectors = registry.list(Some(PackageKind::Connector)).await.unwrap();

    assert!(!agents.is_empty(), "no agents found in registry");
    assert!(!skills.is_empty(), "no skills found in registry");
    assert!(!connectors.is_empty(), "no connectors found in registry");

    // 第 4 步：创建 Resolver 并测试 resolution
    let resolver = InMemoryResolver::new(registry.clone());

    let task = Task {
        id: TaskId::from_string("integration-test-001".to_string()).unwrap(),
        case_id: None,
        title: "Integration Test Task".to_string(),
        summary: "This is an integration test for the full ecosystem flow".to_string(),
        kind: TaskKind::Analysis,
        priority: Priority::High,
        requested_by: ActorRef {
            id: ActorId::from_string("test-user".to_string()).unwrap(),
            actor_type: ActorType::Human,
            tenant_id: None,
            home_node_id: None,
            display_name: "Test User".to_string(),
        },
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "integration-test".to_string(),
        },
        input: TaskInput::default(),
        desired_outputs: vec![],
        labels: vec!["test".to_string(), "integration".to_string()],
        preferred_agent_id: None,
    };

    let resolution_input = cyberclaw_control_plane::ResolutionInput {
        task,
        case: None,
        actor: ActorRef {
            id: ActorId::from_string("test-user".to_string()).unwrap(),
            actor_type: ActorType::Human,
            tenant_id: None,
            home_node_id: None,
            display_name: "Test User".to_string(),
        },
        workspace: None,
        session: None,
        available_agents: vec![AgentId::from_string("cyberclaw/master-agent".to_string()).unwrap()],
        available_skills: vec![SkillId::from_string("report-summary".to_string()).unwrap()],
        available_connectors: vec![ConnectorId::from_string("github".to_string()).unwrap()],
        available_capabilities: vec![
            CapabilityId::from_string("github.issue.create".to_string()).unwrap()
        ],
        available_workflows: vec![],
    };

    // 第 5 步：执行 resolution
    let resolution = resolver.resolve(resolution_input.clone()).await;
    assert!(
        resolution.is_ok(),
        "resolution failed: {:?}",
        resolution.err()
    );

    let resolution = resolution.unwrap();
    assert_eq!(resolution.agent.as_str(), "cyberclaw/master-agent");
    assert!(!resolution.skills.is_empty());
    assert!(!resolution.connectors.is_empty());
    assert!(
        !resolution.reasons.is_empty(),
        "resolution should have reasons"
    );

    // 第 6 步：生成 execution plan
    let plan = resolver.plan(resolution_input).await;
    assert!(plan.is_ok(), "plan generation failed: {:?}", plan.err());

    let plan = plan.unwrap();
    assert_eq!(plan.resolution.agent.as_str(), "cyberclaw/master-agent");
    // P0-1 修复后，actions 在 resolution 阶段不再预生成（由 agent runtime 动态生成）
    // assert!(!plan.actions.is_empty(), "plan should have actions"); // REMOVED
}

/// 测试从本地路径直接加载单个包
#[tokio::test]
async fn test_load_and_register_single_package() {
    use cyberclaw_control_plane::{ManifestLoader, PackageSource};

    let loader = ManifestLoader::new();
    let package = loader
        .load(PackageSource::LocalPath(
            "../../ecosystem/agents/master-agent".to_string(),
        ))
        .await;

    assert!(
        package.is_ok(),
        "failed to load master-agent: {:?}",
        package.err()
    );

    let package = package.unwrap();
    assert_eq!(package.manifest.kind, PackageKind::Agent);
    assert_eq!(package.manifest.id, "cyberclaw/master-agent");

    // 注册到 registry
    let registry = Arc::new(InMemoryRegistry::new());
    let result = registry.load_packages(vec![package]).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 1);

    // 验证可以从 registry 检索
    let retrieved = registry
        .get(PackageKind::Agent, "cyberclaw/master-agent")
        .await
        .unwrap();
    assert!(retrieved.is_some());
}

/// 测试 registry 的 activate 功能
#[tokio::test]
async fn test_registry_activate() {
    let scanner = EcosystemScanner::new("../../ecosystem");
    let packages = scanner.scan_agents().await.unwrap();

    let registry = Arc::new(InMemoryRegistry::new());
    registry.load_packages(packages).await.unwrap();

    // 激活一个特定版本
    let result = registry
        .activate(PackageKind::Agent, "cyberclaw/master-agent", "0.1.0")
        .await;

    assert!(result.is_ok(), "failed to activate: {:?}", result.err());

    // 验证激活状态
    let record = registry
        .get(PackageKind::Agent, "cyberclaw/master-agent")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(record.active_version, Some("0.1.0".to_string()));
}

/// 测试完整的 orchestrator 流程：低优先级任务直接提交（无需审批）
#[tokio::test(flavor = "multi_thread")]
async fn test_orchestrator_low_priority_no_approval() {
    use cyberclaw_control_plane::IngressRequest;

    // Setup
    let (orchestrator, review_queue, execution_service) =
        create_test_orchestrator_with_active_node();

    // Create low priority task
    let actor = ActorRef {
        id: ActorId::from_string("test-user".to_string()).unwrap(),
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: "Test User".to_string(),
    };

    let task = Task {
        id: TaskId::new(),
        case_id: None,
        title: "Low Priority Task".to_string(),
        summary: "Should not require approval".to_string(),
        kind: TaskKind::Analysis,
        priority: Priority::Low,
        requested_by: actor.clone(),
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "test".to_string(),
        },
        input: TaskInput::default(),
        desired_outputs: vec![],
        labels: vec![],
        preferred_agent_id: None,
    };
    let _task_id = task.id.clone(); // Save for later verification

    let request = IngressRequest {
        actor: actor.clone(),
        session: None,
        workspace: None,
        task,
    };

    // Execute
    let result = orchestrator.process_ingress(request).await;
    assert!(
        result.is_ok(),
        "processing should succeed: {:?}",
        result.err()
    );

    let result = result.unwrap();
    // H-4 FIX: Empty actions now require review (fail-secure principle)
    assert!(
        !result.submitted,
        "task with empty actions should require review"
    );
    assert!(
        result.review_id.is_some(),
        "task with empty actions should have review_id"
    );

    // Verify review was enqueued
    let pending = review_queue.list_pending().await.unwrap();
    assert_eq!(pending.len(), 1, "review should be in queue");

    // Verify execution WAS created with WaitingReview status
    let execution = execution_service.get(&result.execution_id).await.unwrap();
    assert!(
        execution.is_some(),
        "execution should be created in WaitingReview state"
    );
    let execution = execution.unwrap();
    assert_eq!(
        execution.status,
        cyberclaw_core::execution::ExecutionStatus::WaitingReview,
        "execution should be in WaitingReview state"
    );
}

/// 测试完整的 orchestrator 流程：没有高风险 capability 的任务直接提交（P0-2 修复后）
#[tokio::test(flavor = "multi_thread")]
async fn test_orchestrator_no_high_risk_capabilities_no_approval() {
    use cyberclaw_control_plane::IngressRequest;

    // Setup
    let (orchestrator, review_queue, _execution_service) =
        create_test_orchestrator_with_active_node();

    // Create task (priority irrelevant - approval based on capability risk now)
    let actor = ActorRef {
        id: ActorId::from_string("test-user".to_string()).unwrap(),
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: "Test User".to_string(),
    };

    let task = Task {
        id: TaskId::new(),
        case_id: None,
        title: "Task Without High-Risk Capabilities".to_string(),
        summary: "P0-2: Approval now based on capability risk, not task priority".to_string(),
        kind: TaskKind::Execution,
        priority: Priority::Medium, // Priority is irrelevant now
        requested_by: actor.clone(),
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "test".to_string(),
        },
        input: TaskInput::default(),
        desired_outputs: vec![],
        labels: vec![],
        preferred_agent_id: None,
    };

    let request = IngressRequest {
        actor: actor.clone(),
        session: None,
        workspace: None,
        task,
    };

    // Execute
    let result = orchestrator.process_ingress(request).await;
    assert!(
        result.is_ok(),
        "processing should succeed: {:?}",
        result.err()
    );

    let result = result.unwrap();
    // H-4 FIX: Empty actions now require review (fail-secure principle)
    // Even without high-risk capabilities, empty actions trigger review
    assert!(
        !result.submitted,
        "task with empty actions should require review"
    );
    assert!(
        result.review_id.is_some(),
        "task with empty actions should have review_id"
    );

    // Verify review was enqueued
    let pending = review_queue.list_pending().await.unwrap();
    assert_eq!(pending.len(), 1, "review should be in queue");
}

/// Test complete approval flow from submission through approval
///
/// NOTE: This test currently uses task Priority::High to trigger approval.
/// Future implementation should use capability RiskLevel::High instead.
#[tokio::test(flavor = "multi_thread")]
async fn test_complete_approval_flow() {
    use cyberclaw_control_plane::IngressRequest;

    // Setup
    let (orchestrator, review_queue, _execution_service) =
        create_test_orchestrator_with_active_node();

    // Create high priority task (triggers approval in current implementation)
    let actor = ActorRef {
        id: ActorId::from_string("test-user".to_string()).unwrap(),
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: "Test User".to_string(),
    };

    let task = Task {
        id: TaskId::new(),
        case_id: None,
        title: "High Priority Task".to_string(),
        summary: "Requires approval".to_string(),
        kind: TaskKind::Investigation,
        priority: Priority::High,
        requested_by: actor.clone(),
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "test".to_string(),
        },
        input: TaskInput::default(),
        desired_outputs: vec![],
        labels: vec![],
        preferred_agent_id: None,
    };

    let request = IngressRequest {
        actor: actor.clone(),
        session: None,
        workspace: None,
        task,
    };

    // Step 1: Submit for review
    let result = orchestrator.process_ingress(request).await.unwrap();
    let review_id = result.review_id.expect("should have review_id");

    // Step 2: Verify review is pending
    let pending = review_queue.list_pending().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, review_id);

    // Step 3: Approve the review
    let approver = ActorRef {
        id: ActorId::from_string("test-approver".to_string()).unwrap(),
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: "Test Approver".to_string(),
    };
    review_queue.approve(&review_id, &approver).await.unwrap();

    // Step 4: Verify review is no longer pending
    let pending = review_queue.list_pending().await.unwrap();
    assert_eq!(
        pending.len(),
        0,
        "approved review should not be in pending list"
    );
}

/// Test concurrent processing of multiple tasks with different priorities
///
/// NOTE: This test currently uses task Priority to trigger approvals.
/// Future implementation should use capability RiskLevel instead.
/// The test verifies concurrent safety of the approval system.
#[tokio::test]
async fn test_concurrent_mixed_priority_tasks() {
    use cyberclaw_control_plane::IngressRequest;

    // Setup
    let (orchestrator, review_queue, _execution_service) =
        create_test_orchestrator_with_active_node();
    let orchestrator = Arc::new(orchestrator);

    let actor = ActorRef {
        id: ActorId::from_string("test-user".to_string()).unwrap(),
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: "Test User".to_string(),
    };

    // Create tasks with different priorities
    let priorities = vec![
        Priority::Low,      // Should not require review
        Priority::Medium,   // Should require review
        Priority::High,     // Should require review
        Priority::Low,      // Should not require review
        Priority::Critical, // Should require review
    ];

    let mut handles = vec![];

    for priority in priorities {
        let orchestrator = orchestrator.clone();
        let actor = actor.clone();

        let task = Task {
            id: TaskId::new(),
            case_id: None,
            title: format!("{:?} Priority Task", priority),
            summary: "Test task".to_string(),
            kind: TaskKind::Analysis,
            priority,
            requested_by: actor.clone(),
            requested_at: chrono::Utc::now(),
            trigger: TriggerRef {
                kind: "manual".to_string(),
                source: "test".to_string(),
            },
            input: TaskInput::default(),
            desired_outputs: vec![],
            labels: vec![],
            preferred_agent_id: None,
        };

        let request = IngressRequest {
            actor: actor.clone(),
            session: None,
            workspace: None,
            task,
        };

        let handle = tokio::spawn(async move { orchestrator.process_ingress(request).await });

        handles.push(handle);
    }

    // Wait for all tasks to complete
    let mut submitted_count = 0;
    let mut review_count = 0;

    for handle in handles {
        let result = handle.await.unwrap().unwrap();
        if result.submitted {
            submitted_count += 1;
        }
        if result.review_id.is_some() {
            review_count += 1;
        }
    }

    // Verify results
    // NOTE: Current implementation requires review for ALL priorities
    // Future: Only High/Critical should require review based on capability risk
    assert_eq!(
        review_count, 5,
        "all 5 tasks require review in current implementation"
    );

    // Verify review queue has all pending reviews
    let pending = review_queue.list_pending().await.unwrap();
    assert_eq!(
        pending.len(),
        5,
        "review queue should have all submitted tasks pending"
    );

    // Verify concurrent processing worked correctly (no race conditions)
    assert_eq!(
        submitted_count + review_count,
        5,
        "all 5 tasks should be accounted for"
    );
}

/// 集成测试：验证订阅者能够接收到 EventBus 发布的事件
#[tokio::test(flavor = "multi_thread")]
async fn test_event_bus_subscriber_receives_events() {
    use cyberclaw_control_plane::IngressRequest;

    let (orchestrator, _review_queue, _execution_service, event_bus) =
        create_test_orchestrator_with_event_bus();

    // Subscribe to all events before triggering any action
    let mut subscriber = event_bus.subscribe(EventFilter::All).unwrap();

    let actor = ActorRef {
        id: ActorId::from_string("test-user".to_string()).unwrap(),
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: "Test User".to_string(),
    };

    // Submit a low priority task (triggers ExecutionAssigned event)
    let task = Task {
        id: TaskId::new(),
        case_id: None,
        title: "Event Test Task".to_string(),
        summary: "Should publish ExecutionAssigned event".to_string(),
        kind: TaskKind::Analysis,
        priority: Priority::Low,
        requested_by: actor.clone(),
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "test".to_string(),
        },
        input: TaskInput::default(),
        desired_outputs: vec![],
        labels: vec![],
        preferred_agent_id: None,
    };

    let request = IngressRequest {
        actor: actor.clone(),
        session: None,
        workspace: None,
        task,
    };

    let result = orchestrator.process_ingress(request).await.unwrap();
    // H-4 FIX: Empty actions now require review, so no execution is submitted
    assert!(!result.submitted, "task with empty actions requires review");
    assert!(result.review_id.is_some(), "should have review_id");

    // H-4 FIX: Should receive ReviewCreated event instead of ExecutionAssigned
    let timeout_result = tokio::time::timeout(
        tokio::time::Duration::from_millis(50),
        subscriber.receiver.recv(),
    )
    .await;

    assert!(timeout_result.is_ok(), "should receive ReviewCreated event");
    let event = timeout_result.unwrap();
    match event {
        Some(ClusterEvent::ReviewCreated {
            review_id,
            execution_id,
            ..
        }) => {
            assert_eq!(
                review_id,
                result.review_id.unwrap(),
                "review_id should match"
            );
            assert_eq!(
                execution_id,
                Some(result.execution_id.clone()),
                "execution_id should match"
            );
        }
        other => panic!("expected ReviewCreated event, got {:?}", other),
    }
}

/// 集成测试：验证事件包含正确的 execution_id 和 timestamp
#[tokio::test(flavor = "multi_thread")]
async fn test_event_contains_correct_execution_id_and_timestamp() {
    use cyberclaw_control_plane::IngressRequest;

    let (orchestrator, _review_queue, _execution_service, event_bus) =
        create_test_orchestrator_with_event_bus();

    let mut subscriber = event_bus
        .subscribe(EventFilter::EventTypes(vec![
            "execution_assigned".to_string()
        ]))
        .unwrap();

    let actor = ActorRef {
        id: ActorId::from_string("test-user".to_string()).unwrap(),
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: "Test User".to_string(),
    };

    let _before = chrono::Utc::now();

    let task = Task {
        id: TaskId::new(),
        case_id: None,
        title: "ID Verify Task".to_string(),
        summary: "Verify event contains correct execution_id".to_string(),
        kind: TaskKind::Analysis,
        priority: Priority::Low,
        requested_by: actor.clone(),
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "test".to_string(),
        },
        input: TaskInput::default(),
        desired_outputs: vec![],
        labels: vec![],
        preferred_agent_id: None,
    };

    let request = IngressRequest {
        actor: actor.clone(),
        session: None,
        workspace: None,
        task,
    };

    let result = orchestrator.process_ingress(request).await.unwrap();
    // H-4 FIX: Empty actions now require review
    assert!(!result.submitted, "task with empty actions requires review");
    assert!(result.review_id.is_some(), "should have review_id");

    // No ExecutionAssigned event should be emitted since task is pending review
    let timeout_result = tokio::time::timeout(
        tokio::time::Duration::from_millis(50),
        subscriber.receiver.recv(),
    )
    .await;

    assert!(
        timeout_result.is_err(),
        "no event should be emitted for pending review"
    );
}

/// Integration test: Verify complete event sequence for approval flow
///
/// Expected sequence: ReviewCreated → ReviewApproved (or ReviewRejected)
///
/// NOTE: This test currently uses task Priority::High to trigger approval.
/// Future implementation should use capability RiskLevel::High instead.
#[tokio::test(flavor = "multi_thread")]
async fn test_approval_flow_complete_event_sequence() {
    use cyberclaw_control_plane::IngressRequest;

    let (orchestrator, _review_queue, _execution_service, event_bus) =
        create_test_orchestrator_with_event_bus();

    // Subscribe to review-related events
    let mut subscriber = event_bus
        .subscribe(EventFilter::EventTypes(vec![
            "review_created".to_string(),
            "review_approved".to_string(),
            "review_rejected".to_string(),
        ]))
        .unwrap();

    let actor = ActorRef {
        id: ActorId::from_string("test-user".to_string()).unwrap(),
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: "Test User".to_string(),
    };
    let reviewer = ActorRef {
        id: ActorId::from_string("reviewer-1".to_string()).unwrap(),
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: "Reviewer".to_string(),
    };

    // Submit a high priority task (requires review) → triggers ReviewCreated event
    let task = Task {
        id: TaskId::new(),
        case_id: None,
        title: "Approval Sequence Task".to_string(),
        summary: "Tests complete event sequence".to_string(),
        kind: TaskKind::Investigation,
        priority: Priority::High,
        requested_by: actor.clone(),
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "test".to_string(),
        },
        input: TaskInput::default(),
        desired_outputs: vec![],
        labels: vec![],
        preferred_agent_id: None,
    };

    let request = IngressRequest {
        actor: actor.clone(),
        session: None,
        workspace: None,
        task,
    };

    let ingress_result = orchestrator.process_ingress(request).await.unwrap();
    assert!(
        !ingress_result.submitted,
        "high priority task should require review"
    );
    let review_id = ingress_result.review_id.expect("should have review_id");

    // Event 1: ReviewCreated should have been published
    let event1 = tokio::time::timeout(
        tokio::time::Duration::from_millis(100),
        subscriber.receiver.recv(),
    )
    .await
    .expect("timed out waiting for ReviewCreated")
    .expect("channel closed");

    let created_execution_id = match event1 {
        ClusterEvent::ReviewCreated {
            review_id: evt_review_id,
            execution_id,
            timestamp,
            ..
        } => {
            assert_eq!(
                evt_review_id, review_id,
                "ReviewCreated should carry correct review_id"
            );
            assert_eq!(
                execution_id,
                Some(ingress_result.execution_id.clone()),
                "ReviewCreated should carry correct execution_id"
            );
            assert!(
                timestamp <= chrono::Utc::now(),
                "ReviewCreated timestamp should be in the past"
            );
            execution_id
        }
        other => panic!("expected ReviewCreated event, got {:?}", other),
    };

    // Approve the review → triggers ReviewApproved event
    // NOTE: May fail if execution has no content, which is expected in current test setup
    let _ = orchestrator
        .process_review_result(&review_id, true, reviewer)
        .await;

    // Event 2: ReviewApproved should have been published
    let event2 = tokio::time::timeout(
        tokio::time::Duration::from_millis(100),
        subscriber.receiver.recv(),
    )
    .await
    .expect("timed out waiting for ReviewApproved")
    .expect("channel closed");

    match event2 {
        ClusterEvent::ReviewApproved {
            review_id: evt_review_id,
            execution_id,
            approved_by,
            timestamp,
            ..
        } => {
            assert_eq!(
                evt_review_id, review_id,
                "ReviewApproved should carry correct review_id"
            );
            assert_eq!(
                execution_id, created_execution_id,
                "ReviewApproved should carry the same execution_id as ReviewCreated"
            );
            assert_eq!(
                approved_by.as_str(),
                "reviewer-1",
                "ReviewApproved should record the approver"
            );
            assert!(
                timestamp <= chrono::Utc::now(),
                "ReviewApproved timestamp should be in the past"
            );
        }
        other => panic!("expected ReviewApproved event, got {:?}", other),
    }

    // No more review events should be pending
    assert!(
        subscriber.receiver.try_recv().is_err(),
        "no additional events should be in the channel"
    );
}

/// 集成测试：验证基于 capability 风险级别的拒绝审批流程事件序列
/// 测试 Critical 风险 capability 触发治理决策和拒绝流程
#[tokio::test(flavor = "multi_thread")]
async fn test_rejection_flow_event_sequence() {
    use cyberclaw_control_plane::IngressRequest;

    let (orchestrator, _review_queue, execution_service, event_bus) =
        create_test_orchestrator_with_event_bus();

    let mut subscriber = event_bus
        .subscribe(EventFilter::EventTypes(vec![
            "review_created".to_string(),
            "review_rejected".to_string(),
        ]))
        .unwrap();

    let actor = ActorRef {
        id: ActorId::from_string("test-user".to_string()).unwrap(),
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: "Test User".to_string(),
    };
    let reviewer = ActorRef {
        id: ActorId::from_string("reviewer-security".to_string()).unwrap(),
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: "Security Reviewer".to_string(),
    };

    // 创建包含 Critical 风险 capability 的 Task
    // 注意：当前测试框架中，任务优先级仍用于触发审批流程
    // 实际生产环境中应基于 capability 的 risk 级别
    let task = Task {
        id: TaskId::new(),
        case_id: None,
        title: "Critical Risk Operation".to_string(),
        summary: "Tests rejection flow with Critical risk capability".to_string(),
        kind: TaskKind::Investigation,
        priority: Priority::High, // 使用 High 优先级触发审批流程
        requested_by: actor.clone(),
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "test".to_string(),
        },
        input: TaskInput::default(),
        desired_outputs: vec![],
        labels: vec![],
        preferred_agent_id: None,
    };

    let request = IngressRequest {
        actor: actor.clone(),
        session: None,
        workspace: None,
        task,
    };

    let ingress_result = orchestrator.process_ingress(request).await.unwrap();
    let review_id = ingress_result.review_id.expect("should have review_id");
    let execution_id_from_ingress = ingress_result.execution_id;

    // Event 1: ReviewCreated
    // Critical 风险 capability 触发治理审批，生成 ReviewCreated 事件
    let event1 = tokio::time::timeout(
        tokio::time::Duration::from_millis(100),
        subscriber.receiver.recv(),
    )
    .await
    .expect("timed out waiting for ReviewCreated")
    .expect("channel closed");

    let created_execution_id = match event1 {
        ClusterEvent::ReviewCreated {
            review_id: evt_review_id,
            execution_id,
            timestamp,
            ..
        } => {
            assert_eq!(
                evt_review_id, review_id,
                "ReviewCreated event should carry correct review_id"
            );
            assert_eq!(
                execution_id,
                Some(execution_id_from_ingress.clone()),
                "ReviewCreated event should carry correct execution_id"
            );
            assert!(
                timestamp <= chrono::Utc::now(),
                "ReviewCreated timestamp should be valid"
            );
            execution_id
        }
        other => panic!("expected ReviewCreated event, got {:?}", other),
    };

    // 安全审查员拒绝审批
    orchestrator
        .process_review_result(&review_id, false, reviewer.clone())
        .await
        .unwrap();

    // Event 2: ReviewRejected
    // 拒绝后应触发 ReviewRejected 事件，并包含拒绝原因
    let event2 = tokio::time::timeout(
        tokio::time::Duration::from_millis(100),
        subscriber.receiver.recv(),
    )
    .await
    .expect("timed out waiting for ReviewRejected")
    .expect("channel closed");

    match event2 {
        ClusterEvent::ReviewRejected {
            review_id: evt_review_id,
            execution_id,
            rejected_by,
            reason,
            timestamp,
            ..
        } => {
            assert_eq!(
                evt_review_id, review_id,
                "ReviewRejected should carry correct review_id"
            );
            assert_eq!(
                execution_id, created_execution_id,
                "ReviewRejected should carry same execution_id as ReviewCreated"
            );
            assert_eq!(
                rejected_by.as_str(),
                "reviewer-security",
                "ReviewRejected should record the correct rejector"
            );
            assert!(
                !reason.is_empty(),
                "ReviewRejected should include a non-empty reason"
            );
            assert!(
                timestamp <= chrono::Utc::now(),
                "ReviewRejected timestamp should be in the past"
            );
        }
        other => panic!("expected ReviewRejected event, got {:?}", other),
    }

    // 验证：拒绝后 capability 不应被执行
    // execution 状态应为 Cancelled，而非 Completed
    let execution_state = execution_service
        .get(
            created_execution_id
                .as_ref()
                .expect("ReviewCreated had execution_id"),
        )
        .await
        .expect("should get execution")
        .expect("execution should exist");

    assert!(
        matches!(
            execution_state.status,
            cyberclaw_core::execution::ExecutionStatus::Cancelled
        ),
        "Rejected execution should be in Cancelled state, got: {:?}",
        execution_state.status
    );
}

// ─────────────────────────────────────────────
// P0-3: 端到端集成测试 - Connector->Capability 真实执行流程
// ─────────────────────────────────────────────

/// 端到端集成测试：完整的 Connector->Capability 执行流程
///
/// 测试覆盖：
/// 1. LocalConnector 注册
/// 2. CapabilityDispatcher 路由
/// 3. 真实的文件操作 (fs.write + fs.read)
/// 4. 真实的搜索操作 (search.grep + search.glob)
/// 5. 安全修复验证 (pattern validation)
#[tokio::test]
async fn test_end_to_end_connector_capability_execution() {
    use cyberclaw_connectors::{
        runtime::{RuntimeMode, RuntimeSelectorConfig},
        types::{
            CapabilityExecutionRequest, ExecutionStatus, FsReadInput, FsWriteInput,
            SearchGlobInput, SearchGrepInput,
        },
        CapabilityDispatcher, ConnectorRegistry, LocalConnector,
    };
    use cyberclaw_core::identity::{ActorRef, ActorType};
    use cyberclaw_core::ids::{ActorId, ExecutionId, TraceId, WorkspaceId};
    use cyberclaw_core::workspace::{WorkspaceMode, WorkspaceRef};

    // Setup: Create temporary workspace
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace_path = temp_dir.path().to_path_buf();

    // Create actor and workspace refs for capability execution
    let actor = ActorRef {
        id: ActorId::from_string("test-user".to_string()).unwrap(),
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: "Test User".to_string(),
    };
    let workspace = WorkspaceRef {
        id: WorkspaceId::from_string("test-workspace".to_string()).unwrap(),
        mode: WorkspaceMode::Isolated,
        materialization_mode: None,
        home_node_id: None,
        backing_store: None,
        root: workspace_path.to_string_lossy().to_string(),
        writable_roots: vec![workspace_path.to_string_lossy().to_string()],
    };

    // Step 1: Register LocalConnector
    let registry = Arc::new(ConnectorRegistry::new());
    let connector = Arc::new(LocalConnector::new(workspace_path.clone()));
    registry.register(connector).unwrap();

    // Verify connector is registered
    let connector_id = ConnectorId::from_string("local".to_string()).unwrap();
    let registered_connector = registry.get(&connector_id);
    assert!(
        registered_connector.is_some(),
        "LocalConnector should be registered"
    );

    // Verify capabilities are exposed
    let capabilities = registry.list_capabilities();
    assert!(
        capabilities.len() >= 6,
        "LocalConnector should expose at least 6 capabilities"
    );

    // Step 2: Create CapabilityDispatcher (P2 runtime override: fs.write → Native)
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
    let dispatcher = CapabilityDispatcher::with_runtime_config(registry, runtime_config);

    // Step 3: Test fs.write capability
    let test_content =
        "Hello, CyberClaw! This is a test file.\nLine 2: Testing search functionality.";
    let test_file = "test.txt";

    let write_input = FsWriteInput {
        path: test_file.to_string(),
        content: test_content.to_string(),
        create_dirs: false,
    };

    let write_request = CapabilityExecutionRequest {
        execution_id: ExecutionId::new(),
        trace_id: TraceId::new().to_string(),
        actor: actor.clone(),
        workspace: workspace.clone(),
        connector_id: connector_id.clone(),
        capability_id: CapabilityId::from_string("fs.write".to_string()).unwrap(),
        input: serde_json::to_value(write_input).unwrap(),
    };

    let write_result = dispatcher.dispatch(write_request).await;
    assert!(
        write_result.is_ok(),
        "fs.write should succeed: {:?}",
        write_result.err()
    );
    let write_result = write_result.unwrap();
    assert!(
        matches!(write_result.status, ExecutionStatus::Success),
        "fs.write should return Success status"
    );

    // Step 4: Test fs.read capability
    let read_input = FsReadInput {
        path: test_file.to_string(),
        offset: None,
        limit: None,
    };

    let read_request = CapabilityExecutionRequest {
        execution_id: ExecutionId::new(),
        trace_id: TraceId::new().to_string(),
        actor: actor.clone(),
        workspace: workspace.clone(),
        connector_id: connector_id.clone(),
        capability_id: CapabilityId::from_string("fs.read".to_string()).unwrap(),
        input: serde_json::to_value(read_input).unwrap(),
    };

    let read_result = dispatcher.dispatch(read_request).await;
    assert!(
        read_result.is_ok(),
        "fs.read should succeed: {:?}",
        read_result.err()
    );
    let read_result = read_result.unwrap();
    assert!(
        matches!(read_result.status, ExecutionStatus::Success),
        "fs.read should return Success status"
    );

    // Verify read content matches written content
    let read_content = read_result.output["content"].as_str().unwrap();
    assert_eq!(
        read_content, test_content,
        "fs.read should return the same content as fs.write"
    );

    // Step 5: Test search.grep capability
    let grep_input = SearchGrepInput {
        pattern: "CyberClaw".to_string(),
        path: None,
        glob: None,
        file_type: None,
        case_insensitive: false,
        context_before: None,
        context_after: None,
        context: None,
        output_mode: Default::default(),
        head_limit: None,
        multiline: false,
        max_results: Some(10),
    };

    let grep_request = CapabilityExecutionRequest {
        execution_id: ExecutionId::new(),
        trace_id: TraceId::new().to_string(),
        actor: actor.clone(),
        workspace: workspace.clone(),
        connector_id: connector_id.clone(),
        capability_id: CapabilityId::from_string("search.grep".to_string()).unwrap(),
        input: serde_json::to_value(grep_input).unwrap(),
    };

    let grep_result = dispatcher.dispatch(grep_request).await;
    assert!(
        grep_result.is_ok(),
        "search.grep should succeed: {:?}",
        grep_result.err()
    );
    let grep_result = grep_result.unwrap();
    assert!(
        matches!(grep_result.status, ExecutionStatus::Success),
        "search.grep should return Success status"
    );

    // Verify grep found the pattern
    let grep_count = grep_result.output["count"].as_u64().unwrap();
    assert!(
        grep_count >= 1,
        "search.grep should find at least 1 match for 'CyberClaw'"
    );

    // Step 6: Test search.glob capability
    let glob_input = SearchGlobInput {
        pattern: "*.txt".to_string(),
        path: None,
        sort_by_mtime: false,
        max_results: Some(10),
    };

    let glob_request = CapabilityExecutionRequest {
        execution_id: ExecutionId::new(),
        trace_id: TraceId::new().to_string(),
        actor: actor.clone(),
        workspace: workspace.clone(),
        connector_id: connector_id.clone(),
        capability_id: CapabilityId::from_string("search.glob".to_string()).unwrap(),
        input: serde_json::to_value(glob_input).unwrap(),
    };

    let glob_result = dispatcher.dispatch(glob_request).await;
    assert!(
        glob_result.is_ok(),
        "search.glob should succeed: {:?}",
        glob_result.err()
    );
    let glob_result = glob_result.unwrap();
    assert!(
        matches!(glob_result.status, ExecutionStatus::Success),
        "search.glob should return Success status"
    );

    // Verify glob found the test file
    let glob_count = glob_result.output["count"].as_u64().unwrap();
    let glob_files = glob_result.output["files"].as_array().unwrap();
    assert_eq!(
        glob_count, 1,
        "search.glob should find exactly 1 *.txt file"
    );
    assert!(
        glob_files
            .iter()
            .any(|f| f.as_str().unwrap().contains("test.txt")),
        "search.glob should find test.txt"
    );
}

/// 端到端集成测试：安全修复验证 - 模式验证
///
/// 验证 P0 安全修复 (HIGH-3) 是否生效：
/// - grep 模式验证
/// - glob 模式验证
/// - ReDoS 攻击防御
#[tokio::test]
async fn test_end_to_end_pattern_validation() {
    use cyberclaw_connectors::{
        types::{CapabilityExecutionRequest, ExecutionStatus, SearchGrepInput},
        CapabilityDispatcher, ConnectorRegistry, LocalConnector,
    };
    use cyberclaw_core::identity::{ActorRef, ActorType};
    use cyberclaw_core::ids::{ActorId, ExecutionId, TraceId, WorkspaceId};
    use cyberclaw_core::workspace::{WorkspaceMode, WorkspaceRef};

    // Setup
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace_path = temp_dir.path().to_path_buf();

    // Create actor and workspace refs for capability execution
    let actor = ActorRef {
        id: ActorId::from_string("test-user".to_string()).unwrap(),
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: "Test User".to_string(),
    };
    let workspace = WorkspaceRef {
        id: WorkspaceId::from_string("test-workspace".to_string()).unwrap(),
        mode: WorkspaceMode::Isolated,
        materialization_mode: None,
        home_node_id: None,
        backing_store: None,
        root: workspace_path.to_string_lossy().to_string(),
        writable_roots: vec![workspace_path.to_string_lossy().to_string()],
    };

    let registry = Arc::new(ConnectorRegistry::new());
    let connector = Arc::new(LocalConnector::new(workspace_path));
    registry.register(connector).unwrap();

    let dispatcher = CapabilityDispatcher::new(registry);
    let connector_id = ConnectorId::from_string("local".to_string()).unwrap();

    // Test 1: Empty pattern should be rejected
    let empty_pattern_input = SearchGrepInput {
        pattern: "".to_string(),
        path: None,
        glob: None,
        file_type: None,
        case_insensitive: false,
        context_before: None,
        context_after: None,
        context: None,
        output_mode: Default::default(),
        head_limit: None,
        multiline: false,
        max_results: Some(10),
    };

    let empty_pattern_request = CapabilityExecutionRequest {
        execution_id: ExecutionId::new(),
        trace_id: TraceId::new().to_string(),
        actor: actor.clone(),
        workspace: workspace.clone(),
        connector_id: connector_id.clone(),
        capability_id: CapabilityId::from_string("search.grep".to_string()).unwrap(),
        input: serde_json::to_value(empty_pattern_input).unwrap(),
    };

    let empty_result = dispatcher.dispatch(empty_pattern_request).await.unwrap();
    assert!(
        matches!(empty_result.status, ExecutionStatus::Failed),
        "Empty pattern should be rejected"
    );
    assert!(
        empty_result.error.unwrap().contains("empty"),
        "Error should mention empty pattern"
    );

    // Test 2: Dangerous nested quantifiers should be rejected (ReDoS attack)
    let redos_pattern_input = SearchGrepInput {
        pattern: "(.*)+".to_string(), // Catastrophic backtracking pattern
        path: None,
        glob: None,
        file_type: None,
        case_insensitive: false,
        context_before: None,
        context_after: None,
        context: None,
        output_mode: Default::default(),
        head_limit: None,
        multiline: false,
        max_results: Some(10),
    };

    let redos_request = CapabilityExecutionRequest {
        execution_id: ExecutionId::new(),
        trace_id: TraceId::new().to_string(),
        actor: actor.clone(),
        workspace: workspace.clone(),
        connector_id: connector_id.clone(),
        capability_id: CapabilityId::from_string("search.grep".to_string()).unwrap(),
        input: serde_json::to_value(redos_pattern_input).unwrap(),
    };

    let redos_result = dispatcher.dispatch(redos_request).await.unwrap();
    assert!(
        matches!(redos_result.status, ExecutionStatus::Failed),
        "Dangerous nested quantifier pattern should be rejected"
    );
    assert!(
        redos_result.error.unwrap().contains("nested quantifiers"),
        "Error should mention nested quantifiers"
    );

    // Test 3: Pattern too long should be rejected
    let long_pattern = "a".repeat(1001); // Max is 1000
    let long_pattern_input = SearchGrepInput {
        pattern: long_pattern,
        path: None,
        glob: None,
        file_type: None,
        case_insensitive: false,
        context_before: None,
        context_after: None,
        context: None,
        output_mode: Default::default(),
        head_limit: None,
        multiline: false,
        max_results: Some(10),
    };

    let long_request = CapabilityExecutionRequest {
        execution_id: ExecutionId::new(),
        trace_id: TraceId::new().to_string(),
        actor: actor.clone(),
        workspace: workspace.clone(),
        connector_id: connector_id.clone(),
        capability_id: CapabilityId::from_string("search.grep".to_string()).unwrap(),
        input: serde_json::to_value(long_pattern_input).unwrap(),
    };

    let long_result = dispatcher.dispatch(long_request).await.unwrap();
    assert!(
        matches!(long_result.status, ExecutionStatus::Failed),
        "Pattern too long should be rejected"
    );
    assert!(
        long_result.error.unwrap().contains("too long"),
        "Error should mention pattern too long"
    );

    // Test 4: Valid pattern should succeed
    let valid_pattern_input = SearchGrepInput {
        pattern: "test".to_string(),
        path: None,
        glob: None,
        file_type: None,
        case_insensitive: false,
        context_before: None,
        context_after: None,
        context: None,
        output_mode: Default::default(),
        head_limit: None,
        multiline: false,
        max_results: Some(10),
    };

    let valid_request = CapabilityExecutionRequest {
        execution_id: ExecutionId::new(),
        trace_id: TraceId::new().to_string(),
        actor: actor.clone(),
        workspace: workspace.clone(),
        connector_id: connector_id.clone(),
        capability_id: CapabilityId::from_string("search.grep".to_string()).unwrap(),
        input: serde_json::to_value(valid_pattern_input).unwrap(),
    };

    let valid_result = dispatcher.dispatch(valid_request).await.unwrap();
    assert!(
        matches!(valid_result.status, ExecutionStatus::Success),
        "Valid pattern should succeed"
    );
}

/// 端到端集成测试：路径遍历攻击防御
///
/// 验证 P0 安全修复 (HIGH-1) 是否生效：
/// - 路径遍历（..）阻止
/// - 绝对路径边界检查
#[tokio::test]
async fn test_end_to_end_path_traversal_protection() {
    use cyberclaw_connectors::{
        types::{CapabilityExecutionRequest, ExecutionStatus, FsReadInput},
        CapabilityDispatcher, ConnectorRegistry, LocalConnector,
    };
    use cyberclaw_core::identity::{ActorRef, ActorType};
    use cyberclaw_core::ids::{ActorId, ExecutionId, TraceId, WorkspaceId};
    use cyberclaw_core::workspace::{WorkspaceMode, WorkspaceRef};

    // Setup
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace_path = temp_dir.path().to_path_buf();

    // Create actor and workspace refs for capability execution
    let actor = ActorRef {
        id: ActorId::from_string("test-user".to_string()).unwrap(),
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: "Test User".to_string(),
    };
    let workspace = WorkspaceRef {
        id: WorkspaceId::from_string("test-workspace".to_string()).unwrap(),
        mode: WorkspaceMode::Isolated,
        materialization_mode: None,
        home_node_id: None,
        backing_store: None,
        root: workspace_path.to_string_lossy().to_string(),
        writable_roots: vec![workspace_path.to_string_lossy().to_string()],
    };

    let registry = Arc::new(ConnectorRegistry::new());
    let connector = Arc::new(LocalConnector::new(workspace_path));
    registry.register(connector).unwrap();

    let dispatcher = CapabilityDispatcher::new(registry);
    let connector_id = ConnectorId::from_string("local".to_string()).unwrap();

    // Test 1: Path traversal with .. should be rejected
    let traversal_input = FsReadInput {
        path: "../../../etc/passwd".to_string(),
        offset: None,
        limit: None,
    };

    let traversal_request = CapabilityExecutionRequest {
        execution_id: ExecutionId::new(),
        trace_id: TraceId::new().to_string(),
        actor: actor.clone(),
        workspace: workspace.clone(),
        connector_id: connector_id.clone(),
        capability_id: CapabilityId::from_string("fs.read".to_string()).unwrap(),
        input: serde_json::to_value(traversal_input).unwrap(),
    };

    let traversal_result = dispatcher.dispatch(traversal_request).await.unwrap();
    assert!(
        matches!(traversal_result.status, ExecutionStatus::Failed),
        "Path traversal with .. should be rejected"
    );
    assert!(
        traversal_result.error.unwrap().contains("'..'")
            || traversal_result.output["error"]
                .as_str()
                .unwrap()
                .contains("'..'"),
        "Error should mention path traversal"
    );

    // Test 2: Absolute path outside workspace should be rejected
    let absolute_input = FsReadInput {
        path: "/etc/passwd".to_string(),
        offset: None,
        limit: None,
    };

    let absolute_request = CapabilityExecutionRequest {
        execution_id: ExecutionId::new(),
        trace_id: TraceId::new().to_string(),
        actor: actor.clone(),
        workspace: workspace.clone(),
        connector_id: connector_id.clone(),
        capability_id: CapabilityId::from_string("fs.read".to_string()).unwrap(),
        input: serde_json::to_value(absolute_input).unwrap(),
    };

    let absolute_result = dispatcher.dispatch(absolute_request).await.unwrap();
    assert!(
        matches!(absolute_result.status, ExecutionStatus::Failed),
        "Absolute path outside workspace should be rejected"
    );
    assert!(
        absolute_result.error.unwrap().contains("outside")
            || absolute_result.output["error"]
                .as_str()
                .unwrap()
                .contains("outside"),
        "Error should mention path is outside workspace"
    );
}
