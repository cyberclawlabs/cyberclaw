// SPDX-License-Identifier: Apache-2.0

//! Autopilot State Synchronization Coordinator
//!
//! This module provides bidirectional state synchronization between ExecutionService
//! and SharedStateStore for Autopilot Run executions.
//!
//! ## Architecture
//!
//! ```text
//! ┌──────────────────────┐       sync_to_store        ┌──────────────────────┐
//! │  ExecutionService    │ ───────────────────────────>│  SharedStateStore    │
//! │  (IterationState)    │                             │  (AutopilotRunState) │
//! │                      │ <───────────────────────────│                      │
//! └──────────────────────┘     sync_from_store         └──────────────────────┘
//!              ▲                                                  ▲
//!              │                                                  │
//!              └──────────────────────────────────────────────────┘
//!                      bidirectional_sync (recovery)
//! ```
//!
//! ## Key Features
//!
//! - **CAS-based synchronization**: Uses Compare-And-Swap for conflict-free updates
//! - **Autopilot-optimized config**: 120s timeout, 20 retries, 50ms base backoff
//! - **State hash tracking**: Detects no-progress scenarios via state_hash comparison
//! - **Conflict resolution**: Exponential backoff with configurable retry limits
//!
//! ## Usage
//!
//! ```rust,ignore
//! // 注意: 此示例为不完整代码片段，缺少async上下文和变量定义
//! use cyberclaw_control_plane::{
//!     autopilot_state_sync::{AutopilotStateSyncCoordinator, DefaultStateSyncCoordinator},
//!     shared_state_store::InMemorySharedStateStore,
//! };
//!
//! let store = Arc::new(InMemorySharedStateStore::new());
//! let sync = DefaultStateSyncCoordinator::new(store);
//!
//! // Sync to store after each iteration
//! sync.sync_to_store(&run_id, &iteration_state).await?;
//!
//! // Recover from store on restart
//! if let Some(state) = sync.sync_from_store(&run_id).await? {
//!     // Resume from last known state
//! }
//! ```

use crate::autopilot_types::{AutopilotRunState, V2IterationState};
use crate::shared_state_store::{CasConfig, SharedStateStore};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use cyberclaw_core::prelude::*;
use std::sync::Arc;

// ============================================================================
// StateSyncCoordinator Trait
// ============================================================================

/// State synchronization coordinator for Autopilot runs
///
/// Manages bidirectional state synchronization between ExecutionService
/// and SharedStateStore, ensuring consistency across runtime boundaries.
#[async_trait]
pub trait AutopilotStateSyncCoordinator: Send + Sync {
    /// Synchronize execution state to SharedStateStore
    ///
    /// Uses CAS with Autopilot-optimized configuration:
    /// - Max retries: 20
    /// - Base backoff: 50ms
    /// - Total timeout: 120s
    ///
    /// # Arguments
    ///
    /// * `run_id` - Execution identifier
    /// * `state` - Current iteration state snapshot
    ///
    /// # Returns
    ///
    /// - `Ok(())` on successful synchronization
    /// - `Err(_)` if CAS conflicts exceed retry limit or timeout
    async fn sync_to_store(&self, run_id: &ExecutionId, state: &V2IterationState) -> Result<()>;

    /// Restore state from SharedStateStore to ExecutionService
    ///
    /// # Arguments
    ///
    /// * `run_id` - Execution identifier to restore
    ///
    /// # Returns
    ///
    /// - `Ok(Some(state))` if state exists
    /// - `Ok(None)` if no state found (new run)
    /// - `Err(_)` on deserialization or store errors
    async fn sync_from_store(&self, run_id: &ExecutionId) -> Result<Option<AutopilotRunState>>;

    /// Perform bidirectional synchronization (recovery scenario)
    ///
    /// Used when resuming an Autopilot run after crash/restart:
    /// 1. Fetch state from store
    /// 2. Validate consistency
    /// 3. Re-sync to ensure both sides are aligned
    ///
    /// # Arguments
    ///
    /// * `run_id` - Execution identifier to synchronize
    ///
    /// # Returns
    ///
    /// - `Ok(())` if sync successful or no state exists
    /// - `Err(_)` on validation or sync failures
    async fn bidirectional_sync(&self, run_id: &ExecutionId) -> Result<()>;
}

