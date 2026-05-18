//! Integration tests for LLM Bridge
//!
//! 端到端测试验证完整的 Tool Call 到 Capability 执行流程。

use cyberclaw_connectors::{
    runtime::{RuntimeMode, RuntimeSelectionStrategy, RuntimeSelectorConfig},
    CapabilityDispatcher, ConnectorRegistry, LocalConnector, LspConnector, LspConnectorConfig,
};
use cyberclaw_core::capability::CapabilityEffect;
use cyberclaw_core::ids::{CapabilityId, ConnectorId};
use cyberclaw_llm::types::{FunctionCall, ToolCall};
use cyberclaw_llm_bridge::{
    register_standard_mappings, ToolCallMapper, ToolExecutionResult, ToolExecutor,
};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;

/// Minimal connector stub — returns a fixed JSON for any execute() call.
/// Used to validate routing paths for connectors not wired in the test harness.
/// `cap_ids` must list every capability this stub will be asked to handle so
/// that `CapabilityDispatcher`'s capability-validation check passes.
#[derive(Debug)]
struct StubConnector {
    id: ConnectorId,
    cap_ids: Vec<String>,
    response: serde_json::Value,
}

impl StubConnector {
    fn new_arc(
        id: &str,
        cap_ids: &[&str],
        response: serde_json::Value,
    ) -> Arc<dyn cyberclaw_connectors::types::Connector> {
        Arc::new(Self {
            id: ConnectorId::from_string(id.to_string()).unwrap(),
            cap_ids: cap_ids.iter().map(|s| s.to_string()).collect(),
            response,
        })
    }
}

#[async_trait::async_trait]
impl cyberclaw_connectors::types::Connector for StubConnector {
    fn id(&self) -> &ConnectorId {
        &self.id
    }
    fn runtime(&self) -> cyberclaw_core::manifests::ConnectorRuntime {
        cyberclaw_core::manifests::ConnectorRuntime::Native
    }
    fn capabilities(&self) -> Vec<cyberclaw_core::manifests::CapabilityContract> {
        self.cap_ids
            .iter()
            .map(|id| cyberclaw_core::manifests::CapabilityContract {
                id: id.clone(),
                title: id.clone(),
                description: None,
                input_schema: String::new(),
                output_schema: String::new(),
                risk: cyberclaw_core::capability::RiskLevel::Low,
                effects: vec![CapabilityEffect::Read],
                placement: None,
                timeouts: cyberclaw_core::manifests::CapabilityTimeouts { request_ms: None },
            })
            .collect()
    }
    async fn execute(
        &self,
        req: cyberclaw_connectors::types::CapabilityExecutionRequest,
    ) -> anyhow::Result<cyberclaw_connectors::types::CapabilityExecutionResult> {
        Ok(cyberclaw_connectors::types::CapabilityExecutionResult {
            execution_id: req.execution_id,
            trace_id: req.trace_id,
            connector_id: req.connector_id,
            capability_id: req.capability_id,
            output: self.response.clone(),
            status: cyberclaw_connectors::types::ExecutionStatus::Success,
            error: None,
            actual_runtime: None,
        })
    }
}

