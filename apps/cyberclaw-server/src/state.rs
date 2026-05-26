//! 应用状态模块

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::broadcast;
use tokio::sync::RwLock;
use tracing::info;

use crate::clarify_broadcast::ClarifyBroadcaster;
use cyberclaw_agent_runtime::clarify::ClarifyCoordinator;
use cyberclaw_agent_runtime::deferred_registry::DeferredToolRegistry;
use cyberclaw_agent_runtime::runtime::MinimalAgentRuntime;
use cyberclaw_agent_runtime::tool_result_pipeline::PipelineGlobalConfig;
use cyberclaw_connectors::dispatcher::CapabilityDispatcher;
use cyberclaw_connectors::handoff::{HandoffConnector, HandoffSink, ReviewQueueSink};
use cyberclaw_connectors::mcp::tool_bridge::McpToolBridge;
use cyberclaw_connectors::registry::ConnectorRegistry;
use cyberclaw_control_plane::capability_discovery::{CapabilityDiscovery, SkillIndex};
use cyberclaw_control_plane::clarify_queue::InMemoryClarifyQueue;
use cyberclaw_control_plane::daily_digest_runtime::{DigestRepository, FileDigestRepository};
use cyberclaw_control_plane::handoff_queue::{HandoffQueue, InMemoryHandoffQueue};
use cyberclaw_control_plane::persistent_story_planner::PersistentStoryPlanner;
use cyberclaw_control_plane::{
    execution_service::InMemoryExecutionService, orchestrator::ControlPlaneOrchestrator,
    review_queue::InMemoryReviewQueue, task_manager::InMemoryTaskManager,
};
use cyberclaw_core::clarify::ClarifyQueue;
use cyberclaw_core::ids::TenantId;
#[cfg(test)]
use cyberclaw_governance::engine::DefaultPolicyEngine;
use cyberclaw_governance::engine::PolicyEngine;
use cyberclaw_governance::tool_output_sanitizer::ToolOutputSanitizer;
use cyberclaw_llm::{EmbedClient, LlmClient, NoopEmbedClient, OpenAiCompatEmbedClient};
use cyberclaw_llm_bridge::executor::ToolExecutor;
use cyberclaw_llm_bridge::mapper::ToolCallMapper;
use cyberclaw_llm_bridge::{register_mcp_tool_mappings, register_standard_mappings};
use cyberclaw_observability::events::InMemoryEventRecorder;
use cyberclaw_skill_runtime::compat::SkillCompatRegistry;
use cyberclaw_skill_runtime::curator::{Curator, CuratorConfig, HeuristicEvaluator};
use cyberclaw_skill_runtime::skill_hub::{PublisherRing, SkillBundle, SkillHub, SkillSource};
use cyberclaw_skill_runtime::skill_scanner::{SkillScanner, SkillTrustLevel};
use cyberclaw_skill_runtime::telemetry::SkillUsageStore;

use cyberclaw_workflow::{TriggerRegistry, WorkflowEngine, WorkflowEngineApi};

use crate::admin_store::AdminStore;
use crate::api::admin::events::{AdminEvent, ADMIN_EVENT_BUS_CAPACITY};
use crate::api::channels::ChannelRecord;
use crate::api::wizard::WizardSessionRegistry;
use crate::audit::AuditSink;
use crate::cluster_assignments::AssignmentQueue;
use crate::profiles_store::ProfileStore;
use crate::types::{AgentExecutionStatus, TaskWithStatus};

// ---------------------------------------------------------------------------
// QueueSink — bridge from HandoffSink (connectors) to HandoffQueue (control-plane)
// ---------------------------------------------------------------------------

/// Thin newtype that lets any `Arc<dyn HandoffQueue>` satisfy the
/// `HandoffSink` trait required by `HandoffConnector`.
///
/// Lives here (server crate) to avoid touching the connectors or
/// control-plane crates for what is purely a wiring concern.
struct QueueSink(Arc<dyn HandoffQueue>);

impl std::fmt::Debug for QueueSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueueSink").finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
impl HandoffSink for QueueSink {
    async fn enqueue(
        &self,
        req: cyberclaw_core::handoff::HandoffRequest,
    ) -> Result<(), cyberclaw_core::handoff::HandoffError> {
        self.0.enqueue(req).await
    }
}

// ---------------------------------------------------------------------------
// ReviewQueueSinkAdapter — bridge from ReviewQueueSink (connectors) to ReviewQueue (control-plane)
// ---------------------------------------------------------------------------

/// Thin newtype that lets `Arc<InMemoryReviewQueue>` satisfy the
/// `ReviewQueueSink` trait required by `HandoffConnector` (S24).
///
/// Builds a `ReviewRequest::for_handoff(...)` from the connector-supplied
/// parameters and delegates to the underlying `ReviewQueue::enqueue`.
struct ReviewQueueSinkAdapter(Arc<dyn cyberclaw_control_plane::review_queue::ReviewQueue>);

impl std::fmt::Debug for ReviewQueueSinkAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReviewQueueSinkAdapter")
            .finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
impl ReviewQueueSink for ReviewQueueSinkAdapter {
    async fn enqueue_review_for_handoff(
        &self,
        review_id: cyberclaw_core::ids::ReviewId,
        handoff_id: cyberclaw_core::ids::HandoffId,
        title: String,
        summary: String,
        requested_by: cyberclaw_core::identity::ActorRef,
    ) -> anyhow::Result<()> {
        let req = cyberclaw_core::review::ReviewRequest::for_handoff(
            review_id,
            handoff_id,
            None, // case_id
            title,
            summary,
            requested_by,
            cyberclaw_core::review::ReviewKind::HumanReview,
            cyberclaw_core::ids::TraceId::new(),
            chrono::Utc::now(),
        );
        self.0
            .enqueue(req)
            .await
            .map_err(|e| anyhow::anyhow!("review queue enqueue failed: {}", e))
    }
}

fn parse_csv_env_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn load_tool_filter_env(var_name: &str) -> Vec<String> {
    std::env::var(var_name)
        .ok()
        .map(|value| parse_csv_env_list(&value))
        .unwrap_or_default()
}

fn apply_tool_visibility_filters(
    tool_mapper: &ToolCallMapper,
    allow_patterns: &[String],
    deny_patterns: &[String],
) {
    if allow_patterns.is_empty() && deny_patterns.is_empty() {
        return;
    }

    let removed = tool_mapper
        .apply_visibility_filters(allow_patterns, deny_patterns)
        .expect("failed to apply tool visibility filters");
    info!(
        allow = ?allow_patterns,
        deny = ?deny_patterns,
        removed_count = removed.len(),
        "Applied tool visibility filters"
    );
}

/// Control Plane 组件集合
///
/// 用于简化 AppState 构造参数
pub struct ControlPlaneComponents {
    pub control_plane: Arc<ControlPlaneOrchestrator>,
    pub task_manager: Arc<InMemoryTaskManager>,
    pub execution_service: Arc<InMemoryExecutionService>,
    pub review_queue: Arc<InMemoryReviewQueue>,
    /// v1.x — shared package registry (Agent / Skill / Connector / PlatformPlugin
    /// metadata). The same `Arc` is also handed to `InMemoryResolver` so
    /// runtime resolution and the `/api/v2/packages` admin surface always
    /// reflect identical state.
    pub package_registry: Arc<cyberclaw_control_plane::registry::InMemoryRegistry>,
}

/// 应用全局状态
///
/// 整合CyberClaw核心组件，提供统一的执行、治理、审计能力。
#[derive(Clone)]
pub struct AppState {
    // ===== 外部接口层 =====
    /// LLM 客户端 (用于直接 LLM 对话)
    pub llm_client: Arc<dyn LlmClient>,

    // ===== 核心平台层 =====
    /// Connector 注册表
    pub connector_registry: Arc<ConnectorRegistry>,

    /// 包注册表 (Agent/Skill/Connector/PlatformPlugin)
    ///
    /// v1.x — server-owned single source of truth backing `/api/v2/packages`
    /// and `/api/v2/status`. Same `Arc` is shared with `InMemoryResolver` so
    /// runtime resolution and the admin surface stay coherent.
    pub package_registry: Arc<cyberclaw_control_plane::registry::InMemoryRegistry>,

    /// User-installed package persistence (`~/.cyberclaw/installed-packages.json`).
    ///
    /// Re-loaded into `package_registry` on every server boot so packages
    /// added via `POST /api/v2/packages` survive a restart.
    pub installed_packages: Arc<crate::installed_packages::InstalledPackageStore>,

    /// Capability 调度器
    pub capability_dispatcher: Arc<CapabilityDispatcher>,

    /// 策略引擎 (治理与审批)
    pub policy_engine: Arc<dyn PolicyEngine>,

    // ===== Control Plane 组件 =====
    /// Control Plane 编排器
    pub control_plane: Arc<ControlPlaneOrchestrator>,

    /// 任务管理器
    pub task_manager: Arc<InMemoryTaskManager>,

    /// 执行服务
    pub execution_service: Arc<InMemoryExecutionService>,

    /// 审核队列
    pub review_queue: Arc<InMemoryReviewQueue>,

    // ===== LLM-Bridge 工具执行层 =====
    /// Tool Call 映射器
    pub tool_mapper: Arc<ToolCallMapper>,

