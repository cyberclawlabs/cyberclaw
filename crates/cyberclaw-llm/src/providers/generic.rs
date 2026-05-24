//! 通用 OpenAI 兼容客户端
//!
//! 支持 DeepSeek、Ollama、LocalAI 等 OpenAI 兼容接口

use crate::client::LlmClient;
use crate::error::{LlmError, LlmResult};
use crate::types::{ChatChunk, ChatRequest, ChatResponse};
use async_trait::async_trait;
use futures::stream::{Stream, StreamExt};
use reqwest::Client;
use std::time::Duration;

/// 通用 OpenAI 兼容客户端
pub struct GenericOpenAiClient {
    client: Client,
    api_key: Option<String>,
    base_url: String,
}

impl GenericOpenAiClient {
    /// 创建新的通用客户端
    ///
    /// # Arguments
    ///
    /// * `api_key` - API 密钥（可选,某些本地服务不需要）
    /// * `base_url` - API 基础 URL
    pub fn new(api_key: Option<String>, base_url: &str) -> LlmResult<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(120)) // 本地服务可能较慢
            .no_proxy() // 直接连接，不经过系统代理（避免本地/localhost 地址被代理拦截）
            .build()
            .map_err(|e| LlmError::Internal(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            client,
            api_key,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }
}

/// Encode a CyberClaw tool name (which uses `dot.notation` like `cmd.run`)
/// into something that satisfies the strict OpenAI / DeepSeek tool-name regex
/// `^[a-zA-Z0-9_-]+$`. We map `.` to `__` (double underscore) — reversible
/// because real CyberClaw tool ids never contain `__`.
///
/// MiniMax accepts `.` directly; DeepSeek and OpenAI reject it with a 400
/// validation error. Doing the substitution here keeps the rest of the
/// codebase using the canonical dotted names.
fn encode_tool_name(name: &str) -> String {
    name.replace('.', "__")
}

/// Reverse of `encode_tool_name`. Applied to every tool_call returned by the
/// LLM so downstream dispatch sees the canonical dotted name.
fn decode_tool_name(name: &str) -> String {
    name.replace("__", ".")
}

/// Normalize the messages array so all system-role messages are merged into
/// a single system message at index 0.
///
/// MiniMax (and some other OpenAI-compatible providers) reject requests that
/// contain a `system` role message anywhere other than the very first position
/// (error 2013: "invalid message role: system" mid-array). The agentic loop
/// may append `Message::system(...)` to the tail via `add_system_hint` and
/// the GAP-4 nudge, so we normalize before sending.
///
/// Algorithm:
/// 1. Collect all system messages; join their content with `\n\n`.
/// 2. Place one merged system message at index 0.
/// 3. Append all non-system messages in their original relative order.
fn merge_system_messages(messages: Vec<crate::types::Message>) -> Vec<crate::types::Message> {
    use crate::types::Role;
    let mut system_parts: Vec<String> = Vec::new();
    let mut non_system: Vec<crate::types::Message> = Vec::new();
    for msg in messages {
        if msg.role == Role::System {
            system_parts.push(msg.content.clone());
        } else {
            non_system.push(msg);
        }
    }
    let mut out = Vec::with_capacity(non_system.len() + 1);
    if !system_parts.is_empty() {
        out.push(crate::types::Message::system(system_parts.join("\n\n")));
    }
    out.extend(non_system);
    out
}

#[async_trait]
impl LlmClient for GenericOpenAiClient {
    async fn chat_completion(&self, mut request: ChatRequest) -> LlmResult<ChatResponse> {
        let url = format!("{}/chat/completions", self.base_url);

        // Sanitize outbound tool names (DeepSeek / OpenAI reject dots).
        if let Some(ref mut tools) = request.tools {
            for t in tools.iter_mut() {
                t.function.name = encode_tool_name(&t.function.name);
            }
        }

        // BUG-CB-17: merge all system-role messages into one at index 0.
        // MiniMax rejects system messages appearing mid-array (error 2013).
        request.messages = merge_system_messages(request.messages);

        let mut req_builder = self
            .client
            .post(&url)
            .header("Content-Type", "application/json");

        // 添加 API Key（如果有）
        if let Some(ref api_key) = self.api_key {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = req_builder.json(&request).send().await?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError {
                status: status.as_u16(),
                message: error_text,
            });
        }