/// 创建测试环境
fn setup_test_env() -> (Arc<ToolExecutor>, TempDir) {
    // 创建临时工作空间
    let temp_dir = TempDir::new().expect("create temp dir");
    let workspace = temp_dir.path().to_path_buf();

    // 创建 registry 和 dispatcher
    let registry = Arc::new(ConnectorRegistry::new());
    let local_connector = LocalConnector::new(workspace.clone());
    let connector: Arc<dyn cyberclaw_connectors::types::Connector> = Arc::new(local_connector);
    registry
        .register(connector)
        .expect("register local connector");

    // Stub connectors for memory / todo / mcp — tests routing path only.
    // cap_ids must list every capability the stub claims to support so the
    // dispatcher's capability-validation check passes.
    registry
        .register(StubConnector::new_arc(
            "memory",
            &["memory_read", "memory_write", "memory_search"],
            serde_json::json!({"value": "stub-memory-value", "key": "test"}),
        ))
        .expect("register memory stub");
    registry
        .register(StubConnector::new_arc(
            "todo",
            &["agent:todo:read", "agent:todo:write"],
            serde_json::json!({"items": []}),
        ))
        .expect("register todo stub");
    registry
        .register(StubConnector::new_arc(
            "mcp-default",
            &["mcp.list_resources", "mcp.read_resource", "mcp.call_tool"],
            serde_json::json!({"result": "mcp-stub-ok"}),
        ))
        .expect("register mcp stub");

    // LSP connector — non-existent binary; tests verify graceful failure.
    let lsp = LspConnector::new(
        "lsp",
        LspConnectorConfig::generic("cyberclaw-no-such-lsp-xyz", vec![]),
    )
    .expect("construct lsp connector");
    registry
        .register(Arc::new(lsp) as Arc<dyn cyberclaw_connectors::types::Connector>)
        .expect("register lsp connector");

    // Override all Medium/High capabilities to Native runtime (Process runtime
    // is not yet implemented).
    let mut capability_overrides = HashMap::new();
    capability_overrides.insert("fs.edit".to_string(), RuntimeMode::Native);
    capability_overrides.insert("fs.write".to_string(), RuntimeMode::Native);
    capability_overrides.insert("fs.multiedit".to_string(), RuntimeMode::Native);
    capability_overrides.insert("cmd.exec".to_string(), RuntimeMode::Native);

    let runtime_config = RuntimeSelectorConfig {
        default_strategy: RuntimeSelectionStrategy::RiskBased,
        capability_overrides,
        strict_mode: false,
    };

    let dispatcher = Arc::new(CapabilityDispatcher::with_runtime_config(
        registry,
        runtime_config,
    ));

    // 创建 mapper 并注册标准映射
    let mapper = Arc::new(ToolCallMapper::new());
    register_standard_mappings(&mapper).expect("register mappings");

    // 创建 executor
    let executor = Arc::new(ToolExecutor::new(dispatcher, mapper));

    (executor, temp_dir)
}

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
                        "bridge fetch body".to_string(),
                    )
                } else if first_line.contains("GET /search?") || first_line.contains("GET /search ")
                {
                    (
                        "200 OK",
                        "application/json",
                        r#"{
                            "results": [
                                {"title":"Result A","url":"https://example.com/a","snippet":"A"},
                                {"title":"Result B","url":"https://example.com/b","snippet":"B"}
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

#[tokio::test]
async fn test_end_to_end_file_read() {
    // Setup
    let (executor, temp_dir) = setup_test_env();
    let test_file = temp_dir.path().join("test.txt");
    let test_content = "Hello, CyberClaw!";

    // 先创建文件 (使用标准库,因为 fs.write 需要 Process runtime)
    std::fs::write(&test_file, test_content).expect("create test file");

    // 读取文件 (fs.read 是 Low 风险,可以在 Native runtime 运行)
    let read_tool_call = ToolCall {
        id: "call-read".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "read_file".to_string(),
            arguments: serde_json::json!({
                "file_path": test_file.to_str().unwrap()
            })
            .to_string(),
        },
    };

    let read_result = executor
        .execute_tool(&read_tool_call, "trace-1".to_string())
        .await
        .expect("execute read_file");

    // 验证读取成功
    assert!(read_result.is_success(), "Read should succeed");

    // 验证读取内容
    match read_result {
        ToolExecutionResult::Success { result, .. } => {
            let content = result["content"]
                .as_str()
                .expect("content should be string");
            assert_eq!(content, test_content, "Content should match");
        }
        _ => panic!("Expected success result"),
    }
}

#[tokio::test]
async fn test_end_to_end_file_edit() {
    // Setup
    let (executor, temp_dir) = setup_test_env();
    let test_file = temp_dir.path().join("edit_test.txt");
    let original_content = "Hello, World!";
    let new_content = "Hello, CyberClaw!";

    // 先写入原始内容
    std::fs::write(&test_file, original_content).expect("write initial content");

    // 执行编辑 (fs.edit 是 Medium 风险,需要 Process runtime)
    let edit_tool_call = ToolCall {
        id: "call-edit".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "edit_file".to_string(),
            arguments: serde_json::json!({
                "file_path": test_file.to_str().unwrap(),
                "old_text": "World",
                "new_text": "CyberClaw"
            })
            .to_string(),
        },
    };

    let edit_result = executor
        .execute_tool(&edit_tool_call, "trace-3".to_string())
        .await
        .expect("execute edit_file");

    // 打印详细结果用于调试
    eprintln!("Edit result: {:#?}", edit_result);

    // 验证编辑成功
    if !edit_result.is_success() {
        panic!("Edit failed: {:#?}", edit_result);
    }

    // 验证文件内容已更新
    let updated_content = std::fs::read_to_string(&test_file).expect("read updated file");
    assert_eq!(updated_content, new_content, "Content should be updated");
}

#[tokio::test]
async fn test_end_to_end_search_grep() {
    // Setup
    let (executor, temp_dir) = setup_test_env();

    // 创建测试文件
    let file1 = temp_dir.path().join("file1.txt");
    let file2 = temp_dir.path().join("file2.txt");
    std::fs::write(&file1, "This contains the pattern").expect("write file1");
    std::fs::write(&file2, "This does not contain it").expect("write file2");

    // 执行搜索
    let search_tool_call = ToolCall {
        id: "call-search".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "search_code".to_string(),
            arguments: serde_json::json!({
                "query": "pattern",
                "directory": temp_dir.path().to_str().unwrap(),
                // search.grep default mode is `files_with_matches` (returns
                // paths only). To assert the matched line content shows up
                // in the JSON result, request `content` mode explicitly.
                "output_mode": "content"
            })
            .to_string(),
        },
    };

    let search_result = executor
        .execute_tool(&search_tool_call, "trace-4".to_string())
        .await
        .expect("execute search_code");

    // 验证搜索成功
    assert!(search_result.is_success(), "Search should succeed");

    // 验证搜索结果
    match search_result {
        ToolExecutionResult::Success { result, .. } => {
            // 结果应该包含 file1 但不包含 file2
            let result_str = serde_json::to_string(&result).expect("serialize result");
            assert!(result_str.contains("file1"), "Result should contain file1");
            assert!(
                result_str.contains("pattern"),
                "Result should contain the pattern"
            );
        }
        _ => panic!("Expected success result"),
    }
}

