//! CyberClaw HTTP Server
//!
//! 提供兼容 OpenAI 格式的 Chat Completions API,
//! 集成 CyberClaw 完整执行链:
//! - Control Plane (执行编排)
//! - Connectors & Capabilities (能力分发)
//! - Governance (策略与审批)
//! - Observability (追踪与审计)

// v1.7 final: mimalloc 在 macOS arm64 实测 -15% RPS（libsystem_malloc 已是 nano-malloc）。
// 仅 Linux glibc 部署受益。见 docs/research/perf-tuning-before-after-2026-05-27.md。
// #[global_allocator]
// static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use anyhow::Result;
use std::{env, net::SocketAddr, sync::Arc};
use tokio::signal;
use tokio::sync::oneshot;
use tonic::transport::Server;
use tracing::{error, info, warn};

use cyberclaw_agent_runtime::{AgentRuntime, MinimalAgentRuntime};
use cyberclaw_connectors::{
    dispatcher::CapabilityDispatcher,
    im_adapters::lark::{LarkConfig, LarkDomain, LarkTransportMode},
    im_adapters::wechat::{WechatAdapter, WechatConfig},
    im_adapters::{LarkAdapter, TelegramAdapter, TelegramConfig},
    registry::ConnectorRegistry,
    AcpRuntimeConnector, ImChannelConnector, McpConnector, McpServerConfig, TransportConfig,
    VoiceProcessingConnector,
};
use cyberclaw_consensus::raft::rpc::RaftRpcService;
use cyberclaw_consensus::{
    ConsensusBuilder, PeerInfo as RaftPeerInfo, RaftConfig as RaftNodeConfig, RaftConsensus,
};
use cyberclaw_control_plane::{
    ecosystem_scanner::EcosystemScanner, execution_service::InMemoryExecutionService,
    gateway_router::IngressRequest, orchestrator::ControlPlaneOrchestrator, registry::Registry,
    resolver::InMemoryResolver, review_queue::InMemoryReviewQueue,
    task_manager::InMemoryTaskManager, DefaultProgressEvaluator, GovernedAutopilotStepRunner,
};
use cyberclaw_core::{
    enums::Priority,
    identity::{ActorRef, ActorType},
    ids::{ActorId, NodeId, TaskId},
    task::{Task, TaskInput, TaskKind, TriggerRef},
};
use cyberclaw_governance::engine::{DefaultPolicyEngine, RuleBasedPolicyEngine};
use cyberclaw_governance::rules::RuleSet;
use cyberclaw_llm::prelude::*;
use cyberclaw_observability::events::InMemoryEventRecorder;
use cyberclaw_plugin_runtime::PluginRegistry;
use cyberclaw_scheduler::{
    prelude::*,
    types::{ExecutionResult, TaskAction},
    TaskExecutor,
};
use cyberclaw_server::{
    cluster_assignments::{
        load_pull_worker_config, spawn_assignment_event_collector, spawn_assignment_pull_worker,
    },
    create_router_with_config,
    persistence::PersistenceConfig,
    AppState, ServerConfig,
};
use cyberclaw_skill_runtime::{MinimalSkillRuntime, SkillRuntime};

struct ConsensusRuntimeHandle {
    _consensus: Arc<RaftConsensus>,
    rpc_shutdown_tx: Option<oneshot::Sender<()>>,
    rpc_server_task: Option<tokio::task::JoinHandle<()>>,
}

impl ConsensusRuntimeHandle {
    async fn shutdown(&mut self) {
        if let Some(shutdown_tx) = self.rpc_shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        if let Some(task) = self.rpc_server_task.take() {
            match task.await {
                Ok(_) => {}
                Err(err) if !err.is_cancelled() => {
                    warn!("Raft RPC server task join failed: {}", err);
                }
                Err(_) => {}
            }
        }
    }
}

fn parse_raft_peers(raw: &str) -> Result<Vec<RaftPeerInfo>> {
    let mut peers = Vec::new();
    for entry in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (id, address) = entry
            .split_once('@')
            .ok_or_else(|| anyhow::anyhow!("invalid CYBERCLAW_RAFT_PEERS entry: {}", entry))?;
        peers.push(RaftPeerInfo {
            id: id.to_string(),
            address: address.to_string(),
        });
    }
    Ok(peers)
}

