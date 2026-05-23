# Autopilot V2 API Reference

## Overview

This document provides a complete API reference for the CyberClaw Autopilot V2 system.

## Table of Contents

1. [Core Types](#core-types)
2. [ExecutionService API](#executionservice-api)
3. [IterationTracker API](#iterationtracker-api)
4. [StateSyncCoordinator API](#statesynccoordinator-api)
5. [SecurityGate API](#securitygate-api)
6. [Error Types](#error-types)

## Core Types

### AutopilotJob

Long-running automation job definition.

```rust
pub struct AutopilotJob {
    pub job_id: String,
    pub goal: String,
    pub max_iterations: u32,
    pub review_gates: Vec<ReviewGate>,
    pub created_at: DateTime<Utc>,
}

impl AutopilotJob {
    /// Create new job
    pub fn new(goal: String, max_iterations: u32) -> Self

    /// Add review gates
    pub fn with_review_gates(self, gates: Vec<ReviewGate>) -> Self

    /// Add security constraints
    pub fn with_security_constraints(self, constraints: SecurityConstraints) -> Self

    /// Validate job configuration
    pub fn validate(&self) -> Result<()>
}
```

### AutopilotRunState

Complete state of an Autopilot run.

```rust
pub struct AutopilotRunState {
    pub run_id: ExecutionId,
    pub job_id: String,
    pub current_iteration: u32,
    pub iterations: Vec<IterationState>,
    pub status: AutopilotStatus,
    pub stuck_count: u32,
    pub last_state_hash: Option<String>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AutopilotRunState {
    /// Create new run state
    pub fn new(run_id: ExecutionId, job_id: String) -> Self

    /// Start new iteration
    pub fn start_iteration(&mut self, step: AutopilotStep)

    /// Complete current iteration
    pub fn complete_iteration(&mut self) -> bool

    /// Add execution result
    pub fn add_execution_result(&mut self, result: ExecutionResult)

    /// Transition to new step
    pub fn transition_step(&mut self, new_step: AutopilotStep)

    /// Mark as stuck
    pub fn mark_stuck(&mut self, reason: String)

    /// Mark as awaiting review
    pub fn mark_awaiting_review(&mut self, gate: ReviewGate)

    /// Mark as completed
    pub fn mark_completed(&mut self)

    /// Mark as failed
    pub fn mark_failed(&mut self, error: String)

    /// Check if should stop due to stuck
    pub fn should_stop_due_to_stuck(&self, max_stuck: u32) -> bool
}
```

### AutopilotStatus

Run-level status enum.

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AutopilotStatus {
    /// Initialized, waiting to start
    Initialized,

    /// Running with current step
    Running { current_step: AutopilotStep },

    /// Stuck (no progress)
    Stuck { reason: String },

    /// Awaiting review
    AwaitingReview { gate: ReviewGate },

    /// Completed
    Completed { iterations: u32 },

    /// Failed
    Failed { error: String },
}
```

### AutopilotStep

9-step loop enumeration.

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AutopilotStep {
    Initialize,  // Step 1
    Plan,        // Step 2
    Execute,     // Step 3
    Review,      // Step 4
    Analyze,     // Step 5
    Decide,      // Step 6
    Update,      // Step 7
    Check,       // Step 8
    Iterate,     // Step 9
}

impl AutopilotStep {
    /// Get next step
    pub fn next(self) -> Self

    /// Check if critical step
    pub fn is_critical(self) -> bool
}
```

### IterationState

Single iteration state.

```rust
pub struct IterationState {
    pub iteration_id: u32,
    pub step: AutopilotStep,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub state_hash: String,
    pub progress_delta: Option<f64>,
    pub execution_results: Vec<ExecutionResult>,
}

impl IterationState {
    /// Create new iteration state
    pub fn new(iteration_id: u32, step: AutopilotStep) -> Self

    /// Compute state hash
    pub fn compute_state_hash(&self) -> String

    /// Mark iteration as completed
    pub fn mark_completed(&mut self)
}
```

## ExecutionService API

Main service for submitting and executing Autopilot jobs.

### submit_autopilot

Submit a new Autopilot job.

```rust
async fn submit_autopilot(&self, job: AutopilotJob) -> Result<ExecutionId>
```

**Parameters:**
- `job`: Autopilot job configuration

**Returns:**
- `ExecutionId`: Unique run identifier

**Errors:**
- `ValidationError`: Invalid job configuration
- `StorageError`: Failed to save initial state

**Example:**
```rust
let job = AutopilotJob::new("Analyze code".to_string(), 50);
let run_id = execution_service.submit_autopilot(job).await?;
```

### execute_autopilot_iteration

Execute a single iteration.

```rust
async fn execute_autopilot_iteration(
    &self,
    execution_id: &ExecutionId,
    iteration: u32,
) -> Result<IterationResult>
```

**Parameters:**
- `execution_id`: Run identifier
- `iteration`: Iteration number

**Returns:**
- `IterationResult`: Result of the iteration

**Example:**
```rust
let result = execution_service
    .execute_autopilot_iteration(&run_id, 1)
    .await?;

if result.should_continue {
    // Continue to next iteration
}
```

### get_run_state

Get current run state.

```rust
async fn get_run_state(&self, run_id: &ExecutionId) -> Result<Option<AutopilotRunState>>
```

**Parameters:**
- `run_id`: Run identifier

**Returns:**
- `Option<AutopilotRunState>`: Current state if exists

**Example:**
```rust
if let Some(state) = execution_service.get_run_state(&run_id).await? {
    println!("Current iteration: {}", state.current_iteration);
}
```

## IterationTracker API

Track iteration history and detect stuck states.

### current_iteration

Get current iteration count.

```rust
fn current_iteration(&self, run_id: &ExecutionId) -> Result<u32>
```

**Parameters:**
- `run_id`: Run identifier

**Returns:**
- `u32`: Current iteration number (0 if not started)

### increment

Increment iteration counter.

```rust
fn increment(&self, run_id: &ExecutionId) -> Result<u32>
```

**Returns:**
- `u32`: New iteration number

### get_history

Get complete iteration history.

```rust
fn get_history(&self, run_id: &ExecutionId) -> Result<Vec<IterationSummary>>
```

**Returns:**
- `Vec<IterationSummary>`: List of iteration summaries

### detect_stuck

Check if execution is stuck (no progress).

```rust
fn detect_stuck(&self, run_id: &ExecutionId) -> Result<bool>
```

**Returns:**
- `bool`: True if last N iterations have same state hash

**Example:**
```rust
if iteration_tracker.detect_stuck(&run_id)? {
    // Handle stuck state
    state.mark_stuck("No progress detected".to_string());
}
```

### record_iteration

Record iteration state.

```rust
fn record_iteration(&self, run_id: &ExecutionId, state: IterationState) -> Result<()>
```

**Parameters:**
- `run_id`: Run identifier
- `state`: Iteration state to record

### get_last_n_hashes

Get recent state hashes.

```rust
fn get_last_n_hashes(&self, run_id: &ExecutionId, n: usize) -> Result<Vec<String>>
```

**Returns:**
- `Vec<String>`: List of state hashes (oldest to newest)

### cleanup_run

Clean up run history.

```rust
fn cleanup_run(&self, run_id: &ExecutionId) -> Result<()>
```

## StateSyncCoordinator API

Synchronize state between execution and storage.

### sync_to_store

Save state to persistent storage.

```rust
async fn sync_to_store(
    &self,
    run_id: &ExecutionId,
    state: &AutopilotRunState,
) -> Result<()>
```

**Parameters:**
- `run_id`: Run identifier
- `state`: State to save

**Example:**
```rust
state_sync.sync_to_store(&run_id, &current_state).await?;
```

### sync_from_store

Load state from storage.

```rust
async fn sync_from_store(&self, run_id: &ExecutionId) -> Result<Option<AutopilotRunState>>
```

**Returns:**
- `Option<AutopilotRunState>`: Stored state if exists

### create_checkpoint

Create state checkpoint.

```rust
async fn create_checkpoint(&self, run_id: &ExecutionId) -> Result<String>
```

**Returns:**
- `String`: Checkpoint identifier

### restore_from_checkpoint

Restore from checkpoint.

```rust
async fn restore_from_checkpoint(
    &self,
    run_id: &ExecutionId,
    checkpoint_id: &str,
) -> Result<AutopilotRunState>
```

**Parameters:**
- `run_id`: Run identifier
- `checkpoint_id`: Checkpoint to restore

**Returns:**
- `AutopilotRunState`: Restored state

## SecurityGate API

Security validation and enforcement.

### is_capability_allowed

Check if capability is whitelisted.

```rust
fn is_capability_allowed(&self, capability: &CapabilityId) -> bool
```

**Parameters:**
- `capability`: Capability to check

**Returns:**
- `bool`: True if allowed

**Example:**
```rust
let capability = CapabilityId("fs:read".to_string());
if security_gate.is_capability_allowed(&capability) {
    // Execute capability
}
```

### detect_prompt_injection

Detect prompt injection attempts.

```rust
fn detect_prompt_injection(&self, input: &str) -> bool
```

**Parameters:**
- `input`: User input to check

**Returns:**
- `bool`: True if injection detected

### is_path_allowed

Validate file path against boundaries.

```rust
fn is_path_allowed(&self, path: &str) -> bool
```

**Parameters:**
- `path`: Path to validate

**Returns:**
- `bool`: True if within boundaries

### requires_review

Check if capability requires review.

```rust
fn requires_review(&self, capability: &CapabilityId) -> bool
```

**Returns:**
- `bool`: True if review required

### validate_execution

Comprehensive validation before execution.

```rust
async fn validate_execution(
    &self,
    state: &AutopilotRunState,
    capability: &CapabilityId,
) -> Result<ValidationResult>
```

**Returns:**
- `ValidationResult`: Validation outcome with details

## Error Types

### AutopilotError

Main error type for Autopilot operations.

```rust
#[derive(Debug, thiserror::Error)]
pub enum AutopilotError {
    #[error("Validation failed: {0}")]
    ValidationError(String),

    #[error("Storage error: {0}")]
    StorageError(#[from] StorageError),

    #[error("Execution failed: {0}")]
    ExecutionError(String),

    #[error("Security violation: {0}")]
    SecurityError(String),

    #[error("State sync failed: {0}")]
    SyncError(String),

    #[error("Review timeout")]
    ReviewTimeout,

    #[error("Max iterations exceeded")]
    MaxIterationsExceeded,

    #[error("Stuck detected: {0}")]
    StuckDetected(String),
}
```

### IterationResult

Result of single iteration execution.

```rust
pub struct IterationResult {
    pub iteration_id: u32,
    pub success: bool,
    pub progress_made: bool,
    pub should_continue: bool,
    pub next_step: Option<AutopilotStep>,
    pub output: Option<String>,
    pub error: Option<String>,
}
```

### ReviewRequest

Review gate request.

```rust
pub struct ReviewRequest {
    pub request_id: String,
    pub gate_id: String,
    pub iteration_id: u32,
    pub capability: CapabilityId,
    pub context: String,
    pub created_at: DateTime<Utc>,
    pub status: ReviewStatus,
    pub reviewer: Option<String>,
    pub decision: Option<ReviewDecision>,
    pub decided_at: Option<DateTime<Utc>>,
}
```

### ReviewStatus

Review request status.

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum ReviewStatus {
    Pending,
    InReview,
    Approved,
    Rejected,
    Timeout,
}
```

## Configuration Types

### GovernedLoopConfig

9-step loop configuration.

```rust
pub struct GovernedLoopConfig {
    pub max_iterations: u32,
    pub stuck_threshold: u32,
    pub iteration_timeout_secs: u64,
    pub review_timeout_secs: u64,
    pub state_sync_interval_ms: u64,
    pub checkpoint_interval: u32,
}

impl Default for GovernedLoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 50,
            stuck_threshold: 3,
            iteration_timeout_secs: 300,
            review_timeout_secs: 300,
            state_sync_interval_ms: 1000,
            checkpoint_interval: 5,
        }
    }
}
```

### SecurityConfig

Security configuration.

```rust
pub struct SecurityConfig {
    pub capability_whitelist: Vec<CapabilityId>,
    pub workspace_boundaries: Vec<String>,
    pub prompt_injection_patterns: Vec<String>,
    pub max_file_size_mb: u64,
    pub require_review_for_capabilities: Vec<CapabilityId>,
}
```

## Usage Examples

### Basic Execution

```rust
use cyberclaw_control_plane::autopilot::*;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize services
    let execution_service = create_execution_service().await?;
    let iteration_tracker = create_iteration_tracker();
    let state_sync = create_state_sync().await?;

    // Create and submit job
    let job = AutopilotJob::new("Test task".to_string(), 10);
    let run_id = execution_service.submit_autopilot(job).await?;

    // Execute iterations
    for i in 1..=10 {
        let result = execution_service
            .execute_autopilot_iteration(&run_id, i)
            .await?;

        if !result.should_continue {
            break;
        }

        // Check for stuck
        if iteration_tracker.detect_stuck(&run_id)? {
            println!("Stuck detected at iteration {}", i);
            break;
        }
    }

    // Get final state
    let state = execution_service.get_run_state(&run_id).await?;
    println!("Final status: {:?}", state.map(|s| s.status));

    Ok(())
}
```

### With Security and Review

```rust
use cyberclaw_control_plane::autopilot::*;

#[tokio::main]
async fn main() -> Result<()> {
    let execution_service = create_execution_service().await?;
    let security_gate = create_security_gate();

    // Configure security
    let job = AutopilotJob::new("Secure task".to_string(), 20)
        .with_review_gates(vec![ReviewGate::HighRisk])
        .with_security_constraints(SecurityConstraints {
            capability_whitelist: vec![
                CapabilityId("fs:read".to_string()),
            ],
            workspace_boundaries: vec!["/safe".to_string()],
            prompt_injection_protection: true,
            ..Default::default()
        });

    let run_id = execution_service.submit_autopilot(job).await?;

    // Execute with security checks
    loop {
        let state = execution_service.get_run_state(&run_id).await?
            .ok_or("State not found")?;

        match state.status {
            AutopilotStatus::AwaitingReview { gate } => {
                // Handle review
                handle_review_request(&run_id, &gate).await?;
            }
            AutopilotStatus::Running { current_step } => {
                // Validate next capability
                let capability = get_next_capability(&current_step);
                if !security_gate.is_capability_allowed(&capability) {
                    return Err(anyhow!("Capability blocked"));
                }

                // Execute iteration
                execution_service
                    .execute_autopilot_iteration(&run_id, state.current_iteration + 1)
                    .await?;
            }
            AutopilotStatus::Completed { .. } => break,
            AutopilotStatus::Failed { error } => {
                return Err(anyhow!("Execution failed: {}", error));
            }
            _ => {}
        }
    }

    Ok(())
}
```

## Performance Considerations

### Recommended Limits

| Parameter | Recommended | Maximum |
|-----------|-------------|---------|
| Max Iterations | 50-200 | 1000 |
| Iteration Timeout | 5 min | 30 min |
| State Size | < 1 MB | < 10 MB |
| History Length | 100 | 1000 |
| Concurrent Runs | 10 | 100 |

### Optimization Tips

1. **Use checkpoints** for long-running jobs
2. **Limit iteration history** to prevent memory growth
3. **Batch state updates** to reduce CAS conflicts
4. **Use appropriate timeouts** based on task complexity
5. **Monitor stuck detection** to avoid infinite loops

## Migration Guide

### From V1 to V2

Key changes:
- 9-step loop replaces 3-step cycle
- State sync is now automatic
- Security controls are mandatory
- Review gates are integrated

Migration steps:

```rust
// V1 code
let job = OldAutopilotJob {
    goal: "Task".to_string(),
    max_iterations: 10,
};

// V2 equivalent
let job = AutopilotJob::new("Task".to_string(), 10)
    .with_review_gates(vec![])
    .with_security_constraints(SecurityConstraints::default());
```

## Troubleshooting

### Common Issues

1. **State not found**
   - Ensure state store is initialized
   - Check run_id is correct
   - Verify state sync is working

2. **Stuck detection false positive**
   - Increase stuck_threshold
   - Ensure state_hash changes between iterations
   - Check progress_delta calculation

3. **Review timeout**
   - Increase review_timeout_secs
   - Configure auto_approve_on_timeout for low risk
   - Add backup reviewers

4. **CAS conflicts**
   - Implement retry logic
   - Reduce concurrent updates
   - Use batch updates

## Support

For issues or questions:
- GitHub: [cyberclaw/issues](https://github.com/cyberclawlabs/cyberclaw/issues)
- Email: api-support@cyberclaw.io
- Documentation: [docs.cyberclaw.io](https://docs.cyberclaw.io)