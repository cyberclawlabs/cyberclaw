//! MCP Connector integration tests

#[cfg(test)]
mod integration {
    use crate::mcp::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::time::Duration;

    // Mock transport for integration testing
    struct MockMcpServer {
        tools: Vec<McpTool>,
        resources: Vec<McpResource>,
        prompts: Vec<McpPrompt>,
    }

    impl MockMcpServer {
        fn new() -> Self {
            Self {
                tools: vec![
                    McpTool {
                        name: "calculator_add".to_string(),
                        description: "Add two numbers".to_string(),
                        input_schema: json!({
                            "type": "object",
                            "properties": {
                                "a": {"type": "number"},
                                "b": {"type": "number"}
                            },
                            "required": ["a", "b"]
                        }),
                    },
                    McpTool {
                        name: "calculator_multiply".to_string(),
                        description: "Multiply two numbers".to_string(),
                        input_schema: json!({
                            "type": "object",
                            "properties": {
                                "a": {"type": "number"},
                                "b": {"type": "number"}
                            }
                        }),
                    },
                ],
                resources: vec![
                    McpResource {
                        uri: "file:///tmp/test.txt".to_string(),
                        name: "Test File".to_string(),
                        mime_type: Some("text/plain".to_string()),
                        description: Some("A test file".to_string()),
                    },
                    McpResource {
                        uri: "http://example.com/data.json".to_string(),
                        name: "Example Data".to_string(),
                        mime_type: Some("application/json".to_string()),
                        description: None,
                    },
                ],
                prompts: vec![McpPrompt {
                    name: "code_review".to_string(),
                    description: "Review code changes".to_string(),
                    arguments: vec![],
                }],
            }
        }

        fn handle_request(&self, request: McpRequest) -> McpResponse {
            match request.method.as_str() {
                "tools/list" => McpResponse {
                    jsonrpc: "2.0".to_string(),
                    result: Some(json!({
                        "tools": self.tools
                    })),
                    error: None,
                    id: request.id,
                },
                "tools/call" => {
                    let params = request.params.unwrap();
                    let tool_name = params["name"].as_str().unwrap();
                    let args = &params["arguments"];

                    let result = match tool_name {
                        "calculator_add" => {
                            let a = args["a"].as_f64().unwrap();
                            let b = args["b"].as_f64().unwrap();
                            json!({"result": a + b})
                        }
                        "calculator_multiply" => {
                            let a = args["a"].as_f64().unwrap();
                            let b = args["b"].as_f64().unwrap();
                            json!({"result": a * b})
                        }
                        _ => json!({"error": "Unknown tool"}),
                    };

                    McpResponse {
                        jsonrpc: "2.0".to_string(),
                        result: Some(result),
                        error: None,
                        id: request.id,
                    }
                }
                "resources/list" => McpResponse {
                    jsonrpc: "2.0".to_string(),
                    result: Some(json!({
                        "resources": self.resources
                    })),
                    error: None,
                    id: request.id,
                },
                "resources/read" => {
                    let params = request.params.unwrap();
                    let uri = params["uri"].as_str().unwrap();

                    let content = match uri {
                        "file:///tmp/test.txt" => "Test file content",
                        "http://example.com/data.json" => r#"{"key": "value"}"#,
                        _ => "Not found",
                    };

                    McpResponse {
                        jsonrpc: "2.0".to_string(),
                        result: Some(json!({
                            "contents": content
                        })),
                        error: None,
                        id: request.id,
                    }
                }
                "prompts/list" => McpResponse {
                    jsonrpc: "2.0".to_string(),
                    result: Some(json!({
                        "prompts": self.prompts
                    })),
                    error: None,
                    id: request.id,
                },
                _ => McpResponse {
                    jsonrpc: "2.0".to_string(),
                    result: None,
                    error: Some(protocol::McpError::method_not_found(&request.method)),
                    id: request.id,
                },
            }
        }
    }

    struct MockTransport {
        server: MockMcpServer,
    }

    impl MockTransport {
        fn new() -> Self {
            Self {
                server: MockMcpServer::new(),
            }
        }
    }

    #[async_trait]
    impl McpTransport for MockTransport {
        async fn send(&self, request: McpRequest) -> anyhow::Result<McpResponse> {
            Ok(self.server.handle_request(request))
        }

        async fn close(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_client_list_tools() {
        let transport = Box::new(MockTransport::new());
        let client = McpClient::new(transport, Duration::from_secs(30), false);

        let tools = client.list_tools().await.unwrap();

        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "calculator_add");
        assert_eq!(tools[1].name, "calculator_multiply");
    }

    #[tokio::test]
    async fn test_client_call_tool() {
        let transport = Box::new(MockTransport::new());
        let client = McpClient::new(transport, Duration::from_secs(30), false);

        let result = client
            .call_tool("calculator_add", json!({"a": 5, "b": 3}))
            .await
            .unwrap();

        assert_eq!(result["result"], 8.0);
    }

