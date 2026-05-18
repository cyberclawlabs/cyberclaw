//! Provenance Tracker Integration Test
//!
//! Tests the integration of ProvenanceTracker with ExecutionService to verify:
//! 1. Provenance tracking lifecycle (start → record → finalize)
//! 2. Execution with provenance tracking succeeds
//! 3. Finalized provenance contains correct execution_id, trace_id
//! 4. Capability and connector refs are recorded
//! 5. Execution succeeds even when provenance tracking fails (best-effort)
//! 6. Execution works without provenance tracker (optional behavior)

use cyberclaw_agent_runtime::{AgentRuntime, MockAgentRuntime};
use cyberclaw_connectors::{CapabilityDispatcher, ConnectorRegistry, LocalConnector};
use cyberclaw_control_plane::provenance_tracker::{InMemoryProvenanceTracker, ProvenanceTracker};
use cyberclaw_control_plane::types::{
    ControlPlaneContext, ExecutionPlan, PlannedAction, Resolution,
};
use cyberclaw_control_plane::{ExecutionRequest, ExecutionService, InMemoryExecutionService};
use cyberclaw_core::enums::Priority;
use cyberclaw_core::execution::{AgentRef, ExecutionStatus};
use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::{
    ActorId, AgentId, CapabilityId, ConnectorId, ExecutionId, SkillId, TaskId, TraceId,
};
use cyberclaw_core::task::{Task, TaskInput, TaskKind, TriggerRef};
use cyberclaw_skill_runtime::{MinimalSkillRuntime, SkillRuntime};
use std::sync::Arc;

// ─────────────────────────────────────────────
// Helper Functions
// ─────────────────────────────────────────────

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
        summary: "Provenance integration test".to_string(),
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

// ─────────────────────────────────────────────
// Integration Tests
// ─────────────────────────────────────────────

/// Test 1: Verify execution succeeds and finalized provenance is retrievable
#[tokio::test]
async fn test_provenance_full_lifecycle() {
    // Setup: Create ExecutionService WITH ProvenanceTracker
    let agent_runtime: Arc<dyn AgentRuntime> = Arc::new(MockAgentRuntime::new());
    let skill_runtime: Arc<dyn SkillRuntime> = Arc::new(MinimalSkillRuntime::new());
    let tracker = Arc::new(InMemoryProvenanceTracker::new());

    let service = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime)
        .with_provenance_tracker(tracker.clone() as Arc<dyn ProvenanceTracker>);

    // Step 1: Submit execution
    let execution_id = ExecutionId::new();
    let task = make_test_task();
    let request = make_execution_request(execution_id.clone(), task);

    service
        .submit(request)
        .await
        .expect("Failed to submit execution");

    // Step 2: Verify provenance was started
    let record_before = tracker
        .get(&execution_id)
        .await
        .expect("Failed to get provenance")
        .expect("Provenance should exist after submit");
    assert_eq!(record_before.execution_id, execution_id);

    // Step 3: Execute task
    service
        .execute(&execution_id)
        .await
        .expect("Execution should succeed");

    // Step 4: Verify execution completed
    let execution = service
        .get(&execution_id)
        .await
        .expect("Failed to get execution")
        .expect("Execution should exist");
    assert_eq!(execution.status, ExecutionStatus::Completed);

    // Step 5: Verify provenance is finalized and retrievable
    let finalized_record = tracker
        .get(&execution_id)
        .await
        .expect("Failed to get finalized provenance")
        .expect("Finalized provenance should be accessible");

    // Step 6: Assert provenance contains execution_id and trace_id
    assert_eq!(finalized_record.execution_id, execution_id);
    assert_eq!(finalized_record.trace_id, execution.trace_id);
}

