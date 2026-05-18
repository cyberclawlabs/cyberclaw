# MCP Connector 集成指南

本文档说明如何将 MCP Connector 集成到 CyberClaw 系统中。

## 架构集成

MCP Connector 通过 CyberClaw 的标准 Connector 架构自动集成：

```text
┌─────────────────┐
│  LLM Bridge     │  (Tool Calling)
│  ToolExecutor   │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Capability      │
│ Dispatcher      │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│  Connector      │
│  Registry       │
└────────┬────────┘
         │
    ┌────┴────┬──────────┬─────────┐
    ▼         ▼          ▼         ▼
┌────────┐ ┌─────┐  ┌──────┐  ┌─────────┐
│ Local  │ │ MCP │  │GitHub│  │ Other   │
└────────┘ └─────┘  └──────┘  └─────────┘
```

## 集成步骤

### 1. 创建 MCP Server 配置

```rust
use cyberclaw_connectors::mcp::{McpServerConfig, TransportConfig};
use std::time::Duration;

let mcp_config = McpServerConfig {
    name: "filesystem".to_string(),
    transport: TransportConfig::Stdio {
        command: "npx".to_string(),
        args: vec![
            "-y".to_string(),
            "@modelcontextprotocol/server-filesystem".to_string(),
            "/tmp".to_string()
        ],
        workdir: None,
    },
    timeout: Duration::from_secs(30),
    enable_cache: true,
};
```

### 2. 创建并注册 MCP Connector

```rust
use cyberclaw_connectors::mcp::McpConnector;
use cyberclaw_connectors::ConnectorRegistry;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 创建 MCP Connector
    let mcp_connector = McpConnector::new(mcp_config).await?;

    // 注册到全局 registry
    let registry = ConnectorRegistry::global();
    registry.register(Arc::new(mcp_connector))?;

    Ok(())
}
```

### 3. 通过 LLM Bridge 使用

MCP 能力自动注册后，可以通过 LLM Tool Calling 调用：

```rust
use cyberclaw_llm_bridge::{ToolExecutor, ToolCallMapper};
use cyberclaw_connectors::CapabilityDispatcher;
use serde_json::json;

async fn use_mcp_via_llm(
    executor: &ToolExecutor,
    trace_id: String,
) -> anyhow::Result<()> {
    // LLM 生成的 tool call
    let tool_call = json!({
        "id": "call-123",
        "type": "function",
        "function": {
            "name": "read_file",
            "arguments": json!({
                "path": "/tmp/test.txt"
            }).to_string()
        }
    });

    // 执行 tool call（内部路由到 MCP Connector）
    let result = executor.execute_tool(&tool_call, trace_id).await?;

    println!("Tool result: {}", result.content);

    Ok(())
}
```

### 4. 完整集成示例

```rust
use cyberclaw_connectors::{
    CapabilityDispatcher,
    ConnectorRegistry,
    mcp::{McpConnector, McpServerConfig, TransportConfig},
};
use cyberclaw_llm_bridge::{
    ToolExecutor,
    ToolCallMapper,
    register_standard_mappings,
};
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. 初始化 tracing
    tracing_subscriber::fmt::init();

    // 2. 创建 ConnectorRegistry
    let registry = Arc::new(ConnectorRegistry::new());

    // 3. 注册 Local Connector (标准能力)
    use cyberclaw_connectors::LocalConnector;
    let workspace = std::env::current_dir()?;
    let local_connector = LocalConnector::new(workspace);
    registry.register(Arc::new(local_connector))?;

    // 4. 注册 MCP Connector (文件系统)
    let mcp_fs_config = McpServerConfig {
        name: "filesystem".to_string(),
        transport: TransportConfig::Stdio {
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-filesystem".to_string(),
                "/tmp".to_string()
            ],
            workdir: None,
        },
        timeout: Duration::from_secs(30),
        enable_cache: true,
    };
    let mcp_fs = McpConnector::new(mcp_fs_config).await?;
    registry.register(Arc::new(mcp_fs))?;

    // 5. 可选：注册更多 MCP Servers (GitHub, PostgreSQL 等)
    // ...

    // 6. 创建 Dispatcher
    let dispatcher = Arc::new(CapabilityDispatcher::new(registry));

    // 7. 创建 Mapper 并注册标准映射
    let mapper = Arc::new(ToolCallMapper::new());
    register_standard_mappings(&mapper)?;

    // 8. 创建 ToolExecutor
    let executor = ToolExecutor::new(dispatcher, mapper);

    // 9. 系统就绪，可以处理 LLM Tool Calls
    tracing::info!("CyberClaw system initialized with MCP support");

    // 示例：执行一个 tool call
    let tool_call = serde_json::json!({
        "id": "call-1",
        "type": "function",
        "function": {
            "name": "fs_read",  // 映射到 Local Connector
            "arguments": serde_json::json!({
                "path": "README.md"
            }).to_string()
        }
    });

    let result = executor.execute_tool(
        &tool_call,
        "trace-example".to_string()
    ).await?;

    println!("✓ Tool executed: {}", result.content);

    Ok(())
}
```

## 多 MCP Server 配置

可以同时注册多个 MCP Server：