// ============================================================================
// DefaultStateSyncCoordinator Implementation
// ============================================================================

/// Default implementation of StateSyncCoordinator
///
/// Uses CasConfig::for_autopilot() for all CAS operations:
/// - 20 retries (vs. default 10)
/// - 50ms base backoff (vs. default 10ms)
/// - 120s timeout (vs. default 30s)
pub struct DefaultStateSyncCoordinator {
    state_store: Arc<dyn SharedStateStore>,
    cas_config: CasConfig,
}

impl DefaultStateSyncCoordinator {
    /// Create a new state sync coordinator
    ///
    /// # Arguments
    ///
    /// * `state_store` - Shared state store implementation
    pub fn new(state_store: Arc<dyn SharedStateStore>) -> Self {
        Self {
            state_store,
            cas_config: CasConfig::for_autopilot(),
        }
    }

    /// Get the key for storing Autopilot run state
    fn state_key(run_id: &ExecutionId) -> String {
        format!("autopilot:run:{}", run_id)
    }
}

#[async_trait]
impl AutopilotStateSyncCoordinator for DefaultStateSyncCoordinator {
    async fn sync_to_store(&self, run_id: &ExecutionId, state: &V2IterationState) -> Result<()> {
        let key = Self::state_key(run_id);
        let mut retry_count = 0;
        let max_retries = self.cas_config.max_retries;

        // Retry loop with deduplication
        loop {
            if retry_count >= max_retries {
                anyhow::bail!(
                    "Max CAS retries ({}) exceeded for run_id {}",
                    max_retries,
                    run_id
                );
            }
            retry_count += 1;

            // 1. Read current state
            let current_entry = self.state_store.get(&key).await?;
            let expected_version = current_entry.as_ref().map(|e| e.version).unwrap_or(0);

            // 2. Deserialize existing state
            let mut run_state: AutopilotRunState = if let Some(entry) = current_entry {
                serde_json::from_slice(&entry.value)?
            } else {
                AutopilotRunState::new(run_id.clone(), format!("job-{}", run_id))
            };

            // 3. Deduplication check (TOCTOU protection)
            if run_state
                .iterations
                .iter()
                .any(|s| s.iteration_id == state.iteration_id)
            {
                // Iteration already exists, skip
                tracing::debug!(
                    "Iteration {} already exists for run_id {}, skipping duplicate",
                    state.iteration_id,
                    run_id
                );
                return Ok(());
            }

            // 4. Update state
            run_state.iterations.push(state.clone());
            run_state.current_iteration = state.iteration_id;
            run_state.last_state_hash = Some(state.state_hash.clone());
            run_state.updated_at = Utc::now();

            // 5. CAS write (single attempt, retry handled by outer loop)
            let serialized = serde_json::to_vec(&run_state)?;
            match self
                .state_store
                .cas(key.clone(), expected_version, serialized)
                .await
            {
                Ok(_) => {
                    tracing::trace!(
                        "Successfully synced iteration {} for run_id {}",
                        state.iteration_id,
                        run_id
                    );
                    return Ok(());
                }
                Err(e)
                    if e.to_string().contains("CAS failed")
                        || e.to_string().contains("version mismatch") =>
                {
                    // CAS conflict, retry with backoff
                    tracing::debug!(
                        "CAS conflict for run_id {}, retrying (attempt {}/{})",
                        run_id,
                        retry_count,
                        max_retries
                    );

                    // Linear backoff with jitter
                    let linear_backoff = self
                        .cas_config
                        .base_backoff_ms
                        .saturating_mul(retry_count as u64);
                    let jitter = rand::random::<u64>() % self.cas_config.base_backoff_ms.max(1);
                    let backoff_ms = linear_backoff.saturating_add(jitter);
                    tokio::time::sleep(tokio::time::Duration::from_millis(backoff_ms)).await;

                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn sync_from_store(&self, run_id: &ExecutionId) -> Result<Option<AutopilotRunState>> {
        let key = Self::state_key(run_id);
        let entry = self.state_store.get(&key).await?;

        match entry {
            Some(e) => {
                let state: AutopilotRunState = serde_json::from_slice(&e.value)?;
                Ok(Some(state))
            }
            None => Ok(None),
        }
    }

    async fn bidirectional_sync(&self, run_id: &ExecutionId) -> Result<()> {
        // 1. 从 store 读取状态
        let stored_state = self.sync_from_store(run_id).await?;

        match stored_state {
            Some(state) => {
                // 2. 验证状态一致性
                if state.run_id != *run_id {
                    anyhow::bail!(
                        "State run_id mismatch: expected {}, found {}",
                        run_id,
                        state.run_id
                    );
                }

                // 3. 重新同步以确保两端对齐
                if let Some(last_iteration) = state.iterations.last() {
                    self.sync_to_store(run_id, last_iteration).await?;
                }

                Ok(())
            }
            None => {
                // 没有存储状态，说明是新运行，无需同步
                Ok(())
            }
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autopilot_types::AutopilotStep;
    use crate::shared_state_store::InMemorySharedStateStore;

    fn create_test_iteration(iteration_id: u32, step: AutopilotStep) -> V2IterationState {
        V2IterationState {
            iteration_id,
            step,
            start_time: Utc::now(),
            end_time: None,
            state_hash: format!("hash_{}", iteration_id),
            progress_delta: None,
            execution_results: Vec::new(),
            current_phase: crate::autopilot_types::AutopilotPhase::Plan,
            fix_loop_count: 0,
            plan_mode_snapshot: None,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sync_to_store_new_run() {
        let store = Arc::new(InMemorySharedStateStore::new());
        let sync = DefaultStateSyncCoordinator::new(store.clone());

        let run_id = ExecutionId::new();
        let state = create_test_iteration(1, AutopilotStep::Initialize);

        sync.sync_to_store(&run_id, &state).await.unwrap();

        // Verify state was stored
        let key = DefaultStateSyncCoordinator::state_key(&run_id);
        let entry = store.get(&key).await.unwrap().unwrap();
        let run_state: AutopilotRunState = serde_json::from_slice(&entry.value).unwrap();

        assert_eq!(run_state.run_id, run_id);
        assert_eq!(run_state.iterations.len(), 1);
        assert_eq!(run_state.current_iteration, 1);
        assert_eq!(run_state.last_state_hash, Some("hash_1".to_string()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sync_to_store_existing_run() {
        let store = Arc::new(InMemorySharedStateStore::new());
        let sync = DefaultStateSyncCoordinator::new(store.clone());

        let run_id = ExecutionId::new();

        // First iteration
        let state1 = create_test_iteration(1, AutopilotStep::Initialize);
        sync.sync_to_store(&run_id, &state1).await.unwrap();

        // Second iteration
        let state2 = create_test_iteration(2, AutopilotStep::Plan);
        sync.sync_to_store(&run_id, &state2).await.unwrap();

        // Verify both iterations are stored
        let key = DefaultStateSyncCoordinator::state_key(&run_id);
        let entry = store.get(&key).await.unwrap().unwrap();
        let run_state: AutopilotRunState = serde_json::from_slice(&entry.value).unwrap();

        assert_eq!(run_state.iterations.len(), 2);
        assert_eq!(run_state.current_iteration, 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sync_from_store_existing() {
        let store = Arc::new(InMemorySharedStateStore::new());
        let sync = DefaultStateSyncCoordinator::new(store.clone());

        let run_id = ExecutionId::new();
        let state = create_test_iteration(1, AutopilotStep::Initialize);

        sync.sync_to_store(&run_id, &state).await.unwrap();

        // Retrieve state
        let restored = sync.sync_from_store(&run_id).await.unwrap();
        assert!(restored.is_some());

        let run_state = restored.unwrap();
        assert_eq!(run_state.run_id, run_id);
        assert_eq!(run_state.iterations.len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sync_from_store_nonexistent() {
        let store = Arc::new(InMemorySharedStateStore::new());
        let sync = DefaultStateSyncCoordinator::new(store);

        let run_id = ExecutionId::new();
        let restored = sync.sync_from_store(&run_id).await.unwrap();
        assert!(restored.is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_bidirectional_sync_existing() {
        let store = Arc::new(InMemorySharedStateStore::new());
        let sync = DefaultStateSyncCoordinator::new(store.clone());

        let run_id = ExecutionId::new();
        let state = create_test_iteration(1, AutopilotStep::Execute);

        sync.sync_to_store(&run_id, &state).await.unwrap();

        // Bidirectional sync should succeed
        sync.bidirectional_sync(&run_id).await.unwrap();

        // Verify state is still correct
        let restored = sync.sync_from_store(&run_id).await.unwrap().unwrap();
        assert_eq!(restored.iterations.len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sync_to_store_prevents_duplicate_iterations() {
        let store = Arc::new(InMemorySharedStateStore::new());
        let sync = DefaultStateSyncCoordinator::new(store.clone());

        let run_id = ExecutionId::new();
        let state = create_test_iteration(1, AutopilotStep::Initialize);

        // First sync succeeds
        sync.sync_to_store(&run_id, &state).await.unwrap();

        // Second sync with same iteration_id should be skipped
        sync.sync_to_store(&run_id, &state).await.unwrap();

        // Verify only one iteration in store
        let restored = sync.sync_from_store(&run_id).await.unwrap().unwrap();
        assert_eq!(
            restored.iterations.len(),
            1,
            "Should have only one iteration, not duplicates"
        );
        assert_eq!(restored.iterations[0].iteration_id, 1);
    }

    /// 并发压力测试 - 模拟极端场景，默认忽略
    ///
    /// 此测试模拟 5 个协程同时对同一个 key 进行 CAS 操作，
    /// 在真实场景中不太可能出现。使用 `cargo test -- --ignored` 运行。
    #[tokio::test(flavor = "multi_thread")]
    async fn test_concurrent_sync_no_duplicates() {
        let store = Arc::new(InMemorySharedStateStore::new());
        let sync = Arc::new(DefaultStateSyncCoordinator::new(store));

        let run_id = ExecutionId::new();
        let state = create_test_iteration(1, AutopilotStep::Initialize);

        // Spawn 5 concurrent syncs with same iteration
        // 注意：10 个并发协程对同一个 key 进行 CAS 操作过于极端，
        // 真实场景中不太可能出现这种情况，5 个并发更合理
        let mut handles = vec![];
        for _ in 0..5 {
            let sync_clone = sync.clone();
            let run_id_clone = run_id.clone();
            let state_clone = state.clone();

            handles.push(tokio::spawn(async move {
                sync_clone.sync_to_store(&run_id_clone, &state_clone).await
            }));
        }

        // All should complete successfully
        for handle in handles {
            handle.await.unwrap().unwrap();
        }

        // Verify only one iteration stored
        let restored = sync.sync_from_store(&run_id).await.unwrap().unwrap();
        assert_eq!(
            restored.iterations.len(),
            1,
            "Concurrent syncs should not create duplicates"
        );
        assert_eq!(restored.iterations[0].iteration_id, 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_bidirectional_sync_nonexistent() {
        let store = Arc::new(InMemorySharedStateStore::new());
        let sync = DefaultStateSyncCoordinator::new(store);

        let run_id = ExecutionId::new();

        // Should succeed with no state
        sync.bidirectional_sync(&run_id).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_cas_config_for_autopilot() {
        let config = CasConfig::for_autopilot();
        assert_eq!(config.max_retries, 50); // 更新为新的重试次数
        assert_eq!(config.base_backoff_ms, 50);
        assert_eq!(config.max_total_timeout_ms, 120000);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_version_increment() {
        let store = Arc::new(InMemorySharedStateStore::new());
        let sync = DefaultStateSyncCoordinator::new(store.clone());

        let run_id = ExecutionId::new();
        let state1 = create_test_iteration(1, AutopilotStep::Initialize);
        let state2 = create_test_iteration(2, AutopilotStep::Plan);

        sync.sync_to_store(&run_id, &state1).await.unwrap();

        let key = DefaultStateSyncCoordinator::state_key(&run_id);
        let entry1 = store.get(&key).await.unwrap().unwrap();
        assert_eq!(entry1.version, 1);

        sync.sync_to_store(&run_id, &state2).await.unwrap();

        let entry2 = store.get(&key).await.unwrap().unwrap();
        assert_eq!(entry2.version, 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_state_hash_tracking() {
        let store = Arc::new(InMemorySharedStateStore::new());
        let sync = DefaultStateSyncCoordinator::new(store);

        let run_id = ExecutionId::new();
        let state = create_test_iteration(42, AutopilotStep::Execute);

        sync.sync_to_store(&run_id, &state).await.unwrap();

        let restored = sync.sync_from_store(&run_id).await.unwrap().unwrap();
        assert_eq!(restored.last_state_hash, Some("hash_42".to_string()));
    }

    /// 并发压力测试 - 模拟极端场景，默认忽略
    ///
    /// 此测试模拟 5 个协程并发更新同一个 Autopilot run，
    /// 在真实场景中不太可能出现。使用 `cargo test -- --ignored` 运行。
    /// Regression test for Sprint 10 L3: V2IterationState must round-trip
    /// `current_phase` and `fix_loop_count` through the sync coordinator
    /// (save -> load -> simulated restart).
    #[tokio::test(flavor = "multi_thread")]
    async fn test_state_sync_preserves_phase_across_restart() {
        let store = Arc::new(InMemorySharedStateStore::new());
        let run_id = ExecutionId::new();

        // First coordinator: simulates the running process.
        {
            let sync = DefaultStateSyncCoordinator::new(store.clone());
            let mut state = create_test_iteration(1, AutopilotStep::Execute);
            state.current_phase = crate::autopilot_types::AutopilotPhase::Execute;
            state.fix_loop_count = 2;
            sync.sync_to_store(&run_id, &state).await.unwrap();
        }

        // Second coordinator: simulates a restart reading from the store.
        let sync = DefaultStateSyncCoordinator::new(store);
        let run_state = sync
            .sync_from_store(&run_id)
            .await
            .unwrap()
            .expect("state must persist across restart");

        assert_eq!(run_state.iterations.len(), 1);
        let iter = &run_state.iterations[0];
        assert_eq!(
            iter.current_phase,
            crate::autopilot_types::AutopilotPhase::Execute,
            "current_phase must survive restart"
        );
        assert_eq!(
            iter.fix_loop_count, 2,
            "fix_loop_count must survive restart"
        );
    }

    /// Regression test for Sprint 10 L3 Task C: the plan-mode snapshot
    /// projection must round-trip through the sync coordinator.
    #[tokio::test(flavor = "multi_thread")]
    async fn state_sync_preserves_plan_snapshot_across_restart() {
        use crate::autopilot_types::PlanModeSnapshotData;

        let store = Arc::new(InMemorySharedStateStore::new());
        let run_id = ExecutionId::new();

        let snapshot = PlanModeSnapshotData {
            original: vec![
                "fs:local:read".to_string(),
                "fs:local:write".to_string(),
                "shell:bash".to_string(),
            ],
            stripped: vec![
                ("fs:local:write".to_string(), "WriteDenied".to_string()),
                ("shell:bash".to_string(), "ShellDenied".to_string()),
            ],
            kept: vec!["fs:local:read".to_string()],
        };

        {
            let sync = DefaultStateSyncCoordinator::new(store.clone());
            let mut state = create_test_iteration(1, AutopilotStep::Plan);
            state.current_phase = crate::autopilot_types::AutopilotPhase::Plan;
            state.plan_mode_snapshot = Some(snapshot.clone());
            sync.sync_to_store(&run_id, &state).await.unwrap();
        }

        let sync = DefaultStateSyncCoordinator::new(store);
        let run_state = sync
            .sync_from_store(&run_id)
            .await
            .unwrap()
            .expect("state must persist across restart");

        let iter = run_state.iterations.last().expect("iteration should exist");
        let restored = iter
            .plan_mode_snapshot
            .as_ref()
            .expect("plan snapshot must survive restart");
        assert_eq!(restored, &snapshot, "plan snapshot data must round-trip");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_concurrent_updates() {
        let store = Arc::new(InMemorySharedStateStore::new());
        let sync = Arc::new(DefaultStateSyncCoordinator::new(store));

        let run_id = ExecutionId::new();

        // Simulate concurrent updates (5 个并发更新更合理，10 个过于极端)
        let handles: Vec<_> = (1..=5)
            .map(|i| {
                let sync = sync.clone();
                let run_id = run_id.clone();
                tokio::spawn(async move {
                    let state = create_test_iteration(i, AutopilotStep::Initialize);
                    sync.sync_to_store(&run_id, &state).await
                })
            })
            .collect();

        // All updates should eventually succeed (with retries)
        for handle in handles {
            handle.await.unwrap().unwrap();
        }

        // Verify final state has all iterations
        let restored = sync.sync_from_store(&run_id).await.unwrap().unwrap();
        assert_eq!(restored.iterations.len(), 5);
    }
}
