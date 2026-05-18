//! MCP client implementation

use super::protocol::{McpRequest, McpResponse, RequestId};
use super::transport::McpTransport;
use super::types::{McpCache, McpPrompt, McpResource, McpTool};
use anyhow::{Context, Result};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// MCP client
pub struct McpClient {
    /// Transport layer
    transport: Arc<RwLock<Box<dyn McpTransport>>>,
    /// Request counter for generating IDs
    request_counter: AtomicU64,
    /// Response cache
    cache: Arc<RwLock<McpCache>>,
    /// Request timeout
    timeout: Duration,
    /// Enable caching
    enable_cache: bool,
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient")
            .field("request_counter", &self.request_counter)
            .field("timeout", &self.timeout)
            .field("enable_cache", &self.enable_cache)
            .finish_non_exhaustive()
    }
}

impl McpClient {
    /// Create a new MCP client
    pub fn new(transport: Box<dyn McpTransport>, timeout: Duration, enable_cache: bool) -> Self {
        Self {
            transport: Arc::new(RwLock::new(transport)),
            request_counter: AtomicU64::new(1),
            cache: Arc::new(RwLock::new(McpCache::new())),
            timeout,
            enable_cache,
        }
    }

    /// Generate a unique request ID
    fn next_request_id(&self) -> RequestId {
        let id = self.request_counter.fetch_add(1, Ordering::SeqCst);
        RequestId::Number(id as i64)
    }

    /// Send a request and wait for response with timeout
    async fn send_request(&self, request: McpRequest) -> Result<McpResponse> {
        let transport = self.transport.read().await;

        let response = tokio::time::timeout(self.timeout, transport.send(request))
            .await
            .context("MCP request timed out")??;

        Ok(response)
    }

    /// List available tools
    pub async fn list_tools(&self) -> Result<Vec<McpTool>> {
        let request = McpRequest::tools_list(self.next_request_id());
        let response = self.send_request(request).await?;

        let result = response.into_result()?;

        let tools: Vec<McpTool> = serde_json::from_value(result["tools"].clone())
            .context("Failed to parse tools list")?;

        Ok(tools)
    }

    /// Call a tool
    pub async fn call_tool(
        &self,
        tool_name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let request = McpRequest::tools_call(tool_name.into(), arguments, self.next_request_id());
        let response = self.send_request(request).await?;

        let result = response.into_result()?;
        Ok(result)
    }

    /// List available resources
    pub async fn list_resources(&self) -> Result<Vec<McpResource>> {
        let request = McpRequest::resources_list(self.next_request_id());
        let response = self.send_request(request).await?;

        let result = response.into_result()?;

        let resources: Vec<McpResource> = serde_json::from_value(result["resources"].clone())
            .context("Failed to parse resources list")?;

        Ok(resources)
    }

    /// Read a resource
    pub async fn read_resource(&self, uri: impl Into<String>) -> Result<serde_json::Value> {
        let uri = uri.into();

        // Check cache first
        if self.enable_cache {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(&uri) {
                tracing::debug!("Cache hit for resource: {}", uri);
                return Ok(cached.clone());
            }
        }

        // Fetch from server
        let request = McpRequest::resources_read(&uri, self.next_request_id());
        let response = self.send_request(request).await?;

        let result = response.into_result()?;

        // Cache the result
        if self.enable_cache {
            let mut cache = self.cache.write().await;
            cache.insert(uri.clone(), result.clone(), Duration::from_secs(300));
            // 5 min TTL
        }

        Ok(result)
    }

    /// List available prompts
    pub async fn list_prompts(&self) -> Result<Vec<McpPrompt>> {
        let request = McpRequest::prompts_list(self.next_request_id());
        let response = self.send_request(request).await?;

        let result = response.into_result()?;

        let prompts: Vec<McpPrompt> = serde_json::from_value(result["prompts"].clone())
            .context("Failed to parse prompts list")?;

        Ok(prompts)
    }

    /// Get a prompt
    pub async fn get_prompt(
        &self,
        prompt_name: impl Into<String>,
        arguments: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let request =
            McpRequest::prompts_get(prompt_name.into(), arguments, self.next_request_id());
        let response = self.send_request(request).await?;

        let result = response.into_result()?;
        Ok(result)
    }

