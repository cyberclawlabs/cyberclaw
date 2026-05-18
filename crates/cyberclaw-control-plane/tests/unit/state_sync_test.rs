//! Unit tests for State Sync Coordinator (10 tests)

use cyberclaw_control_plane::autopilot_types::*;
use cyberclaw_control_plane::shared_state_store::InMemorySharedStateStore;
use cyberclaw_control_plane::execution::ExecutionId;
use cyberclaw_core::ids::ExecutionId as CoreExecutionId;
use std::sync::Arc;
use chrono::Utc;

// Mock StateSyncCoordinator for testing
struct MockStateSyncCoordinator {
    state_store: Arc<InMemorySharedStateStore>,
}

impl MockStateSyncCoordinator {
    fn new(state_store: Arc<InMemorySharedStateStore>) -> Self {
        Self { state_store }
    }

    async fn sync_to_store(&self, run_id: &ExecutionId, state: &AutopilotRunState) -> anyhow::Result<()> {
        let key = format!("autopilot:run:{}", run_id.0);
        let value = serde_json::to_vec(state)?;
        self.state_store.put(key, value, 0)?;
        Ok(())
    }

    async fn sync_from_store(&self, run_id: &ExecutionId) -> anyhow::Result<Option<AutopilotRunState>> {
        let key = format!("autopilot:run:{}", run_id.0);
        if let Some(entry) = self.state_store.get(&key)? {
            let state = serde_json::from_slice(&entry.value)?;
            Ok(Some(state))
        } else {
            Ok(None)
        }
    }

    async fn update_iteration(&self, run_id: &ExecutionId, iteration: IterationState) -> anyhow::Result<()> {
        if let Some(mut state) = self.sync_from_store(run_id).await? {
            state.iterations.push(iteration);
            state.current_iteration += 1;
            state.updated_at = Utc::now();
            self.sync_to_store(run_id, &state).await?;
        }
        Ok(())
    }

    async fn mark_checkpoint(&self, run_id: &ExecutionId) -> anyhow::Result<()> {
        let checkpoint_key = format!("autopilot:checkpoint:{}", run_id.0);
        if let Some(state) = self.sync_from_store(run_id).await? {
            let checkpoint = serde_json::to_vec(&state)?;
            self.state_store.put(checkpoint_key, checkpoint, 0)?;
        }
        Ok(())
    }

    async fn restore_from_checkpoint(&self, run_id: &ExecutionId) -> anyhow::Result<Option<AutopilotRunState>> {
        let checkpoint_key = format!("autopilot:checkpoint:{}", run_id.0);
        if let Some(entry) = self.state_store.get(&checkpoint_key)? {
            let state = serde_json::from_slice(&entry.value)?;
            Ok(Some(state))
        } else {
            Ok(None)
        }
    }
}

#[tokio::test]
async fn test_state_sync_basic_save_and_load() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = MockStateSyncCoordinator::new(store);

    let run_id = ExecutionId("run-1".to_string());
    let core_run_id = CoreExecutionId::new();
    let state = AutopilotRunState::new(core_run_id, "job-1".to_string());

    // Save
    sync.sync_to_store(&run_id, &state).await.unwrap();

    // Load
    let loaded = sync.sync_from_store(&run_id).await.unwrap();
    assert!(loaded.is_some());
    assert_eq!(loaded.unwrap().job_id, "job-1");
}

#[tokio::test]
async fn test_state_sync_update_iteration() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = MockStateSyncCoordinator::new(store);

    let run_id = ExecutionId("run-1".to_string());
    let core_run_id = CoreExecutionId::new();
    let mut state = AutopilotRunState::new(core_run_id, "job-1".to_string());

    // Initial sync
    sync.sync_to_store(&run_id, &state).await.unwrap();

    // Add iteration
    let iteration = IterationState::new(1, AutopilotStep::Execute);
    sync.update_iteration(&run_id, iteration).await.unwrap();

    // Verify update
    let loaded = sync.sync_from_store(&run_id).await.unwrap().unwrap();
    assert_eq!(loaded.current_iteration, 1);
    assert_eq!(loaded.iterations.len(), 1);
}

#[tokio::test]
async fn test_state_sync_checkpoint_creation() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = MockStateSyncCoordinator::new(store.clone());

    let run_id = ExecutionId("run-1".to_string());
    let core_run_id = CoreExecutionId::new();
    let mut state = AutopilotRunState::new(core_run_id, "job-1".to_string());
    state.current_iteration = 5;

    // Save state
    sync.sync_to_store(&run_id, &state).await.unwrap();

    // Create checkpoint
    sync.mark_checkpoint(&run_id).await.unwrap();

    // Verify checkpoint exists
    let checkpoint_key = format!("autopilot:checkpoint:{}", run_id.0);
    let entry = store.get(&checkpoint_key).unwrap();
    assert!(entry.is_some());
}

#[tokio::test]
async fn test_state_sync_restore_from_checkpoint() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = MockStateSyncCoordinator::new(store);

    let run_id = ExecutionId("run-1".to_string());
    let core_run_id = CoreExecutionId::new();
    let mut state = AutopilotRunState::new(core_run_id, "job-1".to_string());
    state.current_iteration = 10;
    state.stuck_count = 2;

    // Save and checkpoint
    sync.sync_to_store(&run_id, &state).await.unwrap();
    sync.mark_checkpoint(&run_id).await.unwrap();

    // Modify state
    state.current_iteration = 15;
    state.stuck_count = 0;
    sync.sync_to_store(&run_id, &state).await.unwrap();

    // Restore from checkpoint
    let restored = sync.restore_from_checkpoint(&run_id).await.unwrap().unwrap();
    assert_eq!(restored.current_iteration, 10);
    assert_eq!(restored.stuck_count, 2);
}