#[tokio::test]
async fn test_end_to_end_find_files() {
    // Setup
    let (executor, temp_dir) = setup_test_env();

    // 创建测试文件
    let rs_file = temp_dir.path().join("test.rs");
    let txt_file = temp_dir.path().join("test.txt");
    std::fs::write(&rs_file, "fn main() {}").expect("write rs file");
    std::fs::write(&txt_file, "text content").expect("write txt file");

    // 搜索 .rs 文件
    let find_tool_call = ToolCall {
        id: "call-find".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "find_files".to_string(),
            arguments: serde_json::json!({
                "pattern": "*.rs",
                "directory": temp_dir.path().to_str().unwrap()
            })
            .to_string(),
        },
    };

    let find_result = executor
        .execute_tool(&find_tool_call, "trace-5".to_string())
        .await
        .expect("execute find_files");

    // 验证查找成功
    assert!(find_result.is_success(), "Find should succeed");

    // 验证查找结果
    match find_result {
        ToolExecutionResult::Success { result, .. } => {
            let result_str = serde_json::to_string(&result).expect("serialize result");
            assert!(result_str.contains("test.rs"), "Should find .rs file");
            assert!(
                !result_str.contains("test.txt"),
                "Should not find .txt file"
            );
        }
        _ => panic!("Expected success result"),
    }
}

#[tokio::test]
async fn test_end_to_end_web_fetch() {
    let (executor, _temp_dir) = setup_test_env();
    let (base_url, server_handle) = start_mock_http_server().await;

    let tool_call = ToolCall {
        id: "call-web-fetch".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "WebFetch".to_string(),
            arguments: serde_json::json!({
                "url": format!("{}/fetch", base_url)
            })
            .to_string(),
        },
    };

    let result = executor
        .execute_tool(&tool_call, "trace-web-fetch".to_string())
        .await
        .expect("execute web_fetch");

    match result {
        ToolExecutionResult::Success { result, .. } => {
            assert_eq!(result["status_code"], 200);
            assert_eq!(result["body"], "bridge fetch body");
        }
        _ => panic!("Expected success result"),
    }

    server_handle.abort();
}

