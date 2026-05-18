//! Embedding provider abstraction (Sprint 25).
//!
//! Separate from LlmClient because some providers do only embeddings (text-only)
//! and some only chat. Implementations may implement both traits.

use crate::error::{LlmError, LlmResult};
use async_trait::async_trait;
use serde::Deserialize;

/// Trait for embedding providers.
#[async_trait]
pub trait EmbedClient: Send + Sync + std::fmt::Debug {
    /// Embed a single input string, returning a float vector.
    async fn embed(&self, input: &str) -> LlmResult<Vec<f32>>;
    /// The dimensionality of embeddings produced by this client.
    fn dimension(&self) -> usize;
}

/// A no-op embedding client that always returns an empty vector.
/// Useful for testing or as a placeholder.
#[derive(Debug, Default, Clone)]
pub struct NoopEmbedClient;

#[async_trait]
impl EmbedClient for NoopEmbedClient {
    async fn embed(&self, _input: &str) -> LlmResult<Vec<f32>> {
        Ok(Vec::new())
    }

    fn dimension(&self) -> usize {
        0
    }
}

/// An OpenAI-compatible embedding client that calls `/embeddings`.
/// Works with OpenAI, Azure OpenAI, and any compatible endpoint.
#[derive(Debug, Clone)]
pub struct OpenAiCompatEmbedClient {
    base_url: String,
    api_key: Option<String>,
    model: String,
    dimension: usize,
    http_client: reqwest::Client,
}

impl OpenAiCompatEmbedClient {
    /// Create a new client.
    ///
    /// `dimension` must match what the chosen model produces; it is not
    /// validated against the provider.
    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<String>,
        model: impl Into<String>,
        dimension: usize,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            api_key,
            model: model.into(),
            dimension,
            http_client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl EmbedClient for OpenAiCompatEmbedClient {
    async fn embed(&self, input: &str) -> LlmResult<Vec<f32>> {
        let url = format!("{}/embeddings", self.base_url.trim_end_matches('/'));
        let mut req = self
            .http_client
            .post(&url)
            .json(&serde_json::json!({"model": self.model, "input": input}));
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let message = resp.text().await.unwrap_or_default();
            return Err(LlmError::ApiError { status, message });
        }
        let parsed: EmbeddingResponse = resp
            .json()
            .await
            .map_err(|e| LlmError::InvalidResponse(format!("embed parse: {e}")))?;
        parsed
            .data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .ok_or_else(|| LlmError::InvalidResponse("embed: no data in response".into()))
    }

    fn dimension(&self) -> usize {
        self.dimension
    }
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_embed_returns_empty_and_zero_dim() {
        let client = NoopEmbedClient;
        let result = client.embed("hello").await.unwrap();
        assert!(result.is_empty());
        assert_eq!(client.dimension(), 0);
    }

    #[test]
    fn openai_compat_dimension_matches_constructor() {
        let client = OpenAiCompatEmbedClient::new(
            "https://api.openai.com/v1",
            Some("sk-test".to_string()),
            "text-embedding-3-small",
            1536,
        );
        assert_eq!(client.dimension(), 1536);
    }
}
