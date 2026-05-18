#[cfg(test)]
mod connector_tests {
    use super::super::*;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::task::JoinHandle;

    async fn start_mock_http_server() -> (String, JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let addr = listener.local_addr().expect("server addr");

        let handle = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buffer = vec![0_u8; 4096];
                    let read_size = socket.read(&mut buffer).await.unwrap_or(0);
                    if read_size == 0 {
                        return;
                    }

                    let request = String::from_utf8_lossy(&buffer[..read_size]);
                    let first_line = request.lines().next().unwrap_or_default();

                    let (status, content_type, body) = if first_line.contains("GET /fetch ") {
                        (
                            "200 OK",
                            "text/plain; charset=utf-8",
                            "mock fetch response".to_string(),
                        )
                    } else if first_line.contains("GET /search?")
                        || first_line.contains("GET /search ")
                    {
                        (
                            "200 OK",
                            "application/json",
                            r#"{
                                "results": [
                                    {"title":"CyberClaw Docs","url":"https://example.com/docs","snippet":"Documentation"},
                                    {"title":"CyberClaw Repo","url":"https://example.com/repo","snippet":"Repository"}
                                ]
                            }"#
                            .to_string(),
                        )
                    } else {
                        ("404 Not Found", "text/plain", "not found".to_string())
                    };

