use crate::rate_limit_tracker::RateLimitSnapshot;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 消息角色
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// 系统消息
    System,
    /// 用户消息
    User,
    /// 助手消息
    Assistant,
    /// 工具调用结果
    Tool,
}

/// Prompt cache 控制标记
///
/// P1.1 — agentic loop prompt cache 优化。透传到原生支持 prompt
/// caching 的 provider（Anthropic native cache、OpenAI auto cache
/// ≥1024 tokens）。MiniMax/Ark/Generic provider 不识别该字段，
/// 通过 `skip_serializing_if = "Option::is_none"` 在请求体中自然
/// 缺省，provider 不会拒绝。
///
/// # 字段
///
/// * `kind` — Anthropic 的 cache 类型：`"ephemeral"`（5 分钟 TTL）
///   或 `"persistent"`（保留语义占位）。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct CacheControl {
    /// 缓存类型（如 "ephemeral"）
    #[serde(rename = "type")]
    pub kind: String,
}

impl CacheControl {
    /// 创建 ephemeral 缓存标记（Anthropic 5 分钟 TTL）
    pub fn ephemeral() -> Self {
        Self {
            kind: "ephemeral".to_string(),
        }
    }
}

/// 聊天消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// 角色
    pub role: Role,
    /// 内容
    pub content: String,
    /// 工具调用（仅 Assistant 角色）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// 工具调用 ID（仅 Tool 角色）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// 名称（用于工具调用）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// P1.1 — prompt cache 控制（Anthropic 透传；OpenAI 忽略；
    /// MiniMax/Ark/Generic 通过 skip_if_none 自然缺省）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

impl Message {
    /// 创建系统消息
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            cache_control: None,
        }
    }

    /// 创建用户消息
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            cache_control: None,
        }
    }

    /// 创建助手消息
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            cache_control: None,
        }
    }

    /// 创建带工具调用的助手消息
    pub fn assistant_with_tools(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
            cache_control: None,
        }
    }

    /// 创建工具响应消息
    pub fn tool(tool_call_id: String, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id),
            name: None,
            cache_control: None,
        }
    }

    /// P1.1 — builder helper：附加 prompt cache 标记
    pub fn with_cache_control(mut self, cache_control: CacheControl) -> Self {
        self.cache_control = Some(cache_control);
        self
    }
}

/// 工具调用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// 调用 ID
    pub id: String,
    /// 类型（默认 "function"）
    #[serde(rename = "type")]
    pub call_type: String,
    /// 函数调用详情
    pub function: FunctionCall,
}

/// 函数调用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    /// 函数名
    pub name: String,
    /// 参数（JSON 字符串）
    pub arguments: String,
}

/// 工具定义
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// 类型（默认 "function"）
    #[serde(rename = "type")]
    pub tool_type: String,
    /// 函数定义
    pub function: FunctionDefinition,
    /// P1.1 — prompt cache 控制（仅 Anthropic 透传；其他 provider
    /// 通过 skip_if_none 自然缺省）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

/// 函数定义
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FunctionDefinition {
    /// 函数名
    pub name: String,
    /// 描述
    pub description: String,
    /// 参数 schema（JSON Schema）
    pub parameters: serde_json::Value,
}

/// 聊天请求
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatRequest {
    /// 模型名称
    #[serde(default)]
    pub model: String,
    /// 消息列表
    #[serde(default)]
    pub messages: Vec<Message>,
    /// 温度（0.0 - 2.0）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// top_p
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// 最大 token 数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// 工具定义列表
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    /// 工具选择策略
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,
    /// 流式响应
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// 额外参数（提供商特定）
    #[serde(flatten, default)]
    pub extra: HashMap<String, serde_json::Value>,
    /// Credential pool key override (not serialized to provider).
    ///
    /// Set by `RetryProvider` when a `CredentialPool` is attached and a
    /// rotation-triggering error occurs.  Providers that support per-request
    /// key injection (e.g. future `PooledAnthropicClient`) read this field
    /// instead of their default key.  Standard providers ignore it.
    #[serde(skip)]
    pub api_key_override: Option<String>,
}

/// 聊天响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    /// 响应 ID
    pub id: String,
    /// 对象类型
    pub object: String,
    /// 创建时间戳
    pub created: i64,
    /// 模型名称
    pub model: String,
    /// 选择列表
    pub choices: Vec<Choice>,
    /// 使用情况
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Rate-limit headers captured from the provider response.
    ///
    /// `None` when the provider did not return any `x-ratelimit-*` headers
    /// (e.g. local Ollama, generic proxies) or when the response was
    /// deserialized from a cached / stored value that predates this field.
    #[serde(skip)]
    pub rate_limit: Option<RateLimitSnapshot>,
}