#[tokio::test]
async fn test_end_to_end_web_search() {
    let (executor, _temp_dir) = setup_test_env();
    let (base_url, server_handle) = start_mock_http_server().await;

    let tool_call = ToolCall {
        id: "call-web-search".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "WebSearch".to_string(),
            arguments: serde_json::json!({
                "query": "cyberclaw",
                "endpoint": format!("{}/search", base_url),
                "max_results": 2
            })
            .to_string(),
        },
    };

    let result = executor
        .execute_tool(&tool_call, "trace-web-search".to_string())
        .await
        .expect("execute web_search");

    match result {
        ToolExecutionResult::Success { result, .. } => {
            assert_eq!(result["total"], 2);
            assert_eq!(result["results"][0]["title"], "Result A");
            assert_eq!(result["results"][1]["title"], "Result B");
        }
        _ => panic!("Expected success result"),
    }

    server_handle.abort();
}

#[tokio::test]
async fn test_end_to_end_error_handling() {
    // Setup
    let (executor, temp_dir) = setup_test_env();

    // 尝试读取不存在的文件
    let nonexistent_file = temp_dir.path().join("does_not_exist.txt");
    let read_tool_call = ToolCall {
        id: "call-read-error".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "read_file".to_string(),
            arguments: serde_json::json!({
                "file_path": nonexistent_file.to_str().unwrap()
            })
            .to_string(),
        },
    };

    let read_result = executor
        .execute_tool(&read_tool_call, "trace-6".to_string())
        .await
        .expect("execute should return result");

    // 验证返回错误
    assert!(!read_result.is_success(), "Should return error");

    // 验证错误是可恢复的(文件不存在通常是可恢复的)
    match read_result {
        ToolExecutionResult::Error {
            error, recoverable, ..
        } => {
            assert!(error.contains("No such file"), "Error should mention file");
            assert!(recoverable, "File not found should be recoverable");
        }
        _ => panic!("Expected error result"),
    }
}

#[tokio::test]
async fn test_unknown_tool_error() {
    // Setup
    let (executor, _temp_dir) = setup_test_env();

    // 调用未注册的工具
    let unknown_tool_call = ToolCall {
        id: "call-unknown".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "unknown_tool".to_string(),
            arguments: serde_json::json!({}).to_string(),
        },
    };

    let result = executor
        .execute_tool(&unknown_tool_call, "trace-7".to_string())
        .await;

    // 验证返回错误 (ToolExecutor 将映射错误转换为 ToolExecutionResult::Error)
    match result {
        Err(e) => {
            assert!(
                e.to_string().contains("Unknown tool"),
                "Error should mention unknown tool"
            );
        }
        Ok(ToolExecutionResult::Error { error, .. }) => {
            assert!(
                error.contains("Unknown tool"),
                "Error should mention unknown tool"
            );
        }
        Ok(ToolExecutionResult::Success { .. }) => {
            panic!("Should not succeed for unknown tool");
        }
    }
}

#[tokio::test]
async fn test_invalid_arguments_error() {
    // Setup
    let (executor, _temp_dir) = setup_test_env();

    // 使用无效的 JSON 参数
    let invalid_tool_call = ToolCall {
        id: "call-invalid".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "read_file".to_string(),
            arguments: "invalid json".to_string(),
        },
    };

    let result = executor
        .execute_tool(&invalid_tool_call, "trace-8".to_string())
        .await;

    // 验证返回错误 (ToolExecutor 将参数解析错误转换为 ToolExecutionResult::Error)
    match result {
        Err(e) => {
            let error_msg = e.to_string();
            assert!(
                error_msg.contains("parse")
                    || error_msg.contains("JSON")
                    || error_msg.contains("Invalid"),
                "Error should mention parsing or JSON issue, got: {}",
                error_msg
            );
        }
        Ok(ToolExecutionResult::Error { error, .. }) => {
            assert!(
                error.contains("parse") || error.contains("JSON") || error.contains("Invalid"),
                "Error should mention parsing or JSON issue, got: {}",
                error
            );
        }
        Ok(ToolExecutionResult::Success { .. }) => {
            panic!("Should not succeed for invalid JSON");
        }
    }
}

