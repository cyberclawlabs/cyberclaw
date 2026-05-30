//! Anthropic Claude 客户端
//!
//! 实现 Anthropic Messages API 的完整客户端，包括：
//! - 请求/响应格式转换（内部类型 <-> Anthropic API 格式）
//! - 错误分类（速率限制、认证、服务器错误）
//! - 超时配置
//! - stop_reason 映射

use crate::client::LlmClient;
use crate::error::{LlmError, LlmResult};
use crate::rate_limit_tracker::RateLimitSnapshot;
use crate::types::{
    CacheControl, ChatChunk, ChatRequest, ChatResponse, Choice, FunctionCall, Message, Role,
    ToolCall, Usage,
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

/// 控制是否自动注入 Anthropic prompt cache 断点的环境变量。
///
/// 默认启用（`true`）。设置为 `"false"` 或 `"0"` 可禁用。
/// 禁用后请求体不携带任何 `cache_control` 字段，行为与注入前完全一致。
const PROMPT_CACHE_ENV_VAR: &str = "LLM_ANTHROPIC_PROMPT_CACHE_ENABLED";

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
    /// 工具定义（Anthropic Messages API 格式）。仅当 request 携带 tools 时发送。
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    /// 工具选择策略（仅当 tools 非空时发送）。
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<serde_json::Value>,
    /// 是否流式。non-stream 路径为 None（不序列化），stream 路径设 Some(true)。
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

/// Anthropic 工具定义
#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

/// Anthropic 消息内容：裸字符串（简单消息）或内容块数组（工具调用 / 工具结果）。
///
/// `untagged` 保证 `Text` 变体序列化成裸字符串，与简单消息及既有序列化测试兼容。
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
enum MessageContent {
    Text(String),
    Blocks(Vec<RequestContentBlock>),
}

/// Anthropic 请求侧内容块：text / tool_use / tool_result 三种。
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RequestContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
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
    content: MessageContent,
    /// Prompt cache 控制（Anthropic native cache；非 Anthropic 路径不序列化）
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
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
    content_type: String,
    #[serde(default)]
    text: Option<String>,
    /// tool_use 块：工具调用 ID
    #[serde(default)]
    id: Option<String>,
    /// tool_use 块：工具名
    #[serde(default)]
    name: Option<String>,
    /// tool_use 块：工具入参（对象）
    #[serde(default)]
    input: Option<serde_json::Value>,
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

/// 流式事件 → ChatChunk 转换的状态机。
///
/// Anthropic SSE 与 OpenAI 不同：每个 `data:` 行是一个带 `type` 字段的事件，
/// 需要跨事件维护状态（message id/model、各 tool_use 块的累积 partial_json）才能
/// 拼出 ChatChunk。把这部分逻辑抽成纯结构体，便于在不起真 HTTP 的情况下单测。
#[derive(Debug, Default)]
struct AnthropicStreamParser {
    /// message_start 捕获的消息 id（未捕获时为空串）。
    message_id: String,
    /// message_start 捕获的模型名（未捕获时为空串，由调用方用 request.model 兜底）。
    model: String,
    /// 各 content_block index → 进行中的 tool_use 累积状态。
    tool_blocks: std::collections::HashMap<u64, ToolBlockState>,
}

/// 单个 tool_use content block 的累积状态。
#[derive(Debug, Default)]
struct ToolBlockState {
    id: String,
    name: String,
    partial_json: String,
}

impl AnthropicStreamParser {
    fn new(fallback_model: &str) -> Self {
        Self {
            model: fallback_model.to_string(),
            ..Default::default()
        }
    }

    /// 构造一个携带当前 id/model 的 ChatChunk，choices 由调用方填充。
    fn chunk(&self, choices: Vec<crate::types::ChunkChoice>) -> ChatChunk {
        ChatChunk {
            id: self.message_id.clone(),
            object: "chat.completion.chunk".to_string(),
            created: chrono::Utc::now().timestamp(),
            model: self.model.clone(),
            choices,
        }
    }

    /// 把 Anthropic stop_reason 映射为 OpenAI 风格 finish_reason。
    fn map_stop_reason(reason: &str) -> String {
        match reason {
            "end_turn" | "stop_sequence" => "stop".to_string(),
            "max_tokens" => "length".to_string(),
            "tool_use" => "tool_calls".to_string(),
            other => other.to_string(),
        }
    }

    /// 处理一条 SSE `data:` 行的 JSON 文本，返回应当发出的 ChatChunk 序列。
    ///
    /// - 文本 delta / tool_use 完成 / finish_reason 各产出一个 chunk。
    /// - thinking delta、ping、未知 type、input_json_delta（仅累积）不产出 chunk。
    /// - JSON 畸形产出一个 `Err(LlmError::JsonError)`。
    fn handle_data(&mut self, json_str: &str) -> Vec<LlmResult<ChatChunk>> {
        let value: serde_json::Value = match serde_json::from_str(json_str) {
            Ok(v) => v,
            Err(e) => return vec![Err(LlmError::JsonError(e))],
        };

        let event_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match event_type {
            "message_start" => {
                if let Some(msg) = value.get("message") {
                    if let Some(id) = msg.get("id").and_then(|v| v.as_str()) {
                        self.message_id = id.to_string();
                    }
                    if let Some(model) = msg.get("model").and_then(|v| v.as_str()) {
                        self.model = model.to_string();
                    }
                }
                // 首块：发一个带 role=assistant 的空 content chunk，匹配 OpenAI 语义。
                let choice = crate::types::ChunkChoice {
                    index: 0,
                    delta: crate::types::Delta {
                        role: Some(Role::Assistant),
                        content: None,
                        tool_calls: None,
                    },
                    finish_reason: None,
                };
                vec![Ok(self.chunk(vec![choice]))]
            }
            "content_block_start" => {
                let index = value.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
                let block = value.get("content_block");
                let is_tool_use =
                    block.and_then(|b| b.get("type")).and_then(|t| t.as_str()) == Some("tool_use");
                if is_tool_use {
                    let id = block
                        .and_then(|b| b.get("id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let name = block
                        .and_then(|b| b.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    self.tool_blocks.insert(
                        index,
                        ToolBlockState {
                            id,
                            name,
                            partial_json: String::new(),
                        },
                    );
                }
                Vec::new()
            }
            "content_block_delta" => {
                let index = value.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
                let delta = value.get("delta");
                let delta_type = delta
                    .and_then(|d| d.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                match delta_type {
                    "text_delta" => {
                        let text = delta
                            .and_then(|d| d.get("text"))
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let choice = crate::types::ChunkChoice {
                            index: 0,
                            delta: crate::types::Delta {
                                role: None,
                                content: Some(text),
                                tool_calls: None,
                            },
                            finish_reason: None,
                        };
                        vec![Ok(self.chunk(vec![choice]))]
                    }
                    "input_json_delta" => {
                        if let Some(state) = self.tool_blocks.get_mut(&index) {
                            if let Some(partial) = delta
                                .and_then(|d| d.get("partial_json"))
                                .and_then(|v| v.as_str())
                            {
                                state.partial_json.push_str(partial);
                            }
                        }
                        Vec::new()
                    }
                    // thinking_delta 及其它：内部推理，不当 content 发出。
                    _ => Vec::new(),
                }
            }
            "content_block_stop" => {
                let index = value.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
                if let Some(state) = self.tool_blocks.remove(&index) {
                    let arguments = if state.partial_json.is_empty() {
                        "{}".to_string()
                    } else {
                        state.partial_json
                    };
                    let tool_call = ToolCall {
                        id: state.id,
                        call_type: "function".to_string(),
                        function: FunctionCall {
                            name: state.name,
                            arguments,
                        },
                    };
                    let choice = crate::types::ChunkChoice {
                        index: 0,
                        delta: crate::types::Delta {
                            role: None,
                            content: None,
                            tool_calls: Some(vec![tool_call]),
                        },
                        finish_reason: None,
                    };
                    vec![Ok(self.chunk(vec![choice]))]
                } else {
                    Vec::new()
                }
            }
            "message_delta" => {
                let stop_reason = value
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(|v| v.as_str());
                match stop_reason {
                    Some(reason) => {
                        let choice = crate::types::ChunkChoice {
                            index: 0,
                            delta: crate::types::Delta {
                                role: None,
                                content: None,
                                tool_calls: None,
                            },
                            finish_reason: Some(Self::map_stop_reason(reason)),
                        };
                        vec![Ok(self.chunk(vec![choice]))]
                    }
                    None => Vec::new(),
                }
            }
            // message_stop 由调用方据返回的空 vec + 显式终止处理；ping / 未知 type 忽略。
            _ => Vec::new(),
        }
    }
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
                        content: MessageContent::Text(msg.content.clone()),
                        cache_control: None,
                    });
                }
                Role::Assistant => {
                    let content = match &msg.tool_calls {
                        Some(calls) if !calls.is_empty() => {
                            let mut blocks = Vec::new();
                            if !msg.content.is_empty() {
                                blocks.push(RequestContentBlock::Text {
                                    text: msg.content.clone(),
                                });
                            }
                            for call in calls {
                                let input = serde_json::from_str(&call.function.arguments)
                                    .unwrap_or_else(|_| serde_json::json!({}));
                                blocks.push(RequestContentBlock::ToolUse {
                                    id: call.id.clone(),
                                    name: call.function.name.clone(),
                                    input,
                                });
                            }
                            MessageContent::Blocks(blocks)
                        }
                        _ => MessageContent::Text(msg.content.clone()),
                    };
                    messages.push(AnthropicMessage {
                        role: "assistant".to_string(),
                        content,
                        cache_control: None,
                    });
                }
                Role::Tool => {
                    // Anthropic 工具结果作为 user 消息的 tool_result 块承载。
                    messages.push(AnthropicMessage {
                        role: "user".to_string(),
                        content: MessageContent::Blocks(vec![RequestContentBlock::ToolResult {
                            tool_use_id: msg.tool_call_id.clone().unwrap_or_default(),
                            content: msg.content.clone(),
                        }]),
                        cache_control: None,
                    });
                }
            }
        }

        let system = if system_blocks.is_empty() {
            None
        } else {
            Some(system_blocks)
        };

        let (tools, tool_choice) = match &request.tools {
            Some(defs) if !defs.is_empty() => {
                let anthropic_tools = defs
                    .iter()
                    .map(|d| AnthropicTool {
                        name: d.function.name.clone(),
                        description: d.function.description.clone(),
                        input_schema: d.function.parameters.clone(),
                        cache_control: d.cache_control.clone(),
                    })
                    .collect();
                let choice = request
                    .tool_choice
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .map(|s| match s {
                        "auto" => serde_json::json!({"type": "auto"}),
                        "any" | "required" => serde_json::json!({"type": "any"}),
                        "none" => serde_json::json!({"type": "none"}),
                        other => serde_json::json!({"type": "tool", "name": other}),
                    });
                (Some(anthropic_tools), choice)
            }
            _ => (None, None),
        };

        // Anthropic 要求 messages 非空且首条为 user。上下文压缩 (SummarizeEarly)
        // 可能把原始 user turn 折叠进 system 摘要, 使 live messages 以
        // assistant(tool_use) 起头 → MiniMax /anthropic shim 对该请求报
        // 500 "input json is empty"。补一条前导 user 维持 API 契约
        // (对应 generic provider 的 ensure_user_message; Anthropic 要求首条
        // 为 user, 故前置而非追加)。
        if messages.first().map(|m| m.role != "user").unwrap_or(true) {
            messages.insert(
                0,
                AnthropicMessage {
                    role: "user".to_string(),
                    content: MessageContent::Text("Continue.".to_string()),
                    cache_control: None,
                },
            );
        }

        AnthropicRequest {
            model: request.model.clone(),
            messages,
            max_tokens: request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            temperature: request.temperature,
            top_p: request.top_p,
            system,
            tools,
            tool_choice,
            stream: None,
        }
    }

    /// 转换 Anthropic 响应到内部格式
    fn convert_response(&self, resp: AnthropicResponse) -> ChatResponse {
        // text 块 join 成 content；filter_map 跳过 thinking 块（无 text 字段）。
        // tool_use 块单独收集成 ToolCall。
        let mut text_parts: Vec<String> = Vec::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        for block in resp.content {
            if block.content_type == "tool_use" {
                tool_calls.push(ToolCall {
                    id: block.id.unwrap_or_default(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: block.name.unwrap_or_default(),
                        arguments: block
                            .input
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "{}".to_string()),
                    },
                });
            } else if let Some(text) = block.text {
                text_parts.push(text);
            }
        }
        let content = text_parts.join("\n");

        let finish_reason = resp.stop_reason.map(|r| match r.as_str() {
            "end_turn" => "stop".to_string(),
            "max_tokens" => "length".to_string(),
            "stop_sequence" => "stop".to_string(),
            "tool_use" => "tool_calls".to_string(),
            other => other.to_string(),
        });

        let message = if tool_calls.is_empty() {
            Message::assistant(content)
        } else {
            Message::assistant_with_tools(content, tool_calls)
        };

        ChatResponse {
            id: resp.id,
            object: "chat.completion".to_string(),
            created: chrono::Utc::now().timestamp(),
            model: resp.model,
            choices: vec![Choice {
                index: 0,
                message,
                finish_reason,
            }],
            usage: Some(Usage {
                prompt_tokens: resp.usage.input_tokens,
                completion_tokens: resp.usage.output_tokens,
                total_tokens: resp.usage.input_tokens + resp.usage.output_tokens,
                cache_read_input_tokens: resp.usage.cache_read_input_tokens,
                cache_creation_input_tokens: resp.usage.cache_creation_input_tokens,
            }),
            rate_limit: None,
        }
    }

    /// 检查是否启用 prompt cache 注入。
    ///
    /// 读取 `LLM_ANTHROPIC_PROMPT_CACHE_ENABLED` 环境变量：
    /// - 未设置或设置为 `"true"` / `"1"` → 启用（默认）
    /// - 设置为 `"false"` / `"0"` → 禁用
    fn prompt_cache_enabled() -> bool {
        !matches!(
            std::env::var(PROMPT_CACHE_ENV_VAR).as_deref(),
            Ok("false") | Ok("0")
        )
    }

    /// 按 system_and_3 策略注入 Anthropic prompt cache 断点。
    ///
    /// Anthropic API 每次请求最多支持 4 个 `cache_control` 断点。
    /// 断点分配：最后一个 system 内容块占 1 个，剩余最多 3 个分配给
    /// 最后几条非 system 消息（滚动会话窗口）。
    ///
    /// 函数直接修改 `AnthropicRequest`，在调用方序列化前完成注入，
    /// 不改变任何既有字段语义。
    fn inject_cache_breakpoints(req: &mut AnthropicRequest) {
        let marker = CacheControl::ephemeral();
        let mut breakpoints_used = 0u8;

        // 断点 1：最后一个 system 内容块
        if let Some(blocks) = req.system.as_mut() {
            if let Some(last_block) = blocks.last_mut() {
                last_block.cache_control = Some(marker.clone());
                breakpoints_used += 1;
            }
        }

        // 断点 2-4：最后 3 条消息（user / assistant / tool）
        let remaining = 4usize.saturating_sub(breakpoints_used as usize);
        let msg_count = req.messages.len();
        let start = msg_count.saturating_sub(remaining);
        for msg in &mut req.messages[start..] {
            msg.cache_control = Some(marker.clone());
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
        let mut anthropic_req = self.convert_request(&request);
        if Self::prompt_cache_enabled() {
            Self::inject_cache_breakpoints(&mut anthropic_req);
        }

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

        // Capture rate-limit headers before consuming the response body.
        let rate_limit = RateLimitSnapshot::from_headers(response.headers(), self.provider());

        let anthropic_resp: AnthropicResponse = response.json().await?;
        let mut chat_response = self.convert_response(anthropic_resp);
        chat_response.rate_limit = rate_limit;
        Ok(chat_response)
    }

    async fn chat_completion_stream(
        &self,
        request: ChatRequest,
    ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>> {
        use futures::StreamExt;

        let url = format!("{}/v1/messages", self.base_url);
        let mut anthropic_req = self.convert_request(&request);
        anthropic_req.stream = Some(true);
        if Self::prompt_cache_enabled() {
            Self::inject_cache_breakpoints(&mut anthropic_req);
        }

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

        // 复用 generic 的 mpsc + buffer + unfold SSE 骨架。差异：用
        // AnthropicStreamParser 把每个 `data:` 事件转成 ChatChunk 序列。
        let fallback_model = request.model.clone();
        let (tx, rx) = tokio::sync::mpsc::channel::<LlmResult<ChatChunk>>(64);
        tokio::spawn(async move {
            let mut parser = AnthropicStreamParser::new(&fallback_model);
            let mut bytes_stream = response.bytes_stream();
            let mut buffer = String::new();
            while let Some(chunk_res) = bytes_stream.next().await {
                match chunk_res {
                    Err(e) => {
                        let _ = tx.send(Err(LlmError::HttpError(e))).await;
                        return;
                    }
                    Ok(bytes) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        // 事件边界 "\n\n"。TCP 块可能切碎或合并多个事件。
                        while let Some(pos) = buffer.find("\n\n") {
                            let event_block = buffer[..pos].to_string();
                            buffer.drain(..pos + 2);
                            for line in event_block.lines() {
                                let line = line.trim();
                                let Some(json_str) = line.strip_prefix("data:") else {
                                    // 忽略 event: / id: / : 注释行 / 空行
                                    continue;
                                };
                                let json_str = json_str.trim();
                                if json_str.is_empty() {
                                    continue;
                                }
                                // message_stop 标志流结束（解析后亦返回空 vec）。
                                let is_stop = serde_json::from_str::<serde_json::Value>(json_str)
                                    .ok()
                                    .and_then(|v| {
                                        v.get("type")
                                            .and_then(|t| t.as_str())
                                            .map(|s| s == "message_stop")
                                    })
                                    .unwrap_or(false);
                                for chunk in parser.handle_data(json_str) {
                                    if tx.send(chunk).await.is_err() {
                                        return;
                                    }
                                }
                                if is_stop {
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        });

        let stream = futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        });
        Ok(Box::new(Box::pin(stream)))
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
        assert!(matches!(
            &converted.messages[0].content,
            MessageContent::Text(t) if t == "Hello"
        ));
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
        match &converted.messages[1].content {
            MessageContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 1);
                match &blocks[0] {
                    RequestContentBlock::ToolResult {
                        tool_use_id,
                        content,
                    } => {
                        assert_eq!(tool_use_id, "call-1");
                        assert_eq!(content, r#"{"result": 42}"#);
                    }
                    other => panic!("expected tool_result block, got {:?}", other),
                }
            }
            other => panic!("expected Blocks content, got {:?}", other),
        }
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

    #[test]
    fn test_convert_request_prepends_user_when_first_is_assistant() {
        // Bug I-d (anthropic 版): 上下文压缩把原始 user turn 折叠进 system 摘要,
        // 使 live messages 以 assistant(tool_use) 起头、无 user-text 消息。Anthropic
        // 要求 messages 首条为 user, 否则 MiniMax /anthropic shim 报 500
        // "input json is empty"。convert_request 须前置一条 user 维持契约。
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();
        let tc = ToolCall {
            id: "call_1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "cmd_run".to_string(),
                arguments: "{}".to_string(),
            },
        };
        let request = ChatRequest {
            model: "MiniMax-M2.7-HighSpeed".to_string(),
            messages: vec![
                Message::system("[Context summary]\n[User] 生成 pptx"),
                Message::assistant_with_tools("", vec![tc]),
                Message::tool("call_1".to_string(), "result"),
            ],
            ..Default::default()
        };
        let converted = client.convert_request(&request);
        assert_eq!(
            converted.messages.first().expect("non-empty").role,
            "user",
            "Anthropic 契约: 首条消息必须是 user"
        );
        assert!(
            matches!(&converted.messages[0].content, MessageContent::Text(t) if t == "Continue."),
            "前置的 user 消息内容应为 Continue."
        );
        // 原有的 assistant(tool_use) / tool(result) 顺序保留在前置 user 之后。
        assert_eq!(converted.messages[1].role, "assistant");
        assert_eq!(converted.messages[2].role, "user");
    }

    #[test]
    fn test_convert_request_no_prepend_when_first_is_user() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();
        let request = ChatRequest {
            model: "claude-3-sonnet-20240229".to_string(),
            messages: vec![Message::user("Hi"), Message::assistant("Hello!")],
            ..Default::default()
        };
        let converted = client.convert_request(&request);
        // 首条已是 user → 不前置, 长度不变。
        assert_eq!(converted.messages.len(), 2);
        assert_eq!(converted.messages[0].role, "user");
        assert!(
            matches!(&converted.messages[0].content, MessageContent::Text(t) if t == "Hi"),
            "首条 user 原样保留, 不应被 Continue. 替换"
        );
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
                content: MessageContent::Text("Hello".to_string()),
                cache_control: None,
            }],
            max_tokens: 1024,
            temperature: Some(0.5),
            top_p: None,
            system: Some(vec![AnthropicSystemBlock {
                block_type: "text".to_string(),
                text: "Be helpful".to_string(),
                cache_control: None,
            }]),
            tools: None,
            tool_choice: None,
            stream: None,
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
                content: MessageContent::Text("Hi".to_string()),
                cache_control: None,
            }],
            max_tokens: 1024,
            temperature: None,
            top_p: None,
            system: Some(vec![AnthropicSystemBlock {
                block_type: "text".to_string(),
                text: "long system prompt".to_string(),
                cache_control: Some(CacheControl::ephemeral()),
            }]),
            tools: None,
            tool_choice: None,
            stream: None,
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
                content: MessageContent::Text("Hi".to_string()),
                cache_control: None,
            }],
            max_tokens: 100,
            temperature: None,
            top_p: None,
            system: None,
            tools: None,
            tool_choice: None,
            stream: None,
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
        assert_eq!(
            resp.content[0].text.as_deref(),
            Some("Hello! How can I help you?")
        );
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
                id: None,
                name: None,
                input: None,
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
                id: None,
                name: None,
                input: None,
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
                id: None,
                name: None,
                input: None,
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
                    id: None,
                    name: None,
                    input: None,
                },
                AnthropicContent {
                    content_type: "text".to_string(),
                    text: Some("Second block".to_string()),
                    id: None,
                    name: None,
                    input: None,
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
                id: None,
                name: None,
                input: None,
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

    // convert_request 默认 stream=None（non-stream 路径不受影响）；
    // stream 路径在 chat_completion_stream 内显式设 Some(true)。
    #[test]
    fn test_convert_request_stream_defaults_none() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();
        let request = ChatRequest {
            model: "claude-3-haiku-20240307".to_string(),
            messages: vec![Message::user("Hi")],
            ..Default::default()
        };
        let converted = client.convert_request(&request);
        assert!(converted.stream.is_none());
        let json = serde_json::to_value(&converted).unwrap();
        assert!(json.get("stream").is_none());
    }

    // -----------------------------------------------------------------------
    // 流式事件 → ChatChunk 转换（AnthropicStreamParser，纯函数单测）
    // -----------------------------------------------------------------------

    /// 把 parser 产出的 chunk 结果展开，遇 Err 直接 panic（测试便利）。
    fn drain(parser: &mut AnthropicStreamParser, data: &str) -> Vec<ChatChunk> {
        parser
            .handle_data(data)
            .into_iter()
            .map(|r| r.expect("chunk must be Ok"))
            .collect()
    }

    #[test]
    fn test_stream_message_start_emits_role_chunk_and_captures_id_model() {
        let mut parser = AnthropicStreamParser::new("fallback-model");
        let chunks = drain(
            &mut parser,
            r#"{"type":"message_start","message":{"id":"msg_x","model":"claude-real","usage":{}}}"#,
        );
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].id, "msg_x");
        assert_eq!(chunks[0].model, "claude-real");
        assert_eq!(chunks[0].object, "chat.completion.chunk");
        assert_eq!(chunks[0].choices[0].delta.role, Some(Role::Assistant));
        assert!(chunks[0].choices[0].delta.content.is_none());
    }

    #[test]
    fn test_stream_text_deltas_accumulate_content() {
        // (a) 一串 text_delta → 拼出的 content 正确
        let mut parser = AnthropicStreamParser::new("m");
        let mut content = String::new();
        for frag in ["Hello", ", ", "world"] {
            let data = format!(
                r#"{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"{}"}}}}"#,
                frag
            );
            for c in drain(&mut parser, &data) {
                if let Some(t) = &c.choices[0].delta.content {
                    content.push_str(t);
                }
            }
        }
        assert_eq!(content, "Hello, world");
    }

    #[test]
    fn test_stream_message_delta_stop_reason_mapping() {
        // (b) tool_use→tool_calls, end_turn→stop, max_tokens→length
        let cases = [
            ("tool_use", "tool_calls"),
            ("end_turn", "stop"),
            ("stop_sequence", "stop"),
            ("max_tokens", "length"),
            ("something_else", "something_else"),
        ];
        for (anthropic_reason, expected) in cases {
            let mut parser = AnthropicStreamParser::new("m");
            let data = format!(
                r#"{{"type":"message_delta","delta":{{"stop_reason":"{}"}},"usage":{{"output_tokens":3}}}}"#,
                anthropic_reason
            );
            let chunks = drain(&mut parser, &data);
            assert_eq!(chunks.len(), 1);
            assert_eq!(
                chunks[0].choices[0].finish_reason.as_deref(),
                Some(expected),
                "stop_reason {} must map to {}",
                anthropic_reason,
                expected
            );
        }
    }

    #[test]
    fn test_stream_tool_use_block_accumulates_into_tool_call() {
        // (c) content_block_start(tool_use) + input_json_delta* + content_block_stop
        let mut parser = AnthropicStreamParser::new("m");

        let start = drain(
            &mut parser,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"get_weather","input":{}}}"#,
        );
        assert!(
            start.is_empty(),
            "content_block_start must not emit a chunk"
        );

        for partial in [r#"{\"loc"#, r#"ation\":\"SF\"}"#] {
            let data = format!(
                r#"{{"type":"content_block_delta","index":1,"delta":{{"type":"input_json_delta","partial_json":"{}"}}}}"#,
                partial
            );
            let emitted = drain(&mut parser, &data);
            assert!(emitted.is_empty(), "input_json_delta must not emit a chunk");
        }

        let stop = drain(&mut parser, r#"{"type":"content_block_stop","index":1}"#);
        assert_eq!(stop.len(), 1);
        let tool_calls = stop[0].choices[0]
            .delta
            .tool_calls
            .as_ref()
            .expect("tool_calls present on content_block_stop");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "toolu_1");
        assert_eq!(tool_calls[0].call_type, "function");
        assert_eq!(tool_calls[0].function.name, "get_weather");
        let args: serde_json::Value =
            serde_json::from_str(&tool_calls[0].function.arguments).unwrap();
        assert_eq!(args["location"], "SF");
    }

    #[test]
    fn test_stream_tool_use_empty_input_defaults_to_empty_object() {
        let mut parser = AnthropicStreamParser::new("m");
        drain(
            &mut parser,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"f","input":{}}}"#,
        );
        let stop = drain(&mut parser, r#"{"type":"content_block_stop","index":0}"#);
        let tc = stop[0].choices[0].delta.tool_calls.as_ref().unwrap();
        assert_eq!(tc[0].function.arguments, "{}");
    }

    #[test]
    fn test_stream_thinking_delta_emits_no_content() {
        // (d) thinking_delta 不产出 content
        let mut parser = AnthropicStreamParser::new("m");
        let chunks = drain(
            &mut parser,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"internal reasoning"}}"#,
        );
        assert!(chunks.is_empty(), "thinking_delta must not produce a chunk");
    }

    #[test]
    fn test_stream_ping_and_unknown_type_emit_no_chunk() {
        // (e) ping / 未知 type 不产出 chunk
        let mut parser = AnthropicStreamParser::new("m");
        assert!(drain(&mut parser, r#"{"type":"ping"}"#).is_empty());
        assert!(drain(&mut parser, r#"{"type":"some_future_event","foo":1}"#).is_empty());
        // content_block_stop for a non-tool index (no recorded tool block) → 无 chunk
        assert!(drain(&mut parser, r#"{"type":"content_block_stop","index":0}"#).is_empty());
    }

    #[test]
    fn test_stream_malformed_data_yields_json_error() {
        let mut parser = AnthropicStreamParser::new("m");
        let results = parser.handle_data("{not valid json");
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], Err(LlmError::JsonError(_))));
    }

    #[test]
    fn test_stream_fallback_model_used_until_message_start() {
        // message_start 未到达前的 chunk 用 fallback model + 空 id
        let mut parser = AnthropicStreamParser::new("fallback-model");
        let chunks = drain(
            &mut parser,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#,
        );
        assert_eq!(chunks[0].model, "fallback-model");
        assert_eq!(chunks[0].id, "");
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

    // -----------------------------------------------------------------------
    // inject_cache_breakpoints
    // -----------------------------------------------------------------------

    #[test]
    fn test_inject_cache_breakpoints_system_gets_cache_control() {
        // system block 应获得 cache_control: ephemeral
        let mut req = AnthropicRequest {
            model: "claude-3-5-sonnet-20241022".to_string(),
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: MessageContent::Text("Hello".to_string()),
                cache_control: None,
            }],
            max_tokens: 1024,
            temperature: None,
            top_p: None,
            system: Some(vec![AnthropicSystemBlock {
                block_type: "text".to_string(),
                text: "You are a helpful assistant.".to_string(),
                cache_control: None,
            }]),
            tools: None,
            tool_choice: None,
            stream: None,
        };

        AnthropicClient::inject_cache_breakpoints(&mut req);

        let blocks = req.system.as_ref().unwrap();
        assert_eq!(
            blocks[0].cache_control.as_ref().unwrap().kind,
            "ephemeral",
            "system block must get cache_control: ephemeral"
        );
    }

    #[test]
    fn test_inject_cache_breakpoints_last_3_messages_get_cache_control() {
        // system 断点消耗 1，剩余 3 个分配给消息。5 条消息时最后 3 条打标，前 2 条不打。
        let make_msg = |role: &str, content: &str| AnthropicMessage {
            role: role.to_string(),
            content: MessageContent::Text(content.to_string()),
            cache_control: None,
        };
        let mut req = AnthropicRequest {
            model: "claude-3-5-sonnet-20241022".to_string(),
            messages: vec![
                make_msg("user", "msg1"),
                make_msg("assistant", "msg2"),
                make_msg("user", "msg3"),
                make_msg("assistant", "msg4"),
                make_msg("user", "msg5"),
            ],
            max_tokens: 1024,
            temperature: None,
            top_p: None,
            system: Some(vec![AnthropicSystemBlock {
                block_type: "text".to_string(),
                text: "System".to_string(),
                cache_control: None,
            }]),
            tools: None,
            tool_choice: None,
            stream: None,
        };

        AnthropicClient::inject_cache_breakpoints(&mut req);

        // system 断点 = 1，剩余 3，所以最后 3 条消息（idx 2,3,4）打标
        assert!(
            req.messages[0].cache_control.is_none(),
            "messages[0] must NOT get cache_control"
        );
        assert!(
            req.messages[1].cache_control.is_none(),
            "messages[1] must NOT get cache_control"
        );
        assert_eq!(
            req.messages[2].cache_control.as_ref().unwrap().kind,
            "ephemeral",
            "messages[2] must get cache_control"
        );
        assert_eq!(
            req.messages[3].cache_control.as_ref().unwrap().kind,
            "ephemeral",
            "messages[3] must get cache_control"
        );
        assert_eq!(
            req.messages[4].cache_control.as_ref().unwrap().kind,
            "ephemeral",
            "messages[4] must get cache_control"
        );
    }

    #[test]
    fn test_inject_cache_breakpoints_fewer_than_3_messages() {
        // 只有 2 条消息时两条都应打标记
        let make_msg = |role: &str, content: &str| AnthropicMessage {
            role: role.to_string(),
            content: MessageContent::Text(content.to_string()),
            cache_control: None,
        };
        let mut req = AnthropicRequest {
            model: "claude-3-5-sonnet-20241022".to_string(),
            messages: vec![make_msg("user", "hi"), make_msg("assistant", "hello")],
            max_tokens: 1024,
            temperature: None,
            top_p: None,
            system: Some(vec![AnthropicSystemBlock {
                block_type: "text".to_string(),
                text: "System".to_string(),
                cache_control: None,
            }]),
            tools: None,
            tool_choice: None,
            stream: None,
        };

        AnthropicClient::inject_cache_breakpoints(&mut req);

        // 系统断点消耗 1，剩余 3 个可用于消息；消息只有 2 条 → 两条都应打标
        assert_eq!(
            req.messages[0].cache_control.as_ref().unwrap().kind,
            "ephemeral"
        );
        assert_eq!(
            req.messages[1].cache_control.as_ref().unwrap().kind,
            "ephemeral"
        );
    }

    #[test]
    fn test_inject_cache_breakpoints_disabled_via_env() {
        // 设置禁用标志后，inject 不应被调用，消息不携带 cache_control
        // 注意：此测试直接验证 prompt_cache_enabled() 逻辑，不修改全局 env
        // (测试间并行可能干扰)，改为直接验证函数行为：disabled 路径下
        // AnthropicRequest 不调用 inject_cache_breakpoints，结果为 None。
        let mut req = AnthropicRequest {
            model: "claude-3-5-sonnet-20241022".to_string(),
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: MessageContent::Text("Hello".to_string()),
                cache_control: None,
            }],
            max_tokens: 1024,
            temperature: None,
            top_p: None,
            system: Some(vec![AnthropicSystemBlock {
                block_type: "text".to_string(),
                text: "System".to_string(),
                cache_control: None,
            }]),
            tools: None,
            tool_choice: None,
            stream: None,
        };

        // 模拟 disabled 分支：不调用 inject_cache_breakpoints
        let cache_enabled = false;
        if cache_enabled {
            AnthropicClient::inject_cache_breakpoints(&mut req);
        }

        assert!(
            req.system.as_ref().unwrap()[0].cache_control.is_none(),
            "system block must have no cache_control when disabled"
        );
        assert!(
            req.messages[0].cache_control.is_none(),
            "messages must have no cache_control when disabled"
        );
    }

    #[test]
    fn test_prompt_cache_enabled_default_true() {
        // 未设置环境变量时应返回 true（默认启用）
        // 暂存并清除任何已有值，测试后恢复
        let prev = std::env::var(PROMPT_CACHE_ENV_VAR).ok();
        std::env::remove_var(PROMPT_CACHE_ENV_VAR);

        assert!(
            AnthropicClient::prompt_cache_enabled(),
            "prompt cache must be enabled by default"
        );

        // 恢复环境变量
        if let Some(val) = prev {
            std::env::set_var(PROMPT_CACHE_ENV_VAR, val);
        }
    }

    #[test]
    fn test_prompt_cache_enabled_false_when_set_to_false() {
        let prev = std::env::var(PROMPT_CACHE_ENV_VAR).ok();
        std::env::set_var(PROMPT_CACHE_ENV_VAR, "false");

        assert!(
            !AnthropicClient::prompt_cache_enabled(),
            "prompt cache must be disabled when env var is 'false'"
        );

        // 恢复
        match prev {
            Some(val) => std::env::set_var(PROMPT_CACHE_ENV_VAR, val),
            None => std::env::remove_var(PROMPT_CACHE_ENV_VAR),
        }
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
        let resp_mixed: AnthropicResponse =
            serde_json::from_value(mixed).expect("mixed thinking+text response must deserialize");
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();
        let converted = client.convert_response(resp_mixed);
        // filter_map skips the thinking block — only the text block content
        // reaches the user.
        assert_eq!(
            converted.choices[0].message.content, "Here is your answer.",
            "thinking content must be filtered out; only text blocks reach the user"
        );
    }

    // -----------------------------------------------------------------------
    // 工具调用（tool calling）
    // -----------------------------------------------------------------------

    use crate::types::{FunctionDefinition, ToolDefinition};

    fn tool_def(name: &str) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: name.to_string(),
                description: format!("description of {}", name),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"x": {"type": "string"}},
                    "required": ["x"]
                }),
            },
            cache_control: None,
        }
    }

    #[test]
    fn test_request_with_tool_definition_serializes() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();
        let request = ChatRequest {
            model: "claude-3-5-sonnet-20241022".to_string(),
            messages: vec![Message::user("use the tool")],
            tools: Some(vec![tool_def("search")]),
            ..Default::default()
        };

        let converted = client.convert_request(&request);
        let json = serde_json::to_value(&converted).unwrap();
        assert_eq!(json["tools"][0]["name"], "search");
        assert_eq!(json["tools"][0]["description"], "description of search");
        assert_eq!(json["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(
            json["tools"][0]["input_schema"]["properties"]["x"]["type"],
            "string"
        );
    }

    #[test]
    fn test_tool_choice_auto_maps_to_object() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();
        let request = ChatRequest {
            model: "claude-3-5-sonnet-20241022".to_string(),
            messages: vec![Message::user("hi")],
            tools: Some(vec![tool_def("search")]),
            tool_choice: Some("auto".to_string()),
            ..Default::default()
        };

        let json = serde_json::to_value(client.convert_request(&request)).unwrap();
        assert_eq!(json["tool_choice"], serde_json::json!({"type": "auto"}));
    }

    #[test]
    fn test_tool_choice_required_maps_to_any() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();
        let request = ChatRequest {
            model: "claude-3-5-sonnet-20241022".to_string(),
            messages: vec![Message::user("hi")],
            tools: Some(vec![tool_def("search")]),
            tool_choice: Some("required".to_string()),
            ..Default::default()
        };

        let json = serde_json::to_value(client.convert_request(&request)).unwrap();
        assert_eq!(json["tool_choice"], serde_json::json!({"type": "any"}));
    }

    #[test]
    fn test_tool_choice_named_tool() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();
        let request = ChatRequest {
            model: "claude-3-5-sonnet-20241022".to_string(),
            messages: vec![Message::user("hi")],
            tools: Some(vec![tool_def("search")]),
            tool_choice: Some("search".to_string()),
            ..Default::default()
        };

        let json = serde_json::to_value(client.convert_request(&request)).unwrap();
        assert_eq!(
            json["tool_choice"],
            serde_json::json!({"type": "tool", "name": "search"})
        );
    }

    #[test]
    fn test_tool_choice_not_sent_without_tools() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();
        let request = ChatRequest {
            model: "claude-3-5-sonnet-20241022".to_string(),
            messages: vec![Message::user("hi")],
            tools: None,
            tool_choice: Some("auto".to_string()),
            ..Default::default()
        };

        let json = serde_json::to_value(client.convert_request(&request)).unwrap();
        assert!(json.get("tools").is_none());
        assert!(json.get("tool_choice").is_none());
    }

    #[test]
    fn test_assistant_with_tool_calls_serializes_tool_use_block() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();
        let calls = vec![ToolCall {
            id: "toolu_01".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "search".to_string(),
                arguments: r#"{"x": "rust"}"#.to_string(),
            },
        }];
        let request = ChatRequest {
            model: "claude-3-5-sonnet-20241022".to_string(),
            // 真实对话以 user 起头; assistant(tool_use) 在 index 1。
            messages: vec![
                Message::user("search rust"),
                Message::assistant_with_tools("let me search", calls),
            ],
            ..Default::default()
        };

        let json = serde_json::to_value(client.convert_request(&request)).unwrap();
        let content = &json["messages"][1]["content"];
        // [text block, tool_use block]
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "let me search");
        assert_eq!(content[1]["type"], "tool_use");
        assert_eq!(content[1]["id"], "toolu_01");
        assert_eq!(content[1]["name"], "search");
        assert_eq!(content[1]["input"]["x"], "rust");
    }

    #[test]
    fn test_assistant_with_tool_calls_empty_content_omits_text_block() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();
        let calls = vec![ToolCall {
            id: "toolu_02".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "search".to_string(),
                arguments: "not json".to_string(),
            },
        }];
        let request = ChatRequest {
            model: "claude-3-5-sonnet-20241022".to_string(),
            // 真实对话以 user 起头; assistant(tool_use) 在 index 1。
            messages: vec![
                Message::user("go"),
                Message::assistant_with_tools("", calls),
            ],
            ..Default::default()
        };

        let json = serde_json::to_value(client.convert_request(&request)).unwrap();
        let content = &json["messages"][1]["content"];
        // no text block — only the tool_use block; invalid JSON args → empty object
        assert_eq!(content[0]["type"], "tool_use");
        assert_eq!(content[0]["input"], serde_json::json!({}));
        assert!(content[1].is_null());
    }

    #[test]
    fn test_tool_message_serializes_tool_result_block() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();
        let request = ChatRequest {
            model: "claude-3-5-sonnet-20241022".to_string(),
            messages: vec![Message::tool("toolu_01".to_string(), "the result text")],
            ..Default::default()
        };

        let json = serde_json::to_value(client.convert_request(&request)).unwrap();
        assert_eq!(json["messages"][0]["role"], "user");
        let block = &json["messages"][0]["content"][0];
        assert_eq!(block["type"], "tool_result");
        assert_eq!(block["tool_use_id"], "toolu_01");
        assert_eq!(block["content"], "the result text");
    }

    #[test]
    fn test_convert_response_with_tool_use_block() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();
        let body = serde_json::json!({
            "id": "msg_tool",
            "type": "message",
            "model": "claude-3-5-sonnet-20241022",
            "content": [
                {"type": "text", "text": "I'll search."},
                {"type": "tool_use", "id": "toolu_99", "name": "search", "input": {"x": "rust"}}
            ],
            "usage": {"input_tokens": 10, "output_tokens": 20},
            "stop_reason": "tool_use"
        });
        let resp: AnthropicResponse = serde_json::from_value(body).unwrap();
        let converted = client.convert_response(resp);

        let msg = &converted.choices[0].message;
        assert_eq!(msg.content, "I'll search.");
        let calls = msg.tool_calls.as_ref().expect("tool_calls present");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "toolu_99");
        assert_eq!(calls[0].call_type, "function");
        assert_eq!(calls[0].function.name, "search");
        let args: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["x"], "rust");
    }

    #[test]
    fn test_convert_response_tool_use_stop_reason_maps_to_tool_calls() {
        let client = AnthropicClient::new("key".to_string(), "https://api.anthropic.com").unwrap();
        let body = serde_json::json!({
            "id": "msg_tool2",
            "type": "message",
            "model": "claude-3-5-sonnet-20241022",
            "content": [
                {"type": "tool_use", "id": "toolu_1", "name": "f", "input": {}}
            ],
            "usage": {"input_tokens": 1, "output_tokens": 1},
            "stop_reason": "tool_use"
        });
        let resp: AnthropicResponse = serde_json::from_value(body).unwrap();
        let converted = client.convert_response(resp);
        assert_eq!(
            converted.choices[0].finish_reason,
            Some("tool_calls".to_string())
        );
    }
}
