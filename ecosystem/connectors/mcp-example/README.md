# CyberClaw MCP Example Server

这是一个示例 MCP (Model Context Protocol) Server，演示了如何为 CyberClaw 平台提供工具和资源。

## 功能特性

### Tools (工具)
- **calculate**: 数学计算（加减乘除、幂、平方根）
- **text_transform**: 文本转换（大小写、反转、Base64 编解码）
- **data_analysis**: 数据分析（均值、中位数、众数、标准差等）
- **http_request**: HTTP 请求（GET、POST、PUT、DELETE）

### Resources (资源)
- **config://server**: 服务器配置信息
- **data://sample**: 示例数据集
- **template://report**: 报告模板

### Prompts (提示模板)
- **analyze_code**: 代码分析模板
- **generate_docs**: 文档生成模板

## 快速开始

### 安装依赖

```bash
npm install
```

### 启动服务器

```bash
# 生产模式
npm start

# 开发模式（自动重启）
npm run dev
```

### 测试

```bash
npm test
```

## API 端点

### JSON-RPC 端点

```
POST http://localhost:8080/rpc
```

### 健康检查

```
GET http://localhost:8080/health
```

### REST 端点（方便调试）

```
GET http://localhost:8080/tools      # 列出所有工具
GET http://localhost:8080/resources  # 列出所有资源
```

## 使用示例

### 列出所有工具

```bash
curl -X POST http://localhost:8080/rpc \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "method": "tools/list",
    "id": 1
  }'
```

### 调用计算工具

```bash
curl -X POST http://localhost:8080/rpc \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "method": "tools/call",
    "params": {
      "name": "calculate",
      "arguments": {
        "operation": "add",
        "a": 5,
        "b": 3
      }
    },
    "id": 2
  }'
```

### 读取资源

```bash
curl -X POST http://localhost:8080/rpc \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "method": "resources/read",
    "params": {
      "uri": "config://server"
    },
    "id": 3
  }'
```

## 与 CyberClaw 集成

### 配置 MCP Connector

```rust
// 在 CyberClaw 中配置
let mcp_connector = McpConnector::new(
    McpConfig {
        server_url: "http://localhost:8080/rpc".to_string(),
        timeout: Duration::from_secs(30),
    }
).await?;

// 注册到 Control Plane
control_plane.register_connector(mcp_connector).await?;
```

### 使用工具

```rust
// 执行计算
let result = control_plane.execute_capability(
    "mcp.tool.calculate",
    json!({
        "operation": "multiply",
        "a": 7,
        "b": 8
    })
).await?;

// 文本转换
let result = control_plane.execute_capability(
    "mcp.tool.text_transform",
    json!({
        "text": "Hello World",
        "transformation": "reverse"
    })
).await?;
```

## 扩展指南

### 添加新工具

1. 在 `tools` 数组中定义工具：

```javascript
{
  name: 'my_tool',
  description: 'My custom tool',
  inputSchema: {
    type: 'object',
    properties: {
      // 定义参数
    }
  }
}
```

2. 实现工具逻辑：

```javascript
function executeMyTool(args) {
  // 实现逻辑
  return result;
}
```

3. 在 `executeTool` 函数中添加 case：

```javascript
case 'my_tool':
  return executeMyTool(args);
```

### 添加新资源

1. 在 `resources` 数组中定义资源：

```javascript
{
  uri: 'custom://myresource',
  name: 'My Resource',
  mimeType: 'application/json',
  description: 'My custom resource'
}
```

2. 在 `readResource` 函数中实现读取逻辑：

```javascript
case 'custom://myresource':
  return {
    // 返回资源内容
  };
```

## 配置选项

### 环境变量

- `PORT`: 服务器端口（默认: 8080）
- `LOG_LEVEL`: 日志级别（默认: info）

### 日志

日志同时输出到：
- 控制台
- `mcp-server.log` 文件

## 故障排除

### 端口被占用

```bash
# 查找占用端口的进程
lsof -i :8080

# 使用其他端口
PORT=8081 npm start
```

### 依赖安装失败

```bash
# 清理缓存
npm cache clean --force

# 重新安装
rm -rf node_modules package-lock.json
npm install
```

## 性能优化

- 使用连接池管理外部请求
- 实施请求缓存
- 使用异步操作
- 合理设置超时

## 安全注意事项

- 验证所有输入参数
- 限制资源访问权限
- 使用 HTTPS 在生产环境
- 实施速率限制

## 许可证

Apache 2.0 License

## 支持

- 文档：https://docs.cyberclaw.io/mcp
- 论坛：https://forum.cyberclaw.io
- Issues：https://github.com/cyberclaw/mcp-example/issues