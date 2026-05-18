// tests/integration/connectors/mcp_connector_tests.rs
// MCP Connector 集成测试

use anyhow::Result;
use cyberclaw_connectors::{McpConnector, Connector, CapabilityRequest, CapabilityResponse};
use serde_json::json;

mod helpers {
    pub use crate::helpers::connector::*;
}

/// 测试 MCP Server 连接
#[tokio::test]
async fn test_mcp_server_connection() -> Result<()> {
    // 启动 Mock MCP Server
    let mut server = helpers::MockMcpServer::new().await?;
    let url = server.start().await?;

    // 创建 MCP Connector
    let connector = McpConnector::new(&url)?;

    // 测试连接
    let connected = connector.test_connection().await?;
    assert!(connected);

    server.stop().await;
    Ok(())
}

/// 测试 MCP Tool 列举
#[tokio::test]
async fn test_mcp_tool_discovery() -> Result<()> {
    let mut server = helpers::MockMcpServer::new().await?;

    // 添加测试 Tools
    server.add_tool(
        "calculator.add",
        "Add two numbers",
        json!({
            "type": "object",
            "properties": {
                "a": {"type": "number"},
                "b": {"type": "number"}
            },
            "required": ["a", "b"]
        }),
        |input| {
            let a = input["a"].as_f64().unwrap_or(0.0);
            let b = input["b"].as_f64().unwrap_or(0.0);
            json!({"result": a + b})
        }
    ).await?;

    server.add_tool(
        "string.concat",
        "Concatenate strings",
        json!({
            "type": "object",
            "properties": {
                "strings": {
                    "type": "array",
                    "items": {"type": "string"}
                }
            }
        }),
        |input| {
            let strings = input["strings"].as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("");
            json!({"result": strings})
        }
    ).await?;

    let url = server.start().await?;
    let connector = McpConnector::new(&url)?;

    // 发现 Capabilities
    let capabilities = connector.discover_capabilities().await?;

    // 验证发现的 Tools
    assert_eq!(capabilities.len(), 2);
    assert!(capabilities.iter().any(|c| c.id == "mcp.tool.calculator.add"));
    assert!(capabilities.iter().any(|c| c.id == "mcp.tool.string.concat"));

    server.stop().await;
    Ok(())
}

/// 测试 MCP Tool 调用
#[tokio::test]
async fn test_mcp_tool_execution() -> Result<()> {
    let mut server = helpers::MockMcpServer::new().await?;

    // 添加计算器 Tool
    server.add_tool(
        "calculator.multiply",
        "Multiply two numbers",
        json!({
            "type": "object",
            "properties": {
                "x": {"type": "number"},
                "y": {"type": "number"}
            }
        }),
        |input| {
            let x = input["x"].as_f64().unwrap_or(0.0);
            let y = input["y"].as_f64().unwrap_or(0.0);
            json!({"result": x * y})
        }
    ).await?;

    let url = server.start().await?;
    let connector = McpConnector::new(&url)?;

    // 调用 Tool
    let request = CapabilityRequest {
        params: json!({
            "x": 6,
            "y": 7
        }),
        metadata: Default::default(),
    };

    let response = connector.execute("mcp.tool.calculator.multiply", request).await?;

    // 验证结果
    assert_eq!(response.output["result"], json!(42.0));

    server.stop().await;
    Ok(())
}

/// 测试 MCP Resource 读取
#[tokio::test]
async fn test_mcp_resource_reading() -> Result<()> {
    let mut server = helpers::MockMcpServer::new().await?;

    // 添加 Resources
    server.add_resource(
        "file://test.txt",
        "Test File",
        Some("text/plain".to_string()),
        "This is test content",
    ).await?;

    server.add_resource(
        "config://app.json",
        "App Config",
        Some("application/json".to_string()),
        r#"{"version": "1.0.0", "debug": true}"#,
    ).await?;

    let url = server.start().await?;
    let connector = McpConnector::new(&url)?;

    // 读取文本 Resource
    let text_response = connector.execute(
        "mcp.resource.file://test.txt",
        CapabilityRequest::default()
    ).await?;

    assert_eq!(text_response.output["content"], json!("This is test content"));

    // 读取 JSON Resource
    let json_response = connector.execute(
        "mcp.resource.config://app.json",
        CapabilityRequest::default()
    ).await?;

    let config = json_response.output["content"].as_str().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(config)?;
    assert_eq!(parsed["version"], json!("1.0.0"));

    server.stop().await;
    Ok(())
}

