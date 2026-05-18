//! # CyberClaw Control Plane
//!
//! 控制面板提供 CyberClaw 平台的核心运行时组件，包括编排、执行、治理和分布式支持。
//!
//! ## 架构层次
//!
//! ### 执行层
//! - `execution_service` - 核心执行服务
//! - `orchestrator` - 编排引擎
//! - `task_manager` - 任务管理
//! - `subagent_scheduler` - 子代理调度
//!
//! ### 治理层
//! - `review_queue` - 评审队列
//! - `autopilot_security` - 安全网关
//! - `provenance_tracker` - 溯源追踪
//!
//! ### 分布式支持
//! - `membership_service` - 节点成员管理
//! - `placement_engine` - 任务放置策略
//! - `lease_manager` - 分布式锁管理
//! - `shared_state_store` - 共享状态存储
//! - `cluster` - 集群管理（节点通信、任务分发、健康检查）
//!
//! ### Autopilot 模式
//! - `autopilot_runtime` - 自动驾驶运行时
//! - `autopilot_iteration` - 迭代控制
//! - `autopilot_progress` - 进度跟踪
//!
//! ## 使用示例
//!
//! ```rust
//! use cyberclaw_control_plane::{AutopilotWorkspace, InMemorySharedStateStore};
//! use std::path::Path;
//!
//! # fn example() -> anyhow::Result<()> {
//! // 创建工作空间
//! let workspace = AutopilotWorkspace::new(Path::new("/tmp/workspace"))?;
//!
//! // 创建共享状态存储
//! let store = InMemorySharedStateStore::new();
//! # Ok(())
//! # }
//! ```

// Auto Mode Gate
/// Auto Mode Gate 模块 - Autopilot 模式权限动态收窄
pub mod auto_mode_gate;
/// 熔断器模块 - 连续失败自动退出 Autopilot
pub mod circuit_breaker;
/// Plan Mode Gate 模块 - Plan 模式只读权限缩放（OMC plan 语义）
pub mod plan_mode_gate;

// Milestone C: Multi-node Foundation v1
/// 工件存储模块 - 分布式工件管理
pub mod artifact_store;
/// 集群管理模块 - 节点通信、任务分发、健康检查
pub mod cluster;
/// 分布式 Brain 与会话外部化模块 - StatelessBrain, AgenticLoopPool, SessionStore, BrainCoordinator
pub mod distributed;
/// 事件总线模块 - 异步事件分发
pub mod event_bus;
/// 租约管理模块 - 分布式锁和租约
pub mod lease_manager;
/// 成员服务模块 - 集群节点管理
pub mod membership_service;
/// 放置引擎模块 - 任务分配策略
pub mod placement_engine;
/// 共享状态存储模块 - 分布式状态同步
pub mod shared_state_store;

// Milestone M4: Runtime Isolation & Provenance
/// 溯源追踪模块 - 执行链完整追踪
pub mod provenance_tracker;

// Skill Evolution (EverOS-inspired Case -> Cluster -> Skill pipeline)
/// 自进化外环 — 周期级摘要数据结构（对齐 Evolver signals.js EvolutionEvent shape）。
pub mod cycle_summary;
/// DailyDigest — per-agent reflection loop scaffold (Sprint 8 L6).
pub mod daily_digest;
/// DailyDigest LLM-powered reflection summarizer — Sprint 9 Wave 3 L4.
pub mod daily_digest_llm;
/// DailyDigest runtime glue — Sprint 9 L9 (StoreDigestCollector + file-backed repository).
pub mod daily_digest_runtime;
/// 自进化外环 — 守护进程（dry-run 循环 + 自适应 sleep + kill switch）。
pub mod evolution_daemon;
/// 自进化外环 — Gene 资产模型 + 默认基因库 + JSON 加载器。
pub mod evolution_gene;
/// EvolutionOrchestrator — drives the self-evolution closed loop (select→mutate→score→verify→archive).
pub mod evolution_orchestrator;
/// 适应度评估器 - 借鉴 Hermes 多维度评分（correctness/procedure/conciseness/length_penalty）
pub mod fitness_evaluator;
/// 自进化外环 — 历史分析 / 信号频次抑制 / 停滞检测（对齐 Evolver signals.js analyzeRecentHistory）。
pub mod history_analyzer;
/// IntentClassifier — keyword-based intent routing (create skill / agent / brainstorm / digest).
pub mod intent_classifier;
/// 自进化外环 — 周期摘要 JSONL 持久化（对齐 Evolver assetStore.js appendEventJsonl）。
pub mod jsonl_event_sink;
/// Sprint 21 path #2 — LLM-backed `EvolutionDispatcher` impl. Wires the
/// orchestrator to a deployed LLM client so description-optimisation
/// cycles can actually run.
pub mod llm_evolution_dispatcher;
/// Mutation Engine — plan-only mutation orchestration for Skill variants.
pub mod mutation_engine;
/// ProductionEvolutionDispatcher — real-world impl of EvolutionDispatcher wiring to connectors.
pub mod production_evolution_dispatcher;
/// 自进化外环 — 信号抽取（regex 层 + 历史合成信号）。
pub mod signal_extractor;
/// 自进化外环 — 信号→Gene 打分路由 + 抑制惩罚 + 备选项。
pub mod signal_router;
/// Skill 变体归档与进化选择 - 借鉴 HyperAgents DGM-H parent selection
pub mod skill_archive;
/// SkillCreator — narrow façade over EvolutionOrchestrator for Skill authoring/optimization.
pub mod skill_creator;
/// Skill 自进化管道 - 基于执行案例质量评估自动生成/更新 Skill
pub mod skill_evolution;

