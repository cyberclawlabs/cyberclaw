//! CLI 应用状态管理
//!
//! v1.x — the CLI is now a thin client of `cyberclaw-server`. Package
//! management, status, and registry queries all flow through HTTP
//! (`/api/v2/packages`, `/api/v2/status`, `/api/v1/{skills,connectors,...}`).
//! Only a few legacy lanes (chat-tui llm client, local Connector registry
//! used by debugging connectors directly) still own in-process objects.
//!
//! The old in-process `InMemoryRegistry` field was removed in v1.x — it
//! produced "Installed package: ..." then evaporated on process exit, and
//! `package list` always returned "No packages installed" because every
//! invocation started with an empty registry. See the package management
//! commands in `commands/package.rs` for the HTTP-driven replacement.

use std::sync::Arc;

use cyberclaw_connectors::dispatcher::CapabilityDispatcher;
use cyberclaw_connectors::registry::ConnectorRegistry;
use cyberclaw_control_plane::task_manager::{InMemoryTaskManager, TaskManager};
use cyberclaw_governance::engine::DefaultPolicyEngine;
use cyberclaw_llm::prelude::GenericOpenAiClient;
use cyberclaw_llm::LlmClient;
use cyberclaw_llm_bridge::executor::ToolExecutor;
use cyberclaw_llm_bridge::mapper::ToolCallMapper;
use cyberclaw_observability::events::InMemoryEventRecorder;

/// CLI 全局状态
///
/// Retains the LLM client + a process-local Connector registry for direct
/// dispatch lanes (chat REPL, capability inspection); all
/// 5-ecosystem-object queries are HTTP-driven.
#[derive(Clone)]
#[allow(dead_code)] // 部分字段保留供未来功能使用
pub struct CliState {
    // ===== 核心平台层 =====
    /// Task 管理器 (legacy `task` subcommand — in-process only;
    /// scheduled to migrate to server's `/api/v1/tasks` in a follow-up).
    pub task_manager: Arc<dyn TaskManager>,

    /// Connector 注册表 (底层能力)
    pub connector_registry: Arc<ConnectorRegistry>,

    /// Capability 调度器
    pub capability_dispatcher: Arc<CapabilityDispatcher>,

    /// 策略引擎 (治理与审批)
    pub policy_engine: Arc<DefaultPolicyEngine>,

    // ===== LLM-Bridge 工具执行层 =====
    /// Tool Call 映射器
    pub tool_mapper: Arc<ToolCallMapper>,

    /// Tool 执行器
    pub tool_executor: Arc<ToolExecutor>,

    // ===== 外部接口层 =====
    /// LLM 客户端 (用于直接 LLM 对话)
    pub llm_client: Arc<dyn LlmClient>,

    // ===== 可观测性层 =====
    /// 事件记录器
    pub event_recorder: Arc<InMemoryEventRecorder>,
}

impl CliState {
    /// 创建新的 CLI 状态（默认配置）
    pub fn new() -> anyhow::Result<Self> {
        Self::with_config(None, None)
    }

    /// 创建带自定义配置的 CLI 状态
    ///
    /// # Arguments
    ///
    /// * `llm_api_key` - LLM API 密钥 (默认从环境变量读取)
    /// * `llm_base_url` - LLM Base URL (默认: http://localhost:8080)
    pub fn with_config(
        llm_api_key: Option<String>,
        llm_base_url: Option<String>,
    ) -> anyhow::Result<Self> {
        // 创建 Task 管理器
        let task_manager: Arc<dyn TaskManager> = Arc::new(InMemoryTaskManager::new());

        // 创建 Connector 注册表
        let connector_registry = Arc::new(ConnectorRegistry::new());

        // 创建 Capability 调度器
        let capability_dispatcher = Arc::new(CapabilityDispatcher::new(connector_registry.clone()));

        // 创建策略引擎
        let policy_engine = Arc::new(DefaultPolicyEngine::default());

        // 创建 Tool Call 映射器
        let tool_mapper = Arc::new(ToolCallMapper::new());

        // 创建 Tool 执行器
        let tool_executor = Arc::new(ToolExecutor::new(
            capability_dispatcher.clone(),
            tool_mapper.clone(),
        ));

        // 创建 LLM 客户端
        let base_url = llm_base_url.as_deref().unwrap_or("http://localhost:8080");
        let llm_client: Arc<dyn LlmClient> =
            Arc::new(GenericOpenAiClient::new(llm_api_key, base_url)?);

        // 创建事件记录器
        let event_recorder = Arc::new(InMemoryEventRecorder::new());

        Ok(Self {
            task_manager,
            connector_registry,
            capability_dispatcher,
            policy_engine,
            tool_mapper,
            tool_executor,
            llm_client,
            event_recorder,
        })
    }
}

impl Default for CliState {
    fn default() -> Self {
        Self::new().expect("failed to create default CLI state")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_state_creation() {
        let state = CliState::new();
        assert!(state.is_ok(), "CLI state should be created successfully");
    }

    #[test]
    fn test_cli_state_with_custom_config() {
        let state = CliState::with_config(None, Some("http://localhost:9000".to_string()));
        assert!(
            state.is_ok(),
            "CLI state with custom config should be created successfully"
        );
    }
}
