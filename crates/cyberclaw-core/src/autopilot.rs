use crate::capability::RiskLevel;
use crate::execution::{AgentRef, ExecutionBudget, ExecutionStatus};
use crate::ids::{
    AutopilotJobId, AutopilotRunId, CaseId, ExecutionId, ReviewId, SessionId, TraceId,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ─── Autopilot Job Types ─────────────────────────────────────────────────────

/// Trigger mechanism for Autopilot runs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AutopilotTrigger {
    /// Triggered by cron schedule (e.g., "0 */6 * * *")
    Cron { schedule: String },
    /// Triggered by heartbeat interval (seconds)
    Heartbeat { interval_seconds: u64 },
    /// Triggered manually by user or API
    Manual,
    /// Triggered by webhook or external event
    Event { event_type: String },
}

/// Goal specification for what Autopilot should achieve
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutopilotGoal {
    /// High-level description of the goal
    pub description: String,
    /// Success criteria (measurable if possible)
    pub success_criteria: Vec<String>,
    /// Maximum iterations allowed to achieve goal
    pub max_iterations: u32,
    /// Maximum execution time in milliseconds
    pub max_duration_ms: Option<u64>,
}

/// Stop condition that determines when Autopilot should halt
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StopCondition {
    /// Stop when goal is achieved
    OnGoalReached,
    /// Stop when no progress is made for N iterations
    OnNoProgress { threshold: u32 },
    /// Stop after N consecutive failures
    OnRepeatedFailure { threshold: u32 },
    /// Stop when budget is exceeded
    OnBudgetExceeded,
    /// Stop when review times out
    OnReviewTimeout { timeout_seconds: u64 },
    /// Stop when denied by policy
    OnPolicyDeny,
    /// Only stop manually
    ManualOnly,
}

/// Session mode for Autopilot execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionMode {
    /// Use main session (accumulate context)
    MainSession,
    /// Use isolated session (clean context per run)
    IsolatedSession,
}

/// Autopilot job specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutopilotSpec {
    /// Goal to achieve
    pub goal: AutopilotGoal,
    /// Trigger mechanism
    pub trigger: AutopilotTrigger,
    /// Stop conditions (evaluated in order)
    pub stop_conditions: Vec<StopCondition>,
    /// Session mode
    pub session_mode: SessionMode,
    /// Agent to use for execution
    pub agent: AgentRef,
    /// Execution budget constraints
    pub budget: ExecutionBudget,
    /// Maximum risk level allowed
    pub max_risk_level: RiskLevel,
    /// Whether to auto-approve low-risk actions
    pub auto_approve_low_risk: bool,
}

/// Autopilot job definition (persistent configuration)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutopilotJob {
    pub id: AutopilotJobId,
    pub name: String,
    pub description: Option<String>,
    pub case_id: Option<CaseId>,
    pub spec: AutopilotSpec,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ─── Autopilot Run Types ──────────────────────────────────────────────────────

/// Status of an Autopilot run
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutopilotRunStatus {
    /// Run is scheduled but not started
    Scheduled,
    /// Run is initializing
    Initializing,
    /// Run is actively executing iterations
    Running,
    /// Run is waiting for review approval
    WaitingReview,
    /// Run is paused (can be resumed)
    Paused,
    /// Run completed successfully
    Completed,
    /// Run failed with error
    Failed,
    /// Run was cancelled
    Cancelled,
    /// Run was aborted due to policy
    Aborted,
}

/// Kind of trigger that started the run
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AutopilotTriggerKind {
    Scheduled,
    Heartbeat,
    Manual,
    Event,
    Resumed, // Resumed from pause/review
}

/// Autopilot run instance (execution tracking)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutopilotRun {
    pub run_id: AutopilotRunId,
    pub job_id: AutopilotJobId,
    pub status: AutopilotRunStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub trigger_kind: AutopilotTriggerKind,
    pub root_execution_id: Option<ExecutionId>,
    pub session_id: Option<SessionId>,
    pub trace_id: TraceId,
    pub iteration_count: u32,
    pub failure_count: u32,
    pub review_wait_count: u32,
}

// ─── Loop Iteration Types ─────────────────────────────────────────────────────

/// Decision made by progress evaluator
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProgressDecision {
    /// Continue with next iteration
    Continue,
    /// Retry current operation
    Retry,
    /// Pause and wait for review
    PauseForReview,
    /// Escalate to higher authority
    Escalate,
    /// Goal is complete
    Complete,
    /// Abort execution
    Abort,
}

/// Single iteration of the Autopilot loop
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopIteration {
    pub run_id: AutopilotRunId,
    pub iteration: u32,
    pub execution_id: Option<ExecutionId>,
    pub review_id: Option<ReviewId>,
    pub plan_summary: String,
    pub actions_executed: Vec<String>,
    pub progress_decision: ProgressDecision,
    pub progress_score: f32,
    pub continue_reason: Option<String>,
    pub timestamp: DateTime<Utc>,
}

// ─── Runtime State Types ──────────────────────────────────────────────────────

/// State snapshot for an iteration (used for checkpoints)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IterationState {
    pub run_id: AutopilotRunId,
    pub iteration: u32,
    pub state_hash: String, // For stuck detection
    pub execution_results: Vec<ExecutionResult>,
    pub memory_context: Vec<String>,
    pub progress_metrics: ProgressMetrics,
    pub timestamp: DateTime<Utc>,
}

/// Result of a single execution within an iteration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub execution_id: ExecutionId,
    pub status: ExecutionStatus,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub artifacts: Vec<String>,
    /// Wall-clock duration of this execution in milliseconds.
    #[serde(default)]
    pub duration_ms: u64,
}

/// Progress metrics for evaluation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressMetrics {
    pub goal_completion_percentage: f32,
    pub actions_completed: u32,
    pub actions_failed: u32,
    pub time_elapsed_ms: u64,
    pub tokens_used: u32,
}

/// Analysis result from progress evaluator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressAnalysis {
    pub has_progress: bool,
    pub progress_delta: f32,
    pub decision: ProgressDecision,
    pub reasoning: String,
    pub suggested_actions: Vec<String>,
}

/// Decision from the Decide step
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Decision {
    /// Continue with next iteration
    Continue,
    /// Stuck - no progress detected
    Stuck,
    /// Await manual review
    AwaitReview,
}

/// Step in the 9-step Autopilot loop
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AutopilotStep {
    Plan,
    Execute,
    Review,
    Analyze,
    Decide,
    Update,
    Check,
    Iterate,
    Complete,
}

/// Result of executing a step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub step: AutopilotStep,
    pub success: bool,
    pub data: Option<serde_json::Value>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

/// Resolution for stuck situations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StuckResolution {
    Retry,
    ChangeStrategy,
    Escalate,
    Abort,
}

/// Summary of an iteration for history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IterationSummary {
    pub iteration: u32,
    pub timestamp: DateTime<Utc>,
    pub steps_completed: Vec<AutopilotStep>,
    pub decision: ProgressDecision,
    pub key_events: Vec<String>,
}