// Persistent Execution (Ralph-inspired story-driven loop)
/// Capability Discovery (Sprint D2) — stateless query service over native /
/// installed-skill / cmd-runtime / SkillHub / provider-modality / capability-request
/// segments. See `capability_discovery::CapabilityDiscovery`.
pub mod capability_discovery;
/// 持久执行引擎 - 故事驱动的持久化执行循环
pub mod persistent_execution;
/// Sprint D3: LLM-driven Story DAG planner for persistent execution mode.
/// Produces a validated `PersistentExecutionPlan` from a free-form goal
/// using a capability allowlist and a corrective-retry pipeline.
pub mod persistent_story_planner;
/// PRD 生成器 - 目标到故事的结构化分解
pub mod prd_generator;
/// Stage handoff 协议 — paseo-style 阶段交接文件格式 (Sprint 9 L1)
pub mod stage_handoff;
/// Team staged pipeline — team-plan/prd/exec/verify/fix 状态机 (Sprint 9 L1)
pub mod team_pipeline;
/// 验证门 - 基于审查者的完成度验证
pub mod verification_gate;
/// Sprint D4: real `VerifierExecutor` implementations for every
/// `VerifierKind` declared in Sprint D1.
pub mod verifier_impl;

// Sprint 4 — Interactive onboarding state machine
/// WizardEngine — declarative wizard state machine for onboarding flows.
pub mod wizard_engine;

// Autopilot Components
/// Autopilot 迭代控制模块
pub mod autopilot_iteration;
/// Autopilot 6-phase 显式状态机模块（Sprint 9 遗留 — Task #18）
pub mod autopilot_phases;
/// Autopilot 进度追踪模块
pub mod autopilot_progress;
/// Autopilot 运行时模块
pub mod autopilot_runtime;
/// Autopilot 安全模块
pub mod autopilot_security;
/// Autopilot 状态同步模块
pub mod autopilot_state_sync;
/// Autopilot 类型定义模块
pub mod autopilot_types;
/// Autopilot 工作空间模块
pub mod autopilot_workspace;
/// Sprint 10 LLM-driven ExecutionPlanner — produces ExpectedOutcome from goals
pub mod llm_planner;

// Test helpers (only compiled in test builds)
/// 测试辅助工具（仅在测试构建中编译）
#[cfg(test)]
pub mod test_helpers;

// Hook Dispatcher
/// Hook 分发器模块 - 执行链扩展点与失败策略
pub mod hook_dispatcher;

// Hook Integration
/// Hook 集成模块 - 将 HookDispatcher 集成到 MiddlewarePipeline，并提供插件 Hook 加载器
pub mod hook_integration;

/// Kanban dispatcher (R-07 — Hermes v0.12 kanban_tools 等价物)
pub mod kanban;

// Script Hook Handler (BT-22)
/// 把 plugin manifest 里声明的 shell 脚本作为子进程跑起来
pub mod script_hook_handler;

// Middleware Pipeline
/// 中间件管道模块 - 统一执行路径的可插拔中间件机制
pub mod middleware_pipeline;

// Tenant Middleware
/// 租户边界中间件 - 多租户隔离集成到中间件管道
pub mod tenant_middleware;

// Extracted modules from execution_service
/// 命令注入防护模块 - 命令验证与安全执行
pub mod command_safety;
/// 执行 Autopilot 类型模块 - V1 Autopilot 类型与 trait 定义
pub mod execution_autopilot_types;
/// 风险计算模块 - 执行风险等级计算
pub mod risk_calculator;

