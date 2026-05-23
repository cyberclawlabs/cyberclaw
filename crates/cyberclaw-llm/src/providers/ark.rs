//! 火山引擎方舟（Volcengine ARK）客户端
//!
//! ARK 使用 OpenAI 兼容协议，提供国内访问友好的 LLM 服务

use crate::client::LlmClient;
use crate::error::{LlmError, LlmResult};
use crate::types::{ChatChunk, ChatRequest, ChatResponse};
use async_trait::async_trait;
use futures::stream::{Stream, StreamExt};
use reqwest::Client;
use std::time::Duration;

/// ARK 客户端
pub struct ArkClient {
    client: Client,
    api_key: String,
    base_url: String,
}

impl ArkClient {
    /// 创建新的 ARK 客户端
    ///
    /// # Arguments
    ///
    /// * `api_key` - ARK API 密钥
    /// * `base_url` - API 基础 URL（如 https://ark.cn-beijing.volces.com/api/v3）
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
impl LlmClient for ArkClient {
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

        let chat_response: ChatResponse = response.json().await?;
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
        "ark"
    }

    async fn validate_connection(&self) -> LlmResult<()> {
        // 发送一个简单的测试请求
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

        // 只验证认证是否通过，不关心具体返回
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
    fn test_ark_client_creation() {
        let client = ArkClient::new(
            "test-key".to_string(),
            "https://ark.cn-beijing.volces.com/api/v3",
        );
        assert!(client.is_ok());
    }

    #[test]
    fn test_provider_name() {
        let client = ArkClient::new(
            "test-key".to_string(),
            "https://ark.cn-beijing.volces.com/api/v3",
        )
        .unwrap();
        assert_eq!(client.provider(), "ark");
    }
}
