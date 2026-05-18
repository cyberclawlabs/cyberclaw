//! # ACP External Agent Runtime Connector
//!
//! 外部编程 Agent 运行时连接器，通过 ACP (Agent Client Protocol) 管理外部编码 Agent
//! (Claude Code, Codex, Gemini CLI, OpenCode) 的生命周期。
//!
//! CyberClaw 作为控制面，ACP 提供运行时传输层。
//!
//! ## 支持的 Capabilities
//!
//! - `external_agent.session.spawn`: 创建外部 Agent 会话
//! - `external_agent.session.resume`: 恢复暂停的会话
//! - `external_agent.session.stop`: 停止活跃会话
//! - `external_agent.session.status`: 获取会话状态
//! - `external_agent.prompt.send`: 发送初始任务提示
//! - `external_agent.prompt.followup`: 发送后续指令
//! - `external_agent.prompt.interrupt`: 中断当前操作
//! - `external_agent.artifact.collect`: 收集会话产物
//! - `external_agent.env.set`: 设置工作目录和模型
//! - `external_agent.permission.set`: 变更权限配置

use crate::types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, Connector, ExecutionStatus,
};
use cyberclaw_core::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// Core Types
// ---------------------------------------------------------------------------

/// 支持的外部编码 Agent 运行时
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ExternalRuntime {
    /// Anthropic Claude Code
    ClaudeCode,
    /// OpenAI Codex
    Codex,
    /// Google Gemini CLI
    GeminiCli,
    /// OpenCode
    OpenCode,
    /// 自定义运行时
    Custom(String),
}

/// 传输后端类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransportBackend {
    /// ACP/acpx 协议（首选）
    Acp { acpx_path: Option<String> },
    /// 无头 CLI 回退
    HeadlessCli { command: String, args: Vec<String> },
}

/// 外部 Agent 会话的权限配置
///
/// 注意：approve-all 被故意排除，以确保治理不让渡。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PermissionProfile {
    /// 每次操作都需要确认
    AskAll,
    /// 允许只读，写入需确认
    ReadOnly,
    /// 限定在指定目录范围内
    Scoped { allowed_paths: Vec<String> },
    /// 预授权的能力列表
    ///
    /// 约束：capabilities 必须在 spawn 时通过 DangerousCapabilityFilter 验证。
    /// High/Critical 风险的能力不能被预授权 — 它们始终需要运行时治理审批。
    PreAuthorized { capabilities: Vec<String> },
}

/// 外部 Agent 会话状态机
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SessionState {
    /// 正在启动
    Spawning,
    /// 活跃中
    Active,
    /// 已暂停
    Paused,
    /// 已完成
    Completed,
    /// 已失败
    Failed { reason: String },
    /// 已取消
    Cancelled,
}

/// 与外部编码 Agent 的受控会话
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpSession {
    /// 会话标识符（ACP 层）
    pub session_id: String,
    /// CyberClaw 执行映射
    pub execution_id: String,
    /// CyberClaw 追踪映射
    pub trace_id: String,
    /// 目标外部运行时
    pub runtime: ExternalRuntime,
    /// 当前状态
    pub state: SessionState,
    /// 权限约束
    pub permission_profile: PermissionProfile,
    /// 工作目录
    pub cwd: Option<String>,
    /// 模型覆盖
    pub model: Option<String>,
    /// 创建时间戳
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// 收集的事件
    pub events: Vec<SessionEvent>,
    /// 收集的产物（文件路径、差异等）
    pub artifacts: Vec<SessionArtifact>,
}

impl AcpSession {
    /// 将会话从 Spawning 转换为 Active
    pub fn activate(&mut self) -> anyhow::Result<()> {
        match &self.state {
            SessionState::Spawning => {
                self.state = SessionState::Active;
                Ok(())
            }
            other => anyhow::bail!(
                "Cannot activate session: invalid state transition from {:?}",
                other
            ),
        }
    }

    /// 将会话从 Active 转换为 Completed
    pub fn complete(&mut self) -> anyhow::Result<()> {
        match &self.state {
            SessionState::Active => {
                self.state = SessionState::Completed;
                Ok(())
            }
            other => anyhow::bail!(
                "Cannot complete session: invalid state transition from {:?}",
                other
            ),
        }
    }