        let mut chat_response: ChatResponse = response.json().await?;
        // Reverse-map any tool_call names so downstream sees canonical dotted ids.
        for choice in chat_response.choices.iter_mut() {
            if let Some(ref mut tool_calls) = choice.message.tool_calls {
                for tc in tool_calls.iter_mut() {
                    tc.function.name = decode_tool_name(&tc.function.name);
                }
            }
        }
        Ok(chat_response)
    }

    async fn chat_completion_stream(
        &self,
        mut request: ChatRequest,
    ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>> {
        request.stream = Some(true);
        // Same outbound sanitization as the non-streaming path.
        if let Some(ref mut tools) = request.tools {
            for t in tools.iter_mut() {
                t.function.name = encode_tool_name(&t.function.name);
            }
        }
        // BUG-CB-17: merge all system-role messages into one at index 0.
        request.messages = merge_system_messages(request.messages);
        let url = format!("{}/chat/completions", self.base_url);

        let mut req_builder = self
            .client
            .post(&url)
            .header("Content-Type", "application/json");

        if let Some(ref api_key) = self.api_key {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = req_builder.json(&request).send().await?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError {
                status: status.as_u16(),
                message: error_text,
            });
        }

        // Proper SSE buffer: TCP chunks can carry multiple `data: ...\n\n` events
        // or split one event across chunks. Naive 1-chunk-1-event parse breaks
        // streaming on real providers (e.g. DeepSeek) — produces "trailing
        // characters" errors and drops most tokens.
        let (tx, rx) = tokio::sync::mpsc::channel::<LlmResult<ChatChunk>>(64);
        tokio::spawn(async move {
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
                        // split on event boundary "\n\n"
                        while let Some(pos) = buffer.find("\n\n") {
                            let event_block = buffer[..pos].to_string();
                            buffer.drain(..pos + 2);
                            for line in event_block.lines() {
                                let line = line.trim();
                                if let Some(json_str) = line.strip_prefix("data:") {
                                    let json_str = json_str.trim();
                                    if json_str.is_empty() {
                                        continue;
                                    }
                                    if json_str == "[DONE]" {
                                        return;
                                    }
                                    match serde_json::from_str::<ChatChunk>(json_str) {
                                        Ok(chunk) => {
                                            if tx.send(Ok(chunk)).await.is_err() {
                                                return;
                                            }
                                        }
                                        Err(e) => {
                                            if tx.send(Err(LlmError::JsonError(e))).await.is_err() {
                                                return;
                                            }
                                        }
                                    }
                                }
                                // 忽略 event: / id: / : 注释行 / 空行
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
        "generic"
    }

    async fn validate_connection(&self) -> LlmResult<()> {
        // 简单的健康检查
        let request = ChatRequest {
            model: "test".to_string(),
            messages: vec![crate::types::Message::user("test")],
            temperature: None,
            top_p: None,
            max_tokens: Some(1),
            tools: None,
            tool_choice: None,
            stream: None,
            extra: Default::default(),
            api_key_override: None,
        };

        let url = format!("{}/chat/completions", self.base_url);
        let mut req_builder = self
            .client
            .post(&url)
            .header("Content-Type", "application/json");

        if let Some(ref api_key) = self.api_key {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = req_builder.json(&request).send().await?;

        let status = response.status();
        if status == 401 || status == 403 {
            return Err(LlmError::ApiError {
                status: status.as_u16(),
                message: "Authentication failed".to_string(),
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Message, Role};

    #[test]
    fn test_generic_client_creation() {
        let client =
            GenericOpenAiClient::new(Some("test-key".to_string()), "http://localhost:11434/v1");
        assert!(client.is_ok());
    }

    #[test]
    fn test_generic_client_without_key() {
        let client = GenericOpenAiClient::new(None, "http://localhost:11434/v1");
        assert!(client.is_ok());
    }

    #[test]
    fn test_provider_name() {
        let client = GenericOpenAiClient::new(None, "http://localhost:11434/v1").unwrap();
        assert_eq!(client.provider(), "generic");
    }

    // BUG-CB-17 tests -----------------------------------------------------------

    #[test]
    fn test_merge_system_messages_combines_multiple_system_into_first_position() {
        let messages = vec![
            Message::system("first system"),
            Message::user("hello"),
            Message::system("second system appended by add_system_hint"),
        ];
        let out = merge_system_messages(messages);
        assert_eq!(out.len(), 2, "merged: 1 system + 1 user");
        assert_eq!(out[0].role, Role::System);
        assert_eq!(
            out[0].content,
            "first system\n\nsecond system appended by add_system_hint"
        );
        assert_eq!(out[1].role, Role::User);
        assert_eq!(out[1].content, "hello");
    }

    #[test]
    fn test_merge_system_messages_preserves_non_system_order() {
        let messages = vec![
            Message::system("sys"),
            Message::user("u1"),
            Message::assistant("a1"),
            Message::user("u2"),
        ];
        let out = merge_system_messages(messages);
        assert_eq!(out.len(), 4);
        assert_eq!(out[0].role, Role::System);
        assert_eq!(out[1].role, Role::User);
        assert_eq!(out[1].content, "u1");
        assert_eq!(out[2].role, Role::Assistant);
        assert_eq!(out[3].role, Role::User);
        assert_eq!(out[3].content, "u2");
    }

    #[test]
    fn test_merge_system_messages_no_system_returns_unchanged() {
        let messages = vec![Message::user("hello"), Message::assistant("world")];
        let out = merge_system_messages(messages);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].role, Role::User);
        assert_eq!(out[1].role, Role::Assistant);
    }
}