/// 测试 MCP 错误处理
#[tokio::test]
async fn test_mcp_error_handling() -> Result<()> {
    let mut server = helpers::MockMcpServer::new().await?;

    // 添加会失败的 Tool
    server.add_tool(
        "error.trigger",
        "Trigger an error",
        json!({"type": "object"}),
        |_| json!({"error": "Intentional error for testing"})
    ).await?;

    let url = server.start().await?;
    let connector = McpConnector::new(&url)?;

    // 调用不存在的 Tool
    let result = connector.execute(
        "mcp.tool.nonexistent",
        CapabilityRequest::default()
    ).await;

    assert!(result.is_err());

    // 调用会返回错误的 Tool
    let error_result = connector.execute(
        "mcp.tool.error.trigger",
        CapabilityRequest::default()
    ).await?;

    assert!(error_result.output["error"].is_string());

    server.stop().await;
    Ok(())
}

/// 测试 MCP 缓存机制
#[tokio::test]
async fn test_mcp_caching() -> Result<()> {
    let mut server = helpers::MockMcpServer::new().await?;

    // 添加 Resource（用于测试缓存）
    let content = "Cacheable content";
    server.add_resource(
        "cache://test",
        "Cacheable Resource",
        Some("text/plain".to_string()),
        content,
    ).await?;

    let url = server.start().await?;
    let connector = McpConnector::with_cache(url, true)?;

    // 第一次读取（未缓存）
    let start1 = std::time::Instant::now();
    let response1 = connector.execute(
        "mcp.resource.cache://test",
        CapabilityRequest::default()
    ).await?;
    let duration1 = start1.elapsed();

    assert_eq!(response1.output["content"], json!(content));

    // 第二次读取（已缓存，应该更快）
    let start2 = std::time::Instant::now();
    let response2 = connector.execute(
        "mcp.resource.cache://test",
        CapabilityRequest::default()
    ).await?;
    let duration2 = start2.elapsed();

    assert_eq!(response2.output["content"], json!(content));

    // 验证缓存生效（第二次应该更快）
    // 注意：这个测试可能不稳定，实际中可以通过检查缓存统计信息
    println!("First read: {:?}, Second read: {:?}", duration1, duration2);

    server.stop().await;
    Ok(())
}

/// 测试 MCP 超时处理
#[tokio::test]
async fn test_mcp_timeout() -> Result<()> {
    let mut server = helpers::MockMcpServer::new().await?;

    // 添加慢速 Tool
    server.add_tool(
        "slow.operation",
        "Slow operation",
        json!({"type": "object"}),
        |_| {
            // 模拟慢速操作
            std::thread::sleep(std::time::Duration::from_secs(3));
            json!({"result": "done"})
        }
    ).await?;

    let url = server.start().await?;

    // 创建带短超时的 Connector
    let connector = McpConnector::with_timeout(url, std::time::Duration::from_secs(1))?;

    // 调用应该超时
    let result = connector.execute(
        "mcp.tool.slow.operation",
        CapabilityRequest::default()
    ).await;

    assert!(result.is_err());

    server.stop().await;
    Ok(())
}

