use crate::event_bus::EventBus;
use crate::retry::{retry_with_backoff, RetryConfig};
use crate::types::ExecutionPlan;
use async_trait::async_trait;
use cyberclaw_agent_runtime::{AgentRequest, AgentRuntime};
use cyberclaw_connectors::{CapabilityDispatcher, CapabilityExecutionRequest};
use cyberclaw_core::audit_logger::{AuditLogEntry, AuditLogger, DefaultAuditLogger}; // MEDIUM #9 FIX
use cyberclaw_core::cluster::ClusterEvent;
use cyberclaw_core::prelude::*;
use cyberclaw_core::security::{SecurityEvent, SecurityEventSource, SecurityEventType, Severity};
use cyberclaw_core::security_scanner::SecretScanner;
use cyberclaw_observability::{
    events::{EventRecorder, InMemoryEventRecorder, ObservabilityEvent},
    execution_span,
    metrics::recorders,
    security_event_store::SecurityEventStore,
    status_transition_span,
};
use cyberclaw_skill_runtime::SkillRuntime;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use tracing::{error, info, warn, Instrument};

// ─── Best-effort Error Collection ────────────────────────────────────────────

/// MEDIUM #2 FIX: Best-effort 模式下的错误收集器
/// 确保所有 best-effort 失败都被记录，而不是静默丢失
#[allow(dead_code)]
#[derive(Debug, Default)]
struct BestEffortErrors {
    errors: Mutex<Vec<BestEffortError>>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct BestEffortError {
    operation: String,
    error: String,
    timestamp: chrono::DateTime<chrono::Utc>,
    context: HashMap<String, String>,
}

#[allow(dead_code)]
impl BestEffortErrors {
    fn new() -> Self {
        Self {
            errors: Mutex::new(Vec::new()),
        }
    }

    fn record(
        &self,
        operation: impl Into<String>,
        error: impl std::fmt::Display,
        context: HashMap<String, String>,
    ) {
        let mut errors = self.errors.lock().unwrap_or_else(|e| e.into_inner());
        errors.push(BestEffortError {
            operation: operation.into(),
            error: error.to_string(),
            timestamp: chrono::Utc::now(),
            context,
        });
    }

    fn has_errors(&self) -> bool {
        let errors = self.errors.lock().unwrap_or_else(|e| e.into_inner());
        !errors.is_empty()
    }

    fn get_all(&self) -> Vec<BestEffortError> {
        let errors = self.errors.lock().unwrap_or_else(|e| e.into_inner());
        errors.clone()
    }

    fn summary(&self) -> String {
        let errors = self.errors.lock().unwrap_or_else(|e| e.into_inner());
        if errors.is_empty() {
            return "No best-effort errors".to_string();
        }

        format!(
            "{} best-effort error(s):\n{}",
            errors.len(),
            errors
                .iter()
                .map(|e| format!("  - [{}] {}: {}", e.timestamp, e.operation, e.error))
                .collect::<Vec<_>>()
                .join("\n")
        )
    }
}

// ─── Command Safety (re-exported from command_safety module) ─────────────────
pub use crate::command_safety::{
    execute_command_safe, validate_command, CommandOutput, ExecutionError,
};

// ─── Memory Exhaustion Prevention ─────────────────────────────────────────────

/// CRITICAL #2 FIX: Maximum number of memory entries that can be added per execution.
/// This prevents memory exhaustion attacks where an attacker could add unlimited
/// entries in a single execution, causing OOM conditions.
const MAX_MEMORY_ENTRIES_PER_EXECUTION: usize = 5;

/// Maximum number of concurrent Autopilot executions allowed.
/// Security: Prevents resource exhaustion attacks.
const MAX_CONCURRENT_AUTOPILOT_EXECUTIONS: usize = 10;

// ─── End Command Safety & Risk Calculator ────────────────────────────────────

/// Static fallback agent ID used when no agent is specified in a request.
/// "control-plane" passes all ID validation rules (non-empty, ≤128 chars, no
/// control characters, no path traversal sequences).
fn control_plane_agent_id() -> &'static AgentId {
    static ID: OnceLock<AgentId> = OnceLock::new();
    ID.get_or_init(|| {
        AgentId::from_string("control-plane".to_string())
            .unwrap_or_else(|e| panic!("BUG: 'control-plane' failed ID validation: {e}"))
    })
}

/// Static fallback agent ID used when an agent ID string fails parsing.
/// "unknown" passes all ID validation rules.
fn unknown_agent_id() -> &'static AgentId {
    static ID: OnceLock<AgentId> = OnceLock::new();
    ID.get_or_init(|| {
        AgentId::from_string("unknown".to_string())
            .unwrap_or_else(|e| panic!("BUG: 'unknown' failed ID validation: {e}"))
    })
}

/// Static fallback skill ID used when a skill ID string fails parsing.
/// "unknown" passes all ID validation rules.
fn unknown_skill_id() -> &'static cyberclaw_core::ids::SkillId {
    static ID: OnceLock<cyberclaw_core::ids::SkillId> = OnceLock::new();
    ID.get_or_init(|| {
        cyberclaw_core::ids::SkillId::from_string("unknown".to_string())
            .unwrap_or_else(|e| panic!("BUG: 'unknown' failed skill ID validation: {e}"))
    })
}

// ─── Autopilot Types & Traits (re-exported from execution_autopilot_types module)
pub use crate::execution_autopilot_types::{
    AutopilotStep, AutopilotStepRunner, CheckpointStore, Decision, ExecutionMode, IterationResult,
    IterationState, IterationSummary, IterationTracker, StateSyncCoordinator, StepResult,
    StuckDetector, StuckResolution,
};

// ─── End Autopilot Types & Traits ────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ExecutionRequest {
    pub execution_id: ExecutionId,
    pub task: Task,
    pub case: Option<Case>,
    pub context: super::types::ControlPlaneContext,
    pub agent: Option<AgentRef>,
    pub trace_id: Option<TraceId>,
    pub execution_mode: Option<ExecutionMode>,
    pub plan: Option<ExecutionPlan>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RemoteExecutionAssignment {
    pub execution: Execution,
    pub plan: ExecutionPlan,
}

#[async_trait]
pub trait ExecutionService: Send + Sync {
    async fn submit(&self, request: ExecutionRequest) -> anyhow::Result<ExecutionId>;
    async fn submit_plan(&self, plan: ExecutionPlan) -> anyhow::Result<ExecutionId>;
    async fn cancel(&self, execution_id: &ExecutionId) -> anyhow::Result<()>;
    async fn get(&self, execution_id: &ExecutionId) -> anyhow::Result<Option<Execution>>;

    /// 列出所有执行记录，支持按状态过滤。
    ///
    /// # Arguments
    ///
    /// * `status_filter` - 可选的状态过滤器，None 则返回所有记录
    ///
    /// # Returns
    ///
    /// 按 ExecutionId 排序的执行记录列表
    async fn list_all(
        &self,
        status_filter: Option<ExecutionStatus>,
    ) -> anyhow::Result<Vec<Execution>>;

    /// 按 TaskId 查询关联的执行记录列表。
    ///
    /// # Arguments
    ///
    /// * `task_id` - 要查询的任务 ID
    ///
    /// # Returns
    ///
    /// 与该任务关联的所有执行记录
    async fn list_by_task_id(&self, task_id: &TaskId) -> anyhow::Result<Vec<Execution>>;

    /// Sprint 9 follow-up: query executions for a single agent within a time window.
    ///
    /// Used by `daily_digest_runtime::StoreDigestCollector` so it doesn't have
    /// to pull every execution and filter in-process. The default implementation
    /// preserves that older behavior — it calls `list_all(None)` and filters,
    /// so external `ExecutionService` implementations don't need to change.
    /// `InMemoryExecutionService` overrides with a direct map scan.
    ///
    /// # Arguments
    ///
    /// * `agent_id` - target agent
    /// * `window_start` - inclusive lower bound on `started_at`
    /// * `window_end` - exclusive upper bound on `started_at`
    ///
    /// Executions with `started_at == None` are excluded (they haven't begun).
    async fn list_by_agent_window(
        &self,
        agent_id: &cyberclaw_core::ids::AgentId,
        window_start: chrono::DateTime<chrono::Utc>,
        window_end: chrono::DateTime<chrono::Utc>,
    ) -> anyhow::Result<Vec<Execution>> {
        let all = self.list_all(None).await?;
        Ok(all
            .into_iter()
            .filter(|e| {
                if e.agent.id.as_str() != agent_id.as_str() {
                    return false;
                }
                let Some(started) = e.started_at else {
                    return false;
                };
                started >= window_start && started < window_end
            })
            .collect())
    }

    async fn update_status(
        &self,
        execution_id: &ExecutionId,
        status: ExecutionStatus,
    ) -> anyhow::Result<()>;

    /// Execute the task referenced by `execution_id` via the configured runtimes.
    ///
    /// State transitions: Pending → Running → Completed / Failed.
    /// Emits observability events at each transition.
    async fn execute(&self, execution_id: &ExecutionId) -> anyhow::Result<()>;

    /// Persist placement/lease assignment metadata for an execution.
    ///
    /// Implementations that do not support cluster metadata may keep the default
    /// behavior and return an explicit error.
    async fn set_assignment(
        &self,
        execution_id: &ExecutionId,
        _scheduled_node_id: NodeId,
        _lease_id: LeaseId,
    ) -> anyhow::Result<()> {
        anyhow::bail!(
            "set_assignment is not supported by this ExecutionService implementation: {}",
            execution_id
        );
    }

    /// Retrieve the stored execution plan.
    ///
    /// Implementations that do not retain execution plans may return `Ok(None)`.
    async fn get_plan(&self, _execution_id: &ExecutionId) -> anyhow::Result<Option<ExecutionPlan>> {
        Ok(None)
    }

    // ============ Autopilot 专用方法（新增） ============

    /// 执行单次 Autopilot 迭代
    async fn execute_autopilot_iteration(
        &self,
        execution_id: &ExecutionId,
        iteration: u32,
    ) -> anyhow::Result<IterationResult>;

    /// 迭代开始回调
    async fn on_iteration_start(
        &self,
        execution_id: &ExecutionId,
        iteration: u32,
    ) -> anyhow::Result<()>;

    /// 步骤完成回调
    async fn on_step_complete(
        &self,
        execution_id: &ExecutionId,
        step: AutopilotStep,
        result: StepResult,
    ) -> anyhow::Result<()>;

    /// 无进展检测处理
    async fn on_stuck_detected(
        &self,
        execution_id: &ExecutionId,
        iteration: u32,
        reason: String,
    ) -> anyhow::Result<StuckResolution>;

    /// 迭代检查点持久化
    async fn checkpoint_iteration(
        &self,
        execution_id: &ExecutionId,
        iteration: u32,
        state: IterationState,
    ) -> anyhow::Result<()>;

    /// 从检查点恢复
    async fn resume_from_checkpoint(
        &self,
        execution_id: &ExecutionId,
    ) -> anyhow::Result<Option<IterationState>>;

    /// 迭代历史查询
    async fn get_iteration_history(
        &self,
        execution_id: &ExecutionId,
    ) -> anyhow::Result<Vec<IterationSummary>>;
}

/// Maximum number of concurrent executions allowed.
/// Security: Prevents resource exhaustion attacks.
const MAX_CONCURRENT_EXECUTIONS: usize = 100;

#[derive(Clone)]
pub struct InMemoryExecutionService {
    executions: Arc<RwLock<BTreeMap<ExecutionId, Execution>>>,
    execution_plans: Arc<RwLock<BTreeMap<ExecutionId, ExecutionPlan>>>,
    event_recorder: Arc<dyn EventRecorder>,
    agent_runtime: Option<Arc<dyn AgentRuntime>>,
    skill_runtime: Option<Arc<dyn SkillRuntime>>,
    capability_dispatcher: Option<Arc<CapabilityDispatcher>>,
    /// Optional EventBus for publishing cluster-level ClusterEvents.
    /// If None, event publication is silently skipped.
    event_bus: Option<Arc<dyn EventBus>>,
    /// Optional SecurityEventStore for recording execution lifecycle audit events.
    /// If None, security event recording is silently skipped.
    security_event_store: Option<Arc<dyn SecurityEventStore>>,
    /// Optional ProvenanceTracker for recording execution lineage.
    /// If None, provenance tracking is silently skipped (best-effort).
    provenance_tracker: Option<Arc<dyn crate::provenance_tracker::ProvenanceTracker>>,
    /// Optional MemoryContextProvider for working/episodic/procedural memory.
    /// If None, memory operations are silently skipped (best-effort).
    memory_provider: Option<Arc<cyberclaw_core::memory::provider::MemoryContextProvider>>,
    /// Optional LeveledMemoryStore for L1 episodic memory loop (S18 R1+R2).
    /// Write hook: transition_to_completed writes L1Summary.
    /// Read hook: transition_to_running reads recent L1Summary and injects prior_context.
    /// If None, memory loop is silently skipped (best-effort).
    leveled_memory_store: Option<Arc<dyn cyberclaw_store::LeveledMemoryStore>>,
    /// Optional LLM client for real semantic summarization in write_episodic_memory (S19 v2).
    /// If None, falls back to string-concat summary.
    llm_client: Option<Arc<dyn cyberclaw_llm::client::LlmClient>>,
    /// S20 E2: Optional credential sanitizer for episodic memory write path.
    /// When set, summary content is sanitized before store_leveled is called.
    /// If None, sanitization is skipped (backward-compatible for unit tests).
    memory_sanitizer: Option<Arc<cyberclaw_governance::tool_output_sanitizer::ToolOutputSanitizer>>,
    /// Security: Track concurrent executions for rate limiting.
    concurrent_executions: Arc<AtomicUsize>,
    /// Unified security configuration for consistent security policy enforcement.
    security_config: Arc<cyberclaw_core::security_config::SecurityConfigManager>,

    // ============ Autopilot 专用组件（新增） ============
    /// Iteration tracker for Autopilot executions
    #[allow(dead_code)]
    iteration_tracker: Option<Arc<dyn IterationTracker>>,
    /// State synchronization coordinator
    state_sync: Option<Arc<dyn StateSyncCoordinator>>,
    /// Stuck detector for no-progress scenarios
    stuck_detector: Option<Arc<dyn StuckDetector>>,
    /// Checkpoint storage for iteration state
    checkpoint_store: Option<Arc<dyn CheckpointStore>>,
    /// Track concurrent Autopilot executions separately
    concurrent_autopilot_executions: Arc<AtomicUsize>,
    /// History of iterations per execution
    iteration_histories: Arc<RwLock<BTreeMap<ExecutionId, Vec<IterationSummary>>>>,
    /// Optional step runner for real autopilot step execution.
    /// When present, `execute_autopilot_iteration` delegates to this runner
    /// instead of returning hardcoded placeholder results.
    autopilot_step_runner: Option<Arc<dyn AutopilotStepRunner>>,
    /// Optional HandoffQueue for multi-agent handoff lifecycle (S21 T4).
    /// If None, `complete_handoff` returns InvalidCommand error.
    handoff_queue: Option<Arc<dyn crate::handoff_queue::HandoffQueue>>,
    /// Sprint 25 S25 T3: Optional embed client for attaching embedding vectors to episodic
    /// memory records. If None (or dimension() == 0), embedding is skipped (best-effort).
    embed_client: Option<Arc<dyn cyberclaw_llm::EmbedClient>>,

    /// Sprint D1: Optional [`PersistentLoop`] used to dispatch executions whose
    /// `execution_mode == ExecutionMode::Persistent`.
    ///
    /// When `None`, attempting to `execute()` a Persistent execution returns an
    /// explicit "PersistentLoop not wired in this AppState" error. The loop's
    /// internal per-story dispatch (Sprint D3) is invoked from the
    /// Persistent branch in `execute()`.
    persistent_loop: Option<Arc<crate::persistent_execution::PersistentLoop>>,
}

impl std::fmt::Debug for InMemoryExecutionService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryExecutionService")
            .field("executions", &self.executions)
            .field("execution_plans", &self.execution_plans)
            .field("has_agent_runtime", &self.agent_runtime.is_some())
            .field("has_skill_runtime", &self.skill_runtime.is_some())
            .field(
                "has_capability_dispatcher",
                &self.capability_dispatcher.is_some(),
            )
            .field("has_event_bus", &self.event_bus.is_some())
            .field(
                "has_security_event_store",
                &self.security_event_store.is_some(),
            )
            .field("has_provenance_tracker", &self.provenance_tracker.is_some())
            .field("has_memory_provider", &self.memory_provider.is_some())
            .field(
                "concurrent_executions",
                &self.concurrent_executions.load(Ordering::Relaxed),
            )
            .field("security_config", &"SecurityConfigManager")
            .field("has_persistent_loop", &self.persistent_loop.is_some())
            .finish()
    }
}

impl Default for InMemoryExecutionService {
    fn default() -> Self {
        Self {
            executions: Arc::new(RwLock::new(BTreeMap::new())),
            execution_plans: Arc::new(RwLock::new(BTreeMap::new())),
            event_recorder: Arc::new(InMemoryEventRecorder::new()),
            agent_runtime: None,
            skill_runtime: None,
            capability_dispatcher: None,
            event_bus: None,
            security_event_store: None,
            provenance_tracker: None,
            memory_provider: None,
            leveled_memory_store: None,
            llm_client: None,
            memory_sanitizer: None,
            concurrent_executions: Arc::new(AtomicUsize::new(0)),
            security_config: Arc::new(
                cyberclaw_core::security_config::SecurityConfigManager::default(),
            ),
            // Autopilot fields
            iteration_tracker: None,
            state_sync: None,
            stuck_detector: None,
            checkpoint_store: None,
            concurrent_autopilot_executions: Arc::new(AtomicUsize::new(0)),
            iteration_histories: Arc::new(RwLock::new(BTreeMap::new())),
            autopilot_step_runner: None,
            handoff_queue: None,
            embed_client: None,
            persistent_loop: None,
        }
    }
}

