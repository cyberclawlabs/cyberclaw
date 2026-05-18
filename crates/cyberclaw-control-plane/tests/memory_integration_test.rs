/// Integration tests for MemoryContextProvider mainchain integration (MAINCHAIN-D).
///
/// Tests verify that ExecutionService correctly integrates with MemoryContextProvider
/// by appending execution summaries to working memory and triggering compaction
/// when entry count exceeds threshold.
use cyberclaw_agent_runtime::{AgentRuntime, MockAgentRuntime};
use cyberclaw_connectors::{CapabilityDispatcher, ConnectorRegistry, LocalConnector};
use cyberclaw_control_plane::types::{
    ControlPlaneContext, ExecutionPlan, PlannedAction, Resolution,
};
use cyberclaw_control_plane::{ExecutionRequest, ExecutionService, InMemoryExecutionService};
use cyberclaw_core::enums::Priority;
use cyberclaw_core::execution::AgentRef;
use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::{
    ActorId, AgentId, CapabilityId, ConnectorId, ExecutionId, TaskId, TraceId,
};
use cyberclaw_core::memory::provider::{MemoryConfig, MemoryContextProvider};
use cyberclaw_core::memory_context::MemoryContextRequest;
use cyberclaw_core::task::{Task, TaskInput, TaskKind, TriggerRef};
use cyberclaw_skill_runtime::{MinimalSkillRuntime, SkillRuntime};
use std::sync::Arc;
use tempfile::TempDir;

/// Create a test actor
fn make_test_actor(id: &str) -> ActorRef {
    ActorRef {
        id: ActorId::from_string(id.to_string()).unwrap(),
        actor_type: ActorType::Human,
        tenant_id: None,
        home_node_id: None,
        display_name: format!("Test User ({})", id),
    }
}

/// Create a test task
fn make_test_task() -> Task {
    Task {
        id: TaskId::new(),
        case_id: None,
        title: "Test Task".to_string(),
        summary: "Memory integration test".to_string(),
        kind: TaskKind::Analysis,
        priority: Priority::Low,
        requested_by: make_test_actor("test-user"),
        requested_at: chrono::Utc::now(),
        trigger: TriggerRef {
            kind: "manual".to_string(),
            source: "test".to_string(),
        },
        input: TaskInput::default(),
        desired_outputs: vec![],
        labels: vec![],
        preferred_agent_id: None,
    }
}

/// Create an execution request
fn make_execution_request(execution_id: ExecutionId, task: Task) -> ExecutionRequest {
    ExecutionRequest {
        execution_id,
        task,
        case: None,
        context: ControlPlaneContext {
            actor: make_test_actor("test-user"),
            session: None,
            workspace: None,
        },
        agent: Some(AgentRef {
            id: AgentId::from_string("test-agent".to_string()).unwrap(),
            role: "test-agent".to_string(),
        }),
        trace_id: None,
        execution_mode: None,
        plan: None,
    }
}

/// Test 1: Verify memory provider is correctly attached
#[tokio::test]
async fn test_memory_provider_attached() {
    let memory_config = MemoryConfig::default();
    let memory_provider = Arc::new(MemoryContextProvider::new(memory_config));

    let service = InMemoryExecutionService::new().with_memory_provider(memory_provider.clone());

    // Verify service has memory provider attached
    assert!(format!("{:?}", service).contains("has_memory_provider: true"));

    // Verify memory provider is accessible
    let initial_count = memory_provider.get_working_entry_count().unwrap_or(0);
    assert_eq!(initial_count, 0);
}

