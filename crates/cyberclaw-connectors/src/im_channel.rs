//! # IM Channel Connector
//!
//! IM 渠道接入连接器，负责收发消息。它不关心消息内容如何处理 — 交给控制平面。
//! 支持文字、语音、卡片等消息类型，通过 ImPlatformAdapter 适配不同 IM 平台。
//!
//! ## 支持的 Capabilities
//!
//! - `channel.message.receive`: 接收并标准化入站消息
//! - `channel.message.send`: 发送文字回复
//! - `channel.card.send`: 发送富文本卡片
//! - `channel.voice.receive`: 接收并下载语音消息
//! - `channel.voice.send`: 发送语音回复
//! - `channel.session.bind`: 绑定 chat 到 CyberClaw session
//! - `channel.session.status`: 查询当前 session 绑定状态

use crate::types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, Connector, ExecutionStatus,
};
use base64::Engine as _;
use cyberclaw_core::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// ImPlatformAdapter trait
// ---------------------------------------------------------------------------

/// IM 平台适配器 trait
/// 每个 IM 平台实现一个 adapter，处理平台特有的 API 差异
#[async_trait::async_trait]
pub trait ImPlatformAdapter: Send + Sync {
    /// 平台标识
    fn platform_id(&self) -> &str;

    /// 是否支持语音消息
    fn supports_voice(&self) -> bool;

    /// 是否支持实时音频（Phase 2+）
    fn supports_realtime_audio(&self) -> bool {
        false
    }

    /// 标准化入站消息（文字或语音）
    async fn normalize_inbound(&self, raw: serde_json::Value) -> anyhow::Result<ImMessage>;

    /// 发送文字回复
    async fn send_text(&self, chat_id: &str, text: &str) -> anyhow::Result<()>;

    /// 发送富文本卡片（飞书特有，其他平台降级为文字）
    async fn send_card(&self, chat_id: &str, card: serde_json::Value) -> anyhow::Result<()> {
        // 默认降级为纯文字
        let text = card.get("text").and_then(|v| v.as_str()).unwrap_or("");
        self.send_text(chat_id, text).await
    }

    /// 发送语音回复
    async fn send_voice(
        &self,
        chat_id: &str,
        audio: &[u8],
        format: AudioFormat,
    ) -> anyhow::Result<()>;

    /// 下载语音附件
    async fn download_voice(&self, media_id: &str) -> anyhow::Result<Vec<u8>>;

    /// 校验 webhook 签名
    fn validate_signature(&self, payload: &[u8], signature: &str) -> bool;
}

// ---------------------------------------------------------------------------
// Core Types
// ---------------------------------------------------------------------------

/// 标准化 IM 消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImMessage {
    /// 消息唯一 ID
    pub id: String,
    /// 来源平台标识
    pub platform: String,
    /// 频道/会话标识
    pub chat_id: String,
    /// 发送者标识
    pub sender: String,
    /// 消息类型
    pub message_type: ImMessageType,
    /// 文字内容（文字消息直接有，语音消息 STT 后填入）
    pub text_content: Option<String>,
    /// 语音媒体 ID（用于下载）
    pub voice_media_id: Option<String>,
    /// 语音时长
    pub voice_duration_secs: Option<f32>,
    /// 消息时间戳
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// 附加元数据
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// 原始 JSON 载荷
    pub raw: serde_json::Value,
}

/// IM 消息类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ImMessageType {
    /// 文字消息
    Text,
    /// 语音消息
    Voice,
    /// 音频消息
    Audio,
    /// /command 格式
    Command,
    /// 卡片按钮回调
    Interactive,
}

/// 音频格式
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AudioFormat {
    Ogg,
    Mp3,
    Wav,
    Opus,
    M4a,
    /// WeCom format
    Amr,
    /// Raw PCM for streaming
    Pcm,
}

/// IM 会话与 CyberClaw 执行的绑定关系
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionBinding {
    /// 频道/会话标识
    pub chat_id: String,
    /// 来源平台标识
    pub platform: String,
    /// CyberClaw session
    pub session_id: String,
    /// 当前活跃的 execution（可为空）
    pub execution_id: Option<String>,
    /// 当前绑定的外部 agent session（可为空，仅当操控外部 agent 时）
    pub external_session_id: Option<String>,
    /// 回复模式
    pub reply_mode: ReplyMode,
    /// 创建时间
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// 最后活跃时间
    pub last_activity: chrono::DateTime<chrono::Utc>,
}

