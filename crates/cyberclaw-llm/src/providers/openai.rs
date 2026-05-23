//! OpenAI 客户端

use crate::client::LlmClient;
use crate::error::{LlmError, LlmResult};
use crate::rate_limit_tracker::RateLimitSnapshot;
use crate::types::{ChatChunk, ChatRequest, ChatResponse};
use async_trait::async_trait;
use futures::stream::{Stream, StreamExt};
use reqwest::Client;
use std::time::Duration;

/// P1.1 — 从 OpenAI 风格 response 提取 `usage.prompt_tokens_details.cached_tokens`
/// 并映射到我们内部 `Usage.cache_read_input_tokens`。
///
/// OpenAI auto cache >1024 tokens 时不需要 explicit marker，命中
/// 通过 `prompt_tokens_details.cached_tokens` 反映。
fn enrich_openai_cache_tokens(body: &[u8]) -> Option<u32> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v.get("usage")?
        .get("prompt_tokens_details")?
        .get("cached_tokens")?
        .as_u64()
        .map(|n| n as u32)
}

/// OpenAI 客户端
pub struct OpenAiClient {
    client: Client,
    api_key: String,
    base_url: String,
}

impl OpenAiClient {
    /// 创建新的 OpenAI 客户端
    ///
    /// # Arguments
    ///
    /// * `api_key` - OpenAI API 密钥
    /// * `base_url` - API 基础 URL（通常为 https://api.openai.com/v1）
    pub fn new(api_key: String, base_url: &str) -> LlmResult<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| LlmError::Internal(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            client,
            api_key,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }
}

#[async_trait]
impl LlmClient for OpenAiClient {
    async fn chat_completion(&self, request: ChatRequest) -> LlmResult<ChatResponse> {
        let url = format!("{}/chat/completions", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError {
                status: status.as_u16(),
                message: error_text,
            });
        }

        // Capture rate-limit headers before consuming the response body.
        let rate_limit = RateLimitSnapshot::from_headers(response.headers(), self.provider());

        // P1.1 — 读为 bytes 后做两次解析：标准 ChatResponse + OpenAI
        // 特有的 `usage.prompt_tokens_details.cached_tokens` 提取，
        // 填回 ChatResponse.usage.cache_read_input_tokens。
        let body = response.bytes().await?;
        let mut chat_response: ChatResponse = serde_json::from_slice(&body)?;
        if let Some(usage) = chat_response.usage.as_mut() {
            if usage.cache_read_input_tokens.is_none() {
                if let Some(cached) = enrich_openai_cache_tokens(&body) {
                    usage.cache_read_input_tokens = Some(cached);
                }
            }
        }
        chat_response.rate_limit = rate_limit;
        Ok(chat_response)
    }

    async fn chat_completion_stream(
        &self,
        mut request: ChatRequest,
    ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>> {
        request.stream = Some(true);
        let url = format!("{}/chat/completions", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError {
                status: status.as_u16(),
                message: error_text,
            });
        }

        let stream = response
            .bytes_stream()
            .map(|result| {
                result.map_err(LlmError::HttpError).and_then(|bytes| {
                    let text = String::from_utf8_lossy(&bytes);
                    // SSE 格式: data: {...}\n\n
                    if text.starts_with("data: ") {
                        let json_str = text.trim_start_matches("data: ").trim();
                        if json_str == "[DONE]" {
                            // 流结束标记
                            return Err(LlmError::Internal("Stream ended".to_string()));
                        }
                        serde_json::from_str::<ChatChunk>(json_str).map_err(LlmError::JsonError)
                    } else {
                        Err(LlmError::InvalidResponse("Invalid SSE format".to_string()))
                    }
                })
            })
            .filter(|result| {
                let should_keep = !matches!(
                    result,
                    Err(LlmError::Internal(msg)) if msg == "Stream ended"
                );
                async move { should_keep }
            });

        Ok(Box::new(Box::pin(stream)))
    }

    fn provider(&self) -> &str {
        "openai"
    }

    async fn validate_connection(&self) -> LlmResult<()> {
        // 发送一个简单的测试请求
        let request = ChatRequest {
            model: "gpt-3.5-turbo".to_string(),
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
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

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
    fn test_openai_client_creation() {
        let client = OpenAiClient::new("sk-test".to_string(), "https://api.openai.com/v1");
        assert!(client.is_ok());
    }

    #[test]
    fn test_provider_name() {
        let client = OpenAiClient::new("sk-test".to_string(), "https://api.openai.com/v1").unwrap();
        assert_eq!(client.provider(), "openai");
    }
}