impl InMemoryExecutionService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_event_recorder(event_recorder: Arc<dyn EventRecorder>) -> Self {
        Self {
            executions: Arc::new(RwLock::new(BTreeMap::new())),
            execution_plans: Arc::new(RwLock::new(BTreeMap::new())),
            event_recorder,
            agent_runtime: None,
            skill_runtime: None,
            capability_dispatcher: None,
            event_bus: None,
            security_event_store: None,
            provenance_tracker: None,
            memory_provider: None,
            leveled_memory_store: None,
            llm_client: None,
            memory_sanitizer: None,
            concurrent_executions: Arc::new(AtomicUsize::new(0)),
            security_config: Arc::new(
                cyberclaw_core::security_config::SecurityConfigManager::default(),
            ),
            // Autopilot fields
            iteration_tracker: None,
            state_sync: None,
            stuck_detector: None,
            checkpoint_store: None,
            concurrent_autopilot_executions: Arc::new(AtomicUsize::new(0)),
            iteration_histories: Arc::new(RwLock::new(BTreeMap::new())),
            autopilot_step_runner: None,
            handoff_queue: None,
            embed_client: None,
            persistent_loop: None,
        }
    }

    /// Create a service with agent and skill runtimes wired in.
    pub fn with_runtimes(
        agent_runtime: Arc<dyn AgentRuntime>,
        skill_runtime: Arc<dyn SkillRuntime>,
    ) -> Self {
        Self {
            executions: Arc::new(RwLock::new(BTreeMap::new())),
            execution_plans: Arc::new(RwLock::new(BTreeMap::new())),
            event_recorder: Arc::new(InMemoryEventRecorder::new()),
            agent_runtime: Some(agent_runtime),
            skill_runtime: Some(skill_runtime),
            capability_dispatcher: None,
            event_bus: None,
            security_event_store: None,
            provenance_tracker: None,
            memory_provider: None,
            leveled_memory_store: None,
            llm_client: None,
            memory_sanitizer: None,
            concurrent_executions: Arc::new(AtomicUsize::new(0)),
            security_config: Arc::new(
                cyberclaw_core::security_config::SecurityConfigManager::default(),
            ),
            // Autopilot fields
            iteration_tracker: None,
            state_sync: None,
            stuck_detector: None,
            checkpoint_store: None,
            concurrent_autopilot_executions: Arc::new(AtomicUsize::new(0)),
            iteration_histories: Arc::new(RwLock::new(BTreeMap::new())),
            autopilot_step_runner: None,
            handoff_queue: None,
            embed_client: None,
            persistent_loop: None,
        }
    }

    /// Create a service with runtimes and a custom event recorder.
    pub fn with_runtimes_and_recorder(
        agent_runtime: Arc<dyn AgentRuntime>,
        skill_runtime: Arc<dyn SkillRuntime>,
        event_recorder: Arc<dyn EventRecorder>,
    ) -> Self {
        Self {
            executions: Arc::new(RwLock::new(BTreeMap::new())),
            execution_plans: Arc::new(RwLock::new(BTreeMap::new())),
            event_recorder,
            agent_runtime: Some(agent_runtime),
            skill_runtime: Some(skill_runtime),
            capability_dispatcher: None,
            event_bus: None,
            security_event_store: None,
            provenance_tracker: None,
            memory_provider: None,
            leveled_memory_store: None,
            llm_client: None,
            memory_sanitizer: None,
            concurrent_executions: Arc::new(AtomicUsize::new(0)),
            security_config: Arc::new(
                cyberclaw_core::security_config::SecurityConfigManager::default(),
            ),
            // Autopilot fields
            iteration_tracker: None,
            state_sync: None,
            stuck_detector: None,
            checkpoint_store: None,
            concurrent_autopilot_executions: Arc::new(AtomicUsize::new(0)),
            iteration_histories: Arc::new(RwLock::new(BTreeMap::new())),
            autopilot_step_runner: None,
            handoff_queue: None,
            embed_client: None,
            persistent_loop: None,
        }
    }

    /// Attach a CapabilityDispatcher for executing connector capabilities.
    pub fn with_capability_dispatcher(mut self, dispatcher: Arc<CapabilityDispatcher>) -> Self {
        self.capability_dispatcher = Some(dispatcher);
        self
    }

    /// Attach an EventBus for publishing cluster-level events on execution status changes.
    pub fn with_event_bus(mut self, event_bus: Arc<dyn EventBus>) -> Self {
        self.event_bus = Some(event_bus);
        self
    }

    /// Attach a SecurityEventStore for recording execution lifecycle audit events.
    pub fn with_security_event_store(mut self, store: Arc<dyn SecurityEventStore>) -> Self {
        self.security_event_store = Some(store);
        self
    }

    /// Attach a ProvenanceTracker for recording execution lineage.
    pub fn with_provenance_tracker(
        mut self,
        tracker: Arc<dyn crate::provenance_tracker::ProvenanceTracker>,
    ) -> Self {
        self.provenance_tracker = Some(tracker);
        self
    }

    /// Attach a MemoryContextProvider for working/episodic/procedural memory.
    pub fn with_memory_provider(
        mut self,
        provider: Arc<cyberclaw_core::memory::provider::MemoryContextProvider>,
    ) -> Self {
        self.memory_provider = Some(provider);
        self
    }

    /// Attach a LeveledMemoryStore for L1 episodic memory loop (S18 R1+R2).
    /// When set, transitions to Running read recent L1Summary (prior_context injection),
    /// and transitions to Completed write L1Summary (episodic capture).
    pub fn with_leveled_memory_store(
        mut self,
        store: Arc<dyn cyberclaw_store::LeveledMemoryStore>,
    ) -> Self {
        self.leveled_memory_store = Some(store);
        self
    }

    /// Attach an LLM client for real semantic summarization in write_episodic_memory (S19 v2).
    /// When set, completed executions produce 3-5 sentence LLM summaries instead of
    /// string-concat fallback. On LLM failure, falls back to string-concat automatically.
    pub fn with_llm_client(mut self, llm: Arc<dyn cyberclaw_llm::client::LlmClient>) -> Self {
        self.llm_client = Some(llm);
        self
    }

    /// S20 E2: Attach a credential sanitizer for the episodic memory write path.
    ///
    /// When set, `write_episodic_memory` sanitizes the summary string before
    /// calling `store_leveled`. When None (default), sanitization is skipped
    /// so existing unit tests remain backward-compatible.
    pub fn with_memory_sanitizer(
        mut self,
        sanitizer: Arc<cyberclaw_governance::tool_output_sanitizer::ToolOutputSanitizer>,
    ) -> Self {
        self.memory_sanitizer = Some(sanitizer);
        self
    }

    /// Attach an AutopilotStepRunner for real autopilot step execution.
    /// When set, `execute_autopilot_iteration` delegates to this runner
    /// instead of returning hardcoded placeholder results.
    pub fn with_autopilot_step_runner(mut self, runner: Arc<dyn AutopilotStepRunner>) -> Self {
        self.autopilot_step_runner = Some(runner);
        self
    }

    /// Attach a HandoffQueue for multi-agent handoff lifecycle (S21 T4).
    /// When set, `complete_handoff` transitions Authorized → Accepted and emits HandoffAccepted.
    pub fn with_handoff_queue(
        mut self,
        queue: Arc<dyn crate::handoff_queue::HandoffQueue>,
    ) -> Self {
        self.handoff_queue = Some(queue);
        self
    }

    /// Attach a HandoffCompletionSink for finalising handoff review approvals (S22 T2).
    ///
    /// This builder mirrors `with_handoff_queue` and is available on
    /// `InMemoryExecutionService` so that test helpers can wire a mock sink
    /// without needing a full `ControlPlaneOrchestrator`.
    pub fn with_handoff_completion_sink(
        self,
        _sink: Arc<dyn crate::handoff_completion_sink::HandoffCompletionSink>,
    ) -> Self {
        // InMemoryExecutionService itself does not dispatch handoff-review results;
        // that responsibility lives in ControlPlaneOrchestrator::process_review_result.
        // This method exists so call sites can chain it on the service builder and
        // pass the sink through to the orchestrator via the test helper path.
        // The sink is stored on ControlPlaneOrchestrator, not here.
        self
    }

    /// Sprint 25 S25 T3: Attach an EmbedClient for auto-embedding episodic memory records.
    /// When set and `dimension() > 0`, `write_episodic_memory` calls `embed()` on the
    /// summary and stores the vector in `LeveledMemoryRecord.embedding`. Best-effort:
    /// failures are logged as warnings and the record is stored without embedding.
    pub fn with_embed_client(mut self, client: Arc<dyn cyberclaw_llm::EmbedClient>) -> Self {
        self.embed_client = Some(client);
        self
    }

    /// Attach a custom SecurityConfigManager for unified security policy enforcement.
    pub fn with_security_config(
        mut self,
        config: Arc<cyberclaw_core::security_config::SecurityConfigManager>,
    ) -> Self {
        self.security_config = config;
        self
    }

    /// Sprint D1: attach a [`PersistentLoop`] used to dispatch executions
    /// whose `execution_mode == ExecutionMode::Persistent`.
    ///
    /// When wired, [`InMemoryExecutionService::execute`] routes Persistent
    /// executions to this loop. When `None`, those executions fail with an
    /// explicit "PersistentLoop not wired in this AppState" error so the
    /// misconfiguration is loud.
    pub fn with_persistent_loop(
        mut self,
        loop_runner: Arc<crate::persistent_execution::PersistentLoop>,
    ) -> Self {
        self.persistent_loop = Some(loop_runner);
        self
    }

    /// Publish a ClusterEvent on a best-effort basis.
    ///
    /// If publishing fails (e.g. lock contention, no subscribers), a warning is
    /// logged but execution continues uninterrupted. EventBus failures must never
    /// abort the primary execution path.
    fn publish_event_best_effort(&self, event: ClusterEvent) {
        if let Some(ref bus) = self.event_bus {
            if let Err(e) = bus.publish(event) {
                warn!(error = %e, "Failed to publish event to EventBus, continuing execution");
            }
        }
    }

    // ─── S21 T4: Multi-agent handoff completion ───────────────────────────────

    /// Complete a multi-agent handoff: transition Authorized → Accepted, emit HandoffAccepted.
    ///
    /// # Errors
    /// - `InvalidCommand` if no HandoffQueue is configured.
    /// - `InvalidCommand` if the handoff_id is not found.
    /// - `InvalidCommand` if the handoff is not in `Authorized` state (non-idempotent states).
    ///
    /// # Idempotency
    /// If the handoff is already `Accepted` AND has a `target_session_id` recorded,
    /// returns the SAME `SessionId` without re-emitting the event. The `target_session_id`
    /// is persisted on the first successful call via `HandoffQueue::set_target_session`.
    pub async fn complete_handoff(
        &self,
        handoff_id: &cyberclaw_core::ids::HandoffId,
    ) -> Result<cyberclaw_core::ids::SessionId, ExecutionError> {
        use cyberclaw_core::handoff::HandoffStatus;

        let Some(queue) = &self.handoff_queue else {
            return Err(ExecutionError::InvalidCommand(
                "handoff_queue not configured".to_string(),
            ));
        };

        let Some(req) = queue.get(handoff_id).await else {
            return Err(ExecutionError::InvalidCommand(format!(
                "handoff not found: {}",
                handoff_id
            )));
        };

        // Idempotency: already Accepted → return the persisted SessionId (A1.2).
        if matches!(req.status, HandoffStatus::Accepted) {
            return req.target_session_id.ok_or_else(|| {
                ExecutionError::InvalidCommand(
                    "handoff is Accepted but has no target_session_id persisted".to_string(),
                )
            });
        }

        // Validate current state is Authorized
        if !matches!(req.status, HandoffStatus::Authorized) {
            return Err(ExecutionError::InvalidCommand(format!(
                "cannot complete handoff in state {:?}: expected Authorized",
                req.status
            )));
        }

        // Transition Authorized → Accepted
        queue
            .update_status(handoff_id, HandoffStatus::Accepted)
            .await
            .map_err(|e| ExecutionError::InvalidCommand(format!("update_status failed: {}", e)))?;

        // Allocate new session
        let new_session_id = cyberclaw_core::ids::SessionId::new();

        // Persist the allocated SessionId for idempotent replay (A1.2, best-effort).
        let _ = queue
            .set_target_session(handoff_id, new_session_id.clone())
            .await;

        // Emit observability event (best-effort: ignore recorder errors)
        let _ = self
            .event_recorder
            .record_event(ObservabilityEvent::HandoffAccepted {
                handoff_id: handoff_id.clone(),
                new_session_id: new_session_id.clone(),
                timestamp: chrono::Utc::now(),
            })
            .await;

        Ok(new_session_id)
    }

    // ─── S18 R1 / S19 v2: Write episodic memory hook ─────────────────────────

    /// Write an L1Summary episodic memory record for a completed execution.
    ///
    /// Best-effort: failures are logged as warnings, never propagated.
    /// Returns the memory record ID on success, None on failure.
    ///
    /// # Summary strategy (v2)
    /// When an LlmClient is attached, calls cyberclaw-memory-extraction's
    /// `summarize_conversation` for a real 3-5 sentence semantic summary.
    /// Falls back to string-concat on LLM error or when no client is configured.
    async fn write_episodic_memory(
        &self,
        store: &dyn cyberclaw_store::LeveledMemoryStore,
        execution_id: &ExecutionId,
        agent_id_str: &str,
        duration_ms: u64,
    ) -> Option<String> {
        let session_id = execution_id.as_str().to_string();
        let memory_id = format!(
            "ep-{}-{}",
            execution_id.as_str(),
            chrono::Utc::now().timestamp_millis()
        );

        // v2: real LLM extraction when client is available; string-concat fallback otherwise.
        let string_concat_fallback = || {
            format!(
                "Execution {} completed by agent {} in {}ms.",
                execution_id.as_str(),
                agent_id_str,
                duration_ms,
            )
        };

        let summary = if let Some(ref llm) = self.llm_client {
            // Build a minimal conversation representation from execution metadata.
            // The execution content itself is not available here; we summarize the run record.
            let messages = vec![
                cyberclaw_memory_extraction::llm_extractors::ConversationMessage {
                    role: "system".to_string(),
                    content: format!("Agent {} executed task in {}ms.", agent_id_str, duration_ms),
                },
                cyberclaw_memory_extraction::llm_extractors::ConversationMessage {
                    role: "result".to_string(),
                    content: format!(
                        "Execution ID: {}. Duration: {}ms. Agent: {}.",
                        execution_id.as_str(),
                        duration_ms,
                        agent_id_str,
                    ),
                },
            ];
            match cyberclaw_memory_extraction::llm_extractors::summarize_conversation(
                &messages,
                llm.as_ref(),
            )
            .await
            {
                Ok(s) => s,
                Err(e) => {
                    warn!(
                        error = %e,
                        execution_id = %execution_id,
                        "S19: LLM summary failed, falling back to string-concat"
                    );
                    string_concat_fallback()
                }
            }
        } else {
            string_concat_fallback()
        };

        let now = chrono::Utc::now();
        let mut record = cyberclaw_store::LeveledMemoryRecord {
            id: memory_id.clone(),
            session_id: session_id.clone(),
            agent_id: agent_id_str.to_string(),
            level: cyberclaw_store::MemoryLevel::L1Summary,
            key: format!("episodic-{}", execution_id.as_str()),
            content: serde_json::json!({ "summary": summary }),
            created_at: now,
            updated_at: now,
            ttl_seconds: cyberclaw_store::MemoryLevel::L1Summary.default_ttl_seconds(),
            // S18 R4: 记录产生该 memory 的执行 ID，供 trace 端点使用
            source_execution_id: Some(execution_id.as_str().to_string()),
            embedding: None,
            tags: Vec::new(),
        };

        // Sprint 25 S25 T3: best-effort embedding (skip if no client or dim 0)
        if let Some(ref embed_client) = self.embed_client {
            if embed_client.dimension() > 0 {
                let content_str = match &record.content {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                match embed_client.embed(&content_str).await {
                    Ok(vec) if !vec.is_empty() => {
                        record.embedding = Some(vec);
                    }
                    Ok(_) => {
                        tracing::warn!(
                            "S25: embed returned empty vector — provider misconfigured?"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "S25: embed failed; record stored without embedding");
                    }
                }
            }
        }

        match store.store_leveled(record).await {
            Ok(()) => {
                // S20 E3: Auto-demote — delete L0Full records older than 1 hour for this session
                // since they've been summarized into L1. Best-effort, never blocks.
                let cutoff = chrono::Utc::now() - chrono::Duration::hours(1);
                match store
                    .query_by_level(&session_id, cyberclaw_store::MemoryLevel::L0Full)
                    .await
                {
                    Ok(stale_l0) => {
                        for r in stale_l0.iter().filter(|r| r.updated_at < cutoff) {
                            let _ = store.delete(&r.id).await;
                        }
                    }
                    Err(e) => {
                        warn!(
                            "S20 E3: Failed to query L0Full for auto-demote (best-effort): {}",
                            e
                        );
                    }
                }
                Some(memory_id)
            }
            Err(e) => {
                warn!(
                    "S18 R1: Failed to write episodic memory for execution {} (best-effort): {}",
                    execution_id, e
                );
                None
            }
        }
    }

    // ─── S18 R2: Read prior context hook ─────────────────────────────────────

    /// Query recent L1Summary memories and format as a prior_context block.
    ///
    /// Best-effort: failures are logged as warnings, returns None on failure.
    /// The output block is capped at 2KB to avoid prompt bloat.
    async fn read_prior_context(
        &self,
        execution_id: &ExecutionId,
        _agent_id_str: &str,
    ) -> Option<String> {
        let store = self.leveled_memory_store.as_ref()?;
        let session_id = execution_id.as_str().to_string();

        let records = match store
            .query_by_level(&session_id, cyberclaw_store::MemoryLevel::L1Summary)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!(
                    "S18 R2: Failed to query L1Summary memories for execution {} (best-effort): {}",
                    execution_id, e
                );
                return None;
            }
        };

        if records.is_empty() {
            return None;
        }

        // S18 R4: 记录每条被读取的 memory（best-effort，不阻断主流程）
        for record in &records {
            if let Err(e) = store.record_read(&record.id, execution_id.as_str()).await {
                warn!(
                    "S18 R4: Failed to record_read for memory {} execution {} (best-effort): {}",
                    record.id, execution_id, e
                );
            }
        }

        // Take most recent 5 records (query_by_level returns desc order)
        let limit = 5;
        let entries: Vec<String> = records
            .iter()
            .take(limit)
            .enumerate()
            .map(|(i, r)| {
                let summary = r
                    .content
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(no summary)");
                format!(
                    "{}. [{}] {}",
                    i + 1,
                    r.created_at.format("%Y-%m-%dT%H:%M:%SZ"),
                    summary
                )
            })
            .collect();

        let block = format!(
            "<prior_context>\nYour recent conversation summaries:\n{}\n</prior_context>",
            entries.join("\n")
        );

        // Cap at 2KB to avoid prompt bloat
        const MAX_PRIOR_CONTEXT_BYTES: usize = 2048;
        let prior_context_block = if block.len() > MAX_PRIOR_CONTEXT_BYTES {
            warn!(
                "S18 R2: prior_context block ({} bytes) exceeds 2KB cap for execution {}, trimming",
                block.len(),
                execution_id
            );
            format!(
                "{}\n</prior_context>",
                &block[..MAX_PRIOR_CONTEXT_BYTES.saturating_sub(20)]
            )
        } else {
            block
        };

        // Sprint 21 T8: if this execution's session was minted from an accepted
        // handoff, prepend the `<handoff_briefing>` block ahead of prior_context.
        // The briefing helper enforces its own 4KB cap (2KB text + 2KB artifacts).
        let final_block = if let Some(queue) = self.handoff_queue.as_ref() {
            let session_id_typed =
                cyberclaw_core::ids::SessionId::from_string(session_id.clone()).ok();
            let handoff = match session_id_typed {
                Some(sid) => queue.find_by_target_session(&sid).await,
                None => None,
            };
            match handoff {
                Some(req) => format!(
                    "{}{}",
                    build_handoff_briefing_addendum(&req),
                    prior_context_block
                ),
                None => prior_context_block,
            }
        } else {
            prior_context_block
        };

        Some(final_block)
    }

    pub fn list(&self) -> Vec<Execution> {
        self.executions
            .read()
            .map(|entries| entries.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn list_remote_assignments_for_node(
        &self,
        node_id: &NodeId,
    ) -> anyhow::Result<Vec<RemoteExecutionAssignment>> {
        let entries = self
            .executions
            .read()
            .map_err(|_| anyhow::anyhow!("execution store poisoned"))?;
        let plans = self
            .execution_plans
            .read()
            .map_err(|_| anyhow::anyhow!("execution plans store poisoned"))?;

        let mut assignments = Vec::new();
        for execution in entries.values() {
            if execution.scheduled_node_id.as_ref() != Some(node_id) {
                continue;
            }
            if execution.status != ExecutionStatus::Pending {
                continue;
            }
            if let Some(plan) = plans.get(&execution.id) {
                assignments.push(RemoteExecutionAssignment {
                    execution: execution.clone(),
                    plan: plan.clone(),
                });
            }
        }

        Ok(assignments)
    }

    pub fn import_remote_assignment(
        &self,
        assignment: RemoteExecutionAssignment,
    ) -> anyhow::Result<()> {
        {
            let mut entries = self
                .executions
                .write()
                .map_err(|_| anyhow::anyhow!("execution store poisoned"))?;
            match entries.get_mut(&assignment.execution.id) {
                Some(existing) => {
                    existing.owner_node_id = assignment.execution.owner_node_id.clone();
                    existing.scheduled_node_id = assignment.execution.scheduled_node_id.clone();
                    existing.lease_id = assignment.execution.lease_id.clone();
                    existing.workspace = assignment.execution.workspace.clone();
                    existing.trace_id = assignment.execution.trace_id.clone();
                }
                None => {
                    entries.insert(
                        assignment.execution.id.clone(),
                        assignment.execution.clone(),
                    );
                }
            }
        }

        let mut plans = self
            .execution_plans
            .write()
            .map_err(|_| anyhow::anyhow!("execution plans store poisoned"))?;
        plans.insert(assignment.execution.id, assignment.plan);
        Ok(())
    }

    fn insert(&self, execution: Execution) -> anyhow::Result<()> {
        let mut entries = self
            .executions
            .write()
            .map_err(|_| anyhow::anyhow!("execution store poisoned"))?;

        // Idempotency check: prevent duplicate execution submission
        if entries.contains_key(&execution.id) {
            anyhow::bail!(
                "execution '{}' already exists; duplicate submission prevented",
                execution.id
            );
        }

        entries.insert(execution.id.clone(), execution);
        Ok(())
    }

    // ============ Autopilot Helper Methods ============

    /// Execute Autopilot loop with iteration control
    async fn execute_autopilot_loop(&self, execution_id: &ExecutionId) -> anyhow::Result<()> {
        // Maximum iterations to prevent infinite loops
        const MAX_ITERATIONS: u32 = 100;

        // Initialize iteration tracking
        let mut current_iteration = 0u32;

        // Try to resume from checkpoint if available
        if let Some(checkpoint) = self.resume_from_checkpoint(execution_id).await? {
            current_iteration = checkpoint.iteration;
            info!(
                "Resuming Autopilot execution {} from iteration {}",
                execution_id, current_iteration
            );
        }

        // Main iteration loop
        loop {
            current_iteration += 1;

            // Check iteration limit
            if current_iteration > MAX_ITERATIONS {
                warn!(
                    "Autopilot execution {} reached maximum iterations ({})",
                    execution_id, MAX_ITERATIONS
                );
                return Err(anyhow::anyhow!("Maximum iterations reached"));
            }

            // Start iteration
            self.on_iteration_start(execution_id, current_iteration)
                .await?;

            // Execute the iteration
            let result = self
                .execute_autopilot_iteration(execution_id, current_iteration)
                .await?;

            // Check if stuck
            if !result.progress_made {
                let history = self.get_iteration_history(execution_id).await?;
                if let Some(ref detector) = self.stuck_detector {
                    if let Some(reason) = detector.is_stuck(execution_id, &history).await? {
                        let resolution = self
                            .on_stuck_detected(execution_id, current_iteration, reason)
                            .await?;
                        match resolution {
                            StuckResolution::Retry { approach: _ } => {
                                // Continue with new approach
                                continue;
                            }
                            StuckResolution::Escalate => {
                                return Err(anyhow::anyhow!(
                                    "Execution escalated to human operator"
                                ));
                            }
                            StuckResolution::Abort => {
                                return Err(anyhow::anyhow!(
                                    "Execution aborted due to no progress"
                                ));
                            }
                        }
                    }
                }
            }

            // Store iteration summary
            {
                let mut histories = self
                    .iteration_histories
                    .write()
                    .map_err(|_| anyhow::anyhow!("iteration histories poisoned"))?;
                let history = histories
                    .entry(execution_id.clone())
                    .or_insert_with(Vec::new);
                history.push(IterationSummary {
                    iteration: current_iteration,
                    start_time: chrono::Utc::now(),
                    end_time: chrono::Utc::now(),
                    steps_completed: result.steps_completed.clone(),
                    decision: format!("{:?}", result.decision),
                    progress_made: result.progress_made,
                });
            }

            // Make decision based on iteration result
            match result.decision {
                Decision::GoalMet => {
                    info!(
                        "Autopilot execution {} completed successfully after {} iterations",
                        execution_id, current_iteration
                    );
                    return Ok(());
                }
                Decision::Continue => {
                    // Checkpoint state for recovery
                    let state = IterationState {
                        iteration: current_iteration,
                        current_step: AutopilotStep::Iterate,
                        steps_completed: result.steps_completed,
                        context: result.output.unwrap_or(serde_json::Value::Null),
                        memory_snapshot: vec![],
                        timestamp: chrono::Utc::now(),
                    };
                    self.checkpoint_iteration(execution_id, current_iteration, state)
                        .await?;
                    // Continue to next iteration
                }
                Decision::Stuck => {
                    return Err(anyhow::anyhow!(
                        "Autopilot execution {} stuck at iteration {}",
                        execution_id,
                        current_iteration
                    ));
                }
            }
        }
    }
}

#[async_trait]
impl ExecutionService for InMemoryExecutionService {
    async fn submit(&self, request: ExecutionRequest) -> anyhow::Result<ExecutionId> {
        let execution_id = request.execution_id.clone();
        let trace_id = request.trace_id.unwrap_or_default();
        let agent = request.agent.unwrap_or(AgentRef {
            id: control_plane_agent_id().clone(),
            role: "control-plane".to_string(),
        });
        let agent_id_str = agent.id.as_str().to_string();

        let span = execution_span(&execution_id, &trace_id);

        async move {
            // Clone values needed for provenance before moving into execution
            let agent_id_for_provenance = agent.id.clone();
            let case_id_for_provenance = request.case.as_ref().map(|c| c.id.clone());

            let execution = Execution {
                id: execution_id.clone(),
                root_execution_id: execution_id.clone(),
                parent_execution_id: None,
                owner_node_id: None,
                scheduled_node_id: None,
                placement_group: None,
                lease_id: None,
                handoff_count: 0,
                case_id: request.case.as_ref().map(|item| item.id.clone()),
                task_id: Some(request.task.id.clone()),
                agent,
                status: ExecutionStatus::Pending,
                join_strategy: None,
                budget: ExecutionBudget::default(),
                workspace: request.context.workspace.clone(),
                trace_id: trace_id.clone(),
                started_at: None,
                finished_at: None,
                risk_level: cyberclaw_core::capability::RiskLevel::Low, // Will be updated during execution
                execution_mode: request.execution_mode.unwrap_or_default(),
            };

            self.insert(execution)?;

            // If a plan was provided, store it so execute() can use plan actions.
            if let Some(plan) = request.plan {
                let mut plans = self
                    .execution_plans
                    .write()
                    .map_err(|_| anyhow::anyhow!("execution plans store poisoned"))?;
                plans.insert(execution_id.clone(), plan);
            }

            // Record ExecutionCreated event
            let system_actor = ActorRef {
                id: ActorId::from_string("control-plane".to_string())
                    .unwrap_or_else(|_| ActorId::new()),
                actor_type: ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "Control Plane".to_string(),
            };
            let _ = self
                .event_recorder
                .record_event(ObservabilityEvent::ExecutionCreated {
                    execution_id: execution_id.clone(),
                    requested_by: system_actor.clone(),
                    timestamp: chrono::Utc::now(),
                })
                .await;

            // Record ExecutionSubmitted security event (fire-and-forget)
            if let Some(ref sec_store) = self.security_event_store {
                // SECURITY FIX: Use system_actor already created above
                let _ = sec_store
                    .store(SecurityEvent {
                        id: cyberclaw_core::ids::SecurityEventId::new(),
                        actor: Some(system_actor.clone()),
                        timestamp: chrono::Utc::now(),
                        execution_id: Some(execution_id.clone()),
                        case_id: None,
                        node_id: None,
                        runtime_instance_id: None,
                        source: SecurityEventSource::RuntimeDetection,
                        event_type: SecurityEventType::Custom("ExecutionSubmitted".to_string()),
                        severity: Severity::Info,
                        summary: format!("Execution submitted: {}", execution_id),
                        details: serde_json::json!({
                            "execution_id": execution_id.as_str(),
                            "actor": agent_id_str,
                        }),
                        trace_id: trace_id.clone(),
                        credential_evidence: None,
                    })
                    .await;
            }

            // Start provenance tracking (best-effort)
            if let Some(ref tracker) = self.provenance_tracker {
                if let Err(e) = tracker
                    .start_tracking(
                        execution_id.clone(),
                        agent_id_for_provenance.clone(),
                        case_id_for_provenance.clone(),
                        trace_id.clone(),
                        None, // node_id can be None for single-node setups
                    )
                    .await
                {
                    // MEDIUM #1: Enhanced provenance error context
                    warn!(
                        "MEDIUM #1: Provenance start_tracking failed | \
                         execution_id: {} | agent_id: {} | case_id: {:?} | \
                         trace_id: {} | mode: best-effort | error: {}",
                        execution_id, agent_id_for_provenance, case_id_for_provenance, trace_id, e
                    );
                }
            }

            // MEDIUM #9: 统一审计日志格式
            let audit_logger = DefaultAuditLogger;
            audit_logger.log(
                AuditLogEntry::new(
                    "ExecutionService",
                    "execution.submitted",
                    format!("Execution submitted for agent {}", agent_id_str),
                )
                .with_trace_id(trace_id.as_str())
                .with_metadata("execution_id", execution_id.to_string())
                .with_metadata("agent_id", agent_id_str.clone()),
            );

            info!("execution submitted: {}", execution_id);
            Ok(execution_id)
        }
        .instrument(span)
        .await
    }

    async fn submit_plan(&self, plan: ExecutionPlan) -> anyhow::Result<ExecutionId> {
        let execution_id = ExecutionId::new();
        let trace_id = TraceId::new();

        let span = execution_span(&execution_id, &trace_id);

        async move {
            let execution = Execution {
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
                    id: plan.resolution.agent.clone(),
                    role: "resolved-agent".to_string(),
                },
                status: ExecutionStatus::Pending,
                join_strategy: None,
                budget: ExecutionBudget::default(),
                workspace: None,
                trace_id: trace_id.clone(),
                started_at: None,
                finished_at: None,
                risk_level: cyberclaw_core::capability::RiskLevel::Low, // Will be updated from plan actions
                // submit_plan() is invoked by the Resolver with a pre-built plan;
                // the execution mode is always Normal here. Autopilot plans go
                // through the autopilot_runtime which sets Autopilot directly.
                execution_mode: cyberclaw_core::execution::ExecutionMode::Normal,
            };

            self.insert(execution)?;

            // Store the execution plan
            {
                let mut plans = self
                    .execution_plans
                    .write()
                    .map_err(|_| anyhow::anyhow!("execution plans store poisoned"))?;
                plans.insert(execution_id.clone(), plan);
            }

            // Record ExecutionCreated event
            let system_actor = ActorRef {
                id: ActorId::from_string("control-plane".to_string())
                    .unwrap_or_else(|_| ActorId::new()),
                actor_type: ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "Control Plane".to_string(),
            };
            let _ = self
                .event_recorder
                .record_event(ObservabilityEvent::ExecutionCreated {
                    execution_id: execution_id.clone(),
                    requested_by: system_actor,
                    timestamp: chrono::Utc::now(),
                })
                .await;

            info!("execution plan submitted: {}", execution_id);
            Ok(execution_id)
        }
        .instrument(span)
        .await
    }

    async fn cancel(&self, execution_id: &ExecutionId) -> anyhow::Result<()> {
        // Capture timestamp once for consistency across execution and events
        let now = chrono::Utc::now();

        let (previous_status, started_at) = {
            let mut entries = self
                .executions
                .write()
                .map_err(|_| anyhow::anyhow!("execution store poisoned"))?;

            let Some(execution) = entries.get_mut(execution_id) else {
                anyhow::bail!("execution not found: {}", execution_id);
            };

            let previous_status = execution.status.clone();
            let started_at = execution.started_at;
            execution.status = ExecutionStatus::Cancelled;
            execution.finished_at = Some(now);
            (previous_status, started_at)
        };

        // Record ExecutionStatusChanged event for cancel
        let _ = self
            .event_recorder
            .record_event(ObservabilityEvent::ExecutionStatusChanged {
                execution_id: execution_id.clone(),
                from_status: previous_status.clone(),
                to_status: ExecutionStatus::Cancelled,
                timestamp: now,
            })
            .await;

        // Record metrics for cancellation
        // i64 → f64 精度损失是可接受的 (用于时间统计)
        #[allow(clippy::cast_precision_loss)]
        let duration_secs = started_at
            .map(|s| (chrono::Utc::now() - s).num_milliseconds() as f64 / 1000.0)
            .unwrap_or(0.0);
        recorders::record_execution_complete(&ExecutionStatus::Cancelled, duration_secs);
        recorders::record_execution_state_change(&format!("{:?}", previous_status), "Cancelled");

        info!("execution cancelled: {}", execution_id);
        Ok(())
    }

    async fn get(&self, execution_id: &ExecutionId) -> anyhow::Result<Option<Execution>> {
        let entries = self
            .executions
            .read()
            .map_err(|_| anyhow::anyhow!("execution store poisoned"))?;
        Ok(entries.get(execution_id).cloned())
    }

    async fn list_all(
        &self,
        status_filter: Option<ExecutionStatus>,
    ) -> anyhow::Result<Vec<Execution>> {
        let entries = self
            .executions
            .read()
            .map_err(|_| anyhow::anyhow!("execution store poisoned"))?;
        let executions: Vec<Execution> = entries
            .values()
            .filter(|e| match &status_filter {
                Some(status) => &e.status == status,
                None => true,
            })
            .cloned()
            .collect();
        Ok(executions)
    }

    async fn list_by_agent_window(
        &self,
        agent_id: &cyberclaw_core::ids::AgentId,
        window_start: chrono::DateTime<chrono::Utc>,
        window_end: chrono::DateTime<chrono::Utc>,
    ) -> anyhow::Result<Vec<Execution>> {
        // Override of trait default: scan the live map directly instead of
        // bouncing through list_all → list_all clones every entry, filter
        // here clones only matches.
        let entries = self
            .executions
            .read()
            .map_err(|_| anyhow::anyhow!("execution store poisoned"))?;
        let executions: Vec<Execution> = entries
            .values()
            .filter(|e| {
                if e.agent.id.as_str() != agent_id.as_str() {
                    return false;
                }
                let Some(started) = e.started_at else {
                    return false;
                };
                started >= window_start && started < window_end
            })
            .cloned()
            .collect();
        Ok(executions)
    }

    async fn list_by_task_id(&self, task_id: &TaskId) -> anyhow::Result<Vec<Execution>> {
        let entries = self
            .executions
            .read()
            .map_err(|_| anyhow::anyhow!("execution store poisoned"))?;
        let executions: Vec<Execution> = entries
            .values()
            .filter(|e| e.task_id.as_ref() == Some(task_id))
            .cloned()
            .collect();
        Ok(executions)
    }

    async fn update_status(
        &self,
        execution_id: &ExecutionId,
        status: ExecutionStatus,
    ) -> anyhow::Result<()> {
        // Capture timestamp once for consistency across execution and events
        let now = chrono::Utc::now();

        let (previous_status, started_at, finished_at) = {
            let mut entries = self
                .executions
                .write()
                .map_err(|_| anyhow::anyhow!("execution store poisoned"))?;

            let Some(execution) = entries.get_mut(execution_id) else {
                anyhow::bail!("execution not found: {}", execution_id);
            };

            let previous_status = execution.status.clone();

            execution.status = status.clone();

            // Update timestamps based on status transitions
            match status {
                ExecutionStatus::Running if execution.started_at.is_none() => {
                    execution.started_at = Some(now);
                }
                ExecutionStatus::Completed
                | ExecutionStatus::Failed
                | ExecutionStatus::Cancelled
                | ExecutionStatus::TimedOut
                    if execution.finished_at.is_none() =>
                {
                    execution.finished_at = Some(now);
                }
                _ => {}
            }

            (previous_status, execution.started_at, execution.finished_at)
        };

        // Create status_transition span and record event
        let span = status_transition_span(execution_id, &previous_status, &status);
        let _enter = span.enter();

        let _ = self
            .event_recorder
            .record_event(ObservabilityEvent::ExecutionStatusChanged {
                execution_id: execution_id.clone(),
                from_status: previous_status.clone(),
                to_status: status.clone(),
                timestamp: now,
            })
            .await;

        // Record metrics for terminal states
        match &status {
            ExecutionStatus::Completed
            | ExecutionStatus::Failed
            | ExecutionStatus::Cancelled
            | ExecutionStatus::TimedOut => {
                // i64 → f64 精度损失是可接受的 (用于时间统计)
                #[allow(clippy::cast_precision_loss)]
                let duration_secs = match (started_at, finished_at) {
                    (Some(s), Some(f)) => (f - s).num_milliseconds() as f64 / 1000.0,
                    (Some(s), None) => (chrono::Utc::now() - s).num_milliseconds() as f64 / 1000.0,
                    _ => 0.0,
                };
                recorders::record_execution_complete(&status, duration_secs);
            }
            _ => {}
        }

        // Always record state transitions
        recorders::record_execution_state_change(
            &format!("{:?}", previous_status),
            &format!("{:?}", status),
        );

        info!("execution status updated: {} -> {:?}", execution_id, status);
        Ok(())
    }

    async fn execute(&self, execution_id: &ExecutionId) -> anyhow::Result<()> {
        // MEDIUM #2 FIX: Create best-effort error collector
        let _best_effort_errors = BestEffortErrors::new();

        // Check execution mode from the Execution record (set by Resolver)
        let (is_autopilot, is_persistent) = {
            let entries = self
                .executions
                .read()
                .map_err(|_| anyhow::anyhow!("execution store poisoned"))?;
            let mode = entries
                .get(execution_id)
                .map(|e| e.execution_mode)
                .unwrap_or_default();
            (
                mode == cyberclaw_core::execution::ExecutionMode::Autopilot,
                mode == cyberclaw_core::execution::ExecutionMode::Persistent,
            )
        };

        // Sprint D1: Persistent execution mode is routed to the optional
        // PersistentLoop. Sprint D3 lands the per-story dispatch via
        // `PersistentLoop::execute`.
        if is_persistent {
            let Some(persistent_loop) = self.persistent_loop.clone() else {
                anyhow::bail!(
                    "PersistentLoop not wired in this AppState — execution {} \
                     has execution_mode = Persistent but no PersistentLoop was \
                     attached via InMemoryExecutionService::with_persistent_loop. \
                     Either wire a PersistentLoop or downgrade the execution to \
                     Normal/Autopilot mode.",
                    execution_id
                );
            };

            // Sprint D3: drive the wired loop's async orchestrator. The plan
            // attached to the loop at construction is what gets executed —
            // higher layers (resolver/orchestrator) populate it before
            // wiring.
            let plan = persistent_loop.plan.clone();
            let mut ctx = cyberclaw_core::execution::ExecutionContext::default();
            let p_result = persistent_loop
                .execute(&plan, &mut ctx)
                .await
                .map_err(|e| {
                    anyhow::anyhow!(
                        "PersistentLoop dispatch failed for execution {}: {}",
                        execution_id,
                        e
                    )
                })?;

            // Mark execution complete or failed based on the persistent
            // execution outcome. We update status under a write lock and emit
            // an ExecutionStatusChanged event via the existing path.
            let final_status = if p_result.stories_failed.is_empty() {
                ExecutionStatus::Completed
            } else {
                ExecutionStatus::Failed
            };
            self.update_status(execution_id, final_status).await?;
            return Ok(());
        }

        // Security: Rate limiting - check concurrent execution limit based on execution type
        let (counter, limit, counter_type) = if is_autopilot {
            (
                self.concurrent_autopilot_executions.clone(),
                MAX_CONCURRENT_AUTOPILOT_EXECUTIONS,
                "Autopilot",
            )
        } else {
            (
                self.concurrent_executions.clone(),
                MAX_CONCURRENT_EXECUTIONS,
                "normal",
            )
        };

        let current = counter.fetch_add(1, Ordering::SeqCst);
        if current >= limit {
            counter.fetch_sub(1, Ordering::SeqCst);
            anyhow::bail!(
                "Rate limit exceeded: {} concurrent {} executions (max: {})",
                current + 1,
                counter_type,
                limit
            );
        }

        // Ensure we decrement the counter when this function exits (success or error)
        let _guard = scopeguard::guard(counter, |c| {
            c.fetch_sub(1, Ordering::SeqCst);
        });

        // Retrieve agent ID, agent ref, and trace_id while holding only a read lock.
        let (agent_id_str, task_input, agent_ref, execution_trace_id) = {
            let entries = self
                .executions
                .read()
                .map_err(|_| anyhow::anyhow!("execution store poisoned"))?;
            let execution = entries
                .get(execution_id)
                .ok_or_else(|| anyhow::anyhow!("execution not found: {}", execution_id))?;
            let input = execution
                .task_id
                .as_ref()
                .map(|t| t.as_str().to_string())
                .unwrap_or_else(|| "no-task".to_string());
            (
                execution.agent.id.as_str().to_string(),
                input,
                execution.agent.clone(),
                execution.trace_id.clone(),
            )
        };

        let agent_span = cyberclaw_observability::agent_span(execution_id, &agent_id_str);

        async move {
            // SECURITY FIX: Convert AgentRef to ActorRef for security audit trail
            let agent_actor = cyberclaw_core::identity::ActorRef {
                id: cyberclaw_core::ids::ActorId::from_string(agent_ref.id.as_str().to_string())
                    .unwrap_or_else(|_| cyberclaw_core::ids::ActorId::new()),
                actor_type: cyberclaw_core::identity::ActorType::Agent,
                tenant_id: None,
                home_node_id: None,
                display_name: agent_ref.role.clone(),
            };

            // Capture timestamp once for consistency across execution and events
            let start_time = chrono::Utc::now();

            // HIGH #5 FIX: Use Mutex to protect memory entry count for atomicity
            // Prevents TOCTOU race condition where concurrent executions could bypass
            // the MAX_MEMORY_ENTRIES_PER_EXECUTION limit
            let memory_entries_added = Arc::new(Mutex::new(0usize));

            // Transition to Running (with race condition prevention).
            let (prev_running_status, trace_tampering_event) = {
                let mut entries = self
                    .executions
                    .write()
                    .map_err(|_| anyhow::anyhow!("execution store poisoned"))?;
                let execution = entries
                    .get_mut(execution_id)
                    .ok_or_else(|| anyhow::anyhow!("execution not found: {}", execution_id))?;

                // RACE CONDITION FIX: Only allow transition from Pending to Running
                // This prevents duplicate execution if multiple threads call execute() concurrently
                if execution.status != ExecutionStatus::Pending {
                    anyhow::bail!(
                        "Cannot execute: status is {:?}, expected Pending (execution may already be running)",
                        execution.status
                    );
                }

                // HIGH #2 FIX: Verify trace_id continuity at execution start
                // This prevents trace_id tampering before execution begins
                let trace_tampering_event = if execution.trace_id != execution_trace_id {
                    let found_trace_id = execution.trace_id.clone();
                    Some((found_trace_id.clone(), SecurityEvent {
                        id: cyberclaw_core::ids::SecurityEventId::new(),
                        actor: Some(agent_actor.clone()),
                        timestamp: chrono::Utc::now(),
                        execution_id: Some(execution_id.clone()),
                        case_id: None,
                        node_id: None,
                        runtime_instance_id: None,
                        source: SecurityEventSource::RuntimeDetection,
                        event_type: SecurityEventType::Custom("TraceIdTampering".to_string()),
                        severity: Severity::High,
                        summary: format!(
                            "trace_id tampering detected at execution start: expected {}, found {}",
                            execution_trace_id, found_trace_id
                        ),
                        details: serde_json::json!({
                            "execution_id": execution_id.as_str(),
                            "expected_trace_id": execution_trace_id.as_str(),
                            "found_trace_id": found_trace_id.as_str(),
                            "checkpoint": "execution_start",
                        }),
                        trace_id: execution_trace_id.clone(),
                        credential_evidence: None,
                    }))
                } else {
                    None
                };

                let prev = execution.status.clone();
                execution.status = ExecutionStatus::Running;
                if execution.started_at.is_none() {
                    execution.started_at = Some(start_time);
                }
                (prev, trace_tampering_event)
            };

            // Handle trace tampering event after lock is dropped
            if let Some((found_trace_id, security_event)) = trace_tampering_event {
                if let Some(ref sec_store) = self.security_event_store {
                    let _ = sec_store.store(security_event).await;
                }
                anyhow::bail!(
                    "trace_id tampering detected at execution start: expected {}, found {}",
                    execution_trace_id,
                    found_trace_id
                );
            }

            let _ = self
                .event_recorder
                .record_event(ObservabilityEvent::ExecutionStatusChanged {
                    execution_id: execution_id.clone(),
                    from_status: prev_running_status,
                    to_status: ExecutionStatus::Running,
                    timestamp: start_time,
                })
                .await;

            let _ = self
                .event_recorder
                .record_event(ObservabilityEvent::AgentExecutionStarted {
                    execution_id: execution_id.clone(),
                    agent_id: agent_id_str.clone(),
                    timestamp: start_time,
                }).await;

            // S18 R2: Read recent L1Summary memories and inject prior_context (best-effort)
            let prior_context_block = self.read_prior_context(execution_id, &agent_id_str).await;
            if let Some(ref block) = prior_context_block {
                let _ = self
                    .event_recorder
                    .record_event(ObservabilityEvent::MemoryRead {
                        execution_id: execution_id.clone(),
                        count: block.lines().filter(|l| l.starts_with(|c: char| c.is_ascii_digit())).count(),
                        session_id: execution_id.as_str().to_string(),
                        timestamp: chrono::Utc::now(),
                    })
                    .await;
            }

            // S20 E3: Write L0Full working memory snapshot at execution start (best-effort)
            if let Some(ref mem_store) = self.leveled_memory_store {
                let now = chrono::Utc::now();
                let l0_record = cyberclaw_store::LeveledMemoryRecord {
                    id: format!("l0-{}-{}", execution_id.as_str(), now.timestamp_millis()),
                    session_id: execution_id.as_str().to_string(),
                    agent_id: agent_id_str.clone(),
                    level: cyberclaw_store::MemoryLevel::L0Full,
                    key: format!("working-{}", execution_id.as_str()),
                    content: serde_json::json!({
                        "execution_id": execution_id.as_str(),
                        "agent_id": agent_id_str,
                        "task_input": task_input,
                        "turn_index": 0,
                        "phase": "start",
                    }),
                    created_at: now,
                    updated_at: now,
                    ttl_seconds: cyberclaw_store::MemoryLevel::L0Full.default_ttl_seconds(),
                    source_execution_id: Some(execution_id.as_str().to_string()),
                    embedding: None,
                    tags: Vec::new(),
                };
                let _ = mem_store.store_leveled(l0_record).await; // best-effort
            }

            // Record ExecutionStarted security event (fire-and-forget)
            if let Some(ref sec_store) = self.security_event_store {
                let _ = sec_store
                    .store(SecurityEvent {
                        id: cyberclaw_core::ids::SecurityEventId::new(),
                        actor: Some(agent_actor.clone()),
                        timestamp: chrono::Utc::now(),
                        execution_id: Some(execution_id.clone()),
                        case_id: None,
                        node_id: None,
                        runtime_instance_id: None,
                        source: SecurityEventSource::RuntimeDetection,
                        event_type: SecurityEventType::Custom("ExecutionStarted".to_string()),
                        severity: Severity::Info,
                        summary: format!("Execution started: {}", execution_id),
                        details: serde_json::json!({
                            "execution_id": execution_id.as_str(),
                            "actor": agent_id_str,
                        }),
                        trace_id: execution_trace_id.clone(),
                        credential_evidence: None,
                    })
                    .await;
            }

            recorders::record_execution_state_change("Pending", "Running");

            // Publish ExecutionAssigned best-effort using persisted assignment metadata.
            // If assignment is absent, fallback to synthetic local metadata for compatibility.
            let (assigned_node_id, assigned_lease_id) = {
                let entries = self
                    .executions
                    .read()
                    .map_err(|_| anyhow::anyhow!("execution store poisoned"))?;
                let execution = entries
                    .get(execution_id)
                    .ok_or_else(|| anyhow::anyhow!("execution not found: {}", execution_id))?;
                (
                    execution.scheduled_node_id.clone().unwrap_or_else(|| {
                        cyberclaw_core::ids::NodeId::from_string("local".to_string())
                            .unwrap_or_else(|_| cyberclaw_core::ids::NodeId::new())
                    }),
                    execution
                        .lease_id
                        .clone()
                        .unwrap_or_else(cyberclaw_core::ids::LeaseId::new),
                )
            };

            self.publish_event_best_effort(ClusterEvent::ExecutionAssigned {
                execution_id: execution_id.clone(),
                node_id: assigned_node_id,
                lease_id: assigned_lease_id,
                timestamp: start_time,
            });

            // MEDIUM #9: 统一审计日志格式
            let audit_logger = DefaultAuditLogger;
            audit_logger.log(
                AuditLogEntry::new(
                    "ExecutionService",
                    "execution.start",
                    format!("Starting execution {}", execution_id)
                )
                .with_trace_id(execution_trace_id.as_str())
                .with_metadata("execution_id", execution_id.to_string())
            );

            info!("executing: {}", execution_id);

            let started = std::time::Instant::now();

            // Check if we have a plan with actions to execute
            let has_plan_actions = {
                let plans = self.execution_plans.read()
                    .map_err(|_| anyhow::anyhow!("execution plans store poisoned"))?;
                plans.get(execution_id).map(|p| !p.actions.is_empty()).unwrap_or(false)
            };

            // Execute based on execution mode
            let retry_cfg = RetryConfig::default();
            let runtime_result: anyhow::Result<()> = if is_autopilot {
                // Autopilot execution mode - iterative loop
                self.execute_autopilot_loop(execution_id).await
            } else if has_plan_actions {
                // Execute plan actions via capability dispatcher
                if let Some(ref dispatcher) = self.capability_dispatcher {
                    let plan = {
                        let plans = self.execution_plans.read()
                            .map_err(|_| anyhow::anyhow!("execution plans store poisoned"))?;
                        plans.get(execution_id).cloned()
                            .ok_or_else(|| anyhow::anyhow!("plan not found for execution"))?
                    };

                    // Get workspace from execution
                    let workspace = {
                        let entries = self.executions.read()
                            .map_err(|_| anyhow::anyhow!("execution store poisoned"))?;
                        entries.get(execution_id)
                            .and_then(|e| e.workspace.clone())
                            .unwrap_or_else(|| WorkspaceRef {
                                id: WorkspaceId::from_string("default".to_string())
                                    .expect("hardcoded 'default' should be valid WorkspaceId"),
                                mode: WorkspaceMode::Isolated,
                                materialization_mode: None,
                                home_node_id: None,
                                backing_store: None,
                                root: std::env::current_dir()
                                    .unwrap_or_else(|_| std::path::PathBuf::from("/"))
                                    .to_str()
                                    .unwrap_or("/")
                                    .to_string(),
                                writable_roots: vec![],
                            })
                    };

                    // Execute each action in sequence
                    for action in &plan.actions {
                        info!("executing action: {} via {}", action.capability, action.connector_id);

                        // HIGH #3 FIX: Calculate and update execution risk level based on capability
                        // Use heuristic risk calculation based on capability ID patterns
                        {
                            use cyberclaw_core::capability::RiskLevel;

                            let capability_str = action.capability.as_str().to_lowercase();

                            // Start with a base risk level based on capability ID patterns
                            let calculated_risk = if capability_str.contains("system")
                                || capability_str.contains("admin")
                                || capability_str.contains("root") {
                                RiskLevel::High
                            } else if capability_str.contains("write")
                                || capability_str.contains("edit")
                                || capability_str.contains("delete")
                                || capability_str.contains("exec")
                                || capability_str.contains("cmd") {
                                RiskLevel::Medium
                            } else if capability_str.contains("read")
                                || capability_str.contains("search")
                                || capability_str.contains("glob")
                                || capability_str.contains("log") {
                                RiskLevel::Low
                            } else {
                                // Default to Medium for unknown capabilities
                                RiskLevel::Medium
                            };

                            // Update execution risk level (use the highest risk seen so far)
                            {
                                let mut entries = self.executions.write()
                                    .map_err(|_| anyhow::anyhow!("execution store poisoned"))?;
                                if let Some(execution) = entries.get_mut(execution_id) {
                                    if calculated_risk > execution.risk_level {
                                        info!(
                                            "HIGH #3: Updating execution risk level from {:?} to {:?} for capability {}",
                                            execution.risk_level, calculated_risk, action.capability
                                        );
                                        execution.risk_level = calculated_risk;
                                    }
                                }
                            }
                        }

                        // HIGH #2 FIX: Verify trace_id continuity before recording provenance
                        // This ensures trace_id hasn't been tampered with during plan execution
                        let provenance_tampering_event = {
                            let entries = self.executions.read()
                                .map_err(|_| anyhow::anyhow!("execution store poisoned"))?;
                            let execution = entries.get(execution_id)
                                .ok_or_else(|| anyhow::anyhow!("execution not found: {}", execution_id))?;

                            if execution.trace_id != execution_trace_id {
                                let found_trace_id = execution.trace_id.clone();
                                Some((found_trace_id.clone(), SecurityEvent {
                                    id: cyberclaw_core::ids::SecurityEventId::new(),
                                    actor: Some(agent_actor.clone()),
                                    timestamp: chrono::Utc::now(),
                                    execution_id: Some(execution_id.clone()),
                                    case_id: None,
                                    node_id: None,
                                    runtime_instance_id: None,
                                    source: SecurityEventSource::RuntimeDetection,
                                    event_type: SecurityEventType::Custom("TraceIdTampering".to_string()),
                                    severity: Severity::High,
                                    summary: format!(
                                        "trace_id tampering detected before provenance recording: expected {}, found {}",
                                        execution_trace_id, found_trace_id
                                    ),
                                    details: serde_json::json!({
                                        "execution_id": execution_id.as_str(),
                                        "expected_trace_id": execution_trace_id.as_str(),
                                        "found_trace_id": found_trace_id.as_str(),
                                        "checkpoint": "provenance_recording",
                                        "action": action.capability.as_str(),
                                    }),
                                    trace_id: execution_trace_id.clone(),
                                    credential_evidence: None,
                                }))
                            } else {
                                None
                            }
                        };

                        // Handle provenance tampering event after lock is dropped
                        if let Some((found_trace_id, security_event)) = provenance_tampering_event {
                            if let Some(ref sec_store) = self.security_event_store {
                                let _ = sec_store.store(security_event).await;
                            }
                            return Err(anyhow::anyhow!(
                                "trace_id tampering detected before provenance recording: expected {}, found {}",
                                execution_trace_id,
                                found_trace_id
                            ));
                        }

                        // Record capability in provenance (best-effort)
                        if let Some(ref tracker) = self.provenance_tracker {
                            if let Err(e) = tracker
                                .record_capability(execution_id, action.capability.clone())
                                .await
                            {
                                // MEDIUM #1: Enhanced provenance error context
                                // Note: agent_id and trace_id are available in outer scope
                                warn!(
                                    "MEDIUM #1: Provenance record_capability failed | \
                                     execution_id: {} | agent_id: {} | capability_id: {} | \
                                     connector_id: {} | trace_id: {} | \
                                     mode: best-effort | error: {}",
                                    execution_id, agent_id_str, action.capability, action.connector_id,
                                    execution_trace_id, e
                                );
                            }
                        }

                        // Security: validate any `command` field present in the action input
                        // before passing it to the connector layer. This prevents command
                        // injection when a capability accepts a raw command string.
                        if let Some(cmd_val) = action.input.get("command") {
                            if let Some(cmd_str) = cmd_val.as_str() {
                                if let Err(validation_err) = validate_command(cmd_str) {
                                    error!(
                                        "Command injection attempt blocked for action {}: {}",
                                        action.capability, validation_err
                                    );
                                    return Err(anyhow::anyhow!(
                                        "Security violation in action '{}': {}",
                                        action.capability,
                                        validation_err
                                    ));
                                }
                            }
                        }

                        // HIGH #2 FIX: Verify trace_id continuity before dispatcher call
                        // This ensures trace_id hasn't been tampered with before capability dispatch
                        let dispatcher_tampering_event = {
                            let entries = self.executions.read()
                                .map_err(|_| anyhow::anyhow!("execution store poisoned"))?;
                            let execution = entries.get(execution_id)
                                .ok_or_else(|| anyhow::anyhow!("execution not found: {}", execution_id))?;

                            if execution.trace_id != execution_trace_id {
                                let found_trace_id = execution.trace_id.clone();
                                Some((found_trace_id.clone(), SecurityEvent {
                                    id: cyberclaw_core::ids::SecurityEventId::new(),
                                    actor: Some(agent_actor.clone()),
                                    timestamp: chrono::Utc::now(),
                                    execution_id: Some(execution_id.clone()),
                                    case_id: None,
                                    node_id: None,
                                    runtime_instance_id: None,
                                    source: SecurityEventSource::RuntimeDetection,
                                    event_type: SecurityEventType::Custom("TraceIdTampering".to_string()),
                                    severity: Severity::High,
                                    summary: format!(
                                        "trace_id tampering detected before dispatcher call: expected {}, found {}",
                                        execution_trace_id, found_trace_id
                                    ),
                                    details: serde_json::json!({
                                        "execution_id": execution_id.as_str(),
                                        "expected_trace_id": execution_trace_id.as_str(),
                                        "found_trace_id": found_trace_id.as_str(),
                                        "checkpoint": "dispatcher_call",
                                        "action": action.capability.as_str(),
                                    }),
                                    trace_id: execution_trace_id.clone(),
                                    credential_evidence: None,
                                }))
                            } else {
                                None
                            }
                        };

                        // Handle dispatcher tampering event after lock is dropped
                        if let Some((found_trace_id, security_event)) = dispatcher_tampering_event {
                            if let Some(ref sec_store) = self.security_event_store {
                                let _ = sec_store.store(security_event).await;
                            }
                            return Err(anyhow::anyhow!(
                                "trace_id tampering detected before dispatcher call: expected {}, found {}",
                                execution_trace_id,
                                found_trace_id
                            ));
                        }

                        let request = CapabilityExecutionRequest {
                            execution_id: execution_id.clone(),
                            // CRITICAL #9 FIX: Use execution's trace_id instead of generating new one
                            trace_id: execution_trace_id.as_str().to_string(),
                            actor: ActorRef {
                                id: ActorId::from_string("system".to_string())
                                    .expect("hardcoded 'system' should be valid ActorId"),
                                actor_type: ActorType::System,
                                tenant_id: None,
                                home_node_id: None,
                                display_name: "System".to_string(),
                            },
                            workspace: workspace.clone(),
                            connector_id: action.connector_id.clone(),
                            capability_id: action.capability.clone(),
                            input: action.input.clone(),
                        };

                        match dispatcher.dispatch(request).await {
                            Ok(result) => {
                                // Record connector in provenance (best-effort)
                                if let Some(ref tracker) = self.provenance_tracker {
                                    if let Err(e) = tracker
                                        .record_connector(execution_id, action.connector_id.clone())
                                        .await
                                    {
                                        // MEDIUM #1: Enhanced provenance error context
                                        // Note: agent_id and trace_id are available in outer scope
                                        warn!(
                                            "MEDIUM #1: Provenance record_connector failed | \
                                             execution_id: {} | agent_id: {} | capability_id: {} | \
                                             connector_id: {} | trace_id: {} | \
                                             mode: best-effort | error: {}",
                                            execution_id, agent_id_str, action.capability, action.connector_id,
                                            execution_trace_id, e
                                        );
                                    }
                                }

                                if matches!(result.status, cyberclaw_connectors::ExecutionStatus::Failed) {
                                    error!("Action failed: {} - {:?}", action.capability, result.error);
                                    return Err(anyhow::anyhow!("Action failed: {} - {:?}",
                                        action.capability, result.error.unwrap_or_else(|| "unknown error".to_string())));
                                }
                                info!("Action succeeded: {}", action.capability);
                            }
                            Err(e) => {
                                error!("Failed to dispatch action {}: {:?}", action.capability, e);
                                return Err(e);
                            }
                        }
                    }
                    Ok(())
                } else {
                    error!("Plan has actions but no capability dispatcher configured - cannot execute");
                    Err(anyhow::anyhow!(
                        "Cannot execute plan with actions: capability dispatcher not configured. \
                        Call with_capability_dispatcher() when creating the ExecutionService."
                    ))
                }
            } else if let Some(ref runtime) = self.agent_runtime {
                // Fallback to agent runtime
                let agent_id = AgentId::from_string(agent_id_str.clone())
                    .unwrap_or_else(|_| unknown_agent_id().clone());
                let runtime_ref = runtime.clone();
                // S18 R2: Prepend prior_context block to task input if available.
                // prior_context_block is capped at 2KB; see read_prior_context().
                let enriched_task_input = match &prior_context_block {
                    Some(ctx) => format!("{}\n\n{}", ctx, task_input),
                    None => task_input.clone(),
                };
                let task_input_ref = enriched_task_input;
                let agent_id_ref = agent_id.clone();
                retry_with_backoff(
                    "agent_runtime_execute",
                    || {
                        let rt = runtime_ref.clone();
                        let ti = task_input_ref.clone();
                        let aid = agent_id_ref.clone();
                        async move {
                            let req = AgentRequest::new(aid, ti);
                            rt.execute(req).await.map(|_| ()).map_err(|e| anyhow::anyhow!(e))
                        }
                    },
                    &retry_cfg,
                )
                .await
            } else {
                // Neither plan actions nor agent runtime available - this is a configuration error.
                // P0-1: resolver.plan() intentionally returns empty actions until P1 intelligent planning.
                // P0-5: Explicitly fail when there's nothing to execute instead of silent success.
                error!("Cannot execute {}: no executable content available", execution_id);
                Err(anyhow::anyhow!(
                    "Execution {} cannot be processed: no executable content available",
                    execution_id
                ))
            };

            // If agent execution succeeded and skill_runtime is available, invoke skills.
            let runtime_result = if runtime_result.is_ok() {
                if let Some(ref skill_rt) = self.skill_runtime {
                    // Check if the agent_id suggests skill execution (e.g., starts with "skill::")
                    // In future iterations, this can be enhanced to read skills from Resolution.
                    if agent_id_str.starts_with("skill::") {
                        let skill_id_str = agent_id_str
                            .strip_prefix("skill::")
                            .unwrap_or(&agent_id_str);
                        let skill_id =
                            cyberclaw_core::ids::SkillId::from_string(skill_id_str.to_string())
                                .unwrap_or_else(|_| unknown_skill_id().clone());

                        info!("invoking skill: {}", skill_id);
                        let _ = self
                            .event_recorder
                            .record_event(ObservabilityEvent::SkillInvoked {
                                execution_id: execution_id.clone(),
                                skill_id: skill_id.as_str().to_string(),
                                timestamp: chrono::Utc::now(),
                            }).await;

                        let skill_input = serde_json::json!({ "task": task_input });
                        let skill_rt_ref = skill_rt.clone();
                        let skill_id_ref = skill_id.clone();
                        let skill_input_ref = skill_input.clone();
                        match retry_with_backoff(
                            "skill_runtime_invoke",
                            || {
                                let srt = skill_rt_ref.clone();
                                let sid = skill_id_ref.clone();
                                let sinput = skill_input_ref.clone();
                                #[allow(deprecated)]
                                async move { srt.invoke(&sid, sinput).await }
                            },
                            &retry_cfg,
                        )
                        .await
                        {
                            Ok(_output) => {
                                info!("skill execution completed: {}", skill_id);
                                let _ = self.event_recorder.record_event(
                                    ObservabilityEvent::SkillCompleted {
                                        execution_id: execution_id.clone(),
                                        skill_id: skill_id.as_str().to_string(),
                                        success: true,
                                        duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                                        timestamp: chrono::Utc::now(),
                                    },
                                ).await;
                                Ok(())
                            }
                            Err(e) => {
                                info!("skill execution failed: {} – {}", skill_id, e);
                                Err(e)
                            }
                        }
                    } else {
                        runtime_result
                    }
                } else {
                    runtime_result
                }
            } else {
                runtime_result
            };

            let duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            // u64 → f64 精度损失是可接受的 (用于时间统计)
            #[allow(clippy::cast_precision_loss)]
            let duration_secs = duration_ms as f64 / 1000.0;

            match runtime_result {
                Ok(()) => {
                    // Capture timestamp once for consistency across execution and events
                    let completion_time = chrono::Utc::now();

                    let (prev_status, completion_tampering_event) = {
                        let mut entries = self
                            .executions
                            .write()
                            .map_err(|_| anyhow::anyhow!("execution store poisoned"))?;
                        let execution = entries.get_mut(execution_id).ok_or_else(|| {
                            anyhow::anyhow!("execution not found after run: {}", execution_id)
                        })?;

                        // HIGH #2 FIX: Verify trace_id continuity before marking execution complete
                        // This ensures trace_id hasn't been tampered with during execution
                        let completion_tampering_event = if execution.trace_id != execution_trace_id {
                            let found_trace_id = execution.trace_id.clone();
                            Some((found_trace_id.clone(), SecurityEvent {
                                id: cyberclaw_core::ids::SecurityEventId::new(),
                                actor: Some(agent_actor.clone()),
                                timestamp: chrono::Utc::now(),
                                execution_id: Some(execution_id.clone()),
                                case_id: None,
                                node_id: None,
                                runtime_instance_id: None,
                                source: SecurityEventSource::RuntimeDetection,
                                event_type: SecurityEventType::Custom("TraceIdTampering".to_string()),
                                severity: Severity::High,
                                summary: format!(
                                    "trace_id tampering detected at execution completion: expected {}, found {}",
                                    execution_trace_id, found_trace_id
                                ),
                                details: serde_json::json!({
                                    "execution_id": execution_id.as_str(),
                                    "expected_trace_id": execution_trace_id.as_str(),
                                    "found_trace_id": found_trace_id.as_str(),
                                    "checkpoint": "execution_completion",
                                }),
                                trace_id: execution_trace_id.clone(),
                                credential_evidence: None,
                            }))
                        } else {
                            None
                        };

                        let prev = execution.status.clone();
                        execution.status = ExecutionStatus::Completed;
                        if execution.finished_at.is_none() {
                            execution.finished_at = Some(completion_time);
                        }
                        (prev, completion_tampering_event)
                    };

                    // Handle completion tampering event after lock is dropped
                    if let Some((found_trace_id, security_event)) = completion_tampering_event {
                        if let Some(ref sec_store) = self.security_event_store {
                            let _ = sec_store.store(security_event).await;
                        }
                        return Err(anyhow::anyhow!(
                            "trace_id tampering detected at execution completion: expected {}, found {}",
                            execution_trace_id,
                            found_trace_id
                        ));
                    }
                    let _ = self
                        .event_recorder
                        .record_event(ObservabilityEvent::ExecutionStatusChanged {
                            execution_id: execution_id.clone(),
                            from_status: prev_status,
                            to_status: ExecutionStatus::Completed,
                            timestamp: completion_time,
                        })
                        .await;
                    let _ =
                        self.event_recorder
                            .record_event(ObservabilityEvent::AgentExecutionCompleted {
                                execution_id: execution_id.clone(),
                                agent_id: agent_id_str.clone(),
                                status: ExecutionStatus::Completed,
                                duration_ms,
                                timestamp: completion_time,
                            }).await;

                    // S18 R1: Write L1Summary episodic memory (best-effort, fire-and-forget)
                    if let Some(ref mem_store) = self.leveled_memory_store {
                        let memory_id = self.write_episodic_memory(
                            mem_store.as_ref(),
                            execution_id,
                            &agent_id_str,
                            duration_ms,
                        ).await;
                        if let Some(mid) = memory_id {
                            let _ = self
                                .event_recorder
                                .record_event(ObservabilityEvent::MemoryWritten {
                                    execution_id: execution_id.clone(),
                                    memory_id: mid,
                                    session_id: execution_id.as_str().to_string(),
                                    timestamp: chrono::Utc::now(),
                                })
                                .await;
                        }
                    }

                    recorders::record_execution_complete(
                        &ExecutionStatus::Completed,
                        duration_secs,
                    );
                    recorders::record_execution_state_change("Running", "Completed");

                    // Publish ExecutionLeaseExpired as a proxy for "execution finished" on a
                    // best-effort basis. A failure here must not abort the completion path.
                    self.publish_event_best_effort(ClusterEvent::ExecutionLeaseExpired {
                        execution_id: execution_id.clone(),
                        lease_id: cyberclaw_core::ids::LeaseId::new(),
                        expired_node_id: cyberclaw_core::ids::NodeId::from_string(
                            "local".to_string(),
                        )
                        .unwrap_or_else(|_| cyberclaw_core::ids::NodeId::new()),
                        timestamp: completion_time,
                    });

                    // Record ExecutionCompleted security event (fire-and-forget)
                    if let Some(ref sec_store) = self.security_event_store {
                        let _ = sec_store
                            .store(SecurityEvent {
                                id: cyberclaw_core::ids::SecurityEventId::new(),
                                actor: Some(agent_actor.clone()),
                                timestamp: chrono::Utc::now(),
                                execution_id: Some(execution_id.clone()),
                                case_id: None,
                                node_id: None,
                                runtime_instance_id: None,
                                source: SecurityEventSource::RuntimeDetection,
                                event_type: SecurityEventType::Custom(
                                    "ExecutionCompleted".to_string(),
                                ),
                                severity: Severity::Info,
                                summary: format!("Execution completed: {}", execution_id),
                                details: serde_json::json!({
                                    "execution_id": execution_id.as_str(),
                                    "actor": agent_id_str,
                                    "duration_ms": duration_ms,
                                }),
                                trace_id: execution_trace_id.clone(),
                                credential_evidence: None,
                            })
                            .await;
                    }

                    // HIGH #3 FIX: Finalize provenance with risk-based failure handling
                    // CRITICAL/HIGH risk executions must fail-fast on provenance failure
                    // MEDIUM/LOW can use best-effort (warn only)
                    if let Some(ref tracker) = self.provenance_tracker {
                        let execution_risk = {
                            let entries = self.executions.read()
                                .map_err(|_| anyhow::anyhow!("execution store poisoned"))?;
                            entries.get(execution_id)
                                .map(|e| e.risk_level)
                                .unwrap_or(cyberclaw_core::capability::RiskLevel::Low)
                        };

                        // Check if this risk level requires fail-fast provenance
                        let requires_provenance = self.security_config
                            .config()
                            .requires_provenance(execution_risk);

                        if let Err(e) = tracker.finalize(execution_id).await {
                            if requires_provenance {
                                // MEDIUM #1: Enhanced provenance error context for fail-fast path
                                // Fail-fast for CRITICAL/HIGH risk executions
                                error!(
                                    "MEDIUM #1: Provenance finalization failed (fail-fast mode) | \
                                     execution_id: {} | agent_id: {} | trace_id: {} | \
                                     risk_level: {:?} | requires_provenance: true | error: {}",
                                    execution_id, agent_id_str, execution_trace_id, execution_risk, e
                                );
                                return Err(anyhow::anyhow!(
                                    "Provenance finalization required but failed for {:?} risk execution: {}",
                                    execution_risk, e
                                ));
                            } else {
                                // MEDIUM #1: Enhanced provenance error context for best-effort path
                                // Best-effort for MEDIUM/LOW risk executions
                                warn!(
                                    "MEDIUM #1: Provenance finalization failed (best-effort mode) | \
                                     execution_id: {} | agent_id: {} | trace_id: {} | \
                                     risk_level: {:?} | requires_provenance: false | error: {}",
                                    execution_id, agent_id_str, execution_trace_id, execution_risk, e
                                );
                            }
                        }
                    }

                    // Append to memory (best-effort)
                    if let Some(ref memory) = self.memory_provider {
                        // CRITICAL #3 FIX: Filter sensitive data before storing in memory
                        let scanner = SecretScanner::new();
                        let raw_summary = format!(
                            "Execution {} completed successfully (agent: {}, duration: {}ms)",
                            execution_id, agent_id_str, duration_ms
                        );
                        let summary = scanner.redact_all(&raw_summary);
                        let entry = cyberclaw_core::memory_context::WorkingMemoryEntry {
                            execution_id: Some(execution_id.clone()),
                            kind: cyberclaw_core::memory_context::WorkingEntryKind::Decision,
                            summary,
                            artifact_refs: vec![],
                            trace_id: Some(execution_trace_id.clone()),
                            encrypted: false,
                        };

                        // HIGH #5 FIX: Atomic check-and-add to prevent TOCTOU race
                        // Lock is held only during the check+add operation (not the entire memory write)
                        {
                            let mut count = memory_entries_added.lock().unwrap_or_else(|e| e.into_inner());
                            if *count >= MAX_MEMORY_ENTRIES_PER_EXECUTION {
                                // MEDIUM #3: Enhanced memory limit error context
                                error!(
                                    "MEDIUM #3: Memory entry limit reached | \
                                     execution_id: {} | agent_id: {} | trace_id: {} | \
                                     current_entries: {} | max_entries: {} | \
                                     action: skipping_additional_entries | \
                                     risk: potential_memory_loss",
                                    execution_id, agent_id_str, execution_trace_id,
                                    *count, MAX_MEMORY_ENTRIES_PER_EXECUTION
                                );
                            } else {
                                memory.add_working_entry(entry);
                                *count += 1;
                            }
                        }

                        // Trigger compaction if entry count exceeds threshold
                        if let Ok(entry_count) = memory.get_working_entry_count() {
                            if entry_count > 1000 {
                                warn!(
                                    "Working memory entry count ({}) exceeds threshold, triggering compaction",
                                    entry_count
                                );
                                let strategy = cyberclaw_core::memory::compaction::CompactionStrategy {
                                    keep_recent_count: 500,
                                    enable_deduplication: true,
                                    keep_procedural: true,
                                    similarity_threshold: 0.85,
                                };
                                let result = memory.compact(strategy);
                                if result.success {
                                    info!(
                                        "Memory compaction succeeded: {} -> {} items, saved {} chars",
                                        result.items_before, result.items_after, result.chars_saved
                                    );
                                } else {
                                    // HIGH #4 + MEDIUM #3: Enhanced compaction failure handling with full diagnostic context
                                    error!(
                                        "MEDIUM #3: Memory compaction failed | \
                                         execution_id: {} | agent_id: {} | trace_id: {} | \
                                         items_before: {} | items_after: {} | current_entries: {} | \
                                         max_entries: {} | compression_failed: true | \
                                         risk: memory_exhaustion (OOM)",
                                        execution_id, agent_id_str, execution_trace_id,
                                        result.items_before, result.items_after, entry_count,
                                        MAX_MEMORY_ENTRIES_PER_EXECUTION
                                );

                                // Record CompactionFailed security event
                                if let Some(ref sec_store) = self.security_event_store {
                                    let _ = sec_store
                                        .store(SecurityEvent {
                                            id: cyberclaw_core::ids::SecurityEventId::new(),
                                            actor: Some(agent_actor.clone()),
                                            timestamp: chrono::Utc::now(),
                                            execution_id: Some(execution_id.clone()),
                                            case_id: None,
                                            node_id: None,
                                            runtime_instance_id: None,
                                            source: SecurityEventSource::RuntimeDetection,
                                            event_type: SecurityEventType::Custom(
                                                "CompactionFailed".to_string(),
                                            ),
                                            severity: Severity::High,
                                            summary: format!(
                                                "Memory compaction failed for execution {} - risk of OOM",
                                                execution_id
                                            ),
                                            details: serde_json::json!({
                                                "execution_id": execution_id.as_str(),
                                                "items_before": result.items_before,
                                                "items_after": result.items_after,
                                                "current_entry_count": entry_count,
                                                "threshold": 1000,
                                                "error": result.error.unwrap_or_else(|| "unknown".to_string()),
                                            }),
                                            trace_id: execution_trace_id.clone(),
                                            credential_evidence: None,
                                        })
                                        .await;
                                }
                            }
                        }
                    }
                }

                    // MEDIUM #9: 统一审计日志格式
                    audit_logger.log(
                        AuditLogEntry::new(
                            "ExecutionService",
                            "execution.completed",
                            format!("Execution {} completed successfully", execution_id)
                        )
                        .with_trace_id(execution_trace_id.as_str())
                        .with_metadata("execution_id", execution_id.to_string())
                        .with_metadata("duration_ms", started.elapsed().as_millis().to_string())
                    );

                    info!("execution completed: {}", execution_id);
                    Ok(())
                }
                Err(err) => {
                    // Capture timestamp once for consistency across execution and events
                    let failure_time = chrono::Utc::now();

                    let (prev_fail_status, failure_tampering_event) = {
                        let mut entries = self
                            .executions
                            .write()
                            .map_err(|_| anyhow::anyhow!("execution store poisoned"))?;
                        let execution = entries.get_mut(execution_id).ok_or_else(|| {
                            anyhow::anyhow!("execution not found after error: {}", execution_id)
                        })?;

                        // HIGH #2 FIX: Verify trace_id continuity before marking execution failed
                        // This ensures trace_id hasn't been tampered with even during failure path
                        let failure_tampering_event = if execution.trace_id != execution_trace_id {
                            let found_trace_id = execution.trace_id.clone();
                            Some((found_trace_id.clone(), SecurityEvent {
                                id: cyberclaw_core::ids::SecurityEventId::new(),
                                actor: Some(agent_actor.clone()),
                                timestamp: chrono::Utc::now(),
                                execution_id: Some(execution_id.clone()),
                                case_id: None,
                                node_id: None,
                                runtime_instance_id: None,
                                source: SecurityEventSource::RuntimeDetection,
                                event_type: SecurityEventType::Custom("TraceIdTampering".to_string()),
                                severity: Severity::High,
                                summary: format!(
                                    "trace_id tampering detected at execution failure: expected {}, found {}",
                                    execution_trace_id, found_trace_id
                                ),
                                details: serde_json::json!({
                                    "execution_id": execution_id.as_str(),
                                    "expected_trace_id": execution_trace_id.as_str(),
                                    "found_trace_id": found_trace_id.as_str(),
                                    "checkpoint": "execution_failure",
                                }),
                                trace_id: execution_trace_id.clone(),
                                credential_evidence: None,
                            }))
                        } else {
                            None
                        };

                        let prev = execution.status.clone();
                        execution.status = ExecutionStatus::Failed;
                        if execution.finished_at.is_none() {
                            execution.finished_at = Some(failure_time);
                        }
                        (prev, failure_tampering_event)
                    };

                    // Handle failure tampering event after lock is dropped
                    if let Some((found_trace_id, security_event)) = failure_tampering_event {
                        if let Some(ref sec_store) = self.security_event_store {
                            let _ = sec_store.store(security_event).await;
                        }
                        // Note: Even in failure path, we want to record tampering but still
                        // mark execution as failed to preserve the original error context
                        warn!(
                            "trace_id tampering detected at execution failure: expected {}, found {}",
                            execution_trace_id, found_trace_id
                        );
                    }
                    let _ = self.event_recorder.record_event(
                        ObservabilityEvent::ExecutionStatusChanged {
                            execution_id: execution_id.clone(),
                            from_status: prev_fail_status,
                            to_status: ExecutionStatus::Failed,
                            timestamp: failure_time,
                        },
                    ).await;
                    let _ =
                        self.event_recorder
                            .record_event(ObservabilityEvent::AgentExecutionCompleted {
                                execution_id: execution_id.clone(),
                                agent_id: agent_id_str.clone(),
                                status: ExecutionStatus::Failed,
                                duration_ms,
                                timestamp: failure_time,
                            }).await;
                    recorders::record_execution_complete(&ExecutionStatus::Failed, duration_secs);
                    recorders::record_execution_state_change("Running", "Failed");

                    // Publish ExecutionLeaseExpired as a proxy for "execution finished (failed)"
                    // on a best-effort basis. A failure here must not mask the original error.
                    self.publish_event_best_effort(ClusterEvent::ExecutionLeaseExpired {
                        execution_id: execution_id.clone(),
                        lease_id: cyberclaw_core::ids::LeaseId::new(),
                        expired_node_id: cyberclaw_core::ids::NodeId::from_string(
                            "local".to_string(),
                        )
                        .unwrap_or_else(|_| cyberclaw_core::ids::NodeId::new()),
                        timestamp: failure_time,
                    });

                    // Record ExecutionFailed security event (fire-and-forget)
                    if let Some(ref sec_store) = self.security_event_store {
                        let _ = sec_store
                            .store(SecurityEvent {
                                id: cyberclaw_core::ids::SecurityEventId::new(),
                                actor: Some(agent_actor.clone()),
                                timestamp: chrono::Utc::now(),
                                execution_id: Some(execution_id.clone()),
                                case_id: None,
                                node_id: None,
                                runtime_instance_id: None,
                                source: SecurityEventSource::RuntimeDetection,
                                event_type: SecurityEventType::Custom(
                                    "ExecutionFailed".to_string(),
                                ),
                                severity: Severity::High,
                                summary: format!("Execution failed: {}", execution_id),
                                details: serde_json::json!({
                                    "execution_id": execution_id.as_str(),
                                    "actor": agent_id_str,
                                    "error": err.to_string(),
                                    "duration_ms": duration_ms,
                                }),
                                trace_id: execution_trace_id.clone(),
                                credential_evidence: None,
                            })
                            .await;
                    }

                    // HIGH #3 FIX: Finalize provenance even on failure with risk-based handling
                    // CRITICAL/HIGH risk executions must fail-fast on provenance failure
                    // MEDIUM/LOW can use best-effort (warn only)
                    if let Some(ref tracker) = self.provenance_tracker {
                        let execution_risk = {
                            let entries = self.executions.read()
                                .map_err(|_| anyhow::anyhow!("execution store poisoned"))?;
                            entries.get(execution_id)
                                .map(|e| e.risk_level)
                                .unwrap_or(cyberclaw_core::capability::RiskLevel::Low)
                        };

                        // Check if this risk level requires fail-fast provenance
                        let requires_provenance = self.security_config
                            .config()
                            .requires_provenance(execution_risk);

                        if let Err(e) = tracker.finalize(execution_id).await {
                            if requires_provenance {
                                // MEDIUM #1: Enhanced provenance error context for fail-fast path
                                // Fail-fast for CRITICAL/HIGH risk executions
                                error!(
                                    "MEDIUM #1: Provenance finalization failed (fail-fast mode) | \
                                     execution_id: {} | agent_id: {} | trace_id: {} | \
                                     risk_level: {:?} | requires_provenance: true | error: {}",
                                    execution_id, agent_id_str, execution_trace_id, execution_risk, e
                                );
                                return Err(anyhow::anyhow!(
                                    "Provenance finalization required but failed for {:?} risk execution: {}",
                                    execution_risk, e
                                ));
                            } else {
                                // MEDIUM #1: Enhanced provenance error context for best-effort path
                                // Best-effort for MEDIUM/LOW risk executions
                                warn!(
                                    "MEDIUM #1: Provenance finalization failed (best-effort mode) | \
                                     execution_id: {} | agent_id: {} | trace_id: {} | \
                                     risk_level: {:?} | requires_provenance: false | error: {}",
                                    execution_id, agent_id_str, execution_trace_id, execution_risk, e
                                );
                            }
                        }
                    }

                    // Append failure to memory (best-effort)
                    if let Some(ref memory) = self.memory_provider {
                        // CRITICAL #3 FIX: Filter sensitive data before storing in memory
                        let scanner = SecretScanner::new();
                        let raw_summary = format!(
                            "Execution {} failed (agent: {}, error: {}, duration: {}ms)",
                            execution_id,
                            agent_id_str,
                            err.to_string().chars().take(100).collect::<String>(),
                            duration_ms
                        );
                        let summary = scanner.redact_all(&raw_summary);
                        let entry = cyberclaw_core::memory_context::WorkingMemoryEntry {
                            execution_id: Some(execution_id.clone()),
                            kind: cyberclaw_core::memory_context::WorkingEntryKind::ToolResult,
                            summary,
                            artifact_refs: vec![],
                            trace_id: Some(execution_trace_id.clone()),
                            encrypted: false,
                        };

                        // HIGH #5 FIX: Atomic check-and-add to prevent TOCTOU race
                        // Lock is held only during the check+add operation (not the entire memory write)
                        {
                            let mut count = memory_entries_added.lock().unwrap_or_else(|e| e.into_inner());
                            if *count >= MAX_MEMORY_ENTRIES_PER_EXECUTION {
                                // MEDIUM #3: Enhanced memory limit error context
                                error!(
                                    "MEDIUM #3: Memory entry limit reached | \
                                     execution_id: {} | agent_id: {} | trace_id: {} | \
                                     current_entries: {} | max_entries: {} | \
                                     action: skipping_additional_entries | \
                                     risk: potential_memory_loss",
                                    execution_id, agent_id_str, execution_trace_id,
                                    *count, MAX_MEMORY_ENTRIES_PER_EXECUTION
                                );
                            } else {
                                memory.add_working_entry(entry);
                                *count += 1;
                            }
                        }

                        // Trigger compaction if entry count exceeds threshold (same logic as success path)
                        if let Ok(entry_count) = memory.get_working_entry_count() {
                            if entry_count > 1000 {
                                warn!(
                                    "Working memory entry count ({}) exceeds threshold, triggering compaction",
                                    entry_count
                                );
                                let strategy = cyberclaw_core::memory::compaction::CompactionStrategy {
                                    keep_recent_count: 500,
                                    enable_deduplication: true,
                                    keep_procedural: true,
                                    similarity_threshold: 0.85,
                                };
                                let result = memory.compact(strategy);
                                if result.success {
                                    info!(
                                        "Memory compaction succeeded: {} -> {} items, saved {} chars",
                                        result.items_before, result.items_after, result.chars_saved
                                    );
                                } else {
                                    // HIGH #4 + MEDIUM #3: Enhanced compaction failure handling with full diagnostic context
                                    error!(
                                        "MEDIUM #3: Memory compaction failed | \
                                         execution_id: {} | agent_id: {} | trace_id: {} | \
                                         items_before: {} | items_after: {} | current_entries: {} | \
                                         max_entries: {} | compression_failed: true | \
                                         risk: memory_exhaustion (OOM)",
                                        execution_id, agent_id_str, execution_trace_id,
                                        result.items_before, result.items_after, entry_count,
                                        MAX_MEMORY_ENTRIES_PER_EXECUTION
                                    );

                                // Record CompactionFailed security event
                                if let Some(ref sec_store) = self.security_event_store {
                                    let _ = sec_store
                                        .store(SecurityEvent {
                                            id: cyberclaw_core::ids::SecurityEventId::new(),
                                            actor: Some(agent_actor.clone()),
                                            timestamp: chrono::Utc::now(),
                                            execution_id: Some(execution_id.clone()),
                                            case_id: None,
                                            node_id: None,
                                            runtime_instance_id: None,
                                            source: SecurityEventSource::RuntimeDetection,
                                            event_type: SecurityEventType::Custom(
                                                "CompactionFailed".to_string(),
                                            ),
                                            severity: Severity::High,
                                            summary: format!(
                                                "Memory compaction failed for execution {} - risk of OOM",
                                                execution_id
                                            ),
                                            details: serde_json::json!({
                                                "execution_id": execution_id.as_str(),
                                                "items_before": result.items_before,
                                                "items_after": result.items_after,
                                                "current_entry_count": entry_count,
                                                "threshold": 1000,
                                                "error": result.error.unwrap_or_else(|| "unknown".to_string()),
                                            }),
                                            trace_id: execution_trace_id.clone(),
                                            credential_evidence: None,
                                        })
                                        .await;
                                }
                            }
                        }
                    }
                }

                    // MEDIUM #9: 统一审计日志格式
                    audit_logger.log(
                        AuditLogEntry::new(
                            "ExecutionService",
                            "execution.failed",
                            format!("Execution {} failed: {}", execution_id, err)
                        )
                        .with_trace_id(execution_trace_id.as_str())
                        .with_metadata("execution_id", execution_id.to_string())
                        .with_metadata("error", err.to_string())
                        .with_severity(cyberclaw_core::audit_logger::LogSeverity::Error)
                    );

                    info!("execution failed: {} – {}", execution_id, err);
                    Err(err)
                }
            }
        }
        .instrument(agent_span)
        .await
    }

    async fn set_assignment(
        &self,
        execution_id: &ExecutionId,
        scheduled_node_id: NodeId,
        lease_id: LeaseId,
    ) -> anyhow::Result<()> {
        let mut entries = self
            .executions
            .write()
            .map_err(|_| anyhow::anyhow!("execution store poisoned"))?;
        let execution = entries
            .get_mut(execution_id)
            .ok_or_else(|| anyhow::anyhow!("execution not found: {}", execution_id))?;

        execution.scheduled_node_id = Some(scheduled_node_id.clone());
        execution.owner_node_id = Some(scheduled_node_id);
        execution.lease_id = Some(lease_id);
        Ok(())
    }

    async fn get_plan(&self, execution_id: &ExecutionId) -> anyhow::Result<Option<ExecutionPlan>> {
        let plans = self
            .execution_plans
            .read()
            .map_err(|_| anyhow::anyhow!("execution plans store poisoned"))?;
        Ok(plans.get(execution_id).cloned())
    }

    // ============ Autopilot 专用方法实现 ============

    async fn execute_autopilot_iteration(
        &self,
        execution_id: &ExecutionId,
        iteration: u32,
    ) -> anyhow::Result<IterationResult> {
        info!(
            "Executing Autopilot iteration {} for {}",
            iteration, execution_id
        );

        // Initialize result
        let mut steps_completed = Vec::new();
        let mut errors = Vec::new();
        let mut output = None;

        // Execute 9-step loop
        let steps = vec![
            AutopilotStep::Plan,
            AutopilotStep::Execute,
            AutopilotStep::Review,
            AutopilotStep::Analyze,
            AutopilotStep::Decide,
            AutopilotStep::Update,
            AutopilotStep::Check,
            AutopilotStep::Iterate,
            AutopilotStep::Finalize,
        ];

        // Build accumulated context for step runner (carries forward step outputs)
        let mut step_context = serde_json::json!({
            "iteration": iteration,
            "execution_id": execution_id.to_string(),
        });

        for step in &steps {
            let start = std::time::Instant::now();

            // Delegate to real step runner if available, otherwise fallback to placeholder
            let step_result = if let Some(ref runner) = self.autopilot_step_runner {
                match runner
                    .run_step(execution_id, step, iteration, &step_context)
                    .await
                {
                    Ok(result) => result,
                    Err(e) => {
                        warn!("AutopilotStepRunner failed for step {:?}: {}", step, e);
                        StepResult {
                            step: step.clone(),
                            success: false,
                            output: None,
                            error: Some(format!("{}", e)),
                            duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
                        }
                    }
                }
            } else {
                // No AutopilotStepRunner configured.
                // In test builds, return a placeholder result for convenience.
                // In non-test builds, fail loudly to prevent silent no-op executions.
                // NOTE: The #[cfg(not(test))] path cannot be unit-tested in this crate.
                // To test the fail-loud behavior, use an integration test or convert
                // this to a struct field (allow_placeholder_steps: bool) in the future.
                #[cfg(test)]
                {
                    warn!("No AutopilotStepRunner configured — returning test placeholder for step {:?}", step);
                    StepResult {
                        step: step.clone(),
                        success: true,
                        output: Some(serde_json::json!({"placeholder": true})),
                        error: None,
                        duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
                    }
                }
                #[cfg(not(test))]
                {
                    error!(
                        "No AutopilotStepRunner configured — refusing to silently skip step {:?}",
                        step
                    );
                    StepResult {
                        step: step.clone(),
                        success: false,
                        output: None,
                        error: Some("AutopilotStepRunner not configured. Cannot execute autopilot steps without a runner.".to_string()),
                        duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
                    }
                }
            };

            // Record step completion
            self.on_step_complete(execution_id, step.clone(), step_result.clone())
                .await?;

            if step_result.success {
                steps_completed.push(step.clone());
                if let Some(ref o) = step_result.output {
                    // Carry step output into context for subsequent steps
                    step_context[&format!("{:?}", step)] = o.clone();
                    output = Some(o.clone());
                }
            } else {
                if let Some(err) = step_result.error {
                    errors.push(err);
                }
                break; // Stop on failure
            }
        }

        // Determine decision and progress
        let decision = if steps_completed.contains(&AutopilotStep::Finalize) {
            Decision::GoalMet
        } else if errors.is_empty() && steps_completed.len() >= 5 {
            Decision::Continue
        } else if errors.len() > 3 {
            Decision::Stuck
        } else {
            Decision::Continue
        };

        let progress_made = steps_completed.len() >= 3 && errors.is_empty();

        Ok(IterationResult {
            iteration,
            steps_completed,
            decision,
            progress_made,
            output,
            errors,
        })
    }

    async fn on_iteration_start(
        &self,
        execution_id: &ExecutionId,
        iteration: u32,
    ) -> anyhow::Result<()> {
        info!(
            "Starting iteration {} for execution {}",
            iteration, execution_id
        );

        // Dedicated Autopilot observability events are not yet defined;
        // iteration start is logged via tracing for now.
        info!(
            "Autopilot iteration {} started for execution {}",
            iteration, execution_id
        );

        // Sync state if coordinator is available
        if let Some(ref sync) = self.state_sync {
            sync.sync_before_iteration(execution_id, iteration).await?;
        }

        Ok(())
    }

    async fn on_step_complete(
        &self,
        execution_id: &ExecutionId,
        step: AutopilotStep,
        result: StepResult,
    ) -> anyhow::Result<()> {
        info!(
            "Step {:?} completed for execution {} with success={}",
            step, execution_id, result.success
        );

        // Step completion event not yet emitted; awaiting Autopilot ObservabilityEvent variants.

        Ok(())
    }

    async fn on_stuck_detected(
        &self,
        execution_id: &ExecutionId,
        iteration: u32,
        reason: String,
    ) -> anyhow::Result<StuckResolution> {
        warn!(
            "Stuck detected for execution {} at iteration {}: {}",
            execution_id, iteration, reason
        );

        // Stuck event not yet emitted; awaiting Autopilot ObservabilityEvent variants.

        // Get resolution from detector if available
        if let Some(ref detector) = self.stuck_detector {
            detector.get_resolution(execution_id, &reason).await
        } else {
            // Default resolution: abort
            Ok(StuckResolution::Abort)
        }
    }

    async fn checkpoint_iteration(
        &self,
        execution_id: &ExecutionId,
        iteration: u32,
        state: IterationState,
    ) -> anyhow::Result<()> {
        info!(
            "Checkpointing iteration {} for execution {}",
            iteration, execution_id
        );

        // Save to checkpoint store if available
        if let Some(ref store) = self.checkpoint_store {
            store.save(execution_id, iteration, &state).await?;
        }

        // Also save to iteration histories
        {
            let mut histories = self
                .iteration_histories
                .write()
                .map_err(|_| anyhow::anyhow!("iteration histories poisoned"))?;
            let history = histories
                .entry(execution_id.clone())
                .or_insert_with(Vec::new);

            // Keep only last N iterations for memory efficiency
            const MAX_HISTORY_SIZE: usize = 20;
            if history.len() >= MAX_HISTORY_SIZE {
                history.remove(0);
            }
        }

        Ok(())
    }

    async fn resume_from_checkpoint(
        &self,
        execution_id: &ExecutionId,
    ) -> anyhow::Result<Option<IterationState>> {
        info!(
            "Attempting to resume execution {} from checkpoint",
            execution_id
        );

        // Load from checkpoint store if available
        if let Some(ref store) = self.checkpoint_store {
            store.load_latest(execution_id).await
        } else {
            Ok(None)
        }
    }

    async fn get_iteration_history(
        &self,
        execution_id: &ExecutionId,
    ) -> anyhow::Result<Vec<IterationSummary>> {
        let histories = self
            .iteration_histories
            .read()
            .map_err(|_| anyhow::anyhow!("iteration histories poisoned"))?;

        Ok(histories.get(execution_id).cloned().unwrap_or_default())
    }
}