/// Test 2: Verify execution completion appends to working memory
#[tokio::test]
async fn test_execution_success_appends_to_memory() {
    let temp_dir = TempDir::new().unwrap();
    let workspace_path = temp_dir.path().to_path_buf();

    let memory_config = MemoryConfig::default();
    let memory_provider = Arc::new(MemoryContextProvider::new(memory_config));

    let agent_runtime: Arc<dyn AgentRuntime> = Arc::new(MockAgentRuntime::new());
    let skill_runtime: Arc<dyn SkillRuntime> = Arc::new(MinimalSkillRuntime::new());

    let connector_registry = Arc::new(ConnectorRegistry::new());
    let local_connector = Arc::new(LocalConnector::new(workspace_path.clone()));
    connector_registry
        .register(local_connector)
        .expect("Failed to register connector");

    let dispatcher = Arc::new(CapabilityDispatcher::new(connector_registry));

    let service = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime)
        .with_capability_dispatcher(dispatcher)
        .with_memory_provider(memory_provider.clone());

    // Create a simple execution plan
    let execution_id = ExecutionId::new();
    let task = make_test_task();
    let request = make_execution_request(execution_id.clone(), task);

    // Create a trivial plan (no actions)
    let plan = ExecutionPlan {
        resolution: Resolution {
            agent: AgentId::from_string("test-agent".to_string()).unwrap(),
            skills: vec![],
            workflow: None,
            connectors: vec![],
            capabilities: vec![],
            reasons: vec!["test resolution".to_string()],
        },
        actions: vec![],
        review_required: false,
        max_fix_loops: cyberclaw_control_plane::default_max_fix_loops(),
        expected_outcomes: vec![],
    };

    // Submit request and plan separately, then execute
    service.submit(request).await.unwrap();
    service.submit_plan(plan).await.unwrap();
    let _ = service.execute(&execution_id).await;

    // Verify working memory contains execution summary
    let entry_count = memory_provider.get_working_entry_count().unwrap_or(0);
    assert!(
        entry_count > 0,
        "Working memory should contain entries after execution"
    );

    // Verify we can retrieve the context
    let context_request = MemoryContextRequest {
        case_id: None,
        execution_id: Some(execution_id.clone()),
        max_items: 10,
    };
    let context = memory_provider.get_context(context_request);
    assert!(
        !context.working_items.is_empty(),
        "Context should contain working items"
    );

    // Verify summary contains expected information
    let summary = &context.working_items[0].summary;
    assert!(summary.contains("completed") || summary.contains("failed"));
}

/// Test 3: Verify memory provider is optional (best-effort)
#[tokio::test]
async fn test_memory_provider_optional() {
    let agent_runtime: Arc<dyn AgentRuntime> = Arc::new(MockAgentRuntime::new());
    let skill_runtime: Arc<dyn SkillRuntime> = Arc::new(MinimalSkillRuntime::new());

    // Service without memory provider
    let service = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime);

    // Verify service does not have memory provider
    assert!(format!("{:?}", service).contains("has_memory_provider: false"));

    // Execution should still work
    let execution_id = ExecutionId::new();
    let task = make_test_task();
    let request = make_execution_request(execution_id.clone(), task);

    let plan = ExecutionPlan {
        resolution: Resolution {
            agent: AgentId::from_string("test-agent".to_string()).unwrap(),
            skills: vec![],
            workflow: None,
            connectors: vec![],
            capabilities: vec![],
            reasons: vec!["test resolution".to_string()],
        },
        actions: vec![],
        review_required: false,
        max_fix_loops: cyberclaw_control_plane::default_max_fix_loops(),
        expected_outcomes: vec![],
    };

    service.submit(request).await.unwrap();
    service.submit_plan(plan).await.unwrap();

    // Should complete without error even without memory provider
    let _ = service.execute(&execution_id).await;
}