/// 选择
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    /// 索引
    pub index: u32,
    /// 消息
    pub message: Message,
    /// 结束原因
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

/// 使用情况
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    /// 提示 token 数
    pub prompt_tokens: u32,
    /// 完成 token 数
    pub completion_tokens: u32,
    /// 总 token 数
    pub total_tokens: u32,
    /// P1.1 — 从 cache 读取的 input token 数（Anthropic
    /// `cache_read_input_tokens` / OpenAI
    /// `usage.prompt_tokens_details.cached_tokens`）。Provider 不
    /// 返回时缺省为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u32>,
    /// P1.1 — 写入 cache 的 input token 数（Anthropic
    /// `cache_creation_input_tokens`）。Provider 不返回时缺省为
    /// None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u32>,
}

/// 流式响应块
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChunk {
    /// 响应 ID
    pub id: String,
    /// 对象类型
    pub object: String,
    /// 创建时间戳
    pub created: i64,
    /// 模型名称
    pub model: String,
    /// 选择增量
    pub choices: Vec<ChunkChoice>,
}

/// 流式选择
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkChoice {
    /// 索引
    pub index: u32,
    /// 增量消息
    pub delta: Delta,
    /// 结束原因
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

/// 增量消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delta {
    /// 角色
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    /// 内容增量
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// 工具调用增量
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

#[cfg(test)]
mod cache_control_tests {
    //! P1.1 — 验证 cache_control 字段对各 provider 的兼容性。
    //!
    //! - MiniMax/Ark/Generic：cache_control = None 时序列化体不含
    //!   该字段，provider 不会拒绝。
    //! - Anthropic：cache_control = Some(ephemeral) 时透传到请求体。
    //! - OpenAI：cache_control 仍透传（OpenAI 忽略未知字段）。
    use super::*;

    #[test]
    fn message_without_cache_control_omits_field() {
        let msg = Message::user("hi");
        let json = serde_json::to_value(&msg).unwrap();
        assert!(
            json.get("cache_control").is_none(),
            "expected cache_control field to be omitted when None; got {json}"
        );
    }

    #[test]
    fn message_with_cache_control_serializes_field() {
        let msg = Message::system("long prompt").with_cache_control(CacheControl::ephemeral());
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn tool_definition_without_cache_control_omits_field() {
        let tool = ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition::default(),
            cache_control: None,
        };
        let json = serde_json::to_value(&tool).unwrap();
        assert!(json.get("cache_control").is_none());
    }

    #[test]
    fn tool_definition_with_cache_control_serializes_field() {
        let tool = ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition::default(),
            cache_control: Some(CacheControl::ephemeral()),
        };
        let json = serde_json::to_value(&tool).unwrap();
        assert_eq!(json["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn usage_omits_cache_token_fields_when_none() {
        // P1.1 — 默认 Usage 没有 cache 信息时序列化体不含 cache 字段。
        // MiniMax/Ark/Generic provider 的响应反序列化为这种形态。
        let usage = Usage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            ..Default::default()
        };
        assert!(usage.cache_read_input_tokens.is_none());
        assert!(usage.cache_creation_input_tokens.is_none());
        let json = serde_json::to_value(&usage).unwrap();
        assert!(json.get("cache_read_input_tokens").is_none());
        assert!(json.get("cache_creation_input_tokens").is_none());
    }

    #[test]
    fn usage_includes_cache_token_fields_when_set() {
        let usage = Usage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
            cache_read_input_tokens: Some(80),
            cache_creation_input_tokens: Some(20),
        };
        let json = serde_json::to_value(&usage).unwrap();
        assert_eq!(json["cache_read_input_tokens"], 80);
        assert_eq!(json["cache_creation_input_tokens"], 20);
    }

    #[test]
    fn chat_request_with_no_cache_serializes_without_cache_fields() {
        // P1.1 — MiniMax/Ark/Generic 调用路径：请求体不含 cache_control。
        let req = ChatRequest {
            model: "minimax-test".to_string(),
            messages: vec![Message::system("hi"), Message::user("ping")],
            tools: Some(vec![ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: "noop".to_string(),
                    description: "no-op".to_string(),
                    parameters: serde_json::json!({}),
                },
                cache_control: None,
            }]),
            ..Default::default()
        };
        let body = serde_json::to_string(&req).unwrap();
        assert!(
            !body.contains("cache_control"),
            "MiniMax-style request should not carry cache_control; got: {body}"
        );
    }
}