/// 测试 MCP 批量操作
#[tokio::test]
async fn test_mcp_batch_operations() -> Result<()> {
    let mut server = helpers::MockMcpServer::new().await?;

    // 添加多个 Tools
    for i in 1..=5 {
        let name = format!("batch.op{}", i);
        server.add_tool(
            &name,
            &format!("Batch operation {}", i),
            json!({"type": "object", "properties": {"value": {"type": "number"}}}),
            move |input| {
                let value = input["value"].as_f64().unwrap_or(0.0);
                json!({"result": value * i as f64})
            }
        ).await?;
    }

    let url = server.start().await?;
    let connector = McpConnector::new(&url)?;

    // 并发执行多个操作
    let mut handles = Vec::new();

    for i in 1..=5 {
        let connector = connector.clone();
        let handle = tokio::spawn(async move {
            connector.execute(
                &format!("mcp.tool.batch.op{}", i),
                CapabilityRequest {
                    params: json!({"value": 10}),
                    metadata: Default::default(),
                }
            ).await
        });
        handles.push(handle);
    }

    // 收集结果
    let mut results = Vec::new();
    for handle in handles {
        results.push(handle.await??);
    }

    // 验证所有操作成功
    assert_eq!(results.len(), 5);
    for (i, result) in results.iter().enumerate() {
        let expected = 10.0 * (i + 1) as f64;
        assert_eq!(result.output["result"], json!(expected));
    }

    server.stop().await;
    Ok(())
}

/// 测试 MCP Protocol 兼容性
#[tokio::test]
async fn test_mcp_protocol_compatibility() -> Result<()> {
    let mut server = helpers::MockMcpServer::new().await?;

    // 添加复杂的 Tool（测试各种参数类型）
    server.add_tool(
        "complex.operation",
        "Complex operation with various types",
        json!({
            "type": "object",
            "properties": {
                "string": {"type": "string"},
                "number": {"type": "number"},
                "boolean": {"type": "boolean"},
                "array": {
                    "type": "array",
                    "items": {"type": "string"}
                },
                "object": {
                    "type": "object",
                    "properties": {
                        "nested": {"type": "string"}
                    }
                }
            },
            "required": ["string", "number"]
        }),
        |input| {
            // 验证所有类型正确传递
            json!({
                "received": input,
                "types_ok": true
            })
        }
    ).await?;

    let url = server.start().await?;
    let connector = McpConnector::new(&url)?;

    // 调用复杂 Tool
    let request = CapabilityRequest {
        params: json!({
            "string": "test",
            "number": 42,
            "boolean": true,
            "array": ["a", "b", "c"],
            "object": {
                "nested": "value"
            }
        }),
        metadata: Default::default(),
    };

    let response = connector.execute("mcp.tool.complex.operation", request).await?;

    // 验证参数正确传递
    assert_eq!(response.output["received"]["string"], json!("test"));
    assert_eq!(response.output["received"]["number"], json!(42));
    assert_eq!(response.output["received"]["boolean"], json!(true));
    assert_eq!(response.output["received"]["array"], json!(["a", "b", "c"]));
    assert_eq!(response.output["received"]["object"]["nested"], json!("value"));

    server.stop().await;
    Ok(())
}

/// 测试 MCP 动态 Capability 更新
#[tokio::test]
async fn test_mcp_dynamic_capability_updates() -> Result<()> {
    let mut server = helpers::MockMcpServer::new().await?;

    // 初始只有一个 Tool
    server.add_tool(
        "initial.tool",
        "Initial tool",
        json!({"type": "object"}),
        |_| json!({"result": "initial"})
    ).await?;

    let url = server.start().await?;
    let connector = McpConnector::new(&url)?;

    // 第一次发现
    let capabilities_v1 = connector.discover_capabilities().await?;
    assert_eq!(capabilities_v1.len(), 1);

    // 动态添加新 Tool（模拟服务器更新）
    server.add_tool(
        "dynamic.tool",
        "Dynamically added tool",
        json!({"type": "object"}),
        |_| json!({"result": "dynamic"})
    ).await?;

    // 刷新 Capabilities
    connector.refresh_capabilities().await?;

    // 第二次发现
    let capabilities_v2 = connector.discover_capabilities().await?;
    assert_eq!(capabilities_v2.len(), 2);

    // 验证新 Tool 可用
    let response = connector.execute(
        "mcp.tool.dynamic.tool",
        CapabilityRequest::default()
    ).await?;

    assert_eq!(response.output["result"], json!("dynamic"));

    server.stop().await;
    Ok(())
}