    /// Tool 执行器
    pub tool_executor: Arc<ToolExecutor>,

    // ===== 可观测性层 =====
    /// 事件记录器
    pub event_recorder: Arc<InMemoryEventRecorder>,

    // ===== Agent 运行时层 =====
    /// 共享的 Agent 运行时 (消除每请求创建新实例的资源泄漏)
    pub agent_runtime: Arc<MinimalAgentRuntime>,

    // ===== 安全层 =====
    /// JWT 签名密钥
    pub jwt_secret: Arc<String>,

    // ===== 应用状态存储层 =====
    /// 任务状态存储 (统一管理)
    pub task_store: Arc<RwLock<HashMap<String, TaskWithStatus>>>,

    /// Agent 执行状态存储 (统一管理)
    pub agent_status_store: Arc<RwLock<HashMap<String, AgentExecutionStatus>>>,

    /// 跨实例执行分配队列（用于远端 worker pull）
    pub assignment_queue: Arc<AssignmentQueue>,

    // ===== 工作流层 =====
    /// 工作流引擎
    pub workflow_engine: Arc<dyn WorkflowEngineApi>,

    /// 触发器注册表
    pub trigger_registry: Arc<TriggerRegistry>,

    // ===== Webhook 安全层 =====
    /// 每个平台的 webhook 签名密钥 (platform_id -> HMAC secret)
    pub webhook_secrets: Arc<HashMap<String, String>>,

    /// 每个平台的 webhook 租户绑定 (platform_id -> tenant_id)
    pub webhook_tenants: Arc<HashMap<String, TenantId>>,

    // ===== Onboarding 向导层 =====
    /// 活跃的 `/wizard/*` 会话注册表。
    pub wizard_registry: Arc<WizardSessionRegistry>,

    /// One-shot bootstrap token for `/wizard/*`.
    ///
    /// Populated at startup when `~/.cyberclaw/users.toml` is missing or empty
    /// (Sprint 5). The token is printed to stdout exactly once and then held
    /// in memory for comparison against the `X-Wizard-Bootstrap-Token` header
    /// on wizard requests. Cleared to `None` after the wizard successfully
    /// persists its first operator record.
    pub wizard_bootstrap_token: Arc<RwLock<Option<String>>>,

    // ===== Sprint 7 — Admin console backing stores =====
    /// Remote skill hub (discovery, quarantine, install).
    ///
    /// Sprint 7: backs the `/api/v1/skills` admin surface. Wrapped in an
    /// async `RwLock` because [`SkillHub`] itself is `!Sync` on mutation
    /// and every list operation may mutate the audit log.
    pub skill_hub: Arc<RwLock<SkillHub>>,

    /// Publisher GPG key ring for verifying skill bundle signatures.
    ///
    /// Loaded from `~/.cyberclaw/trust/publishers.asc` on startup.
    /// Backs `GET /api/v1/skills/publisher-keys` and `POST /api/v1/skills/publisher-keys`.
    pub publisher_ring: Arc<RwLock<PublisherRing>>,

    /// IM / webhook channel configuration store (Discord/Slack/Telegram/...).
    ///
    /// Persisted to `~/.cyberclaw/channels.toml` — same pattern as
    /// `users.toml`. Backs the `/api/v1/channels` admin surface.
    pub channels_store: Arc<RwLock<HashMap<String, ChannelRecord>>>,

    /// Broadcast sender for admin console live event stream
    /// (`/admin/events` SSE).
    ///
    /// §2 EventSink-style adapter: any long-running component can be wired
    /// to publish here, subscribers get an `axum::response::sse::Sse`-wrapped
    /// stream.
    pub admin_event_bus: broadcast::Sender<AdminEvent>,

    /// This node's identifier, used by `/api/v1/cluster/nodes` MVP.
    pub node_id: String,

    /// Server process start time, used by `/api/v1/settings/about`.
    pub started_at: Arc<Instant>,

    /// Sprint 8 Phase A — in-memory registry backing the admin console.
    ///
    /// See [`crate::admin_store`] for shape + seed semantics. Handlers
    /// for `/api/v1/{agents,skills,tasks,executions,reviews}` fall back
    /// to this store when the control-plane registries are empty, which
    /// is the default for a freshly-booted process.
    pub admin_store: Arc<AdminStore>,

    /// Sprint 8 Lane 1 — append-only audit sink.
    ///
    /// Shared by the auth middleware (login success/failure), admin
    /// mutation handlers (task/skill/review/etc.), and the
    /// `/api/v1/audit/logs` read surface. `None` in test builds that
    /// don't need the audit lane; real server boot always populates it.
    pub audit: Option<Arc<AuditSink>>,

    /// Sprint 21 path #3 — FTS5 skill discovery index.
    ///
    /// Mirrors installed skill metadata into a separate SQLite DB
    /// (`~/.cyberclaw/skill-index.db` by default) with an FTS5 virtual
    /// table for natural-language search. Best-effort: failures are
    /// logged but never block skill registration. `None` in test
    /// builds; real server boot populates it via
    /// `SkillIndex::new(SkillIndex::default_path())`.
    pub skill_index: Option<Arc<crate::skill_index::SkillIndex>>,

    /// Sprint 44 — SkillCompat translator registry.
    ///
    /// Holds the 5 default translators (Hermes / Anthropic / ClaudeCode /
    /// OpenClaw / Superpowers). Used by chat_handler at LLM tool-call
    /// dispatch to translate ecosystem-specific tool names (e.g.
    /// `browser_navigate`, `Bash`) into cyberclaw facade names
    /// (`browser.navigate`, `cmd.run`) before [`ToolCallMapper`] resolves
    /// them to a `(connector_id, capability_id)` pair. See
    /// `cyberclaw_skill_runtime::compat` for the translator contract.
    pub skill_compat: Arc<SkillCompatRegistry>,

    /// Sprint 9 L9 — Daily-digest repository.
    ///
    /// Backs the `GET /api/v1/agents/:id/digest` endpoint and the Cron-driven
    /// `DefaultDailyDigestCoordinator` fanout. The default
    /// [`FileDigestRepository`] is a placeholder until `cyberclaw-store`
    /// exposes a real Semantic Memory write API (Sprint 9 backlog).
    pub digest_repo: Arc<dyn DigestRepository>,

    /// Sprint 10 W2 L3 — Global pipeline configuration.
    ///
    /// Loaded at startup from `~/.cyberclaw/pipeline.toml` (falls back to
    /// defaults when the file is absent). Environment variable overrides
    /// (`CYBERCLAW_TRUNCATE`, `CYBERCLAW_TRUNCATE_MAX_BYTES`,
    /// `CYBERCLAW_TRUNCATE_HEAD_PCT`) are applied after file loading.
    pub pipeline_config: Arc<PipelineGlobalConfig>,

    /// Sprint 11 W2 L1 — Pending permission-rule patch proposals.
    ///
    /// Backs `PATCH /api/v1/security/permission/rules/:id`. Never mutates
    /// the live governance rule set; operators promote proposals via the
    /// normal `/reviews` surface. In-memory because proposals are
    /// short-lived (hours, not days) and a restart is an acceptable reset.
    pub permission_rule_proposals: Arc<RwLock<Vec<crate::api::governance::PermissionRuleProposal>>>,

    /// Sprint 11 W2 L1 — In-process Global-scope semantic memory.
    ///
    /// Backs `GET/POST /api/v1/learning/org-memory`. Entries mirror the
    /// shape of `SemanticMemoryEntry` in `cyberclaw-store`; when the real
    /// store is wired into `AppState` in a follow-up task, swap callers to
    /// `SemanticMemoryStore::query_by_scope(&MemoryScope::Global, ...)`.
    pub org_memory_store: Arc<RwLock<Vec<crate::api::learning::OrgMemoryEntry>>>,

    // ===== Sprint 17 Lane B — Govern Memory Console =====
    /// Leveled memory store backing the Govern Memory Console.
    ///
    /// S18 R3: migrated from server-local `InMemoryMemoryStore` to the
    /// `cyberclaw_store::LeveledMemoryStore` trait. Default impl at startup
    /// is `cyberclaw_store::InMemoryLeveledStore`. All `/api/v1/memory/*`
    /// handlers read/write this field.
    pub memory_store: Arc<dyn cyberclaw_store::LeveledMemoryStore>,

    /// S20 E2 — Credential sanitizer for memory write paths.
    ///
    /// Applied in `create_memory`, `edit_memory`, and `write_episodic_memory`
    /// before content reaches the store. Redacts detected credentials and sets
    /// `metadata.sanitized = "true"` when redactions occur.
    pub memory_sanitizer: Arc<ToolOutputSanitizer>,

    // ===== Sprint 15 — Clarify Card =====
    /// In-memory clarify request queue.
    ///
    /// Shared between the HTTP surface (`chat_clarify.rs`) and the
    /// `ClarifyCoordinator` so the agent loop can be awakened when the
    /// user submits an answer.
    pub clarify_queue: Arc<dyn ClarifyQueue>,

    /// Coordinator that bridges the agent loop's `ask()` future with the
    /// HTTP `respond` endpoint.
    pub clarify_coordinator: Arc<ClarifyCoordinator>,