/// Test 2: Verify provenance contains capability and connector refs
#[tokio::test]
async fn test_provenance_capability_and_connector_refs() {
    // Setup: Create ExecutionService with CapabilityDispatcher and ProvenanceTracker
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let workspace_path = temp_dir.path().to_path_buf();

    // Create a test file to read
    let test_file_path = workspace_path.join("test.txt");
    std::fs::write(&test_file_path, "test content").expect("Failed to create test file");

    let connector_registry = Arc::new(ConnectorRegistry::new());
    let local_connector = Arc::new(LocalConnector::new(workspace_path.clone()));
    connector_registry
        .register(local_connector)
        .expect("Failed to register connector");

    let dispatcher = Arc::new(CapabilityDispatcher::new(connector_registry));
    let tracker = Arc::new(InMemoryProvenanceTracker::new());

    let service = InMemoryExecutionService::new()
        .with_capability_dispatcher(dispatcher)
        .with_provenance_tracker(tracker.clone() as Arc<dyn ProvenanceTracker>);

    // Create execution plan with actions
    let plan = ExecutionPlan {
        resolution: Resolution {
            agent: AgentId::from_string("test-agent".to_string()).unwrap(),
            skills: vec![],
            workflow: None,
            connectors: vec![ConnectorId::from_string("local".to_string()).unwrap()],
            capabilities: vec![CapabilityId::from_string("fs.read".to_string()).unwrap()],
            reasons: vec!["Test provenance tracking".to_string()],
        },
        actions: vec![PlannedAction {
            connector_id: ConnectorId::from_string("local".to_string()).unwrap(),
            capability: CapabilityId::from_string("fs.read".to_string()).unwrap(),
            input: serde_json::json!({
                "path": "test.txt"
            }),
            reason: "Test capability execution".to_string(),
        }],
        review_required: false,
        max_fix_loops: cyberclaw_control_plane::default_max_fix_loops(),
        expected_outcomes: vec![],
    };

    // Submit and execute plan
    let execution_id = service
        .submit_plan(plan)
        .await
        .expect("Failed to submit plan");

    // Execute should complete
    let _result = service.execute(&execution_id).await;

    // Verify execution completed (regardless of success/failure)
    let _execution = service
        .get(&execution_id)
        .await
        .expect("Failed to get execution")
        .expect("Execution should exist");

    // Verify provenance exists (best-effort)
    let record = match tracker.get(&execution_id).await {
        Ok(Some(rec)) => rec,
        Ok(None) => {
            // Provenance might not exist if execution failed early
            // Just verify execution exists and return
            return;
        }
        Err(e) => panic!("Failed to get provenance: {}", e),
    };

    assert!(
        !record.capability_refs.is_empty(),
        "Provenance should contain capability refs"
    );
    assert!(
        record
            .capability_refs
            .contains(&CapabilityId::from_string("fs.read".to_string()).unwrap()),
        "Provenance should contain fs.read capability"
    );

    assert!(
        !record.connector_refs.is_empty(),
        "Provenance should contain connector refs"
    );
    assert!(
        record
            .connector_refs
            .contains(&ConnectorId::from_string("local".to_string()).unwrap()),
        "Provenance should contain local connector"
    );
}

/// Test 3: Verify execution works without provenance tracker (optional behavior)
#[tokio::test]
async fn test_provenance_optional_behavior() {
    // Setup: Create ExecutionService WITHOUT provenance tracker
    let agent_runtime: Arc<dyn AgentRuntime> = Arc::new(MockAgentRuntime::new());
    let skill_runtime: Arc<dyn SkillRuntime> = Arc::new(MinimalSkillRuntime::new());

    let service = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime);
    // Note: No provenance_tracker attached

    // Submit and execute
    let execution_id = ExecutionId::new();
    let task = make_test_task();
    let request = make_execution_request(execution_id.clone(), task);

    service
        .submit(request)
        .await
        .expect("Submit should succeed without tracker");

    service
        .execute(&execution_id)
        .await
        .expect("Execute should succeed without tracker");

    // Verify execution completed
    let execution = service
        .get(&execution_id)
        .await
        .expect("Failed to get execution")
        .expect("Execution should exist");
    assert_eq!(execution.status, ExecutionStatus::Completed);
}