    /// 将会话从 Active 转换为 Failed
    pub fn fail(&mut self, reason: String) -> anyhow::Result<()> {
        match &self.state {
            SessionState::Active => {
                self.state = SessionState::Failed { reason };
                Ok(())
            }
            other => anyhow::bail!(
                "Cannot fail session: invalid state transition from {:?}",
                other
            ),
        }
    }

    /// 将会话从 Active / Spawning / Paused 转换为 Cancelled
    ///
    /// 取消操作应从任何非终态状态都可执行。
    pub fn cancel(&mut self) -> anyhow::Result<()> {
        match &self.state {
            SessionState::Active | SessionState::Spawning | SessionState::Paused => {
                self.state = SessionState::Cancelled;
                Ok(())
            }
            other => anyhow::bail!(
                "Cannot cancel session: invalid state transition from {:?} (already terminal)",
                other
            ),
        }
    }

    /// 将会话从 Active 转换为 Paused
    pub fn pause(&mut self) -> anyhow::Result<()> {
        match &self.state {
            SessionState::Active => {
                self.state = SessionState::Paused;
                Ok(())
            }
            other => anyhow::bail!(
                "Cannot pause session: invalid state transition from {:?}",
                other
            ),
        }
    }

    /// 将会话从 Paused 转换为 Active
    pub fn resume(&mut self) -> anyhow::Result<()> {
        match &self.state {
            SessionState::Paused => {
                self.state = SessionState::Active;
                Ok(())
            }
            other => anyhow::bail!(
                "Cannot resume session: invalid state transition from {:?}",
                other
            ),
        }
    }
}

/// 外部 Agent 会话发出的事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEvent {
    /// 事件时间戳
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// 事件类型
    pub event_type: SessionEventType,
    /// 事件详情
    pub detail: serde_json::Value,
}

/// 会话事件类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionEventType {
    /// 会话已启动
    SessionStarted,
    /// 收到提示词
    PromptReceived,
    /// 工具调用开始
    ToolCallStarted,
    /// 工具调用完成
    ToolCallCompleted,
    /// 文件已变更
    FileChanged,
    /// 测试执行
    TestRun,
    /// 错误
    Error,
    /// 进度更新
    ProgressUpdate,
    /// 会话已完成
    SessionCompleted,
    /// 会话已失败
    SessionFailed,
}

/// 外部 Agent 会话产生的产物
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionArtifact {
    /// 产物类型
    pub artifact_type: ArtifactType,
    /// 文件路径
    pub path: Option<String>,
    /// 内容
    pub content: Option<String>,
    /// 元数据
    pub metadata: HashMap<String, String>,
}

/// 产物类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArtifactType {
    /// 文件差异
    FileDiff,
    /// 测试结果
    TestResult,
    /// 截图
    Screenshot,
    /// 日志
    Log,
    /// 摘要
    Summary,
}

/// ACP Runtime Connector 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpConfig {
    /// 默认传输后端
    pub default_transport: TransportBackend,
    /// 每运行时传输覆盖
    pub runtime_transports: HashMap<String, TransportBackend>,
    /// 最大并发会话数
    pub max_concurrent_sessions: usize,
    /// 会话超时（秒）
    pub session_timeout_secs: u64,
    /// 默认权限配置
    pub default_permission: PermissionProfile,
}

impl Default for AcpConfig {
    fn default() -> Self {
        Self {
            default_transport: TransportBackend::Acp { acpx_path: None },
            runtime_transports: HashMap::new(),
            max_concurrent_sessions: 4,
            session_timeout_secs: 3600,
            default_permission: PermissionProfile::AskAll,
        }
    }
}

/// 密钥存储引用
///
/// 实际密钥值永远不存储在 AcpSession 或 SessionEvent 中。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretRef {
    /// 密钥存储类型，例如 "env", "vault", "keychain"
    pub store: String,
    /// 密钥名称，例如 "CLAUDE_API_KEY"
    pub key: String,
}

// ---------------------------------------------------------------------------
// PreAuthorized 验证
// ---------------------------------------------------------------------------