    /// Per-conversation SSE broadcaster for clarify events.
    ///
    /// SSE handlers subscribe via `clarify_broadcaster.subscribe(conv_id)` and
    /// receive both `ClarifyEvent::Requested` (when the agent calls `ask()`) and
    /// `ClarifyEvent::Resolved` (when the user submits an answer). The agent loop
    /// calls `ask_user_clarify()` which publishes first, then awaits the answer.
    pub clarify_broadcaster: Arc<ClarifyBroadcaster>,

    /// v1.2.15 — per-conversation SSE event replay buffer.
    ///
    /// Append every SSE payload via `sse_replay.append(conv_id, data)` to make
    /// it available to clients reconnecting with a `Last-Event-ID` header.
    /// Sized at 64 events × 1000 conversations × 5-min TTL (see module docs).
    /// Wiring into `chat.rs` SSE handlers is the recommended v1.3 follow-up;
    /// shipping the buffer as state now keeps the integration point stable.
    pub sse_replay: crate::sse_replay::SseReplayBuffer,

    // ===== Sprint 21 — Multi-Agent Handoff =====
    /// In-memory handoff request queue.
    ///
    /// Backs the `/api/v1/chat/handoff` HTTP surface (T10) and the
    /// `HandoffConnector` dispatch path (T7/T8). Shared between the HTTP
    /// handlers and the connector layer so admin accept/reject actions and
    /// TTL expiry are visible to both.
    pub handoff_queue: Arc<dyn HandoffQueue>,

    /// S21 T20 — Feature flag for multi-agent handoff (default: false).
    ///
    /// Set `CYBERCLAW_HANDOFF_ENABLED=true` or `=1` to opt in to the
    /// handoff feature. When false, all `/api/v1/chat/handoff` mutation
    /// handlers return 400 and the `HandoffConnector` short-circuits with
    /// `HandoffError::Denied("feature_disabled")`.
    pub handoff_feature_enabled: bool,

    /// Sprint 25 S25 T3 — Embedding client for semantic memory search.
    ///
    /// Env-var-driven: `CYBERCLAW_EMBED_ENABLED=true|1` activates
    /// `OpenAiCompatEmbedClient`; otherwise `NoopEmbedClient` (dim=0, no-op).
    /// Also passed to `InMemoryExecutionService::with_embed_client` so that
    /// `write_episodic_memory` auto-attaches vectors to L1Summary records.
    pub embed_client: Arc<dyn EmbedClient>,

    /// W2 D-1 — Autonomous skill Curator (background grader / pruner).
    ///
    /// `Some(curator)` when wired by `AppState::with_curator(...)` (the
    /// production server boot path). `None` in test/build paths that
    /// don't need the curator. Backs `/api/v1/admin/curator/status` and
    /// `/api/v1/admin/curator/run-now`.
    pub curator: Option<Arc<Curator>>,

    /// Sprint D3 — LLM-driven Story DAG planner for persistent execution.
    ///
    /// Used by `persistent_chat_dispatch` to build a real
    /// [`cyberclaw_control_plane::persistent_execution::PersistentExecutionPlan`]
    /// from the user's last message. Constructed at startup with the set of
    /// capabilities registered at that moment; the capability list is
    /// intentionally snapshotted once (connectors registered after boot are
    /// not considered for plan generation, which is acceptable for the
    /// current single-node topology).
    pub persistent_story_planner: Arc<PersistentStoryPlanner>,

    /// F3 — D2 CapabilityDiscovery wired to production.
    ///
    /// Six-stage discovery service (3 sync + 3 async, stop-on-first-hit):
    /// native connectors → installed skills → cmd binaries → SkillHub
    /// remote → provider modalities → capability_request gap-recording.
    ///
    /// Was previously instantiated only inside `#[cfg(test)]`. Now wired so
    /// the `/api/v1/capabilities/discover_for_goal` admin endpoint can issue
    /// goal-relevant queries against the live registry, and so future
    /// per-request enrichment of `PersistentStoryPlanner` has a single
    /// authoritative source.
    pub capability_discovery: Arc<CapabilityDiscovery>,

    /// F7 — `DeferredToolRegistry` wired to production.
    ///
    /// Holds active vs deferred tool facades so the prompt builder can show
    /// the LLM only the active ones (full schema) and the deferred ones by
    /// name (LLM uses `ToolSearch` to demand the schema). Was previously
    /// 0-prod-reference; now seeded from `BuiltinToolRegistry::default_facades`
    /// at startup and exposed via `/api/v1/tools/state` for admin
    /// observability.
    pub deferred_tool_registry: Arc<RwLock<DeferredToolRegistry>>,

    /// F6 — `McpToolBridge` wired to production.
    ///
    /// Translates third-party MCP server tool descriptors into CyberClaw
    /// `BridgedTool` instances (namespaced + risk-classified). Previously
    /// 0-prod-reference; now exposed via `POST /api/v1/mcp/bridge_tool` for
    /// admin/dev tooling that registers MCP tools dynamically.
    pub mcp_tool_bridge: Arc<McpToolBridge>,

    // ===== F4 — Multi-node coordination primitives =====
    /// F4 (2026-05-06) — multi-node coordination primitives.
    ///
    /// 单节点部署下，brain_coordinator 退化为 identity assigner（自身就是
    /// 唯一 brain）。多节点部署时，外部 brain 通过 POST /api/v1/cluster/brain/register
    /// 注册 + 周期性 POST /api/v1/cluster/heartbeat/{brain_id} 维持 alive
    /// 状态，coordinator 用 LeastLoadedAssigner 把新 session 分配到负载最低
    /// 的 brain。
    pub heartbeat_monitor: Arc<cyberclaw_control_plane::cluster::node::HeartbeatMonitor>,
    pub session_store: Arc<dyn cyberclaw_control_plane::distributed::SessionStore>,
    pub brain_coordinator: Arc<cyberclaw_control_plane::distributed::BrainCoordinator>,

    /// In-memory rolling usage counters backing GET /api/v1/usage.
    pub usage: Arc<crate::usage::UsageCounters>,

    /// Cron job store — in-memory + file backup at `~/.cyberclaw/cron.toml`.
    pub cron_store: Arc<crate::cron::CronStore>,

    /// User profile store — in-memory + file backup at `~/.cyberclaw/profiles.toml`.
    pub profile_store: Arc<ProfileStore>,

    /// v1.3 WP-1 — server-side conversation session store.
    ///
    /// Backs `POST /v2/agent/chat/completions`. Holds the authoritative
    /// per-conversation message history (typed `cyberclaw_llm::types::Message`,
    /// tool_calls preserved) so clients only send the new turn's user message
    /// rather than a `messages` array. Eliminates R7-01 / R9-01 / R9-02
    /// context-loss failure modes.
    pub conversation_session_store: Arc<dyn cyberclaw_store::SessionStore>,
}

/// SkillIndex adapter wrapping the workspace's `Arc<RwLock<SkillHub>>`.
///
/// `SkillHubIndex` in cyberclaw-control-plane wants a bare `Arc<SkillHub>`,
/// but our AppState owns the SkillHub behind an async RwLock so admin
/// mutations are coordinated. This adapter takes a read guard per call —
/// `SkillHub::search` is read-only — and runs the same matching logic as
/// `SkillHubIndex`.
struct LockedSkillHubIndex {
    inner: Arc<RwLock<SkillHub>>,
}

impl SkillIndex for LockedSkillHubIndex {
    fn search(&self, search_terms: &[String]) -> Vec<cyberclaw_core::ids::SkillId> {
        if search_terms.is_empty() {
            return Vec::new();
        }
        // Block briefly on the read guard. SkillHub::search is fast (in-memory
        // bundle scan) so this stays sub-ms even under admin write contention.
        let guard = match self.inner.try_read() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        let mut hits: Vec<cyberclaw_core::ids::SkillId> = Vec::new();
        for term in search_terms {
            for bundle in guard.search(term) {
                if let Ok(id) = cyberclaw_core::ids::SkillId::from_string(bundle.name.clone()) {
                    if !hits.iter().any(|existing| existing == &id) {
                        hits.push(id);
                    }
                }
            }
        }
        hits
    }
}