#[tokio::test]
async fn test_mapper_capability_mapping() {
    // Setup
    let (_executor, _temp_dir) = setup_test_env();

    // 测试 mapper 是否正确注册了所有工具
    let mapper = ToolCallMapper::new();
    register_standard_mappings(&mapper).expect("register mappings");

    // 验证所有标准工具已注册
    assert!(mapper.has_tool("read_file"));
    assert!(mapper.has_tool("write_file"));
    assert!(mapper.has_tool("edit_file"));
    assert!(mapper.has_tool("search_code"));
    assert!(mapper.has_tool("find_files"));
    assert!(mapper.has_tool("execute_command"));
    assert!(mapper.has_tool("WebFetch"));
    assert!(mapper.has_tool("WebSearch"));
    assert!(mapper.has_tool("WebFetchTool"));
    assert!(mapper.has_tool("WebSearchTool"));
    assert!(mapper.has_tool("web_fetch"));
    assert!(mapper.has_tool("web_search"));
    assert!(mapper.has_tool("Read"));
    assert!(mapper.has_tool("Write"));
    assert!(mapper.has_tool("Edit"));
    assert!(mapper.has_tool("Grep"));
    assert!(mapper.has_tool("Glob"));
    assert!(mapper.has_tool("Bash"));
    assert!(mapper.has_tool("FileReadTool"));
    assert!(mapper.has_tool("FileWriteTool"));
    assert!(mapper.has_tool("FileEditTool"));
    assert!(mapper.has_tool("GrepTool"));
    assert!(mapper.has_tool("GlobTool"));
    assert!(mapper.has_tool("BashTool"));
    assert!(mapper.has_tool("PowerShellTool"));
    assert!(mapper.has_tool("SendMessageTool"));
    assert!(mapper.has_tool("ListMcpResourcesTool"));
    assert!(mapper.has_tool("ReadMcpResourceTool"));
    assert!(mapper.has_tool("MCPTool"));

    // 验证工具到 capability 的映射
    let read_call = ToolCall {
        id: "test".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "read_file".to_string(),
            arguments: serde_json::json!({"file_path": "/tmp/test.txt"}).to_string(),
        },
    };

    let request = mapper
        .map_tool_call(&read_call, "trace".to_string())
        .expect("map tool call");

    assert_eq!(
        request.capability_id,
        CapabilityId::from_string("fs.read".to_string()).unwrap()
    );
    assert_eq!(
        request.connector_id,
        ConnectorId::from_string("local".to_string()).unwrap()
    );
}

// ── Sprint 18 W3: 12 missing tool E2E tests ──────────────────────────────────

#[tokio::test]
async fn test_end_to_end_file_write() {
    let (executor, temp_dir) = setup_test_env();
    let test_file = temp_dir.path().join("write_test.txt");

    let tool_call = ToolCall {
        id: "call-file-write".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "file_write".to_string(),
            arguments: serde_json::json!({
                "file_path": test_file.to_str().unwrap(),
                "content": "written by bridge"
            })
            .to_string(),
        },
    };

    let result = executor
        .execute_tool(&tool_call, "trace-fw".to_string())
        .await
        .expect("execute file_write");
    assert!(
        result.is_success(),
        "file_write should succeed: {:#?}",
        result
    );

    let content = std::fs::read_to_string(&test_file).expect("read written file");
    assert_eq!(content, "written by bridge");
}

