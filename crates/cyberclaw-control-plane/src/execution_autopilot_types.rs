//! Autopilot execution types and traits (V1).
//!
//! This module contains the core types and traits used by the execution service
//! for Autopilot mode, including execution mode selection, iteration results,
//! step definitions, and the traits for iteration tracking, state synchronization,
//! stuck detection, checkpoint storage, and step execution.
//!
//! Note: The V2 autopilot types (AutopilotRunState, AutopilotStatus, etc.) are
//! defined in the `autopilot_types` module.

use async_trait::async_trait;
use cyberclaw_core::ids::ExecutionId;

// ─── Autopilot Types ─────────────────────────────────────────────────────────

// ExecutionMode is now defined in cyberclaw_core::execution::ExecutionMode
// (Normal / Autopilot / Persistent). Re-export from core for backward compatibility.
pub use cyberclaw_core::execution::ExecutionMode;

/// Result of a single Autopilot iteration
#[derive(Debug, Clone)]
pub struct IterationResult {
    pub iteration: u32,
    pub steps_completed: Vec<AutopilotStep>,
    pub decision: Decision,
    pub progress_made: bool,
    pub output: Option<serde_json::Value>,
    pub errors: Vec<String>,
}

/// Decision made at the end of an iteration
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Continue to next iteration
    Continue,
    /// Goal has been met, stop iterating
    GoalMet,
    /// No progress detected, execution stuck
    Stuck,
}

/// Individual steps in the Autopilot 9-step loop
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum AutopilotStep {
    Plan,
    Execute,
    Review,
    Analyze,
    Decide,
    Update,
    Check,
    Iterate,
    Finalize,
}

/// Result of executing a single step
#[derive(Debug, Clone)]
pub struct StepResult {
    pub step: AutopilotStep,
    pub success: bool,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

/// Resolution when stuck is detected
#[derive(Debug, Clone)]
pub enum StuckResolution {
    /// Retry with different approach
    Retry { approach: String },
    /// Escalate to human operator
    Escalate,
    /// Abort execution
    Abort,
}

/// State of an iteration for checkpoint/resume
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IterationState {
    pub iteration: u32,
    pub current_step: AutopilotStep,
    pub steps_completed: Vec<AutopilotStep>,
    pub context: serde_json::Value,
    pub memory_snapshot: Vec<String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Summary of a completed iteration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IterationSummary {
    pub iteration: u32,
    pub start_time: chrono::DateTime<chrono::Utc>,
    pub end_time: chrono::DateTime<chrono::Utc>,
    pub steps_completed: Vec<AutopilotStep>,
    pub decision: String,
    pub progress_made: bool,
}

// ─── End Autopilot Types ─────────────────────────────────────────────────────

// ─── Autopilot Traits ─────────────────────────────────────────────────────────

/// Trait for tracking iteration progress
#[async_trait]
pub trait IterationTracker: Send + Sync {
    /// Get current iteration number for an execution
    async fn current_iteration(&self, execution_id: &ExecutionId) -> anyhow::Result<u32>;

    /// Increment iteration counter
    async fn increment(&self, execution_id: &ExecutionId) -> anyhow::Result<u32>;

    /// Reset iteration counter
    async fn reset(&self, execution_id: &ExecutionId) -> anyhow::Result<()>;
}

/// Trait for state synchronization coordinator
#[async_trait]
pub trait StateSyncCoordinator: Send + Sync {
    /// Sync state before iteration
    async fn sync_before_iteration(
        &self,
        execution_id: &ExecutionId,
        iteration: u32,
    ) -> anyhow::Result<()>;

    /// Sync state after iteration
    async fn sync_after_iteration(
        &self,
        execution_id: &ExecutionId,
        iteration: u32,
        result: &IterationResult,
    ) -> anyhow::Result<()>;
}

/// Trait for stuck detection
#[async_trait]
pub trait StuckDetector: Send + Sync {
    /// Check if execution is stuck
    async fn is_stuck(
        &self,
        execution_id: &ExecutionId,
        iteration_history: &[IterationSummary],
    ) -> anyhow::Result<Option<String>>;

    /// Get stuck resolution strategy
    async fn get_resolution(
        &self,
        execution_id: &ExecutionId,
        reason: &str,
    ) -> anyhow::Result<StuckResolution>;
}

/// Trait for checkpoint storage
#[async_trait]
pub trait CheckpointStore: Send + Sync {
    /// Save checkpoint
    async fn save(
        &self,
        execution_id: &ExecutionId,
        iteration: u32,
        state: &IterationState,
    ) -> anyhow::Result<()>;

    /// Load latest checkpoint
    async fn load_latest(
        &self,
        execution_id: &ExecutionId,
    ) -> anyhow::Result<Option<IterationState>>;

    /// Delete all checkpoints for an execution
    async fn clear(&self, execution_id: &ExecutionId) -> anyhow::Result<()>;
}

/// Trait for running individual autopilot steps with real service calls.
///
/// This trait breaks the circular dependency between `GovernedLoopRuntime`
/// (which holds `Arc<dyn ExecutionService>`) and `InMemoryExecutionService`
/// (which needs step execution logic). A `GovernedAutopilotStepRunner`
/// implementation holds the real services (SecurityGate, ProgressEvaluator, etc.)
/// and is injected into `InMemoryExecutionService` as an optional dependency.
#[async_trait]
pub trait AutopilotStepRunner: Send + Sync {
    /// Execute a single autopilot step and return the result.
    async fn run_step(
        &self,
        execution_id: &ExecutionId,
        step: &AutopilotStep,
        iteration: u32,
        step_context: &serde_json::Value,
    ) -> anyhow::Result<StepResult>;
}

// ─── End Autopilot Traits ─────────────────────────────────────────────────────