impl AppState {
    /// 创建新的应用状态
    ///
    /// # Arguments
    ///
    /// * `llm_client` - LLM 客户端
    /// * `connector_registry` - Connector 注册表
    /// * `policy_engine` - 策略引擎
    /// * `event_recorder` - 事件记录器
    /// * `jwt_secret` - JWT 签名密钥
    /// * `cp_components` - Control Plane 组件集合
    pub fn new(
        llm_client: Arc<dyn LlmClient>,
        connector_registry: Arc<ConnectorRegistry>,
        policy_engine: Arc<dyn PolicyEngine>,
        event_recorder: Arc<InMemoryEventRecorder>,
        jwt_secret: String,
        cp_components: ControlPlaneComponents,
    ) -> Self {
        // 创建 Capability 调度器
        // Sprint 18 W3 — env-driven runtime strategy. Default is RiskBased
        // (production-correct: Medium → Process, High/Critical → Container).
        // Sprint 19 W1 — Process runtime is now wired with a whitelisted
        // ProcessExecutor by default, so Medium-risk capabilities really
        // dispatch instead of fail-fast. CYBERCLAW_RUNTIME_STRATEGY still
        // works as an override (native / process / container / risk-based).
        let capability_dispatcher = {
            use cyberclaw_connectors::runtime::{
                ProcessExecutor, RuntimeSelectionStrategy, RuntimeSelectorConfig,
            };
            let strategy = match std::env::var("CYBERCLAW_RUNTIME_STRATEGY")
                .unwrap_or_default()
                .to_ascii_lowercase()
                .as_str()
            {
                "native" => Some(RuntimeSelectionStrategy::AlwaysNative),
                "process" => Some(RuntimeSelectionStrategy::AlwaysProcess),
                "container" => Some(RuntimeSelectionStrategy::AlwaysContainer),
                "risk-based" | "riskbased" => Some(RuntimeSelectionStrategy::RiskBased),
                "" => None,
                other => {
                    tracing::warn!(
                        "unknown CYBERCLAW_RUNTIME_STRATEGY '{}', falling back to default",
                        other
                    );
                    None
                }
            };

            // Build the ProcessExecutor that backs RuntimeMode::Process.
            // Default whitelist mirrors what local::cmd::exec already
            // permits (read-only inspection commands + workspace tools).
            // CYBERCLAW_PROCESS_WHITELIST=cmd1,cmd2,... overrides this.
            let process_runtime: Arc<dyn cyberclaw_connectors::runtime::ProcessRuntime> = {
                let whitelist_env = std::env::var("CYBERCLAW_PROCESS_WHITELIST").ok();
                if let Some(csv) = whitelist_env {
                    let entries: std::collections::HashSet<String> = csv
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    tracing::info!(
                        "Sprint 19 W1: ProcessExecutor configured with {} whitelisted commands",
                        entries.len()
                    );
                    Arc::new(ProcessExecutor::with_whitelist(entries))
                } else {
                    let default_whitelist: std::collections::HashSet<String> = [
                        "ls", "cat", "head", "tail", "wc", "grep", "find", "echo", "pwd", "date",
                        "env", "uname", "whoami", "stat", "file", "rg", "fd", "tree",
                    ]
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                    tracing::info!(
                        "Sprint 19 W1: ProcessExecutor configured with default \
                         {}-command whitelist (override via CYBERCLAW_PROCESS_WHITELIST)",
                        default_whitelist.len()
                    );
                    Arc::new(ProcessExecutor::with_whitelist(default_whitelist))
                }
            };

            let dispatcher = if let Some(strategy) = strategy {
                let cfg = RuntimeSelectorConfig {
                    default_strategy: strategy,
                    capability_overrides: Default::default(),
                    strict_mode: false,
                };
                CapabilityDispatcher::with_runtime_config(connector_registry.clone(), cfg)
            } else {
                CapabilityDispatcher::new(connector_registry.clone())
            };

            // v0.2.10 fix: dispatcher needs the same container runtime that
            // main.rs configures on the ExecutionService dispatcher; without
            // this the chat tool-dispatch path (used by /v1/chat/completions
            // with auto tool execution) hits "Container runtime not
            // configured" for Critical-risk caps like cmd.run, even though
            // docker IS installed and main.rs auto-detected it for the
            // other dispatcher. Detect synchronously here (sync `docker
            // --version` probe) to keep the existing sync new() signature.
            let container_runtime = {
                let docker_ok = std::process::Command::new("docker")
                    .arg("--version")
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);
                let podman_ok = !docker_ok
                    && std::process::Command::new("podman")
                        .arg("--version")
                        .output()
                        .map(|o| o.status.success())
                        .unwrap_or(false);
                if docker_ok || podman_ok {
                    let backend = if docker_ok {
                        cyberclaw_connectors::runtime::ContainerBackend::Docker
                    } else {
                        cyberclaw_connectors::runtime::ContainerBackend::Podman
                    };

                    // v0.2.12: env-driven container defaults.
                    //   CYBERCLAW_CONTAINER_IMAGE   default "alpine:3.20" if unset
                    //     (small image; override to "python:3.12-slim" or
                    //     "node:20" when the agent needs richer tooling).
                    //   CYBERCLAW_CONTAINER_NETWORK default "none"; "bridge"
                    //     enables outbound network (use with care).
                    let mut cfg = cyberclaw_connectors::runtime::ContainerConfig::default();
                    if let Ok(img) = std::env::var("CYBERCLAW_CONTAINER_IMAGE") {
                        if !img.trim().is_empty() {
                            cfg.image = img;
                        }
                    }
                    if let Ok(net) = std::env::var("CYBERCLAW_CONTAINER_NETWORK") {
                        cfg.network = match net.to_lowercase().as_str() {
                            "bridge" => cyberclaw_connectors::runtime::NetworkMode::Bridge,
                            "host" => cyberclaw_connectors::runtime::NetworkMode::Host,
                            _ => cyberclaw_connectors::runtime::NetworkMode::None,
                        };
                    }

                    Some(std::sync::Arc::new(
                        cyberclaw_connectors::runtime::ContainerRuntime::with_backend(backend)
                            .with_default_config(cfg),
                    ))
                } else {
                    None
                }
            };
            let mut dispatcher = dispatcher.with_process_runtime(process_runtime);
            if let Some(rt) = container_runtime {
                tracing::info!(
                    "v0.2.10: chat-tool dispatcher attached container runtime (docker/podman detected)"
                );
                dispatcher = dispatcher.with_container_runtime(rt);
            }
            Arc::new(dispatcher)
        };

        // 创建 Tool Call 映射器
        let tool_mapper = Arc::new(ToolCallMapper::new());
        register_standard_mappings(&tool_mapper)
            .expect("failed to register standard tool mappings");
        if let Some(mcp_connector) = connector_registry
            .list_connectors()
            .into_iter()
            .find(|id| id.as_ref().starts_with("mcp-"))
        {
            register_mcp_tool_mappings(&tool_mapper, mcp_connector.as_ref())
                .expect("failed to register mcp tool mappings");
        }
        let allow_patterns = load_tool_filter_env("CYBERCLAW_TOOL_ALLOWLIST");
        let deny_patterns = load_tool_filter_env("CYBERCLAW_TOOL_DENYLIST");
        apply_tool_visibility_filters(&tool_mapper, &allow_patterns, &deny_patterns);

        // 创建 Tool 执行器 (带默认配置)
        let tool_executor = Arc::new(ToolExecutor::new(
            capability_dispatcher.clone(),
            tool_mapper.clone(),
        ));

        // 创建共享的 Agent 运行时
        let agent_runtime = Arc::new(MinimalAgentRuntime::new());

        // 创建状态存储
        let task_store = Arc::new(RwLock::new(HashMap::new()));
        let agent_status_store = Arc::new(RwLock::new(HashMap::new()));
        let assignment_queue = Arc::new(AssignmentQueue::new());

        // 创建工作流引擎和触发器注册表
        let workflow_engine: Arc<dyn WorkflowEngineApi> = Arc::new(WorkflowEngine::new());
        let trigger_registry = Arc::new(TriggerRegistry::new());

        // 加载 webhook 签名密钥 (CYBERCLAW_WEBHOOK_SECRET_<PLATFORM> 环境变量)
        let webhook_secrets = Arc::new(load_webhook_secrets());
        let webhook_tenants = Arc::new(load_webhook_tenants());

        // Onboarding 向导会话注册表
        let wizard_registry = Arc::new(WizardSessionRegistry::new());

        // Sprint 5: generate a bootstrap token if no operators exist yet.
        let wizard_bootstrap_token = Arc::new(RwLock::new(maybe_generate_bootstrap_token()));

