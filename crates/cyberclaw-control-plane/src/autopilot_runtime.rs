use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use cyberclaw_core::autopilot::{
    AutopilotJob, AutopilotRun, AutopilotRunStatus, AutopilotStep, Decision, ExecutionResult,
    IterationState, ProgressAnalysis, StepResult, StuckResolution,
};
use cyberclaw_core::memory::MemoryContextProvider;
use cyberclaw_core::memory_context::{MemoryContextRequest, WorkingEntryKind, WorkingMemoryEntry};
use cyberclaw_core::prelude::*;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::auto_mode_gate::{AutoModeConfig, AutoModeGate, ExitReason, PermissionSnapshot};
use crate::circuit_breaker::CircuitBreaker;
use crate::execution_service::ExecutionService;
use crate::plan_mode_gate::{
    DefaultPlanModeGate, PlanModeGate, PlanPermissionSnapshot, StripReason,
};
use crate::provenance_tracker::ProvenanceTracker;
use crate::review_queue::ReviewQueue;
use crate::shared_state_store::SharedStateStore;
use crate::types::{ExecutionPlan, PlannedAction};
use cyberclaw_core::capability::CapabilityRef;
use std::sync::Mutex;

// Import the traits we'll define in separate files
use crate::autopilot_iteration::{AutopilotIterationState, AutopilotIterationTracker};
use crate::autopilot_progress::ProgressEvaluator;
use crate::autopilot_state_sync::AutopilotStateSyncCoordinator;
use crate::autopilot_types::{
    AutopilotPhase, PlanModeSnapshotData, V2ExecutionResult, V2IterationState, VerifyVerdict,
};

// AgenticLoop integration
use cyberclaw_agent_runtime::agentic_loop::{
    AgenticLoop, DefaultAgenticLoop, IterationBudget, IterationResult, LoopConfig, LoopSummary,
};
use cyberclaw_agent_runtime::loop_delegate::AutopilotDelegate;
use cyberclaw_core::gateway::OrchestratorGateway;
use cyberclaw_llm::client::LlmClient;

/// Security gate for evaluating execution risks
#[async_trait]
pub trait SecurityGate: Send + Sync {
    /// Check if execution results meet security requirements
    async fn check_execution_results(
        &self,
        results: &[ExecutionResult],
    ) -> anyhow::Result<SecurityCheckResult>;
}

#[derive(Debug, Clone)]
pub struct SecurityCheckResult {
    pub passed: bool,
    pub issues: Vec<String>,
    pub requires_review: bool,
}

// ---------------------------------------------------------------------------
// AutopilotLoopBridge — bridges Autopilot Execute step to AgenticLoop
// ---------------------------------------------------------------------------

/// Configuration derived from autopilot context for feeding into `LoopConfig`.
#[derive(Debug, Clone)]
pub struct AutopilotLoopConfig {
    /// System prompt built from the autopilot job goal and plan context.
    pub system_prompt: String,
    /// LLM model identifier.
    pub model: String,
    /// Maximum iterations for the execute step's agentic loop session.
    pub max_iterations: u32,
    /// Stuck detector threshold.
    pub stuck_threshold: u32,
}

impl Default for AutopilotLoopConfig {
    fn default() -> Self {
        Self {
            system_prompt: String::new(),
            model: "gpt-4".to_string(),
            max_iterations: 30,
            stuck_threshold: 3,
        }
    }
}

impl AutopilotLoopConfig {
    /// Convert to the agentic loop's native `LoopConfig`.
    pub fn to_loop_config(&self) -> LoopConfig {
        LoopConfig {
            system_prompt: self.system_prompt.clone(),
            model: self.model.clone(),
            budget: IterationBudget {
                max_iterations: self.max_iterations,
                ..Default::default()
            },
            stuck_threshold: self.stuck_threshold,
            // Autopilot uses gateway-scoped capability dispatch, not the
            // LLM-side tool palette. Callers that want LLM tool-calling
            // should populate this from `BuiltinToolRegistry`.
            tools: Vec::new(),
            // P1.1 — Autopilot 默认不开启 system prompt 缓存：每次
            // 推进时 system prompt 可能因 stuck/strategy 切换而变。
            // 上层若需开启可显式置 true。
            cache_system_prompt: false,
        }
    }
}

/// Bridges the AutopilotRuntime's Execute step to `AgenticLoop`.
///
/// When attached to the runtime, `step_execute()` delegates to a
/// `DefaultAgenticLoop` configured with `AutopilotDelegate` (auto-approve
/// all tool calls) instead of calling `ExecutionService` directly.
pub struct AutopilotLoopBridge {
    llm: Arc<dyn LlmClient>,
    gateway: Arc<dyn OrchestratorGateway>,
    config: AutopilotLoopConfig,
}

impl AutopilotLoopBridge {
    /// Create a new bridge with the given LLM client and orchestrator gateway.
    pub fn new(
        llm: Arc<dyn LlmClient>,
        gateway: Arc<dyn OrchestratorGateway>,
        config: AutopilotLoopConfig,
    ) -> Self {
        Self {
            llm,
            gateway,
            config,
        }
    }

    /// Build a system prompt from the execution plan context.
    fn build_system_prompt(plan: &ExecutionPlan) -> String {
        let action_descriptions: Vec<String> = plan
            .actions
            .iter()
            .enumerate()
            .map(|(i, a)| format!("  {}. [{}] {}", i + 1, a.capability.as_str(), a.reason))
            .collect();

        format!(
            "You are an autonomous agent executing a plan.\n\
             Execute the following actions in order:\n{}\n\
             Use the available tools to complete each action. \
             Report results when all actions are complete.",
            action_descriptions.join("\n")
        )
    }

    /// Run the agentic loop for a given execution plan and return a summary.
    ///
    /// The loop uses `AutopilotDelegate` (fully autonomous, auto-approve all
    /// tool calls). The budget is derived from the bridge's config.
    pub async fn execute_with_loop(&self, plan: &ExecutionPlan) -> anyhow::Result<LoopSummary> {
        let system_prompt = Self::build_system_prompt(plan);

        let mut loop_config = self.config.to_loop_config();
        loop_config.system_prompt = system_prompt;

        let mut agentic_loop = DefaultAgenticLoop::new(self.llm.clone(), self.gateway.clone());

        agentic_loop.init(loop_config).await?;

        // Feed the plan as the initial user message
        let user_msg = format!(
            "Execute the planned actions now. There are {} actions to complete.",
            plan.actions.len()
        );
        agentic_loop.add_user_message(user_msg);

        // Drive the loop to completion
        loop {
            let result = agentic_loop.next_iteration().await?;
            match result {
                IterationResult::Done(_) => break,
                IterationResult::BudgetExhausted(reason) => {
                    info!(
                        reason = %reason,
                        "AgenticLoop budget exhausted during autopilot execute step"
                    );
                    break;
                }
                IterationResult::Stuck(reason) => {
                    warn!(
                        "AgenticLoop stuck during autopilot execute step: {}",
                        reason
                    );
                    break;
                }
                IterationResult::ToolCalls(calls) => {
                    // AutopilotDelegate auto-approves; execute via gateway
                    let _delegate = AutopilotDelegate;
                    for call in &calls {
                        let cap_request = cyberclaw_core::gateway::CapabilityRequest {
                            execution_id: ExecutionId::new(),
                            requested_by: ActorRef {
                                id: ActorId::from_string("autopilot-loop".to_string())
                                    .unwrap_or_else(|_| ActorId::new()),
                                actor_type: ActorType::System,
                                tenant_id: None,
                                home_node_id: None,
                                display_name: "Autopilot AgenticLoop".to_string(),
                            },
                            capability_id: CapabilityId::from_string(normalize_capability_id(
                                &call.function.name,
                            ))?,
                            connector_id: ConnectorId::from_string("local".to_string())?,
                            input: serde_json::from_str(&call.function.arguments)
                                .unwrap_or(serde_json::json!({})),
                            reason: format!("Autopilot loop tool call: {}", call.function.name),
                        };
                        match agentic_loop.gateway().execute_capability(cap_request).await {
                            Ok(result) => {
                                agentic_loop.add_tool_result(
                                    call.id.clone(),
                                    serde_json::to_string(&result.output)
                                        .unwrap_or_else(|_| "{}".to_string()),
                                );
                            }
                            Err(e) => {
                                agentic_loop
                                    .add_tool_result(call.id.clone(), format!("Error: {}", e));
                            }
                        }
                    }
                }
                IterationResult::Continue | IterationResult::TextResponse(_) => {
                    // Continue looping
                }
            }
        }

        agentic_loop.finalize().await
    }
}

/// Handle to a running Autopilot execution
pub struct AutopilotHandle {
    run_id: AutopilotRunId,
    join_handle: JoinHandle<Result<()>>,
    cancel_token: CancellationToken,
}

impl AutopilotHandle {
    /// Cancel the Autopilot execution
    pub async fn cancel(self) -> Result<()> {
        tracing::info!("Cancelling Autopilot run: {}", self.run_id.as_str());

        // Signal cancellation
        self.cancel_token.cancel();

        // Wait for task to finish
        match self.join_handle.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(anyhow!("Run failed: {}", e)),
            Err(e) if e.is_cancelled() => Ok(()),
            Err(e) => Err(anyhow!("Task panicked: {}", e)),
        }
    }

    /// Wait for the Autopilot execution to complete
    pub async fn wait(self) -> Result<()> {
        self.join_handle.await?
    }

    /// Get the run ID
    pub fn run_id(&self) -> &AutopilotRunId {
        &self.run_id
    }
}

/// Main GovernedLoop runtime for Autopilot V2
pub struct GovernedLoopRuntime {
    execution_service: Arc<dyn ExecutionService>,
    state_store: Arc<dyn SharedStateStore>,
    state_sync: Arc<dyn AutopilotStateSyncCoordinator>,
    iteration_tracker: Arc<dyn AutopilotIterationTracker>,
    progress_evaluator: Arc<dyn ProgressEvaluator>,
    security_gate: Arc<dyn SecurityGate>,
    review_queue: Arc<dyn ReviewQueue>,
    provenance_tracker: Arc<dyn ProvenanceTracker>,
    /// 可选的记忆上下文提供者。若未提供，Autopilot 仍可正常运行（无记忆功能）。
    memory_provider: Option<Arc<MemoryContextProvider>>,
    /// Auto Mode Gate for permission scoping during autopilot.
    auto_mode_gate: Option<Arc<dyn AutoModeGate>>,
    /// Circuit breaker for consecutive failure detection.
    circuit_breaker: Arc<Mutex<CircuitBreaker>>,
    /// Tracks which strategy variant is active. Incremented on each ChangeStrategy resolution.
    strategy_variant: Arc<Mutex<u32>>,
    /// Optional AgenticLoop bridge. When present, `step_execute` delegates to
    /// the agentic loop instead of calling `ExecutionService` directly.
    loop_bridge: Option<Arc<AutopilotLoopBridge>>,
    // -- 6-phase state machine (Task #18 — additive overlay) ----------------
    /// Current 6-phase pointer (Expansion/Planning/Execution/Qa/Validation/Cleanup).
    /// Independent of the legacy 5-phase driver; exposed for future phase-aware
    /// dispatch lanes.
    current_phase: Arc<Mutex<crate::autopilot_phases::AutopilotPhase>>,
    /// Append-only audit trail of 6-phase transitions.
    phase_history: Arc<Mutex<Vec<crate::autopilot_phases::PhaseTransition>>>,
    /// Policy governing forward skips / rollbacks during `advance_phase`.
    phase_skip_policy: crate::autopilot_phases::PhaseSkipPolicy,
    /// Dispatcher invoked per phase. Defaults to `StubPhaseDispatcher`.
    phase_dispatcher: Arc<dyn crate::autopilot_phases::AutopilotPhaseDispatcher>,
}