async fn maybe_start_raft_consensus() -> Result<Option<ConsensusRuntimeHandle>> {
    let cluster_mode = env::var("CYBERCLAW_CLUSTER_MODE").unwrap_or_else(|_| "single".to_string());
    if cluster_mode.to_lowercase() != "raft" {
        info!(
            "Cluster mode '{}' detected, Raft consensus runtime disabled",
            cluster_mode
        );
        return Ok(None);
    }

    let node_id = env::var("CYBERCLAW_NODE_ID").unwrap_or_else(|_| "local-server-node".to_string());
    let bind_addr_raw =
        env::var("CYBERCLAW_RAFT_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:7700".to_string());
    let bind_addr: SocketAddr = bind_addr_raw.parse().map_err(|e| {
        anyhow::anyhow!(
            "invalid CYBERCLAW_RAFT_BIND_ADDR '{}': {}",
            bind_addr_raw,
            e
        )
    })?;

    let peers_raw = env::var("CYBERCLAW_RAFT_PEERS").unwrap_or_default();
    let peers = parse_raft_peers(&peers_raw)?;

    if peers.is_empty() {
        warn!("Raft mode enabled, but CYBERCLAW_RAFT_PEERS is empty. Running as standalone node.");
    }

    let raft_config = RaftNodeConfig {
        election_timeout_min: env::var("CYBERCLAW_RAFT_ELECTION_TIMEOUT_MIN_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(150),
        election_timeout_max: env::var("CYBERCLAW_RAFT_ELECTION_TIMEOUT_MAX_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(300),
        heartbeat_interval: env::var("CYBERCLAW_RAFT_HEARTBEAT_INTERVAL_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(50),
        max_append_entries: env::var("CYBERCLAW_RAFT_MAX_APPEND_ENTRIES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100),
    };

    let consensus = Arc::new(
        ConsensusBuilder::new(node_id.clone())
            .with_peers(peers)
            .with_config(raft_config)
            .build_raft(),
    );
    consensus.start().await?;

    let node_handle = consensus.node_handle();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let rpc_server_task = tokio::spawn(async move {
        let service = RaftRpcService::new(node_handle).into_service();
        let result = Server::builder()
            .add_service(service)
            .serve_with_shutdown(bind_addr, async move {
                let _ = shutdown_rx.await;
            })
            .await;
        if let Err(err) = result {
            warn!(
                "Raft RPC server exited with error on {}: {}",
                bind_addr, err
            );
        }
    });

    info!(
        "✓ Raft consensus runtime started (node_id={}, bind_addr={}, peers={})",
        node_id, bind_addr, peers_raw
    );

    Ok(Some(ConsensusRuntimeHandle {
        _consensus: consensus,
        rpc_shutdown_tx: Some(shutdown_tx),
        rpc_server_task: Some(rpc_server_task),
    }))
}

async fn bootstrap_registry_from_ecosystem(
    registry: Arc<cyberclaw_control_plane::registry::InMemoryRegistry>,
) -> Result<()> {
    let default_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ecosystem");
    let ecosystem_path = env::var("CYBERCLAW_ECOSYSTEM_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or(default_path);

    let scanner = EcosystemScanner::new(&ecosystem_path);
    let packages = scanner.scan_all().await.map_err(|e| {
        anyhow::anyhow!(
            "failed to scan ecosystem packages from {}: {}",
            ecosystem_path.display(),
            e
        )
    })?;
    let loaded_count = registry.load_packages(packages).await.map_err(|e| {
        anyhow::anyhow!(
            "failed to load ecosystem packages into registry from {}: {}",
            ecosystem_path.display(),
            e
        )
    })?;
    anyhow::ensure!(
        loaded_count > 0,
        "ecosystem registry bootstrap loaded 0 packages from {}",
        ecosystem_path.display()
    );

    let capability_count = registry.list_capabilities().await?.len();
    info!(
        "✓ Registry bootstrapped from ecosystem: packages={} capabilities={} path={}",
        loaded_count,
        capability_count,
        ecosystem_path.display()
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt::init();

    info!("Starting CyberClaw HTTP Server...");

    // ===== 0. 初始化持久化存储 =====
    let persistence_config = PersistenceConfig::from_env();
    info!("Initializing state store: backend={}", persistence_config);
    let _store = cyberclaw_server::persistence::init_store(&persistence_config)
        .await
        .expect("failed to initialize state store");
    info!("✓ State store initialized ({})", persistence_config);

    // ===== 1. 创建 LLM 客户端 =====
    let llm_client = create_llm_client()?;

    // ===== 2. 创建核心平台组件 =====
    info!("Initializing core platform components...");

    // Connector 注册表
    let connector_registry = Arc::new(ConnectorRegistry::new());

    // 注册默认 LocalConnector，确保 capability 主链可执行（避免空注册表）
    let workspace_path = env::var("CYBERCLAW_WORKSPACE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        });
    // Sprint 19 W4 — auto-detect a container backend (docker/podman). When
    // present, the LocalConnector receives an Arc<ContainerRuntime> so
    // High-risk capabilities (cmd.exec / bash) actually run inside an
    // alpine container with NetworkMode::None. When absent, the connector
    // falls back to the legacy bare-subprocess path. The same Arc is
    // also passed to the dispatcher's `with_container_runtime` builder
    // (later in this file) so the dispatcher's Container branch can
    // attest "Container is configured".
    let container_runtime: Option<Arc<cyberclaw_connectors::runtime::ContainerRuntime>> = {
        match cyberclaw_connectors::runtime::ContainerBackend::detect().await {
            Some(backend) => {
                info!(
                    "Sprint 19 W4: ContainerBackend detected: {:?} — High-risk handlers will isolate via container",
                    backend
                );
                Some(Arc::new(
                    cyberclaw_connectors::runtime::ContainerRuntime::with_backend(backend),
                ))
            }
            None => {
                info!(
                    "Sprint 19 W4: no ContainerBackend (docker/podman) detected — High-risk handlers fall back to subprocess"
                );
                None
            }
        }
    };

    let local_connector = {
        let mut conn = cyberclaw_connectors::LocalConnector::new(workspace_path.clone());
        if let Some(rt) = container_runtime.as_ref() {
            conn = conn.with_container_runtime(rt.clone());
        }
        Arc::new(conn)
    };
    connector_registry
        .register(local_connector)
        .expect("failed to register LocalConnector");

    // 可选：注册 MCP Connector（用于 Claude MCP 工具族）
    maybe_register_mcp_connector(&connector_registry).await?;
    maybe_register_im_voice_remote_connectors(&connector_registry).await?;
    maybe_register_browser_connector(&connector_registry, workspace_path.clone())?;
    // Sprint 20 W1 — opt-in LSP connector registration. When the
    // operator sets `CYBERCLAW_LSP_ENABLED=true` and a rust-analyzer
    // (or other LSP server) binary is available on PATH, register an
    // LspConnector with id="lsp" so the LLM facades (lsp.hover /
    // lsp.diagnostics / etc.) actually dispatch.
    maybe_register_lsp_connector(&connector_registry).await?;

    info!("✓ Connector Registry initialized");

    // 策略引擎 (治理)
    // S27: env-driven engine selection — CYBERCLAW_POLICY_RULES_PATH → RuleBasedPolicyEngine,
    // otherwise DefaultPolicyEngine (risk-based fallback).
    // S28: when CYBERCLAW_POLICY_RULES_RELOAD_SECS is set to a positive integer,
    // spawn a background watcher that re-reads the YAML on mtime changes.
    // Sprint 18 W3 — env-driven review threshold so staging / autopilot
    // demos can run medium-risk tools without manual approval. In prod
    // leave unset (defaults to Low — strictest review).
    fn parse_review_threshold() -> cyberclaw_core::capability::RiskLevel {
        use cyberclaw_core::capability::RiskLevel;
        match std::env::var("CYBERCLAW_POLICY_REVIEW_THRESHOLD")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "low" => RiskLevel::Low,
            "medium" => RiskLevel::Medium,
            "high" => RiskLevel::High,
            "critical" => RiskLevel::Critical,
            _ => RiskLevel::Low,
        }
    }
    let review_threshold = parse_review_threshold();

    let policy_engine: Arc<dyn cyberclaw_governance::engine::PolicyEngine> =
        match std::env::var("CYBERCLAW_POLICY_RULES_PATH") {
            Ok(path) if !path.is_empty() => {
                let rules = RuleSet::from_yaml_path(&path);
                let engine =
                    RuleBasedPolicyEngine::new(rules, DefaultPolicyEngine::new(review_threshold));
                if let Some(secs) = std::env::var("CYBERCLAW_POLICY_RULES_RELOAD_SECS")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                    .filter(|&s| s > 0)
                {
                    let _watcher = engine.start_hot_reload(
                        std::path::PathBuf::from(&path),
                        std::time::Duration::from_secs(secs),
                    );
                    info!(
                        "✓ Policy rules hot-reload watcher started (path={}, interval={}s)",
                        path, secs
                    );
                }
                Arc::new(engine) as Arc<dyn cyberclaw_governance::engine::PolicyEngine>
            }
            _ => Arc::new(DefaultPolicyEngine::new(review_threshold)),
        };
    info!(
        "✓ Policy Engine initialized (review_threshold={:?})",
        review_threshold
    );

    // 事件记录器 (可观测性)
    let event_recorder = Arc::new(InMemoryEventRecorder::new());
    info!("✓ Event Recorder initialized");

    // 可选：启动 Raft 共识运行时（用于多节点集群模式）
    let mut consensus_runtime = maybe_start_raft_consensus().await?;

    // JWT 密钥 (安全) - 必须通过环境变量明确设置，不允许默认值
    // 2026-05-18 (v1.2.11): replaced expect() with anyhow::bail! so the
    // friendly multiline guidance reaches operators without a Rust panic
    // stack trace burying it. main() returns Result<()>, so this exits
    // cleanly with the message instead of crashing.
    let jwt_secret = env::var("JWT_SECRET").map_err(|_| {
        anyhow::anyhow!(
            "CRITICAL: JWT_SECRET environment variable must be set.\n\
             \n\
             Generate a secure secret with:\n\
             openssl rand -base64 48 > ~/.cyberclaw/jwt-secret\n\
             chmod 600 ~/.cyberclaw/jwt-secret\n\
             \n\
             Then export it in your environment:\n\
             export JWT_SECRET=\"$(cat ~/.cyberclaw/jwt-secret)\"\n\
             \n\
             Or use the start script which sources it automatically:\n\
             ~/.cyberclaw/bin/start-cyberclaw.sh\n\
             \n\
             For production, use a secrets manager (HashiCorp Vault, AWS Secrets Manager, etc)."
        )
    })?;

    // 验证密钥强度
    if jwt_secret.len() < 32 {
        return Err(anyhow::anyhow!(
            "JWT_SECRET must be at least 32 characters long for security.\n\
             Current length: {} characters.\n\
             Generate a new secret with: openssl rand -base64 48",
            jwt_secret.len()
        ));
    }
    info!("✓ JWT Secret loaded (length: {} chars)", jwt_secret.len());

    // ===== 3. 初始化 Control Plane 组件 =====
    info!("Initializing Control Plane components...");

    // 创建 TaskManager
    let task_manager = Arc::new(InMemoryTaskManager::new());
    info!("✓ Task Manager initialized");

    // ===== 3.1. 初始化 AgentRuntime 和 SkillRuntime =====
    info!("Initializing Agent and Skill Runtimes...");
    let agent_runtime: Arc<dyn AgentRuntime> = Arc::new(MinimalAgentRuntime::new());
    let skill_runtime: Arc<dyn SkillRuntime> = Arc::new(MinimalSkillRuntime::new());
    info!("✓ AgentRuntime and SkillRuntime initialized");

    // 创建 CapabilityDispatcher 并注入到 ExecutionService（修复 capability 执行装配缺失）
    let mut dispatcher = CapabilityDispatcher::new(connector_registry.clone())
        .with_event_recorder(event_recorder.clone());
    if let Some(rt) = container_runtime.as_ref() {
        dispatcher = dispatcher.with_container_runtime(rt.clone());
    }
    let capability_dispatcher = Arc::new(dispatcher);

    // 根据环境选择 SecurityGate 实现
    // Deny-by-default: 仅 development/test 环境使用 PassthroughSecurityGate，
    // 其余所有环境（包括未设置 ENVIRONMENT 时）默认使用 ProductionSecurityGate。
    let current_env = std::env::var("ENVIRONMENT")
        .unwrap_or_else(|_| "production".to_string())
        .to_lowercase();
    let security_gate: Arc<dyn cyberclaw_control_plane::autopilot_runtime::SecurityGate> =
        if current_env == "development" || current_env == "test" {
            tracing::warn!(
                "Using PassthroughSecurityGate — environment='{}' (development/test only)",
                current_env
            );
            Arc::new(PassthroughSecurityGate)
        } else {
            info!(
                "Using ProductionSecurityGate with DangerousCapabilityFilter (environment='{}')",
                current_env
            );
            Arc::new(cyberclaw_control_plane::ProductionSecurityGate::with_defaults())
        };

    // 创建 ExecutionService（注入 Runtimes + CapabilityDispatcher + LLM for S19 A memory summary）
    // Sprint 25 S25 T3: env-var-driven embed client (mirrors AppState::new logic).
    let embed_enabled_main = std::env::var("CYBERCLAW_EMBED_ENABLED")
        .ok()
        .as_deref()
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);
    let embed_client_for_svc: Arc<dyn cyberclaw_llm::EmbedClient> = if embed_enabled_main {
        let base_url = std::env::var("CYBERCLAW_EMBED_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
        let api_key = std::env::var("CYBERCLAW_EMBED_API_KEY").ok();
        let model = std::env::var("CYBERCLAW_EMBED_MODEL")
            .unwrap_or_else(|_| "text-embedding-3-small".to_string());
        let dimension: usize = std::env::var("CYBERCLAW_EMBED_DIMENSION")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1536);
        Arc::new(cyberclaw_llm::OpenAiCompatEmbedClient::new(
            base_url, api_key, model, dimension,
        ))
    } else {
        Arc::new(cyberclaw_llm::NoopEmbedClient)
    };
    // Sprint D3/D5 — wire a default PersistentLoop so requests with
    // `execution_mode = Some(Persistent)` route into the D1 dispatch
    // branch in `InMemoryExecutionService::execute()` (commit 6447fca)
    // instead of failing loudly with "PersistentLoop not wired". The
    // default plan is empty; per-request callers (e.g. the chat
    // handler's persistent dispatch path) submit their own
    // `ExecutionRequest` and the loop's plan is the one supplied at
    // wiring time. Story-from-messages plan synthesis is a follow-up.
    let default_persistent_loop = Arc::new(
        cyberclaw_control_plane::persistent_execution::PersistentLoop::new(
            cyberclaw_control_plane::persistent_execution::ExecutionPlan::new(
                "chat-default-persistent",
            ),
            cyberclaw_control_plane::persistent_execution::LoopConfig::default(),
        ),
    );

    let execution_service = Arc::new(
        InMemoryExecutionService::with_runtimes_and_recorder(
            agent_runtime.clone(),
            skill_runtime.clone(),
            event_recorder.clone(),
        )
        .with_capability_dispatcher(capability_dispatcher)
        .with_autopilot_step_runner(Arc::new(GovernedAutopilotStepRunner::new(
            security_gate,
            Arc::new(DefaultProgressEvaluator::new()),
            Arc::new(DefaultIterationTracker::default()),
        )))
        .with_llm_client(llm_client.clone())
        .with_embed_client(embed_client_for_svc)
        .with_persistent_loop(default_persistent_loop),
    );
    info!("✓ Execution Service initialized with AgentRuntime + SkillRuntime + LLM + PersistentLoop (Sprint D3 wiring)");

    // 创建 Resolver (需要 Registry)
    use cyberclaw_control_plane::registry::InMemoryRegistry;
    let registry = Arc::new(InMemoryRegistry::new());
    bootstrap_registry_from_ecosystem(registry.clone()).await?;

    // v1.x — re-apply user-installed packages (`~/.cyberclaw/installed-packages.json`)
    // AFTER ecosystem auto-scan so any package the operator explicitly
    // installed via `POST /api/v2/packages` wins over the repo default
    // with the same id. Failures are logged + skipped — never fatal.
    let installed_pkg_store =
        Arc::new(cyberclaw_server::installed_packages::InstalledPackageStore::load_default());
    match cyberclaw_server::api::packages::bootstrap_user_packages(&registry, &installed_pkg_store)
        .await
    {
        Ok(count) if count > 0 => info!("✓ Restored {} user-installed package(s)", count),
        Ok(_) => info!("No user-installed packages to restore"),
        Err(e) => warn!(error = %e, "user-installed packages reload failed; continuing"),
    }

    let resolver = Arc::new(InMemoryResolver::new(registry.clone()));
    info!("✓ Resolver initialized");

    // 创建 ReviewQueue
    let review_queue = Arc::new(InMemoryReviewQueue::new(None));
    info!("✓ Review Queue initialized");

    // 创建其他必需的 Control Plane 组件
    use cyberclaw_control_plane::{
        event_bus::InMemoryEventBus, gateway_router::InMemoryGatewayRouter,
        lease_manager::InMemoryLeaseManager, membership_service::InMemoryMembershipService,
        placement_engine::InMemoryPlacementEngine,
    };
    use cyberclaw_observability::security_event_store::InMemorySecurityEventStore;

    let gateway = Arc::new(InMemoryGatewayRouter::new());
    let placement_engine = Arc::new(InMemoryPlacementEngine);
    let lease_manager = Arc::new(InMemoryLeaseManager::default());
    let membership_service = Arc::new(InMemoryMembershipService::default());
    let event_bus = Arc::new(InMemoryEventBus::default());
    let security_event_store = Arc::new(InMemorySecurityEventStore::new());

    // 创建 PluginRegistry（使用默认插件目录）
    let plugin_dir =
        std::env::var("CYBERCLAW_PLUGIN_DIR").unwrap_or_else(|_| "./plugins".to_string());
    let plugin_registry = Arc::new(PluginRegistry::new(std::path::PathBuf::from(plugin_dir)));
    let local_node_id_raw =
        env::var("CYBERCLAW_NODE_ID").unwrap_or_else(|_| "local-server-node".to_string());
    let local_node_id = NodeId::from_string(local_node_id_raw.clone()).expect("valid node id");

    // 创建 ControlPlaneOrchestrator
    let control_plane = Arc::new(
        ControlPlaneOrchestrator::new(
            gateway.clone(),
            resolver.clone(),
            registry.clone(),
            review_queue.clone(),
            task_manager.clone(),
            execution_service.clone(),
            placement_engine.clone(),
            lease_manager.clone(),
            membership_service.clone(),
            event_bus.clone(),
            policy_engine.clone(),
            plugin_registry.clone(),
            security_event_store.clone(),
        )
        .with_local_node_id(local_node_id.clone())
        .with_event_recorder(event_recorder.clone()),
    );
    info!("✓ Control Plane Orchestrator initialized");

    // ===== 3.1. 注册并激活本地节点（修复 "no active nodes available" 错误）=====
    {
        use chrono::Utc;
        use cyberclaw_control_plane::membership_service::MembershipService;
        use cyberclaw_core::cluster::{
            MembershipState, NodeCapacity, NodeHealth, NodeRecord, NodeRole,
        };
        let node_id = local_node_id.clone();
        let node = NodeRecord {
            id: node_id.clone(),
            role: NodeRole::Hybrid,
            labels: vec!["worker".to_string(), "runtime:native".to_string()],
            region: None,
            zone: None,
            health: NodeHealth::Healthy,
            membership_state: MembershipState::Active,
            capacity: NodeCapacity::default(),
            current_executions: 0,
            last_heartbeat_at: Utc::now(),
        };
        membership_service
            .join(node)
            .expect("failed to join local node");
        membership_service
            .heartbeat(&node_id)
            .expect("failed to heartbeat local node");
        info!("✓ Local server node registered and activated");
    }

    // ===== 3.2. 初始化 CronScheduler 并配置 TaskExecutor =====
    info!("Initializing CronScheduler with Control Plane integration...");
    let task_executor = Arc::new(ControlPlaneTaskExecutor::new(control_plane.clone()));
    let cron_scheduler = Arc::new(
        CronScheduler::new()
            .expect("Failed to create CronScheduler")
            .with_executor(task_executor),
    );
    info!("✓ CronScheduler initialized with Control Plane TaskExecutor");
    let scheduler_for_task = cron_scheduler.clone();
    let scheduler_handle = tokio::spawn(async move {
        if let Err(err) = scheduler_for_task.start().await {
            warn!("CronScheduler stopped unexpectedly: {}", err);
        }
    });
    info!("✓ CronScheduler started in background");

    // ===== 4. 创建应用状态 (集成所有组件) =====
    use cyberclaw_server::ControlPlaneComponents;
    let cp_components = ControlPlaneComponents {
        control_plane,
        task_manager,
        execution_service: execution_service.clone(),
        review_queue,
        package_registry: registry.clone(),
    };

    // Initialize append-only audit sink (Sprint 8 Lane 1). The path
    // defaults to `$HOME/.cyberclaw/audit.db` (SQLite WAL + hash chain)
    // and can be overridden via `CYBERCLAW_AUDIT_DB`.
    let audit_path = cyberclaw_server::audit::AuditSink::default_path();
    let audit_sink = match cyberclaw_server::audit::AuditSink::new(audit_path.clone()).await {
        Ok(sink) => {
            info!("✓ Audit sink initialized at {}", audit_path.display());
            Some(Arc::new(sink))
        }
        Err(err) => {
            warn!(path = %audit_path.display(), %err, "audit sink init failed; continuing without audit");
            None
        }
    };

    let mut app_state = AppState::new(
        llm_client,
        connector_registry,
        policy_engine,
        event_recorder,
        jwt_secret.clone(),
        cp_components,
    );
    if let Some(sink) = audit_sink.clone() {
        app_state = app_state.with_audit_sink(sink);
    }

    // 2026-05-18 (v1.2.15): boot-time OTLP-HTTP/JSON exporter init. Reads
    // `CYBERCLAW_OTLP_ENDPOINT` and friends; returns `None` (no overhead) when
    // unset. When configured, starts a background flush loop on the tokio
    // runtime. The exporter is presently stored as a side-effect singleton —
    // span/metric callers don't reach it automatically yet; v1.3 ships the
    // tracing-subscriber → OtelExporter bridge so every `tracing::info_span!`
    // call exports a span without manual plumbing.
    match cyberclaw_observability::otel_exporter::init_from_env() {
        Ok(Some(exporter)) => {
            info!(
                endpoint = %exporter.config().endpoint,
                service = %exporter.config().service_name,
                "✓ OTLP exporter initialized (HTTP/JSON)"
            );
            exporter.start_background_flush();
            // Leak the Arc so the background flush task keeps a live
            // reference for the rest of the process. Boot-only path; the
            // memory cost is one `OtelExporter`.
            let _ = Box::leak(Box::new(exporter));
        }
        Ok(None) => {
            tracing::debug!(
                "OTLP export disabled (CYBERCLAW_OTLP_ENDPOINT not set) — \
                 set it to e.g. http://localhost:4318 to enable"
            );
        }
        Err(e) => {
            tracing::warn!(error = %e, "OTLP exporter config invalid; proceeding without export");
        }
    }

    // W2 D-1 — attach autonomous skill Curator (HeuristicEvaluator, no LLM).
    // Backs `/api/v1/admin/curator/status` + `/run-now`. Built unconditionally
    // so the admin SPA always sees real data; gated by env var if operators
    // want the spawn_loop background tick to be optional.
    let curator = cyberclaw_server::state::build_default_curator();
    app_state = app_state.with_curator(curator.clone());
    // Default cycle is 7 days (604800 s) from CuratorConfig::for_skills_root.
    const CURATOR_DEFAULT_INTERVAL_SECS: u64 = 7 * 24 * 3600;
    if std::env::var("CYBERCLAW_CURATOR_ENABLED")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes"))
        .unwrap_or(false)
    {
        let _curator_handle = curator.clone().spawn_loop();
        info!(
            "curator scheduler: enabled, interval={}s",
            CURATOR_DEFAULT_INTERVAL_SECS
        );
    } else {
        info!("curator scheduler: disabled (set CYBERCLAW_CURATOR_ENABLED=1 to enable)");
    }

    // Sprint 21 path #3 — open the FTS5 skill discovery index.
    // Best-effort: failures log warn and the server continues without
    // the index (search endpoint will return 503; skill creation still
    // succeeds without indexing).
    let skill_index_path = cyberclaw_server::skill_index::SkillIndex::default_path();
    match cyberclaw_server::skill_index::SkillIndex::new(skill_index_path.clone()).await {
        Ok(idx) => {
            info!(
                "✓ Skill index initialized at {}",
                skill_index_path.display()
            );
            // Bulk-sync installed skills from SkillHub so /api/v1/skills/search
            // surfaces filesystem-installed skills (not only API-created ones).
            // Previously the index only saw skills created via /api/v1/skills/create;
            // /api/v1/skills (107) and /api/v1/skills/search (5) were out of sync
            // and the LLM tool palette couldn't find e.g. the `powerpoint` skill.
            let hub_read = app_state.skill_hub.read().await;
            let installed = hub_read.list_installed();
            let hub_base = hub_read.base_dir().to_path_buf();
            // Also probe ecosystem/skills/ as a fallback path for SKILL.md.
            // Mirrors the order in `to_admin_view_with_base` in skills.rs.
            let ecosystem_dirs: Vec<std::path::PathBuf> = [
                std::env::var_os("CYBERCLAW_ECOSYSTEM_SKILLS").map(std::path::PathBuf::from),
                Some(
                    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("../../ecosystem/skills"),
                ),
                std::env::current_dir()
                    .ok()
                    .map(|p| p.join("ecosystem/skills")),
            ]
            .into_iter()
            .flatten()
            .filter(|p| p.is_dir())
            .collect();
            let total = installed.len();
            let now_ts = chrono::Utc::now().timestamp();
            let mut synced = 0usize;
            let mut enriched = 0usize;
            for b in &installed {
                // Prefer SKILL.md description from frontmatter; fall back to bundle.description.
                let mut description = b.description.clone();
                let candidate_paths: Vec<std::path::PathBuf> =
                    std::iter::once(hub_base.join("installed").join(&b.name).join("SKILL.md"))
                        .chain(
                            ecosystem_dirs
                                .iter()
                                .map(|d| d.join(&b.name).join("SKILL.md")),
                        )
                        .collect();
                for p in &candidate_paths {
                    if p.exists() {
                        let meta = cyberclaw_server::api::skills::parse_skill_md(p);
                        if let Some(desc) = meta.description {
                            if !desc.trim().is_empty() {
                                if description.trim().is_empty() {
                                    description = desc;
                                } else if !description.contains(&desc) {
                                    // Bundle desc + frontmatter desc may differ — prefer frontmatter
                                    description = desc;
                                }
                                enriched += 1;
                                break;
                            }
                        }
                    }
                }
                let entry = cyberclaw_server::skill_index::SkillIndexEntry {
                    skill_id: format!("sk_{}", b.name),
                    name: b.name.clone(),
                    description,
                    category: "Other".to_string(),
                    source_request_id: None,
                    installed_at: now_ts,
                };
                if idx.upsert(entry).await.is_ok() {
                    synced += 1;
                }
            }
            drop(hub_read);
            info!(
                "✓ Skill index enriched {} / {} descriptions from SKILL.md frontmatter",
                enriched, total
            );
            info!(
                "✓ Skill index synced {}/{} installed skills from SkillHub",
                synced, total
            );
            app_state = app_state.with_skill_index(Arc::new(idx));
        }
        Err(err) => {
            warn!(path = %skill_index_path.display(), %err, "skill index init failed; search endpoint disabled");
        }
    }

    // Sprint 19 W1 — register the memory connector backed by the
    // LeveledMemoryStore so the LLM-side `memory_read`/`memory_write`/
    // `memory_search` tool calls actually reach storage. Registered
    // here (production binary) rather than inside `AppState::new` so
    // tests using `build_test_state()` keep their existing fixture
    // semantics (empty registry = ship-list fallback path).
    {
        let memory_connector = cyberclaw_server::memory_connector::MemoryConnector::new(
            app_state.memory_store.clone(),
        );
        app_state
            .connector_registry
            .register(Arc::new(memory_connector))
            .expect("failed to register MemoryConnector");
        tracing::info!(
            "Sprint 19 W1: MemoryConnector registered with capabilities \
             [memory_read, memory_write, memory_search]"
        );
    }

    // Sprint 19 W3 — register the todo connector. Same dead-letter
    // pattern as memory: the facade pointed at connector_id="todo"
    // (was "internal" before this sprint re-pointed it) but no
    // connector with that id was ever registered.
    {
        let todo_connector = cyberclaw_server::todo_connector::TodoConnector::new();
        app_state
            .connector_registry
            .register(Arc::new(todo_connector))
            .expect("failed to register TodoConnector");
        tracing::info!(
            "Sprint 19 W3: TodoConnector registered with capabilities \
             [agent:todo:read, agent:todo:write]"
        );
    }

    // Sprint 20 W1 — connector / facade / mapper drift audit. Logs
    // a WARN line for every mapper alias or facade that targets a
    // connector_id which isn't registered (the recurring anti-pattern
    // that bit memory/todo/MCP/LSP this sprint). Result is also
    // exposed via /api/v1/system/connector-drift for live re-query.
    let drift_report = cyberclaw_server::connector_drift::audit(
        &app_state.connector_registry,
        &app_state.tool_mapper,
    );
    if drift_report.warning_count() > 0 {
        // F12 Phase D (2026-05-06) — drift used to be silently warn-only,
        // which let the F8 / F1-F7 type-of-orphan basic problem persist
        // for months. Now in production (or when CYBERCLAW_STRICT_DRIFT=1
        // is set in any environment) drift becomes a fatal startup error.
        // Dev still logs warn so iterative work isn't blocked.
        let strict = std::env::var("CYBERCLAW_STRICT_DRIFT").as_deref() == Ok("1");
        let prod = std::env::var("ENVIRONMENT").as_deref() == Ok("production");
        if strict || prod {
            error!(
                count = drift_report.warning_count(),
                "Connector/facade/mapper drift detected at startup — refusing to launch \
                 (set CYBERCLAW_STRICT_DRIFT=0 to downgrade to warn for emergency starts)"
            );
            anyhow::bail!(
                "connector drift audit found {} issue(s); see preceding warn lines",
                drift_report.warning_count()
            );
        }
        warn!(
            "connector drift: {} warnings — see startup log for details \
             (will become fatal in production; set CYBERCLAW_STRICT_DRIFT=1 to opt-in here)",
            drift_report.warning_count()
        );
    }

    let state = Arc::new(app_state);

    // Record server startup as an audit entry so the log always has
    // at least one bootstrap marker the operator can verify from the UI.
    if let Some(sink) = audit_sink.as_ref() {
        sink.record(cyberclaw_server::audit::AuditEntry::now(
            "system",
            cyberclaw_server::audit::AuditKind::Config,
            "server.start",
            None,
            serde_json::json!({ "node_id": state.node_id.clone() }),
            cyberclaw_server::audit::AuditResult::Success,
        ))
        .await;
    }

    info!("✓ Application State assembled");

    // Cron scheduler — ticks every 30 s and fires overdue jobs.
    let _cron_scheduler_handle =
        cyberclaw_server::cron::spawn_cron_scheduler(state.cron_store.clone());
    info!("✓ Cron scheduler started in background");

    // S20 E3: Spawn background memory cleanup job (expire_stale on LeveledMemoryStore)
    cyberclaw_server::memory_cleanup::spawn_memory_cleanup_from_env(state.memory_store.clone());
    info!("✓ Memory cleanup job started");

    // v1.3 WP-1 — Session eviction background task.
    // Sweeps idle conversations every 60 s; threshold controlled by
    // CYBERCLAW_SESSION_IDLE_TIMEOUT_SECS (default 1800 = 30 min).
    {
        let session_store = state.conversation_session_store.clone();
        tokio::spawn(async move {
            let idle_secs = std::env::var("CYBERCLAW_SESSION_IDLE_TIMEOUT_SECS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(1800);
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                ticker.tick().await;
                let evicted = session_store
                    .evict_idle(std::time::Duration::from_secs(idle_secs))
                    .await;
                if evicted > 0 {
                    tracing::info!(evicted, "session eviction sweep: removed idle sessions");
                }
            }
        });
        info!("✓ Session eviction task started (idle_timeout=CYBERCLAW_SESSION_IDLE_TIMEOUT_SECS)");
    }

    // Sprint 20 W1 — RB-11 audit archive task. Hourly VACUUM INTO +
    // verify_chain + optional GPG sign + 30d retention sweep. Disable
    // by setting CYBERCLAW_AUDIT_ARCHIVE_INTERVAL_SECS=0 in production
    // K8s deployments that run the CronJob template instead (the
    // CronJob path survives a server crash).
    if let Some(sink) = state.audit.clone() {
        let cfg_dir = audit_path
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let cfg = cyberclaw_server::audit_archive::AuditArchiveConfig::from_env(&cfg_dir);
        if cfg.enabled() {
            cyberclaw_server::audit_archive::spawn(sink, cfg);
            info!("✓ Audit archive task started");
        } else {
            info!("Audit archive disabled (CYBERCLAW_AUDIT_ARCHIVE_INTERVAL_SECS=0)");
        }
    }

    // D-4 (2026-05-05) — Capability Feedback Loop. Default-disabled
    // background scanner that folds historical failed `capability.complete`
    // rows back into `capability_requests`. Enable via
    // CYBERCLAW_FEEDBACK_LOOP_ENABLED=1.
    if let Some(sink) = state.audit.clone() {
        let fb_cfg = cyberclaw_server::feedback_loop::FeedbackLoopConfig::from_env();
        cyberclaw_server::feedback_loop::spawn_feedback_loop(sink, fb_cfg);
    }

    let mut assignment_event_collector_handle = spawn_assignment_event_collector(
        event_bus.clone(),
        execution_service.clone(),
        state.assignment_queue.clone(),
        local_node_id.clone(),
    );
    let mut assignment_pull_worker_handle = load_pull_worker_config(&local_node_id).map(|config| {
        info!(
            coordinator_url = %config.coordinator_url,
            poll_interval_ms = config.poll_interval.as_millis(),
            "✓ Assignment pull worker initialized"
        );
        spawn_assignment_pull_worker(execution_service.clone(), config)
    });

    // ===== 5. 创建路由（从环境变量读取配置）=====
    let server_config = ServerConfig::from_env();
    info!(
        "✓ Server Config: rate_limit={}/s burst={} max_body={}KB",
        server_config.rate_limit_per_second,
        server_config.rate_limit_burst_size,
        server_config.max_body_size / 1024
    );
    let app = create_router_with_config(state, server_config.clone());

    // ===== 6. 启动服务器 =====
    // 使用 ServerConfig 中的地址配置（从 CYBERCLAW_ADDR 环境变量读取）
    let addr = server_config.addr;

    // TLS 配置（可选，根据环境变量）
    let use_tls = env::var("USE_TLS")
        .unwrap_or_else(|_| "false".to_string())
        .parse::<bool>()
        .unwrap_or(false);

    // 检查是否在生产环境
    let is_production = env::var("ENVIRONMENT")
        .unwrap_or_else(|_| "development".to_string())
        .to_lowercase()
        == "production";

    if is_production && !use_tls {
        return Err(anyhow::anyhow!(
            "CRITICAL SECURITY ERROR: TLS must be enabled in production environment.\n\
             \n\
             Set the following environment variables:\n\
             export USE_TLS=true\n\
             export TLS_CERT_PATH=/path/to/cert.pem\n\
             export TLS_KEY_PATH=/path/to/key.pem\n\
             \n\
             For development/testing only:\n\
             export ENVIRONMENT=development\n\
             \n\
             To generate self-signed certificates for testing:\n\
             openssl req -x509 -newkey rsa:4096 -nodes -keyout key.pem -out cert.pem -days 365"
        ));
    }

    let server_result: Result<()> = if use_tls {
        // 2026-05-18 (v1.2.11): friendly error vs panic for TLS path env vars.
        // Production deployments setting USE_TLS=true without cert paths used
        // to crash with a Rust panic stack; now they get a clear pointer to
        // the missing env var.
        let cert_path = env::var("TLS_CERT_PATH").map_err(|_| {
            anyhow::anyhow!(
            "USE_TLS=true requires TLS_CERT_PATH to point at your PEM-format TLS certificate file."
        )
        })?;
        let key_path = env::var("TLS_KEY_PATH").map_err(|_| {
            anyhow::anyhow!(
            "USE_TLS=true requires TLS_KEY_PATH to point at your PEM-format TLS private key file."
        )
        })?;

        use axum_server::tls_rustls::RustlsConfig;
        let tls_config = RustlsConfig::from_pem_file(cert_path, key_path).await?;

        info!("Starting HTTPS server on {}", addr);
        info!("   - Health check: https://{}/health", addr);
        info!("   - Chat API: https://{}/v1/chat/completions", addr);

        let handle = axum_server::Handle::new();
        let shutdown_handle = handle.clone();

        tokio::spawn(async move {
            shutdown_signal().await;
            info!("Received shutdown signal, initiating graceful shutdown...");
            shutdown_handle.graceful_shutdown(Some(std::time::Duration::from_secs(30)));
        });

        axum_server::bind_rustls(addr, tls_config)
            .handle(handle)
            .serve(app.into_make_service())
            .await
            .map_err(anyhow::Error::from)
    } else {
        if is_production {
            unreachable!("Should have returned Err above");
        }
        eprintln!("\n╔══════════════════════════════════════════════════════════════════╗");
        eprintln!("║  WARNING: RUNNING IN INSECURE HTTP MODE                         ║");
        eprintln!("║  This is ONLY acceptable for local development and testing.     ║");
        eprintln!("║  NEVER use this configuration in production!                    ║");
        eprintln!("╚══════════════════════════════════════════════════════════════════╝\n");
        warn!("TLS disabled - running insecure HTTP server");

        // 先绑定TCP监听器，确保端口可用
        let listener = tokio::net::TcpListener::bind(addr).await?;

        // 绑定成功后才打印日志
        info!("Server listening on {}", addr);
        info!("   - Health check: http://{}/health", addr);
        info!("   - Chat API: http://{}/v1/chat/completions", addr);

        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .map_err(anyhow::Error::from)
    };

    info!("Stopping CronScheduler...");
    cron_scheduler.stop().await;
    scheduler_handle.abort();
    if let Err(err) = scheduler_handle.await {
        if !err.is_cancelled() {
            warn!("CronScheduler task join failed: {}", err);
        }
    }

    if let Some(runtime) = consensus_runtime.as_mut() {
        info!("Stopping Raft consensus runtime...");
        runtime.shutdown().await;
    }

    if let Some(handle) = assignment_event_collector_handle.take() {
        handle.abort();
        if let Err(err) = handle.await {
            if !err.is_cancelled() {
                warn!("Assignment event collector task join failed: {}", err);
            }
        }
    }

    if let Some(handle) = assignment_pull_worker_handle.take() {
        handle.abort();
        if let Err(err) = handle.await {
            if !err.is_cancelled() {
                warn!("Assignment pull worker task join failed: {}", err);
            }
        }
    }

    server_result?;

    info!("Server shutdown complete.");
    Ok(())
}

/// 等待关闭信号（SIGINT 或 SIGTERM）
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            info!("Received SIGINT (Ctrl+C)");
        },
        _ = terminate => {
            info!("Received SIGTERM");
        },
    }
}

/// 创建 LLM 客户端
fn create_llm_client() -> Result<Arc<dyn LlmClient>> {
    // 从环境变量读取 LLM 配置
    let provider = env::var("LLM_PROVIDER").unwrap_or_else(|_| "openai".to_string());
    // 2026-05-18 (v1.2.10): replaced expect() with friendly anyhow::bail! so
    // the boot failure is a clean error instead of a Rust panic stack trace.
    // Operators were getting "thread 'main' panicked at LLM_API_KEY must be
    // set" with no actionable guidance.
    let api_key = env::var("LLM_API_KEY").map_err(|_| {
        anyhow::anyhow!(
            "LLM_API_KEY environment variable is not set.\n\
             \n\
             cyberclaw-server requires an LLM API key to start. Set one of:\n\
             \n\
             - Source ~/.cyberclaw/llm.env via ~/.cyberclaw/bin/start-cyberclaw.sh\n\
               (recommended — the start script sources llm.env and runs the server)\n\
             - Or export LLM_API_KEY directly:\n\
               export LLM_API_KEY=<your-key>\n\
               export LLM_PROVIDER=generic   # or openai/anthropic/ark\n\
               export LLM_BASE_URL=https://api.deepseek.com/v1   # for generic\n\
             \n\
             See docs/configuration/llm-providers.md for per-vendor templates."
        )
    })?;
    let base_url = env::var("LLM_BASE_URL").ok();

    info!("Creating LLM client: provider={}", provider);

    let client: Arc<dyn LlmClient> = match provider.as_str() {
        "openai" => {
            let base = base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string());
            Arc::new(OpenAiClient::new(api_key, &base)?)
        }
        "ark" => {
            let base =
                base_url.unwrap_or_else(|| "https://ark.cn-beijing.volces.com/api/v3".to_string());
            Arc::new(ArkClient::new(api_key, &base)?)
        }
        "anthropic" => {
            let base = base_url.unwrap_or_else(|| "https://api.anthropic.com/v1".to_string());
            Arc::new(AnthropicClient::new(api_key, &base)?)
        }
        "generic" => {
            // 2026-05-18 (v1.2.10): friendly error vs panic — same rationale as
            // LLM_API_KEY above.
            let base = base_url.ok_or_else(|| anyhow::anyhow!(
                "LLM_PROVIDER=generic requires LLM_BASE_URL (e.g. https://api.deepseek.com/v1 for DeepSeek). Set it in ~/.cyberclaw/llm.env."
            ))?;
            Arc::new(GenericOpenAiClient::new(Some(api_key), &base)?)
        }
        _ => {
            warn!("Unknown provider '{}', using OpenAI", provider);
            let base = base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string());
            Arc::new(OpenAiClient::new(api_key, &base)?)
        }
    };

    Ok(client)
}

