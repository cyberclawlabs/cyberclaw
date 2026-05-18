//! # Slack Connector
//!
//! 提供 Slack 集成能力，包括消息发送、频道管理、文件上传等功能。
//!
//! ## 支持的 Capabilities
//!
//! - `slack.send_message`: 发送消息到频道
//! - `slack.create_channel`: 创建新频道
//! - `slack.upload_file`: 上传文件到频道
//! - `slack.react_emoji`: 添加 emoji 反应到消息
//!
//! ## 消息模板系统
//!
//! 使用 Handlebars 模板引擎支持动态消息渲染。

use crate::types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, Connector, ExecutionStatus,
};
use cyberclaw_core::prelude::*;
use handlebars::Handlebars;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Slack Connector 实现
#[derive(Debug)]
pub struct SlackConnector {
    id: ConnectorId,
    client: SlackClient,
    templates: Arc<RwLock<MessageTemplates>>,
}

/// Slack 客户端封装
#[derive(Debug, Clone)]
struct SlackClient {
    bot_token: String,
    http_client: reqwest::Client,
    api_base: String,
}

/// 消息模板管理器
#[derive(Debug)]
struct MessageTemplates {
    registry: Handlebars<'static>,
    templates: HashMap<String, String>,
}

/// slack.send_message 输入参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageInput {
    /// 目标频道 ID 或名称
    pub channel: String,
    /// 消息文本
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// 使用模板（模板名称）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// 模板数据
    #[serde(default)]
    pub data: serde_json::Value,
    /// 消息块（高级格式）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocks: Option<serde_json::Value>,
    /// 线程 TS（回复消息）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_ts: Option<String>,
}

/// slack.send_message 输出结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageOutput {
    /// 消息时间戳
    pub ts: String,
    /// 频道 ID
    pub channel: String,
    /// 是否成功
    pub ok: bool,
}

/// slack.create_channel 输入参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateChannelInput {
    /// 频道名称
    pub name: String,
    /// 是否为私有频道
    #[serde(default)]
    pub is_private: bool,
    /// 频道描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// slack.create_channel 输出结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateChannelOutput {
    /// 频道 ID
    pub id: String,
    /// 频道名称
    pub name: String,
    /// 是否成功
    pub ok: bool,
}

/// slack.upload_file 输入参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadFileInput {
    /// 目标频道 ID 或名称
    pub channel: String,
    /// 文件内容（Base64 编码）
    pub content: String,
    /// 文件名
    pub filename: String,
    /// 文件类型（MIME type）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filetype: Option<String>,
    /// 初始评论
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_comment: Option<String>,
}

/// slack.upload_file 输出结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadFileOutput {
    /// 文件 ID
    pub file_id: String,
    /// 文件 URL
    pub url: String,
    /// 是否成功
    pub ok: bool,
}

/// slack.react_emoji 输入参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactEmojiInput {
    /// 目标频道 ID
    pub channel: String,
    /// 消息时间戳
    pub timestamp: String,
    /// Emoji 名称（不含冒号）
    pub emoji: String,
}

/// slack.react_emoji 输出结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactEmojiOutput {
    /// 是否成功
    pub ok: bool,
}

impl SlackConnector {
    /// 创建新的 Slack Connector
    pub fn new(bot_token: String) -> Self {
        let http_client_builder =
            reqwest::Client::builder().timeout(std::time::Duration::from_secs(30));
        #[cfg(test)]
        let http_client_builder = http_client_builder.no_proxy();
        let http_client = http_client_builder
            .build()
            .expect("Failed to create HTTP client");

        let client = SlackClient {
            bot_token,
            http_client,
            api_base: "https://slack.com/api".to_string(),
        };

        let templates = Arc::new(RwLock::new(MessageTemplates::new()));

        Self {
            id: ConnectorId::from_string("slack-connector".to_string())
                .expect("Failed to create ConnectorId"),
            client,
            templates,
        }
    }

    /// 注册消息模板
    pub async fn register_template(&self, name: String, template: String) -> anyhow::Result<()> {
        self.templates.write().await.register(name, template)
    }

    /// 执行 slack.send_message
    async fn send_message(&self, input: SendMessageInput) -> anyhow::Result<SendMessageOutput> {
        // 解析消息文本
        let text = if let Some(template_name) = &input.template {
            // 使用模板渲染
            self.templates
                .read()
                .await
                .render(template_name, &input.data)?
        } else if let Some(text) = &input.text {
            text.clone()
        } else {
            return Err(anyhow::anyhow!(
                "Either 'text' or 'template' must be provided"
            ));
        };

        // 构建请求体
        let mut body = serde_json::json!({
            "channel": input.channel,
            "text": text,
        });

        if let Some(blocks) = input.blocks {
            body["blocks"] = blocks;
        }

        if let Some(thread_ts) = input.thread_ts {
            body["thread_ts"] = serde_json::Value::String(thread_ts);
        }

        // 调用 Slack API
        let response = self.client.post("chat.postMessage", body).await?;

        Ok(SendMessageOutput {
            ts: response["ts"].as_str().unwrap_or_default().to_string(),
            channel: response["channel"].as_str().unwrap_or_default().to_string(),
            ok: response["ok"].as_bool().unwrap_or(false),
        })
    }