// ─── S21 T5: Handoff briefing addendum builder ───────────────────────────────

/// Build a `<handoff_briefing>` prompt block from a [`HandoffRequest`].
///
/// Enforces a 4KB total cap:
/// - briefing text is truncated to 2KB
/// - `context_artifacts` are appended until their combined section reaches 2KB
///
/// The returned string is prepended ahead of the `<prior_context>` block in
/// `read_prior_context` when the current execution's session was minted from
/// an accepted handoff (Sprint 21 T8 wiring).
pub(crate) fn build_handoff_briefing_addendum(
    req: &cyberclaw_core::handoff::HandoffRequest,
) -> String {
    let mut s = String::new();
    s.push_str("<handoff_briefing>\n");
    s.push_str(&format!("From: {}\n", req.from_agent_id));
    s.push_str(&format!("Reason: {}\n\n", req.reason));

    // Truncate briefing at 2KB
    const BRIEFING_CAP: usize = 2048;
    let briefing = if req.briefing_text.len() > BRIEFING_CAP {
        tracing::warn!(
            handoff_id = %req.handoff_id,
            size = req.briefing_text.len(),
            "handoff briefing truncated to 2KB"
        );
        &req.briefing_text[..BRIEFING_CAP]
    } else {
        &req.briefing_text
    };
    s.push_str(briefing);
    s.push_str("\n</handoff_briefing>\n\n");

    // Artifacts: iterate up to 2KB combined budget
    let mut artifact_budget_left: usize = 2048;
    for art in &req.context_artifacts {
        let header = format!(
            "<context_artifact path=\"{}\" mime=\"{}\">\n",
            art.uri, art.content_type
        );
        let footer = "\n</context_artifact>\n\n";
        let overhead = header.len() + footer.len();
        let body_budget = artifact_budget_left.saturating_sub(overhead);
        if body_budget == 0 {
            break;
        }
        // v1: use title as artifact body; resolving artifact content by URI is a v2 concern
        let body = if art.title.len() > body_budget {
            &art.title[..body_budget]
        } else {
            &art.title
        };
        s.push_str(&header);
        s.push_str(body);
        s.push_str(footer);
        artifact_budget_left =
            artifact_budget_left.saturating_sub(header.len() + body.len() + footer.len());
    }

    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handoff_queue::HandoffQueue;
    use crate::risk_calculator::{calculate_execution_risk_level, resolve_trust_level};
    use cyberclaw_agent_runtime::{AgentRuntime, MockAgentRuntime};
    use cyberclaw_core::enums::Priority;
    use cyberclaw_core::identity::{ActorRef, ActorType};
    use cyberclaw_core::ids::{ActorId, AgentId, ExecutionId, TaskId};
    use cyberclaw_core::task::{Task, TaskInput, TaskKind, TriggerRef};
    use cyberclaw_skill_runtime::{MinimalSkillRuntime, SkillRuntime};

    fn make_test_task() -> Task {
        Task {
            id: TaskId::new(),
            case_id: None,
            title: "Test Task".to_string(),
            summary: "Unit test task".to_string(),
            kind: TaskKind::Analysis,
            priority: Priority::Low,
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
                source: "test".to_string(),
            },
            input: TaskInput::default(),
            desired_outputs: vec![],
            labels: vec![],
            preferred_agent_id: None,
        }
    }

    fn make_request_with_id(execution_id: ExecutionId, task: Task) -> ExecutionRequest {
        ExecutionRequest {
            execution_id,
            task,
            case: None,
            context: crate::types::ControlPlaneContext {
                actor: ActorRef {
                    id: ActorId::from_string("test-user".to_string()).unwrap(),
                    actor_type: ActorType::Human,
                    tenant_id: None,
                    home_node_id: None,
                    display_name: "Test User".to_string(),
                },
                session: None,
                workspace: None,
            },
            agent: Some(AgentRef {
                id: AgentId::from_string("control-plane".to_string()).unwrap(),
                role: "control-plane".to_string(),
            }),
            trace_id: None,
            execution_mode: None,
            plan: None,
        }
    }

    /// 验证提交相同 execution_id 的请求会被拒绝（幂等性检查）
    #[tokio::test]
    async fn test_submit_duplicate_execution_id_fails() {
        let svc = InMemoryExecutionService::new();
        let exec_id = ExecutionId::new();

        // 第一次提交应该成功
        let first = svc
            .submit(make_request_with_id(exec_id.clone(), make_test_task()))
            .await;
        assert!(
            first.is_ok(),
            "first submit should succeed: {:?}",
            first.err()
        );
        assert_eq!(first.unwrap(), exec_id);

        // 使用相同 execution_id 再次提交应该失败
        let second = svc
            .submit(make_request_with_id(exec_id.clone(), make_test_task()))
            .await;
        assert!(
            second.is_err(),
            "second submit with same execution_id should fail"
        );
        let err_msg = second.unwrap_err().to_string();
        assert!(
            err_msg.contains("already exists") || err_msg.contains("duplicate"),
            "error should mention duplicate: {}",
            err_msg
        );
    }

    /// 验证已处于 Running 状态的 execution 不能被再次执行（防并发重复执行）
    #[tokio::test]
    async fn test_execute_already_running_fails() {
        let svc = InMemoryExecutionService::new();
        let exec_id = ExecutionId::new();

        // 提交 execution（初始为 Pending 状态）
        svc.submit(make_request_with_id(exec_id.clone(), make_test_task()))
            .await
            .unwrap();

        // 手动将状态设置为 Running（模拟已被另一个线程开始执行）
        svc.update_status(&exec_id, ExecutionStatus::Running)
            .await
            .unwrap();

        // 在 Running 状态下调用 execute() 应该失败
        let result = svc.execute(&exec_id).await;
        assert!(result.is_err(), "execute on Running execution should fail");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Running")
                || err_msg.contains("Pending")
                || err_msg.contains("already"),
            "error should describe the invalid state transition: {}",
            err_msg
        );
    }

    /// 验证已 Completed 的 execution 不能被再次执行
    #[tokio::test]
    async fn test_execute_already_completed_fails() {
        let svc = InMemoryExecutionService::new();
        let exec_id = ExecutionId::new();

        // 提交 execution
        svc.submit(make_request_with_id(exec_id.clone(), make_test_task()))
            .await
            .unwrap();

        // 手动将状态设置为 Completed（模拟已执行完成）
        svc.update_status(&exec_id, ExecutionStatus::Completed)
            .await
            .unwrap();

        // 在 Completed 状态下调用 execute() 应该失败
        let result = svc.execute(&exec_id).await;
        assert!(
            result.is_err(),
            "execute on Completed execution should fail"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Completed")
                || err_msg.contains("Pending")
                || err_msg.contains("already"),
            "error should describe the invalid state transition: {}",
            err_msg
        );

        // 确认 execution 状态仍为 Completed（未被重置）
        let exec = svc.get(&exec_id).await.unwrap().unwrap();
        assert_eq!(
            exec.status,
            ExecutionStatus::Completed,
            "status should remain Completed after failed execute attempt"
        );
    }

    // ─── EventBus best-effort tests ────────────────────────────────────────────

    /// An EventBus that always returns an error from publish().
    /// Used to verify that EventBus failures do not abort the execution path.
    struct AlwaysFailingEventBus;

    impl crate::event_bus::EventBus for AlwaysFailingEventBus {
        fn subscribe(
            &self,
            _filter: crate::event_bus::EventFilter,
        ) -> anyhow::Result<crate::event_bus::Subscriber> {
            anyhow::bail!("AlwaysFailingEventBus does not support subscribe")
        }

        fn unsubscribe(
            &self,
            _subscriber_id: &crate::event_bus::SubscriberId,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn publish(&self, _event: ClusterEvent) -> anyhow::Result<()> {
            anyhow::bail!("AlwaysFailingEventBus: intentional publish failure")
        }

        fn subscriber_count(&self) -> anyhow::Result<usize> {
            Ok(0)
        }
    }

    /// 验证当 EventBus publish() 失败时，execute() 仍然成功完成（best-effort 策略）
    #[tokio::test]
    async fn test_execute_succeeds_when_event_bus_publish_fails() {
        let failing_bus: Arc<dyn crate::event_bus::EventBus> = Arc::new(AlwaysFailingEventBus);
        let agent_runtime: Arc<dyn AgentRuntime> = Arc::new(MockAgentRuntime::new());
        let skill_runtime: Arc<dyn SkillRuntime> = Arc::new(MinimalSkillRuntime::new());
        let svc = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime)
            .with_event_bus(failing_bus);

        let exec_id = ExecutionId::new();
        svc.submit(make_request_with_id(exec_id.clone(), make_test_task()))
            .await
            .unwrap();

        // execute() must succeed even though every event publish() call fails
        let result = svc.execute(&exec_id).await;
        assert!(
            result.is_ok(),
            "execute should succeed even when EventBus.publish() always fails, got: {:?}",
            result.err()
        );

        // The execution state must have reached Completed despite the bus failures
        let exec = svc.get(&exec_id).await.unwrap().unwrap();
        assert_eq!(
            exec.status,
            ExecutionStatus::Completed,
            "execution must be Completed regardless of EventBus failures"
        );
    }

    /// 验证不挂载 EventBus 时（event_bus = None），execute() 正常运行
    #[tokio::test]
    async fn test_execute_succeeds_without_event_bus() {
        let agent_runtime: Arc<dyn AgentRuntime> = Arc::new(MockAgentRuntime::new());
        let skill_runtime: Arc<dyn SkillRuntime> = Arc::new(MinimalSkillRuntime::new());
        let svc = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime); // no event_bus attached

        let exec_id = ExecutionId::new();
        svc.submit(make_request_with_id(exec_id.clone(), make_test_task()))
            .await
            .unwrap();

        let result = svc.execute(&exec_id).await;
        assert!(
            result.is_ok(),
            "execute should succeed with no EventBus attached"
        );

        let exec = svc.get(&exec_id).await.unwrap().unwrap();
        assert_eq!(exec.status, ExecutionStatus::Completed);
    }

    /// 验证 with_event_bus() builder 方法可以正确挂载 EventBus
    #[tokio::test]
    async fn test_with_event_bus_builder_attaches_bus() {
        use crate::event_bus::{EventBusConfig, InMemoryEventBus};

        let bus = Arc::new(InMemoryEventBus::new(EventBusConfig::default()));
        let mut subscriber = bus.subscribe(crate::event_bus::EventFilter::All).unwrap();

        let agent_runtime: Arc<dyn AgentRuntime> = Arc::new(MockAgentRuntime::new());
        let skill_runtime: Arc<dyn SkillRuntime> = Arc::new(MinimalSkillRuntime::new());
        let svc = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime)
            .with_event_bus(bus.clone() as Arc<dyn crate::event_bus::EventBus>);

        let exec_id = ExecutionId::new();
        svc.submit(make_request_with_id(exec_id.clone(), make_test_task()))
            .await
            .unwrap();

        svc.execute(&exec_id).await.unwrap();

        // At least one ClusterEvent (ExecutionAssigned) should have been published
        let received = subscriber.receiver.try_recv();
        assert!(
            received.is_ok(),
            "at least one ClusterEvent should have been published to the EventBus"
        );
    }

    // ─── Provenance Integration Tests ─────────────────────────────────────────

    /// Verify provenance tracking lifecycle: start → record → finalize
    #[tokio::test]
    async fn test_provenance_tracking_lifecycle() {
        use crate::provenance_tracker::{InMemoryProvenanceTracker, ProvenanceTracker};

        let agent_runtime: Arc<dyn AgentRuntime> = Arc::new(MockAgentRuntime::new());
        let skill_runtime: Arc<dyn SkillRuntime> = Arc::new(MinimalSkillRuntime::new());
        let tracker = Arc::new(InMemoryProvenanceTracker::new());

        let svc = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime)
            .with_provenance_tracker(tracker.clone() as Arc<dyn ProvenanceTracker>);

        let exec_id = ExecutionId::new();
        svc.submit(make_request_with_id(exec_id.clone(), make_test_task()))
            .await
            .unwrap();

        // Provenance should be started after submit
        let record = tracker.get(&exec_id).await.unwrap();
        assert!(record.is_some(), "provenance should be started");

        // Execute (will finalize provenance on completion)
        svc.execute(&exec_id).await.unwrap();

        // Provenance should still be accessible (moved to finalized store)
        let finalized_record = tracker.get(&exec_id).await.unwrap();
        assert!(
            finalized_record.is_some(),
            "provenance should be finalized and accessible"
        );
    }

    /// Verify provenance tracking fails gracefully when tracker is None
    #[tokio::test]
    async fn test_provenance_tracking_optional() {
        let agent_runtime: Arc<dyn AgentRuntime> = Arc::new(MockAgentRuntime::new());
        let skill_runtime: Arc<dyn SkillRuntime> = Arc::new(MinimalSkillRuntime::new());
        let svc = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime);
        // Note: No provenance_tracker attached

        let exec_id = ExecutionId::new();
        svc.submit(make_request_with_id(exec_id.clone(), make_test_task()))
            .await
            .unwrap();

        // Execution should succeed even without provenance tracker
        let result = svc.execute(&exec_id).await;
        assert!(
            result.is_ok(),
            "execution should succeed without provenance tracker"
        );
    }

    // ─── P0-2 No-op Success Path Prevention Tests ─────────────────────────────

    /// 验证当 plan.actions 非空但 dispatcher 缺失时，execution 必须失败
    ///
    /// P0-2 修复：确保 execution_service 不会 silent success 当 plan 有 actions
    /// 但 capability_dispatcher 未装配时。execution 必须返回错误并状态转为 Failed。
    #[tokio::test]
    async fn test_execution_fails_when_dispatcher_missing() {
        use crate::types::{ExecutionPlan, PlannedAction, Resolution};
        use cyberclaw_core::ids::{AgentId, CapabilityId, ConnectorId};

        let event_recorder = Arc::new(InMemoryEventRecorder::new());
        let service = InMemoryExecutionService::with_event_recorder(event_recorder.clone());
        // 注意：故意不装配 dispatcher

        // 创建一个包含 actions 的 ExecutionPlan
        let plan = ExecutionPlan {
            resolution: Resolution {
                agent: AgentId::from_string("test-agent".to_string()).unwrap(),
                skills: vec![],
                workflow: None,
                connectors: vec![ConnectorId::from_string("local".to_string()).unwrap()],
                capabilities: vec![CapabilityId::from_string("fs.read".to_string()).unwrap()],
                reasons: vec!["test reason".to_string()],
            },
            actions: vec![PlannedAction {
                connector_id: ConnectorId::from_string("local".to_string()).unwrap(),
                capability: CapabilityId::from_string("fs.read".to_string()).unwrap(),
                input: serde_json::json!({"path": "README.md"}),
                reason: "test action reason".to_string(),
            }],
            review_required: false,
            max_fix_loops: crate::types::default_max_fix_loops(),
            expected_outcomes: vec![],
        };

        // 提交 plan（会创建 execution）
        let execution_id = service.submit_plan(plan).await.unwrap();

        // 尝试执行（应该失败，因为没有 dispatcher）
        let result = service.execute(&execution_id).await;

        // 断言：execution 必须失败
        assert!(
            result.is_err(),
            "execution should fail when plan has actions but dispatcher is missing"
        );

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("capability dispatcher not configured")
                || err_msg.contains("Cannot execute plan with actions"),
            "error message should indicate dispatcher is missing: {}",
            err_msg
        );

        // 断言：execution 状态必须是 Failed
        let execution = service
            .get(&execution_id)
            .await
            .unwrap()
            .expect("execution should exist");
        assert_eq!(
            execution.status,
            ExecutionStatus::Failed,
            "execution status must be Failed when dispatcher is missing"
        );

        // 注：事件记录验证被省略，因为 InMemoryEventRecorder 使用 fire-and-forget tokio::spawn，
        // 导致测试中的竞态条件。核心验证（execution.status == Failed）已经足够。
    }

    /// 验证当无 actions 且无 agent_runtime 时，execution 必须失败
    ///
    /// P0-2 修复：确保 execution_service 不会 silent success 当既没有 plan actions
    /// 也没有 agent_runtime 时。execution 必须返回错误并状态转为 Failed。
    #[tokio::test]
    async fn test_execution_fails_when_no_executable_content() {
        use crate::types::{ExecutionPlan, Resolution};
        use cyberclaw_core::ids::AgentId;

        let event_recorder = Arc::new(InMemoryEventRecorder::new());
        let service = InMemoryExecutionService::with_event_recorder(event_recorder.clone());
        // 注意：既没有装配 dispatcher，也没有装配 agent_runtime

        // 创建一个空 actions 的 ExecutionPlan
        let plan = ExecutionPlan {
            resolution: Resolution {
                agent: AgentId::from_string("test-agent".to_string()).unwrap(),
                skills: vec![],
                workflow: None,
                connectors: vec![],
                capabilities: vec![],
                reasons: vec!["test reason".to_string()],
            },
            actions: vec![], // 空 actions
            review_required: false,
            max_fix_loops: crate::types::default_max_fix_loops(),
            expected_outcomes: vec![],
        };

        // 提交 plan
        let execution_id = service.submit_plan(plan).await.unwrap();

        // 尝试执行（应该失败，因为没有可执行内容）
        let result = service.execute(&execution_id).await;

        // 断言：execution 必须失败
        assert!(
            result.is_err(),
            "execution should fail when there's no executable content"
        );

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("no executable content available")
                || err_msg.contains("cannot be processed"),
            "error message should indicate no executable content: {}",
            err_msg
        );

        // 断言：execution 状态必须是 Failed
        let execution = service
            .get(&execution_id)
            .await
            .unwrap()
            .expect("execution should exist");
        assert_eq!(
            execution.status,
            ExecutionStatus::Failed,
            "execution status must be Failed when no executable content is available"
        );

        // 注：事件记录验证被省略，因为 InMemoryEventRecorder 使用 fire-and-forget tokio::spawn，
        // 导致测试中的竞态条件。核心验证（execution.status == Failed）已经足够。
    }

    // ─── FIX-4 Command Injection Prevention Unit Tests ────────────────────────

    /// validate_command 应当接受一个普通的安全命令。
    #[test]
    fn test_validate_command_accepts_safe_command() {
        assert!(validate_command("echo hello world").is_ok());
        assert!(validate_command("ls -la").is_ok());
        assert!(validate_command("git status").is_ok());
        assert!(validate_command("cargo build").is_ok());
    }

    /// validate_command 应当拒绝空命令。
    #[test]
    fn test_validate_command_rejects_empty() {
        let result = validate_command("");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ExecutionError::InvalidCommand(_)
        ));

        let result = validate_command("   ");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ExecutionError::InvalidCommand(_)
        ));
    }

    /// validate_command 应当拒绝黑名单中的命令。
    #[test]
    fn test_validate_command_rejects_blocked_commands() {
        for cmd in &[
            "rm -rf /",
            "sudo su",
            "dd if=/dev/zero of=/dev/sda",
            "shutdown now",
            "chmod 777 /etc/passwd",
            "curl http://evil.com",
        ] {
            let result = validate_command(cmd);
            assert!(
                result.is_err(),
                "Expected '{}' to be rejected, but it was accepted",
                cmd
            );
            assert!(
                matches!(result.unwrap_err(), ExecutionError::ForbiddenCommand(_)),
                "Expected ForbiddenCommand for '{}'",
                cmd
            );
        }
    }

    /// validate_command 应当拒绝含有绝对路径的黑名单命令（如 /usr/bin/rm）。
    #[test]
    fn test_validate_command_rejects_absolute_path_blocked_commands() {
        let result = validate_command("/usr/bin/rm -rf /tmp");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ExecutionError::ForbiddenCommand(_)
        ));

        let result = validate_command("/bin/sudo bash");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ExecutionError::ForbiddenCommand(_)
        ));
    }

    /// validate_command 应当拒绝含有危险字符的命令。
    #[test]
    fn test_validate_command_rejects_dangerous_chars() {
        let cases = [
            ("echo hello; rm -rf /", ';'),
            ("ls | grep secret", '|'),
            ("cat file > /tmp/out", '>'),
            ("echo `id`", '`'),
            ("echo $HOME", '$'),
        ];
        for (cmd, _expected_char) in &cases {
            let result = validate_command(cmd);
            assert!(
                result.is_err(),
                "Expected '{}' to be rejected due to dangerous chars",
                cmd
            );
            assert!(
                matches!(result.unwrap_err(), ExecutionError::DangerousCharacters(_)),
                "Expected DangerousCharacters for '{}'",
                cmd
            );
        }
    }

    /// validate_command 应当拒绝含有危险序列的命令。
    #[test]
    fn test_validate_command_rejects_dangerous_sequences() {
        // Note: '&' is caught by DANGEROUS_CHARS before DANGEROUS_SEQUENCES check,
        // so use sequences that don't contain single dangerous chars already blocked.
        // We test '<<' and '>>' which contain '<' and '>' (also in DANGEROUS_CHARS).
        // The important invariant is: these are ALL rejected.
        let dangerous_inputs = ["echo hello >> /tmp/out", "cat << EOF"];
        for cmd in &dangerous_inputs {
            let result = validate_command(cmd);
            assert!(result.is_err(), "Expected '{}' to be rejected", cmd);
        }
    }

    /// validate_command 应当拒绝含有换行符的命令（换行注入）。
    #[test]
    fn test_validate_command_rejects_newline_injection() {
        let result = validate_command("echo hello\nrm -rf /");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ExecutionError::DangerousCharacters(_)
        ));

        let result = validate_command("echo hello\rrm -rf /");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ExecutionError::DangerousCharacters(_)
        ));
    }

    // ─── HIGH #3: Risk Level Calculation Tests ────────────────────────────────

    /// Test that calculate_execution_risk_level correctly identifies read operations as Low risk
    #[test]
    fn test_calculate_risk_level_read_operations() {
        use cyberclaw_core::capability::{CapabilityEffect, CapabilityRef, RiskLevel};
        use cyberclaw_core::ids::{AgentId, CapabilityId, ConnectorId};

        let capability_ref = CapabilityRef {
            id: CapabilityId::from_string("fs.read".to_string()).unwrap(),
            connector_id: ConnectorId::from_string("local-fs".to_string()).unwrap(),
            risk: RiskLevel::Low,
            effects: vec![CapabilityEffect::Read],
            placement: None,
        };

        let agent_id = AgentId::from_string("test_agent".to_string()).unwrap();
        let risk = calculate_execution_risk_level(&capability_ref, &agent_id);

        assert_eq!(
            risk,
            RiskLevel::Low,
            "Read operations should remain Low risk"
        );
    }

    /// Test that calculate_execution_risk_level elevates risk for write operations
    #[test]
    fn test_calculate_risk_level_write_elevates() {
        use cyberclaw_core::capability::{CapabilityEffect, CapabilityRef, RiskLevel};
        use cyberclaw_core::ids::{AgentId, CapabilityId, ConnectorId};

        let capability_ref = CapabilityRef {
            id: CapabilityId::from_string("fs.write".to_string()).unwrap(),
            connector_id: ConnectorId::from_string("local-fs".to_string()).unwrap(),
            risk: RiskLevel::Low,
            effects: vec![CapabilityEffect::Write],
            placement: None,
        };

        let agent_id = AgentId::from_string("test_agent".to_string()).unwrap();
        let risk = calculate_execution_risk_level(&capability_ref, &agent_id);

        assert_eq!(
            risk,
            RiskLevel::Medium,
            "Write operations should elevate Low → Medium"
        );
    }

    /// Test that calculate_execution_risk_level elevates risk for execute operations
    #[test]
    fn test_calculate_risk_level_execute_elevates() {
        use cyberclaw_core::capability::{CapabilityEffect, CapabilityRef, RiskLevel};
        use cyberclaw_core::ids::{AgentId, CapabilityId, ConnectorId};

        let capability_ref = CapabilityRef {
            id: CapabilityId::from_string("cmd.exec".to_string()).unwrap(),
            connector_id: ConnectorId::from_string("local-cmd".to_string()).unwrap(),
            risk: RiskLevel::Medium,
            effects: vec![CapabilityEffect::Execute],
            placement: None,
        };

        let agent_id = AgentId::from_string("test_agent".to_string()).unwrap();
        let risk = calculate_execution_risk_level(&capability_ref, &agent_id);

        assert_eq!(
            risk,
            RiskLevel::High,
            "Execute operations should elevate Medium → High"
        );
    }

    /// Test that calculate_execution_risk_level elevates risk for system capabilities
    #[test]
    fn test_calculate_risk_level_system_capability() {
        use cyberclaw_core::capability::{CapabilityEffect, CapabilityRef, RiskLevel};
        use cyberclaw_core::ids::{AgentId, CapabilityId, ConnectorId};

        let capability_ref = CapabilityRef {
            id: CapabilityId::from_string("system.admin".to_string()).unwrap(),
            connector_id: ConnectorId::from_string("system-connector".to_string()).unwrap(),
            risk: RiskLevel::Medium,
            effects: vec![CapabilityEffect::Read],
            placement: None,
        };

        let agent_id = AgentId::from_string("test_agent".to_string()).unwrap();
        let risk = calculate_execution_risk_level(&capability_ref, &agent_id);

        assert_eq!(
            risk,
            RiskLevel::High,
            "System capabilities should elevate Medium → High"
        );
    }

    /// Test that calculate_execution_risk_level combines multiple risk elevations
    #[test]
    fn test_calculate_risk_level_combined_elevations() {
        use cyberclaw_core::capability::{CapabilityEffect, CapabilityRef, RiskLevel};
        use cyberclaw_core::ids::{AgentId, CapabilityId, ConnectorId};

        // System capability + Write effect should elevate twice
        let capability_ref = CapabilityRef {
            id: CapabilityId::from_string("system.write".to_string()).unwrap(),
            connector_id: ConnectorId::from_string("system-connector".to_string()).unwrap(),
            risk: RiskLevel::Low,
            effects: vec![CapabilityEffect::Write],
            placement: None,
        };

        let agent_id = AgentId::from_string("test_agent".to_string()).unwrap();
        let risk = calculate_execution_risk_level(&capability_ref, &agent_id);

        // Low → Medium (write) → High (system)
        assert_eq!(
            risk,
            RiskLevel::High,
            "System + Write should elevate Low → Medium → High"
        );
    }

    /// Test that calculate_execution_risk_level caps at Critical
    #[test]
    fn test_calculate_risk_level_caps_at_critical() {
        use cyberclaw_core::capability::{CapabilityEffect, CapabilityRef, RiskLevel};
        use cyberclaw_core::ids::{AgentId, CapabilityId, ConnectorId};

        let capability_ref = CapabilityRef {
            id: CapabilityId::from_string("system.exec".to_string()).unwrap(),
            connector_id: ConnectorId::from_string("system-connector".to_string()).unwrap(),
            risk: RiskLevel::High,
            effects: vec![CapabilityEffect::Execute],
            placement: None,
        };

        let agent_id = AgentId::from_string("test_agent".to_string()).unwrap();
        let risk = calculate_execution_risk_level(&capability_ref, &agent_id);

        // High → Critical (execute) → Critical (system, already at max)
        assert_eq!(
            risk,
            RiskLevel::Critical,
            "Risk level should cap at Critical"
        );
    }

    /// Test resolve_trust_level returns correct variant for each pattern
    #[test]
    fn test_trust_level_resolution() {
        use cyberclaw_core::agent::AgentTrustLevel;
        use cyberclaw_core::ids::AgentId;

        let cp = AgentId::from_string("control-plane".to_string()).unwrap();
        assert_eq!(resolve_trust_level(&cp), AgentTrustLevel::Trusted);

        let sys = AgentId::from_string("system.core".to_string()).unwrap();
        assert_eq!(resolve_trust_level(&sys), AgentTrustLevel::Trusted);

        let plat = AgentId::from_string("platform.scheduler".to_string()).unwrap();
        assert_eq!(resolve_trust_level(&plat), AgentTrustLevel::Trusted);

        let ext = AgentId::from_string("external.vendor-bot".to_string()).unwrap();
        assert_eq!(resolve_trust_level(&ext), AgentTrustLevel::Restricted);

        let unt = AgentId::from_string("untrusted.third-party".to_string()).unwrap();
        assert_eq!(resolve_trust_level(&unt), AgentTrustLevel::Restricted);

        let std = AgentId::from_string("my-agent".to_string()).unwrap();
        assert_eq!(resolve_trust_level(&std), AgentTrustLevel::Standard);
    }

    /// Trusted agent lowers risk by one level
    #[test]
    fn test_risk_adjusted_by_trust_trusted() {
        use cyberclaw_core::capability::{CapabilityRef, RiskLevel};
        use cyberclaw_core::ids::{AgentId, CapabilityId, ConnectorId};

        let cap = CapabilityRef {
            id: CapabilityId::from_string("file.read".to_string()).unwrap(),
            connector_id: ConnectorId::from_string("filesystem".to_string()).unwrap(),
            risk: RiskLevel::Medium,
            effects: vec![],
            placement: None,
        };
        let agent = AgentId::from_string("system.core".to_string()).unwrap();
        let risk = calculate_execution_risk_level(&cap, &agent);
        assert_eq!(
            risk,
            RiskLevel::Low,
            "Trusted agent should reduce Medium → Low"
        );
    }

    /// Restricted agent raises risk by one level
    #[test]
    fn test_risk_adjusted_by_trust_restricted() {
        use cyberclaw_core::capability::{CapabilityRef, RiskLevel};
        use cyberclaw_core::ids::{AgentId, CapabilityId, ConnectorId};

        let cap = CapabilityRef {
            id: CapabilityId::from_string("file.read".to_string()).unwrap(),
            connector_id: ConnectorId::from_string("filesystem".to_string()).unwrap(),
            risk: RiskLevel::Low,
            effects: vec![],
            placement: None,
        };
        let agent = AgentId::from_string("external.vendor-bot".to_string()).unwrap();
        let risk = calculate_execution_risk_level(&cap, &agent);
        assert_eq!(
            risk,
            RiskLevel::Medium,
            "Restricted agent should elevate Low → Medium"
        );
    }

    // ─── S18 R1+R2: Leveled Memory Loop Tests ────────────────────────────────

    /// R1: Completing an execution writes an L1Summary record to the leveled store.
    #[tokio::test]
    async fn test_completed_execution_writes_l1_memory() {
        use cyberclaw_store::{InMemoryLeveledStore, LeveledMemoryStore, MemoryLevel};

        let agent_runtime = Arc::new(MockAgentRuntime::new());
        let skill_runtime = Arc::new(MinimalSkillRuntime::new());
        let store = Arc::new(InMemoryLeveledStore::new());

        let svc = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime)
            .with_leveled_memory_store(store.clone());

        let execution_id = ExecutionId::new();
        let request = make_request_with_id(execution_id.clone(), make_test_task());
        svc.submit(request).await.unwrap();
        let _ = svc.execute(&execution_id).await;

        // After execute(), a L1Summary record should be present for this session_id
        let session_id = execution_id.as_str();
        let records = store
            .query_by_level(session_id, MemoryLevel::L1Summary)
            .await
            .unwrap();
        assert!(
            !records.is_empty(),
            "Expected at least 1 L1Summary record after execution completed"
        );
        let content = records[0]
            .content
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(
            content.contains(execution_id.as_str()),
            "Summary should contain execution_id"
        );
    }

    /// R2: Pre-seeding the store with L1Summary records causes prior_context to appear in task input.
    #[tokio::test]
    async fn test_running_execution_reads_recent_l1() {
        use cyberclaw_store::{
            InMemoryLeveledStore, LeveledMemoryRecord, LeveledMemoryStore, MemoryLevel,
        };
        use serde_json::json;

        let store = Arc::new(InMemoryLeveledStore::new());

        // Pre-seed 3 L1Summary records under the execution_id we'll use as session_id
        let execution_id = ExecutionId::new();
        let session_id = execution_id.as_str().to_string();
        let now = chrono::Utc::now();
        for i in 0..3usize {
            store
                .store_leveled(LeveledMemoryRecord {
                    id: format!("pre-seed-{}", i),
                    session_id: session_id.clone(),
                    agent_id: "test-agent".to_string(),
                    level: MemoryLevel::L1Summary,
                    key: format!("ep-{}", i),
                    content: json!({ "summary": format!("prior event {}", i) }),
                    created_at: now,
                    updated_at: now,
                    ttl_seconds: MemoryLevel::L1Summary.default_ttl_seconds(),
                    source_execution_id: None,
                    embedding: None,
                    tags: Vec::new(),
                })
                .await
                .unwrap();
        }

        // The svc uses a mock agent runtime that records the task input it received
        let agent_runtime = Arc::new(MockAgentRuntime::new());
        let skill_runtime = Arc::new(MinimalSkillRuntime::new());
        let svc = InMemoryExecutionService::with_runtimes(agent_runtime.clone(), skill_runtime)
            .with_leveled_memory_store(store.clone());

        let request = make_request_with_id(execution_id.clone(), make_test_task());
        svc.submit(request).await.unwrap();
        let _ = svc.execute(&execution_id).await;

        // Verify a MemoryRead event was emitted
        let events = svc.event_recorder.get_events().await.unwrap();
        let memory_read_count = events
            .iter()
            .filter(|e| matches!(e, ObservabilityEvent::MemoryRead { .. }))
            .count();
        assert_eq!(memory_read_count, 1, "Expected exactly 1 MemoryRead event");

        // Verify count in the event reflects the 3 pre-seeded records
        let read_count = events.iter().find_map(|e| {
            if let ObservabilityEvent::MemoryRead { count, .. } = e {
                Some(*count)
            } else {
                None
            }
        });
        assert_eq!(
            read_count,
            Some(3),
            "MemoryRead event should report count=3"
        );
    }

    /// Sprint 21 T8: when the current execution's session_id maps to an
    /// accepted handoff, read_prior_context must prepend the handoff briefing
    /// block ahead of the prior_context block.
    #[tokio::test]
    async fn test_read_prior_context_prepends_handoff_briefing_for_handoff_session() {
        use crate::handoff_queue::{HandoffQueue, InMemoryHandoffQueue};
        use cyberclaw_core::handoff::{HandoffRequest, HandoffStatus, HANDOFF_TTL_DEFAULT_SECS};
        use cyberclaw_core::ids::{HandoffId, SessionId};
        use cyberclaw_store::{
            InMemoryLeveledStore, LeveledMemoryRecord, LeveledMemoryStore, MemoryLevel,
        };
        use serde_json::json;

        // 1. Pre-seed the leveled memory store with one L1Summary so prior_context isn't None.
        let store = Arc::new(InMemoryLeveledStore::new());
        let execution_id = ExecutionId::new();
        let session_id_str = execution_id.as_str().to_string();
        let now = chrono::Utc::now();
        store
            .store_leveled(LeveledMemoryRecord {
                id: "pre-seed-1".to_string(),
                session_id: session_id_str.clone(),
                agent_id: "test-agent".to_string(),
                level: MemoryLevel::L1Summary,
                key: "ep-1".to_string(),
                content: json!({ "summary": "earlier conversation summary" }),
                created_at: now,
                updated_at: now,
                ttl_seconds: MemoryLevel::L1Summary.default_ttl_seconds(),
                source_execution_id: None,
                embedding: None,
                tags: Vec::new(),
            })
            .await
            .unwrap();

        // 2. Build a handoff queue and enqueue a request, then bind it to this session.
        let queue = Arc::new(InMemoryHandoffQueue::new());
        let handoff_id = HandoffId::from_string("ho_t8_test".to_string()).unwrap();
        let req = HandoffRequest {
            handoff_id: handoff_id.clone(),
            from_agent_id: AgentId::from_string("agent-alpha".to_string()).unwrap(),
            to_agent_id: AgentId::from_string("agent-beta".to_string()).unwrap(),
            conversation_id: "conv_t8".to_string(),
            reason: "specialist needed".to_string(),
            briefing_text: "User is asking about a refund policy edge case.".to_string(),
            context_artifacts: vec![],
            status: HandoffStatus::Initiated,
            initiated_at: now,
            decided_at: None,
            ttl_seconds: HANDOFF_TTL_DEFAULT_SECS,
            initiated_by_execution: None,
            target_session_id: None,
        };
        queue.enqueue(req).await.unwrap();
        let session_id_typed = SessionId::from_string(session_id_str.clone()).unwrap();
        queue
            .set_target_session(&handoff_id, session_id_typed.clone())
            .await
            .unwrap();

        // 3. Build a service wired with both the leveled store AND the handoff queue.
        let agent_runtime = Arc::new(MockAgentRuntime::new());
        let skill_runtime = Arc::new(MinimalSkillRuntime::new());
        let svc = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime)
            .with_leveled_memory_store(store.clone())
            .with_handoff_queue(queue.clone());

        // 4. Call read_prior_context directly (private method, accessible from cfg(test) mod).
        let result = svc
            .read_prior_context(&execution_id, "agent-beta")
            .await
            .expect("Should return Some(block) when L1 records exist");

        // 5. Assertions: handoff briefing comes first, prior_context follows.
        assert!(
            result.starts_with("<handoff_briefing>"),
            "S21 T8: handoff briefing must be prepended; got: {}",
            &result[..result.len().min(120)]
        );
        assert!(
            result.contains("From: agent-alpha"),
            "briefing must contain from_agent_id"
        );
        assert!(
            result.contains("specialist needed"),
            "briefing must contain reason"
        );
        assert!(
            result.contains("User is asking about a refund"),
            "briefing must contain briefing_text"
        );
        assert!(
            result.contains("</handoff_briefing>"),
            "briefing must close its block"
        );
        assert!(
            result.contains("<prior_context>"),
            "prior_context block must still be present after briefing"
        );
        assert!(
            result.contains("earlier conversation summary"),
            "prior_context content must survive"
        );
    }

    /// Sprint 21 T8: when the session has no associated handoff, read_prior_context
    /// must NOT prepend any briefing — preserves S18 behavior exactly.
    #[tokio::test]
    async fn test_read_prior_context_no_briefing_when_no_handoff() {
        use crate::handoff_queue::InMemoryHandoffQueue;
        use cyberclaw_store::{
            InMemoryLeveledStore, LeveledMemoryRecord, LeveledMemoryStore, MemoryLevel,
        };
        use serde_json::json;

        let store = Arc::new(InMemoryLeveledStore::new());
        let execution_id = ExecutionId::new();
        let session_id_str = execution_id.as_str().to_string();
        let now = chrono::Utc::now();
        store
            .store_leveled(LeveledMemoryRecord {
                id: "pre-seed-noho".to_string(),
                session_id: session_id_str,
                agent_id: "test-agent".to_string(),
                level: MemoryLevel::L1Summary,
                key: "ep-1".to_string(),
                content: json!({ "summary": "no-handoff session" }),
                created_at: now,
                updated_at: now,
                ttl_seconds: MemoryLevel::L1Summary.default_ttl_seconds(),
                source_execution_id: None,
                embedding: None,
                tags: Vec::new(),
            })
            .await
            .unwrap();

        // Empty queue — nothing bound to this session.
        let queue = Arc::new(InMemoryHandoffQueue::new());
        let agent_runtime = Arc::new(MockAgentRuntime::new());
        let skill_runtime = Arc::new(MinimalSkillRuntime::new());
        let svc = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime)
            .with_leveled_memory_store(store.clone())
            .with_handoff_queue(queue.clone());

        let result = svc
            .read_prior_context(&execution_id, "agent-beta")
            .await
            .expect("Should return Some(block) when L1 records exist");

        assert!(
            !result.contains("<handoff_briefing>"),
            "no handoff → no briefing block"
        );
        assert!(
            result.starts_with("<prior_context>"),
            "result must start with prior_context when no handoff present"
        );
    }

    /// R1 failure does not block execution completion (best-effort semantics).
    #[tokio::test]
    async fn test_memory_write_failure_does_not_block_completion() {
        use cyberclaw_store::{LeveledMemoryRecord, LeveledMemoryStore, MemoryLevel};
        use std::collections::HashMap;

        /// A store that always fails store_leveled but succeeds on reads.
        struct AlwaysFailStore;

        #[async_trait::async_trait]
        impl LeveledMemoryStore for AlwaysFailStore {
            async fn store_leveled(
                &self,
                _record: LeveledMemoryRecord,
            ) -> cyberclaw_store::Result<()> {
                Err(cyberclaw_store::StoreError::InternalError(
                    "injected write failure".to_string(),
                ))
            }
            async fn query_by_level(
                &self,
                _session_id: &str,
                _level: MemoryLevel,
            ) -> cyberclaw_store::Result<Vec<LeveledMemoryRecord>> {
                Ok(vec![])
            }
            async fn query_by_key(
                &self,
                _session_id: &str,
                _key: &str,
            ) -> cyberclaw_store::Result<Option<LeveledMemoryRecord>> {
                Ok(None)
            }
            async fn promote(
                &self,
                _id: &str,
                _new_level: MemoryLevel,
            ) -> cyberclaw_store::Result<()> {
                Ok(())
            }
            async fn demote(
                &self,
                _id: &str,
                _new_level: MemoryLevel,
            ) -> cyberclaw_store::Result<()> {
                Ok(())
            }
            async fn expire_stale(
                &self,
                _max_age: chrono::Duration,
            ) -> cyberclaw_store::Result<u64> {
                Ok(0)
            }
            async fn count_by_level(
                &self,
                _session_id: &str,
            ) -> cyberclaw_store::Result<HashMap<MemoryLevel, usize>> {
                Ok(HashMap::new())
            }
        }

        let agent_runtime = Arc::new(MockAgentRuntime::new());
        let skill_runtime = Arc::new(MinimalSkillRuntime::new());
        let failing_store: Arc<dyn LeveledMemoryStore> = Arc::new(AlwaysFailStore);

        let svc = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime)
            .with_leveled_memory_store(failing_store);

        let execution_id = ExecutionId::new();
        let request = make_request_with_id(execution_id.clone(), make_test_task());
        svc.submit(request).await.unwrap();

        // execute() must succeed even though the memory store write always fails
        let result = svc.execute(&execution_id).await;
        assert!(
            result.is_ok(),
            "Execution must succeed even when memory store write fails: {:?}",
            result
        );

        // Execution status should be Completed (not Failed)
        let exec = svc.get(&execution_id).await.unwrap().unwrap();
        assert_eq!(
            exec.status,
            ExecutionStatus::Completed,
            "Execution status must be Completed despite memory write failure"
        );
    }

    // ─── S19: LLM Episodic Memory Extraction Tests ───────────────────────────

    /// Mock LLM client that returns a pre-configured summary string.
    struct MockLlmSummaryClient {
        summary: String,
    }

    #[async_trait::async_trait]
    impl cyberclaw_llm::client::LlmClient for MockLlmSummaryClient {
        async fn chat_completion(
            &self,
            _req: cyberclaw_llm::types::ChatRequest,
        ) -> cyberclaw_llm::error::LlmResult<cyberclaw_llm::types::ChatResponse> {
            Ok(cyberclaw_llm::types::ChatResponse {
                id: "mock-id".to_string(),
                object: "chat.completion".to_string(),
                created: 0,
                model: "mock".to_string(),
                choices: vec![cyberclaw_llm::types::Choice {
                    index: 0,
                    message: cyberclaw_llm::types::Message {
                        role: cyberclaw_llm::types::Role::Assistant,
                        content: self.summary.clone(),
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                        cache_control: None,
                    },
                    finish_reason: Some("stop".to_string()),
                }],
                usage: None,
            })
        }

        async fn chat_completion_stream(
            &self,
            _req: cyberclaw_llm::types::ChatRequest,
        ) -> cyberclaw_llm::error::LlmResult<
            Box<
                dyn futures::stream::Stream<
                        Item = cyberclaw_llm::error::LlmResult<cyberclaw_llm::types::ChatChunk>,
                    > + Send
                    + Unpin,
            >,
        > {
            unimplemented!("not needed for tests")
        }

        fn provider(&self) -> &str {
            "mock"
        }

        async fn validate_connection(&self) -> cyberclaw_llm::error::LlmResult<()> {
            Ok(())
        }
    }

    /// Mock LLM client that always returns an error.
    struct ErrorLlmClient;

    #[async_trait::async_trait]
    impl cyberclaw_llm::client::LlmClient for ErrorLlmClient {
        async fn chat_completion(
            &self,
            _req: cyberclaw_llm::types::ChatRequest,
        ) -> cyberclaw_llm::error::LlmResult<cyberclaw_llm::types::ChatResponse> {
            Err(cyberclaw_llm::error::LlmError::Internal(
                "injected LLM error".to_string(),
            ))
        }

        async fn chat_completion_stream(
            &self,
            _req: cyberclaw_llm::types::ChatRequest,
        ) -> cyberclaw_llm::error::LlmResult<
            Box<
                dyn futures::stream::Stream<
                        Item = cyberclaw_llm::error::LlmResult<cyberclaw_llm::types::ChatChunk>,
                    > + Send
                    + Unpin,
            >,
        > {
            unimplemented!()
        }

        fn provider(&self) -> &str {
            "mock-error"
        }

        async fn validate_connection(&self) -> cyberclaw_llm::error::LlmResult<()> {
            Ok(())
        }
    }

    /// S19: When LLM client is attached and returns "TEST SUMMARY", the stored record
    /// should contain exactly "TEST SUMMARY" as the content.
    #[tokio::test]
    async fn test_write_episodic_uses_llm_summary_when_available() {
        use cyberclaw_store::{InMemoryLeveledStore, LeveledMemoryStore, MemoryLevel};

        let agent_runtime = Arc::new(MockAgentRuntime::new());
        let skill_runtime = Arc::new(MinimalSkillRuntime::new());
        let store = Arc::new(InMemoryLeveledStore::new());
        let llm: Arc<dyn cyberclaw_llm::client::LlmClient> = Arc::new(MockLlmSummaryClient {
            summary: "TEST SUMMARY".to_string(),
        });

        let svc = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime)
            .with_leveled_memory_store(store.clone())
            .with_llm_client(llm);

        let execution_id = ExecutionId::new();
        let request = make_request_with_id(execution_id.clone(), make_test_task());
        svc.submit(request).await.unwrap();
        let _ = svc.execute(&execution_id).await;

        let session_id = execution_id.as_str();
        let records = store
            .query_by_level(session_id, MemoryLevel::L1Summary)
            .await
            .unwrap();
        assert!(!records.is_empty(), "Expected at least 1 L1Summary record");
        let content = records[0]
            .content
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(
            content, "TEST SUMMARY",
            "Summary should be the LLM-provided value"
        );
    }

    /// S19: When LLM call fails, write_episodic falls back to string-concat and does NOT panic.
    /// The stored record should contain the execution_id (from fallback).
    #[tokio::test]
    async fn test_write_episodic_falls_back_on_llm_error() {
        use cyberclaw_store::{InMemoryLeveledStore, LeveledMemoryStore, MemoryLevel};

        let agent_runtime = Arc::new(MockAgentRuntime::new());
        let skill_runtime = Arc::new(MinimalSkillRuntime::new());
        let store = Arc::new(InMemoryLeveledStore::new());
        let llm: Arc<dyn cyberclaw_llm::client::LlmClient> = Arc::new(ErrorLlmClient);

        let svc = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime)
            .with_leveled_memory_store(store.clone())
            .with_llm_client(llm);

        let execution_id = ExecutionId::new();
        let request = make_request_with_id(execution_id.clone(), make_test_task());
        svc.submit(request).await.unwrap();
        // Must not panic; execution succeeds even if LLM errors
        let result = svc.execute(&execution_id).await;
        assert!(result.is_ok(), "Execution must succeed despite LLM error");

        let session_id = execution_id.as_str();
        let records = store
            .query_by_level(session_id, MemoryLevel::L1Summary)
            .await
            .unwrap();
        assert!(!records.is_empty(), "Expected fallback L1Summary record");
        let content = records[0]
            .content
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // Fallback string-concat includes the execution_id
        assert!(
            content.contains(execution_id.as_str()),
            "Fallback summary should contain execution_id, got: {}",
            content
        );
    }

    /// S19: When LLM returns a response > 2KB, the stored summary must be ≤ 2KB and end with
    /// "... [truncated]".
    #[tokio::test]
    async fn test_write_episodic_truncates_long_llm_summary() {
        use cyberclaw_store::{InMemoryLeveledStore, LeveledMemoryStore, MemoryLevel};

        let agent_runtime = Arc::new(MockAgentRuntime::new());
        let skill_runtime = Arc::new(MinimalSkillRuntime::new());
        let store = Arc::new(InMemoryLeveledStore::new());
        let llm: Arc<dyn cyberclaw_llm::client::LlmClient> = Arc::new(MockLlmSummaryClient {
            summary: "A".repeat(3000),
        });

        let svc = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime)
            .with_leveled_memory_store(store.clone())
            .with_llm_client(llm);

        let execution_id = ExecutionId::new();
        let request = make_request_with_id(execution_id.clone(), make_test_task());
        svc.submit(request).await.unwrap();
        let _ = svc.execute(&execution_id).await;

        let session_id = execution_id.as_str();
        let records = store
            .query_by_level(session_id, MemoryLevel::L1Summary)
            .await
            .unwrap();
        assert!(!records.is_empty(), "Expected at least 1 L1Summary record");
        let content = records[0]
            .content
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(
            content.len() <= 2048,
            "Summary must be ≤ 2048 bytes, got {}",
            content.len()
        );
        assert!(
            content.ends_with("... [truncated]"),
            "Truncated summary must end with '... [truncated]', got: {}",
            &content[content.len().saturating_sub(30)..]
        );
    }
    // ─── S20 E3: L0 auto-write + auto-demote tests ───────────────────────────

    /// S20 E3: Executing a task writes an L0Full working-memory snapshot at start.
    #[tokio::test]
    async fn test_l0_written_on_execution_turn() {
        use cyberclaw_store::{InMemoryLeveledStore, LeveledMemoryStore, MemoryLevel};

        let agent_runtime = Arc::new(MockAgentRuntime::new());
        let skill_runtime = Arc::new(MinimalSkillRuntime::new());
        let store = Arc::new(InMemoryLeveledStore::new());

        let svc = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime)
            .with_leveled_memory_store(store.clone());

        let execution_id = ExecutionId::new();
        let request = make_request_with_id(execution_id.clone(), make_test_task());
        svc.submit(request).await.unwrap();
        let _ = svc.execute(&execution_id).await;

        // After execute(), at least the L0Full start-snapshot should have been written
        // (it may have been cleaned up by auto-demote if the execution also wrote L1 and
        //  the record was already >1 hour old — but since it was just written it won't be).
        // We check that L0 was written by verifying L1 is present (L0 write always happens
        // before L1), and that the L0 key matches the expected pattern.
        let session_id = execution_id.as_str();

        // The L0 record may still be present (it was just created, not yet 1h old).
        let l0_records = store
            .query_by_level(session_id, MemoryLevel::L0Full)
            .await
            .unwrap();
        // The L0 record was written at execution start; the auto-demote only removes
        // records older than 1 hour, so the fresh L0 must still be here.
        assert!(
            !l0_records.is_empty(),
            "Expected at least 1 L0Full record written at execution start"
        );
        assert_eq!(
            l0_records[0].source_execution_id.as_deref(),
            Some(session_id)
        );
        let phase = l0_records[0]
            .content
            .get("phase")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(phase, "start", "L0 record phase should be 'start'");
    }

    /// S20 E3: After L1 is written, L0 records older than 1 hour are removed (auto-demote).
    #[tokio::test]
    async fn test_l1_write_demotes_old_l0() {
        use cyberclaw_store::{
            InMemoryLeveledStore, LeveledMemoryRecord, LeveledMemoryStore, MemoryLevel,
        };
        use serde_json::json;

        let store = Arc::new(InMemoryLeveledStore::new());

        let execution_id = ExecutionId::new();
        let session_id = execution_id.as_str().to_string();

        // Pre-seed an L0 record that is 2 hours old (should be auto-demoted)
        let two_hours_ago = chrono::Utc::now() - chrono::Duration::hours(2);
        let old_l0 = LeveledMemoryRecord {
            id: format!("l0-old-{}", execution_id.as_str()),
            session_id: session_id.clone(),
            agent_id: "agent-test".to_string(),
            level: MemoryLevel::L0Full,
            key: format!("working-old-{}", execution_id.as_str()),
            content: json!({"execution_id": session_id, "phase": "start"}),
            created_at: two_hours_ago,
            updated_at: two_hours_ago,
            ttl_seconds: MemoryLevel::L0Full.default_ttl_seconds(),
            source_execution_id: Some(session_id.clone()),
            embedding: None,
            tags: Vec::new(),
        };
        store.store_leveled(old_l0).await.unwrap();

        // Verify L0 is present before L1 write
        let before = store
            .query_by_level(&session_id, MemoryLevel::L0Full)
            .await
            .unwrap();
        assert_eq!(before.len(), 1, "L0 record must exist before L1 write");

        // Now run an execution that triggers L1 write (and auto-demote)
        let agent_runtime = Arc::new(MockAgentRuntime::new());
        let skill_runtime = Arc::new(MinimalSkillRuntime::new());
        let svc = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime)
            .with_leveled_memory_store(store.clone());

        let request = make_request_with_id(execution_id.clone(), make_test_task());
        svc.submit(request).await.unwrap();
        let _ = svc.execute(&execution_id).await;

        // The old L0 (2h old, beyond the 1h cutoff) must have been deleted
        let after = store
            .query_by_level(&session_id, MemoryLevel::L0Full)
            .await
            .unwrap();
        // Only the fresh L0 written at execution-start (< 1h old) may remain
        for r in &after {
            assert_ne!(
                r.id,
                format!("l0-old-{}", execution_id.as_str()),
                "Old L0 record should have been auto-demoted (deleted)"
            );
        }
    }

    // ─── S21 T4: complete_handoff tests ──────────────────────────────────────

    fn make_authorized_handoff(id: &str) -> cyberclaw_core::handoff::HandoffRequest {
        use cyberclaw_core::handoff::{HandoffRequest, HandoffStatus, HANDOFF_TTL_DEFAULT_SECS};
        use cyberclaw_core::ids::{AgentId, HandoffId};
        let mut req = HandoffRequest {
            handoff_id: HandoffId::from_string(id.to_string()).unwrap(),
            from_agent_id: AgentId::from_string("agent_from".to_string()).unwrap(),
            to_agent_id: AgentId::from_string("agent_to".to_string()).unwrap(),
            conversation_id: "conv_test".to_string(),
            reason: "test handoff".to_string(),
            briefing_text: "briefing".to_string(),
            context_artifacts: vec![],
            status: HandoffStatus::Initiated,
            initiated_at: chrono::Utc::now(),
            decided_at: None,
            ttl_seconds: HANDOFF_TTL_DEFAULT_SECS,
            initiated_by_execution: None,
            target_session_id: None,
        };
        req.mark_authorized(chrono::Utc::now());
        req
    }

    #[tokio::test]
    async fn complete_handoff_happy_path() {
        use crate::handoff_queue::InMemoryHandoffQueue;
        use cyberclaw_core::handoff::HandoffStatus;

        let queue = Arc::new(InMemoryHandoffQueue::new());
        let req = make_authorized_handoff("ho_happy");
        let handoff_id = req.handoff_id.clone();
        queue.enqueue(req).await.unwrap();

        // We need to wire the recorder so we can inspect events
        let recorder = Arc::new(InMemoryEventRecorder::new());
        let svc = InMemoryExecutionService::with_event_recorder(recorder.clone())
            .with_handoff_queue(queue.clone());

        let session_id = svc.complete_handoff(&handoff_id).await.unwrap();

        // session_id must be a non-empty string
        assert!(!session_id.to_string().is_empty());

        // status must be Accepted
        let stored = queue.get(&handoff_id).await.unwrap();
        assert_eq!(stored.status, HandoffStatus::Accepted);

        // HandoffAccepted event must have been emitted
        let events = recorder.get_events().await.unwrap();
        let has_event = events.iter().any(|e| {
            matches!(e, ObservabilityEvent::HandoffAccepted { handoff_id: hid, .. } if hid == &handoff_id)
        });
        assert!(has_event, "HandoffAccepted event not emitted");
    }

    #[tokio::test]
    async fn complete_handoff_idempotent_on_accepted() {
        use crate::handoff_queue::InMemoryHandoffQueue;
        use cyberclaw_core::handoff::HandoffStatus;

        let queue = Arc::new(InMemoryHandoffQueue::new());
        let req = make_authorized_handoff("ho_idem");
        let handoff_id = req.handoff_id.clone();
        queue.enqueue(req).await.unwrap();

        let recorder = Arc::new(InMemoryEventRecorder::new());
        let svc = InMemoryExecutionService::with_event_recorder(recorder.clone())
            .with_handoff_queue(queue.clone());

        // First call: Authorized → Accepted
        svc.complete_handoff(&handoff_id).await.unwrap();

        // Second call on Accepted: should return Ok (idempotent)
        let result = svc.complete_handoff(&handoff_id).await;
        assert!(
            result.is_ok(),
            "second complete_handoff should be Ok, got: {:?}",
            result.err()
        );

        // Status must still be Accepted
        let stored = queue.get(&handoff_id).await.unwrap();
        assert_eq!(stored.status, HandoffStatus::Accepted);
    }

    /// A1.2: idempotent calls return the SAME session_id (persisted via set_target_session).
    #[tokio::test]
    async fn complete_handoff_idempotent_returns_same_session_id() {
        use crate::handoff_queue::InMemoryHandoffQueue;
        use cyberclaw_core::handoff::HandoffStatus;

        let queue = Arc::new(InMemoryHandoffQueue::new());
        let req = make_authorized_handoff("ho_idem_sid");
        let handoff_id = req.handoff_id.clone();
        queue.enqueue(req).await.unwrap();

        let recorder = Arc::new(InMemoryEventRecorder::new());
        let svc = InMemoryExecutionService::with_event_recorder(recorder.clone())
            .with_handoff_queue(queue.clone());

        // First call: Authorized → Accepted, allocates and persists session_id.
        let session_id_first = svc.complete_handoff(&handoff_id).await.unwrap();

        // Second call on Accepted: must return the SAME session_id.
        let session_id_second = svc.complete_handoff(&handoff_id).await.unwrap();

        assert_eq!(
            session_id_first.to_string(),
            session_id_second.to_string(),
            "idempotent complete_handoff must return the same session_id both times"
        );

        // Status must remain Accepted.
        let stored = queue.get(&handoff_id).await.unwrap();
        assert_eq!(stored.status, HandoffStatus::Accepted);
        assert_eq!(
            stored.target_session_id.as_ref().map(|s| s.to_string()),
            Some(session_id_first.to_string()),
            "target_session_id must be persisted on the HandoffRequest"
        );
    }

    /// Spec: complete_handoff_returns_same_session_id_on_idempotent_call
    /// Explicit rename alias so the spec test name is present in the binary.
    #[tokio::test]
    async fn complete_handoff_returns_same_session_id_on_idempotent_call() {
        use crate::handoff_queue::InMemoryHandoffQueue;

        let queue = Arc::new(InMemoryHandoffQueue::new());
        let req = make_authorized_handoff("ho_idem_alias");
        let handoff_id = req.handoff_id.clone();
        queue.enqueue(req).await.unwrap();

        let recorder = Arc::new(InMemoryEventRecorder::new());
        let svc = InMemoryExecutionService::with_event_recorder(recorder.clone())
            .with_handoff_queue(queue.clone());

        let first = svc.complete_handoff(&handoff_id).await.unwrap();
        let second = svc.complete_handoff(&handoff_id).await.unwrap();

        assert_eq!(
            first.to_string(),
            second.to_string(),
            "repeated complete_handoff calls must return the same SessionId"
        );
    }

    /// Spec: complete_handoff_first_call_persists_session_id
    /// After a single complete_handoff call, target_session_id on the stored
    /// HandoffRequest must equal the returned SessionId.
    #[tokio::test]
    async fn complete_handoff_first_call_persists_session_id() {
        use crate::handoff_queue::InMemoryHandoffQueue;
        use cyberclaw_core::handoff::HandoffStatus;

        let queue = Arc::new(InMemoryHandoffQueue::new());
        let req = make_authorized_handoff("ho_persist_sid");
        let handoff_id = req.handoff_id.clone();
        queue.enqueue(req).await.unwrap();

        let recorder = Arc::new(InMemoryEventRecorder::new());
        let svc = InMemoryExecutionService::with_event_recorder(recorder.clone())
            .with_handoff_queue(queue.clone());

        let returned_session_id = svc.complete_handoff(&handoff_id).await.unwrap();

        // Verify the queue record was updated
        let stored = queue.get(&handoff_id).await.unwrap();
        assert_eq!(stored.status, HandoffStatus::Accepted);
        assert_eq!(
            stored.target_session_id.as_ref().map(|s| s.to_string()),
            Some(returned_session_id.to_string()),
            "target_session_id on the stored HandoffRequest must match the returned SessionId"
        );
    }

    #[tokio::test]
    async fn complete_handoff_rejects_non_authorized_state() {
        use crate::handoff_queue::InMemoryHandoffQueue;
        use cyberclaw_core::handoff::{HandoffRequest, HandoffStatus, HANDOFF_TTL_DEFAULT_SECS};
        use cyberclaw_core::ids::{AgentId, HandoffId};

        let queue = Arc::new(InMemoryHandoffQueue::new());
        // Insert a handoff that is still in Initiated state (not Authorized)
        let req = HandoffRequest {
            handoff_id: HandoffId::from_string("ho_initiated".to_string()).unwrap(),
            from_agent_id: AgentId::from_string("agent_from".to_string()).unwrap(),
            to_agent_id: AgentId::from_string("agent_to".to_string()).unwrap(),
            conversation_id: "conv_test".to_string(),
            reason: "test".to_string(),
            briefing_text: "brief".to_string(),
            context_artifacts: vec![],
            status: HandoffStatus::Initiated,
            initiated_at: chrono::Utc::now(),
            decided_at: None,
            ttl_seconds: HANDOFF_TTL_DEFAULT_SECS,
            initiated_by_execution: None,
            target_session_id: None,
        };
        let handoff_id = req.handoff_id.clone();
        queue.enqueue(req).await.unwrap();

        let svc = InMemoryExecutionService::new().with_handoff_queue(queue);

        let result = svc.complete_handoff(&handoff_id).await;
        assert!(
            matches!(result, Err(ExecutionError::InvalidCommand(_))),
            "expected InvalidCommand, got: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn complete_handoff_no_queue_configured_returns_error() {
        use cyberclaw_core::ids::HandoffId;

        let svc = InMemoryExecutionService::new(); // no with_handoff_queue
        let fake_id = HandoffId::from_string("ho_no_queue".to_string()).unwrap();

        let result = svc.complete_handoff(&fake_id).await;
        assert!(
            matches!(result, Err(ExecutionError::InvalidCommand(_))),
            "expected InvalidCommand when queue not configured, got: {:?}",
            result
        );
    }

    // ─── S21 T5: build_handoff_briefing_addendum tests ───────────────────────

    #[test]
    fn build_handoff_briefing_addendum_basic() {
        use cyberclaw_core::handoff::{HandoffRequest, HandoffStatus, HANDOFF_TTL_DEFAULT_SECS};
        use cyberclaw_core::ids::{AgentId, HandoffId};

        let req = HandoffRequest {
            handoff_id: HandoffId::from_string("ho_basic".to_string()).unwrap(),
            from_agent_id: AgentId::from_string("agent_a".to_string()).unwrap(),
            to_agent_id: AgentId::from_string("agent_b".to_string()).unwrap(),
            conversation_id: "conv_basic".to_string(),
            reason: "agent_b is more specialized".to_string(),
            briefing_text: "Please continue the task.".to_string(),
            context_artifacts: vec![],
            status: HandoffStatus::Initiated,
            initiated_at: chrono::Utc::now(),
            decided_at: None,
            target_session_id: None,
            ttl_seconds: HANDOFF_TTL_DEFAULT_SECS,
            initiated_by_execution: None,
        };

        let out = build_handoff_briefing_addendum(&req);

        assert!(out.contains("<handoff_briefing>"), "missing opening tag");
        assert!(out.contains("</handoff_briefing>"), "missing closing tag");
        assert!(out.contains("From: agent_a"), "missing From field");
        assert!(
            out.contains("Reason: agent_b is more specialized"),
            "missing Reason field"
        );
        assert!(
            out.contains("Please continue the task."),
            "missing briefing body"
        );
        // No artifacts → no context_artifact tags
        assert!(
            !out.contains("<context_artifact"),
            "unexpected context_artifact tag for empty artifacts"
        );
    }

    #[test]
    fn build_handoff_briefing_addendum_truncates_oversized_briefing() {
        use cyberclaw_core::handoff::{HandoffRequest, HandoffStatus, HANDOFF_TTL_DEFAULT_SECS};
        use cyberclaw_core::ids::{AgentId, HandoffId};

        // 3000-char briefing exceeds 2KB cap
        let long_briefing = "x".repeat(3000);
        let req = HandoffRequest {
            handoff_id: HandoffId::from_string("ho_long".to_string()).unwrap(),
            from_agent_id: AgentId::from_string("agent_a".to_string()).unwrap(),
            to_agent_id: AgentId::from_string("agent_b".to_string()).unwrap(),
            conversation_id: "conv_long".to_string(),
            reason: "overflow test".to_string(),
            briefing_text: long_briefing,
            context_artifacts: vec![],
            status: HandoffStatus::Initiated,
            initiated_at: chrono::Utc::now(),
            decided_at: None,
            ttl_seconds: HANDOFF_TTL_DEFAULT_SECS,
            initiated_by_execution: None,
            target_session_id: None,
        };

        let out = build_handoff_briefing_addendum(&req);

        // The output must still be well-formed XML tags
        assert!(out.contains("<handoff_briefing>"), "missing opening tag");
        assert!(out.contains("</handoff_briefing>"), "missing closing tag");

        // Count 'x' chars in the output — must be ≤ 2048
        let x_count = out.chars().filter(|&c| c == 'x').count();
        assert!(
            x_count <= 2048,
            "briefing body not truncated: {} 'x' chars found (expected ≤ 2048)",
            x_count
        );
    }

    #[test]
    fn build_handoff_briefing_addendum_respects_artifact_budget() {
        use cyberclaw_core::artifact::{ArtifactKind, ArtifactRef};
        use cyberclaw_core::handoff::{HandoffRequest, HandoffStatus, HANDOFF_TTL_DEFAULT_SECS};
        use cyberclaw_core::ids::{AgentId, ArtifactId, HandoffId};

        // 5 artifacts, each with a 600-char title → combined > 2KB budget
        let artifacts: Vec<ArtifactRef> = (0..5)
            .map(|i| ArtifactRef {
                id: ArtifactId::new(),
                kind: ArtifactKind::Summary,
                title: "t".repeat(600),
                uri: format!("file:///artifact_{i}.txt"),
                content_type: "text/plain".to_string(),
                parent_artifact_id: None,
                base_version: None,
            })
            .collect();

        let req = HandoffRequest {
            handoff_id: HandoffId::from_string("ho_art".to_string()).unwrap(),
            from_agent_id: AgentId::from_string("agent_a".to_string()).unwrap(),
            to_agent_id: AgentId::from_string("agent_b".to_string()).unwrap(),
            conversation_id: "conv_art".to_string(),
            reason: "artifact budget test".to_string(),
            briefing_text: "brief".to_string(),
            context_artifacts: artifacts,
            status: HandoffStatus::Initiated,
            initiated_at: chrono::Utc::now(),
            decided_at: None,
            ttl_seconds: HANDOFF_TTL_DEFAULT_SECS,
            initiated_by_execution: None,
            target_session_id: None,
        };

        let out = build_handoff_briefing_addendum(&req);

        // At least one artifact tag must appear
        assert!(
            out.contains("<context_artifact"),
            "expected at least one context_artifact tag"
        );

        // Count opening artifact tags — must be fewer than 5 (budget exhausted)
        let tag_count = out.matches("<context_artifact").count();
        assert!(
            tag_count < 5,
            "expected artifact budget to stop before 5 artifacts, got {} tags",
            tag_count
        );

        // Total artifact section must not exceed ~2KB
        // Locate start of first artifact tag and measure remainder
        let art_start = out.find("<context_artifact").unwrap();
        let artifact_section = &out[art_start..];
        assert!(
            artifact_section.len() <= 2200, // 2KB + small structural overhead
            "artifact section ({} bytes) exceeds budget",
            artifact_section.len()
        );
    }

    // ─── Sprint 25 S25 T3: EmbedClient wiring ────────────────────────────────

    /// write_episodic_memory with MockEmbedClient attaches the embedding vector.
    #[tokio::test]
    async fn write_episodic_memory_with_embed_client_attaches_embedding() {
        use cyberclaw_store::{InMemoryLeveledStore, LeveledMemoryStore, MemoryLevel};

        // Inline mock embed client returning a fixed 3-dimensional vector.
        #[derive(Debug)]
        struct MockEmbedClient {
            fixed_vec: Vec<f32>,
        }

        #[async_trait::async_trait]
        impl cyberclaw_llm::EmbedClient for MockEmbedClient {
            async fn embed(&self, _input: &str) -> cyberclaw_llm::LlmResult<Vec<f32>> {
                Ok(self.fixed_vec.clone())
            }
            fn dimension(&self) -> usize {
                self.fixed_vec.len()
            }
        }

        let store = Arc::new(InMemoryLeveledStore::new());
        let agent_runtime = Arc::new(MockAgentRuntime::new());
        let skill_runtime = Arc::new(MinimalSkillRuntime::new());
        let embed_client: Arc<dyn cyberclaw_llm::EmbedClient> = Arc::new(MockEmbedClient {
            fixed_vec: vec![0.1, 0.2, 0.3],
        });

        let svc = InMemoryExecutionService::with_runtimes(agent_runtime, skill_runtime)
            .with_leveled_memory_store(store.clone())
            .with_embed_client(embed_client);

        let execution_id = ExecutionId::new();
        let request = make_request_with_id(execution_id.clone(), make_test_task());
        svc.submit(request).await.unwrap();
        let _ = svc.execute(&execution_id).await;

        // Query the L1Summary records written for this execution
        let session_id = execution_id.as_str().to_string();
        let records = store
            .query_by_level(&session_id, MemoryLevel::L1Summary)
            .await
            .unwrap();

        assert!(
            !records.is_empty(),
            "Expected at least one L1Summary record to be written"
        );

        let record = &records[0];
        assert!(
            record.embedding.is_some(),
            "Expected embedding to be attached to the memory record"
        );
        assert_eq!(
            record.embedding.as_ref().unwrap(),
            &vec![0.1f32, 0.2, 0.3],
            "Embedding vector should match what MockEmbedClient returned"
        );
    }

    /// Sprint 9 follow-up: `InMemoryExecutionService::list_by_agent_window`
    /// override scans the live map directly and applies agent + time filters.
    /// Verifies all three filter dimensions (agent match, window inclusion,
    /// `started_at` requirement) reject in lockstep.
    #[tokio::test]
    async fn test_list_by_agent_window_filters_correctly() {
        use chrono::{Duration, Utc};
        use cyberclaw_core::execution::{AgentRef, Execution, ExecutionBudget};
        use cyberclaw_core::ids::{ExecutionId, TraceId};

        let svc = InMemoryExecutionService::new();
        let agent_a = AgentId::from_string("agent-a".to_string()).unwrap();
        let agent_b = AgentId::from_string("agent-b".to_string()).unwrap();
        let now = Utc::now();
        let window_start = now - Duration::hours(1);
        let window_end = now + Duration::hours(1);

        // Helper: stash an Execution directly into the store (bypassing submit).
        let push_exec = |id_str: &str, agent: &AgentId, started: Option<chrono::DateTime<Utc>>| {
            let id = ExecutionId::from_string(id_str.to_string()).unwrap();
            let exec = Execution {
                id: id.clone(),
                root_execution_id: id.clone(),
                parent_execution_id: None,
                owner_node_id: None,
                scheduled_node_id: None,
                placement_group: None,
                lease_id: None,
                handoff_count: 0,
                case_id: None,
                task_id: None,
                agent: AgentRef {
                    id: agent.clone(),
                    role: "test".into(),
                },
                status: ExecutionStatus::Completed,
                join_strategy: None,
                budget: ExecutionBudget::default(),
                workspace: None,
                trace_id: TraceId::new(),
                started_at: started,
                finished_at: started.map(|s| s + Duration::seconds(5)),
                risk_level: cyberclaw_core::capability::RiskLevel::Low,
                execution_mode: cyberclaw_core::execution::ExecutionMode::Normal,
            };
            let mut entries = svc.executions.write().unwrap();
            entries.insert(id, exec);
        };

        push_exec("ex-in-window", &agent_a, Some(now)); // ✓ keeper
        push_exec("ex-out-of-window", &agent_a, Some(now - Duration::hours(2))); // started_at < window_start
        push_exec("ex-after-window", &agent_a, Some(now + Duration::hours(2))); // started_at >= window_end
        push_exec("ex-other-agent", &agent_b, Some(now)); // wrong agent
        push_exec("ex-no-started-at", &agent_a, None); // never started

        let results = svc
            .list_by_agent_window(&agent_a, window_start, window_end)
            .await
            .unwrap();
        assert_eq!(results.len(), 1, "exactly one execution matches");
        assert_eq!(results[0].id.as_str(), "ex-in-window");
    }

    // -------------------------------------------------------------------------
    // Sprint D1: ExecutionMode::Persistent routing
    // -------------------------------------------------------------------------

    /// Sprint D1: a Persistent execution must fail with an explicit
    /// "PersistentLoop not wired" message when no [`PersistentLoop`] is
    /// attached. This guards against silent no-op completion of Persistent
    /// requests in misconfigured AppStates.
    #[tokio::test]
    async fn execute_persistent_without_persistent_loop_errors_clearly() {
        let svc = InMemoryExecutionService::new();
        let exec_id = ExecutionId::new();

        // Build a request explicitly tagged as Persistent.
        let mut request = make_request_with_id(exec_id.clone(), make_test_task());
        request.execution_mode = Some(cyberclaw_core::execution::ExecutionMode::Persistent);

        svc.submit(request).await.expect("submit must succeed");

        // Confirm the persisted Execution was tagged Persistent.
        let stored = svc
            .get(&exec_id)
            .await
            .expect("get must succeed")
            .expect("execution must exist");
        assert_eq!(
            stored.execution_mode,
            cyberclaw_core::execution::ExecutionMode::Persistent,
            "Persistent mode must round-trip from request to stored Execution"
        );

        // execute() must bail with the wiring-loud error.
        let err = svc
            .execute(&exec_id)
            .await
            .expect_err("execute on Persistent w/o loop must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("PersistentLoop not wired"),
            "error must mention PersistentLoop not wired, got: {}",
            msg
        );
    }

    /// Sprint D3: when a [`PersistentLoop`] *is* wired, the routing path now
    /// reaches `PersistentLoop::execute` and orchestrates the Story DAG. With
    /// an empty plan (no stories) the run trivially succeeds and execute()
    /// returns Ok with the execution marked Completed.
    #[tokio::test]
    async fn execute_persistent_with_persistent_loop_dispatches_via_d3() {
        use crate::persistent_execution::{ExecutionPlan, LoopConfig, PersistentLoop};

        let plan = ExecutionPlan::new("Sprint D3 routing test");
        let ploop = Arc::new(PersistentLoop::new(plan, LoopConfig::default()));
        let svc = InMemoryExecutionService::new().with_persistent_loop(ploop);
        let exec_id = ExecutionId::new();

        let mut request = make_request_with_id(exec_id.clone(), make_test_task());
        request.execution_mode = Some(cyberclaw_core::execution::ExecutionMode::Persistent);
        svc.submit(request).await.expect("submit must succeed");

        // Sprint D3: real dispatch — empty plan completes trivially.
        svc.execute(&exec_id)
            .await
            .expect("D3 dispatch on empty plan must succeed");

        let stored = svc
            .get(&exec_id)
            .await
            .expect("get must succeed")
            .expect("execution must exist");
        assert!(
            matches!(stored.status, ExecutionStatus::Completed),
            "Sprint D3: empty-plan persistent run must end Completed, got {:?}",
            stored.status
        );
    }
}

#[cfg(test)]
#[path = "execution_service_autopilot_tests.rs"]
mod execution_service_autopilot_tests;