        // Sprint 7 — admin console backing stores.
        let skill_hub = Arc::new(RwLock::new(build_skill_hub()));
        let publisher_ring = {
            let ring_path = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".cyberclaw/trust/publishers.asc");
            let ring = PublisherRing::load_from_file(&ring_path).unwrap_or_default();
            Arc::new(RwLock::new(ring))
        };
        let channels_store = Arc::new(RwLock::new(crate::api::channels::load_channels_from_disk()));
        let (admin_event_bus, _rx) = broadcast::channel(ADMIN_EVENT_BUS_CAPACITY);
        let node_id = std::env::var("CYBERCLAW_NODE_ID")
            .unwrap_or_else(|_| format!("node-{}", uuid::Uuid::new_v4().simple()));
        let started_at = Arc::new(Instant::now());

        // Sprint 9 L9 — Daily digest repository (file-backed placeholder).
        let digest_repo: Arc<dyn DigestRepository> = Arc::new(FileDigestRepository::default_root());

        // Sprint 10 W2 L3 — Global pipeline config (file + env overrides).
        let pipeline_config = Arc::new(PipelineGlobalConfig::load());

        // Sprint 8 Phase A — admin store (optionally seeded for demo).
        // Agent registry is disk-backed at $HOME/.cyberclaw/agents.json
        // (overridable via CYBERCLAW_AGENTS_PATH) so admin-console-created
        // agents survive server restarts (AGT-BUG-2).
        let admin_store = Arc::new(AdminStore::new_with_persistence(
            AdminStore::default_agents_path(),
        ));
        if std::env::var("CYBERCLAW_ADMIN_SEED_DEMO")
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes"))
            .unwrap_or(false)
        {
            let store = admin_store.clone();
            // seed_demo is async because it takes `.write()` on each
            // RwLock; run it on a background task so `AppState::new`
            // stays sync. The server boot flow awaits the runtime, so
            // this lands before the HTTP listener opens connections.
            tokio::spawn(async move {
                let counts = store.seed_demo().await;
                tracing::info!(
                    agents = counts.agents,
                    skills = counts.skills,
                    tasks = counts.tasks,
                    executions = counts.executions,
                    reviews = counts.reviews,
                    nodes = counts.nodes,
                    "admin_store seeded from CYBERCLAW_ADMIN_SEED_DEMO=1"
                );
            });
        }

        // Sprint 17 Lane B / S18 R3 — Govern Memory Console: store-crate leveled store.
        // S19 B: If CYBERCLAW_MEMORY_DB is set, use SQLite for persistence across restarts.
        // SqliteLeveledStore is always available because cyberclaw-store is built with
        // features = ["sqlite"] in the workspace (Cargo.toml).
        let memory_store: Arc<dyn cyberclaw_store::LeveledMemoryStore> = if let Ok(db_path) =
            std::env::var("CYBERCLAW_MEMORY_DB")
        {
            match cyberclaw_store::SqliteLeveledStore::new(&db_path) {
                Ok(store) => {
                    tracing::info!(db_path = %db_path, "memory_store: using SqliteLeveledStore");
                    Arc::new(store)
                }
                Err(e) => {
                    tracing::warn!(
                        db_path = %db_path,
                        error = %e,
                        "SqliteLeveledStore open failed, falling back to InMemoryLeveledStore"
                    );
                    Arc::new(cyberclaw_store::InMemoryLeveledStore::new())
                }
            }
        } else {
            Arc::new(cyberclaw_store::InMemoryLeveledStore::new())
        };

        // S17 Lane B seed demo records so Memory Console isn't empty on first login.
        // AppState::new is sync — spawn on Tokio runtime like admin_store seeding above.
        // Background: lands before HTTP listener opens connections (see main.rs order).
        {
            let seed_store = memory_store.clone();
            tokio::spawn(async move {
                crate::api::memory::seed_memory_demo(seed_store.as_ref()).await;
                tracing::info!("memory_store seeded with Lane B demo records");
            });
        }

        // Sprint 15 — Clarify Card: shared queue + coordinator (both sync constructors).
        let clarify_queue: Arc<dyn ClarifyQueue> = Arc::new(InMemoryClarifyQueue::new());
        let clarify_coordinator = Arc::new(ClarifyCoordinator::new(clarify_queue.clone()));
        let clarify_broadcaster = Arc::new(ClarifyBroadcaster::new());

        // S20 E2 — Shared credential sanitizer for all memory write paths.
        let memory_sanitizer = Arc::new(ToolOutputSanitizer::new());

        // Sprint 25 S25 T3 — env-var-driven embed client.
        let embed_enabled = std::env::var("CYBERCLAW_EMBED_ENABLED")
            .ok()
            .as_deref()
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        let embed_client: Arc<dyn EmbedClient> = if embed_enabled {
            let base_url = std::env::var("CYBERCLAW_EMBED_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
            let api_key = std::env::var("CYBERCLAW_EMBED_API_KEY").ok();
            let model = std::env::var("CYBERCLAW_EMBED_MODEL")
                .unwrap_or_else(|_| "text-embedding-3-small".to_string());
            let dimension: usize = std::env::var("CYBERCLAW_EMBED_DIMENSION")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1536);
            tracing::info!(
                base_url = %base_url,
                model = %model,
                dim = %dimension,
                "Sprint 25 S25 T3 — embedding enabled"
            );
            Arc::new(OpenAiCompatEmbedClient::new(
                base_url, api_key, model, dimension,
            ))
        } else {
            Arc::new(NoopEmbedClient)
        };

        // Sprint D3 — PersistentStoryPlanner: snapshot capability list at construction
        // time so the planner prompt lists real capabilities from this registry.
        //
        // Model resolution: `LLM_DEFAULT_MODEL` is the canonical env var the rest
        // of the workspace uses (see e2e_integration_test.rs / common/mod.rs /
        // workbench.rs / agents.rs — 11 call sites). The original D5 code read
        // `CYBERCLAW_MODEL` which only existed in this one file, so on real
        // deployments where the operator only set `LLM_DEFAULT_MODEL`, the
        // planner silently fell back to "gpt-4" and every call against a
        // MiniMax/Doubao/Claude/Ollama base URL failed model-not-found, which
        // triggered placeholder_plan() — masking the entire D5 pipeline.
        // Fix: read LLM_DEFAULT_MODEL like everyone else.
        let persistent_story_planner = {
            let caps = connector_registry.list_capabilities();
            let planner_model =
                std::env::var("LLM_DEFAULT_MODEL").unwrap_or_else(|_| "gpt-4".to_string());
            Arc::new(PersistentStoryPlanner::new(
                llm_client.clone(),
                planner_model,
                caps,
            ))
        };

        // F3 — wire CapabilityDiscovery as a production-reachable service.
        // Builder chain: registry (segment 1) + LockedSkillHubIndex (segment 2)
        // + LLM client for modality probe (segment 5). Segments 3 (cmd_runtime)
        // + 6 (capability_request) need no construction-time wiring; segment 4
        // (SkillHub remote) is left unwired pending publisher-trust signoff.
        let capability_discovery = {
            let skill_index: Arc<dyn SkillIndex> = Arc::new(LockedSkillHubIndex {
                inner: skill_hub.clone(),
            });
            let cd = CapabilityDiscovery::new(connector_registry.clone(), skill_index)
                .with_llm_client(llm_client.clone());
            Arc::new(cd)
        };

        // F7 — DeferredToolRegistry: seed with BuiltinToolRegistry default
        // facades, all marked Active by default. Operators can flip names to
        // Deferred via the admin API to keep the prompt small while still
        // letting the LLM discover them via ToolSearch-style retrieval.
        //
        // F8 Phase 3 (2026-05-06) — additionally aggregate the 6 connector
        // modules' `capability_facades()` so the LLM tool palette includes
        // every facade declared by its owner Connector (cmd: cmd_run /
        // cmd_run_streaming / cmd_run_powershell, fs: fs.read / fs.write /
        // …, lsp: lsp.* …, etc.). This finally fulfills EVOLUTION_IDIOMS §4
        // — connectors are now the source of truth for their own facades,
        // and the host binary composes them into the registry. Registration
        // is name-keyed so any overlap with default_facades (e.g. `bash` ↔
        // `cmd_run`) becomes a last-write-wins overwrite; for now we keep
        // default_facades pre-existing names intact by registering connector
        // facades AFTER the defaults seed (so any clash is benign — same
        // capability_id underneath either name).
        let deferred_tool_registry = {
            use crate::{memory_connector, todo_connector};
            use cyberclaw_agent_runtime::builtin_tools::{BuiltinToolRegistry, ToolsetConfig};
            use cyberclaw_connectors::browser as browser_connector;
            use cyberclaw_connectors::local::{
                cmd as cmd_facades, fs as fs_facades, lsp as lsp_facades,
                memory as memory_local_facades, search as search_facades, slides as slides_facades,
                task as task_facades, verify as verify_facades, web as web_facades,
                workdir_checkpoint as wd_facades,
            };
            use cyberclaw_connectors::mcp::facades as mcp_facades;
            use cyberclaw_core::facade::{CapabilityFacade, FacadeExposure};
            let mut registry = DeferredToolRegistry::new(50);
            let builtin = BuiltinToolRegistry::with_defaults();
            let mut active_count = 0usize;
            let mut deferred_count = 0usize;
            let mut hidden_count = 0usize;
            // F12 Phase B (2026-05-06) — route by FacadeExposure:
            //   LlmDefault   → register_active   (LLM sees full schema)
            //   LlmAdvanced  → register_deferred (LLM sees name, fetches via ToolSearch)
            //   Internal     → drop              (only admin REST endpoints can reach)
            //   AdminOnly    → drop              (operator-token routes only)
            let mut route = |facade: CapabilityFacade,
                             active: &mut usize,
                             deferred: &mut usize,
                             hidden: &mut usize| {
                match facade.exposure {
                    FacadeExposure::LlmDefault => {
                        registry.register_active(facade);
                        *active += 1;
                    }
                    FacadeExposure::LlmAdvanced => {
                        registry.register_deferred(facade);
                        *deferred += 1;
                    }
                    FacadeExposure::Internal | FacadeExposure::AdminOnly => {
                        // Intentionally not registered into the LLM-facing
                        // DeferredToolRegistry. Reachable only via admin
                        // REST handlers that look up capabilities directly.
                        *hidden += 1;
                    }
                }
            };
            for facade in builtin.get_facades(&ToolsetConfig::default_config()) {
                route(
                    facade,
                    &mut active_count,
                    &mut deferred_count,
                    &mut hidden_count,
                );
            }
            // §4 connector-owned facades.
            for (facade, _cat) in cmd_facades::capability_facades() {
                route(
                    facade,
                    &mut active_count,
                    &mut deferred_count,
                    &mut hidden_count,
                );
            }
            for (facade, _cat) in fs_facades::capability_facades() {
                route(
                    facade,
                    &mut active_count,
                    &mut deferred_count,
                    &mut hidden_count,
                );
            }
            for (facade, _cat) in lsp_facades::capability_facades() {
                route(
                    facade,
                    &mut active_count,
                    &mut deferred_count,
                    &mut hidden_count,
                );
            }
            for (facade, _cat) in search_facades::capability_facades() {
                route(
                    facade,
                    &mut active_count,
                    &mut deferred_count,
                    &mut hidden_count,
                );
            }
            for (facade, _cat) in task_facades::capability_facades() {
                route(
                    facade,
                    &mut active_count,
                    &mut deferred_count,
                    &mut hidden_count,
                );
            }
            for (facade, _cat) in wd_facades::capability_facades() {
                route(
                    facade,
                    &mut active_count,
                    &mut deferred_count,
                    &mut hidden_count,
                );
            }
            // F12 Phase C — 7 new connector modules.
            for (facade, _cat) in web_facades::capability_facades() {
                route(
                    facade,
                    &mut active_count,
                    &mut deferred_count,
                    &mut hidden_count,
                );
            }
            for (facade, _cat) in memory_local_facades::capability_facades() {
                route(
                    facade,
                    &mut active_count,
                    &mut deferred_count,
                    &mut hidden_count,
                );
            }
            for (facade, _cat) in verify_facades::capability_facades() {
                route(
                    facade,
                    &mut active_count,
                    &mut deferred_count,
                    &mut hidden_count,
                );
            }
            for (facade, _cat) in browser_connector::capability_facades() {
                route(
                    facade,
                    &mut active_count,
                    &mut deferred_count,
                    &mut hidden_count,
                );
            }
            for (facade, _cat) in mcp_facades::capability_facades() {
                route(
                    facade,
                    &mut active_count,
                    &mut deferred_count,
                    &mut hidden_count,
                );
            }
            for (facade, _cat) in memory_connector::capability_facades() {
                route(
                    facade,
                    &mut active_count,
                    &mut deferred_count,
                    &mut hidden_count,
                );
            }
            for (facade, _cat) in todo_connector::capability_facades() {
                route(
                    facade,
                    &mut active_count,
                    &mut deferred_count,
                    &mut hidden_count,
                );
            }
            // Avoid unused-symbol warning if slides exposes no capability_facades yet.
            let _ = slides_facades::SlidesRenderInput {
                markdown: String::new(),
                output_path: String::new(),
                title: None,
            };
            tracing::info!(
                active_count,
                deferred_count,
                hidden_count,
                total = active_count + deferred_count + hidden_count,
                "DeferredToolRegistry seeded: §4 connector facades + exposure routing"
            );
            // 2026-05-17 (v1.2.8): apply persisted operator overrides from
            // ~/.cyberclaw/tools.json on top of the default seeding. Without
            // this, every `cyberclaw-cli tools promote X` is lost on restart.
            crate::tools_overrides::apply_overrides(&mut registry);
            Arc::new(RwLock::new(registry))
        };

        // F6 — McpToolBridge: default config (namespace separator "__") so
        // admin/dev tooling can bridge MCP server tools without per-instance
        // configuration. Per-tool risk classification kicks in inside
        // bridge_tool() based on tool-name patterns.
        let mcp_tool_bridge = Arc::new(McpToolBridge::with_defaults());

        // F4 — Multi-node coordination primitives.
        // Single-node deployments: LeastLoadedAssigner has no tracked nodes so
        // assign_session() returns NoAvailableNodes until a remote brain
        // registers via POST /api/v1/cluster/brain/register. Harmless for
        // single-node because session assignment is only used by the
        // /cluster/sessions/assign admin endpoint.
        let heartbeat_monitor = Arc::new(
            cyberclaw_control_plane::cluster::node::HeartbeatMonitor::new(
                std::time::Duration::from_secs(30),
            ),
        );
        let session_store: Arc<dyn cyberclaw_control_plane::distributed::SessionStore> =
            Arc::new(cyberclaw_control_plane::distributed::InMemorySessionStore::default());
        let brain_coordinator =
            Arc::new(cyberclaw_control_plane::distributed::BrainCoordinator::new(
                Arc::new(
                    cyberclaw_control_plane::cluster::node::LeastLoadedAssigner::new(
                        heartbeat_monitor.clone(),
                    ),
                ),
                session_store.clone(),
            ));

        // Sprint 21 — Multi-Agent Handoff queue.
        let handoff_queue: Arc<dyn HandoffQueue> = Arc::new(InMemoryHandoffQueue::new());

        // S21 T20 — Feature flag: CYBERCLAW_HANDOFF_ENABLED=true|1 opts in; default false.
        let handoff_feature_enabled = std::env::var("CYBERCLAW_HANDOFF_ENABLED")
            .ok()
            .as_deref()
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        // Sprint 21 T8 — Register HandoffConnector so the gateway can dispatch
        // "agent.handoff" capability calls to the in-process queue.
        // Gated on T20 feature flag: only register when handoff is enabled, so
        // /api/v1/connectors and dashboard counts stay clean when off.
        if handoff_feature_enabled {
            use cyberclaw_core::ids::ConnectorId;
            let sink: Arc<dyn HandoffSink> = Arc::new(QueueSink(handoff_queue.clone()));
            let review_sink: Arc<dyn ReviewQueueSink> =
                Arc::new(ReviewQueueSinkAdapter(cp_components.review_queue.clone()));
            let handoff_connector = HandoffConnector::new(
                ConnectorId::from_string("handoff".to_string())
                    .expect("static connector id is valid"),
                sink,
                Arc::new(ToolOutputSanitizer::new()),
                policy_engine.clone(),
                review_sink,
            );
            connector_registry
                .register(Arc::new(handoff_connector))
                .expect("failed to register HandoffConnector");
        }

        // Materialize installed-packages store at construction time so
        // the boot path can drive load + restore before the router serves
        // its first request. The same Arc is exposed on AppState for the
        // `/api/v2/packages` install/uninstall handlers.
        let installed_packages =
            Arc::new(crate::installed_packages::InstalledPackageStore::load_default());

        Self {
            llm_client,
            connector_registry,
            package_registry: cp_components.package_registry,
            installed_packages,
            capability_dispatcher,
            policy_engine,
            control_plane: cp_components.control_plane,
            task_manager: cp_components.task_manager,
            execution_service: cp_components.execution_service,
            review_queue: cp_components.review_queue,
            tool_mapper,
            tool_executor,
            event_recorder,
            agent_runtime,
            jwt_secret: Arc::new(jwt_secret),
            task_store,
            agent_status_store,
            assignment_queue,
            workflow_engine,
            trigger_registry,
            webhook_secrets,
            webhook_tenants,
            wizard_registry,
            wizard_bootstrap_token,
            skill_hub,
            publisher_ring,
            channels_store,
            admin_event_bus,
            node_id,
            started_at,
            admin_store,
            audit: None,
            skill_index: None,
            skill_compat: Arc::new(SkillCompatRegistry::with_default()),
            digest_repo,
            pipeline_config,
            permission_rule_proposals: Arc::new(RwLock::new(Vec::new())),
            org_memory_store: Arc::new(RwLock::new(Vec::new())),
            memory_store,
            memory_sanitizer,
            clarify_queue,
            clarify_coordinator,
            clarify_broadcaster,
            sse_replay: crate::sse_replay::SseReplayBuffer::new(),
            handoff_queue,
            handoff_feature_enabled,
            embed_client,
            curator: None,
            persistent_story_planner,
            capability_discovery,
            deferred_tool_registry,
            mcp_tool_bridge,
            heartbeat_monitor,
            session_store,
            brain_coordinator,
            usage: Arc::new(crate::usage::UsageCounters::new()),
            cron_store: crate::cron::CronStore::load_default(),
            profile_store: ProfileStore::load_default(),
            // v1.3 WP-1 — single global in-memory conversation session store.
            // Cap defaults to InMemorySessionStore::DEFAULT_MAX_SESSIONS (1000).
            conversation_session_store: Arc::new(
                cyberclaw_store::InMemorySessionStore::new(),
            ),
        }
    }

    /// High-level API: publish a clarify request to all SSE subscribers of
    /// `conversation_id`, then await the user's answer via the coordinator.
    ///
    /// # Flow
    ///
    /// 1. Broadcast `ClarifyEvent::Requested(req)` to all active SSE connections
    ///    for this conversation (fire-and-forget; 0 receivers is normal).
    /// 2. Delegate to `clarify_coordinator.ask(req, timeout)` which enqueues the
    ///    request and blocks until the HTTP `respond` endpoint calls
    ///    `notify_resolved()`.
    ///
    /// # Returns
    ///
    /// `Ok(ClarifyAnswer)` when the user responds within `timeout`, or a
    /// `ClarifyError` variant (e.g. `AlreadyTimedOut`) on failure.
    pub async fn ask_user_clarify(
        &self,
        conversation_id: &str,
        req: cyberclaw_core::clarify::ClarifyRequest,
        timeout: std::time::Duration,
    ) -> Result<cyberclaw_core::clarify::ClarifyAnswer, cyberclaw_core::clarify::ClarifyError> {
        use crate::clarify_broadcast::ClarifyEvent;

        // Step 1: fan-out to SSE subscribers (best-effort, ignore count).
        self.clarify_broadcaster
            .publish(conversation_id, ClarifyEvent::Requested(req.clone()))
            .await;

        // Step 2: persist clarify request to conversation history so a page
        // refresh can restore the ClarifyCard state. Spawned as a background
        // task so the write never delays the enqueue+waiter registration in
        // coordinator.ask(), which is timing-sensitive. Failures are swallowed
        // (.ok()) — persistence must never block the primary clarify flow.
        {
            use crate::api::chat_conversations::ChatMessage;
            let clarify_msg = ChatMessage {
                role: "clarify".to_string(),
                content: req
                    .questions
                    .first()
                    .map(|q| q.question.clone())
                    .unwrap_or_default(),
                metadata: Some(serde_json::json!({
                    "clarify_id": req.id.as_str(),
                    "questions": req.questions,
                    "expires_at": req.expires_at,
                })),
                ts: Some(chrono::Utc::now().timestamp_millis()),
            };
            let store = self.conversation_store();
            let conv_id = conversation_id.to_string();
            tokio::spawn(async move {
                store
                    .append_message_internal(&conv_id, clarify_msg)
                    .await
                    .ok();
            });
        }

        // Step 3: enqueue + await answer (blocks until notify_resolved or timeout).
        self.clarify_coordinator.ask(req, timeout).await
    }

    /// Return a handle to the in-process conversation store singleton.
    ///
    /// The store is a process-wide `OnceLock` managed by
    /// `api::chat_conversations::store()` — not a per-`AppState` object. This
    /// accessor exists so handlers in other modules (e.g. `chat_clarify`) can
    /// call `state.conversation_store()` without importing the internal singleton
    /// directly.
    pub fn conversation_store(
        &self,
    ) -> std::sync::Arc<crate::api::chat_conversations::ConversationStore> {
        crate::api::chat_conversations::store()
    }

    /// Attach an append-only audit sink to an already-constructed state.
    ///
    /// Called from `main.rs` after the sink is initialized so the server
    /// boot flow can log the initial startup event. Idempotent: overwrites
    /// any previously attached sink.
    /// Sprint 21 path #3 — attach the skill discovery index. Builder pattern;
    /// production server boot calls this before serving traffic.
    pub fn with_skill_index(mut self, index: Arc<crate::skill_index::SkillIndex>) -> Self {
        self.skill_index = Some(index);
        self
    }

    pub fn with_audit_sink(mut self, sink: Arc<AuditSink>) -> Self {
        self.audit = Some(sink);
        self
    }

    /// W2 D-1 — attach an autonomous skill Curator. The curator drives
    /// `/api/v1/admin/curator/status` and `/run-now`; without this the
    /// admin endpoints surface "disabled" / 501.
    pub fn with_curator(mut self, curator: Arc<Curator>) -> Self {
        self.curator = Some(curator);
        self
    }
}

