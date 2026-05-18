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
                                            if tx
                                                .send(Err(LlmError::JsonError(e)))
                                                .await
                                                .is_err()
                                            {
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
}