fn parse_bool_env(key: &str) -> bool {
    matches!(
        env::var(key).unwrap_or_default().to_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn parse_http_headers(raw: &str) -> std::collections::HashMap<String, String> {
    raw.split(';')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            let key = k.trim();
            let value = v.trim();
            if key.is_empty() || value.is_empty() {
                return None;
            }
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

async fn maybe_register_mcp_connector(registry: &Arc<ConnectorRegistry>) -> Result<()> {
    let explicit_enabled = parse_bool_env("CYBERCLAW_MCP_ENABLED");
    let has_http_url = env::var("CYBERCLAW_MCP_HTTP_URL").is_ok();
    let has_stdio_cmd = env::var("CYBERCLAW_MCP_STDIO_COMMAND").is_ok();
    let should_enable = explicit_enabled || has_http_url || has_stdio_cmd;

    if !should_enable {
        info!("MCP connector not configured, skipping");
        return Ok(());
    }

    let name = env::var("CYBERCLAW_MCP_NAME").unwrap_or_else(|_| "default".to_string());
    let transport_mode = env::var("CYBERCLAW_MCP_TRANSPORT").unwrap_or_else(|_| {
        if has_http_url {
            "http".to_string()
        } else {
            "stdio".to_string()
        }
    });

    let transport = match transport_mode.as_str() {
        "http" => {
            let url = env::var("CYBERCLAW_MCP_HTTP_URL").map_err(|_| {
                anyhow::anyhow!(
                    "CYBERCLAW_MCP_HTTP_URL is required when CYBERCLAW_MCP_TRANSPORT=http"
                )
            })?;
            let headers = env::var("CYBERCLAW_MCP_HTTP_HEADERS")
                .map(|s| parse_http_headers(&s))
                .unwrap_or_default();
            TransportConfig::Http { url, headers }
        }
        "stdio" => {
            let command = env::var("CYBERCLAW_MCP_STDIO_COMMAND").map_err(|_| {
                anyhow::anyhow!(
                    "CYBERCLAW_MCP_STDIO_COMMAND is required when CYBERCLAW_MCP_TRANSPORT=stdio"
                )
            })?;
            let args = env::var("CYBERCLAW_MCP_STDIO_ARGS")
                .map(|s| s.split_whitespace().map(|v| v.to_string()).collect())
                .unwrap_or_else(|_| Vec::new());
            let workdir = env::var("CYBERCLAW_MCP_STDIO_WORKDIR").ok();
            TransportConfig::Stdio {
                command,
                args,
                workdir,
            }
        }
        other => {
            return Err(anyhow::anyhow!(
                "Unsupported CYBERCLAW_MCP_TRANSPORT: {} (expected http|stdio)",
                other
            ));
        }
    };

    let config = McpServerConfig {
        name: name.clone(),
        transport,
        timeout: std::time::Duration::from_secs(
            env::var("CYBERCLAW_MCP_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(30),
        ),
        enable_cache: !matches!(
            env::var("CYBERCLAW_MCP_ENABLE_CACHE")
                .unwrap_or_else(|_| "true".to_string())
                .to_lowercase()
                .as_str(),
            "0" | "false" | "no" | "off"
        ),
    };

    let connector = Arc::new(McpConnector::new(config).await?);
    registry.register(connector)?;

    info!("✓ MCP Connector registered as mcp-{}", name);
    Ok(())
}

/// Opt-in Browser CDP connector registration.
///
/// Activates when `CYBERCLAW_BROWSER_ENABLED=true`. The connector attaches
/// to an existing Chrome/Chromium DevTools endpoint:
/// - `CYBERCLAW_BROWSER_WS_URL`: direct page WebSocket URL
/// - `CYBERCLAW_BROWSER_DEBUG_URL`: DevTools HTTP endpoint, default `http://127.0.0.1:9222`
///
/// It does not spawn or install Chromium. Runtime ownership remains with the
/// operator or deployment profile, while CyberClaw keeps the capability under
/// the normal Connector -> Capability governance chain.
fn maybe_register_browser_connector(
    registry: &Arc<ConnectorRegistry>,
    workspace: std::path::PathBuf,
) -> Result<()> {
    if !parse_bool_env("CYBERCLAW_BROWSER_ENABLED") {
        info!(
            "Browser CDP connector not enabled, skipping (set CYBERCLAW_BROWSER_ENABLED=true to wire)"
        );
        return Ok(());
    }

    let connector = cyberclaw_connectors::BrowserConnector::from_env(workspace)?;
    registry.register(Arc::new(connector))?;
    info!("✓ Browser CDP Connector registered as 'browser'");

    // 2026-05-18 (v1.2.15): async background CDP probe. Was sync in v1.2.13
    // and added up to 500ms to boot whenever obscura wasn't running yet —
    // common during dev because operators start the server before the browser.
    // Now we spawn_blocking the connect_timeout call so boot returns
    // immediately; the probe still logs ✓/warn a moment later. Connector stays
    // registered either way, so calls work once obscura comes up later (no
    // restart needed).
    let debug_url = std::env::var("CYBERCLAW_BROWSER_DEBUG_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:9222".to_string());
    if let Some((host, port)) = parse_debug_url(&debug_url) {
        tokio::task::spawn_blocking(move || {
            let addr = format!("{host}:{port}");
            let reachable = std::net::TcpStream::connect_timeout(
                &addr
                    .parse()
                    .ok()
                    .or_else(|| {
                        // host might be a name not an IP; try DNS resolve via to_socket_addrs
                        use std::net::ToSocketAddrs;
                        addr.to_socket_addrs().ok().and_then(|mut a| a.next())
                    })
                    .unwrap_or_else(|| "127.0.0.1:0".parse().unwrap()),
                std::time::Duration::from_millis(500),
            )
            .is_ok();
            if reachable {
                info!("✓ Browser CDP probe: {} is reachable", debug_url);
            } else {
                tracing::warn!(
                    "Browser CDP endpoint {} not reachable at boot. The connector \
                     is still registered — browser.* tool calls will fail until \
                     obscura (or another CDP server) is started. Try: \
                     `obscura serve --port 9222 --stealth`",
                    debug_url
                );
            }
        });
    }
    Ok(())
}

/// Parse `http://host:port[/...]` into (host, port). Returns None if parse fails.
fn parse_debug_url(url: &str) -> Option<(String, u16)> {
    let trimmed = url
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let authority = trimmed.split('/').next()?;
    let (host, port_str) = authority.rsplit_once(':')?;
    let port = port_str.parse::<u16>().ok()?;
    Some((host.to_string(), port))
}

/// Sprint 20 W1 — opt-in LSP connector registration.
///
/// Activates when `CYBERCLAW_LSP_ENABLED=true` AND a server binary is
/// reachable on PATH. Registered with id="lsp" to match the LLM facade
/// `connector_id` declared in `cyberclaw_agent_runtime::builtin_tools`,
/// so dispatch of `lsp.hover` / `lsp.diagnostics` / `lsp.definition` /
/// `lsp.references` actually reaches the connector.
///
/// Configuration env vars:
///   - `CYBERCLAW_LSP_ENABLED`     = `true|false` (default false)
///   - `CYBERCLAW_LSP_COMMAND`     = `rust-analyzer` (default)
///   - `CYBERCLAW_LSP_ARGS`        = whitespace-separated args (default empty)
///   - `CYBERCLAW_LSP_WORKSPACE`   = workspace root (default `CYBERCLAW_WORKSPACE` or `cwd`)
///
/// When enabled but the binary cannot be detected, logs a warning and
/// skips registration — the lsp.* tools then fall through to dispatch
/// "Connector lsp not registered" (matches the MCP-not-configured
/// pattern). Falling back rather than failing startup keeps the rest of
/// the platform usable when an operator forgets the binary install.
async fn maybe_register_lsp_connector(registry: &Arc<ConnectorRegistry>) -> Result<()> {
    let enabled = parse_bool_env("CYBERCLAW_LSP_ENABLED");
    if !enabled {
        info!("LSP connector not enabled, skipping (set CYBERCLAW_LSP_ENABLED=true to wire)");
        return Ok(());
    }

    let command = env::var("CYBERCLAW_LSP_COMMAND").unwrap_or_else(|_| "rust-analyzer".to_string());
    let args: Vec<String> = env::var("CYBERCLAW_LSP_ARGS")
        .map(|s| s.split_whitespace().map(String::from).collect())
        .unwrap_or_default();
    let workspace = env::var("CYBERCLAW_LSP_WORKSPACE")
        .or_else(|_| env::var("CYBERCLAW_WORKSPACE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        });

    // Probe: does `command --version` succeed? Skip registration on failure.
    match tokio::process::Command::new(&command)
        .arg("--version")
        .output()
        .await
    {
        Ok(out) if out.status.success() => {
            info!(
                "Sprint 20 W1: LSP server '{}' detected, registering with workspace={}",
                command,
                workspace.display()
            );
        }
        _ => {
            warn!(
                "CYBERCLAW_LSP_ENABLED=true but '{}' is not callable on PATH — skipping LSP connector registration",
                command
            );
            return Ok(());
        }
    }

    let config = cyberclaw_connectors::local::lsp::LspConnectorConfig::generic(
        command.clone(),
        args.clone(),
    );
    let connector = cyberclaw_connectors::local::lsp::LspConnector::new("lsp", config)
        .map_err(|e| anyhow::anyhow!("failed to construct LspConnector: {}", e))?;
    registry.register(Arc::new(connector))?;
    info!(
        "✓ LSP Connector registered as 'lsp' (command={}, args={:?})",
        command, args
    );
    Ok(())
}

/// 注册 IM 语音远控链路所需 Connector（IM Channel / Voice Processing / ACP Runtime）。
///
/// 配置来源（可选）：
/// - Telegram: `CYBERCLAW_TELEGRAM_BOT_TOKEN`
/// - Lark: `CYBERCLAW_LARK_APP_ID` + `CYBERCLAW_LARK_APP_SECRET` + `CYBERCLAW_LARK_VERIFICATION_TOKEN`
///   - 可选 `CYBERCLAW_LARK_ENCRYPT_KEY`
///   - 可选 `CYBERCLAW_LARK_DOMAIN` (`feishu`/`lark`)
///   - 可选 `CYBERCLAW_LARK_TRANSPORT` (`webhook`/`websocket`)
/// - WeChat iLink Bot: `CYBERCLAW_WECHAT_BOT_TOKEN`
///   - 可选 `CYBERCLAW_WECHAT_BASE_URL`
///   - 可选 `CYBERCLAW_WECHAT_CHANNEL_VERSION`
async fn maybe_register_im_voice_remote_connectors(
    registry: &Arc<ConnectorRegistry>,
) -> Result<()> {
    // 1) 外部 Agent 运行时控制面（Claude Code / Codex / Gemini CLI 等）
    registry.register(Arc::new(AcpRuntimeConnector::with_defaults()))?;
    info!("✓ ACP Runtime Connector registered");

    // 2) 语音处理（当前使用 mock STT/TTS，便于端到端链路先跑通）
    registry.register(Arc::new(VoiceProcessingConnector::with_mocks()))?;
    info!("✓ Voice Processing Connector registered (mock STT/TTS)");

    // 3) IM 渠道（根据环境变量按需挂载具体平台适配器）
    let im_connector = Arc::new(ImChannelConnector::default());
    let mut platforms = Vec::new();

    if let Ok(bot_token) = env::var("CYBERCLAW_TELEGRAM_BOT_TOKEN") {
        let telegram_adapter = TelegramAdapter::new(TelegramConfig::new(bot_token))?;
        im_connector
            .register_adapter(Arc::new(telegram_adapter))
            .await;
        platforms.push("telegram");
    }

    if let (Ok(app_id), Ok(app_secret), Ok(verification_token)) = (
        env::var("CYBERCLAW_LARK_APP_ID"),
        env::var("CYBERCLAW_LARK_APP_SECRET"),
        env::var("CYBERCLAW_LARK_VERIFICATION_TOKEN"),
    ) {
        let encrypt_key = env::var("CYBERCLAW_LARK_ENCRYPT_KEY").ok();
        let domain = match env::var("CYBERCLAW_LARK_DOMAIN")
            .unwrap_or_else(|_| "feishu".to_string())
            .to_lowercase()
            .as_str()
        {
            "lark" => LarkDomain::Lark,
            _ => LarkDomain::Feishu,
        };
        let transport = match env::var("CYBERCLAW_LARK_TRANSPORT")
            .unwrap_or_else(|_| "webhook".to_string())
            .to_lowercase()
            .as_str()
        {
            "websocket" | "ws" => LarkTransportMode::WebSocket,
            _ => LarkTransportMode::Webhook,
        };

        let lark_adapter = LarkAdapter::new(LarkConfig {
            app_id,
            app_secret,
            verification_token,
            encrypt_key,
            domain,
            transport,
        });
        im_connector.register_adapter(Arc::new(lark_adapter)).await;
        platforms.push("lark");
    }

    if let Ok(bot_token) = env::var("CYBERCLAW_WECHAT_BOT_TOKEN") {
        let mut config = WechatConfig::new(bot_token);
        if let Ok(base_url) = env::var("CYBERCLAW_WECHAT_BASE_URL") {
            config.base_url = base_url;
        }
        if let Ok(channel_version) = env::var("CYBERCLAW_WECHAT_CHANNEL_VERSION") {
            config.channel_version = channel_version;
        }
        let wechat_adapter = WechatAdapter::new(config);
        im_connector
            .register_adapter(Arc::new(wechat_adapter))
            .await;
        platforms.push("wechat");
    }

    registry.register(im_connector)?;
    if platforms.is_empty() {
        info!("✓ IM Channel Connector registered (no platform adapter configured yet)");
    } else {
        info!(platforms = ?platforms, "✓ IM Channel Connector registered with adapters");
    }

    Ok(())
}

/// Passthrough security gate: approves all execution results (suitable for dev/single-node).
struct PassthroughSecurityGate;

#[async_trait::async_trait]
impl cyberclaw_control_plane::autopilot_runtime::SecurityGate for PassthroughSecurityGate {
    async fn check_execution_results(
        &self,
        _results: &[cyberclaw_core::autopilot::ExecutionResult],
    ) -> anyhow::Result<cyberclaw_control_plane::autopilot_runtime::SecurityCheckResult> {
        Ok(
            cyberclaw_control_plane::autopilot_runtime::SecurityCheckResult {
                passed: true,
                issues: vec![],
                requires_review: false,
            },
        )
    }
}

/// Default in-memory iteration tracker backed by an atomic counter per execution.
#[derive(Default)]
struct DefaultIterationTracker {
    counters: std::sync::Arc<
        std::sync::RwLock<std::collections::HashMap<cyberclaw_core::prelude::ExecutionId, u32>>,
    >,
}

#[async_trait::async_trait]
impl cyberclaw_control_plane::execution_service::IterationTracker for DefaultIterationTracker {
    async fn current_iteration(
        &self,
        execution_id: &cyberclaw_core::prelude::ExecutionId,
    ) -> anyhow::Result<u32> {
        Ok(self
            .counters
            .read()
            .unwrap_or_else(|e| {
                tracing::error!("RwLock poisoned in iteration tracker (read): {}", e);
                e.into_inner()
            })
            .get(execution_id)
            .copied()
            .unwrap_or(0))
    }

    async fn increment(
        &self,
        execution_id: &cyberclaw_core::prelude::ExecutionId,
    ) -> anyhow::Result<u32> {
        let mut map = self.counters.write().unwrap_or_else(|e| {
            tracing::error!("RwLock poisoned in iteration tracker (write): {}", e);
            e.into_inner()
        });
        let entry = map.entry(execution_id.clone()).or_insert(0);
        *entry += 1;
        Ok(*entry)
    }

    async fn reset(
        &self,
        execution_id: &cyberclaw_core::prelude::ExecutionId,
    ) -> anyhow::Result<()> {
        self.counters
            .write()
            .unwrap_or_else(|e| {
                tracing::error!("RwLock poisoned in iteration tracker (reset): {}", e);
                e.into_inner()
            })
            .insert(execution_id.clone(), 0);
        Ok(())
    }
}

/// ControlPlaneTaskExecutor - Adapter 将 Scheduler 的 TaskAction 转换为 Control Plane 的 IngressRequest
///
/// 这个 adapter 实现了 Dependency Inversion Principle (DIP):
/// - Scheduler 依赖 TaskExecutor trait（抽象）
/// - ControlPlaneTaskExecutor 实现该 trait（具体实现）
/// - 两者通过抽象解耦，避免循环依赖
struct ControlPlaneTaskExecutor {
    orchestrator: Arc<ControlPlaneOrchestrator>,
}

impl ControlPlaneTaskExecutor {
    fn new(orchestrator: Arc<ControlPlaneOrchestrator>) -> Self {
        Self { orchestrator }
    }
}

#[async_trait::async_trait]
impl TaskExecutor for ControlPlaneTaskExecutor {
    async fn execute_task(&self, action: &TaskAction) -> ExecutionResult {
        // 将 TaskAction 转换为 IngressRequest
        let (task_title, task_summary, task_input) = match action {
            TaskAction::ExecuteCapability {
                connector_id,
                capability,
                params,
            } => {
                let title = format!("Execute capability: {}", capability);
                let summary = format!(
                    "Scheduled task to execute capability '{}' via connector '{}'",
                    capability, connector_id
                );
                let input = TaskInput {
                    payload: serde_json::json!({
                        "connector_id": connector_id,
                        "capability": capability,
                        "params": params,
                    }),
                };
                (title, summary, input)
            }
            TaskAction::ExecuteAgent { agent_id, task } => {
                let title = format!("Execute agent: {}", agent_id);
                let summary = format!("Scheduled task for agent '{}': {}", agent_id, task);
                let input = TaskInput {
                    payload: serde_json::json!({
                        "agent_id": agent_id,
                        "task": task,
                    }),
                };
                (title, summary, input)
            }
        };

        // 创建 Task
        let task = Task {
            id: TaskId::new(),
            case_id: None,
            title: task_title,
            summary: task_summary,
            kind: TaskKind::Execution,
            priority: Priority::Medium,
            requested_by: ActorRef {
                id: ActorId::new(),
                actor_type: ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "Scheduler".to_string(),
            },
            requested_at: chrono::Utc::now(),
            trigger: TriggerRef {
                kind: "scheduled".to_string(),
                source: "cron-scheduler".to_string(),
            },
            input: task_input,
            desired_outputs: vec![],
            labels: vec!["scheduled".to_string()],
            preferred_agent_id: None,
        };

        // 创建 IngressRequest
        let ingress_request = IngressRequest {
            actor: ActorRef {
                id: ActorId::new(),
                actor_type: ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "Scheduler".to_string(),
            },
            session: None,
            workspace: None,
            task,
        };

        // 调用 Control Plane 处理
        match self.orchestrator.process_ingress(ingress_request).await {
            Ok(result) => {
                tracing::info!(
                    "Scheduled task execution submitted: execution_id={:?}",
                    result.execution_id
                );
                ExecutionResult::Success {
                    output: serde_json::json!({
                        "execution_id": result.execution_id.to_string(),
                        "status": "submitted",
                    }),
                }
            }
            Err(err) => {
                tracing::error!("Failed to submit scheduled task: {}", err);
                ExecutionResult::Failure {
                    error: format!("Control Plane execution failed: {}", err),
                }
            }
        }
    }
}