/// 验证 PreAuthorized 能力列表中不包含 High/Critical 风险的能力。
///
/// 接收能力 ID 列表和一个风险级别查找函数。
/// 任何映射到 `RiskLevel::High` 或 `RiskLevel::Critical` 的能力将被拒绝。
pub fn validate_pre_authorized(
    capabilities: &[String],
    risk_lookup: impl Fn(&str) -> Option<RiskLevel>,
) -> anyhow::Result<()> {
    for cap in capabilities {
        if let Some(risk) = risk_lookup(cap) {
            if risk >= RiskLevel::High {
                anyhow::bail!(
                    "Cannot pre-authorize capability '{}': risk level {:?} requires runtime governance approval",
                    cap,
                    risk
                );
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// AcpRuntimeConnector
// ---------------------------------------------------------------------------

/// ACP 外部 Agent 运行时连接器
///
/// 管理外部编码 Agent（Claude Code, Codex, Gemini CLI, OpenCode）的生命周期，
/// 通过 ACP 协议或无头 CLI 进行通信。
#[derive(Debug)]
pub struct AcpRuntimeConnector {
    id: ConnectorId,
    sessions: Arc<RwLock<HashMap<String, AcpSession>>>,
    config: AcpConfig,
}

impl AcpRuntimeConnector {
    /// 创建新的 ACP Runtime Connector
    pub fn new(config: AcpConfig) -> Self {
        Self {
            id: ConnectorId::from_string("acp-runtime".to_string())
                .expect("Failed to create ConnectorId"),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// 使用默认配置创建
    pub fn with_defaults() -> Self {
        Self::new(AcpConfig::default())
    }

    /// 获取当前活跃会话数
    pub async fn active_session_count(&self) -> usize {
        let sessions = self.sessions.read().await;
        sessions
            .values()
            .filter(|s| matches!(s.state, SessionState::Active | SessionState::Spawning))
            .count()
    }

    /// 获取会话
    pub async fn get_session(&self, session_id: &str) -> Option<AcpSession> {
        self.sessions.read().await.get(session_id).cloned()
    }

    /// 获取配置引用
    pub fn config(&self) -> &AcpConfig {
        &self.config
    }

    /// 处理 session.spawn 请求
    async fn handle_spawn(
        &self,
        request: &CapabilityExecutionRequest,
    ) -> anyhow::Result<serde_json::Value> {
        #[derive(Deserialize)]
        struct SpawnInput {
            runtime: String,
            cwd: Option<String>,
            model: Option<String>,
            permission_profile: Option<String>,
            #[serde(default)]
            allowed_paths: Vec<String>,
            #[serde(default)]
            pre_authorized_capabilities: Vec<String>,
        }
        let input: SpawnInput = serde_json::from_value(request.input.clone())?;

        // 强制并发会话数上限
        let active_count = self.active_session_count().await;
        if active_count >= self.config.max_concurrent_sessions {
            anyhow::bail!(
                "Cannot spawn session: concurrent session limit reached ({}/{})",
                active_count,
                self.config.max_concurrent_sessions
            );
        }

        let runtime = match input.runtime.as_str() {
            "claude_code" => ExternalRuntime::ClaudeCode,
            "codex" => ExternalRuntime::Codex,
            "gemini_cli" => ExternalRuntime::GeminiCli,
            "opencode" => ExternalRuntime::OpenCode,
            other => ExternalRuntime::Custom(other.to_string()),
        };

        let permission = match input.permission_profile.as_deref() {
            Some("read_only") => PermissionProfile::ReadOnly,
            Some("scoped") => PermissionProfile::Scoped {
                allowed_paths: input.allowed_paths,
            },
            Some("pre_authorized") => PermissionProfile::PreAuthorized {
                capabilities: input.pre_authorized_capabilities,
            },
            _ => self.config.default_permission.clone(),
        };

        let session_id = uuid::Uuid::new_v4().to_string();
        let session = AcpSession {
            session_id: session_id.clone(),
            execution_id: request.execution_id.to_string(),
            trace_id: request.trace_id.clone(),
            runtime,
            state: SessionState::Spawning,
            permission_profile: permission,
            cwd: input.cwd,
            model: input.model,
            created_at: chrono::Utc::now(),
            events: vec![],
            artifacts: vec![],
        };

        self.sessions
            .write()
            .await
            .insert(session_id.clone(), session);

        Ok(serde_json::json!({
            "session_id": session_id,
            "state": "spawning"
        }))
    }

    /// 处理 session.status 请求
    async fn handle_status(
        &self,
        request: &CapabilityExecutionRequest,
    ) -> anyhow::Result<serde_json::Value> {
        #[derive(Deserialize)]
        struct StatusInput {
            session_id: String,
        }
        let input: StatusInput = serde_json::from_value(request.input.clone())?;
        let sessions = self.sessions.read().await;
        let session = sessions
            .get(&input.session_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", input.session_id))?;
        Ok(serde_json::to_value(&session.state)?)
    }

    /// 处理 session.stop 请求
    async fn handle_stop(
        &self,
        request: &CapabilityExecutionRequest,
    ) -> anyhow::Result<serde_json::Value> {
        #[derive(Deserialize)]
        struct StopInput {
            session_id: String,
        }
        let input: StopInput = serde_json::from_value(request.input.clone())?;
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(&input.session_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", input.session_id))?;
        session.cancel()?;
        Ok(serde_json::json!({
            "session_id": input.session_id,
            "state": "cancelled"
        }))
    }

    /// 处理 prompt.send 请求
    async fn handle_prompt_send(
        &self,
        request: &CapabilityExecutionRequest,
    ) -> anyhow::Result<serde_json::Value> {
        #[derive(Deserialize)]
        struct PromptInput {
            session_id: String,
            prompt: String,
        }
        let input: PromptInput = serde_json::from_value(request.input.clone())?;
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(&input.session_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", input.session_id))?;

        // 如果会话仍在 Spawning 状态，先激活它
        if session.state == SessionState::Spawning {
            session.activate()?;
        }

        // 记录事件
        session.events.push(SessionEvent {
            timestamp: chrono::Utc::now(),
            event_type: SessionEventType::PromptReceived,
            detail: serde_json::json!({ "prompt": input.prompt }),
        });

        Ok(serde_json::json!({
            "session_id": input.session_id,
            "accepted": true
        }))
    }

    /// 处理 session.resume 请求
    async fn handle_resume(
        &self,
        request: &CapabilityExecutionRequest,
    ) -> anyhow::Result<serde_json::Value> {
        #[derive(Deserialize)]
        struct ResumeInput {
            session_id: String,
        }
        let input: ResumeInput = serde_json::from_value(request.input.clone())?;
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(&input.session_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", input.session_id))?;

        match session.state {
            SessionState::Paused => {
                session.resume()?;
            }
            SessionState::Active => {}
            _ => {
                anyhow::bail!(
                    "Cannot resume session '{}' from state {:?}",
                    input.session_id,
                    session.state
                );
            }
        }

        Ok(serde_json::json!({
            "session_id": input.session_id,
            "state": "active"
        }))
    }

    /// 处理 prompt.followup 请求
    async fn handle_prompt_followup(
        &self,
        request: &CapabilityExecutionRequest,
    ) -> anyhow::Result<serde_json::Value> {
        #[derive(Deserialize)]
        struct FollowupInput {
            session_id: String,
            message: String,
        }
        let input: FollowupInput = serde_json::from_value(request.input.clone())?;
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(&input.session_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", input.session_id))?;

        if session.state == SessionState::Spawning {
            session.activate()?;
        }
        if session.state != SessionState::Active {
            anyhow::bail!(
                "Cannot send follow-up to session '{}' in state {:?}",
                input.session_id,
                session.state
            );
        }

        session.events.push(SessionEvent {
            timestamp: chrono::Utc::now(),
            event_type: SessionEventType::PromptReceived,
            detail: serde_json::json!({ "message": input.message }),
        });

        Ok(serde_json::json!({
            "session_id": input.session_id,
            "accepted": true
        }))
    }

    /// 处理 prompt.interrupt 请求
    async fn handle_prompt_interrupt(
        &self,
        request: &CapabilityExecutionRequest,
    ) -> anyhow::Result<serde_json::Value> {
        #[derive(Deserialize)]
        struct InterruptInput {
            session_id: String,
            reason: Option<String>,
        }
        let input: InterruptInput = serde_json::from_value(request.input.clone())?;
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(&input.session_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", input.session_id))?;

        match session.state {
            SessionState::Spawning => {
                session.activate()?;
                session.pause()?;
            }
            SessionState::Active => {
                session.pause()?;
            }
            SessionState::Paused => {}
            _ => {
                anyhow::bail!(
                    "Cannot interrupt session '{}' from state {:?}",
                    input.session_id,
                    session.state
                );
            }
        }

        session.events.push(SessionEvent {
            timestamp: chrono::Utc::now(),
            event_type: SessionEventType::ProgressUpdate,
            detail: serde_json::json!({
                "event": "interrupted",
                "reason": input.reason.unwrap_or_else(|| "manual interrupt".to_string())
            }),
        });

        Ok(serde_json::json!({
            "session_id": input.session_id,
            "acknowledged": true
        }))
    }

    /// 处理 artifact.collect 请求
    async fn handle_artifact_collect(
        &self,
        request: &CapabilityExecutionRequest,
    ) -> anyhow::Result<serde_json::Value> {
        #[derive(Deserialize)]
        struct ArtifactInput {
            session_id: String,
        }
        let input: ArtifactInput = serde_json::from_value(request.input.clone())?;
        let sessions = self.sessions.read().await;
        let session = sessions
            .get(&input.session_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", input.session_id))?;

        Ok(serde_json::json!({
            "session_id": input.session_id,
            "artifacts": session.artifacts.clone()
        }))
    }

    /// 处理 env.set 请求
    async fn handle_env_set(
        &self,
        request: &CapabilityExecutionRequest,
    ) -> anyhow::Result<serde_json::Value> {
        #[derive(Deserialize)]
        struct EnvInput {
            session_id: String,
            cwd: Option<String>,
            model: Option<String>,
        }
        let input: EnvInput = serde_json::from_value(request.input.clone())?;
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(&input.session_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", input.session_id))?;

        if let Some(cwd) = input.cwd {
            session.cwd = Some(cwd);
        }
        if let Some(model) = input.model {
            session.model = Some(model);
        }

        session.events.push(SessionEvent {
            timestamp: chrono::Utc::now(),
            event_type: SessionEventType::ProgressUpdate,
            detail: serde_json::json!({
                "event": "environment_updated",
                "cwd": session.cwd.clone(),
                "model": session.model.clone()
            }),
        });

        Ok(serde_json::json!({
            "session_id": input.session_id,
            "updated": true
        }))
    }

    /// 处理 permission.set 请求
    async fn handle_permission_set(
        &self,
        request: &CapabilityExecutionRequest,
    ) -> anyhow::Result<serde_json::Value> {
        #[derive(Deserialize)]
        struct PermissionInput {
            session_id: String,
            permission_profile: String,
            #[serde(default)]
            allowed_paths: Vec<String>,
            #[serde(default)]
            pre_authorized_capabilities: Vec<String>,
        }
        let input: PermissionInput = serde_json::from_value(request.input.clone())?;
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(&input.session_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", input.session_id))?;

        let next_permission = match input.permission_profile.as_str() {
            "ask_all" => PermissionProfile::AskAll,
            "read_only" => PermissionProfile::ReadOnly,
            "scoped" => PermissionProfile::Scoped {
                allowed_paths: input.allowed_paths,
            },
            "pre_authorized" => PermissionProfile::PreAuthorized {
                capabilities: input.pre_authorized_capabilities,
            },
            other => anyhow::bail!("Unknown permission profile: {}", other),
        };
        session.permission_profile = next_permission;
        session.events.push(SessionEvent {
            timestamp: chrono::Utc::now(),
            event_type: SessionEventType::ProgressUpdate,
            detail: serde_json::json!({
                "event": "permission_profile_updated",
                "profile": input.permission_profile
            }),
        });

        Ok(serde_json::json!({
            "session_id": input.session_id,
            "updated": true
        }))
    }
}

#[async_trait::async_trait]
impl Connector for AcpRuntimeConnector {
    fn id(&self) -> &ConnectorId {
        &self.id
    }

    fn runtime(&self) -> ConnectorRuntime {
        ConnectorRuntime::Process
    }

    fn capabilities(&self) -> Vec<CapabilityContract> {
        static CAPABILITIES: once_cell::sync::Lazy<Vec<CapabilityContract>> =
            once_cell::sync::Lazy::new(|| {
                vec![
                    CapabilityContract {
                        id: "external_agent.session.spawn".to_string(),
                        title: "Spawn External Agent Session".to_string(),
                        description: Some("Create a new external agent session".to_string()),
                        risk: RiskLevel::Medium,
                        effects: vec![CapabilityEffect::Write],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "runtime": {"type": "string"},
                                "cwd": {"type": "string"},
                                "model": {"type": "string"},
                                "permission_profile": {"type": "string"}
                            },
                            "required": ["runtime"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "session_id": {"type": "string"},
                                "state": {"type": "string"}
                            }
                        }))
                        .unwrap(),
                    },
                    CapabilityContract {
                        id: "external_agent.session.resume".to_string(),
                        title: "Resume External Agent Session".to_string(),
                        description: Some("Resume a paused session".to_string()),
                        risk: RiskLevel::Low,
                        effects: vec![CapabilityEffect::Write],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": { "session_id": {"type": "string"} },
                            "required": ["session_id"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": { "session_id": {"type": "string"}, "state": {"type": "string"} }
                        }))
                        .unwrap(),
                    },
                    CapabilityContract {
                        id: "external_agent.session.stop".to_string(),
                        title: "Stop External Agent Session".to_string(),
                        description: Some("Stop an active session".to_string()),
                        risk: RiskLevel::Medium,
                        effects: vec![CapabilityEffect::Write],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": { "session_id": {"type": "string"} },
                            "required": ["session_id"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": { "session_id": {"type": "string"}, "state": {"type": "string"} }
                        }))
                        .unwrap(),
                    },
                    CapabilityContract {
                        id: "external_agent.session.status".to_string(),
                        title: "Get Session Status".to_string(),
                        description: Some("Get session state and progress".to_string()),
                        risk: RiskLevel::Low,
                        effects: vec![CapabilityEffect::Read],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": { "session_id": {"type": "string"} },
                            "required": ["session_id"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": { "state": {"type": "string"} }
                        }))
                        .unwrap(),
                    },
                    CapabilityContract {
                        id: "external_agent.prompt.send".to_string(),
                        title: "Send Prompt".to_string(),
                        description: Some("Send initial task prompt to session".to_string()),
                        risk: RiskLevel::Medium,
                        effects: vec![CapabilityEffect::Write],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "session_id": {"type": "string"},
                                "prompt": {"type": "string"}
                            },
                            "required": ["session_id", "prompt"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": { "session_id": {"type": "string"}, "accepted": {"type": "boolean"} }
                        }))
                        .unwrap(),
                    },
                    CapabilityContract {
                        id: "external_agent.prompt.followup".to_string(),
                        title: "Send Follow-up".to_string(),
                        description: Some("Send follow-up instruction".to_string()),
                        risk: RiskLevel::Low,
                        effects: vec![CapabilityEffect::Write],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "session_id": {"type": "string"},
                                "message": {"type": "string"}
                            },
                            "required": ["session_id", "message"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": { "session_id": {"type": "string"}, "accepted": {"type": "boolean"} }
                        }))
                        .unwrap(),
                    },
                    CapabilityContract {
                        id: "external_agent.prompt.interrupt".to_string(),
                        title: "Interrupt Session".to_string(),
                        description: Some("Interrupt current operation".to_string()),
                        risk: RiskLevel::High,
                        effects: vec![CapabilityEffect::Write],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "session_id": {"type": "string"},
                                "reason": {"type": "string"}
                            },
                            "required": ["session_id"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": { "acknowledged": {"type": "boolean"} }
                        }))
                        .unwrap(),
                    },
                    CapabilityContract {
                        id: "external_agent.artifact.collect".to_string(),
                        title: "Collect Artifacts".to_string(),
                        description: Some("Collect session artifacts".to_string()),
                        risk: RiskLevel::Low,
                        effects: vec![CapabilityEffect::Read],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": { "session_id": {"type": "string"} },
                            "required": ["session_id"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": { "artifacts": {"type": "array"} }
                        }))
                        .unwrap(),
                    },
                    CapabilityContract {
                        id: "external_agent.env.set".to_string(),
                        title: "Set Environment".to_string(),
                        description: Some("Set working directory and model".to_string()),
                        risk: RiskLevel::Medium,
                        effects: vec![CapabilityEffect::Write],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "session_id": {"type": "string"},
                                "cwd": {"type": "string"},
                                "model": {"type": "string"}
                            },
                            "required": ["session_id"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": { "session_id": {"type": "string"}, "updated": {"type": "boolean"} }
                        }))
                        .unwrap(),
                    },
                    CapabilityContract {
                        id: "external_agent.permission.set".to_string(),
                        title: "Set Permission Profile".to_string(),
                        description: Some("Change permission profile".to_string()),
                        risk: RiskLevel::High,
                        effects: vec![CapabilityEffect::Write],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "session_id": {"type": "string"},
                                "permission_profile": {"type": "string"}
                            },
                            "required": ["session_id", "permission_profile"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": { "session_id": {"type": "string"}, "updated": {"type": "boolean"} }
                        }))
                        .unwrap(),
                    },
                ]
            });
        CAPABILITIES.clone()
    }

    async fn execute(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<CapabilityExecutionResult> {
        let capability_name = request.capability_id.as_str();

        let (output, status, error) = match capability_name {
            "external_agent.session.spawn" => match self.handle_spawn(&request).await {
                Ok(out) => (out, ExecutionStatus::Success, None),
                Err(e) => (
                    serde_json::Value::Null,
                    ExecutionStatus::Failed,
                    Some(e.to_string()),
                ),
            },
            "external_agent.session.status" => match self.handle_status(&request).await {
                Ok(out) => (out, ExecutionStatus::Success, None),
                Err(e) => (
                    serde_json::Value::Null,
                    ExecutionStatus::Failed,
                    Some(e.to_string()),
                ),
            },
            "external_agent.session.resume" => match self.handle_resume(&request).await {
                Ok(out) => (out, ExecutionStatus::Success, None),
                Err(e) => (
                    serde_json::Value::Null,
                    ExecutionStatus::Failed,
                    Some(e.to_string()),
                ),
            },
            "external_agent.session.stop" => match self.handle_stop(&request).await {
                Ok(out) => (out, ExecutionStatus::Success, None),
                Err(e) => (
                    serde_json::Value::Null,
                    ExecutionStatus::Failed,
                    Some(e.to_string()),
                ),
            },
            "external_agent.prompt.send" => match self.handle_prompt_send(&request).await {
                Ok(out) => (out, ExecutionStatus::Success, None),
                Err(e) => (
                    serde_json::Value::Null,
                    ExecutionStatus::Failed,
                    Some(e.to_string()),
                ),
            },
            "external_agent.prompt.followup" => match self.handle_prompt_followup(&request).await {
                Ok(out) => (out, ExecutionStatus::Success, None),
                Err(e) => (
                    serde_json::Value::Null,
                    ExecutionStatus::Failed,
                    Some(e.to_string()),
                ),
            },
            "external_agent.prompt.interrupt" => {
                match self.handle_prompt_interrupt(&request).await {
                    Ok(out) => (out, ExecutionStatus::Success, None),
                    Err(e) => (
                        serde_json::Value::Null,
                        ExecutionStatus::Failed,
                        Some(e.to_string()),
                    ),
                }
            }
            "external_agent.artifact.collect" => match self.handle_artifact_collect(&request).await
            {
                Ok(out) => (out, ExecutionStatus::Success, None),
                Err(e) => (
                    serde_json::Value::Null,
                    ExecutionStatus::Failed,
                    Some(e.to_string()),
                ),
            },
            "external_agent.env.set" => match self.handle_env_set(&request).await {
                Ok(out) => (out, ExecutionStatus::Success, None),
                Err(e) => (
                    serde_json::Value::Null,
                    ExecutionStatus::Failed,
                    Some(e.to_string()),
                ),
            },
            "external_agent.permission.set" => match self.handle_permission_set(&request).await {
                Ok(out) => (out, ExecutionStatus::Success, None),
                Err(e) => (
                    serde_json::Value::Null,
                    ExecutionStatus::Failed,
                    Some(e.to_string()),
                ),
            },
            other => (
                serde_json::Value::Null,
                ExecutionStatus::Failed,
                Some(format!("Unknown capability: {}", other)),
            ),
        };

        Ok(CapabilityExecutionResult {
            execution_id: request.execution_id,
            trace_id: request.trace_id,
            connector_id: self.id.clone(),
            capability_id: request.capability_id,
            output,
            status,
            error,
            actual_runtime: Some(crate::runtime::RuntimeMode::Process),
        })
    }
}