    #[tokio::test]
    async fn test_client_list_resources() {
        let transport = Box::new(MockTransport::new());
        let client = McpClient::new(transport, Duration::from_secs(30), false);

        let resources = client.list_resources().await.unwrap();

        assert_eq!(resources.len(), 2);
        assert_eq!(resources[0].uri, "file:///tmp/test.txt");
        assert_eq!(resources[1].uri, "http://example.com/data.json");
    }

    #[tokio::test]
    async fn test_client_read_resource() {
        let transport = Box::new(MockTransport::new());
        let client = McpClient::new(transport, Duration::from_secs(30), false);

        let result = client.read_resource("file:///tmp/test.txt").await.unwrap();

        assert_eq!(result["contents"], "Test file content");
    }

    #[tokio::test]
    async fn test_client_with_cache() {
        let transport = Box::new(MockTransport::new());
        let client = McpClient::new(transport, Duration::from_secs(30), true);

        // First read - should hit server
        let result1 = client.read_resource("file:///tmp/test.txt").await.unwrap();
        assert_eq!(result1["contents"], "Test file content");

        // Second read - should use cache
        let result2 = client.read_resource("file:///tmp/test.txt").await.unwrap();
        assert_eq!(result2["contents"], "Test file content");

        // Clear cache
        client.clear_cache().await;

        // Third read - should hit server again
        let result3 = client.read_resource("file:///tmp/test.txt").await.unwrap();
        assert_eq!(result3["contents"], "Test file content");
    }

    #[test]
    fn test_transport_config_stdio() {
        let config = TransportConfig::Stdio {
            command: "node".to_string(),
            args: vec!["server.js".to_string()],
            workdir: Some("/tmp".to_string()),
        };

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("stdio"));
        assert!(json.contains("node"));
    }