    /// 执行 slack.create_channel
    async fn create_channel(
        &self,
        input: CreateChannelInput,
    ) -> anyhow::Result<CreateChannelOutput> {
        let method = "conversations.create";

        let mut body = serde_json::json!({
            "name": input.name,
            "is_private": input.is_private,
        });

        if let Some(description) = input.description {
            body["description"] = serde_json::Value::String(description);
        }

        let response = self.client.post(method, body).await?;

        let channel = &response["channel"];
        Ok(CreateChannelOutput {
            id: channel["id"].as_str().unwrap_or_default().to_string(),
            name: channel["name"].as_str().unwrap_or_default().to_string(),
            ok: response["ok"].as_bool().unwrap_or(false),
        })
    }

    /// 执行 slack.upload_file
    async fn upload_file(&self, input: UploadFileInput) -> anyhow::Result<UploadFileOutput> {
        // 解码 Base64 内容
        use base64::Engine;
        let content = base64::engine::general_purpose::STANDARD
            .decode(&input.content)
            .map_err(|e| anyhow::anyhow!("Failed to decode Base64 content: {}", e))?;

        // 使用 files.upload API
        let mut body = serde_json::json!({
            "channels": input.channel,
            "filename": input.filename,
            "content": String::from_utf8_lossy(&content),
        });

        if let Some(filetype) = input.filetype {
            body["filetype"] = serde_json::Value::String(filetype);
        }

        if let Some(comment) = input.initial_comment {
            body["initial_comment"] = serde_json::Value::String(comment);
        }

        let response = self.client.post("files.upload", body).await?;

        let file = &response["file"];
        Ok(UploadFileOutput {
            file_id: file["id"].as_str().unwrap_or_default().to_string(),
            url: file["permalink"].as_str().unwrap_or_default().to_string(),
            ok: response["ok"].as_bool().unwrap_or(false),
        })
    }

    /// 执行 slack.react_emoji
    async fn react_emoji(&self, input: ReactEmojiInput) -> anyhow::Result<ReactEmojiOutput> {
        let body = serde_json::json!({
            "channel": input.channel,
            "timestamp": input.timestamp,
            "name": input.emoji,
        });

        let response = self.client.post("reactions.add", body).await?;

        Ok(ReactEmojiOutput {
            ok: response["ok"].as_bool().unwrap_or(false),
        })
    }
}