/// 回复模式：控制 Bot 用什么方式回复
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ReplyMode {
    /// 始终用文字回复
    TextOnly,
    /// 始终用语音回复
    VoiceOnly,
    /// 跟随用户：用户发文字就回文字，发语音就回语音
    FollowUser,
    /// 驾驶模式：强制语音回复 + 安全策略
    DrivingMode,
}

/// IM 渠道配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImChannelConfig {
    /// 默认回复模式
    pub default_reply_mode: ReplyMode,
    /// session 超时（秒）
    pub session_timeout_secs: u64,
    /// 最大并发 session 数
    pub max_sessions: usize,
    /// 消息去重窗口大小
    pub dedup_window_size: usize,
}

impl Default for ImChannelConfig {
    fn default() -> Self {
        Self {
            default_reply_mode: ReplyMode::FollowUser,
            session_timeout_secs: 3600,
            max_sessions: 1000,
            dedup_window_size: 1000,
        }
    }
}

// ---------------------------------------------------------------------------
// ImChannelConnector
// ---------------------------------------------------------------------------

/// IM 渠道连接器
#[derive(Debug)]
pub struct ImChannelConnector {
    id: ConnectorId,
    adapters: Arc<RwLock<HashMap<String, Arc<dyn ImPlatformAdapter>>>>,
    bindings: Arc<RwLock<HashMap<String, SessionBinding>>>,
    config: ImChannelConfig,
    /// 消息 ID 去重 FIFO 队列（有界，自动淘汰最旧条目）
    seen_message_ids: Arc<RwLock<VecDeque<String>>>,
}

impl ImChannelConnector {
    /// 创建新的 IM Channel Connector
    pub fn new(config: ImChannelConfig) -> Self {
        Self {
            id: ConnectorId::from_string("im-channel".to_string())
                .expect("Failed to create ConnectorId"),
            adapters: Arc::new(RwLock::new(HashMap::new())),
            bindings: Arc::new(RwLock::new(HashMap::new())),
            config,
            seen_message_ids: Arc::new(RwLock::new(VecDeque::new())),
        }
    }

    /// 注册平台适配器
    pub async fn register_adapter(&self, adapter: Arc<dyn ImPlatformAdapter>) {
        self.adapters
            .write()
            .await
            .insert(adapter.platform_id().to_string(), adapter);
    }

    /// 创建或更新 session 绑定
    pub async fn bind_session(&self, binding: SessionBinding) -> anyhow::Result<()> {
        let mut bindings = self.bindings.write().await;
        if bindings.len() >= self.config.max_sessions && !bindings.contains_key(&binding.chat_id) {
            anyhow::bail!("已达到最大 session 数 ({})", self.config.max_sessions);
        }
        bindings.insert(binding.chat_id.clone(), binding);
        Ok(())
    }

    /// 查询 session 绑定
    pub async fn lookup_binding(&self, chat_id: &str) -> Option<SessionBinding> {
        let bindings = self.bindings.read().await;
        let binding = bindings.get(chat_id)?;
        // 检查是否过期
        let elapsed = chrono::Utc::now()
            .signed_duration_since(binding.last_activity)
            .num_seconds() as u64;
        if elapsed > self.config.session_timeout_secs {
            return None;
        }
        Some(binding.clone())
    }

    /// 更新 session 最后活跃时间
    pub async fn touch_session(&self, chat_id: &str) -> bool {
        let mut bindings = self.bindings.write().await;
        if let Some(binding) = bindings.get_mut(chat_id) {
            binding.last_activity = chrono::Utc::now();
            true
        } else {
            false
        }
    }

    /// 检查消息是否重复（基于消息 ID 去重，FIFO 淘汰最旧条目）
    pub async fn is_duplicate(&self, message_id: &str) -> bool {
        let mut seen = self.seen_message_ids.write().await;
        if seen.iter().any(|id| id == message_id) {
            return true;
        }
        // FIFO 淘汰：超出窗口大小时移除最旧条目，而非清空全部
        while seen.len() >= self.config.dedup_window_size {
            seen.pop_front();
        }
        seen.push_back(message_id.to_string());
        false
    }