    #[test]
    fn test_transport_config_http() {
        let config = TransportConfig::Http {
            url: "http://localhost:8080/mcp".to_string(),
            headers: std::collections::HashMap::from([(
                "Authorization".to_string(),
                "Bearer token".to_string(),
            )]),
        };

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("http"));
        assert!(json.contains("localhost"));
    }

    #[test]
    fn test_mcp_tool_serialization() {
        let tool = McpTool {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: json!({"type": "object"}),
        };

        let json = serde_json::to_string(&tool).unwrap();
        let parsed: McpTool = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.name, tool.name);
        assert_eq!(parsed.description, tool.description);
    }

    #[test]
    fn test_mcp_resource_serialization() {
        let resource = McpResource {
            uri: "file:///test.txt".to_string(),
            name: "Test".to_string(),
            mime_type: Some("text/plain".to_string()),
            description: Some("Test description".to_string()),
        };

        let json = serde_json::to_string(&resource).unwrap();
        let parsed: McpResource = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.uri, resource.uri);
        assert_eq!(parsed.name, resource.name);
        assert_eq!(parsed.mime_type, resource.mime_type);
    }

    #[tokio::test]
    async fn test_multiple_tool_calls() {
        let transport = Box::new(MockTransport::new());
        let client = McpClient::new(transport, Duration::from_secs(30), false);

        let result1 = client
            .call_tool("calculator_add", json!({"a": 10, "b": 20}))
            .await
            .unwrap();
        assert_eq!(result1["result"], 30.0);

        let result2 = client
            .call_tool("calculator_multiply", json!({"a": 5, "b": 6}))
            .await
            .unwrap();
        assert_eq!(result2["result"], 30.0);
    }

    #[tokio::test]
    async fn test_concurrent_requests() {
        use tokio::task::JoinSet;

        let transport = Box::new(MockTransport::new());
        let client = std::sync::Arc::new(McpClient::new(transport, Duration::from_secs(30), false));

        let mut set = JoinSet::new();

        for i in 0..10 {
            let client_clone = client.clone();
            set.spawn(async move {
                client_clone
                    .call_tool("calculator_add", json!({"a": i, "b": i}))
                    .await
            });
        }

        let mut results = Vec::new();
        while let Some(res) = set.join_next().await {
            results.push(res.unwrap().unwrap());
        }

        assert_eq!(results.len(), 10);
    }

    #[test]
    fn test_request_id_types() {
        use protocol::RequestId;

        let id1 = RequestId::String("test".to_string());
        let id2 = RequestId::Number(42);
        let id3 = RequestId::Null;

        // Test serialization
        assert_eq!(serde_json::to_string(&id1).unwrap(), r#""test""#);
        assert_eq!(serde_json::to_string(&id2).unwrap(), "42");
        assert_eq!(serde_json::to_string(&id3).unwrap(), "null");
    }

    #[test]
    fn test_error_codes() {
        use protocol::McpError;

        let parse_error = McpError::parse_error("Invalid JSON");
        assert_eq!(parse_error.code, -32700);

        let invalid_request = McpError::invalid_request("Missing field");
        assert_eq!(invalid_request.code, -32600);

        let method_not_found = McpError::method_not_found("unknown");
        assert_eq!(method_not_found.code, -32601);

        let invalid_params = McpError::invalid_params("Wrong type");
        assert_eq!(invalid_params.code, -32602);

        let internal_error = McpError::internal_error("Server error");
        assert_eq!(internal_error.code, -32603);
    }

    #[tokio::test]
    async fn test_cache_ttl() {
        let transport = Box::new(MockTransport::new());
        let client = McpClient::new(transport, Duration::from_secs(30), true);

        // Manually insert with short TTL
        {
            let mut cache = client.cache().write().await;
            cache.insert(
                "test://short-ttl".to_string(),
                json!({"data": "test"}),
                Duration::from_millis(50),
            );
        }

        // Should be cached
        {
            let cache = client.cache().read().await;
            assert!(cache.get("test://short-ttl").is_some());
        }

        // Wait for expiry
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Should be expired
        {
            let cache = client.cache().read().await;
            assert!(cache.get("test://short-ttl").is_none());
        }
    }

    #[test]
    fn test_sanitize_resource_uri() {
        use crate::mcp::connector::sanitize_resource_uri;

        assert_eq!(
            sanitize_resource_uri("file:///path/to/file"),
            "file..path.to.file"
        );
        assert_eq!(
            sanitize_resource_uri("https://api.example.com/v1/data"),
            "https.api.example.com.v1.data"
        );
    }

    #[tokio::test]
    async fn test_list_prompts() {
        let transport = Box::new(MockTransport::new());
        let client = McpClient::new(transport, Duration::from_secs(30), false);

        let prompts = client.list_prompts().await.unwrap();

        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].name, "code_review");
    }

    #[test]
    fn test_server_config_defaults() {
        let config = McpServerConfig {
            name: "test".to_string(),
            transport: TransportConfig::Http {
                url: "http://localhost".to_string(),
                headers: std::collections::HashMap::new(),
            },
            timeout: Duration::from_secs(30),
            enable_cache: true,
        };

        assert_eq!(config.timeout, Duration::from_secs(30));
        assert!(config.enable_cache);
    }

    #[tokio::test]
    async fn test_error_handling() {
        use protocol::{McpError, McpResponse, RequestId};

        let error_response = McpResponse {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(McpError::internal_error("Server crashed")),
            id: RequestId::Number(1),
        };

        let result = error_response.into_result();
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert_eq!(err.code, -32603);
        assert!(err.message.contains("crashed"));
    }

    /// BT-36 — full Connector::execute path for `mcp.list_resources`.
    /// Uses the in-process MockMcpServer so the test does not need a
    /// real MCP server reachable over HTTP.
    #[tokio::test]
    async fn test_connector_execute_list_resources_bt36() {
        use crate::types::{CapabilityExecutionRequest, Connector};
        use cyberclaw_core::identity::{ActorRef, ActorType};
        use cyberclaw_core::ids::{ActorId, CapabilityId, ConnectorId, ExecutionId, WorkspaceId};
        use cyberclaw_core::workspace::{WorkspaceMode, WorkspaceRef};

        let config = McpServerConfig {
            name: "bt36".to_string(),
            transport: TransportConfig::Http {
                url: "http://unused".to_string(),
                headers: std::collections::HashMap::new(),
            },
            timeout: Duration::from_secs(10),
            enable_cache: false,
        };
        let transport: Box<dyn McpTransport> = Box::new(MockTransport::new());
        let connector = McpConnector::new_with_transport_for_test(config, transport)
            .await
            .expect("construct connector");

        // Confirm the capability is registered (the connector advertised it).
        let caps = connector.capabilities();
        assert!(
            caps.iter().any(|c| c.id == "mcp.list_resources"),
            "mcp.list_resources must be advertised by McpConnector"
        );

        let req = CapabilityExecutionRequest {
            execution_id: ExecutionId::new(),
            trace_id: "bt36-trace".to_string(),
            actor: ActorRef {
                id: ActorId::new(),
                actor_type: ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "test".to_string(),
            },
            workspace: WorkspaceRef {
                id: WorkspaceId::new(),
                mode: WorkspaceMode::Ephemeral,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: "/tmp/bt36".to_string(),
                writable_roots: vec![],
            },
            connector_id: ConnectorId::from_string("mcp-bt36".to_string())
                .expect("valid connector id"),
            capability_id: CapabilityId::from_string("mcp.list_resources".to_string())
                .expect("valid capability id"),
            input: json!({}),
        };

        let result = connector.execute(req).await.expect("execute ok");
        let arr = result.output.as_array().expect("output is array");
        assert_eq!(arr.len(), 2, "MockMcpServer exposes 2 resources");
        let uris: Vec<&str> = arr
            .iter()
            .filter_map(|v| v.get("uri").and_then(|u| u.as_str()))
            .collect();
        assert!(uris.contains(&"file:///tmp/test.txt"));
        assert!(uris.contains(&"http://example.com/data.json"));
    }
}