// Existing modules
/// 自动化模块 - 自动化任务和流程
pub mod automation;
/// 用例管理模块 - 测试用例管理
pub mod case_manager;
/// Clarify 队列模块 - Agent→User 结构化澄清请求队列
pub mod clarify_queue;
/// 生态扫描模块 - Agent/Skill/Connector 发现
pub mod ecosystem_scanner;
/// 执行服务模块 - 核心执行引擎
pub mod execution_service;
/// OrchestratorGateway 生产实现 - 桥接 PolicyEngine → CapabilityDispatcher → Connector
pub mod gateway_impl;
/// 网关路由模块 - 请求路由和分发
pub mod gateway_router;
/// Governed autopilot step runner — bridges ExecutionService to real governance services
pub mod governed_step_runner;
/// Handoff 完成回调 — S22 T2 ReviewTarget::Handoff 审批后的服务层回调接口
pub mod handoff_completion_sink;
/// Handoff 队列模块 - Multi-Agent 会话控制权转移请求队列（Sprint 21）
pub mod handoff_queue;
/// 加载器模块 - 动态组件加载
pub mod loader;
/// 编排器模块 - 任务编排引擎
pub mod orchestrator;
/// 生产级 SecurityGate — 基于 DangerousCapabilityFilter 的安全网关
pub mod production_security_gate;
/// 注册表模块 - 组件注册管理
pub mod registry;
/// 解析器模块 - 能力和依赖解析
pub mod resolver;
/// 重试模块 - 失败重试策略
pub mod retry;
/// 评审队列模块 - 人工评审流程
pub mod review_queue;
/// 子代理调度模块 - 子任务调度
pub mod subagent_scheduler;
/// 任务管理模块 - 任务生命周期管理
pub mod task_manager;
/// 类型定义模块 - 共享类型定义
pub mod types;

// Auto Mode Gate exports
pub use auto_mode_gate::{
    AutoModeConfig, AutoModeGate, DefaultAutoModeGate, ExitReason, PermissionSnapshot,
    StrippedCapability,
};
pub use circuit_breaker::{BreakerDecision, CircuitBreaker, CircuitState};

// Milestone C exports
pub use artifact_store::*;
pub use cluster::*;
pub use event_bus::*;
pub use lease_manager::*;
pub use membership_service::*;
pub use placement_engine::*;
pub use shared_state_store::*;

// Distributed Brain exports
pub use distributed::{
    AgenticLoopPool, BrainCoordinator, DistributedError, ExternalizedSession, InMemorySessionStore,
    SessionRequest, SessionResponse, SessionStore, StatelessBrain,
};

// Milestone M4 exports
pub use provenance_tracker::*;

// Autopilot exports
pub use autopilot_iteration::*;
// Autopilot 6-phase state machine exports (Task #18)
pub use autopilot_phases::{
    validate_transition, AutopilotPhase, AutopilotPhaseDispatcher, PhaseArtifact, PhaseContext,
    PhaseError, PhaseOutcome, PhaseSkipPolicy, PhaseTransition, StubPhaseDispatcher,
};
pub use autopilot_progress::*;
// Selective import from autopilot_runtime to avoid SecurityGate conflict
pub use autopilot_runtime::{GovernedLoopRuntime, SecurityCheckResult};
// Selective import from autopilot_security to avoid SecurityGate conflict
pub use autopilot_security::{
    AutopilotCapabilityWhitelist, DefaultSecurityGate, PromptInjectionDetector,
};
// Re-export the primary SecurityGate from autopilot_security module
pub use autopilot_security::SecurityGate;
pub use autopilot_state_sync::*;
pub use autopilot_workspace::*;
// Note: autopilot_types exports only V2* types to avoid naming conflicts
pub use autopilot_types::{
    AutopilotRunState, AutopilotStatus, ReviewGate, V2ExecutionResult, V2IterationState,
};

// Governed step runner
pub use governed_step_runner::GovernedAutopilotStepRunner;

// Production security gate
pub use production_security_gate::ProductionSecurityGate;

// Hook Dispatcher exports
pub use hook_dispatcher::{
    DispatchResult, FailurePolicy, HookContext, HookDispatcher, HookHandler, HookPoint, HookResult,
};

// Hook Integration exports
pub use hook_integration::{
    HookDeclaration, HookFailurePolicy, HookPhase, HookRegistry, IntegratedHookMiddleware,
    PluginHookLoader,
};

// Middleware Pipeline exports
pub use middleware_pipeline::{
    AuditMiddleware, HookMiddleware, Middleware, MiddlewareError, MiddlewareNext,
    MiddlewarePipeline, MiddlewareRequest, MiddlewareResponse, PolicyMiddleware, TraceMiddleware,
};

// Tenant Middleware exports
pub use tenant_middleware::{TenantContext, TenantMiddleware};

// HandoffCompletionSink exports (S22 T2)
pub use handoff_completion_sink::{HandoffCompletionSink, NoopHandoffCompletionSink};

// Existing exports
pub use automation::*;
pub use case_manager::*;
pub use clarify_queue::InMemoryClarifyQueue;
pub use cyberclaw_core::clarify::ClarifyQueue;
pub use ecosystem_scanner::*;
pub use execution_service::*;
pub use gateway_router::*;
pub use loader::*;
pub use orchestrator::*;
pub use registry::*;
pub use resolver::*;
pub use review_queue::*;
pub use subagent_scheduler::*;
pub use task_manager::*;
pub use types::*;

// Test helpers exports (only in test builds)
#[cfg(test)]
pub use test_helpers::*;