    /// 获取适配器
    async fn get_adapter(&self, platform: &str) -> anyhow::Result<Arc<dyn ImPlatformAdapter>> {
        self.adapters
            .read()
            .await
            .get(platform)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("未知平台: {}", platform))
    }
}

impl Default for ImChannelConnector {
    fn default() -> Self {
        Self::new(ImChannelConfig::default())
    }
}

// ---------------------------------------------------------------------------
// Connector impl
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl Connector for ImChannelConnector {
    fn id(&self) -> &ConnectorId {
        &self.id
    }

    fn runtime(&self) -> ConnectorRuntime {
        ConnectorRuntime::Native
    }

    fn capabilities(&self) -> Vec<CapabilityContract> {
        static CAPABILITIES: once_cell::sync::Lazy<Vec<CapabilityContract>> =
            once_cell::sync::Lazy::new(|| {
                vec![
                    CapabilityContract {
                        id: "channel.message.receive".to_string(),
                        title: "Receive IM Message".to_string(),
                        description: Some("接收并标准化入站消息（文字或语音）".to_string()),
                        risk: RiskLevel::Low,
                        effects: vec![CapabilityEffect::Read],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "platform": {"type": "string"},
                                "payload": {"type": "object"}
                            },
                            "required": ["platform", "payload"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "message": {"type": "object"}
                            }
                        }))
                        .unwrap(),
                    },
                    CapabilityContract {
                        id: "channel.message.send".to_string(),
                        title: "Send Text Message".to_string(),
                        description: Some("发送文字回复".to_string()),
                        risk: RiskLevel::Low,
                        effects: vec![CapabilityEffect::Notification],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "platform": {"type": "string"},
                                "chat_id": {"type": "string"},
                                "text": {"type": "string"}
                            },
                            "required": ["platform", "chat_id", "text"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "success": {"type": "boolean"}
                            }
                        }))
                        .unwrap(),
                    },
                    CapabilityContract {
                        id: "channel.card.send".to_string(),
                        title: "Send Card Message".to_string(),
                        description: Some("发送富文本卡片（飞书）".to_string()),
                        risk: RiskLevel::Low,
                        effects: vec![CapabilityEffect::Notification],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "platform": {"type": "string"},
                                "chat_id": {"type": "string"},
                                "card": {"type": "object"}
                            },
                            "required": ["platform", "chat_id", "card"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "success": {"type": "boolean"}
                            }
                        }))
                        .unwrap(),
                    },
                    CapabilityContract {
                        id: "channel.voice.receive".to_string(),
                        title: "Receive Voice Message".to_string(),
                        description: Some("接收并下载语音消息".to_string()),
                        risk: RiskLevel::Low,
                        effects: vec![CapabilityEffect::Read],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "platform": {"type": "string"},
                                "media_id": {"type": "string"}
                            },
                            "required": ["platform", "media_id"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "audio": {"type": "string", "description": "base64 encoded audio"}
                            }
                        }))
                        .unwrap(),
                    },
                    CapabilityContract {
                        id: "channel.voice.send".to_string(),
                        title: "Send Voice Message".to_string(),
                        description: Some("发送语音回复".to_string()),
                        risk: RiskLevel::Low,
                        effects: vec![CapabilityEffect::Notification],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "platform": {"type": "string"},
                                "chat_id": {"type": "string"},
                                "audio": {"type": "string"},
                                "format": {"type": "string"}
                            },
                            "required": ["platform", "chat_id", "audio", "format"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "success": {"type": "boolean"}
                            }
                        }))
                        .unwrap(),
                    },
                    CapabilityContract {
                        id: "channel.session.bind".to_string(),
                        title: "Bind Session".to_string(),
                        description: Some("绑定 chat 到 CyberClaw session".to_string()),
                        risk: RiskLevel::Low,
                        effects: vec![CapabilityEffect::Write],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "chat_id": {"type": "string"},
                                "platform": {"type": "string"},
                                "session_id": {"type": "string"},
                                "execution_id": {"type": "string"},
                                "external_session_id": {"type": "string"},
                                "reply_mode": {"type": "string"}
                            },
                            "required": ["chat_id", "platform", "session_id"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "success": {"type": "boolean"}
                            }
                        }))
                        .unwrap(),
                    },
                    CapabilityContract {
                        id: "channel.session.status".to_string(),
                        title: "Session Status".to_string(),
                        description: Some("查询当前 session 绑定状态".to_string()),
                        risk: RiskLevel::Low,
                        effects: vec![CapabilityEffect::Read],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "chat_id": {"type": "string"}
                            },
                            "required": ["chat_id"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "binding": {"type": "object"}
                            }
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
            "channel.message.receive" => {
                #[derive(Deserialize)]
                struct Input {
                    platform: String,
                    payload: serde_json::Value,
                }
                let input: Input = serde_json::from_value(request.input.clone())?;
                let adapter = self.get_adapter(&input.platform).await;
                match adapter {
                    Ok(adapter) => match adapter.normalize_inbound(input.payload).await {
                        Ok(msg) => {
                            if self.is_duplicate(&msg.id).await {
                                (
                                    serde_json::json!({"duplicate": true}),
                                    ExecutionStatus::Success,
                                    None,
                                )
                            } else {
                                (serde_json::to_value(&msg)?, ExecutionStatus::Success, None)
                            }
                        }
                        Err(e) => (
                            serde_json::Value::Null,
                            ExecutionStatus::Failed,
                            Some(e.to_string()),
                        ),
                    },
                    Err(e) => (
                        serde_json::Value::Null,
                        ExecutionStatus::Failed,
                        Some(e.to_string()),
                    ),
                }
            }
            "channel.message.send" => {
                #[derive(Deserialize)]
                struct Input {
                    platform: String,
                    chat_id: String,
                    text: String,
                }
                let input: Input = serde_json::from_value(request.input.clone())?;
                let adapter = self.get_adapter(&input.platform).await;
                match adapter {
                    Ok(adapter) => match adapter.send_text(&input.chat_id, &input.text).await {
                        Ok(()) => (
                            serde_json::json!({"success": true}),
                            ExecutionStatus::Success,
                            None,
                        ),
                        Err(e) => (
                            serde_json::Value::Null,
                            ExecutionStatus::Failed,
                            Some(e.to_string()),
                        ),
                    },
                    Err(e) => (
                        serde_json::Value::Null,
                        ExecutionStatus::Failed,
                        Some(e.to_string()),
                    ),
                }
            }
            "channel.card.send" => {
                #[derive(Deserialize)]
                struct Input {
                    platform: String,
                    chat_id: String,
                    card: serde_json::Value,
                }
                let input: Input = serde_json::from_value(request.input.clone())?;
                let adapter = self.get_adapter(&input.platform).await;
                match adapter {
                    Ok(adapter) => match adapter.send_card(&input.chat_id, input.card).await {
                        Ok(()) => (
                            serde_json::json!({"success": true}),
                            ExecutionStatus::Success,
                            None,
                        ),
                        Err(e) => (
                            serde_json::Value::Null,
                            ExecutionStatus::Failed,
                            Some(e.to_string()),
                        ),
                    },
                    Err(e) => (
                        serde_json::Value::Null,
                        ExecutionStatus::Failed,
                        Some(e.to_string()),
                    ),
                }
            }
            "channel.voice.receive" => {
                #[derive(Deserialize)]
                struct Input {
                    platform: String,
                    media_id: String,
                }
                let input: Input = serde_json::from_value(request.input.clone())?;
                let adapter = self.get_adapter(&input.platform).await;
                match adapter {
                    Ok(adapter) => match adapter.download_voice(&input.media_id).await {
                        Ok(audio) => {
                            let encoded = base64::engine::general_purpose::STANDARD.encode(&audio);
                            (
                                serde_json::json!({"audio": encoded}),
                                ExecutionStatus::Success,
                                None,
                            )
                        }
                        Err(e) => (
                            serde_json::Value::Null,
                            ExecutionStatus::Failed,
                            Some(e.to_string()),
                        ),
                    },
                    Err(e) => (
                        serde_json::Value::Null,
                        ExecutionStatus::Failed,
                        Some(e.to_string()),
                    ),
                }
            }
            "channel.voice.send" => {
                #[derive(Deserialize)]
                struct Input {
                    platform: String,
                    chat_id: String,
                    audio: String,
                    format: AudioFormat,
                }
                let input: Input = serde_json::from_value(request.input.clone())?;
                let audio_bytes = base64::engine::general_purpose::STANDARD
                    .decode(&input.audio)
                    .map_err(|e| anyhow::anyhow!("base64 解码失败: {}", e))?;
                let adapter = self.get_adapter(&input.platform).await;
                match adapter {
                    Ok(adapter) => {
                        match adapter
                            .send_voice(&input.chat_id, &audio_bytes, input.format)
                            .await
                        {
                            Ok(()) => (
                                serde_json::json!({"success": true}),
                                ExecutionStatus::Success,
                                None,
                            ),
                            Err(e) => (
                                serde_json::Value::Null,
                                ExecutionStatus::Failed,
                                Some(e.to_string()),
                            ),
                        }
                    }
                    Err(e) => (
                        serde_json::Value::Null,
                        ExecutionStatus::Failed,
                        Some(e.to_string()),
                    ),
                }
            }
            "channel.session.bind" => {
                #[derive(Deserialize)]
                struct Input {
                    chat_id: String,
                    platform: String,
                    session_id: String,
                    execution_id: Option<String>,
                    external_session_id: Option<String>,
                    reply_mode: Option<ReplyMode>,
                }
                let input: Input = serde_json::from_value(request.input.clone())?;
                let now = chrono::Utc::now();
                let existing = self.lookup_binding(&input.chat_id).await;
                let binding = SessionBinding {
                    chat_id: input.chat_id,
                    platform: input.platform,
                    session_id: input.session_id,
                    execution_id: input
                        .execution_id
                        .or_else(|| existing.as_ref().and_then(|b| b.execution_id.clone())),
                    external_session_id: input.external_session_id.or_else(|| {
                        existing
                            .as_ref()
                            .and_then(|b| b.external_session_id.clone())
                    }),
                    reply_mode: input
                        .reply_mode
                        .or_else(|| existing.as_ref().map(|b| b.reply_mode.clone()))
                        .unwrap_or_else(|| self.config.default_reply_mode.clone()),
                    created_at: existing.as_ref().map(|b| b.created_at).unwrap_or(now),
                    last_activity: now,
                };
                match self.bind_session(binding).await {
                    Ok(()) => (
                        serde_json::json!({"success": true}),
                        ExecutionStatus::Success,
                        None,
                    ),
                    Err(e) => (
                        serde_json::Value::Null,
                        ExecutionStatus::Failed,
                        Some(e.to_string()),
                    ),
                }
            }
            "channel.session.status" => {
                #[derive(Deserialize)]
                struct Input {
                    chat_id: String,
                }
                let input: Input = serde_json::from_value(request.input.clone())?;
                let binding = self.lookup_binding(&input.chat_id).await;
                (
                    serde_json::json!({"binding": binding}),
                    ExecutionStatus::Success,
                    None,
                )
            }
            other => (
                serde_json::Value::Null,
                ExecutionStatus::Failed,
                Some(format!("未知 capability: {}", other)),
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
            actual_runtime: Some(crate::runtime::RuntimeMode::Native),
        })
    }
}

