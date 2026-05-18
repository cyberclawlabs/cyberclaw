//! Anthropic Claude 客户端
//!
//! 实现 Anthropic Messages API 的完整客户端，包括：
//! - 请求/响应格式转换（内部类型 <-> Anthropic API 格式）
//! - 错误分类（速率限制、认证、服务器错误）
//! - 超时配置
//! - stop_reason 映射

use crate::client::LlmClient;
use crate::error::{LlmError, LlmResult};
use crate::types::{
    CacheControl, ChatChunk, ChatRequest, ChatResponse, Choice, Message, Role, Usage,
};
use async_trait::async_trait;
use futures::stream::Stream;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::warn;

/// Anthropic API 版本
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// 默认最大 token 数
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// 默认请求超时（秒）
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Anthropic 客户端
pub struct AnthropicClient {
    client: Client,
    api_key: String,
    base_url: String,
}

impl std::fmt::Debug for AnthropicClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicClient")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}

/// Anthropic 请求格式（不同于 OpenAI）
#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    messages: Vec<AnthropicMessage>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    /// P1.1 — 改为内容块数组以承载 cache_control。当某条 system
    /// 内容携带 `cache_control` 时，Anthropic native cache 会被启用。
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Vec<AnthropicSystemBlock>>,
}

/// Anthropic 系统提示块（支持 prompt cache）
#[derive(Debug, Serialize)]
struct AnthropicSystemBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

/// Anthropic 消息
#[derive(Debug, Serialize, Deserialize, Clone)]
struct AnthropicMessage {
    role: String,
    content: String,
}

/// Anthropic 响应
#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    id: String,
    model: String,
    content: Vec<AnthropicContent>,
    stop_reason: Option<String>,
    usage: AnthropicUsage,
}

/// Anthropic 内容块
///
/// 兼容多种 block type:
/// - `text` block: 有 `text` 字段（标准 Anthropic 文本输出）
/// - `thinking` block: Extended Thinking 模式（Claude 4 + MiniMax via Anthropic shim）
///   有 `thinking` + `signature` 字段，无 `text` 字段
/// - `tool_use` block: 工具调用意图（暂不在此 struct 反序列化）
///
/// 2026-05-17: 修复 MiniMax via `api.minimaxi.com/anthropic` shim 返回 thinking
/// block 时 "error decoding response body" — 之前 `text: String` 是必填，遇
/// thinking-only response 必失败。改成 Option<String> + #[serde(default)]，
/// downstream `convert_response` 用 filter_map 跳过无 text 的块。
#[derive(Debug, Deserialize)]
struct AnthropicContent {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    content_type: String,
    #[serde(default)]
    text: Option<String>,
}

/// Anthropic 使用情况
#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
    /// P1.1 — 从 prompt cache 读取的 token 数
    #[serde(default)]
    cache_read_input_tokens: Option<u32>,
    /// P1.1 — 写入 prompt cache 的 token 数
    #[serde(default)]
    cache_creation_input_tokens: Option<u32>,
}

/// Anthropic API 错误响应
#[derive(Debug, Deserialize)]
struct AnthropicErrorResponse {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    error_type: Option<String>,
    error: Option<AnthropicErrorDetail>,
}

/// Anthropic 错误详情
#[derive(Debug, Deserialize)]
struct AnthropicErrorDetail {
    #[serde(rename = "type")]
    error_type: String,
    message: String,
}

impl AnthropicClient {
    /// 创建新的 Anthropic 客户端
    ///
    /// # Arguments
    ///
    /// * `api_key` - Anthropic API 密钥
    /// * `base_url` - API 基础 URL（默认 https://api.anthropic.com）
    pub fn new(api_key: String, base_url: &str) -> LlmResult<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .map_err(|e| LlmError::Internal(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            client,
            api_key,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    /// 从环境变量创建客户端
    ///
    /// 读取 `ANTHROPIC_API_KEY` 和可选的 `ANTHROPIC_BASE_URL`
    pub fn from_env() -> LlmResult<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| LlmError::ConfigError("ANTHROPIC_API_KEY not set".to_string()))?;

        let base_url = std::env::var("ANTHROPIC_BASE_URL")
            .unwrap_or_else(|_| "https://api.anthropic.com".to_string());

        Self::new(api_key, &base_url)
    }