#[tokio::test]
async fn test_end_to_end_file_multiedit() {
    let (executor, temp_dir) = setup_test_env();
    let test_file = temp_dir.path().join("multiedit_test.txt");
    std::fs::write(&test_file, "foo bar baz").expect("write initial");

    let tool_call = ToolCall {
        id: "call-multiedit".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "file_multiedit".to_string(),
            arguments: serde_json::json!({
                "file_path": test_file.to_str().unwrap(),
                "edits": [
                    {"old_string": "foo", "new_string": "qux"},
                    {"old_string": "baz", "new_string": "quux"}
                ]
            })
            .to_string(),
        },
    };

    let result = executor
        .execute_tool(&tool_call, "trace-fme".to_string())
        .await
        .expect("execute file_multiedit");
    assert!(
        result.is_success(),
        "file_multiedit should succeed: {:#?}",
        result
    );

    let content = std::fs::read_to_string(&test_file).expect("read after multiedit");
    assert_eq!(content, "qux bar quux");

    match result {
        ToolExecutionResult::Success { result, .. } => {
            assert_eq!(result["total_replacements"], 2);
        }
        _ => panic!("expected success"),
    }
}

#[tokio::test]
async fn test_end_to_end_bash() {
    let (executor, _temp_dir) = setup_test_env();

    let tool_call = ToolCall {
        id: "call-bash".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "bash".to_string(),
            arguments: serde_json::json!({"command": "echo hello-bridge"}).to_string(),
        },
    };

    let result = executor
        .execute_tool(&tool_call, "trace-bash".to_string())
        .await
        .expect("execute bash");

    // R-1 (2026-05-05) — `bash` now routes to `cmd.run` (was `cmd.exec`),
    // which is also High risk. RuntimeSelector still enforces isolation, so
    // either we get a successful container-isolated dispatch (when a
    // ContainerRuntime is wired) or a structured rejection. This test
    // verifies that the routing path is intact (tool name → capability →
    // connector) and that any rejection is from the runtime gate, NOT from
    // "unknown tool" or "connector not found".
    match result {
        ToolExecutionResult::Success { result, .. } => {
            // Accepted if a runtime relaxation is in effect.
            let stdout = result["stdout"].as_str().unwrap_or_default();
            assert!(stdout.contains("hello-bridge"));
        }
        ToolExecutionResult::Error { error, .. } => {
            assert!(
                error.contains("Native runtime not allowed")
                    || error.contains("isolation")
                    || error.contains("Process")
                    || error.contains("Container runtime not configured")
                    || error.contains("RB-09"),
                "expected runtime-isolation/container-config error, got: {}",
                error
            );
        }
    }
}

#[tokio::test]
async fn test_end_to_end_memory_read() {
    let (executor, _temp_dir) = setup_test_env();

    let tool_call = ToolCall {
        id: "call-mem-r".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "memory_read".to_string(),
            arguments: serde_json::json!({"scope": "agent", "key": "test"}).to_string(),
        },
    };

    let result = executor
        .execute_tool(&tool_call, "trace-mem-r".to_string())
        .await
        .expect("execute memory_read");
    assert!(
        result.is_success(),
        "memory_read stub should succeed: {:#?}",
        result
    );
}

#[tokio::test]
async fn test_end_to_end_memory_write() {
    let (executor, _temp_dir) = setup_test_env();

    let tool_call = ToolCall {
        id: "call-mem-w".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "memory_write".to_string(),
            arguments: serde_json::json!({"scope": "agent", "key": "test", "value": "hello"})
                .to_string(),
        },
    };

    let result = executor
        .execute_tool(&tool_call, "trace-mem-w".to_string())
        .await
        .expect("execute memory_write");
    assert!(
        result.is_success(),
        "memory_write stub should succeed: {:#?}",
        result
    );
}

#[tokio::test]
async fn test_end_to_end_todo_read() {
    let (executor, _temp_dir) = setup_test_env();

    let tool_call = ToolCall {
        id: "call-todo-r".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "todo_read".to_string(),
            arguments: serde_json::json!({"agent_id": "test-agent"}).to_string(),
        },
    };

    let result = executor
        .execute_tool(&tool_call, "trace-todo-r".to_string())
        .await
        .expect("execute todo_read");
    assert!(
        result.is_success(),
        "todo_read stub should succeed: {:#?}",
        result
    );
}