/// Build the default Curator wired against the on-disk skills root and a
/// `HeuristicEvaluator` (no LLM dependency). The HeuristicEvaluator is
/// pure age + use_count math, which is enough for status/run-now to
/// return real data without coupling the boot path to a LLM client.
pub fn build_default_curator() -> Arc<Curator> {
    let skills_root = std::env::var_os("CYBERCLAW_SKILLS_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(|h| {
                    std::path::PathBuf::from(h)
                        .join(".cyberclaw")
                        .join("skills")
                })
                .unwrap_or_else(|| std::path::PathBuf::from(".cyberclaw").join("skills"))
        });
    let _ = std::fs::create_dir_all(&skills_root);

    let usage = SkillUsageStore::default_for(&skills_root)
        .map(Arc::new)
        .unwrap_or_else(|err| {
            tracing::warn!(
                path = %skills_root.display(),
                %err,
                "SkillUsageStore::default_for failed; falling back to in-memory store"
            );
            Arc::new(SkillUsageStore::in_memory())
        });

    let config = CuratorConfig::for_skills_root(skills_root);
    let evaluator: Arc<dyn cyberclaw_skill_runtime::curator::SkillEvaluator> =
        Arc::new(HeuristicEvaluator);

    Arc::new(Curator::new(config, usage, evaluator))
}