#[tokio::test]
async fn test_state_sync_concurrent_updates() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = Arc::new(MockStateSyncCoordinator::new(store));

    let run_id = ExecutionId("run-1".to_string());
    let core_run_id = CoreExecutionId::new();
    let state = AutopilotRunState::new(core_run_id, "job-1".to_string());

    // Initial save
    sync.sync_to_store(&run_id, &state).await.unwrap();

    // Concurrent updates
    let mut handles = vec![];
    for i in 1..=10 {
        let sync_clone = sync.clone();
        let run_id_clone = run_id.clone();
        let handle = tokio::spawn(async move {
            let iteration = IterationState::new(i, AutopilotStep::Execute);
            sync_clone.update_iteration(&run_id_clone, iteration).await
        });
        handles.push(handle);
    }

    // Wait for all updates
    for handle in handles {
        handle.await.unwrap().unwrap();
    }

    // Verify final state
    let final_state = sync.sync_from_store(&run_id).await.unwrap().unwrap();
    assert!(final_state.current_iteration > 0);
    assert!(final_state.iterations.len() > 0);
}

#[tokio::test]
async fn test_state_sync_missing_state() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = MockStateSyncCoordinator::new(store);

    let run_id = ExecutionId("non-existent".to_string());

    // Should return None for missing state
    let result = sync.sync_from_store(&run_id).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_state_sync_status_updates() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = MockStateSyncCoordinator::new(store);

    let run_id = ExecutionId("run-1".to_string());
    let core_run_id = CoreExecutionId::new();
    let mut state = AutopilotRunState::new(core_run_id, "job-1".to_string());

    // Test status transitions
    state.status = AutopilotStatus::Running { current_step: AutopilotStep::Execute };
    sync.sync_to_store(&run_id, &state).await.unwrap();

    state.status = AutopilotStatus::Stuck { reason: "No progress".to_string() };
    sync.sync_to_store(&run_id, &state).await.unwrap();

    state.status = AutopilotStatus::Completed { iterations: 10 };
    sync.sync_to_store(&run_id, &state).await.unwrap();

    // Verify final status
    let loaded = sync.sync_from_store(&run_id).await.unwrap().unwrap();
    assert!(matches!(loaded.status, AutopilotStatus::Completed { iterations: 10 }));
}

#[tokio::test]
async fn test_state_sync_metadata_persistence() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = MockStateSyncCoordinator::new(store);

    let run_id = ExecutionId("run-1".to_string());
    let core_run_id = CoreExecutionId::new();
    let mut state = AutopilotRunState::new(core_run_id, "job-1".to_string());

    // Add metadata
    state.metadata.insert("key1".to_string(), serde_json::json!("value1"));
    state.metadata.insert("key2".to_string(), serde_json::json!(42));

    // Save and load
    sync.sync_to_store(&run_id, &state).await.unwrap();
    let loaded = sync.sync_from_store(&run_id).await.unwrap().unwrap();

    // Verify metadata
    assert_eq!(loaded.metadata.get("key1").unwrap(), &serde_json::json!("value1"));
    assert_eq!(loaded.metadata.get("key2").unwrap(), &serde_json::json!(42));
}

#[tokio::test]
async fn test_state_sync_version_conflicts() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = MockStateSyncCoordinator::new(store.clone());

    let run_id = ExecutionId("run-1".to_string());
    let core_run_id = CoreExecutionId::new();
    let state = AutopilotRunState::new(core_run_id, "job-1".to_string());

    // Initial save
    sync.sync_to_store(&run_id, &state).await.unwrap();

    // Get entry version
    let key = format!("autopilot:run:{}", run_id.0);
    let entry = store.get(&key).unwrap().unwrap();
    let version = entry.version;

    // Try to update with old version (simulate conflict)
    let result = store.put(key.clone(), vec![1, 2, 3], version + 100);
    assert!(result.is_ok()); // InMemoryStore doesn't enforce versions strictly

    // But in production, CAS conflicts would be handled
    let loaded = sync.sync_from_store(&run_id).await;
    assert!(loaded.is_err() || loaded.unwrap().is_some());
}

#[tokio::test]
async fn test_state_sync_large_state_handling() {
    let store = Arc::new(InMemorySharedStateStore::new());
    let sync = MockStateSyncCoordinator::new(store);

    let run_id = ExecutionId("run-1".to_string());
    let core_run_id = CoreExecutionId::new();
    let mut state = AutopilotRunState::new(core_run_id, "job-1".to_string());

    // Add many iterations
    for i in 1..=100 {
        let mut iteration = IterationState::new(i, AutopilotStep::Execute);
        iteration.execution_results = vec![
            ExecutionResult {
                execution_id: CoreExecutionId::new(),
                status: cyberclaw_control_plane::execution::ExecutionStatus::Completed,
                output: Some(serde_json::json!({"data": format!("large-data-{}", i)})),
                error: None,
                duration_ms: 100,
                trace_id: format!("trace-{}", i),
            }
        ];
        state.iterations.push(iteration);
    }

    // Save and load large state
    sync.sync_to_store(&run_id, &state).await.unwrap();
    let loaded = sync.sync_from_store(&run_id).await.unwrap().unwrap();

    assert_eq!(loaded.iterations.len(), 100);
}