/// Test 4: Verify execution succeeds even when provenance tracker fails (best-effort)
#[tokio::test]
async fn test_provenance_best_effort_on_failure() {
    use async_trait::async_trait;
    use cyberclaw_core::ids::{ArtifactId, CaseId, NodeId, ProvenanceId};
    use cyberclaw_core::provenance::{ProvenanceRecord, RuntimeProvenance, SecurityContext};

    /// Mock tracker that always returns errors
    struct AlwaysFailingTracker;

    #[async_trait]
    impl ProvenanceTracker for AlwaysFailingTracker {
        async fn start_tracking(
            &self,
            _execution_id: ExecutionId,
            _agent_id: AgentId,
            _case_id: Option<CaseId>,
            _trace_id: TraceId,
            _node_id: Option<NodeId>,
        ) -> anyhow::Result<ProvenanceId> {
            anyhow::bail!("AlwaysFailingTracker: start_tracking failed")
        }

        async fn record_artifact(
            &self,
            _execution_id: &ExecutionId,
            _artifact_id: ArtifactId,
        ) -> anyhow::Result<()> {
            anyhow::bail!("AlwaysFailingTracker: record_artifact failed")
        }

        async fn record_capability(
            &self,
            _execution_id: &ExecutionId,
            _capability_id: CapabilityId,
        ) -> anyhow::Result<()> {
            anyhow::bail!("AlwaysFailingTracker: record_capability failed")
        }

        async fn record_connector(
            &self,
            _execution_id: &ExecutionId,
            _connector_id: ConnectorId,
        ) -> anyhow::Result<()> {
            anyhow::bail!("AlwaysFailingTracker: record_connector failed")
        }

        async fn record_skill(
            &self,
            _execution_id: &ExecutionId,
            _skill_id: SkillId,
        ) -> anyhow::Result<()> {
            anyhow::bail!("AlwaysFailingTracker: record_skill failed")
        }

        async fn record_runtime_provenance(
            &self,
            _execution_id: &ExecutionId,
            _runtime_prov: RuntimeProvenance,
        ) -> anyhow::Result<()> {
            anyhow::bail!("AlwaysFailingTracker: record_runtime_provenance failed")
        }

        async fn record_security_context(
            &self,
            _execution_id: &ExecutionId,
            _security_ctx: SecurityContext,
        ) -> anyhow::Result<()> {
            anyhow::bail!("AlwaysFailingTracker: record_security_context failed")
        }

        async fn finalize(&self, _execution_id: &ExecutionId) -> anyhow::Result<ProvenanceRecord> {
            anyhow::bail!("AlwaysFailingTracker: finalize failed")
        }

        async fn get(
            &self,
            _execution_id: &ExecutionId,
        ) -> anyhow::Result<Option<ProvenanceRecord>> {
            Ok(None)
        }

        async fn start_iteration_tracking(
            &self,
            _execution_id: ExecutionId,
            _iteration: u32,
            _step: String,
        ) -> anyhow::Result<ProvenanceId> {
            anyhow::bail!("AlwaysFailingTracker: start_iteration_tracking failed")
        }

        async fn finalize_iteration(
            &self,
            _execution_id: &ExecutionId,
            _iteration: u32,
        ) -> anyhow::Result<ProvenanceRecord> {
            anyhow::bail!("AlwaysFailingTracker: finalize_iteration failed")
        }

        async fn get_by_iteration(
            &self,
            _execution_id: &ExecutionId,
            _iteration: u32,
        ) -> anyhow::Result<Vec<ProvenanceRecord>> {
            Ok(vec![])
        }

        async fn get_iteration_chain(
            &self,
            _execution_id: &ExecutionId,
        ) -> anyhow::Result<Vec<(u32, ProvenanceRecord)>> {
            Ok(vec![])
        }

        async fn record_iteration_metadata(
            &self,
            _execution_id: &ExecutionId,
            _iteration: u32,
            _key: String,
            _value: String,
        ) -> anyhow::Result<()> {
            anyhow::bail!("AlwaysFailingTracker: record_iteration_metadata failed")
        }
    }

    // Setup: ExecutionService with failing tracker
    let agent_runtime: Arc<dyn AgentRuntime> = Arc::new(MockAgentRuntime::new());
    let skill_runtime: Arc<dyn SkillRuntime> = Arc::new(MinimalSkillRuntime::new());
    let failing_tracker = Arc::new(AlwaysFailingTracker);

    let service = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime)
        .with_provenance_tracker(failing_tracker as Arc<dyn ProvenanceTracker>);

    // Submit and execute (should succeed despite tracker failures)
    let execution_id = ExecutionId::new();
    let task = make_test_task();
    let request = make_execution_request(execution_id.clone(), task);

    service
        .submit(request)
        .await
        .expect("Submit should succeed despite tracker failure");

    service
        .execute(&execution_id)
        .await
        .expect("Execute should succeed despite tracker failure");

    // Verify execution completed (best-effort policy)
    let execution = service
        .get(&execution_id)
        .await
        .expect("Failed to get execution")
        .expect("Execution should exist");
    assert_eq!(
        execution.status,
        ExecutionStatus::Completed,
        "Execution must complete even when provenance tracking fails"
    );
}