fn normalize_capability_id(raw: &str) -> String {
    raw.replace(':', ".")
}

/// Stringify a [`StripReason`] for serialised plan-mode snapshots.
///
/// `StripReason` is defined in `plan_mode_gate` and does not derive
/// `Serialize`; we keep the gate module untouched (Sprint 10 L3 constraint)
/// and persist a stable text form instead.
fn strip_reason_to_str(reason: &StripReason) -> &'static str {
    match reason {
        StripReason::WriteDenied => "WriteDenied",
        StripReason::DeleteDenied => "DeleteDenied",
        StripReason::DeployDenied => "DeployDenied",
        StripReason::ShellDenied => "ShellDenied",
        StripReason::SpawnDenied => "SpawnDenied",
        StripReason::OtherDenied => "OtherDenied",
    }
}

/// Convert a runtime [`PlanPermissionSnapshot`] into its serialisable
/// projection [`PlanModeSnapshotData`].
///
/// `PlanPermissionSnapshot` owns `CapabilityRef` handles that live only for
/// the current process. For persistence we project the public string-based
/// views (original / stripped / kept) so a restart can still inspect the
/// plan-mode permission envelope even though the gate needs to be
/// re-entered on the fresh `CapabilityRef` list.
fn project_plan_snapshot(snapshot: &PlanPermissionSnapshot) -> PlanModeSnapshotData {
    PlanModeSnapshotData {
        original: snapshot.original.clone(),
        stripped: snapshot
            .stripped
            .iter()
            .map(|(cap_id, reason)| (cap_id.clone(), strip_reason_to_str(reason).to_string()))
            .collect(),
        kept: snapshot.kept.clone(),
    }
}