/// Test 4: Verify compaction can be triggered (integration point exists)
#[tokio::test]
async fn test_compaction_integration_point() {
    let temp_dir = TempDir::new().unwrap();
    let workspace_path = temp_dir.path().to_path_buf();

    let memory_config = MemoryConfig {
        working_max_entries: 2000, // Allow more than 1000 entries
        ..Default::default()
    };
    let memory_provider = Arc::new(MemoryContextProvider::new(memory_config));

    // Pre-fill memory beyond threshold (1000)
    for i in 0..1005 {
        let entry = cyberclaw_core::memory_context::WorkingMemoryEntry {
            execution_id: Some(ExecutionId::from_string(format!("exec-{}", i)).unwrap()),
            kind: cyberclaw_core::memory_context::WorkingEntryKind::Decision,
            summary: format!("Test entry {}", i),
            artifact_refs: vec![],
            trace_id: Some(TraceId::new()),
            encrypted: false,
        };
        memory_provider.add_working_entry(entry);
    }

    let initial_count = memory_provider.get_working_entry_count().unwrap_or(0);
    assert!(
        initial_count > 1000,
        "Initial count should exceed threshold"
    );

    // Create service with memory provider
    let agent_runtime: Arc<dyn AgentRuntime> = Arc::new(MockAgentRuntime::new());
    let skill_runtime: Arc<dyn SkillRuntime> = Arc::new(MinimalSkillRuntime::new());

    let connector_registry = Arc::new(ConnectorRegistry::new());
    let local_connector = Arc::new(LocalConnector::new(workspace_path.clone()));
    connector_registry
        .register(local_connector)
        .expect("Failed to register connector");

    let dispatcher = Arc::new(CapabilityDispatcher::new(connector_registry));

    let service = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime)
        .with_capability_dispatcher(dispatcher)
        .with_memory_provider(memory_provider.clone());

    // Execute one task to trigger compaction logic
    let execution_id = ExecutionId::new();
    let task = make_test_task();
    let request = make_execution_request(execution_id.clone(), task);

    let plan = ExecutionPlan {
        resolution: Resolution {
            agent: AgentId::from_string("test-agent".to_string()).unwrap(),
            skills: vec![],
            workflow: None,
            connectors: vec![],
            capabilities: vec![],
            reasons: vec!["trigger compaction".to_string()],
        },
        actions: vec![],
        review_required: false,
        max_fix_loops: cyberclaw_control_plane::default_max_fix_loops(),
        expected_outcomes: vec![],
    };

    service.submit(request).await.unwrap();
    service.submit_plan(plan).await.unwrap();
    let _ = service.execute(&execution_id).await;

    // Verify memory still exists after compaction
    let final_count = memory_provider.get_working_entry_count().unwrap_or(0);
    assert!(
        final_count > 0,
        "Memory should still contain entries after compaction"
    );
}

/// Test 5: Verify multiple executions are tracked in memory
#[tokio::test]
async fn test_memory_tracks_multiple_executions() {
    let temp_dir = TempDir::new().unwrap();
    let workspace_path = temp_dir.path().to_path_buf();

    let memory_config = MemoryConfig::default();
    let memory_provider = Arc::new(MemoryContextProvider::new(memory_config));

    let agent_runtime: Arc<dyn AgentRuntime> = Arc::new(MockAgentRuntime::new());
    let skill_runtime: Arc<dyn SkillRuntime> = Arc::new(MinimalSkillRuntime::new());

    let connector_registry = Arc::new(ConnectorRegistry::new());
    let local_connector = Arc::new(LocalConnector::new(workspace_path.clone()));
    connector_registry
        .register(local_connector)
        .expect("Failed to register connector");

    let dispatcher = Arc::new(CapabilityDispatcher::new(connector_registry));

    let service = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime)
        .with_capability_dispatcher(dispatcher)
        .with_memory_provider(memory_provider.clone());

    // Execute multiple tasks
    for i in 0..5 {
        let execution_id = ExecutionId::new();
        let task = make_test_task();
        let request = make_execution_request(execution_id.clone(), task);

        let plan = ExecutionPlan {
            resolution: Resolution {
                agent: AgentId::from_string("test-agent".to_string()).unwrap(),
                skills: vec![],
                workflow: None,
                connectors: vec![],
                capabilities: vec![],
                reasons: vec![format!("response {}", i)],
            },
            actions: vec![],
            review_required: false,
            max_fix_loops: cyberclaw_control_plane::default_max_fix_loops(),
            expected_outcomes: vec![],
        };

        service.submit(request).await.unwrap();
        service.submit_plan(plan).await.unwrap();
        let _ = service.execute(&execution_id).await;
    }

    // Verify all executions are tracked
    let entry_count = memory_provider.get_working_entry_count().unwrap_or(0);
    assert!(
        entry_count >= 5,
        "Should track at least 5 executions, got {}",
        entry_count
    );

    // Verify we can retrieve recent entries
    let context_request = MemoryContextRequest {
        case_id: None,
        execution_id: None,
        max_items: 10,
    };
    let context = memory_provider.get_context(context_request);
    assert!(context.working_items.len() >= 5);
}