    /// 转换内部请求格式到 Anthropic API 格式
    ///
    /// P1.1 — system 块使用数组格式以承载 cache_control。如果任何
    /// 一个 system 消息带有 `cache_control`，对应的内容块会附上
    /// cache_control 字段，启用 Anthropic native prompt cache。
    /// 多个 system 消息以独立 content block 形式保留，便于精细
    /// 控制 cache 边界。
    fn convert_request(&self, request: &ChatRequest) -> AnthropicRequest {
        let mut system_blocks: Vec<AnthropicSystemBlock> = Vec::new();
        let mut messages = Vec::new();

        for msg in &request.messages {
            match msg.role {
                Role::System => {
                    system_blocks.push(AnthropicSystemBlock {
                        block_type: "text".to_string(),
                        text: msg.content.clone(),
                        cache_control: msg.cache_control.clone(),
                    });
                }
                Role::User => {
                    messages.push(AnthropicMessage {
                        role: "user".to_string(),
                        content: msg.content.clone(),
                    });
                }
                Role::Assistant => {
                    messages.push(AnthropicMessage {
                        role: "assistant".to_string(),
                        content: msg.content.clone(),
                    });
                }
                Role::Tool => {
                    // Anthropic 工具结果转换为 user 消息
                    messages.push(AnthropicMessage {
                        role: "user".to_string(),
                        content: format!("Tool result: {}", msg.content),
                    });
                }
            }
        }

        let system = if system_blocks.is_empty() {
            None
        } else {
            Some(system_blocks)
        };

        AnthropicRequest {
            model: request.model.clone(),
            messages,
            max_tokens: request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            temperature: request.temperature,
            top_p: request.top_p,
            system,
        }
    }

    /// 转换 Anthropic 响应到内部格式
    fn convert_response(&self, resp: AnthropicResponse) -> ChatResponse {
        // filter_map skips thinking blocks (no text field) — see AnthropicContent doc.
        let content = resp
            .content
            .into_iter()
            .filter_map(|c| c.text)
            .collect::<Vec<_>>()
            .join("\n");

        let finish_reason = resp.stop_reason.map(|r| match r.as_str() {
            "end_turn" => "stop".to_string(),
            "max_tokens" => "length".to_string(),
            "stop_sequence" => "stop".to_string(),
            other => other.to_string(),
        });

        ChatResponse {
            id: resp.id,
            object: "chat.completion".to_string(),
            created: chrono::Utc::now().timestamp(),
            model: resp.model,
            choices: vec![Choice {
                index: 0,
                message: Message::assistant(content),
                finish_reason,
            }],
            usage: Some(Usage {
                prompt_tokens: resp.usage.input_tokens,
                completion_tokens: resp.usage.output_tokens,
                total_tokens: resp.usage.input_tokens + resp.usage.output_tokens,
                cache_read_input_tokens: resp.usage.cache_read_input_tokens,
                cache_creation_input_tokens: resp.usage.cache_creation_input_tokens,
            }),
        }
    }