    /// Clear cache
    pub async fn clear_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
    }

    /// Cleanup expired cache entries
    pub async fn cleanup_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.cleanup();
    }

    /// Get cache reference (test-only)
    #[cfg(test)]
    pub(crate) fn cache(&self) -> &Arc<RwLock<McpCache>> {
        &self.cache
    }

    /// Close the client
    pub async fn close(&mut self) -> Result<()> {
        let mut transport = self.transport.write().await;
        transport.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::protocol::McpError;
    use async_trait::async_trait;

    /// Mock transport for testing
    struct MockTransport {
        responses: Vec<McpResponse>,
        current: std::sync::atomic::AtomicUsize,
    }

    impl MockTransport {
        fn new(responses: Vec<McpResponse>) -> Self {
            Self {
                responses,
                current: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl McpTransport for MockTransport {
        async fn send(&self, _request: McpRequest) -> Result<McpResponse> {
            let idx = self.current.fetch_add(1, Ordering::SeqCst);
            Ok(self.responses[idx % self.responses.len()].clone())
        }

        async fn close(&mut self) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_list_tools() {
        let mock_response = McpResponse {
            jsonrpc: "2.0".to_string(),
            result: Some(serde_json::json!({
                "tools": [
                    {
                        "name": "test_tool",
                        "description": "A test tool",
                        "input_schema": {"type": "object"}
                    }
                ]
            })),
            error: None,
            id: RequestId::Number(1),
        };

        let transport = Box::new(MockTransport::new(vec![mock_response]));
        let client = McpClient::new(transport, Duration::from_secs(30), false);

        let tools = client.list_tools().await.unwrap();

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "test_tool");
        assert_eq!(tools[0].description, "A test tool");
    }

    #[tokio::test]
    async fn test_call_tool() {
        let mock_response = McpResponse {
            jsonrpc: "2.0".to_string(),
            result: Some(serde_json::json!({"output": "success"})),
            error: None,
            id: RequestId::Number(1),
        };

        let transport = Box::new(MockTransport::new(vec![mock_response]));
        let client = McpClient::new(transport, Duration::from_secs(30), false);

        let result = client
            .call_tool("test_tool", serde_json::json!({"input": "test"}))
            .await
            .unwrap();

        assert_eq!(result["output"], "success");
    }

    #[tokio::test]
    async fn test_error_response() {
        let mock_response = McpResponse {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(McpError::method_not_found("unknown_method")),
            id: RequestId::Number(1),
        };

        let transport = Box::new(MockTransport::new(vec![mock_response]));
        let client = McpClient::new(transport, Duration::from_secs(30), false);

        let result = client.list_tools().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cache() {
        let mock_response = McpResponse {
            jsonrpc: "2.0".to_string(),
            result: Some(serde_json::json!({"content": "cached_data"})),
            error: None,
            id: RequestId::Number(1),
        };

        let transport = Box::new(MockTransport::new(vec![mock_response]));
        let client = McpClient::new(transport, Duration::from_secs(30), true);

        // First call - should fetch from server
        let result1 = client.read_resource("test://resource").await.unwrap();
        assert_eq!(result1["content"], "cached_data");

        // Second call - should use cache (transport won't be called again)
        let result2 = client.read_resource("test://resource").await.unwrap();
        assert_eq!(result2["content"], "cached_data");
    }

    #[tokio::test]
    async fn test_clear_cache() {
        let client = McpClient::new(
            Box::new(MockTransport::new(vec![])),
            Duration::from_secs(30),
            true,
        );

        // Manually add to cache
        {
            let mut cache = client.cache().write().await;
            cache.insert(
                "test://resource".to_string(),
                serde_json::json!({"test": true}),
                Duration::from_secs(60),
            );
        }

        // Verify cached
        {
            let cache = client.cache().read().await;
            assert!(cache.get("test://resource").is_some());
        }

        // Clear cache
        client.clear_cache().await;

        // Verify cleared
        {
            let cache = client.cache().read().await;
            assert!(cache.get("test://resource").is_none());
        }
    }
}
