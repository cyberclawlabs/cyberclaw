//! # Message Gateway Connector
//!
//! Webhook 网关连接器，处理来自外部平台（Slack、Discord、GitHub 等）的消息接收和发送。
//! 将外部平台消息标准化为 CyberClaw 统一格式。
//!
//! ## 支持的 Capabilities
//!
//! - `gateway.receive`: 接收并标准化入站消息
//! - `gateway.send`: 格式化并发送出站消息

use crate::types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, Connector, ExecutionStatus,
};
use cyberclaw_core::prelude::*;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tokio::sync::RwLock;

type HmacSha256 = Hmac<Sha256>;

// ---------------------------------------------------------------------------
// NormalizedMessage
// ---------------------------------------------------------------------------

/// 平台无关的标准化消息格式
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedMessage {
    /// 消息唯一 ID
    pub id: String,
    /// 来源平台标识
    pub platform: String,
    /// 发送者标识
    pub sender: String,
    /// 频道/会话标识
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// 消息内容
    pub content: String,
    /// 消息时间戳
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// 附加元数据
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// 原始 JSON 载荷
    pub raw: serde_json::Value,
}

// ---------------------------------------------------------------------------
// PlatformAdapter trait
// ---------------------------------------------------------------------------

/// 平台适配器 trait，负责入站消息标准化、出站消息格式化和签名校验
#[async_trait::async_trait]
pub trait PlatformAdapter: Send + Sync {
    /// 返回平台标识
    fn platform_id(&self) -> &str;

    /// 将原始入站 JSON 标准化为 NormalizedMessage
    async fn normalize_inbound(&self, raw: serde_json::Value) -> anyhow::Result<NormalizedMessage>;

    /// 将 NormalizedMessage 格式化为平台出站 JSON
    async fn format_outbound(&self, msg: NormalizedMessage) -> anyhow::Result<serde_json::Value>;

    /// 使用平台密钥校验 webhook 签名
    fn validate_signature(&self, payload: &[u8], signature: &str) -> bool;
}

// ---------------------------------------------------------------------------
// GenericWebhookAdapter
// ---------------------------------------------------------------------------

/// 通用 Webhook 适配器，使用 HMAC-SHA256 校验签名并从常见 JSON 路径提取字段
#[derive(Debug, Clone)]
pub struct GenericWebhookAdapter {
    /// HMAC 密钥
    secret: String,
}

impl GenericWebhookAdapter {
    /// 创建新的通用 Webhook 适配器
    pub fn new(secret: String) -> Self {
        Self { secret }
    }

    /// 从 JSON 中按优先级提取字符串字段
    fn extract_string(value: &serde_json::Value, keys: &[&str]) -> String {
        for key in keys {
            if let Some(s) = value.get(key).and_then(|v| v.as_str()) {
                return s.to_string();
            }
        }
        String::new()
    }
}

#[async_trait::async_trait]
impl PlatformAdapter for GenericWebhookAdapter {
    fn platform_id(&self) -> &str {
        "generic"
    }

    async fn normalize_inbound(&self, raw: serde_json::Value) -> anyhow::Result<NormalizedMessage> {
        let id = Self::extract_string(&raw, &["id", "message_id", "event_id"]);
        let id = if id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            id
        };

        let sender = Self::extract_string(&raw, &["sender", "user", "from", "author"]);
        let content = Self::extract_string(&raw, &["content", "text", "message", "body"]);
        let channel_str = Self::extract_string(&raw, &["channel", "room", "conversation"]);
        let channel = if channel_str.is_empty() {
            None
        } else {
            Some(channel_str)
        };