```rust
// 文件系统 MCP Server
let mcp_fs = McpConnector::new(McpServerConfig {
    name: "filesystem".to_string(),
    transport: TransportConfig::Stdio { /* ... */ },
    // ...
}).await?;
registry.register(Arc::new(mcp_fs))?;

// GitHub MCP Server
let mcp_github = McpConnector::new(McpServerConfig {
    name: "github".to_string(),
    transport: TransportConfig::Stdio {
        command: "npx".to_string(),
        args: vec![
            "-y".to_string(),
            "@modelcontextprotocol/server-github".to_string(),
        ],
        workdir: None,
    },
    timeout: Duration::from_secs(30),
    enable_cache: true,
}).await?;
registry.register(Arc::new(mcp_github))?;

// PostgreSQL MCP Server
let mcp_postgres = McpConnector::new(McpServerConfig {
    name: "postgres".to_string(),
    transport: TransportConfig::Stdio {
        command: "npx".to_string(),
        args: vec![
            "-y".to_string(),
            "@modelcontextprotocol/server-postgres".to_string(),
            "postgresql://localhost/mydb".to_string(),
        ],
        workdir: None,
    },
    timeout: Duration::from_secs(30),
    enable_cache: true,
}).await?;
registry.register(Arc::new(mcp_postgres))?;
```

## 能力发现流程

```text
┌──────────────────────┐
│ 初始化 MCP Connector  │
└──────────┬───────────┘
           │
           ▼
┌──────────────────────┐
│ 发现 Tools           │  tools/list
│ 发现 Resources       │  resources/list
│ 发现 Prompts         │  prompts/list
└──────────┬───────────┘
           │
           ▼
┌──────────────────────┐
│ 映射为 Capabilities   │
│ - mcp.tool.{name}    │
│ - mcp.resource.{uri} │
│ - mcp.prompt.{name}  │
└──────────┬───────────┘
           │
           ▼
┌──────────────────────┐
│ 注册到 Registry       │
└──────────────────────┘
```

## Tool Call 到 MCP 的映射

### 标准映射（可选）

如果需要为 MCP 工具创建标准映射：

```rust
use cyberclaw_llm_bridge::ToolCallMapping;

// 注册 MCP Tool 的映射
mapper.register_mapping(ToolCallMapping {
    tool_name: "read_mcp_file".to_string(),
    capability_id: "mcp.tool.read_file".to_string(),
    connector_id: Some("mcp-filesystem".to_string()),
    transform_input: Box::new(|args| {
        // 转换 LLM 参数到 MCP 格式
        Ok(args)
    }),
    transform_output: Box::new(|result| {
        // 转换 MCP 结果到 LLM 格式
        Ok(result)
    }),
})?;
```

### 动态映射

MCP 能力在发现后自动可用，无需显式映射。Dispatcher 会根据 `capability_id` 自动路由。

## 错误处理

MCP Connector 的错误会通过 Bridge 层传播：

```rust
match executor.execute_tool(&tool_call, trace_id).await {
    Ok(result) => {
        // 成功
        println!("Result: {}", result.content);
    }
    Err(e) => {
        // 错误处理
        if let Some(bridge_err) = e.downcast_ref::<BridgeError>() {
            match bridge_err {
                BridgeError::CapabilityNotFound(_) => {
                    // MCP 工具未发现
                }
                BridgeError::ExecutionFailed(_) => {
                    // MCP 执行失败
                }
                _ => {}
            }
        }
    }
}
```

## 治理与审批

MCP 能力遵循 CyberClaw 治理流程：

1. **风险评估**:
   - Tool: `RiskLevel::Medium` (可执行)
   - Resource: `RiskLevel::Low` (只读)
   - Prompt: `RiskLevel::Low` (只读)

2. **审批流程**:
   - High/Medium 风险需要审批
   - Low 风险可自动通过

3. **审计追踪**:
   - 所有 MCP 调用都有 `trace_id` 和 `execution_id`
   - 审计日志记录完整调用链

## 监控与观测

```rust
use cyberclaw_observability::event::Event;

// MCP Connector 自动产生事件
// - mcp.tool.called
// - mcp.resource.read
// - mcp.prompt.retrieved
// - mcp.error.occurred

// 可以通过 observability 层订阅这些事件
```

## 配置建议

### 生产环境

```rust
McpServerConfig {
    name: "production-mcp".to_string(),
    transport: TransportConfig::Http {
        url: "https://mcp.production.example.com".to_string(),
        headers: {
            let mut h = HashMap::new();
            h.insert("Authorization".to_string(),
                     std::env::var("MCP_TOKEN")?);
            h
        },
    },
    timeout: Duration::from_secs(60),  // 生产环境更长超时
    enable_cache: true,                // 启用缓存提升性能
}
```

### 开发环境

```rust
McpServerConfig {
    name: "dev-mcp".to_string(),
    transport: TransportConfig::Stdio {
        command: "npx".to_string(),
        args: vec![/* ... */],
        workdir: Some("/tmp/mcp-dev".to_string()),
    },
    timeout: Duration::from_secs(10),   // 开发环境短超时，快速失败
    enable_cache: false,                // 开发时禁用缓存，便于调试
}
```

## 常见问题

### Q: 如何为 MCP Tool 创建 LLM Function Definition？

A: MCP Tool 的 `input_schema` 就是 JSON Schema，可以直接用于 LLM：

```rust
// 获取 MCP Tool 的 schema
let tools = mcp_client.list_tools().await?;
for tool in tools {
    println!("Function: {}", tool.name);
    println!("Schema: {}", serde_json::to_string_pretty(&tool.input_schema)?);

    // 可以直接传给 LLM
    let function_def = serde_json::json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.input_schema
        }
    });
}
```

### Q: MCP Connector 是否支持流式响应？

A: 当前版本不支持，MCP 协议本身支持但 Connector 接口是 request-response。未来版本可能支持。

### Q: 如何刷新 MCP 能力？

A: 调用 `refresh_capabilities()` 方法：

```rust
let mcp_connector = /* ... */;
mcp_connector.refresh_capabilities().await?;
```

## 相关文档

- [MCP README](./README.md) - MCP Connector 详细文档
- [Connector 接口](../types.rs) - Connector trait 定义
- [LLM Bridge 文档](../../cyberclaw-llm-bridge/README.md)
- [架构文档](../../../docs/architecture/README.md)