impl GovernedLoopRuntime {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        execution_service: Arc<dyn ExecutionService>,
        state_store: Arc<dyn SharedStateStore>,
        state_sync: Arc<dyn AutopilotStateSyncCoordinator>,
        iteration_tracker: Arc<dyn AutopilotIterationTracker>,
        progress_evaluator: Arc<dyn ProgressEvaluator>,
        security_gate: Arc<dyn SecurityGate>,
        review_queue: Arc<dyn ReviewQueue>,
        provenance_tracker: Arc<dyn ProvenanceTracker>,
    ) -> Self {
        Self {
            execution_service,
            state_store,
            state_sync,
            iteration_tracker,
            progress_evaluator,
            security_gate,
            review_queue,
            provenance_tracker,
            memory_provider: None,
            auto_mode_gate: None,
            circuit_breaker: Arc::new(Mutex::new(CircuitBreaker::default())),
            strategy_variant: Arc::new(Mutex::new(0)),
            loop_bridge: None,
            current_phase: Arc::new(Mutex::new(
                crate::autopilot_phases::AutopilotPhase::Expansion,
            )),
            phase_history: Arc::new(Mutex::new(Vec::new())),
            phase_skip_policy: crate::autopilot_phases::PhaseSkipPolicy::Strict,
            phase_dispatcher: Arc::new(crate::autopilot_phases::StubPhaseDispatcher),
        }
    }

    /// 附加记忆上下文提供者（builder 模式，向下兼容）
    pub fn with_memory_provider(mut self, provider: Arc<MemoryContextProvider>) -> Self {
        self.memory_provider = Some(provider);
        self
    }

    /// Attach an Auto Mode Gate for permission scoping (builder pattern).
    pub fn with_auto_mode_gate(mut self, gate: Arc<dyn AutoModeGate>) -> Self {
        self.auto_mode_gate = Some(gate);
        self
    }

    /// Configure the circuit breaker threshold and cooldown.
    pub fn with_circuit_breaker(mut self, threshold: u32, cooldown: Duration) -> Self {
        self.circuit_breaker = Arc::new(Mutex::new(CircuitBreaker::new(threshold, cooldown)));
        self
    }

    /// Attach an AgenticLoop bridge for the Execute step (builder pattern).
    ///
    /// When set, `step_execute` delegates to the agentic loop instead of
    /// calling `ExecutionService` directly. If not set, the original
    /// `ExecutionService`-based path is used (backward compatible).
    pub fn with_loop_bridge(mut self, bridge: AutopilotLoopBridge) -> Self {
        self.loop_bridge = Some(Arc::new(bridge));
        self
    }

    /// Start an Autopilot run
    /// Start an Autopilot run
    pub async fn start_run(&self, job: AutopilotJob) -> anyhow::Result<AutopilotHandle> {
        let execution_id = ExecutionId::new();
        let run_id = AutopilotRunId::new();
        let trace_id = TraceId::new();

        info!(
            "Starting Autopilot run: job_id={}, run_id={}, execution_id={}",
            job.id.as_str(),
            run_id.as_str(),
            execution_id.as_str()
        );

        // Initialize run state
        let _run = AutopilotRun {
            run_id: run_id.clone(),
            job_id: job.id.clone(),
            status: AutopilotRunStatus::Initializing,
            started_at: Some(chrono::Utc::now()),
            finished_at: None,
            trigger_kind: cyberclaw_core::autopilot::AutopilotTriggerKind::Manual,
            root_execution_id: Some(execution_id.clone()),
            session_id: None, // Will be set based on session mode
            trace_id: trace_id.clone(),
            iteration_count: 0,
            failure_count: 0,
            review_wait_count: 0,
        };

        // Store initial state as V2IterationState
        let initial_state = crate::autopilot_types::V2IterationState {
            iteration_id: 0,
            step: crate::autopilot_types::AutopilotStep::Initialize,
            start_time: chrono::Utc::now(),
            end_time: None,
            state_hash: String::new(),
            progress_delta: None,
            execution_results: Vec::new(),
            current_phase: crate::autopilot_types::AutopilotPhase::Plan,
            fix_loop_count: 0,
            plan_mode_snapshot: None,
        };

        self.state_sync
            .sync_to_store(&execution_id, &initial_state)
            .await
            .context("Failed to sync initial run state")?;

        // Start provenance tracking
        self.provenance_tracker
            .start_tracking(
                execution_id.clone(),
                job.spec.agent.id.clone(),
                job.case_id.clone(),
                trace_id,
                None,
            )
            .await
            .context("Failed to start provenance tracking")?;

        // Create cancellation token
        let cancel_token = CancellationToken::new();
        let cancel_token_clone = cancel_token.clone();

        // Start the main loop
        let join_handle = tokio::spawn({
            let runtime = self.clone_runtime();
            let job = job.clone();
            let run_id = run_id.clone();
            async move {
                tokio::select! {
                    result = runtime.execute_loop(&run_id, &job) => {
                        result
                    }
                    _ = cancel_token_clone.cancelled() => {
                        tracing::info!("Autopilot run {} cancelled", run_id.as_str());
                        Ok(())
                    }
                }
            }
        });

        Ok(AutopilotHandle {
            run_id: run_id.clone(),
            join_handle,
            cancel_token,
        })
    }

    /// Resume a paused Autopilot run
    pub async fn resume_run(
        &self,
        run_id: &AutopilotRunId,
        execution_id: &ExecutionId,
    ) -> anyhow::Result<()> {
        // Load run state from store
        let run_state = self
            .state_sync
            .sync_from_store(execution_id)
            .await
            .context("Failed to load run state")?
            .ok_or_else(|| anyhow!("Run not found: {}", run_id.as_str()))?;

        // Load job configuration - convert String to AutopilotJobId
        let job_id = AutopilotJobId::from_string(run_state.job_id.clone())?;
        let job = self
            .load_job(&job_id)
            .await
            .context("Failed to load job configuration")?;

        // Resume the loop
        self.execute_loop(run_id, &job).await
    }

    /// Main 9-step execution loop
    async fn execute_loop(
        &self,
        run_id: &AutopilotRunId,
        job: &AutopilotJob,
    ) -> anyhow::Result<()> {
        let start_time = Instant::now();
        let max_iterations = job.spec.goal.max_iterations;

        // Convert run_id to ExecutionId for iteration tracking
        let execution_id = ExecutionId::from_string(run_id.as_str().to_string())?;

        // Enter auto mode if gate is configured
        let auto_snapshot: Option<PermissionSnapshot> = if let Some(gate) = &self.auto_mode_gate {
            let config = AutoModeConfig::default();
            match gate.enter_auto_mode(&execution_id, &config).await {
                Ok(snapshot) => Some(snapshot),
                Err(e) => {
                    warn!(
                        "Failed to enter auto mode: {}, continuing without auto mode gate",
                        e
                    );
                    None
                }
            }
        } else {
            None
        };

        loop {
            let iteration = self
                .iteration_tracker
                .current_iteration(&execution_id)
                .await
                .context("Failed to get current iteration")?;

            info!(
                "Starting iteration {} for run_id={} (max={})",
                iteration,
                run_id.as_str(),
                max_iterations
            );

            // Check circuit breaker
            let breaker_decision = {
                let mut cb = self
                    .circuit_breaker
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                cb.check()
            };
            if let crate::circuit_breaker::BreakerDecision::Deny { reason } = breaker_decision {
                warn!(
                    "Circuit breaker tripped for run_id={}: {}",
                    run_id.as_str(),
                    reason
                );
                if let (Some(gate), Some(snapshot)) = (&self.auto_mode_gate, &auto_snapshot) {
                    let failures = self
                        .circuit_breaker
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .consecutive_failures();
                    let _ = gate
                        .exit_auto_mode(
                            &execution_id,
                            snapshot,
                            ExitReason::CircuitBreak {
                                consecutive_failures: failures,
                            },
                        )
                        .await;
                }
                self.finalize_run(run_id, AutopilotRunStatus::Aborted)
                    .await?;
                return Ok(());
            }

            // Check iteration limit
            if iteration >= max_iterations {
                warn!(
                    "Reached max iterations ({}) for run_id={}",
                    max_iterations,
                    run_id.as_str()
                );
                self.finalize_run(run_id, AutopilotRunStatus::Completed)
                    .await?;
                break;
            }

            // Check time limit
            if let Some(max_duration_ms) = job.spec.goal.max_duration_ms {
                if start_time.elapsed() > Duration::from_millis(max_duration_ms) {
                    warn!(
                        "Exceeded time limit ({}ms) for run_id={}",
                        max_duration_ms,
                        run_id.as_str()
                    );
                    self.finalize_run(run_id, AutopilotRunStatus::Completed)
                        .await?;
                    break;
                }
            }

            // Execute the 9 steps
            let iteration_start = Instant::now();

            // Memory: 迭代开始时获取记忆上下文
            if let Some(mem) = &self.memory_provider {
                let ctx_request = MemoryContextRequest {
                    case_id: None,
                    execution_id: Some(execution_id.clone()),
                    max_items: 20,
                };
                let _ctx = mem.get_context(ctx_request);
                info!(
                    "Memory context loaded for run_id={}, iteration={}",
                    run_id.as_str(),
                    iteration
                );
            }

            // Step 1: Plan
            let plan = match self.step_plan(run_id, iteration, job).await {
                Ok(p) => p,
                Err(e) => {
                    error!("Plan step failed: {}", e);
                    self.handle_step_failure(run_id, AutopilotStep::Plan, e)
                        .await?;
                    continue;
                }
            };

            // Step 2: Execute
            let results = match self.step_execute(run_id, &plan).await {
                Ok(r) => r,
                Err(e) => {
                    error!("Execute step failed: {}", e);
                    self.handle_step_failure(run_id, AutopilotStep::Execute, e)
                        .await?;
                    continue;
                }
            };

            // Memory: 执行完成后存储执行记录到情景记忆
            if let Some(mem) = &self.memory_provider {
                use cyberclaw_core::execution::{AgentRef, ExecutionBudget, ExecutionStatus};
                let memory_execution = cyberclaw_core::execution::Execution {
                    id: execution_id.clone(),
                    root_execution_id: execution_id.clone(),
                    parent_execution_id: None,
                    owner_node_id: None,
                    scheduled_node_id: None,
                    placement_group: None,
                    lease_id: None,
                    handoff_count: 0,
                    case_id: None,
                    task_id: None,
                    agent: AgentRef {
                        id: job.spec.agent.id.clone(),
                        role: "autopilot".to_string(),
                    },
                    status: ExecutionStatus::Completed,
                    join_strategy: None,
                    budget: ExecutionBudget::default(),
                    workspace: None,
                    trace_id: TraceId::new(),
                    started_at: Some(chrono::Utc::now()),
                    finished_at: Some(chrono::Utc::now()),
                    risk_level: cyberclaw_core::capability::RiskLevel::Low,
                    execution_mode: cyberclaw_core::execution::ExecutionMode::Autopilot,
                };
                mem.add_execution(memory_execution);
            }

            // Step 3: Review
            let review = match self.step_review(run_id, &results).await {
                Ok(r) => r,
                Err(e) => {
                    error!("Review step failed: {}", e);
                    self.handle_step_failure(run_id, AutopilotStep::Review, e)
                        .await?;
                    continue;
                }
            };

            // If review requires manual approval, pause
            if review.requires_review {
                info!("Pausing for manual review: run_id={}", run_id.as_str());
                self.update_run_status(run_id, AutopilotRunStatus::WaitingReview)
                    .await?;
                return Ok(()); // Exit loop, will be resumed later
            }

            // Step 4: Analyze
            let analysis = match self.step_analyze(run_id, &review).await {
                Ok(a) => a,
                Err(e) => {
                    error!("Analyze step failed: {}", e);
                    self.handle_step_failure(run_id, AutopilotStep::Analyze, e)
                        .await?;
                    continue;
                }
            };

            // Step 5: Decide
            let decision = match self.step_decide(run_id, &analysis).await {
                Ok(d) => d,
                Err(e) => {
                    error!("Decide step failed: {}", e);
                    self.handle_step_failure(run_id, AutopilotStep::Decide, e)
                        .await?;
                    continue;
                }
            };

            match decision {
                Decision::Continue => {
                    // Step 6: Update
                    if let Err(e) = self.step_update(run_id, &results).await {
                        error!("Update step failed: {}", e);
                        self.handle_step_failure(run_id, AutopilotStep::Update, e)
                            .await?;
                    }

                    // Step 7: Check
                    let goal_met = match self.step_check(run_id, job).await {
                        Ok(met) => met,
                        Err(e) => {
                            error!("Check step failed: {}", e);
                            self.handle_step_failure(run_id, AutopilotStep::Check, e)
                                .await?;
                            false
                        }
                    };

                    if goal_met {
                        info!("Goal achieved for run_id={}", run_id.as_str());
                        self.finalize_run(run_id, AutopilotRunStatus::Completed)
                            .await?;
                        break;
                    }

                    // Step 8: Iterate
                    if let Err(e) = self.step_iterate(run_id).await {
                        error!("Iterate step failed: {}", e);
                        self.handle_step_failure(run_id, AutopilotStep::Iterate, e)
                            .await?;
                    }

                    // Record success in circuit breaker
                    {
                        let mut cb = self
                            .circuit_breaker
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        cb.record_success();
                    }

                    let iteration_duration = iteration_start.elapsed();
                    info!(
                        "Completed iteration {} in {:?} for run_id={}",
                        iteration,
                        iteration_duration,
                        run_id.as_str()
                    );

                    // Memory: 迭代结束时存储工作记忆条目
                    if let Some(mem) = &self.memory_provider {
                        mem.add_working_entry(WorkingMemoryEntry {
                            execution_id: Some(execution_id.clone()),
                            kind: WorkingEntryKind::PhaseEnd,
                            summary: format!(
                                "Autopilot iteration {} completed in {:?} for run {}",
                                iteration,
                                iteration_duration,
                                run_id.as_str()
                            ),
                            artifact_refs: vec![],
                            trace_id: None,
                            encrypted: false,
                        });
                    }
                }
                Decision::Stuck => {
                    warn!("No progress detected for run_id={}", run_id.as_str());

                    // Try to resolve stuck situation
                    let resolution = self.resolve_stuck(run_id, &analysis).await?;
                    match resolution {
                        StuckResolution::Retry => continue,
                        StuckResolution::ChangeStrategy => {
                            let mut variant = self
                                .strategy_variant
                                .lock()
                                .unwrap_or_else(|e| e.into_inner());
                            *variant += 1;
                            info!(
                                "Changing strategy to variant {} for run_id={}",
                                *variant,
                                run_id.as_str()
                            );
                            drop(variant); // release lock before any await
                            continue;
                        }
                        StuckResolution::Escalate => {
                            self.update_run_status(run_id, AutopilotRunStatus::WaitingReview)
                                .await?;
                            return Ok(());
                        }
                        StuckResolution::Abort => {
                            self.finalize_run(run_id, AutopilotRunStatus::Aborted)
                                .await?;
                            break;
                        }
                    }
                }
                Decision::AwaitReview => {
                    info!("Awaiting review for run_id={}", run_id.as_str());
                    self.update_run_status(run_id, AutopilotRunStatus::WaitingReview)
                        .await?;
                    return Ok(());
                }
            }
        }

        // Exit auto mode if we entered it
        if let (Some(gate), Some(snapshot)) = (&self.auto_mode_gate, &auto_snapshot) {
            let _ = gate
                .exit_auto_mode(&execution_id, snapshot, ExitReason::GoalMet)
                .await;
        }

        Ok(())
    }

    // ─── Step 1: Plan ─────────────────────────────────────────────────────────

    async fn step_plan(
        &self,
        run_id: &AutopilotRunId,
        iteration: u32,
        job: &AutopilotJob,
    ) -> anyhow::Result<ExecutionPlan> {
        let start = Instant::now();
        info!(
            "Step 1 - Plan: run_id={}, iteration={}",
            run_id.as_str(),
            iteration
        );

        // Load iteration state from sync coordinator
        let execution_id_for_state = ExecutionId::from_string(run_id.as_str().to_string())?;
        let state: Option<IterationState> = match self
            .state_sync
            .sync_from_store(&execution_id_for_state)
            .await
        {
            Ok(Some(run_state)) => Some(IterationState {
                run_id: run_id.clone(),
                iteration: run_state.current_iteration,
                state_hash: run_state.last_state_hash.unwrap_or_default(),
                execution_results: Vec::new(),
                memory_context: Vec::new(),
                progress_metrics: cyberclaw_core::autopilot::ProgressMetrics {
                    goal_completion_percentage: 0.0,
                    actions_completed: 0,
                    actions_failed: 0,
                    time_elapsed_ms: 0,
                    tokens_used: 0,
                },
                timestamp: run_state.updated_at,
            }),
            Ok(None) => {
                info!(
                    "No prior state found for run_id={}, starting fresh",
                    run_id.as_str()
                );
                None
            }
            Err(e) => {
                warn!(
                    "Failed to load state for run_id={}: {}, proceeding without state",
                    run_id.as_str(),
                    e
                );
                None
            }
        };

        // Create execution request based on goal and current state
        let plan = self.build_execution_request(job, &state)?;

        let duration = start.elapsed();
        self.record_step_result(
            run_id,
            StepResult {
                step: AutopilotStep::Plan,
                success: true,
                data: Some(serde_json::json!({
                    "actions_count": plan.actions.len(),
                    "review_required": plan.review_required,
                })),
                error: None,
                duration_ms: duration.as_millis().min(u64::MAX as u128) as u64,
            },
        )
        .await?;

        Ok(plan)
    }

    // ─── Step 2: Execute ──────────────────────────────────────────────────────

    async fn step_execute(
        &self,
        run_id: &AutopilotRunId,
        plan: &ExecutionPlan,
    ) -> anyhow::Result<Vec<ExecutionResult>> {
        let start = Instant::now();
        info!(
            "Step 2 - Execute: run_id={}, actions={}",
            run_id.as_str(),
            plan.actions.len()
        );

        // If an AgenticLoop bridge is configured, delegate to it.
        // Otherwise fall back to the original ExecutionService path.
        let results = if let Some(bridge) = &self.loop_bridge {
            info!(
                "Delegating execute step to AgenticLoop for run_id={}",
                run_id.as_str()
            );
            match bridge.execute_with_loop(plan).await {
                Ok(summary) => {
                    info!(
                        "AgenticLoop completed: iterations={}, tokens={}",
                        summary.iterations, summary.tokens_used
                    );
                    // Convert LoopSummary to Vec<ExecutionResult>
                    let execution_id = ExecutionId::new();
                    vec![ExecutionResult {
                        execution_id,
                        status: ExecutionStatus::Completed,
                        output: Some(serde_json::json!({
                            "actions_executed": plan.actions.len(),
                            "plan_review_required": plan.review_required,
                            "loop_iterations": summary.iterations,
                            "loop_tokens_used": summary.tokens_used,
                            "loop_output": summary.final_output,
                        })),
                        error: None,
                        artifacts: Vec::new(),
                        duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
                    }]
                }
                Err(e) => {
                    warn!(
                        "AgenticLoop failed for run_id={}: {}, falling back to ExecutionService",
                        run_id.as_str(),
                        e
                    );
                    self.step_execute_via_service(plan, start).await?
                }
            }
        } else {
            self.step_execute_via_service(plan, start).await?
        };

        let duration = start.elapsed();
        self.record_step_result(
            run_id,
            StepResult {
                step: AutopilotStep::Execute,
                success: true,
                data: Some(serde_json::json!({
                    "executed": results.len(),
                    "successful": results.iter().filter(|r| r.status == ExecutionStatus::Completed).count(),
                    "failed": results.iter().filter(|r| r.status == ExecutionStatus::Failed).count(),
                })),
                error: None,
                duration_ms: duration.as_millis().min(u64::MAX as u128) as u64,
            },
        )
        .await?;

        Ok(results)
    }

    /// Original ExecutionService-based execute path (extracted for reuse as fallback).
    async fn step_execute_via_service(
        &self,
        plan: &ExecutionPlan,
        start: Instant,
    ) -> anyhow::Result<Vec<ExecutionResult>> {
        // Submit the plan for execution
        let execution_id = self.execution_service.submit_plan(plan.clone()).await?;

        // Execute the submitted plan
        self.execution_service.execute(&execution_id).await?;

        // Get execution results
        let execution = self
            .execution_service
            .get(&execution_id)
            .await?
            .ok_or_else(|| anyhow!("Execution not found after completion"))?;

        // Convert execution results to Vec<ExecutionResult>
        let mut results = Vec::new();

        // Create a result for the overall execution
        results.push(ExecutionResult {
            execution_id: execution_id.clone(),
            status: execution.status,
            output: Some(serde_json::json!({
                "actions_executed": plan.actions.len(),
                "plan_review_required": plan.review_required,
            })),
            error: None,
            artifacts: Vec::new(),
            duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
        });

        Ok(results)
    }

    // ─── Step 3: Review ───────────────────────────────────────────────────────

    async fn step_review(
        &self,
        run_id: &AutopilotRunId,
        results: &[ExecutionResult],
    ) -> anyhow::Result<SecurityCheckResult> {
        let start = Instant::now();
        info!(
            "Step 3 - Review: run_id={}, results={}",
            run_id.as_str(),
            results.len()
        );

        // Security gate check
        let review = self
            .security_gate
            .check_execution_results(results)
            .await
            .context("Security review failed")?;

        if !review.passed {
            warn!(
                "Security review failed for run_id={}: {:?}",
                run_id.as_str(),
                review.issues
            );
        }

        if review.requires_review {
            // Submit to review queue
            let review_id = ReviewId::new();
            let review_request = ReviewRequest::for_execution(
                review_id,
                ExecutionId::from_string(run_id.as_str().to_string())?,
                None,
                format!("Security Review for Autopilot Run {}", run_id.as_str()),
                format!("Security issues detected: {:?}", review.issues),
                ActorRef {
                    id: ActorId::from_string("autopilot-system".to_string())?,
                    actor_type: ActorType::System,
                    tenant_id: None,
                    home_node_id: None,
                    display_name: "Autopilot System".to_string(),
                },
                ReviewKind::HumanReview,
                TraceId::new(),
                chrono::Utc::now(),
            );
            self.review_queue.enqueue(review_request).await?;
        }

        let duration = start.elapsed();
        self.record_step_result(
            run_id,
            StepResult {
                step: AutopilotStep::Review,
                success: review.passed,
                data: Some(serde_json::json!({
                    "passed": review.passed,
                    "issues": review.issues,
                    "requires_review": review.requires_review,
                })),
                error: None,
                duration_ms: duration.as_millis().min(u64::MAX as u128) as u64,
            },
        )
        .await?;

        Ok(review)
    }

    // ─── Step 4: Analyze ──────────────────────────────────────────────────────

    async fn step_analyze(
        &self,
        run_id: &AutopilotRunId,
        _review: &SecurityCheckResult,
    ) -> anyhow::Result<ProgressAnalysis> {
        let start = Instant::now();
        info!("Step 4 - Analyze: run_id={}", run_id.as_str());

        // Load state from sync coordinator for analysis context
        let execution_id = ExecutionId::from_string(run_id.as_str().to_string())?;

        let _iteration = self
            .iteration_tracker
            .current_iteration(&execution_id)
            .await?;
        let _state: Option<AutopilotIterationState> = None;

        // Analyze progress using empty results since we don't have execution results in AutopilotIterationState
        let empty_results: Vec<ExecutionResult> = Vec::new();
        let analysis = self
            .progress_evaluator
            .analyze(run_id, &empty_results)
            .await
            .context("Progress analysis failed")?;

        let duration = start.elapsed();
        self.record_step_result(
            run_id,
            StepResult {
                step: AutopilotStep::Analyze,
                success: true,
                data: Some(serde_json::json!({
                    "has_progress": analysis.has_progress,
                    "progress_delta": analysis.progress_delta,
                    "decision": format!("{:?}", analysis.decision),
                })),
                error: None,
                duration_ms: duration.as_millis().min(u64::MAX as u128) as u64,
            },
        )
        .await?;

        Ok(analysis)
    }

    // ─── Step 5: Decide ───────────────────────────────────────────────────────

    async fn step_decide(
        &self,
        run_id: &AutopilotRunId,
        analysis: &ProgressAnalysis,
    ) -> anyhow::Result<Decision> {
        let start = Instant::now();
        info!("Step 5 - Decide: run_id={}", run_id.as_str());

        // Check if stuck
        // Convert run_id to ExecutionId for iteration tracking
        let execution_id = ExecutionId::from_string(run_id.as_str().to_string())?;

        let is_stuck = self.iteration_tracker.detect_stuck(&execution_id).await?;

        let decision = if is_stuck {
            Decision::Stuck
        } else if analysis.has_progress {
            Decision::Continue
        } else if analysis.decision == cyberclaw_core::autopilot::ProgressDecision::PauseForReview {
            Decision::AwaitReview
        } else {
            Decision::Continue // Default to continue with adjusted strategy
        };

        let duration = start.elapsed();
        self.record_step_result(
            run_id,
            StepResult {
                step: AutopilotStep::Decide,
                success: true,
                data: Some(serde_json::json!({
                    "decision": format!("{:?}", decision),
                    "is_stuck": is_stuck,
                })),
                error: None,
                duration_ms: duration.as_millis().min(u64::MAX as u128) as u64,
            },
        )
        .await?;

        Ok(decision)
    }

    // ─── Step 6: Update ───────────────────────────────────────────────────────

    async fn step_update(
        &self,
        run_id: &AutopilotRunId,
        results: &[ExecutionResult],
    ) -> anyhow::Result<()> {
        let start = Instant::now();
        info!("Step 6 - Update: run_id={}", run_id.as_str());

        // Get current iteration
        // Convert run_id to ExecutionId for iteration tracking
        let execution_id = ExecutionId::from_string(run_id.as_str().to_string())?;

        let iteration = self
            .iteration_tracker
            .current_iteration(&execution_id)
            .await?;

        // Convert ExecutionResult to V2ExecutionResult
        let v2_results: Vec<V2ExecutionResult> = results
            .iter()
            .map(|r| V2ExecutionResult {
                execution_id: r.execution_id.clone(),
                // ExecutionResult carries no separate capability_id; use the execution_id
                // as a stable identifier until the execution layer surfaces capability metadata.
                capability_id: r.execution_id.as_str().to_string(),
                status: r.status.clone(),
                output: r.output.clone(),
                error: r.error.clone(),
                duration_ms: r.duration_ms,
                trace_id: TraceId::new().as_str().to_string(),
            })
            .collect();

        // Create V2IterationState for sync
        let v2_state = crate::autopilot_types::V2IterationState {
            iteration_id: iteration,
            step: crate::autopilot_types::AutopilotStep::Update,
            start_time: chrono::Utc::now(),
            end_time: None,
            state_hash: self.compute_state_hash(results),
            progress_delta: None,
            execution_results: v2_results,
            current_phase: crate::autopilot_types::AutopilotPhase::Plan,
            fix_loop_count: 0,
            plan_mode_snapshot: None,
        };

        // Sync to store - convert run_id to ExecutionId
        let exec_id = ExecutionId::from_string(run_id.as_str().to_string())?;
        self.state_sync
            .sync_to_store(&exec_id, &v2_state)
            .await
            .context("Failed to sync state to store")?;

        let duration = start.elapsed();
        self.record_step_result(
            run_id,
            StepResult {
                step: AutopilotStep::Update,
                success: true,
                data: Some(serde_json::json!({
                    "iteration": iteration,
                    "state_hash": v2_state.state_hash,
                })),
                error: None,
                duration_ms: duration.as_millis().min(u64::MAX as u128) as u64,
            },
        )
        .await?;

        Ok(())
    }

    // ─── Step 7: Check ────────────────────────────────────────────────────────

    async fn step_check(
        &self,
        run_id: &AutopilotRunId,
        job: &AutopilotJob,
    ) -> anyhow::Result<bool> {
        let start = Instant::now();
        info!("Step 7 - Check: run_id={}", run_id.as_str());

        // Check if goal is met
        let goal_met = self
            .progress_evaluator
            .is_goal_met(run_id, job)
            .await
            .context("Goal check failed")?;

        let duration = start.elapsed();
        self.record_step_result(
            run_id,
            StepResult {
                step: AutopilotStep::Check,
                success: true,
                data: Some(serde_json::json!({
                    "goal_met": goal_met,
                })),
                error: None,
                duration_ms: duration.as_millis().min(u64::MAX as u128) as u64,
            },
        )
        .await?;

        Ok(goal_met)
    }

    // ─── Step 8: Iterate ──────────────────────────────────────────────────────

    async fn step_iterate(&self, run_id: &AutopilotRunId) -> anyhow::Result<()> {
        let start = Instant::now();
        info!("Step 8 - Iterate: run_id={}", run_id.as_str());

        // Convert run_id to ExecutionId for iteration tracking
        let execution_id = ExecutionId::from_string(run_id.as_str().to_string())?;

        // Increment iteration counter
        self.iteration_tracker
            .increment(&execution_id)
            .await
            .context("Failed to increment iteration")?;

        let new_iteration = self
            .iteration_tracker
            .current_iteration(&execution_id)
            .await?;

        let duration = start.elapsed();
        self.record_step_result(
            run_id,
            StepResult {
                step: AutopilotStep::Iterate,
                success: true,
                data: Some(serde_json::json!({
                    "new_iteration": new_iteration,
                })),
                error: None,
                duration_ms: duration.as_millis().min(u64::MAX as u128) as u64,
            },
        )
        .await?;

        Ok(())
    }

    // ─── Helper Methods ───────────────────────────────────────────────────────

    fn clone_runtime(&self) -> Self {
        Self {
            execution_service: self.execution_service.clone(),
            state_store: self.state_store.clone(),
            state_sync: self.state_sync.clone(),
            iteration_tracker: self.iteration_tracker.clone(),
            progress_evaluator: self.progress_evaluator.clone(),
            security_gate: self.security_gate.clone(),
            review_queue: self.review_queue.clone(),
            provenance_tracker: self.provenance_tracker.clone(),
            memory_provider: self.memory_provider.clone(),
            auto_mode_gate: self.auto_mode_gate.clone(),
            circuit_breaker: self.circuit_breaker.clone(),
            strategy_variant: self.strategy_variant.clone(),
            loop_bridge: self.loop_bridge.clone(),
            current_phase: self.current_phase.clone(),
            phase_history: self.phase_history.clone(),
            phase_skip_policy: self.phase_skip_policy,
            phase_dispatcher: self.phase_dispatcher.clone(),
        }
    }

    async fn load_job(&self, job_id: &AutopilotJobId) -> anyhow::Result<AutopilotJob> {
        // 1. Query SharedStateStore for job configuration
        let key = format!("autopilot:job:{}", job_id.as_str());
        let entry = self
            .state_store
            .get(&key)
            .await?
            .ok_or_else(|| anyhow!("Job not found: {}", job_id.as_str()))?;

        // 2. Deserialize AutopilotJob
        let job: AutopilotJob =
            serde_json::from_slice(&entry.value).context("Failed to deserialize AutopilotJob")?;

        // 3. Validate job configuration
        if job.spec.goal.description.is_empty() {
            anyhow::bail!("Invalid job: empty goal description");
        }

        if job.spec.goal.max_iterations == 0 {
            anyhow::bail!("Invalid job: max_iterations must be > 0");
        }

        Ok(job)
    }

    /// Persist run status to the store.
    ///
    /// Currently a no-op; store integration is pending.
    async fn update_run_status(
        &self,
        _run_id: &AutopilotRunId,
        _status: AutopilotRunStatus,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn finalize_run(
        &self,
        run_id: &AutopilotRunId,
        status: AutopilotRunStatus,
    ) -> anyhow::Result<()> {
        info!(
            "Finalizing run: run_id={}, status={:?}",
            run_id.as_str(),
            status
        );
        self.update_run_status(run_id, status.clone()).await?;

        // Finalize provenance tracking
        let execution_id = ExecutionId::from_string(run_id.as_str().to_string())?;
        match self.provenance_tracker.finalize(&execution_id).await {
            Ok(record) => {
                info!(
                    "Provenance finalized for run_id={}: artifact_id={}",
                    run_id.as_str(),
                    record.artifact_id.as_str()
                );
            }
            Err(e) => {
                warn!(
                    "Failed to finalize provenance for run_id={}: {}",
                    run_id.as_str(),
                    e
                );
            }
        }

        // Reset circuit breaker for next run
        {
            let mut cb = self
                .circuit_breaker
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            cb.reset();
        }

        // Exit auto mode if active
        if let Some(gate) = &self.auto_mode_gate {
            if gate.is_auto_mode(&execution_id) {
                let reason = if status == AutopilotRunStatus::Completed {
                    ExitReason::GoalMet
                } else {
                    ExitReason::UserRequested
                };
                // Use a dummy snapshot since we don't have the original here
                let snapshot = crate::auto_mode_gate::PermissionSnapshot {
                    created_at: Instant::now(),
                    stripped_capabilities: vec![],
                    original_config: serde_json::json!({}),
                };
                let _ = gate.exit_auto_mode(&execution_id, &snapshot, reason).await;
            }
        }

        info!(
            "Run finalized: run_id={}, status={:?}",
            run_id.as_str(),
            status
        );
        Ok(())
    }

    async fn handle_step_failure(
        &self,
        run_id: &AutopilotRunId,
        step: AutopilotStep,
        error: anyhow::Error,
    ) -> anyhow::Result<()> {
        error!(
            "Step {:?} failed for run_id={}: {}",
            step,
            run_id.as_str(),
            error
        );

        // Record failure in circuit breaker
        let tripped = {
            let mut cb = self
                .circuit_breaker
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            cb.record_failure()
        };
        if tripped {
            warn!(
                "Circuit breaker tripped after step {:?} failure for run_id={}",
                step,
                run_id.as_str()
            );
        }

        Ok(())
    }

    /// Record a step result for observability.
    ///
    /// Currently a no-op; observability integration is pending.
    async fn record_step_result(
        &self,
        _run_id: &AutopilotRunId,
        _result: StepResult,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn resolve_stuck(
        &self,
        run_id: &AutopilotRunId,
        _analysis: &ProgressAnalysis,
    ) -> anyhow::Result<StuckResolution> {
        // Simple resolution strategy for now
        // Convert run_id to ExecutionId for iteration tracking
        let execution_id = ExecutionId::from_string(run_id.as_str().to_string())?;

        let iteration = self
            .iteration_tracker
            .current_iteration(&execution_id)
            .await?;

        if iteration < 3 {
            Ok(StuckResolution::Retry)
        } else if iteration < 5 {
            Ok(StuckResolution::ChangeStrategy)
        } else {
            Ok(StuckResolution::Escalate)
        }
    }

    fn build_execution_request(
        &self,
        job: &AutopilotJob,
        state: &Option<IterationState>,
    ) -> anyhow::Result<ExecutionPlan> {
        // 1. Parse goal into actionable steps
        let goal_description = &job.spec.goal.description;
        let _success_criteria = &job.spec.goal.success_criteria;

        // 2. Generate actions based on goal keywords
        let actions = self.build_actions_from_goal(goal_description)?;

        // 3. Filter actions based on current state (avoid repeating)
        let filtered_actions = if let Some(iter_state) = state {
            self.filter_completed_actions(actions, iter_state)?
        } else {
            actions
        };

        // 4. Determine if review required
        let review_required = filtered_actions.iter().any(|a| {
            a.capability == CapabilityId::from_string(normalize_capability_id("fs:write")).unwrap()
                || a.capability
                    == CapabilityId::from_string(normalize_capability_id("cmd.exec")).unwrap()
        });

        // 5. Extract required connectors and capabilities
        let connectors = self.extract_required_connectors(&filtered_actions);
        let capabilities = self.extract_required_capabilities(&filtered_actions);

        Ok(ExecutionPlan {
            resolution: crate::types::Resolution {
                agent: job.spec.agent.id.clone(),
                // Skill resolution requires registry lookup (AgentRef only carries id+role).
                // The Resolver layer populates skills during full Task->Resolution flow;
                // Autopilot direct-plan bypasses the Resolver, so skills remain empty here.
                skills: Vec::new(),
                workflow: None,
                connectors,
                capabilities,
                reasons: vec![format!("Autopilot goal: {}", goal_description)],
            },
            actions: filtered_actions,
            review_required,
            max_fix_loops: DEFAULT_MAX_FIX_LOOPS,
            expected_outcomes: vec![],
        })
    }

    fn compute_state_hash(&self, results: &[ExecutionResult]) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();

        for result in results {
            // Hash execution identity
            result.execution_id.as_str().hash(&mut hasher);

            // Hash status
            format!("{:?}", result.status).hash(&mut hasher);

            // Hash output content (prevents false no-progress on data changes)
            if let Some(output) = &result.output {
                output.to_string().hash(&mut hasher);
            }

            // Hash error messages (different errors = different states)
            if let Some(error) = &result.error {
                error.hash(&mut hasher);
            }

            // Hash artifact IDs (new artifacts = progress)
            for artifact in &result.artifacts {
                artifact.hash(&mut hasher);
            }
        }

        format!("{:x}", hasher.finish())
    }

    #[allow(dead_code)]
    fn compute_progress_metrics(
        &self,
        results: &[ExecutionResult],
    ) -> cyberclaw_core::autopilot::ProgressMetrics {
        let completed = results
            .iter()
            .filter(|r| r.status == ExecutionStatus::Completed)
            .count() as u32;

        let failed = results
            .iter()
            .filter(|r| r.status == ExecutionStatus::Failed)
            .count() as u32;

        // Goal completion percentage, elapsed time, and token usage are not yet
        // tracked; they default to zero until metric collection is implemented.
        cyberclaw_core::autopilot::ProgressMetrics {
            goal_completion_percentage: 0.0,
            actions_completed: completed,
            actions_failed: failed,
            time_elapsed_ms: 0,
            tokens_used: 0,
        }
    }

    // Helper methods for build_execution_request
    fn build_actions_from_goal(&self, goal: &str) -> anyhow::Result<Vec<PlannedAction>> {
        let mut actions = Vec::new();

        // Parse goal keywords to determine action type
        let goal_lower = goal.to_lowercase();

        // Analysis goals - read-only operations
        if goal_lower.contains("analyze")
            || goal_lower.contains("review")
            || goal_lower.contains("audit")
        {
            actions.push(PlannedAction {
                connector_id: ConnectorId::from_string("local".to_string())?,
                capability: CapabilityId::from_string(normalize_capability_id("search.glob"))?,
                input: serde_json::json!({"pattern": "**/*", "path": "."}),
                reason: "List workspace files for analysis".to_string(),
            });

            actions.push(PlannedAction {
                connector_id: ConnectorId::from_string("local".to_string())?,
                capability: CapabilityId::from_string(normalize_capability_id("search:grep"))?,
                input: serde_json::json!({"pattern": goal_lower}),
                reason: format!("Search for patterns related to: {}", goal),
            });
        }
        // Implementation goals - write operations
        else if goal_lower.contains("implement")
            || goal_lower.contains("create")
            || goal_lower.contains("build")
        {
            actions.push(PlannedAction {
                connector_id: ConnectorId::from_string("local".to_string())?,
                capability: CapabilityId::from_string(normalize_capability_id("fs:read"))?,
                input: serde_json::json!({"path": "README.md"}),
                reason: "Read project documentation".to_string(),
            });

            actions.push(PlannedAction {
                connector_id: ConnectorId::from_string("local".to_string())?,
                capability: CapabilityId::from_string(normalize_capability_id("fs:write"))?,
                input: serde_json::json!({"path": "output.txt", "content": "Implementation placeholder"}),
                reason: format!("Implement: {}", goal),
            });
        }
        // Investigation goals - deep search
        else if goal_lower.contains("investigate")
            || goal_lower.contains("debug")
            || goal_lower.contains("find")
        {
            actions.push(PlannedAction {
                connector_id: ConnectorId::from_string("local".to_string())?,
                capability: CapabilityId::from_string(normalize_capability_id("search.glob"))?,
                input: serde_json::json!({"pattern": "**/*"}),
                reason: "Find all files in workspace".to_string(),
            });

            actions.push(PlannedAction {
                connector_id: ConnectorId::from_string("local".to_string())?,
                capability: CapabilityId::from_string(normalize_capability_id("cmd.exec"))?,
                input: serde_json::json!({"command": "ls -la"}),
                reason: "List detailed file information".to_string(),
            });
        }
        // Default/custom goals
        else {
            actions.push(PlannedAction {
                connector_id: ConnectorId::from_string("local".to_string())?,
                capability: CapabilityId::from_string(normalize_capability_id("search.glob"))?,
                input: serde_json::json!({"pattern": "**/*", "path": "."}),
                reason: format!("Explore workspace for: {}", goal),
            });
        }

        // When an alternative strategy is active, reverse action order to try a different path.
        let variant = *self
            .strategy_variant
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if variant > 0 {
            actions.reverse();
        }

        Ok(actions)
    }

    fn filter_completed_actions(
        &self,
        actions: Vec<PlannedAction>,
        state: &IterationState,
    ) -> anyhow::Result<Vec<PlannedAction>> {
        // Filter out actions that have already been successfully completed
        let completed_actions: Vec<String> = state
            .execution_results
            .iter()
            .filter(|r| r.status == ExecutionStatus::Completed)
            .filter_map(|r| r.output.as_ref())
            .filter_map(|o| o.get("action"))
            .filter_map(|a| a.as_str())
            .map(|s| s.to_string())
            .collect();

        let filtered = actions
            .into_iter()
            .filter(|action| !completed_actions.contains(&action.reason))
            .collect();

        Ok(filtered)
    }

    fn extract_required_connectors(&self, actions: &[PlannedAction]) -> Vec<ConnectorId> {
        let mut connectors = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for action in actions {
            if seen.insert(action.connector_id.clone()) {
                connectors.push(action.connector_id.clone());
            }
        }

        connectors
    }

    fn extract_required_capabilities(&self, actions: &[PlannedAction]) -> Vec<CapabilityId> {
        let mut capabilities = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for action in actions {
            if seen.insert(action.capability.clone()) {
                capabilities.push(action.capability.clone());
            }
        }

        capabilities
    }

    /// Save a job to the state store
    pub async fn save_job(&self, job: &AutopilotJob) -> anyhow::Result<()> {
        let key = format!("autopilot:job:{}", job.id.as_str());
        let serialized = serde_json::to_vec(job)?;
        self.state_store.put(key, serialized, 0).await?;
        Ok(())
    }

    // -- 6-phase overlay (Task #18) -----------------------------------------

    /// Install a custom phase dispatcher (builder pattern).
    pub fn with_phase_dispatcher(
        mut self,
        dispatcher: Arc<dyn crate::autopilot_phases::AutopilotPhaseDispatcher>,
    ) -> Self {
        self.phase_dispatcher = dispatcher;
        self
    }

    /// Install a custom [`PhaseSkipPolicy`] (builder pattern).
    pub fn with_phase_skip_policy(
        mut self,
        policy: crate::autopilot_phases::PhaseSkipPolicy,
    ) -> Self {
        self.phase_skip_policy = policy;
        self
    }

    /// Snapshot of the current 6-phase pointer.
    pub fn current_phase(&self) -> crate::autopilot_phases::AutopilotPhase {
        *self.current_phase.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Clone of the append-only 6-phase transition history.
    pub fn phase_history(&self) -> Vec<crate::autopilot_phases::PhaseTransition> {
        self.phase_history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Advance the 6-phase pointer to `AutopilotPhase::next()`.
    ///
    /// Returns the new phase on success. Appends a [`PhaseTransition`] record
    /// to `phase_history`. Fails with [`PhaseError::AlreadyTerminal`] when
    /// called after reaching `Cleanup`.
    ///
    /// NOTE: this is the additive overlay layer for Task #18. The legacy
    /// 5-phase loop in `drive_phase_loop` is unchanged.
    pub fn advance_phase(
        &self,
        reason: impl Into<String>,
    ) -> Result<crate::autopilot_phases::AutopilotPhase, crate::autopilot_phases::PhaseError> {
        let mut current_guard = self.current_phase.lock().unwrap_or_else(|e| e.into_inner());
        let from = *current_guard;
        let (to, transition) =
            crate::autopilot_phases::compute_advance_phase(from, self.phase_skip_policy, reason)?;
        *current_guard = to;
        drop(current_guard);

        self.phase_history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(transition);

        Ok(to)
    }

    /// Jump the 6-phase pointer directly to `to` under the configured policy.
    ///
    /// Emits the dispatcher-agnostic audit record on success.
    pub fn transition_phase(
        &self,
        to: crate::autopilot_phases::AutopilotPhase,
        reason: impl Into<String>,
    ) -> Result<crate::autopilot_phases::AutopilotPhase, crate::autopilot_phases::PhaseError> {
        let reason = reason.into();
        let mut current_guard = self.current_phase.lock().unwrap_or_else(|e| e.into_inner());
        let from = *current_guard;

        crate::autopilot_phases::validate_transition(from, to, self.phase_skip_policy)?;

        *current_guard = to;
        drop(current_guard);

        self.phase_history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(crate::autopilot_phases::PhaseTransition::new(
                from, to, reason,
            ));

        Ok(to)
    }

    /// Invoke the installed phase dispatcher for the current phase.
    ///
    /// Thin pass-through so callers can route one phase's work without
    /// owning the dispatcher themselves. Does not mutate phase state;
    /// callers are responsible for subsequently calling `advance_phase`.
    pub fn dispatch_current_phase(
        &self,
        ctx: &crate::autopilot_phases::PhaseContext,
    ) -> Result<crate::autopilot_phases::PhaseOutcome, crate::autopilot_phases::PhaseError> {
        let phase = self.current_phase();
        self.phase_dispatcher.dispatch(phase, ctx)
    }
}

// ---------------------------------------------------------------------------
// Autopilot 5-Phase Runtime (Sprint 9 Wave 2 L1)
// ---------------------------------------------------------------------------

/// Default `max_fix_loops` budget for the phase-based Autopilot loop.
///
/// Kept as a `const` for callers that don't have an `ExecutionPlan` in hand
/// (e.g. test setup that calls `drive_phase_loop` directly with a pre-set
/// budget). For plan-driven flows prefer reading `plan.max_fix_loops`
/// directly via [`drive_phase_loop_from_plan`] (Sprint 10 partial landing —
/// `types::ExecutionPlan::max_fix_loops` field added 2026-04-26).
pub const DEFAULT_MAX_FIX_LOOPS: u32 = 5;

/// Outcome of a single phase transition in the 5-phase Autopilot runtime.
///
/// Used exclusively by the phase-dispatch driver (`drive_phase_loop`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhaseRunOutcome {
    /// The phase advanced cleanly; the driver should compute the next phase.
    ///
    /// The embedded `Option<VerifyVerdict>` only applies to the `Verify`
    /// phase; other phases always carry `None`.
    Advance(Option<VerifyVerdict>),
    /// The phase terminated the run with a failure.
    ///
    /// The `String` is a human-readable reason (e.g. `"max_fix_loops exceeded"`).
    Fail(String),
}

/// Stub `ExecutionPlanner` trait invoked by the Plan phase.
///
/// Full planning (LLM-driven decomposition, dependency analysis, etc.)
/// is tracked for Sprint 10. For now the runtime only needs a hook that
/// can be injected in tests so the state machine can be exercised
/// end-to-end.
#[async_trait]
pub trait ExecutionPlanner: Send + Sync {
    /// Produce an [`ExecutionPlan`] for the given autopilot job / iteration.
    async fn plan(
        &self,
        _job: &AutopilotJob,
        _iteration: &V2IterationState,
    ) -> anyhow::Result<ExecutionPlan>;
}

/// Default no-op planner that returns an empty plan.
///
/// Retained as a zero-dependency fallback for unit tests, dev environments
/// without an `LlmClient`, and any future planner whose construction can
/// fail. Production deployments should wire
/// [`crate::llm_planner::LlmExecutionPlanner`] for real goal → plan
/// authoring (Sprint 10 landed 2026-04-26).
pub struct NoopExecutionPlanner;

#[async_trait]
impl ExecutionPlanner for NoopExecutionPlanner {
    async fn plan(
        &self,
        job: &AutopilotJob,
        _iteration: &V2IterationState,
    ) -> anyhow::Result<ExecutionPlan> {
        // Intentional placeholder: returns an empty plan so the phase loop
        // can advance without an LLM. See `LlmExecutionPlanner` for the path
        // that actually authors `expected_outcomes` (and optionally `actions`).
        Ok(ExecutionPlan {
            resolution: crate::types::Resolution {
                agent: job.spec.agent.id.clone(),
                skills: Vec::new(),
                workflow: None,
                connectors: Vec::new(),
                capabilities: Vec::new(),
                reasons: vec!["NoopExecutionPlanner placeholder".to_string()],
            },
            actions: Vec::new(),
            review_required: false,
            max_fix_loops: DEFAULT_MAX_FIX_LOOPS,
            expected_outcomes: vec![],
        })
    }
}

/// Verification gate invoked by the Verify phase.
///
/// Distinct from `crate::verification_gate::StagedVerificationGate` which
/// operates on `persistent_execution::ExecutionPlan`; this gate operates
/// on the raw execute-phase output for the 5-phase Autopilot loop.
///
/// Two impls live in this module:
/// - [`AlwaysPassVerificationGate`] — legacy fallback (used when a plan has
///   no evidence contract).
/// - [`EvidenceBasedVerificationGate`] — Sprint 10 default, consumes
///   `ExecutionPlan.expected_outcomes` and rejects when results don't match.
#[async_trait]
pub trait PhaseVerificationGate: Send + Sync {
    /// Verify the results of the latest Execute phase.
    async fn verify(
        &self,
        _plan: &ExecutionPlan,
        _results: &[ExecutionResult],
    ) -> anyhow::Result<VerifyVerdict>;
}

/// Default verifier that always returns `VerifyVerdict::Pass`.
///
/// Kept for legacy callers and as the fallback for plans without
/// `expected_outcomes`. Use [`EvidenceBasedVerificationGate`] for plans that
/// declare evidence contracts (Sprint 10 partial landing).
pub struct AlwaysPassVerificationGate;

#[async_trait]
impl PhaseVerificationGate for AlwaysPassVerificationGate {
    async fn verify(
        &self,
        _plan: &ExecutionPlan,
        _results: &[ExecutionResult],
    ) -> anyhow::Result<VerifyVerdict> {
        Ok(VerifyVerdict::Pass)
    }
}

/// Sprint 10 (gradual landing): verifier that consumes
/// [`crate::types::ExpectedOutcome`] records on the `ExecutionPlan` and
/// matches them against the collected [`ExecutionResult`]s.
///
/// # Semantics
///
/// - **Empty `plan.expected_outcomes`** → `Pass` (backward-compat: existing
///   plans with no evidence contract behave like `AlwaysPassVerificationGate`).
/// - **Non-empty** → every entry must be satisfied by *at least one* result.
///   Missing any single expectation → `Fail`.
///
/// Matchers (v1):
///
/// - `OutputContains(needle)` — at least one `result.output`, when serialised
///   to a JSON string via `serde_json::to_string`, contains `needle` as a
///   substring (case-sensitive).
/// - `StatusEquals(status)` — at least one `result.status`, formatted via
///   `format!("{:?}", status).to_lowercase()`, equals `status`.
///
/// Richer matchers (JSON-path equality, error-pattern, artifact presence)
/// are deferred until LLM-driven planners need them.
pub struct EvidenceBasedVerificationGate;

#[async_trait]
impl PhaseVerificationGate for EvidenceBasedVerificationGate {
    async fn verify(
        &self,
        plan: &ExecutionPlan,
        results: &[ExecutionResult],
    ) -> anyhow::Result<VerifyVerdict> {
        if plan.expected_outcomes.is_empty() {
            return Ok(VerifyVerdict::Pass);
        }

        for outcome in &plan.expected_outcomes {
            let satisfied = match outcome {
                crate::types::ExpectedOutcome::OutputContains(needle) => results.iter().any(|r| {
                    r.output.as_ref().is_some_and(|v| {
                        serde_json::to_string(v)
                            .map(|s| s.contains(needle))
                            .unwrap_or(false)
                    })
                }),
                crate::types::ExpectedOutcome::StatusEquals(expected) => {
                    let want = expected.to_lowercase();
                    results
                        .iter()
                        .any(|r| format!("{:?}", r.status).to_lowercase() == want)
                }
            };
            if !satisfied {
                tracing::debug!(?outcome, "S10: expected outcome not satisfied → Fail");
                return Ok(VerifyVerdict::Fail);
            }
        }
        Ok(VerifyVerdict::Pass)
    }
}

/// Hook invoked on entering the `Plan` phase.
///
/// Strips mutating capabilities via [`PlanModeGate::enter_plan_mode`] and
/// records the serialisable projection on `iteration.plan_mode_snapshot`.
/// Returns the runtime [`PlanPermissionSnapshot`] so the caller can hold
/// it (in-process) and hand it back to [`exit_plan_phase`] when the phase
/// transitions away from `Plan`.
///
/// The persisted projection (`PlanModeSnapshotData`) survives restarts and
/// lets observers inspect which capabilities were stripped during the plan
/// window. The in-memory `PlanPermissionSnapshot` is required to re-expand
/// the full [`CapabilityRef`] list on exit.
pub fn enter_plan_phase(
    iteration: &mut V2IterationState,
    gate: &dyn PlanModeGate,
    caps: &[CapabilityRef],
) -> PlanPermissionSnapshot {
    let snapshot = gate.enter_plan_mode(caps);
    iteration.plan_mode_snapshot = Some(project_plan_snapshot(&snapshot));
    snapshot
}

/// Hook invoked when leaving the `Plan` phase.
///
/// Restores the original capability set via
/// [`PlanModeGate::exit_plan_mode`] and clears
/// `iteration.plan_mode_snapshot`. Must be paired with [`enter_plan_phase`]
/// using the same in-process snapshot.
pub fn exit_plan_phase(
    iteration: &mut V2IterationState,
    gate: &dyn PlanModeGate,
    snapshot: PlanPermissionSnapshot,
) -> Vec<CapabilityRef> {
    let restored = gate.exit_plan_mode(snapshot);
    iteration.plan_mode_snapshot = None;
    restored
}

/// Compute the next phase for `iteration` given an optional `verdict`.
///
/// Transition table (mirrors
/// [`AutopilotPhase::next_phase`](crate::autopilot_types::AutopilotPhase::next_phase)):
///
/// | Current | Verdict     | Next    |
/// |---------|-------------|---------|
/// | Plan    | *           | Execute |
/// | Execute | *           | Verify  |
/// | Verify  | Pass        | Done    |
/// | Verify  | Fail        | Fix     |
/// | Verify  | None        | Verify  |
/// | Fix     | *           | Execute |
/// | Done    | *           | Done    |
///
/// When `current_phase == Verify` and no verdict is available yet the
/// transition is held (returns `Verify`), ensuring the driver will
/// re-invoke the Verify phase before moving on.
pub fn next_phase_after(
    iteration: &V2IterationState,
    verdict: Option<VerifyVerdict>,
) -> AutopilotPhase {
    iteration
        .current_phase
        .next_phase(verdict)
        .unwrap_or(iteration.current_phase)
}

/// Drive `iteration` through the 5-phase state machine until a terminal
/// state is reached, using `phase_runner` to execute each individual
/// phase.
///
/// This is the heart of the new phase-based runtime. It is pure with
/// respect to I/O — all side effects are delegated to `phase_runner` so
/// the state-machine invariants can be unit-tested in isolation.
///
/// Stop conditions (in order):
/// 1. `phase_runner` returns [`PhaseRunOutcome::Fail`] → run is terminated
///    with the reason surfaced to the caller.
/// 2. `iteration.current_phase` becomes [`AutopilotPhase::Done`] → the
///    driver exits cleanly.
/// 3. The Fix phase is entered with `fix_loop_count >= max_fix_loops` →
///    the run is terminated as `Failed("max_fix_loops exceeded")`.
pub async fn drive_phase_loop<F, Fut>(
    iteration: &mut V2IterationState,
    max_fix_loops: u32,
    mut phase_runner: F,
) -> anyhow::Result<Result<(), String>>
where
    F: FnMut(AutopilotPhase, u32) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<PhaseRunOutcome>>,
{
    loop {
        // Enforce max_fix_loops before entering a Fix phase. This guarantees
        // that Fix -> Execute -> Verify(Fail) -> Fix cannot loop forever.
        if iteration.current_phase == AutopilotPhase::Fix
            && iteration.fix_loop_count >= max_fix_loops
        {
            iteration.current_phase = AutopilotPhase::Done;
            return Ok(Err("max_fix_loops exceeded".to_string()));
        }

        if iteration.current_phase.is_terminal() {
            return Ok(Ok(()));
        }

        let phase = iteration.current_phase;
        let outcome = phase_runner(phase, iteration.fix_loop_count).await?;

        match outcome {
            PhaseRunOutcome::Fail(reason) => {
                iteration.current_phase = AutopilotPhase::Done;
                return Ok(Err(reason));
            }
            PhaseRunOutcome::Advance(verdict) => {
                let next = next_phase_after(iteration, verdict);

                // Count *completed* Fix iterations, not entries. This lets
                // the guard permit exactly `max_fix_loops` Fix runs before
                // tripping on entry to the next one.
                if phase == AutopilotPhase::Fix {
                    iteration.fix_loop_count = iteration.fix_loop_count.saturating_add(1);
                }

                iteration.current_phase = next;
            }
        }
    }
}

/// Variant of [`drive_phase_loop`] that wires a [`PlanModeGate`] around the
/// `Plan` phase.
///
/// Semantics:
///
/// 1. When the driver is about to dispatch `AutopilotPhase::Plan`, it invokes
///    [`enter_plan_phase`] with `caps` to strip mutating capabilities. The
///    returned runtime snapshot is retained across the Plan phase.
/// 2. After the Plan phase advances (transitioning to some non-`Plan` phase)
///    the driver invokes [`exit_plan_phase`] with the in-process snapshot to
///    restore the original capability envelope on `caps` for subsequent
///    phases.
///
/// Capabilities handed to `phase_runner` are always the current `caps`
/// slice; downstream phases (Execute / Verify / Fix) therefore see the
/// full permission set, while any enforcement layer reading
/// `iteration.plan_mode_snapshot` sees the Plan-window snapshot.
pub async fn drive_phase_loop_with_plan_gate<F, Fut>(
    iteration: &mut V2IterationState,
    max_fix_loops: u32,
    gate: &dyn PlanModeGate,
    mut caps: Vec<CapabilityRef>,
    mut phase_runner: F,
) -> anyhow::Result<Result<(), String>>
where
    F: FnMut(AutopilotPhase, u32) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<PhaseRunOutcome>>,
{
    let mut active_plan_snapshot: Option<PlanPermissionSnapshot> = None;

    loop {
        // Enforce max_fix_loops before entering a Fix phase. Keep parity
        // with `drive_phase_loop`.
        if iteration.current_phase == AutopilotPhase::Fix
            && iteration.fix_loop_count >= max_fix_loops
        {
            // If we were holding a plan snapshot somehow, restore before
            // bailing. This should not normally happen but we defend
            // against it for crash-consistency. We drop the restored caps
            // because we are about to return.
            if let Some(snap) = active_plan_snapshot.take() {
                let _ = exit_plan_phase(iteration, gate, snap);
            }
            iteration.current_phase = AutopilotPhase::Done;
            return Ok(Err("max_fix_loops exceeded".to_string()));
        }

        if iteration.current_phase.is_terminal() {
            if let Some(snap) = active_plan_snapshot.take() {
                let _ = exit_plan_phase(iteration, gate, snap);
            }
            return Ok(Ok(()));
        }

        let phase = iteration.current_phase;

        // Enter plan mode when the upcoming phase is Plan and we are not
        // already holding a snapshot.
        if phase == AutopilotPhase::Plan && active_plan_snapshot.is_none() {
            active_plan_snapshot = Some(enter_plan_phase(iteration, gate, &caps));
        }

        let outcome = phase_runner(phase, iteration.fix_loop_count).await?;

        match outcome {
            PhaseRunOutcome::Fail(reason) => {
                if let Some(snap) = active_plan_snapshot.take() {
                    let _ = exit_plan_phase(iteration, gate, snap);
                }
                iteration.current_phase = AutopilotPhase::Done;
                return Ok(Err(reason));
            }
            PhaseRunOutcome::Advance(verdict) => {
                let next = next_phase_after(iteration, verdict);

                if phase == AutopilotPhase::Fix {
                    iteration.fix_loop_count = iteration.fix_loop_count.saturating_add(1);
                }

                // If we were inside Plan and the state machine is moving
                // to a non-Plan phase, exit plan mode and restore caps.
                if phase == AutopilotPhase::Plan && next != AutopilotPhase::Plan {
                    if let Some(snap) = active_plan_snapshot.take() {
                        caps = exit_plan_phase(iteration, gate, snap);
                    }
                }

                iteration.current_phase = next;
            }
        }
    }
}

/// Convenience wrapper using the default [`DefaultPlanModeGate`] rules.
///
/// Kept as a thin forwarder so callers without custom gate configuration
/// avoid having to instantiate one themselves.
pub async fn drive_phase_loop_with_default_plan_gate<F, Fut>(
    iteration: &mut V2IterationState,
    max_fix_loops: u32,
    caps: Vec<CapabilityRef>,
    phase_runner: F,
) -> anyhow::Result<Result<(), String>>
where
    F: FnMut(AutopilotPhase, u32) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<PhaseRunOutcome>>,
{
    let gate = DefaultPlanModeGate::new();
    drive_phase_loop_with_plan_gate(iteration, max_fix_loops, &gate, caps, phase_runner).await
}

/// Sprint 10 (partial landing): plan-driven phase loop that reads
/// `max_fix_loops` from `plan.max_fix_loops` instead of taking it as a
/// separate argument. Use this when you already have an [`ExecutionPlan`]
/// constructed by a planner — it avoids the caller having to pluck the
/// field out and pass it through manually.
///
/// Equivalent to `drive_phase_loop_with_default_plan_gate(iteration,
/// plan.max_fix_loops, caps, phase_runner)` — kept as a separate symbol
/// for clarity at call sites.
pub async fn drive_phase_loop_from_plan<F, Fut>(
    iteration: &mut V2IterationState,
    plan: &crate::types::ExecutionPlan,
    caps: Vec<CapabilityRef>,
    phase_runner: F,
) -> anyhow::Result<Result<(), String>>
where
    F: FnMut(AutopilotPhase, u32) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<PhaseRunOutcome>>,
{
    drive_phase_loop_with_default_plan_gate(iteration, plan.max_fix_loops, caps, phase_runner).await
}

#[cfg(test)]
mod tests {
    use super::normalize_capability_id;
    use cyberclaw_core::autopilot::ExecutionResult;
    use cyberclaw_core::execution::ExecutionStatus;
    use cyberclaw_core::ids::ExecutionId;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    // Helper to compute state hash without runtime (copy of the implementation)
    fn compute_state_hash(results: &[ExecutionResult]) -> String {
        let mut hasher = DefaultHasher::new();

        for result in results {
            // Hash execution identity
            result.execution_id.as_str().hash(&mut hasher);

            // Hash status
            format!("{:?}", result.status).hash(&mut hasher);

            // Hash output content
            if let Some(output) = &result.output {
                output.to_string().hash(&mut hasher);
            }

            // Hash error messages
            if let Some(error) = &result.error {
                error.hash(&mut hasher);
            }

            // Hash artifact IDs
            for artifact in &result.artifacts {
                artifact.hash(&mut hasher);
            }
        }

        format!("{:x}", hasher.finish())
    }

    fn create_test_result(
        execution_id: ExecutionId,
        status: ExecutionStatus,
        output: Option<serde_json::Value>,
        error: Option<String>,
        artifacts: Vec<String>,
    ) -> ExecutionResult {
        ExecutionResult {
            execution_id,
            status,
            output,
            error,
            artifacts,
            duration_ms: 0,
        }
    }

    #[test]
    fn test_state_hash_changes_with_output() {
        let execution_id = ExecutionId::new();

        let results1 = vec![create_test_result(
            execution_id.clone(),
            ExecutionStatus::Completed,
            Some(serde_json::json!({"data": "version 1"})),
            None,
            Vec::new(),
        )];

        let results2 = vec![create_test_result(
            execution_id.clone(), // Same ID
            ExecutionStatus::Completed,
            Some(serde_json::json!({"data": "version 2"})), // Different output
            None,
            Vec::new(),
        )];

        let hash1 = compute_state_hash(&results1);
        let hash2 = compute_state_hash(&results2);

        assert_ne!(
            hash1, hash2,
            "Different outputs should produce different hashes"
        );
    }

    #[test]
    fn test_state_hash_changes_with_errors() {
        let execution_id = ExecutionId::new();

        let results1 = vec![create_test_result(
            execution_id.clone(),
            ExecutionStatus::Failed,
            None,
            Some("Connection timeout".to_string()),
            Vec::new(),
        )];

        let results2 = vec![create_test_result(
            execution_id.clone(), // Same ID
            ExecutionStatus::Failed,
            None,
            Some("Authentication failed".to_string()), // Different error
            Vec::new(),
        )];

        let hash1 = compute_state_hash(&results1);
        let hash2 = compute_state_hash(&results2);

        assert_ne!(
            hash1, hash2,
            "Different errors should produce different hashes"
        );
    }

    #[test]
    fn test_state_hash_changes_with_artifacts() {
        let execution_id = ExecutionId::new();

        let results1 = vec![create_test_result(
            execution_id.clone(),
            ExecutionStatus::Completed,
            Some(serde_json::json!({"data": "same"})),
            None,
            vec!["artifact-1".to_string()],
        )];

        let results2 = vec![create_test_result(
            execution_id.clone(), // Same ID
            ExecutionStatus::Completed,
            Some(serde_json::json!({"data": "same"})), // Same output
            None,
            vec!["artifact-1".to_string(), "artifact-2".to_string()], // New artifact
        )];

        let hash1 = compute_state_hash(&results1);
        let hash2 = compute_state_hash(&results2);

        assert_ne!(
            hash1, hash2,
            "Different artifacts should produce different hashes"
        );
    }

    #[test]
    fn test_state_hash_stable_for_identical_results() {
        let execution_id = ExecutionId::new();

        let results1 = vec![create_test_result(
            execution_id.clone(),
            ExecutionStatus::Completed,
            Some(serde_json::json!({"data": "value"})),
            None,
            vec!["artifact-1".to_string()],
        )];

        let results2 = vec![create_test_result(
            execution_id.clone(),
            ExecutionStatus::Completed,
            Some(serde_json::json!({"data": "value"})),
            None,
            vec!["artifact-1".to_string()],
        )];

        let hash1 = compute_state_hash(&results1);
        let hash2 = compute_state_hash(&results2);

        assert_eq!(
            hash1, hash2,
            "Identical results should produce identical hashes"
        );
    }

    #[test]
    fn test_state_hash_with_none_values() {
        let execution_id = ExecutionId::new();

        let results1 = vec![create_test_result(
            execution_id.clone(),
            ExecutionStatus::Completed,
            None,       // No output
            None,       // No error
            Vec::new(), // No artifacts
        )];

        let results2 = vec![create_test_result(
            execution_id.clone(),
            ExecutionStatus::Completed,
            Some(serde_json::json!({"data": "value"})), // Has output now
            None,
            Vec::new(),
        )];

        let hash1 = compute_state_hash(&results1);
        let hash2 = compute_state_hash(&results2);

        assert_ne!(hash1, hash2, "Adding output should change the hash");
    }

    // ──────────────────────────────────────────────────────────────────────────
    // AutopilotHandle Tests
    // ──────────────────────────────────────────────────────────────────────────

    // AutopilotHandle cancel/wait tests require a complete mock ExecutionService
    // and full runtime setup. They will be added when integration test
    // infrastructure is available.

    #[test]
    fn test_no_block_in_place() {
        // This test verifies that we're not using block_in_place anymore
        // by checking the source code (a simple grep would be better but this is a compile-time check)

        let source = include_str!("autopilot_iteration.rs");

        // Check that block_in_place is not present in the iteration tracker
        assert!(
            !source.contains("block_in_place"),
            "autopilot_iteration.rs should not contain block_in_place"
        );
    }

    #[test]
    fn test_normalize_capability_id_colon_to_dot() {
        assert_eq!(normalize_capability_id("fs:write"), "fs.write");
        assert_eq!(normalize_capability_id("search:grep"), "search.grep");
        assert_eq!(normalize_capability_id("cmd.exec"), "cmd.exec");
    }

    // ──────────────────────────────────────────────────────────────────────────
    // AutopilotLoopBridge & AutopilotLoopConfig Tests
    // ──────────────────────────────────────────────────────────────────────────

    use super::{AutopilotLoopBridge, AutopilotLoopConfig};

    #[test]
    fn test_autopilot_loop_config_default() {
        let config = AutopilotLoopConfig::default();
        assert_eq!(config.max_iterations, 30);
        assert_eq!(config.stuck_threshold, 3);
        assert_eq!(config.model, "gpt-4");
        assert!(config.system_prompt.is_empty());
    }

    #[test]
    fn test_autopilot_loop_config_to_loop_config() {
        let config = AutopilotLoopConfig {
            system_prompt: "You are a test agent.".to_string(),
            model: "claude-3".to_string(),
            max_iterations: 20,
            stuck_threshold: 5,
        };

        let loop_config = config.to_loop_config();
        assert_eq!(loop_config.system_prompt, "You are a test agent.");
        assert_eq!(loop_config.model, "claude-3");
        assert_eq!(loop_config.budget.max_iterations, 20);
        assert_eq!(loop_config.stuck_threshold, 5);
    }

    #[test]
    fn test_autopilot_loop_bridge_build_system_prompt() {
        use crate::types::{ExecutionPlan, PlannedAction, Resolution};
        use cyberclaw_core::ids::{AgentId, CapabilityId, ConnectorId};

        let plan = ExecutionPlan {
            resolution: Resolution {
                agent: AgentId::from_string("test-agent".to_string()).unwrap(),
                skills: Vec::new(),
                workflow: None,
                connectors: Vec::new(),
                capabilities: Vec::new(),
                reasons: vec!["test".to_string()],
            },
            actions: vec![
                PlannedAction {
                    connector_id: ConnectorId::from_string("local".to_string()).unwrap(),
                    capability: CapabilityId::from_string("fs.read".to_string()).unwrap(),
                    input: serde_json::json!({"path": "test.txt"}),
                    reason: "Read test file".to_string(),
                },
                PlannedAction {
                    connector_id: ConnectorId::from_string("local".to_string()).unwrap(),
                    capability: CapabilityId::from_string("fs.write".to_string()).unwrap(),
                    input: serde_json::json!({"path": "out.txt"}),
                    reason: "Write output".to_string(),
                },
            ],
            review_required: false,
            max_fix_loops: crate::types::default_max_fix_loops(),
            expected_outcomes: vec![],
        };

        let prompt = AutopilotLoopBridge::build_system_prompt(&plan);
        assert!(prompt.contains("autonomous agent"));
        assert!(prompt.contains("fs.read"));
        assert!(prompt.contains("fs.write"));
        assert!(prompt.contains("Read test file"));
        assert!(prompt.contains("Write output"));
    }

    #[test]
    fn test_autopilot_loop_bridge_creation() {
        use async_trait::async_trait;
        use cyberclaw_core::gateway::{
            CapabilityInfo, CapabilityRequest, CapabilityResult, GatewayError, OrchestratorGateway,
        };
        use cyberclaw_llm::client::LlmClient;
        use cyberclaw_llm::error::LlmResult;
        use cyberclaw_llm::prelude::Stream;
        use cyberclaw_llm::types::{ChatChunk, ChatRequest, ChatResponse};
        use std::sync::Arc;

        // Minimal mock LLM
        struct StubLlm;
        #[async_trait]
        impl LlmClient for StubLlm {
            async fn chat_completion(&self, _req: ChatRequest) -> LlmResult<ChatResponse> {
                unimplemented!()
            }
            async fn chat_completion_stream(
                &self,
                _req: ChatRequest,
            ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>>
            {
                unimplemented!()
            }
            fn provider(&self) -> &str {
                "stub"
            }
            async fn validate_connection(&self) -> LlmResult<()> {
                Ok(())
            }
        }

        // Minimal mock Gateway
        struct StubGateway;
        #[async_trait]
        impl OrchestratorGateway for StubGateway {
            async fn execute_capability(
                &self,
                _req: CapabilityRequest,
            ) -> Result<CapabilityResult, GatewayError> {
                unimplemented!()
            }
            async fn list_capabilities(&self) -> Result<Vec<CapabilityInfo>, GatewayError> {
                Ok(vec![])
            }
        }

        let bridge = AutopilotLoopBridge::new(
            Arc::new(StubLlm),
            Arc::new(StubGateway),
            AutopilotLoopConfig::default(),
        );

        // Verify bridge holds the expected config
        assert_eq!(bridge.config.max_iterations, 30);
        assert_eq!(bridge.config.stuck_threshold, 3);
    }
}

#[cfg(test)]
#[path = "autopilot_runtime_tests.rs"]
mod autopilot_runtime_tests;