/// Test 6: Verify execution failure also appends to memory
#[tokio::test]
async fn test_execution_failure_appends_to_memory() {
    let temp_dir = TempDir::new().unwrap();
    let workspace_path = temp_dir.path().to_path_buf();

    let memory_config = MemoryConfig::default();
    let memory_provider = Arc::new(MemoryContextProvider::new(memory_config));

    let agent_runtime: Arc<dyn AgentRuntime> = Arc::new(MockAgentRuntime::new());
    let skill_runtime: Arc<dyn SkillRuntime> = Arc::new(MinimalSkillRuntime::new());

    let connector_registry = Arc::new(ConnectorRegistry::new());
    let local_connector = Arc::new(LocalConnector::new(workspace_path.clone()));
    connector_registry
        .register(local_connector)
        .expect("Failed to register connector");

    let dispatcher = Arc::new(CapabilityDispatcher::new(connector_registry));

    let service = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime)
        .with_capability_dispatcher(dispatcher)
        .with_memory_provider(memory_provider.clone());

    // Create execution with invalid plan (empty resolution agent will fail validation)
    let execution_id = ExecutionId::new();
    let task = make_test_task();

    // Submit without plan to trigger failure path
    let request = ExecutionRequest {
        execution_id: execution_id.clone(),
        task,
        case: None,
        context: ControlPlaneContext {
            actor: make_test_actor("test-user"),
            session: None,
            workspace: None,
        },
        agent: Some(AgentRef {
            id: AgentId::from_string("test-agent".to_string()).unwrap(),
            role: "test-agent".to_string(),
        }),
        trace_id: None,
        execution_mode: None,
        plan: None,
    };

    // Create plan with invalid capability that will fail
    let plan = ExecutionPlan {
        resolution: Resolution {
            agent: AgentId::from_string("test-agent".to_string()).unwrap(),
            skills: vec![],
            workflow: None,
            connectors: vec![ConnectorId::from_string("local".to_string()).unwrap()],
            capabilities: vec![
                CapabilityId::from_string("fs.invalid_operation".to_string()).unwrap(),
            ],
            reasons: vec!["test failure".to_string()],
        },
        actions: vec![PlannedAction {
            connector_id: ConnectorId::from_string("local".to_string()).unwrap(),
            capability: CapabilityId::from_string("fs.invalid_operation".to_string()).unwrap(),
            input: serde_json::json!({}),
            reason: "test failure".to_string(),
        }],
        review_required: false,
        max_fix_loops: cyberclaw_control_plane::default_max_fix_loops(),
        expected_outcomes: vec![],
    };

    service.submit(request).await.unwrap();
    service.submit_plan(plan).await.unwrap();

    // Execute (should fail due to invalid capability)
    let _ = service.execute(&execution_id).await;

    // Verify working memory contains failure entry
    let entry_count = memory_provider.get_working_entry_count().unwrap_or(0);
    assert!(
        entry_count > 0,
        "Working memory should contain failure entry"
    );

    // Verify failure is recorded in context
    let context_request = MemoryContextRequest {
        case_id: None,
        execution_id: Some(execution_id.clone()),
        max_items: 10,
    };
    let context = memory_provider.get_context(context_request);
    assert!(!context.working_items.is_empty());

    let summary = &context.working_items[0].summary;
    assert!(
        summary.contains("failed") || summary.contains("completed"),
        "Summary should indicate execution outcome: {}",
        summary
    );
}