#[tokio::test]
async fn test_end_to_end_todo_write() {
    let (executor, _temp_dir) = setup_test_env();

    let tool_call = ToolCall {
        id: "call-todo-w".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "todo_write".to_string(),
            arguments: serde_json::json!({
                "agent_id": "test-agent",
                "action": "add",
                "item": {"id": "1", "content": "test task", "status": "pending"}
            })
            .to_string(),
        },
    };

    let result = executor
        .execute_tool(&tool_call, "trace-todo-w".to_string())
        .await
        .expect("execute todo_write");
    assert!(
        result.is_success(),
        "todo_write stub should succeed: {:#?}",
        result
    );
}

#[tokio::test]
async fn test_end_to_end_lsp_hover() {
    let (executor, _temp_dir) = setup_test_env();

    let tool_call = ToolCall {
        id: "call-lsp-hover".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "lsp.hover".to_string(),
            arguments: serde_json::json!({"file": "/tmp/test.rs", "line": 1, "col": 1}).to_string(),
        },
    };

    // Backend is a non-existent binary — routing is exercised, graceful failure expected.
    match executor
        .execute_tool(&tool_call, "trace-lsp-hover".to_string())
        .await
    {
        Ok(ToolExecutionResult::Success { .. }) => {
            panic!("lsp.hover with missing backend should not succeed")
        }
        Ok(ToolExecutionResult::Error { .. }) | Err(_) => {}
    }
}

#[tokio::test]
async fn test_end_to_end_lsp_diagnostics() {
    let (executor, _temp_dir) = setup_test_env();

    let tool_call = ToolCall {
        id: "call-lsp-diag".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "lsp.diagnostics".to_string(),
            arguments: serde_json::json!({"file": "/tmp/test.rs"}).to_string(),
        },
    };

    match executor
        .execute_tool(&tool_call, "trace-lsp-diag".to_string())
        .await
    {
        Ok(ToolExecutionResult::Success { .. }) => {
            panic!("lsp.diagnostics with missing backend should not succeed")
        }
        Ok(ToolExecutionResult::Error { .. }) | Err(_) => {}
    }
}

#[tokio::test]
async fn test_end_to_end_lsp_goto_definition() {
    let (executor, _temp_dir) = setup_test_env();

    let tool_call = ToolCall {
        id: "call-lsp-def".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "lsp.goto_definition".to_string(),
            arguments: serde_json::json!({"file": "/tmp/test.rs", "line": 1, "col": 1}).to_string(),
        },
    };

    match executor
        .execute_tool(&tool_call, "trace-lsp-def".to_string())
        .await
    {
        Ok(ToolExecutionResult::Success { .. }) => {
            panic!("lsp.goto_definition with missing backend should not succeed")
        }
        Ok(ToolExecutionResult::Error { .. }) | Err(_) => {}
    }
}

#[tokio::test]
async fn test_end_to_end_lsp_find_references() {
    let (executor, _temp_dir) = setup_test_env();

    let tool_call = ToolCall {
        id: "call-lsp-refs".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "lsp.find_references".to_string(),
            arguments: serde_json::json!({"file": "/tmp/test.rs", "line": 1, "col": 1}).to_string(),
        },
    };

    match executor
        .execute_tool(&tool_call, "trace-lsp-refs".to_string())
        .await
    {
        Ok(ToolExecutionResult::Success { .. }) => {
            panic!("lsp.find_references with missing backend should not succeed")
        }
        Ok(ToolExecutionResult::Error { .. }) | Err(_) => {}
    }
}

#[tokio::test]
async fn test_end_to_end_mcp_call() {
    let (executor, _temp_dir) = setup_test_env();

    let tool_call = ToolCall {
        id: "call-mcp".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "MCPTool".to_string(),
            arguments: serde_json::json!({
                "tool_name": "test_tool",
                "arguments": {"key": "value"}
            })
            .to_string(),
        },
    };

    let result = executor
        .execute_tool(&tool_call, "trace-mcp".to_string())
        .await
        .expect("execute MCPTool");
    assert!(
        result.is_success(),
        "mcp_call stub should succeed: {:#?}",
        result
    );

    match result {
        ToolExecutionResult::Success { result, .. } => {
            assert_eq!(result["result"], "mcp-stub-ok");
        }
        _ => panic!("expected success"),
    }
}