/// Build the default `SkillHub` rooted at `~/.cyberclaw/skills`.
///
/// Falls back to a temp-dir-like `.cyberclaw/skills` under CWD if `$HOME`
/// is unset. Silently degrades to a hub rooted at `std::env::temp_dir()`
/// if directory creation fails so the server still boots.
///
/// After construction, the hub is seeded from `ecosystem/skills/` so the
/// admin console's `/api/v1/skills` surface shows the repo's built-in
/// skills on a freshly-booted server. The directory is located via
/// `CYBERCLAW_ECOSYSTEM_DIR` (set by main.rs / tests) or the workspace
/// layout relative to `CARGO_MANIFEST_DIR`.
fn build_skill_hub() -> SkillHub {
    let base = std::env::var_os("HOME")
        .map(|h| {
            std::path::PathBuf::from(h)
                .join(".cyberclaw")
                .join("skills")
        })
        .unwrap_or_else(|| std::path::PathBuf::from(".cyberclaw").join("skills"));

    let mut hub = SkillHub::new(base.clone()).unwrap_or_else(|err| {
        tracing::warn!(
            path = %base.display(),
            %err,
            "failed to initialize SkillHub at default path, falling back to temp dir"
        );
        let fallback = std::env::temp_dir().join("cyberclaw-skills");
        SkillHub::new(fallback).expect("temp-dir skill hub must initialize")
    });

    // Skip ecosystem seeding under `cargo test` or when the operator opts
    // out via `CYBERCLAW_SKIP_ECOSYSTEM_SEED=1`. Tests share a HOME-rooted
    // skill hub and parallel seed runs race on `installed/` mutations.
    let skip_seed = cfg!(test)
        || std::env::var("CYBERCLAW_SKIP_ECOSYSTEM_SEED")
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes"))
            .unwrap_or(false);

    if !skip_seed {
        if let Some(eco_skills_dir) = ecosystem_skills_dir() {
            let installed = seed_hub_from_ecosystem(&mut hub, &eco_skills_dir);
            tracing::info!(
                path = %eco_skills_dir.display(),
                count = installed,
                "✓ SkillHub seeded from ecosystem/skills"
            );
        } else {
            tracing::debug!("no ecosystem skills directory found, skipping hub seed");
        }
    }

    hub
}

/// Resolve the `ecosystem/skills/` directory.
///
/// Priority:
/// 1. `CYBERCLAW_ECOSYSTEM_DIR` / `ECOSYSTEM_DIR` env var (+ `/skills`)
/// 2. `$CARGO_MANIFEST_DIR/../../ecosystem/skills` (workspace dev layout)
/// 3. `$PWD/ecosystem/skills` (common invocation from repo root)
fn ecosystem_skills_dir() -> Option<std::path::PathBuf> {
    let from_env = std::env::var_os("CYBERCLAW_ECOSYSTEM_DIR")
        .or_else(|| std::env::var_os("ECOSYSTEM_DIR"))
        .map(std::path::PathBuf::from);
    let candidates = [
        from_env.map(|p| p.join("skills")),
        Some(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ecosystem/skills")),
        std::env::current_dir()
            .ok()
            .map(|p| p.join("ecosystem").join("skills")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|candidate| candidate.is_dir())
}

/// Scan `eco_skills_dir`, register each subdirectory with a `SKILL.md`
/// as a `SkillBundle`, then run the full download + scan + install flow.
///
/// Only directories that ship a `SKILL.md` count as skills — top-level
/// files like `README.md` are silently skipped. Returns the number of
/// skills that reached the `Installed` state.
fn seed_hub_from_ecosystem(hub: &mut SkillHub, eco_skills_dir: &std::path::Path) -> usize {
    hub.add_source(SkillSource::Local {
        path: eco_skills_dir.to_path_buf(),
    });

    let scanner = SkillScanner::new();
    let entries = match std::fs::read_dir(eco_skills_dir) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!(
                path = %eco_skills_dir.display(),
                %err,
                "failed to read ecosystem skills dir; skipping seed"
            );
            return 0;
        }
    };

    let mut installed = 0usize;
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !entry.path().join("SKILL.md").exists() {
            continue;
        }

        let bundle = SkillBundle {
            name: name.clone(),
            version: "0.0.0".to_string(),
            description: format!("Built-in ecosystem skill: {name}"),
            source: format!("ecosystem:{name}"),
            trust_level: SkillTrustLevel::Builtin,
            sha256: None,
            signature: None,
            publisher_fingerprint: None,
        };
        hub.register_bundle(bundle.clone());

        match hub.download(&bundle) {
            Ok(_) => match hub.scan_and_install(&bundle, &scanner) {
                Ok(_state) => {
                    installed += 1;
                }
                Err(err) => {
                    tracing::warn!(
                        skill = %name,
                        %err,
                        "failed to scan/install ecosystem skill; skipping"
                    );
                }
            },
            Err(err) => {
                tracing::warn!(
                    skill = %name,
                    %err,
                    "failed to download ecosystem skill into quarantine; skipping"
                );
            }
        }
    }
    installed
}