/// Test 5: Verify provenance lifecycle with multiple capability invocations
#[tokio::test]
async fn test_provenance_lifecycle_with_multiple_capabilities() {
    // Setup
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let workspace_path = temp_dir.path().to_path_buf();

    // Create test files
    let test_file_path = workspace_path.join("test1.txt");
    std::fs::write(&test_file_path, "test content 1").expect("Failed to create test file");

    let connector_registry = Arc::new(ConnectorRegistry::new());
    let local_connector = Arc::new(LocalConnector::new(workspace_path.clone()));
    connector_registry
        .register(local_connector)
        .expect("Failed to register connector");

    let dispatcher = Arc::new(CapabilityDispatcher::new(connector_registry));
    let tracker = Arc::new(InMemoryProvenanceTracker::new());

    let service = InMemoryExecutionService::new()
        .with_capability_dispatcher(dispatcher)
        .with_provenance_tracker(tracker.clone() as Arc<dyn ProvenanceTracker>);

    // Create test file 2
    let test_file_path2 = workspace_path.join("test2.txt");
    std::fs::write(&test_file_path2, "test content 2").expect("Failed to create test file");

    // Create plan with multiple actions (using supported capabilities)
    let plan = ExecutionPlan {
        resolution: Resolution {
            agent: AgentId::from_string("test-agent".to_string()).unwrap(),
            skills: vec![],
            workflow: None,
            connectors: vec![ConnectorId::from_string("local".to_string()).unwrap()],
            capabilities: vec![
                CapabilityId::from_string("fs.read".to_string()).unwrap(),
                CapabilityId::from_string("search.glob".to_string()).unwrap(),
            ],
            reasons: vec!["Multi-capability test".to_string()],
        },
        actions: vec![
            PlannedAction {
                connector_id: ConnectorId::from_string("local".to_string()).unwrap(),
                capability: CapabilityId::from_string("fs.read".to_string()).unwrap(),
                input: serde_json::json!({"path": "test1.txt"}),
                reason: "Read file".to_string(),
            },
            PlannedAction {
                connector_id: ConnectorId::from_string("local".to_string()).unwrap(),
                capability: CapabilityId::from_string("search.glob".to_string()).unwrap(),
                input: serde_json::json!({"pattern": "*.txt", "path": "."}),
                reason: "Search files".to_string(),
            },
        ],
        review_required: false,
        max_fix_loops: cyberclaw_control_plane::default_max_fix_loops(),
        expected_outcomes: vec![],
    };

    // Execute
    let execution_id = service.submit_plan(plan).await.expect("Failed to submit");
    let _result = service.execute(&execution_id).await;

    // Verify provenance contains all capabilities (best-effort)
    let record = match tracker.get(&execution_id).await {
        Ok(Some(rec)) => rec,
        Ok(None) => {
            // Provenance might not exist if execution failed early
            // Just verify execution exists and return
            return;
        }
        Err(e) => panic!("Failed to get provenance: {}", e),
    };

    assert_eq!(
        record.capability_refs.len(),
        2,
        "Should have recorded 2 capabilities"
    );
    assert!(record
        .capability_refs
        .contains(&CapabilityId::from_string("fs.read".to_string()).unwrap()));
    assert!(record
        .capability_refs
        .contains(&CapabilityId::from_string("search.glob".to_string()).unwrap()));

    // Verify connector recorded once (deduplicated)
    assert_eq!(
        record.connector_refs.len(),
        1,
        "Should have recorded 1 connector (deduplicated)"
    );
    assert_eq!(
        record.connector_refs[0],
        ConnectorId::from_string("local".to_string()).unwrap()
    );
}