    /// 解析 API 错误响应，提取结构化错误信息
    fn parse_error_response(status: u16, body: &str) -> LlmError {
        // 尝试解析 Anthropic 结构化错误
        if let Ok(err_resp) = serde_json::from_str::<AnthropicErrorResponse>(body) {
            if let Some(detail) = err_resp.error {
                let message = format!("{}: {}", detail.error_type, detail.message);
                return match status {
                    429 => {
                        warn!(error_type = %detail.error_type, "Anthropic rate limit hit");
                        LlmError::ApiError { status, message }
                    }
                    401 | 403 => LlmError::ApiError {
                        status,
                        message: format!("Authentication failed: {}", detail.message),
                    },
                    408 => LlmError::Timeout,
                    _ => LlmError::ApiError { status, message },
                };
            }
        }

        // 无法解析结构化错误，截断原始 body 以避免泄漏敏感数据
        match status {
            408 => LlmError::Timeout,
            _ => {
                let truncated = if body.len() > 500 {
                    format!("{}... [truncated]", &body[..500])
                } else {
                    body.to_string()
                };
                LlmError::ApiError {
                    status,
                    message: truncated,
                }
            }
        }
    }
}

#[async_trait]
impl LlmClient for AnthropicClient {
    async fn chat_completion(&self, request: ChatRequest) -> LlmResult<ChatResponse> {
        let url = format!("{}/v1/messages", self.base_url);
        let anthropic_req = self.convert_request(&request);

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&anthropic_req)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    LlmError::Timeout
                } else {
                    LlmError::HttpError(e)
                }
            })?;

        let status = response.status();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            return Err(Self::parse_error_response(status.as_u16(), &error_body));
        }

        let anthropic_resp: AnthropicResponse = response.json().await?;
        Ok(self.convert_response(anthropic_resp))
    }

    async fn chat_completion_stream(
        &self,
        _request: ChatRequest,
    ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>> {
        // 流式响应需要 SSE 解析，暂不支持
        Err(LlmError::Internal(
            "Streaming not yet implemented for Anthropic".to_string(),
        ))
    }

    fn provider(&self) -> &str {
        "anthropic"
    }

    async fn validate_connection(&self) -> LlmResult<()> {
        let request = ChatRequest {
            model: "claude-3-haiku-20240307".to_string(),
            messages: vec![Message::user("test")],
            max_tokens: Some(1),
            ..Default::default()
        };

        let url = format!("{}/v1/messages", self.base_url);
        let anthropic_req = self.convert_request(&request);

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&anthropic_req)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    LlmError::Timeout
                } else {
                    LlmError::HttpError(e)
                }
            })?;

        let status = response.status();
        if status == 401 || status == 403 {
            let error_body = response.text().await.unwrap_or_default();
            return Err(Self::parse_error_response(status.as_u16(), &error_body));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // 客户端创建
    // -----------------------------------------------------------------------

    #[test]
    fn test_client_creation() {
        let client = AnthropicClient::new("sk-ant-test".to_string(), "https://api.anthropic.com");
        assert!(client.is_ok());
    }

    #[test]
    fn test_client_creation_trims_trailing_slash() {
        let client =
            AnthropicClient::new("sk-ant-test".to_string(), "https://api.anthropic.com/").unwrap();
        assert_eq!(client.base_url, "https://api.anthropic.com");
    }

    #[test]
    fn test_provider_name() {
        let client =
            AnthropicClient::new("sk-ant-test".to_string(), "https://api.anthropic.com").unwrap();
        assert_eq!(client.provider(), "anthropic");
    }

    // -----------------------------------------------------------------------
    // 请求转换
    // -----------------------------------------------------------------------

    #[test]
    fn test_convert_request_basic() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();

        let request = ChatRequest {
            model: "claude-3-opus-20240229".to_string(),
            messages: vec![Message::user("Hello")],
            max_tokens: Some(1024),
            temperature: Some(0.7),
            ..Default::default()
        };

        let converted = client.convert_request(&request);
        assert_eq!(converted.model, "claude-3-opus-20240229");
        assert_eq!(converted.max_tokens, 1024);
        assert_eq!(converted.temperature, Some(0.7));
        assert!(converted.system.is_none());
        assert_eq!(converted.messages.len(), 1);
        assert_eq!(converted.messages[0].role, "user");
        assert_eq!(converted.messages[0].content, "Hello");
    }

    #[test]
    fn test_convert_request_with_system_message() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();

        let request = ChatRequest {
            model: "claude-3-sonnet-20240229".to_string(),
            messages: vec![
                Message::system("You are a helpful assistant."),
                Message::user("Hello"),
            ],
            ..Default::default()
        };

        let converted = client.convert_request(&request);
        let system_blocks = converted.system.as_ref().expect("system blocks present");
        assert_eq!(system_blocks.len(), 1);
        assert_eq!(system_blocks[0].block_type, "text");
        assert_eq!(system_blocks[0].text, "You are a helpful assistant.");
        assert!(system_blocks[0].cache_control.is_none());
        // system 消息不应出现在 messages 中
        assert_eq!(converted.messages.len(), 1);
        assert_eq!(converted.messages[0].role, "user");
    }

    #[test]
    fn test_convert_request_multiple_system_messages_as_blocks() {
        // P1.1 — Anthropic system 现在是 content-block 数组，多个
        // system 消息以独立 block 形式保留（旧版本是用 "\n" 合并）。
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();

        let request = ChatRequest {
            model: "claude-3-sonnet-20240229".to_string(),
            messages: vec![
                Message::system("Rule 1"),
                Message::system("Rule 2"),
                Message::user("Hello"),
            ],
            ..Default::default()
        };

        let converted = client.convert_request(&request);
        let system_blocks = converted.system.as_ref().expect("system blocks present");
        assert_eq!(system_blocks.len(), 2);
        assert_eq!(system_blocks[0].text, "Rule 1");
        assert_eq!(system_blocks[1].text, "Rule 2");
        assert_eq!(converted.messages.len(), 1);
    }

    #[test]
    fn test_convert_request_default_max_tokens() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();

        let request = ChatRequest {
            model: "claude-3-haiku-20240307".to_string(),
            messages: vec![Message::user("Hi")],
            max_tokens: None,
            ..Default::default()
        };

        let converted = client.convert_request(&request);
        assert_eq!(converted.max_tokens, DEFAULT_MAX_TOKENS);
    }

    #[test]
    fn test_convert_request_with_top_p() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();

        let request = ChatRequest {
            model: "claude-3-haiku-20240307".to_string(),
            messages: vec![Message::user("Hi")],
            top_p: Some(0.9),
            ..Default::default()
        };

        let converted = client.convert_request(&request);
        assert_eq!(converted.top_p, Some(0.9));
    }

    #[test]
    fn test_convert_request_tool_message_becomes_user() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();

        let request = ChatRequest {
            model: "claude-3-sonnet-20240229".to_string(),
            messages: vec![
                Message::user("Call the tool"),
                Message::tool("call-1".to_string(), r#"{"result": 42}"#),
            ],
            ..Default::default()
        };

        let converted = client.convert_request(&request);
        assert_eq!(converted.messages.len(), 2);
        assert_eq!(converted.messages[1].role, "user");
        assert!(converted.messages[1].content.contains("Tool result:"));
    }

    #[test]
    fn test_convert_request_multi_turn() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();

        let request = ChatRequest {
            model: "claude-3-sonnet-20240229".to_string(),
            messages: vec![
                Message::system("Be concise"),
                Message::user("Hi"),
                Message::assistant("Hello!"),
                Message::user("How are you?"),
            ],
            ..Default::default()
        };

        let converted = client.convert_request(&request);
        let system_blocks = converted.system.as_ref().expect("system blocks present");
        assert_eq!(system_blocks.len(), 1);
        assert_eq!(system_blocks[0].text, "Be concise");
        assert_eq!(converted.messages.len(), 3);
        assert_eq!(converted.messages[0].role, "user");
        assert_eq!(converted.messages[1].role, "assistant");
        assert_eq!(converted.messages[2].role, "user");
    }

    // -----------------------------------------------------------------------
    // 请求序列化
    // -----------------------------------------------------------------------

    #[test]
    fn test_request_serialization() {
        let req = AnthropicRequest {
            model: "claude-3-opus-20240229".to_string(),
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: "Hello".to_string(),
            }],
            max_tokens: 1024,
            temperature: Some(0.5),
            top_p: None,
            system: Some(vec![AnthropicSystemBlock {
                block_type: "text".to_string(),
                text: "Be helpful".to_string(),
                cache_control: None,
            }]),
        };

        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "claude-3-opus-20240229");
        assert_eq!(json["max_tokens"], 1024);
        assert_eq!(json["temperature"], 0.5);
        // P1.1 — system 现在序列化为内容块数组
        assert_eq!(json["system"][0]["type"], "text");
        assert_eq!(json["system"][0]["text"], "Be helpful");
        assert!(json["system"][0].get("cache_control").is_none());
        assert!(json.get("top_p").is_none()); // skip_serializing_if
        assert_eq!(json["messages"][0]["role"], "user");
        assert_eq!(json["messages"][0]["content"], "Hello");
    }

    #[test]
    fn test_request_serialization_with_cache_control() {
        // P1.1 — 验证 cache_control 透传到 Anthropic 请求体
        let req = AnthropicRequest {
            model: "claude-3-5-sonnet-20241022".to_string(),
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: "Hi".to_string(),
            }],
            max_tokens: 1024,
            temperature: None,
            top_p: None,
            system: Some(vec![AnthropicSystemBlock {
                block_type: "text".to_string(),
                text: "long system prompt".to_string(),
                cache_control: Some(CacheControl::ephemeral()),
            }]),
        };

        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["system"][0]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn test_request_serialization_no_optional_fields() {
        let req = AnthropicRequest {
            model: "claude-3-haiku-20240307".to_string(),
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: "Hi".to_string(),
            }],
            max_tokens: 100,
            temperature: None,
            top_p: None,
            system: None,
        };

        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("temperature").is_none());
        assert!(json.get("system").is_none());
        assert!(json.get("top_p").is_none());
    }

    // -----------------------------------------------------------------------
    // 响应反序列化与转换
    // -----------------------------------------------------------------------

    #[test]
    fn test_response_deserialization() {
        let json = r#"{
            "id": "msg_01XFDUDYJgAACzvnptvVoYEL",
            "type": "message",
            "role": "assistant",
            "content": [
                {
                    "type": "text",
                    "text": "Hello! How can I help you?"
                }
            ],
            "model": "claude-3-opus-20240229",
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 25
            }
        }"#;

        let resp: AnthropicResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, "msg_01XFDUDYJgAACzvnptvVoYEL");
        assert_eq!(resp.model, "claude-3-opus-20240229");
        assert_eq!(resp.content.len(), 1);
        assert_eq!(resp.content[0].text.as_deref(), Some("Hello! How can I help you?"));
        assert_eq!(resp.stop_reason, Some("end_turn".to_string()));
        assert_eq!(resp.usage.input_tokens, 10);
        assert_eq!(resp.usage.output_tokens, 25);
    }

    #[test]
    fn test_convert_response_end_turn() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();

        let resp = AnthropicResponse {
            id: "msg_123".to_string(),
            model: "claude-3-opus-20240229".to_string(),
            content: vec![AnthropicContent {
                content_type: "text".to_string(),
                text: Some("Hello!".to_string()),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: AnthropicUsage {
                input_tokens: 5,
                output_tokens: 10,
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            },
        };

        let converted = client.convert_response(resp);
        assert_eq!(converted.id, "msg_123");
        assert_eq!(converted.object, "chat.completion");
        assert_eq!(converted.model, "claude-3-opus-20240229");
        assert_eq!(converted.choices.len(), 1);
        assert_eq!(converted.choices[0].message.content, "Hello!");
        assert_eq!(converted.choices[0].message.role, Role::Assistant);
        assert_eq!(converted.choices[0].finish_reason, Some("stop".to_string()));

        let usage = converted.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 5);
        assert_eq!(usage.completion_tokens, 10);
        assert_eq!(usage.total_tokens, 15);
    }

    #[test]
    fn test_convert_response_max_tokens() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();

        let resp = AnthropicResponse {
            id: "msg_456".to_string(),
            model: "claude-3-sonnet-20240229".to_string(),
            content: vec![AnthropicContent {
                content_type: "text".to_string(),
                text: Some("Truncated output...".to_string()),
            }],
            stop_reason: Some("max_tokens".to_string()),
            usage: AnthropicUsage {
                input_tokens: 100,
                output_tokens: 4096,
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            },
        };

        let converted = client.convert_response(resp);
        assert_eq!(
            converted.choices[0].finish_reason,
            Some("length".to_string())
        );
    }

    #[test]
    fn test_convert_response_stop_sequence() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();

        let resp = AnthropicResponse {
            id: "msg_789".to_string(),
            model: "claude-3-haiku-20240307".to_string(),
            content: vec![AnthropicContent {
                content_type: "text".to_string(),
                text: Some("Output".to_string()),
            }],
            stop_reason: Some("stop_sequence".to_string()),
            usage: AnthropicUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            },
        };

        let converted = client.convert_response(resp);
        assert_eq!(converted.choices[0].finish_reason, Some("stop".to_string()));
    }

    #[test]
    fn test_convert_response_multiple_content_blocks() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();

        let resp = AnthropicResponse {
            id: "msg_multi".to_string(),
            model: "claude-3-opus-20240229".to_string(),
            content: vec![
                AnthropicContent {
                    content_type: "text".to_string(),
                    text: Some("First block".to_string()),
                },
                AnthropicContent {
                    content_type: "text".to_string(),
                    text: Some("Second block".to_string()),
                },
            ],
            stop_reason: Some("end_turn".to_string()),
            usage: AnthropicUsage {
                input_tokens: 10,
                output_tokens: 20,
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            },
        };

        let converted = client.convert_response(resp);
        assert_eq!(
            converted.choices[0].message.content,
            "First block\nSecond block"
        );
    }

    #[test]
    fn test_convert_response_no_stop_reason() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();

        let resp = AnthropicResponse {
            id: "msg_no_stop".to_string(),
            model: "claude-3-haiku-20240307".to_string(),
            content: vec![AnthropicContent {
                content_type: "text".to_string(),
                text: Some("Hello".to_string()),
            }],
            stop_reason: None,
            usage: AnthropicUsage {
                input_tokens: 1,
                output_tokens: 1,
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            },
        };

        let converted = client.convert_response(resp);
        assert_eq!(converted.choices[0].finish_reason, None);
    }

    // -----------------------------------------------------------------------
    // 错误解析
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_error_response_structured_rate_limit() {
        let body = r#"{
            "type": "error",
            "error": {
                "type": "rate_limit_error",
                "message": "Number of request tokens has exceeded your daily rate limit"
            }
        }"#;

        let err = AnthropicClient::parse_error_response(429, body);
        match err {
            LlmError::ApiError { status, message } => {
                assert_eq!(status, 429);
                assert!(message.contains("rate_limit_error"));
            }
            _ => panic!("Expected ApiError, got {:?}", err),
        }
    }

    #[test]
    fn test_parse_error_response_auth_failure() {
        let body = r#"{
            "type": "error",
            "error": {
                "type": "authentication_error",
                "message": "invalid x-api-key"
            }
        }"#;

        let err = AnthropicClient::parse_error_response(401, body);
        match err {
            LlmError::ApiError { status, message } => {
                assert_eq!(status, 401);
                assert!(message.contains("Authentication failed"));
                assert!(message.contains("invalid x-api-key"));
            }
            _ => panic!("Expected ApiError, got {:?}", err),
        }
    }

    #[test]
    fn test_parse_error_response_forbidden() {
        let body = r#"{
            "type": "error",
            "error": {
                "type": "permission_error",
                "message": "Your API key does not have permission"
            }
        }"#;

        let err = AnthropicClient::parse_error_response(403, body);
        match err {
            LlmError::ApiError { status, message } => {
                assert_eq!(status, 403);
                assert!(message.contains("Authentication failed"));
            }
            _ => panic!("Expected ApiError, got {:?}", err),
        }
    }

    #[test]
    fn test_parse_error_response_timeout() {
        let body =
            r#"{"type": "error", "error": {"type": "timeout", "message": "Request timed out"}}"#;

        let err = AnthropicClient::parse_error_response(408, body);
        assert!(matches!(err, LlmError::Timeout));
    }

    #[test]
    fn test_parse_error_response_server_error() {
        let body = r#"{
            "type": "error",
            "error": {
                "type": "api_error",
                "message": "Internal server error"
            }
        }"#;

        let err = AnthropicClient::parse_error_response(500, body);
        match err {
            LlmError::ApiError { status, message } => {
                assert_eq!(status, 500);
                assert!(message.contains("Internal server error"));
            }
            _ => panic!("Expected ApiError, got {:?}", err),
        }
    }

    #[test]
    fn test_parse_error_response_overloaded() {
        let body = r#"{
            "type": "error",
            "error": {
                "type": "overloaded_error",
                "message": "Overloaded"
            }
        }"#;

        let err = AnthropicClient::parse_error_response(529, body);
        match err {
            LlmError::ApiError { status, .. } => {
                assert_eq!(status, 529);
            }
            _ => panic!("Expected ApiError, got {:?}", err),
        }
    }

    #[test]
    fn test_parse_error_response_unstructured_body() {
        let body = "Bad Gateway";

        let err = AnthropicClient::parse_error_response(502, body);
        match err {
            LlmError::ApiError { status, message } => {
                assert_eq!(status, 502);
                assert_eq!(message, "Bad Gateway");
            }
            _ => panic!("Expected ApiError, got {:?}", err),
        }
    }

    #[test]
    fn test_parse_error_response_empty_body() {
        let err = AnthropicClient::parse_error_response(500, "");
        match err {
            LlmError::ApiError { status, message } => {
                assert_eq!(status, 500);
                assert!(message.is_empty());
            }
            _ => panic!("Expected ApiError, got {:?}", err),
        }
    }

    #[test]
    fn test_parse_error_response_timeout_without_body() {
        let err = AnthropicClient::parse_error_response(408, "");
        assert!(matches!(err, LlmError::Timeout));
    }

    // -----------------------------------------------------------------------
    // 流式响应
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_streaming_returns_not_implemented() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();

        let request = ChatRequest {
            model: "claude-3-haiku-20240307".to_string(),
            messages: vec![Message::user("Hi")],
            ..Default::default()
        };

        let result = client.chat_completion_stream(request).await;
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(matches!(err, LlmError::Internal(_)));
    }

    // -----------------------------------------------------------------------
    // 错误分类验证
    // -----------------------------------------------------------------------

    #[test]
    fn test_rate_limit_error_produces_correct_status() {
        let err = AnthropicClient::parse_error_response(
            429,
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"rate limited"}}"#,
        );
        match &err {
            LlmError::ApiError { status, .. } => assert_eq!(*status, 429),
            _ => panic!("Expected ApiError(429), got {:?}", err),
        }
    }

    #[test]
    fn test_auth_error_produces_correct_status() {
        let err = AnthropicClient::parse_error_response(
            401,
            r#"{"type":"error","error":{"type":"authentication_error","message":"bad key"}}"#,
        );
        match &err {
            LlmError::ApiError { status, .. } => assert_eq!(*status, 401),
            _ => panic!("Expected ApiError(401), got {:?}", err),
        }
    }

    #[test]
    fn test_server_error_produces_correct_status() {
        let err = AnthropicClient::parse_error_response(
            500,
            r#"{"type":"error","error":{"type":"api_error","message":"internal"}}"#,
        );
        match &err {
            LlmError::ApiError { status, .. } => assert_eq!(*status, 500),
            _ => panic!("Expected ApiError(500), got {:?}", err),
        }
    }

    #[test]
    fn test_timeout_error_classification() {
        let err = AnthropicClient::parse_error_response(408, "");
        assert!(
            matches!(err, LlmError::Timeout),
            "Expected Timeout, got {:?}",
            err
        );
    }

    #[test]
    fn test_debug_does_not_leak_api_key() {
        let client = AnthropicClient::new(
            "sk-ant-secret-key-12345".to_string(),
            "https://api.anthropic.com",
        )
        .unwrap();
        let debug_output = format!("{:?}", client);
        assert!(!debug_output.contains("sk-ant-secret-key-12345"));
        assert!(debug_output.contains("REDACTED"));
    }

    /// Regression test for 2026-05-17 MiniMax via Anthropic-shim bug.
    ///
    /// The `api.minimaxi.com/anthropic` shim returns Extended Thinking blocks
    /// (`type: thinking` with `thinking` + `signature` fields, no `text` field).
    /// Before the fix, `AnthropicContent.text: String` was required and
    /// deserialization failed with "error decoding response body" → 502.
    ///
    /// After the fix: `text: Option<String>` + `#[serde(default)]` parses both
    /// `text` and `thinking` blocks, and `convert_response` `filter_map`s out
    /// the thinking blocks so user-visible content is only the actual text.
    #[test]
    fn test_minimax_anthropic_shim_thinking_block_does_not_break_deserialization() {
        // Real response body shape from `api.minimaxi.com/anthropic/v1/messages`
        // with `MiniMax-M2.7-HighSpeed` model when max_tokens is small enough
        // to truncate before any text block emits.
        let body = serde_json::json!({
            "id": "0658ac8cecb5a94de849339d9a9b5621",
            "type": "message",
            "role": "assistant",
            "model": "MiniMax-M2.7-highspeed",
            "content": [
                {
                    "thinking": "The user just sent a ping...",
                    "signature": "3d68bd156c32855b96435f8063aee0045bbcaa1d5935e20bfaef57c9eeb6ab6b",
                    "type": "thinking"
                }
            ],
            "usage": {"input_tokens": 42, "output_tokens": 5},
            "stop_reason": "max_tokens"
        });

        let resp: AnthropicResponse = serde_json::from_value(body)
            .expect("MiniMax anthropic-shim thinking-only response must deserialize");
        assert_eq!(resp.content.len(), 1);
        assert_eq!(resp.content[0].content_type, "thinking");
        assert!(
            resp.content[0].text.is_none(),
            "thinking block has no text field — Option<String> must be None"
        );

        // Mixed thinking + text block (the common case for non-truncated
        // responses): both must parse, and convert_response must yield only
        // the text part (thinking is internal reasoning, not user-visible).
        let mixed = serde_json::json!({
            "id": "msg_mixed",
            "type": "message",
            "model": "MiniMax-M2.7-highspeed",
            "content": [
                {"type": "thinking", "thinking": "Let me consider…", "signature": "abc"},
                {"type": "text", "text": "Here is your answer."}
            ],
            "usage": {"input_tokens": 10, "output_tokens": 20},
            "stop_reason": "end_turn"
        });
        let resp_mixed: AnthropicResponse = serde_json::from_value(mixed)
            .expect("mixed thinking+text response must deserialize");
        let client =
            AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();
        let converted = client.convert_response(resp_mixed);
        // filter_map skips the thinking block — only the text block content
        // reaches the user.
        assert_eq!(
            converted.choices[0].message.content, "Here is your answer.",
            "thinking content must be filtered out; only text blocks reach the user"
        );
    }
}