/// Generate and log a one-time bootstrap token if no operators exist yet.
///
/// Returns `None` when `~/.cyberclaw/users.toml` already lists at least one
/// operator — in that case the wizard is gated by JWT instead.
fn maybe_generate_bootstrap_token() -> Option<String> {
    use cyberclaw_core::users::UsersConfig;

    let path = UsersConfig::default_path();
    let need_token = match UsersConfig::load_from_path(&path) {
        Ok(cfg) => cfg.users.is_empty(),
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                %err,
                "failed to load users.toml; treating as empty and requiring bootstrap token"
            );
            true
        }
    };
    if !need_token {
        return None;
    }

    // 32 random bytes, hex-encoded (64 chars). Sourced from two UUID v4s so
    // we don't pull in a new crate just for this.
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(a.as_bytes());
    bytes[16..].copy_from_slice(b.as_bytes());
    let token = bytes
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();

    // Print exactly once. Banner is intentionally noisy so the operator
    // cannot miss it on a long server log.
    println!();
    println!("============================================================");
    println!("  CyberClaw bootstrap token (first-run wizard only)");
    println!("  Pass this in the `X-Wizard-Bootstrap-Token` header when");
    println!("  calling /wizard/* until the first operator is registered:");
    println!();
    println!("  {}", token);
    println!();
    println!("  This token is printed ONCE and discarded after the wizard");
    println!("  writes an operator to ~/.cyberclaw/users.toml.");
    println!("============================================================");
    println!();

    tracing::info!("Generated one-time wizard bootstrap token (see stdout banner)");
    Some(token)
}

/// 从环境变量加载 webhook 签名密钥。
///
/// 读取所有 `CYBERCLAW_WEBHOOK_SECRET_<PLATFORM>` 环境变量，
/// 例如 `CYBERCLAW_WEBHOOK_SECRET_GITHUB=abc123` 会映射为 `github -> abc123`。
fn load_webhook_secrets() -> HashMap<String, String> {
    let mut secrets = HashMap::new();
    let prefix = "CYBERCLAW_WEBHOOK_SECRET_";
    for (key, value) in std::env::vars() {
        if let Some(platform) = key.strip_prefix(prefix) {
            let platform = platform.to_lowercase();
            secrets.insert(platform, value);
        }
    }
    secrets
}

/// 从环境变量加载 webhook 平台到租户的绑定。
///
/// 读取所有 `CYBERCLAW_WEBHOOK_TENANT_<PLATFORM>` 环境变量，
/// 例如 `CYBERCLAW_WEBHOOK_TENANT_GITHUB=tenant-a` 会映射为 `github -> tenant-a`。
fn load_webhook_tenants() -> HashMap<String, TenantId> {
    let mut tenants = HashMap::new();
    let prefix = "CYBERCLAW_WEBHOOK_TENANT_";

    for (key, value) in std::env::vars() {
        if let Some(platform) = key.strip_prefix(prefix) {
            let platform = platform.to_lowercase();
            match TenantId::from_string(value) {
                Ok(tenant_id) => {
                    tenants.insert(platform, tenant_id);
                }
                Err(error) => {
                    tracing::warn!(
                        %platform,
                        %error,
                        "Ignoring invalid webhook tenant binding"
                    );
                }
            }
        }
    }

    tenants
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_llm::prelude::GenericOpenAiClient;
    use cyberclaw_llm_bridge::register_standard_mappings;

    struct EnvVarGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(original) = &self.original {
                unsafe {
                    std::env::set_var(self.key, original);
                }
            } else {
                unsafe {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    #[test]
    fn test_parse_csv_env_list() {
        let parsed = parse_csv_env_list("Read, Write,  WebSearch  ,,");
        assert_eq!(
            parsed,
            vec![
                "Read".to_string(),
                "Write".to_string(),
                "WebSearch".to_string()
            ]
        );
    }

    #[test]
    fn test_load_tool_filter_env() {
        let key = "CYBERCLAW_TEST_TOOL_ALLOWLIST";
        let _guard = EnvVarGuard::set(key, "Read, Write,Web*");
        let allow = load_tool_filter_env(key);
        assert_eq!(
            allow,
            vec!["Read".to_string(), "Write".to_string(), "Web*".to_string()]
        );
    }

    #[test]
    fn test_apply_tool_visibility_filters_from_env_chain() {
        let allow_key = "CYBERCLAW_TEST_TOOL_ALLOWLIST_CHAIN";
        let deny_key = "CYBERCLAW_TEST_TOOL_DENYLIST_CHAIN";
        let _allow_guard = EnvVarGuard::set(allow_key, "Read,Write");
        let _deny_guard = EnvVarGuard::set(deny_key, "Write");

        let mapper = ToolCallMapper::new();
        register_standard_mappings(&mapper).expect("register standard mappings");

        let allow = load_tool_filter_env(allow_key);
        let deny = load_tool_filter_env(deny_key);
        apply_tool_visibility_filters(&mapper, &allow, &deny);

        // allowlist 生效：未命中的 canonical host 工具应被移除
        assert!(!mapper.has_tool("TaskCreate"));
        // deny 优先生效：即使命中 allow，也会被 deny 移除
        assert!(!mapper.has_tool("Write"));
        // 命中 allow 且未被 deny 的工具保留
        assert!(mapper.has_tool("Read"));
    }

    #[test]
    fn test_load_webhook_tenant_bindings() {
        let _github = EnvVarGuard::set("CYBERCLAW_WEBHOOK_TENANT_GITHUB", "tenant-github");
        let _slack = EnvVarGuard::set("CYBERCLAW_WEBHOOK_TENANT_SLACK", "tenant-slack");

        let tenants = load_webhook_tenants();

        assert_eq!(tenants.len(), 2);
        assert_eq!(
            tenants.get("github").map(|tenant| tenant.as_ref()),
            Some("tenant-github")
        );
        assert_eq!(
            tenants.get("slack").map(|tenant| tenant.as_ref()),
            Some("tenant-slack")
        );
    }

    #[tokio::test]
    async fn test_app_state_creation() {
        use cyberclaw_control_plane::{
            event_bus::InMemoryEventBus, execution_service::InMemoryExecutionService,
            gateway_router::InMemoryGatewayRouter, lease_manager::InMemoryLeaseManager,
            membership_service::InMemoryMembershipService, orchestrator::ControlPlaneOrchestrator,
            placement_engine::InMemoryPlacementEngine, registry::InMemoryRegistry,
            resolver::InMemoryResolver, review_queue::InMemoryReviewQueue,
            task_manager::InMemoryTaskManager,
        };
        use cyberclaw_observability::security_event_store::InMemorySecurityEventStore;

        // 创建 LLM 客户端
        let llm_client: Arc<dyn LlmClient> =
            Arc::new(GenericOpenAiClient::new(None, "http://localhost:8080").unwrap());

        // 创建 Connector 注册表
        let connector_registry = Arc::new(ConnectorRegistry::new());

        // 创建策略引擎
        let policy_engine: Arc<dyn PolicyEngine> = Arc::new(DefaultPolicyEngine::default());

        // 创建事件记录器
        let event_recorder = Arc::new(InMemoryEventRecorder::new());

        // 创建 Control Plane 组件
        let task_manager = Arc::new(InMemoryTaskManager::new());
        let execution_service = Arc::new(InMemoryExecutionService::with_event_recorder(
            event_recorder.clone(),
        ));
        let registry = Arc::new(InMemoryRegistry::new());
        let resolver = Arc::new(InMemoryResolver::new(registry.clone()));
        let review_queue = Arc::new(InMemoryReviewQueue::new(None));

        let gateway = Arc::new(InMemoryGatewayRouter::new());
        let placement_engine = Arc::new(InMemoryPlacementEngine);
        let lease_manager = Arc::new(InMemoryLeaseManager::default());
        let membership_service = Arc::new(InMemoryMembershipService::default());
        let event_bus = Arc::new(InMemoryEventBus::default());
        let security_event_store = Arc::new(InMemorySecurityEventStore::new());

        // 创建 PluginRegistry 实例
        let plugin_registry = Arc::new(cyberclaw_plugin_runtime::PluginRegistry::new(
            std::path::PathBuf::from("/tmp/test_plugins"),
        ));

        let control_plane = Arc::new(
            ControlPlaneOrchestrator::new(
                gateway,
                resolver,
                registry.clone(),
                review_queue.clone(),
                task_manager.clone(),
                execution_service.clone(),
                placement_engine,
                lease_manager,
                membership_service,
                event_bus,
                policy_engine.clone(),
                plugin_registry,
                security_event_store,
            )
            .with_event_recorder(event_recorder.clone()),
        );

        // 创建 Control Plane 组件集合
        let cp_components = ControlPlaneComponents {
            control_plane,
            task_manager,
            execution_service,
            review_queue,
            package_registry: registry,
        };

        // 创建应用状态
        let state = AppState::new(
            llm_client,
            connector_registry,
            policy_engine,
            event_recorder,
            "test-jwt-secret".to_string(),
            cp_components,
        );

        // 验证状态已正确初始化
        assert!(Arc::strong_count(&state.connector_registry) >= 1);
        assert!(Arc::strong_count(&state.policy_engine) >= 1);
        assert!(Arc::strong_count(&state.event_recorder) >= 1);
        assert_eq!(*state.jwt_secret, "test-jwt-secret");
    }
}
