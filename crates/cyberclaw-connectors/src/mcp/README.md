# MCP (Model Context Protocol) Connector

MCP Connector 实现了 [Model Context Protocol](https://modelcontextprotocol.io/) 规范，用于将外部工具、资源和提示模板桥接到 CyberClaw 平台。

## 架构

```text
┌────────────────────┐
│  MCP Connector     │
│                    │
│  ┌──────────────┐  │
│  │ JSON-RPC     │  │
│  │ Client       │  │
│  └──────────────┘  │
│         │          │
│  ┌──────▼──────┐   │
│  │ Transport   │   │
│  │ stdio/HTTP  │   │
│  └─────────────┘   │
└────────────────────┘
        │
        ▼
┌────────────────────┐
│  MCP Server        │
│  (External)        │
└────────────────────┘
```

## 功能特性

- **JSON-RPC 2.0 协议**: 完整实现 JSON-RPC 2.0 规范
- **多种传输方式**: 支持 Stdio 和 HTTP/HTTPS 传输
- **动态能力发现**: 自动发现并注册 MCP Server 提供的工具、资源、提示模板
- **能力映射**: 将 MCP 实体映射到 CyberClaw Capability 系统
- **响应缓存**: 可选的响应缓存机制，减少重复请求
- **错误处理**: 标准 JSON-RPC 错误码和重试逻辑

## 核心概念

### MCP 实体类型

1. **Tool**: 可执行的工具/函数
   - 映射为 `CapabilityEffect::Execute`
   - 风险级别：`Medium`
   - 能力 ID: `mcp.tool.{tool_name}`

2. **Resource**: 可读取的资源（文件、API 数据等）
   - 映射为 `CapabilityEffect::Read`
   - 风险级别：`Low`
   - 能力 ID: `mcp.resource.{sanitized_uri}`

3. **Prompt**: 提示模板
   - 映射为 `CapabilityEffect::Read`
   - 风险级别：`Low`
   - 能力 ID: `mcp.prompt.{prompt_name}`

### 传输配置

#### Stdio Transport
```rust
use cyberclaw_connectors::mcp::{McpServerConfig, TransportConfig};
use std::time::Duration;

let config = McpServerConfig {
    name: "my-mcp-server".to_string(),
    transport: TransportConfig::Stdio {
        command: "npx".to_string(),
        args: vec!["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
        workdir: None,
    },
    timeout: Duration::from_secs(30),
    enable_cache: true,
};
```

#### HTTP Transport
```rust
use std::collections::HashMap;

let config = McpServerConfig {
    name: "remote-mcp-server".to_string(),
    transport: TransportConfig::Http {
        url: "https://api.example.com/mcp".to_string(),
        headers: {
            let mut h = HashMap::new();
            h.insert("Authorization".to_string(), "Bearer token".to_string());
            h
        },
    },
    timeout: Duration::from_secs(30),
    enable_cache: true,
};
```

## 使用示例

### 创建 MCP Connector

```rust
use cyberclaw_connectors::mcp::{McpConnector, McpServerConfig, TransportConfig};
use cyberclaw_connectors::ConnectorRegistry;
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 配置 MCP Server
    let config = McpServerConfig {
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

    // 创建 Connector
    let connector = McpConnector::new(config).await?;

    // 注册到全局注册表
    let registry = ConnectorRegistry::global();
    registry.register(Arc::new(connector))?;

    Ok(())
}
```

### 执行 MCP Tool

```rust
use cyberclaw_connectors::types::{CapabilityExecutionRequest, ExecutionStatus};
use cyberclaw_connectors::Connector;
use cyberclaw_core::prelude::*;
use serde_json::json;

async fn execute_tool(connector: &impl Connector) -> anyhow::Result<()> {
    let request = CapabilityExecutionRequest {
        execution_id: ExecutionId::from_string("exec-1")?,
        trace_id: TraceId::from_string("trace-1")?,
        capability_id: CapabilityId::from_string("mcp.tool.read_file")?,
        connector_id: connector.id().clone(),
        input: json!({
            "path": "/tmp/test.txt"
        }),
        context: None,
    };

    let result = connector.execute(request).await?;

    match result.status {
        ExecutionStatus::Success => {
            println!("Result: {}", result.output);
        }
        ExecutionStatus::Failed => {
            eprintln!("Error: {:?}", result.error);
        }
        _ => {}
    }

    Ok(())
}
```

### 读取 MCP Resource

```rust
async fn read_resource(connector: &impl Connector) -> anyhow::Result<()> {
    let request = CapabilityExecutionRequest {
        execution_id: ExecutionId::from_string("exec-2")?,
        trace_id: TraceId::from_string("trace-2")?,
        capability_id: CapabilityId::from_string("mcp.resource.file..tmp.config.json")?,
        connector_id: connector.id().clone(),
        input: json!({}),
        context: None,
    };

    let result = connector.execute(request).await?;
    println!("Resource content: {}", result.output);

    Ok(())
}
```

### 获取 Prompt 模板

```rust
async fn get_prompt(connector: &impl Connector) -> anyhow::Result<()> {
    let request = CapabilityExecutionRequest {
        execution_id: ExecutionId::from_string("exec-3")?,
        trace_id: TraceId::from_string("trace-3")?,
        capability_id: CapabilityId::from_string("mcp.prompt.code_review")?,
        connector_id: connector.id().clone(),
        input: json!({
            "language": "rust",
            "file_path": "src/main.rs"
        }),
        context: None,
    };

    let result = connector.execute(request).await?;
    println!("Prompt template: {}", result.output);

    Ok(())
}
```

## 能力发现

MCP Connector 在初始化时会自动发现并注册所有可用的能力：

1. **工具发现**: 调用 `tools/list` 方法获取所有工具
2. **资源发现**: 调用 `resources/list` 方法获取所有资源
3. **提示模板发现**: 调用 `prompts/list` 方法获取所有提示模板
4. **能力注册**: 将发现的实体注册为 CyberClaw Capability

### 刷新能力

```rust
let connector = McpConnector::new(config).await?;

// 初始化后刷新能力（重新发现）
connector.refresh_capabilities().await?;
```

## JSON-RPC 协议

### 请求格式

```json
{
  "jsonrpc": "2.0",
  "method": "tools/call",
  "params": {
    "name": "read_file",
    "arguments": {
      "path": "/tmp/test.txt"
    }
  },
  "id": "req-123"
}
```

### 响应格式

```json
{
  "jsonrpc": "2.0",
  "result": {
    "content": "file content here"
  },
  "id": "req-123"
}
```

### 错误响应

```json
{
  "jsonrpc": "2.0",
  "error": {
    "code": -32601,
    "message": "Method not found: invalid_method"
  },
  "id": "req-123"
}
```

### 标准错误码

| 错误码 | 含义 | 描述 |
|-------|------|------|
| -32700 | Parse error | JSON 解析失败 |
| -32600 | Invalid Request | 请求格式无效 |
| -32601 | Method not found | 方法不存在 |
| -32602 | Invalid params | 参数无效 |
| -32603 | Internal error | 服务器内部错误 |

## 缓存机制

MCP Connector 支持响应缓存以减少重复请求：

```rust
let config = McpServerConfig {
    // ... 其他配置
    enable_cache: true,  // 启用缓存
};
```

缓存特性：
- 基于 `(method, params)` 的键值缓存
- 默认 TTL: 5 分钟
- LRU 淘汰策略
- 自动清理过期条目

## URI 清理规则

资源 URI 会被清理为有效的 Capability ID：

| 原始 URI | 清理后的 ID |
|---------|-----------|
| `file:///path/to/file.txt` | `file..path.to.file.txt` |
| `http://example.com/api/v1` | `http.example.com.api.v1` |
| `custom://resource-name` | `custom.resource-name` |

清理规则：
- `file:///` (三斜杠) → `..`
- `://` → `.`
- `/` 和 `:` → `.`
- 仅保留字母数字、`.`、`_`、`-`

## 测试

```bash
# 运行所有 MCP 测试
cargo test -p cyberclaw-connectors --lib mcp

# 运行特定测试
cargo test -p cyberclaw-connectors test_mcp_tool_serialization

# 运行集成测试
cargo test -p cyberclaw-connectors mcp::tests::integration
```

## Provider 实现指南

### 标准 MCP Server

MCP Connector 可以连接任何符合 MCP 规范的服务器：

- [@modelcontextprotocol/server-filesystem](https://github.com/modelcontextprotocol/servers/tree/main/src/filesystem) - 文件系统访问
- [@modelcontextprotocol/server-github](https://github.com/modelcontextprotocol/servers/tree/main/src/github) - GitHub API
- [@modelcontextprotocol/server-postgres](https://github.com/modelcontextprotocol/servers/tree/main/src/postgres) - PostgreSQL 数据库
- 自定义 MCP Server

### 自定义 MCP Server 实现

如果需要实现自定义 MCP Server，请参考：
- [MCP 规范文档](https://spec.modelcontextprotocol.io/)
- [MCP SDK](https://github.com/modelcontextprotocol/typescript-sdk)

### OpenAI/Anthropic 集成

**注意**: OpenAI 和 Anthropic 的 LLM API 集成**不在 MCP Connector 范围内**。

- MCP Connector 负责桥接**外部工具和资源**
- LLM API 集成应在 `cyberclaw-llm-bridge` crate 中实现

## 安全考虑

1. **权限控制**: 所有 MCP 能力都经过治理层审批
2. **风险分级**:
   - Tool: Medium (可执行，需审批)
   - Resource: Low (只读，低风险)
   - Prompt: Low (只读，低风险)
3. **输入验证**: 基于 JSON Schema 验证输入参数
4. **超时控制**: 可配置的请求超时时间
5. **错误消毒**: 敏感信息不会暴露在错误消息中

## 性能优化

1. **响应缓存**: 减少重复请求
2. **连接复用**: HTTP 传输复用连接
3. **并发请求**: 支持并发处理多个请求
4. **懒加载**: 能力按需发现和加载

## 故障排查

### 问题: Stdio Transport 启动失败

```
Error: Failed to spawn process: No such file or directory
```

**解决方案**:
- 检查 `command` 路径是否正确
- 确保命令在 PATH 中可用
- 验证 `workdir` 存在且可访问

### 问题: HTTP Transport 连接超时

```
Error: Request timed out after 30s
```

**解决方案**:
- 增加 `timeout` 配置
- 检查网络连接
- 验证服务器 URL 正确

### 问题: 能力未发现

```
Error: No MCP mapping found for capability: mcp.tool.my_tool
```

**解决方案**:
- 调用 `refresh_capabilities()` 重新发现
- 检查 MCP Server 是否正确实现了 `tools/list` 等方法
- 查看日志确认发现过程

## 相关资源

- [MCP 官方网站](https://modelcontextprotocol.io/)
- [MCP 规范](https://spec.modelcontextprotocol.io/)
- [MCP Servers 列表](https://github.com/modelcontextprotocol/servers)
- [CyberClaw 文档](../../docs/INDEX.md)
- [Connector 接口文档](../types.rs)