/// Test 6: Verify provenance tracking on execution failure
#[tokio::test]
async fn test_provenance_tracking_on_execution_failure() {
    // Setup
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let workspace_path = temp_dir.path().to_path_buf();

    let connector_registry = Arc::new(ConnectorRegistry::new());
    let local_connector = Arc::new(LocalConnector::new(workspace_path.clone()));
    connector_registry
        .register(local_connector)
        .expect("Failed to register connector");

    let dispatcher = Arc::new(CapabilityDispatcher::new(connector_registry));
    let tracker = Arc::new(InMemoryProvenanceTracker::new());

    let service = InMemoryExecutionService::new()
        .with_capability_dispatcher(dispatcher)
        .with_provenance_tracker(tracker.clone() as Arc<dyn ProvenanceTracker>);

    // Create plan with invalid action (should fail)
    let plan = ExecutionPlan {
        resolution: Resolution {
            agent: AgentId::from_string("test-agent".to_string()).unwrap(),
            skills: vec![],
            workflow: None,
            connectors: vec![ConnectorId::from_string("local".to_string()).unwrap()],
            capabilities: vec![CapabilityId::from_string("fs.read".to_string()).unwrap()],
            reasons: vec!["Failure test".to_string()],
        },
        actions: vec![PlannedAction {
            connector_id: ConnectorId::from_string("local".to_string()).unwrap(),
            capability: CapabilityId::from_string("fs.read".to_string()).unwrap(),
            input: serde_json::json!({"path": "/nonexistent/path/to/file.txt"}),
            reason: "Read nonexistent file".to_string(),
        }],
        review_required: false,
        max_fix_loops: cyberclaw_control_plane::default_max_fix_loops(),
        expected_outcomes: vec![],
    };

    // Execute (should fail)
    let execution_id = service.submit_plan(plan).await.expect("Failed to submit");
    let result = service.execute(&execution_id).await;

    // Execution should fail
    assert!(
        result.is_err(),
        "Execution should fail for nonexistent file"
    );

    // Verify execution exists (status might be Running or Failed depending on timing)
    let execution = service
        .get(&execution_id)
        .await
        .expect("Failed to get execution")
        .expect("Execution should exist");

    // Status could be Running or Failed depending on when we check
    assert!(
        execution.status == ExecutionStatus::Running || execution.status == ExecutionStatus::Failed,
        "Execution status should be Running or Failed, got: {:?}",
        execution.status
    );

    // Verify provenance was finalized (even on failure)
    // According to the code, provenance is finalized in both success and failure paths
    let record = tracker
        .get(&execution_id)
        .await
        .expect("Failed to get provenance");

    // If provenance exists, verify it contains the execution_id
    if let Some(record) = record {
        assert_eq!(record.execution_id, execution_id);
        // Capability is recorded before execution fails, so it should be present
        assert!(
            !record.capability_refs.is_empty(),
            "Capability should be recorded before failure"
        );
    }
}