                    let response = format!(
                        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        status,
                        content_type,
                        body.len(),
                        body
                    );

                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });

        (format!("http://{}", addr), handle)
    }

    /// Test ConnectorRegistry registration and retrieval
    #[tokio::test]
    async fn test_connector_registry() {
        let temp_dir = TempDir::new().unwrap();
        let workspace = temp_dir.path().to_path_buf();

        let registry = ConnectorRegistry::new();
        let connector: Arc<dyn Connector> = Arc::new(LocalConnector::new(workspace.clone()));

        // Register connector
        registry.register(connector).unwrap();

        // Verify connector is registered
        let connectors = registry.list_connectors();
        assert_eq!(connectors.len(), 1);
        assert!(connectors[0].as_str().contains("local"));

        // Verify capability lookup
        let cap_id = cyberclaw_core::ids::CapabilityId::from_string("fs.read".to_string()).unwrap();
        let found = registry.find_connector_for_capability(&cap_id);
        assert!(found.is_some());
        assert!(found.unwrap().as_str().contains("local"));
    }

    /// Test CapabilityDispatcher dispatch
    #[tokio::test]
    async fn test_capability_dispatcher() {
        let temp_dir = TempDir::new().unwrap();
        let workspace = temp_dir.path().to_path_buf();

        // Setup registry and connector
        let registry = Arc::new(ConnectorRegistry::new());
        let connector: Arc<dyn Connector> = Arc::new(LocalConnector::new(workspace.clone()));
        registry.register(connector).unwrap();

        // Create dispatcher
        let dispatcher = CapabilityDispatcher::new(registry);

        // Create a test file
        let test_file = workspace.join("test.txt");
        std::fs::write(&test_file, "Hello, World!").unwrap();

        // Create request for fs.read
        let request = CapabilityExecutionRequest {
            execution_id: cyberclaw_core::ids::ExecutionId::new(),
            trace_id: uuid::Uuid::new_v4().to_string(),
            actor: cyberclaw_core::identity::ActorRef {
                id: cyberclaw_core::ids::ActorId::new(),
                actor_type: cyberclaw_core::identity::ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "Test".to_string(),
            },
            workspace: cyberclaw_core::workspace::WorkspaceRef {
                id: cyberclaw_core::ids::WorkspaceId::new(),
                mode: cyberclaw_core::workspace::WorkspaceMode::Isolated,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: workspace.to_str().unwrap().to_string(),
                writable_roots: vec![],
            },
            connector_id: cyberclaw_core::ids::ConnectorId::from_string("local".to_string())
                .unwrap(),
            capability_id: cyberclaw_core::ids::CapabilityId::from_string("fs.read".to_string())
                .unwrap(),
            input: serde_json::json!({
                "path": "test.txt"
            }),
        };

        // Dispatch request
        let result = dispatcher.dispatch(request).await.unwrap();
        assert!(matches!(result.status, ExecutionStatus::Success));

        // Verify output
        let output: crate::types::FsReadOutput = serde_json::from_value(result.output).unwrap();
        assert_eq!(output.content, "Hello, World!");
        assert!(!output.truncated);
    }

    /// Test LocalConnector fs.read capability
    #[tokio::test]
    async fn test_local_connector_fs_read() {
        let temp_dir = TempDir::new().unwrap();
        let workspace = temp_dir.path().to_path_buf();
        let connector = LocalConnector::new(workspace.clone());

        // Create test file
        let test_file = workspace.join("test.txt");
        std::fs::write(&test_file, "Test content").unwrap();

        let request = CapabilityExecutionRequest {
            execution_id: cyberclaw_core::ids::ExecutionId::new(),
            trace_id: uuid::Uuid::new_v4().to_string(),
            actor: cyberclaw_core::identity::ActorRef {
                id: cyberclaw_core::ids::ActorId::new(),
                actor_type: cyberclaw_core::identity::ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "Test".to_string(),
            },
            workspace: cyberclaw_core::workspace::WorkspaceRef {
                id: cyberclaw_core::ids::WorkspaceId::new(),
                mode: cyberclaw_core::workspace::WorkspaceMode::Isolated,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: workspace.to_str().unwrap().to_string(),
                writable_roots: vec![],
            },
            connector_id: cyberclaw_core::ids::ConnectorId::from_string("local".to_string())
                .unwrap(),
            capability_id: cyberclaw_core::ids::CapabilityId::from_string("fs.read".to_string())
                .unwrap(),
            input: serde_json::json!({
                "path": "test.txt"
            }),
        };

        let result = connector.execute(request).await.unwrap();
        assert!(matches!(result.status, ExecutionStatus::Success));

        let output: crate::types::FsReadOutput = serde_json::from_value(result.output).unwrap();
        assert_eq!(output.content, "Test content");
    }

    /// Test LocalConnector fs.write capability
    #[tokio::test]
    async fn test_local_connector_fs_write() {
        let temp_dir = TempDir::new().unwrap();
        let workspace = temp_dir.path().to_path_buf();
        let connector = LocalConnector::new(workspace.clone());

        let request = CapabilityExecutionRequest {
            execution_id: cyberclaw_core::ids::ExecutionId::new(),
            trace_id: uuid::Uuid::new_v4().to_string(),
            actor: cyberclaw_core::identity::ActorRef {
                id: cyberclaw_core::ids::ActorId::new(),
                actor_type: cyberclaw_core::identity::ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "Test".to_string(),
            },
            workspace: cyberclaw_core::workspace::WorkspaceRef {
                id: cyberclaw_core::ids::WorkspaceId::new(),
                mode: cyberclaw_core::workspace::WorkspaceMode::Isolated,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: workspace.to_str().unwrap().to_string(),
                writable_roots: vec![],
            },
            connector_id: cyberclaw_core::ids::ConnectorId::from_string("local".to_string())
                .unwrap(),
            capability_id: cyberclaw_core::ids::CapabilityId::from_string("fs.write".to_string())
                .unwrap(),
            input: serde_json::json!({
                "path": "output.txt",
                "content": "Written content"
            }),
        };

        let result = connector.execute(request).await.unwrap();
        if !matches!(result.status, ExecutionStatus::Success) {
            eprintln!("fs.write test failed with error: {:?}", result.error);
            eprintln!("Output: {:?}", result.output);
        }
        assert!(matches!(result.status, ExecutionStatus::Success));

        // Verify file was written
        let written = std::fs::read_to_string(workspace.join("output.txt")).unwrap();
        assert_eq!(written, "Written content");
    }

    /// Test workspace boundary protection
    #[tokio::test]
    async fn test_workspace_boundary_protection() {
        let temp_dir = TempDir::new().unwrap();
        let workspace = temp_dir.path().to_path_buf();
        let connector = LocalConnector::new(workspace.clone());

        // Try to read outside workspace
        let request = CapabilityExecutionRequest {
            execution_id: cyberclaw_core::ids::ExecutionId::new(),
            trace_id: uuid::Uuid::new_v4().to_string(),
            actor: cyberclaw_core::identity::ActorRef {
                id: cyberclaw_core::ids::ActorId::new(),
                actor_type: cyberclaw_core::identity::ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "Test".to_string(),
            },
            workspace: cyberclaw_core::workspace::WorkspaceRef {
                id: cyberclaw_core::ids::WorkspaceId::new(),
                mode: cyberclaw_core::workspace::WorkspaceMode::Isolated,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: workspace.to_str().unwrap().to_string(),
                writable_roots: vec![],
            },
            connector_id: cyberclaw_core::ids::ConnectorId::from_string("local".to_string())
                .unwrap(),
            capability_id: cyberclaw_core::ids::CapabilityId::from_string("fs.read".to_string())
                .unwrap(),
            input: serde_json::json!({
                "path": "../../../etc/passwd"
            }),
        };

        let result = connector.execute(request).await.unwrap();
        assert!(matches!(result.status, ExecutionStatus::Failed));
        assert!(result.error.is_some());
        let error = result.error.unwrap();
        // Accept either ".." component error or "outside workspace" error
        assert!(
            error.contains("..") || error.contains("outside workspace"),
            "Expected path traversal error, got: {}",
            error
        );
    }

    /// Test cmd.exec with timeout
    #[tokio::test]
    async fn test_cmd_exec_with_timeout() {
        let temp_dir = TempDir::new().unwrap();
        let workspace = temp_dir.path().to_path_buf();
        let connector = LocalConnector::new(workspace.clone());

        let request = CapabilityExecutionRequest {
            execution_id: cyberclaw_core::ids::ExecutionId::new(),
            trace_id: uuid::Uuid::new_v4().to_string(),
            actor: cyberclaw_core::identity::ActorRef {
                id: cyberclaw_core::ids::ActorId::new(),
                actor_type: cyberclaw_core::identity::ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "Test".to_string(),
            },
            workspace: cyberclaw_core::workspace::WorkspaceRef {
                id: cyberclaw_core::ids::WorkspaceId::new(),
                mode: cyberclaw_core::workspace::WorkspaceMode::Isolated,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: workspace.to_str().unwrap().to_string(),
                writable_roots: vec![],
            },
            connector_id: cyberclaw_core::ids::ConnectorId::from_string("local".to_string())
                .unwrap(),
            capability_id: cyberclaw_core::ids::CapabilityId::from_string("cmd.exec".to_string())
                .unwrap(),
            input: serde_json::json!({
                "command": "echo Hello",
                "timeout_ms": 5000
            }),
        };

        let result = connector.execute(request).await.unwrap();
        assert!(matches!(result.status, ExecutionStatus::Success));

        let output: crate::types::CmdExecOutput = serde_json::from_value(result.output).unwrap();
        assert_eq!(output.stdout.trim(), "Hello");
        assert!(!output.timed_out);
    }

    /// Test that nonexistent capability returns error
    #[tokio::test]
    async fn test_nonexistent_capability() {
        let temp_dir = TempDir::new().unwrap();
        let workspace = temp_dir.path().to_path_buf();

        let registry = Arc::new(ConnectorRegistry::new());
        let connector: Arc<dyn Connector> = Arc::new(LocalConnector::new(workspace.clone()));
        registry.register(connector).unwrap();

        let dispatcher = CapabilityDispatcher::new(registry);

        let request = CapabilityExecutionRequest {
            execution_id: cyberclaw_core::ids::ExecutionId::new(),
            trace_id: uuid::Uuid::new_v4().to_string(),
            actor: cyberclaw_core::identity::ActorRef {
                id: cyberclaw_core::ids::ActorId::new(),
                actor_type: cyberclaw_core::identity::ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "Test".to_string(),
            },
            workspace: cyberclaw_core::workspace::WorkspaceRef {
                id: cyberclaw_core::ids::WorkspaceId::new(),
                mode: cyberclaw_core::workspace::WorkspaceMode::Isolated,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: workspace.to_str().unwrap().to_string(),
                writable_roots: vec![],
            },
            connector_id: cyberclaw_core::ids::ConnectorId::from_string("local".to_string())
                .unwrap(),
            capability_id: cyberclaw_core::ids::CapabilityId::from_string(
                "nonexistent.capability".to_string(),
            )
            .unwrap(),
            input: serde_json::json!({}),
        };

        let result = dispatcher.dispatch(request).await.unwrap();
        assert!(matches!(result.status, ExecutionStatus::Failed));
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn test_local_connector_web_fetch() {
        let temp_dir = TempDir::new().unwrap();
        let workspace = temp_dir.path().to_path_buf();
        let connector = LocalConnector::new(workspace.clone());
        let (base_url, server_handle) = start_mock_http_server().await;

        let request = CapabilityExecutionRequest {
            execution_id: cyberclaw_core::ids::ExecutionId::new(),
            trace_id: uuid::Uuid::new_v4().to_string(),
            actor: cyberclaw_core::identity::ActorRef {
                id: cyberclaw_core::ids::ActorId::new(),
                actor_type: cyberclaw_core::identity::ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "Test".to_string(),
            },
            workspace: cyberclaw_core::workspace::WorkspaceRef {
                id: cyberclaw_core::ids::WorkspaceId::new(),
                mode: cyberclaw_core::workspace::WorkspaceMode::Isolated,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: workspace.to_str().unwrap().to_string(),
                writable_roots: vec![],
            },
            connector_id: cyberclaw_core::ids::ConnectorId::from_string("local".to_string())
                .unwrap(),
            capability_id: cyberclaw_core::ids::CapabilityId::from_string("web.fetch".to_string())
                .unwrap(),
            input: serde_json::json!({
                "url": format!("{}/fetch", base_url),
                "max_bytes": 8
            }),
        };

        let result = connector.execute(request).await.unwrap();
        assert!(matches!(result.status, ExecutionStatus::Success));

        let output: crate::types::WebFetchOutput = serde_json::from_value(result.output).unwrap();
        assert_eq!(output.status_code, 200);
        assert!(output.truncated);
        assert_eq!(output.body, "mock fet");

        server_handle.abort();
    }

    #[tokio::test]
    async fn test_local_connector_web_search() {
        let temp_dir = TempDir::new().unwrap();
        let workspace = temp_dir.path().to_path_buf();
        let connector = LocalConnector::new(workspace.clone());
        let (base_url, server_handle) = start_mock_http_server().await;

        let request = CapabilityExecutionRequest {
            execution_id: cyberclaw_core::ids::ExecutionId::new(),
            trace_id: uuid::Uuid::new_v4().to_string(),
            actor: cyberclaw_core::identity::ActorRef {
                id: cyberclaw_core::ids::ActorId::new(),
                actor_type: cyberclaw_core::identity::ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "Test".to_string(),
            },
            workspace: cyberclaw_core::workspace::WorkspaceRef {
                id: cyberclaw_core::ids::WorkspaceId::new(),
                mode: cyberclaw_core::workspace::WorkspaceMode::Isolated,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: workspace.to_str().unwrap().to_string(),
                writable_roots: vec![],
            },
            connector_id: cyberclaw_core::ids::ConnectorId::from_string("local".to_string())
                .unwrap(),
            capability_id: cyberclaw_core::ids::CapabilityId::from_string("web.search".to_string())
                .unwrap(),
            input: serde_json::json!({
                "query": "cyberclaw",
                "endpoint": format!("{}/search", base_url),
                "max_results": 2
            }),
        };

        let result = connector.execute(request).await.unwrap();
        assert!(matches!(result.status, ExecutionStatus::Success));

        let output: crate::types::WebSearchOutput = serde_json::from_value(result.output).unwrap();
        assert_eq!(output.total, 2);
        assert_eq!(output.results.len(), 2);
        assert_eq!(output.results[0].title, "CyberClaw Docs");
        assert_eq!(output.results[1].title, "CyberClaw Repo");

        server_handle.abort();
    }
}