// ---------------------------------------------------------------------------
// Debug impl for dyn ImPlatformAdapter
// ---------------------------------------------------------------------------

impl std::fmt::Debug for dyn ImPlatformAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImPlatformAdapter")
            .field("platform_id", &self.platform_id())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用 mock 适配器
    struct MockImAdapter;

    #[async_trait::async_trait]
    impl ImPlatformAdapter for MockImAdapter {
        fn platform_id(&self) -> &str {
            "mock"
        }

        fn supports_voice(&self) -> bool {
            true
        }

        async fn normalize_inbound(&self, raw: serde_json::Value) -> anyhow::Result<ImMessage> {
            let id = raw
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("mock-id")
                .to_string();
            let text = raw
                .get("text")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let msg_type = if raw.get("voice").is_some() {
                ImMessageType::Voice
            } else {
                ImMessageType::Text
            };
            Ok(ImMessage {
                id,
                platform: "mock".to_string(),
                chat_id: raw
                    .get("chat_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("chat-1")
                    .to_string(),
                sender: raw
                    .get("sender")
                    .and_then(|v| v.as_str())
                    .unwrap_or("user-1")
                    .to_string(),
                message_type: msg_type,
                text_content: text,
                voice_media_id: raw
                    .get("voice")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                voice_duration_secs: None,
                timestamp: chrono::Utc::now(),
                metadata: HashMap::new(),
                raw,
            })
        }

        async fn send_text(&self, _chat_id: &str, _text: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn send_voice(
            &self,
            _chat_id: &str,
            _audio: &[u8],
            _format: AudioFormat,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn download_voice(&self, _media_id: &str) -> anyhow::Result<Vec<u8>> {
            Ok(vec![0u8; 100])
        }

        fn validate_signature(&self, _payload: &[u8], _signature: &str) -> bool {
            true
        }
    }

    #[test]
    fn test_im_channel_capabilities() {
        let connector = ImChannelConnector::default();
        let caps = connector.capabilities();
        assert_eq!(caps.len(), 7);
        assert!(caps.iter().any(|c| c.id == "channel.message.receive"));
        assert!(caps.iter().any(|c| c.id == "channel.message.send"));
        assert!(caps.iter().any(|c| c.id == "channel.card.send"));
        assert!(caps.iter().any(|c| c.id == "channel.voice.receive"));
        assert!(caps.iter().any(|c| c.id == "channel.voice.send"));
        assert!(caps.iter().any(|c| c.id == "channel.session.bind"));
        assert!(caps.iter().any(|c| c.id == "channel.session.status"));
    }

    #[test]
    fn test_im_channel_runtime() {
        let connector = ImChannelConnector::default();
        assert!(matches!(connector.runtime(), ConnectorRuntime::Native));
    }

    #[tokio::test]
    async fn test_dedup_messages() {
        let connector = ImChannelConnector::default();
        assert!(!connector.is_duplicate("msg-1").await);
        assert!(connector.is_duplicate("msg-1").await);
        assert!(!connector.is_duplicate("msg-2").await);
    }

    #[tokio::test]
    async fn test_session_binding() {
        let connector = ImChannelConnector::default();
        let now = chrono::Utc::now();
        let binding = SessionBinding {
            chat_id: "chat-1".to_string(),
            platform: "mock".to_string(),
            session_id: "sess-1".to_string(),
            execution_id: None,
            external_session_id: None,
            reply_mode: ReplyMode::FollowUser,
            created_at: now,
            last_activity: now,
        };
        connector.bind_session(binding).await.unwrap();
        let found = connector.lookup_binding("chat-1").await;
        assert!(found.is_some());
        assert_eq!(found.unwrap().session_id, "sess-1");
    }

    #[tokio::test]
    async fn test_session_expiry() {
        let config = ImChannelConfig {
            session_timeout_secs: 0, // 立即过期
            ..Default::default()
        };
        let connector = ImChannelConnector::new(config);
        let past = chrono::Utc::now() - chrono::Duration::seconds(10);
        let binding = SessionBinding {
            chat_id: "chat-exp".to_string(),
            platform: "mock".to_string(),
            session_id: "sess-exp".to_string(),
            execution_id: None,
            external_session_id: None,
            reply_mode: ReplyMode::TextOnly,
            created_at: past,
            last_activity: past,
        };
        connector.bind_session(binding).await.unwrap();
        let found = connector.lookup_binding("chat-exp").await;
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_register_and_normalize() {
        let connector = ImChannelConnector::default();
        connector.register_adapter(Arc::new(MockImAdapter)).await;
        let adapter = connector.get_adapter("mock").await.unwrap();
        let msg = adapter
            .normalize_inbound(serde_json::json!({"id": "m1", "text": "hello"}))
            .await
            .unwrap();
        assert_eq!(msg.id, "m1");
        assert_eq!(msg.text_content.as_deref(), Some("hello"));
        assert_eq!(msg.message_type, ImMessageType::Text);
    }
}