#[async_trait::async_trait]
impl Connector for SlackConnector {
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
                        id: "slack.send_message".to_string(),
                        title: "Send Slack Message".to_string(),
                        description: Some("Send a message to a Slack channel".to_string()),
                        risk: RiskLevel::Low,
                        effects: vec![CapabilityEffect::Notification],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "channel": {"type": "string"},
                                "text": {"type": "string"},
                                "template": {"type": "string"},
                                "data": {"type": "object"},
                                "blocks": {"type": "array"},
                                "thread_ts": {"type": "string"}
                            },
                            "required": ["channel"],
                            "oneOf": [
                                {"required": ["text"]},
                                {"required": ["template"]}
                            ]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "ts": {"type": "string"},
                                "channel": {"type": "string"},
                                "ok": {"type": "boolean"}
                            }
                        }))
                        .unwrap(),
                    },
                    CapabilityContract {
                        id: "slack.create_channel".to_string(),
                        title: "Create Slack Channel".to_string(),
                        description: Some("Create a new Slack channel".to_string()),
                        risk: RiskLevel::High,
                        effects: vec![CapabilityEffect::Write],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "name": {"type": "string"},
                                "is_private": {"type": "boolean"},
                                "description": {"type": "string"}
                            },
                            "required": ["name"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "id": {"type": "string"},
                                "name": {"type": "string"},
                                "ok": {"type": "boolean"}
                            }
                        }))
                        .unwrap(),
                    },
                    CapabilityContract {
                        id: "slack.upload_file".to_string(),
                        title: "Upload File to Slack".to_string(),
                        description: Some("Upload a file to a Slack channel".to_string()),
                        risk: RiskLevel::Medium,
                        effects: vec![CapabilityEffect::Write, CapabilityEffect::Network],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "channel": {"type": "string"},
                                "content": {"type": "string", "format": "base64"},
                                "filename": {"type": "string"},
                                "filetype": {"type": "string"},
                                "initial_comment": {"type": "string"}
                            },
                            "required": ["channel", "content", "filename"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "file_id": {"type": "string"},
                                "url": {"type": "string"},
                                "ok": {"type": "boolean"}
                            }
                        }))
                        .unwrap(),
                    },
                    CapabilityContract {
                        id: "slack.react_emoji".to_string(),
                        title: "React with Emoji".to_string(),
                        description: Some("Add an emoji reaction to a message".to_string()),
                        risk: RiskLevel::Low,
                        effects: vec![CapabilityEffect::Write],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "channel": {"type": "string"},
                                "timestamp": {"type": "string"},
                                "emoji": {"type": "string"}
                            },
                            "required": ["channel", "timestamp", "emoji"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "ok": {"type": "boolean"}
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
            "slack.send_message" => {
                let input: SendMessageInput = serde_json::from_value(request.input.clone())?;
                match self.send_message(input).await {
                    Ok(output) => (
                        serde_json::to_value(output)?,
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
            "slack.create_channel" => {
                let input: CreateChannelInput = serde_json::from_value(request.input.clone())?;
                match self.create_channel(input).await {
                    Ok(output) => (
                        serde_json::to_value(output)?,
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
            "slack.upload_file" => {
                let input: UploadFileInput = serde_json::from_value(request.input.clone())?;
                match self.upload_file(input).await {
                    Ok(output) => (
                        serde_json::to_value(output)?,
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
            "slack.react_emoji" => {
                let input: ReactEmojiInput = serde_json::from_value(request.input.clone())?;
                match self.react_emoji(input).await {
                    Ok(output) => (
                        serde_json::to_value(output)?,
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
            _ => {
                return Err(anyhow::anyhow!("Unknown capability: {}", capability_name));
            }
        };

        Ok(CapabilityExecutionResult {
            execution_id: request.execution_id,
            trace_id: request.trace_id,
            connector_id: request.connector_id,
            capability_id: request.capability_id,
            output,
            status,
            error,
            actual_runtime: Some(crate::runtime::RuntimeMode::Native),
        })
    }
}

impl SlackClient {
    /// 调用 Slack API
    async fn post(
        &self,
        method: &str,
        body: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let url = format!("{}/{}", self.api_base, method);

        let response = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let body = response.json::<serde_json::Value>().await?;

        if !status.is_success() || !body["ok"].as_bool().unwrap_or(false) {
            let error = body["error"].as_str().unwrap_or("Unknown error");
            return Err(anyhow::anyhow!("Slack API error: {}", error));
        }

        Ok(body)
    }
}

impl MessageTemplates {
    /// 创建新的模板管理器
    fn new() -> Self {
        Self {
            registry: Handlebars::new(),
            templates: HashMap::new(),
        }
    }

    /// 注册模板
    fn register(&mut self, name: String, template: String) -> anyhow::Result<()> {
        self.registry.register_template_string(&name, &template)?;
        self.templates.insert(name, template);
        Ok(())
    }

    /// 渲染模板
    fn render(&self, name: &str, data: &serde_json::Value) -> anyhow::Result<String> {
        self.registry
            .render(name, data)
            .map_err(|e| anyhow::anyhow!("Template render error: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_slack_connector_creation() {
        let connector = SlackConnector::new("xoxb-test-token".to_string());
        assert_eq!(connector.id().as_str(), "slack-connector");
        assert_eq!(connector.capabilities().len(), 4);
    }

    #[tokio::test]
    async fn test_template_registration() {
        let connector = SlackConnector::new("xoxb-test-token".to_string());
        let result = connector
            .register_template(
                "welcome".to_string(),
                "Welcome {{name}} to {{channel}}!".to_string(),
            )
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_template_rendering() {
        let mut templates = MessageTemplates::new();
        templates
            .register("welcome".to_string(), "Hello {{name}}!".to_string())
            .unwrap();

        let data = serde_json::json!({"name": "Alice"});
        let result = templates.render("welcome", &data).unwrap();
        assert_eq!(result, "Hello Alice!");
    }

    #[test]
    fn test_send_message_input_serialization() {
        let input = SendMessageInput {
            channel: "#general".to_string(),
            text: Some("Hello".to_string()),
            template: None,
            data: serde_json::json!({}),
            blocks: None,
            thread_ts: None,
        };

        let json = serde_json::to_string(&input).unwrap();
        let deserialized: SendMessageInput = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.channel, "#general");
    }

    #[test]
    fn test_capability_contracts() {
        let connector = SlackConnector::new("xoxb-test-token".to_string());
        let caps = connector.capabilities();

        assert_eq!(caps[0].id.as_str(), "slack.send_message");
        assert_eq!(caps[0].risk, RiskLevel::Low);

        assert_eq!(caps[1].id.as_str(), "slack.create_channel");
        assert_eq!(caps[1].risk, RiskLevel::High);

        assert_eq!(caps[2].id.as_str(), "slack.upload_file");
        assert_eq!(caps[2].risk, RiskLevel::Medium);

        assert_eq!(caps[3].id.as_str(), "slack.react_emoji");
        assert_eq!(caps[3].risk, RiskLevel::Low);
    }
}
