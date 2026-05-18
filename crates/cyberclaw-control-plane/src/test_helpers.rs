//! Test helper types and mock implementations
//!
//! This module provides simple in-memory implementations of various traits
//! for testing purposes. These are not meant for production use.

use anyhow::Result;
use async_trait::async_trait;
use cyberclaw_core::autopilot::{AutopilotJob, ExecutionResult, ProgressAnalysis};
use cyberclaw_core::prelude::*;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

#[allow(unused_imports)]
use crate::autopilot_iteration::{
    AutopilotIterationState, AutopilotIterationSummary, AutopilotIterationTracker,
};
use crate::autopilot_progress::ProgressEvaluator;
use crate::autopilot_runtime::SecurityGate;
use crate::autopilot_state_sync::AutopilotStateSyncCoordinator;
use crate::autopilot_types::V2IterationState;

// ============================================================================
// Workspace Factory
// ============================================================================

/// Simple in-memory workspace factory for testing
#[derive(Debug, Clone)]
pub struct InMemoryWorkspaceFactory {
    #[allow(dead_code)]
    default_root: PathBuf,
}

impl InMemoryWorkspaceFactory {
    pub fn new() -> Self {
        Self {
            default_root: PathBuf::from("/tmp/test-workspace"),
        }
    }

    pub fn with_root(root: PathBuf) -> Self {
        Self { default_root: root }
    }
}

impl Default for InMemoryWorkspaceFactory {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Progress Evaluator
// ============================================================================

/// Simple progress evaluator for testing
#[derive(Debug, Clone)]
pub struct SimpleProgressEvaluator {
    default_progress: f64,
}

impl SimpleProgressEvaluator {
    pub fn new() -> Self {
        Self {
            default_progress: 50.0,
        }
    }

    pub fn with_progress(progress: f64) -> Self {
        Self {
            default_progress: progress,
        }
    }
}

impl Default for SimpleProgressEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ProgressEvaluator for SimpleProgressEvaluator {
    async fn analyze(
        &self,
        _run_id: &AutopilotRunId,
        _results: &[ExecutionResult],
    ) -> Result<ProgressAnalysis> {
        Ok(ProgressAnalysis {
            has_progress: true,
            progress_delta: 10.0,
            decision: cyberclaw_core::autopilot::ProgressDecision::Continue,
            reasoning: "Mock progress analysis".to_string(),
            suggested_actions: Vec::new(),
        })
    }

    async fn is_goal_met(&self, _run_id: &AutopilotRunId, _job: &AutopilotJob) -> Result<bool> {
        Ok(false)
    }

    async fn get_completion_percentage(
        &self,
        _run_id: &AutopilotRunId,
        _job: &AutopilotJob,
    ) -> Result<f32> {
        Ok(self.default_progress as f32)
    }
}

// ============================================================================
// State Sync Coordinator
// ============================================================================

/// Simple in-memory state sync for testing
#[derive(Debug, Clone)]
pub struct InMemoryStateSync {
    states: Arc<RwLock<std::collections::HashMap<ExecutionId, V2IterationState>>>,
}

impl InMemoryStateSync {
    pub fn new() -> Self {
        Self {
            states: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }
}

impl Default for InMemoryStateSync {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AutopilotStateSyncCoordinator for InMemoryStateSync {
    async fn sync_to_store(
        &self,
        execution_id: &ExecutionId,
        state: &V2IterationState,
    ) -> Result<()> {
        let mut states = self.states.write().await;
        states.insert(execution_id.clone(), state.clone());
        Ok(())
    }

    async fn sync_from_store(
        &self,
        _execution_id: &ExecutionId,
    ) -> Result<Option<crate::autopilot_types::AutopilotRunState>> {
        // For testing, return None (no previous state)
        Ok(None)
    }

    async fn bidirectional_sync(&self, _execution_id: &ExecutionId) -> Result<()> {
        // For testing, no-op
        Ok(())
    }
}

// ============================================================================
// Automation Registry
// ============================================================================

/// Simple automation registry for testing
#[derive(Debug, Clone)]
pub struct AutomationRegistry {
    enabled: bool,
}

impl AutomationRegistry {
    pub fn new() -> Self {
        Self { enabled: true }
    }

    pub fn with_enabled(enabled: bool) -> Self {
        Self { enabled }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

impl Default for AutomationRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Security Gateway
// ============================================================================

/// Simple security gateway for testing
#[derive(Debug, Clone)]
pub struct SimpleSecurityGateway {
    always_pass: bool,
}

impl SimpleSecurityGateway {
    pub fn new() -> Self {
        Self { always_pass: true }
    }

    pub fn with_always_pass(always_pass: bool) -> Self {
        Self { always_pass }
    }
}

impl Default for SimpleSecurityGateway {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SecurityGate for SimpleSecurityGateway {
    async fn check_execution_results(
        &self,
        _results: &[ExecutionResult],
    ) -> Result<crate::autopilot_runtime::SecurityCheckResult> {
        Ok(crate::autopilot_runtime::SecurityCheckResult {
            passed: self.always_pass,
            issues: Vec::new(),
            requires_review: false,
        })
    }
}

// Note: InMemoryReviewQueue already exists in review_queue.rs and is re-exported