        Ok(NormalizedMessage {
            id,
            platform: "generic".to_string(),
            sender,
            channel,
            content,
            timestamp: chrono::Utc::now(),
            metadata: HashMap::new(),
            raw,
        })
    }

    async fn format_outbound(&self, msg: NormalizedMessage) -> anyhow::Result<serde_json::Value> {
        Ok(serde_json::json!({
            "id": msg.id,
            "platform": msg.platform,
            "sender": msg.sender,
            "channel": msg.channel,
            "content": msg.content,
            "timestamp": msg.timestamp.to_rfc3339(),
            "metadata": msg.metadata,
        }))
    }

    fn validate_signature(&self, payload: &[u8], signature: &str) -> bool {
        let Ok(mut mac) = HmacSha256::new_from_slice(self.secret.as_bytes()) else {
            return false;
        };
        mac.update(payload);
        let expected = hex::encode(mac.finalize().into_bytes());
        // Constant-time comparison to prevent timing side-channel attacks
        expected.as_bytes().ct_eq(signature.as_bytes()).into()
    }
}

// ---------------------------------------------------------------------------
// MessageRouter
// ---------------------------------------------------------------------------

/// 消息路由器，按平台标识将消息路由到已注册的适配器
#[derive(Default)]
pub struct MessageRouter {
    adapters: HashMap<String, Arc<dyn PlatformAdapter>>,
}

impl std::fmt::Debug for MessageRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MessageRouter")
            .field("platforms", &self.adapters.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl MessageRouter {
    /// 创建空的消息路由器
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册平台适配器
    pub fn register_adapter(&mut self, adapter: Arc<dyn PlatformAdapter>) {
        self.adapters
            .insert(adapter.platform_id().to_string(), adapter);
    }

    /// 路由入站消息到对应平台适配器
    pub async fn route_inbound(
        &self,
        platform: &str,
        raw: serde_json::Value,
    ) -> anyhow::Result<NormalizedMessage> {
        let adapter = self
            .adapters
            .get(platform)
            .ok_or_else(|| anyhow::anyhow!("Unknown platform: {}", platform))?;
        adapter.normalize_inbound(raw).await
    }

    /// 路由出站消息到对应平台适配器
    pub async fn route_outbound(
        &self,
        msg: NormalizedMessage,
    ) -> anyhow::Result<serde_json::Value> {
        let adapter = self
            .adapters
            .get(&msg.platform)
            .ok_or_else(|| anyhow::anyhow!("Unknown platform: {}", msg.platform))?;
        adapter.format_outbound(msg).await
    }
}

// ---------------------------------------------------------------------------
// MessageGatewayConnector
// ---------------------------------------------------------------------------

/// Webhook 网关连接器
#[derive(Debug)]
pub struct MessageGatewayConnector {
    id: ConnectorId,
    router: Arc<RwLock<MessageRouter>>,
    queue: Arc<RwLock<Vec<NormalizedMessage>>>,
}

impl MessageGatewayConnector {
    /// 创建新的 Message Gateway Connector
    pub fn new() -> Self {
        Self {
            id: ConnectorId::from_string("message-gateway".to_string())
                .expect("Failed to create ConnectorId"),
            router: Arc::new(RwLock::new(MessageRouter::new())),
            queue: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// 注册平台适配器
    pub async fn register_adapter(&self, adapter: Arc<dyn PlatformAdapter>) {
        self.router.write().await.register_adapter(adapter);
    }

    /// 接收入站消息，标准化后加入队列
    pub async fn receive(
        &self,
        platform: &str,
        raw: serde_json::Value,
    ) -> anyhow::Result<NormalizedMessage> {
        let msg = self
            .router
            .read()
            .await
            .route_inbound(platform, raw)
            .await?;
        self.queue.write().await.push(msg.clone());
        Ok(msg)
    }

    /// 格式化出站消息
    pub async fn send(&self, msg: NormalizedMessage) -> anyhow::Result<serde_json::Value> {
        self.router.read().await.route_outbound(msg).await
    }

    /// 获取当前队列中的消息数量
    pub async fn queue_len(&self) -> usize {
        self.queue.read().await.len()
    }

    /// 排出所有队列中的消息
    pub async fn drain_queue(&self) -> Vec<NormalizedMessage> {
        let mut q = self.queue.write().await;
        q.drain(..).collect()
    }
}

impl Default for MessageGatewayConnector {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Connector for MessageGatewayConnector {
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
                        id: "gateway.receive".to_string(),
                        title: "Receive Webhook Message".to_string(),
                        description: Some(
                            "Receive and normalize an inbound webhook message".to_string(),
                        ),
                        risk: RiskLevel::Low,
                        effects: vec![CapabilityEffect::Write],
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
                                "id": {"type": "string"},
                                "platform": {"type": "string"},
                                "sender": {"type": "string"},
                                "content": {"type": "string"}
                            }
                        }))
                        .unwrap(),
                    },
                    CapabilityContract {
                        id: "gateway.send".to_string(),
                        title: "Send Webhook Message".to_string(),
                        description: Some(
                            "Format and send an outbound message to a platform".to_string(),
                        ),
                        risk: RiskLevel::Low,
                        effects: vec![CapabilityEffect::Notification],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "message": {"type": "object"}
                            },
                            "required": ["message"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "formatted": {"type": "object"}
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
            "gateway.receive" => {
                #[derive(Deserialize)]
                struct ReceiveInput {
                    platform: String,
                    payload: serde_json::Value,
                }
                let input: ReceiveInput = serde_json::from_value(request.input.clone())?;
                match self.receive(&input.platform, input.payload).await {
                    Ok(msg) => (serde_json::to_value(msg)?, ExecutionStatus::Success, None),
                    Err(e) => (
                        serde_json::Value::Null,
                        ExecutionStatus::Failed,
                        Some(e.to_string()),
                    ),
                }
            }
            "gateway.send" => {
                #[derive(Deserialize)]
                struct SendInput {
                    message: NormalizedMessage,
                }
                let input: SendInput = serde_json::from_value(request.input.clone())?;
                match self.send(input.message).await {
                    Ok(formatted) => (
                        serde_json::json!({ "formatted": formatted }),
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
            actual_runtime: Some(crate::runtime::RuntimeMode::Native),
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_adapter() -> GenericWebhookAdapter {
        GenericWebhookAdapter::new("test-secret".to_string())
    }

    fn sample_inbound() -> serde_json::Value {
        serde_json::json!({
            "id": "msg-001",
            "sender": "alice",
            "content": "Hello world",
            "channel": "general"
        })
    }

    // 1. Generic adapter normalize
    #[tokio::test]
    async fn test_generic_adapter_normalize() {
        let adapter = make_adapter();
        let msg = adapter.normalize_inbound(sample_inbound()).await.unwrap();
        assert_eq!(msg.id, "msg-001");
        assert_eq!(msg.sender, "alice");
        assert_eq!(msg.content, "Hello world");
        assert_eq!(msg.channel.as_deref(), Some("general"));
        assert_eq!(msg.platform, "generic");
    }

    // 2. Generic adapter format outbound
    #[tokio::test]
    async fn test_generic_adapter_format_outbound() {
        let adapter = make_adapter();
        let msg = NormalizedMessage {
            id: "msg-002".to_string(),
            platform: "generic".to_string(),
            sender: "bob".to_string(),
            channel: Some("dev".to_string()),
            content: "Hi there".to_string(),
            timestamp: chrono::Utc::now(),
            metadata: HashMap::new(),
            raw: serde_json::Value::Null,
        };
        let out = adapter.format_outbound(msg).await.unwrap();
        assert_eq!(out["sender"], "bob");
        assert_eq!(out["content"], "Hi there");
        assert_eq!(out["channel"], "dev");
    }

    // 3. Normalize/format roundtrip
    #[tokio::test]
    async fn test_normalize_format_roundtrip() {
        let adapter = make_adapter();
        let raw = sample_inbound();
        let msg = adapter.normalize_inbound(raw).await.unwrap();
        let out = adapter.format_outbound(msg.clone()).await.unwrap();
        assert_eq!(out["sender"], msg.sender);
        assert_eq!(out["content"], msg.content);
    }

    // 4. HMAC signature validation — valid
    #[test]
    fn test_hmac_valid_signature() {
        let adapter = make_adapter();
        let payload = b"test payload";
        let mut mac =
            HmacSha256::new_from_slice(b"test-secret").expect("HMAC accepts any key size");
        mac.update(payload);
        let sig = hex::encode(mac.finalize().into_bytes());
        assert!(adapter.validate_signature(payload, &sig));
    }

    // 5. HMAC signature validation — invalid
    #[test]
    fn test_hmac_invalid_signature() {
        let adapter = make_adapter();
        assert!(!adapter.validate_signature(b"test payload", "invalid-signature"));
    }

    // 6. Router registration and routing
    #[tokio::test]
    async fn test_router_register_and_route() {
        let mut router = MessageRouter::new();
        let adapter: Arc<dyn PlatformAdapter> = Arc::new(make_adapter());
        router.register_adapter(adapter);

        let msg = router
            .route_inbound("generic", sample_inbound())
            .await
            .unwrap();
        assert_eq!(msg.sender, "alice");
    }

    // 7. Router unknown platform error
    #[tokio::test]
    async fn test_router_unknown_platform() {
        let router = MessageRouter::new();
        let result = router
            .route_inbound("nonexistent", serde_json::Value::Null)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown platform"));
    }

    // 8. Router outbound unknown platform error
    #[tokio::test]
    async fn test_router_outbound_unknown_platform() {
        let router = MessageRouter::new();
        let msg = NormalizedMessage {
            id: "x".to_string(),
            platform: "unknown".to_string(),
            sender: "s".to_string(),
            channel: None,
            content: "c".to_string(),
            timestamp: chrono::Utc::now(),
            metadata: HashMap::new(),
            raw: serde_json::Value::Null,
        };
        let result = router.route_outbound(msg).await;
        assert!(result.is_err());
    }

    // 9. NormalizedMessage serialization roundtrip
    #[test]
    fn test_normalized_message_serialization() {
        let msg = NormalizedMessage {
            id: "msg-ser".to_string(),
            platform: "generic".to_string(),
            sender: "charlie".to_string(),
            channel: Some("test-chan".to_string()),
            content: "serialize me".to_string(),
            timestamp: chrono::Utc::now(),
            metadata: {
                let mut m = HashMap::new();
                m.insert("key".to_string(), "value".to_string());
                m
            },
            raw: serde_json::json!({"original": true}),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: NormalizedMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "msg-ser");
        assert_eq!(deserialized.sender, "charlie");
        assert_eq!(deserialized.channel.as_deref(), Some("test-chan"));
        assert_eq!(deserialized.metadata["key"], "value");
    }

    // 10. MessageGatewayConnector receive queues messages
    #[tokio::test]
    async fn test_gateway_connector_receive_and_queue() {
        let connector = MessageGatewayConnector::new();
        let adapter: Arc<dyn PlatformAdapter> = Arc::new(make_adapter());
        connector.register_adapter(adapter).await;

        assert_eq!(connector.queue_len().await, 0);

        let msg = connector
            .receive("generic", sample_inbound())
            .await
            .unwrap();
        assert_eq!(msg.sender, "alice");
        assert_eq!(connector.queue_len().await, 1);

        let drained = connector.drain_queue().await;
        assert_eq!(drained.len(), 1);
        assert_eq!(connector.queue_len().await, 0);
    }

    // 11. MessageGatewayConnector capabilities
    #[test]
    fn test_gateway_connector_capabilities() {
        let connector = MessageGatewayConnector::new();
        let caps = connector.capabilities();
        assert_eq!(caps.len(), 2);
        assert!(caps.iter().any(|c| c.id == "gateway.receive"));
        assert!(caps.iter().any(|c| c.id == "gateway.send"));
    }

    // 12. Normalize with missing optional fields generates UUID id
    #[tokio::test]
    async fn test_normalize_missing_fields() {
        let adapter = make_adapter();
        let raw = serde_json::json!({"text": "bare message"});
        let msg = adapter.normalize_inbound(raw).await.unwrap();
        // id should be a UUID since no id/message_id/event_id in payload
        assert!(!msg.id.is_empty());
        assert_eq!(msg.content, "bare message");
        assert_eq!(msg.sender, "");
        assert!(msg.channel.is_none());
    }
}
