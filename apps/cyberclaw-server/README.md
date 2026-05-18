# CyberClaw HTTP Server

兼容 OpenAI 格式的 Chat Completions API Server。

## 功能特性

- ✅ 兼容 OpenAI Chat Completions API 格式
- ✅ 支持多个 LLM Provider（OpenAI, ARK, Anthropic, Generic）
- ✅ 支持流式和非流式响应
- ✅ 完整的错误处理
- ✅ CORS 支持
- ✅ 请求追踪和日志

## 快速开始

> 💡 **新用户？** 查看 [QUICKSTART.md](QUICKSTART.md) 获取 5 分钟快速上手指南！

### 方法 1: 使用配置文件（推荐）

1. **复制环境变量模板**
   ```bash
   cd apps/cyberclaw-server
   cp .env.example .env
   ```

2. **编辑 `.env` 文件，填入您的配置**
   ```bash
   # 编辑 .env 文件
   vim .env

   # 或者直接设置 MiniMax 配置
   echo "LLM_PROVIDER=minimax" > .env
   echo "LLM_API_KEY=your-minimax-api-key" >> .env
   echo "LLM_BASE_URL=https://api.minimax.chat/v1" >> .env
   echo "LLM_DEFAULT_MODEL=minimax-m2.5" >> .env
   echo "SERVER_PORT=8080" >> .env
   ```

3. **使用启动脚本运行**
   ```bash
   # MiniMax 配置
   ./start-minimax.sh

   # 或使用通用启动脚本
   ../../start-server.sh
   ```

### 方法 2: 使用环境变量

```bash
# LLM Provider 类型（可选值：openai, ark, anthropic, generic, minimax）
export LLM_PROVIDER=minimax

# API Key
export LLM_API_KEY=your-minimax-api-key

# Base URL
export LLM_BASE_URL=https://api.minimax.chat/v1

# 默认模型
export LLM_DEFAULT_MODEL=minimax-m2.5

# 服务器端口（可选，默认 8080）
export SERVER_PORT=8080

# CORS 允许的来源列表（逗号分隔，可选）
export ALLOWED_ORIGINS=http://localhost:3000,http://localhost:5173

# JWT 认证密钥（必需，服务器启动时强制检查，至少 32 字符）
# 生成命令: openssl rand -base64 48
export JWT_SECRET=your-256-bit-secret-at-least-32-chars

# 运行环境（可选值：development, production）
export ENVIRONMENT=development
```

### 方法 3: 运行服务器

```bash
# 开发模式
cargo run -p cyberclaw-server

# 生产模式
cargo build -p cyberclaw-server --release
./target/release/cyberclaw-server
```

服务器将在 `http://0.0.0.0:8080` 启动。

## API 文档

### 健康检查

```bash
# 健康检查
curl http://localhost:8080/health

# 就绪检查
curl http://localhost:8080/ready
```

### Chat Completions（非流式）

```bash
curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4",
    "messages": [
      {"role": "user", "content": "Hello, how are you?"}
    ],
    "temperature": 0.7,
    "max_tokens": 100,
    "stream": false
  }'
```

响应示例：

```json
{
  "id": "chatcmpl-123",
  "object": "chat.completion",
  "created": 1234567890,
  "model": "gpt-4",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "I'm doing well, thank you for asking!"
      },
      "finish_reason": "stop"
    }
  ],
  "usage": {
    "prompt_tokens": 10,
    "completion_tokens": 20,
    "total_tokens": 30
  }
}
```

### Chat Completions（流式）

```bash
curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4",
    "messages": [
      {"role": "user", "content": "Tell me a story"}
    ],
    "stream": true
  }'
```

响应为 Server-Sent Events (SSE) 格式：

```
data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4","choices":[{"index":0,"delta":{"role":"assistant","content":"Once"},"finish_reason":null}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4","choices":[{"index":0,"delta":{"content":" upon"},"finish_reason":null}]}

...
```

## 支持的 LLM Providers

### 1. MiniMax（推荐）

MiniMax 是中国领先的 AI 公司，提供高性能的大语言模型服务。

```bash
export LLM_PROVIDER=minimax
export LLM_API_KEY=your-minimax-api-key
export LLM_BASE_URL=https://api.minimax.chat/v1
export LLM_DEFAULT_MODEL=minimax-m2.5
```

**支持的模型：**
- `minimax-m2.5` - 最新旗舰模型（推荐）
- `abab6.5-chat` - 高性能对话模型
- `abab6.5s-chat` - 快速对话模型
- `abab5.5-chat` - 通用对话模型
- `abab5.5s-chat` - 轻量对话模型

**快速启动：**
```bash
cd apps/cyberclaw-server
./start-minimax.sh
```

### 2. OpenAI

```bash
export LLM_PROVIDER=openai
export LLM_API_KEY=sk-your-openai-key
export LLM_BASE_URL=https://api.openai.com/v1  # 可选
```

### 2. 火山引擎 ARK（推荐 - 支持 MiniMax）

火山引擎 ARK 平台集成了多种大语言模型，包括 MiniMax、豆包等。

```bash
export LLM_PROVIDER=ark
export LLM_API_KEY=your-ark-api-key
export LLM_BASE_URL=https://ark.cn-beijing.volces.com/api/v3
export LLM_DEFAULT_MODEL=minimax-abab6.5  # 或 doubao-pro-32k
```

**支持的模型：**
- **MiniMax 系列:**
  - `minimax-abab6.5` - MiniMax 6.5（推荐）
  - `minimax-abab6` - MiniMax 6.0
- **豆包系列:**
  - `doubao-pro-32k` - 豆包 Pro 32K
  - `doubao-lite-32k` - 豆包 Lite 32K

**快速启动：**
```bash
cd apps/cyberclaw-server
./start-ark-minimax.sh
```

**使用 Endpoint ID（推荐）：**
```bash
export ARK_ENDPOINT_ID=ep-xxxxx-xxxxx  # 从火山引擎控制台获取
```

参考文档：[火山方舟 API 文档](https://www.volcengine.com/docs/82379/1928261)

### 3. Anthropic

```bash
export LLM_PROVIDER=anthropic
export LLM_API_KEY=sk-ant-your-key
export LLM_BASE_URL=https://api.anthropic.com/v1  # 可选
```

### 4. Generic (通用 OpenAI 兼容接口)

适用于 DeepSeek、Ollama 等兼容 OpenAI API 的服务：

```bash
export LLM_PROVIDER=generic
export LLM_API_KEY=your-api-key  # 某些本地服务可能不需要
export LLM_BASE_URL=http://localhost:11434/v1  # 必需
```

## 测试

### 运行单元测试

```bash
cargo test -p cyberclaw-server
```

### 运行集成测试

```bash
cargo test -p cyberclaw-server --test chat_api_test
```

### 运行 E2E 测试

完整的端到端测试流程，验证 HTTP 服务器的所有功能：

```bash
# 运行完整 E2E 测试套件
./run-e2e-tests.sh
```

E2E 测试包括：
- ✅ HTTP 服务器启动和停止
- ✅ 健康检查端点 (`/health`, `/ready`)
- ✅ Chat Completions API
- ✅ 错误处理（无效 JSON、不存在路由）
- ✅ 并发请求处理
- ✅ 性能基准测试

测试结果详见 [E2E_TEST_REPORT.md](../../E2E_TEST_REPORT.md)

## 环境变量配置

### 核心配置

| 变量名 | 说明 | 默认值 | 示例 |
|--------|------|--------|------|
| `LLM_PROVIDER` | LLM 提供商类型 | - | `minimax`, `openai`, `ark`, `anthropic`, `generic` |
| `LLM_API_KEY` | LLM API 密钥 | - | `sk-xxx...` |
| `LLM_BASE_URL` | LLM 服务基础 URL | - | `https://api.minimax.chat/v1` |
| `LLM_DEFAULT_MODEL` | 默认使用的模型 | - | `minimax-m2.5` |
| `SERVER_PORT` | 服务器监听端口 | `8080` | `8080` |

### 安全配置

| 变量名 | 说明 | 默认值 | 示例 |
|--------|------|--------|------|
| `ALLOWED_ORIGINS` | CORS 允许的来源列表（逗号分隔） | `http://localhost:3000,http://localhost:5173` | `https://app.example.com,https://admin.example.com` |
| `JWT_SECRET` | JWT 签名密钥（**必需**，至少 32 字符，未设置时服务器 panic 拒绝启动） | 无默认值 | `openssl rand -base64 48` 生成的值 |
| `ENVIRONMENT` | 服务器运行环境（控制 TLS 强制检查和安全头启用） | `development` | `production`, `development` |
| `USE_TLS` | 是否启用 TLS/HTTPS | `false` | `true`, `false` |
| `TLS_CERT_PATH` | TLS 证书文件路径（`USE_TLS=true` 时必需） | - | `/etc/ssl/certs/cert.pem` |
| `TLS_KEY_PATH` | TLS 私钥文件路径（`USE_TLS=true` 时必需） | - | `/etc/ssl/private/key.pem` |

### 速率限制与请求体限制

| 变量名 | 说明 | 默认值 | 示例 |
|--------|------|--------|------|
| `RATE_LIMIT_PER_SECOND` | 每秒允许的请求数 | `1` | `10`, `100` |
| `RATE_LIMIT_BURST_SIZE` | 允许的突发请求数 | `60` | `100`, `200` |
| `MAX_REQUEST_BODY_SIZE` | 最大请求体大小（字节） | `10485760` (10 MB) | `5242880` (5 MB) |

**生产环境安全建议：**

1. **CORS 配置：**
   - 生产环境必须通过 `ALLOWED_ORIGINS` 显式配置允许的来源
   - 不要使用通配符 `*`（已在代码层面禁止）
   - 示例：`export ALLOWED_ORIGINS=https://app.example.com,https://admin.example.com`

2. **JWT 密钥（启动前必须配置）：**
   - `JWT_SECRET` 是必需的环境变量，未设置时服务器将立即 panic 拒绝启动
   - 密钥长度至少 32 字符，建议使用 `openssl rand -base64 48` 生成 64 字符密钥
   - 绝不要将密钥提交到版本控制系统（确保 `.env` 在 `.gitignore` 中）
   - 生产环境请使用 HashiCorp Vault、AWS Secrets Manager 等密钥管理服务

3. **TLS/HTTPS 强制要求（重要）：**
   - 设置 `ENVIRONMENT=production` 后，服务器将**强制要求** TLS 启用
   - 若 `ENVIRONMENT=production` 但 `USE_TLS=false`（或未设置），服务器将以 panic 拒绝启动
   - 生产环境 TLS 配置步骤：
     ```bash
     export ENVIRONMENT=production
     export USE_TLS=true
     export TLS_CERT_PATH=/etc/ssl/certs/cyberclaw/cert.pem
     export TLS_KEY_PATH=/etc/ssl/private/cyberclaw/key.pem
     ```
   - 使用 Let's Encrypt 获取免费证书：
     ```bash
     certbot certonly --standalone -d your-domain.com
     ```
   - 测试用自签名证书（仅开发/测试）：
     ```bash
     openssl req -x509 -newkey rsa:4096 -nodes -keyout key.pem -out cert.pem -days 365
     ```
   - 参考生产环境配置模板：`.env.production`

4. **环境隔离：**
   - 开发、测试、生产环境使用不同的 JWT 密钥
   - 开发环境可以使用 localhost CORS 白名单

5. **速率限制：**
   - 生产环境根据实际负载调整 `RATE_LIMIT_PER_SECOND` 和 `RATE_LIMIT_BURST_SIZE`
   - 防止 API 滥用和 DoS 攻击
   - 默认配置：每秒 1 次请求，允许突发 60 次（即每分钟最多 60 次请求）
   - 超过限制返回 HTTP 429 Too Many Requests

6. **请求体大小限制：**
   - 默认限制为 10 MB，可通过 `MAX_REQUEST_BODY_SIZE` 调整
   - 防止过大的请求体导致资源耗尽
   - 超过限制返回 HTTP 413 Payload Too Large

## 项目结构

```
apps/cyberclaw-server/
├── Cargo.toml
├── README.md
├── config.toml              # 配置文件（包含 MiniMax 等 LLM 配置）
├── .env.example             # 环境变量配置示例
├── start-minimax.sh         # MiniMax 启动脚本
├── src/
│   ├── lib.rs               # 库入口（供测试使用）
│   ├── main.rs              # 二进制入口
│   ├── config.rs            # 配置管理
│   ├── api/
│   │   ├── mod.rs
│   │   ├── chat.rs          # Chat Completions API
│   │   ├── health.rs        # 健康检查 API
│   │   ├── agents.rs        # Agent 管理 API
│   │   └── tasks.rs         # 任务管理 API
│   ├── error.rs             # 错误处理
│   ├── middleware/
│   │   ├── mod.rs           # 中间件模块
│   │   ├── auth.rs          # JWT 认证中间件
│   │   ├── security_headers.rs  # 安全响应头中间件
│   │   ├── rate_limit.rs    # 速率限制中间件
│   │   └── body_limit.rs    # 请求体大小限制中间件
│   ├── handlers/
│   │   └── mod.rs           # 请求处理器
│   └── state.rs             # 应用状态
└── tests/
    ├── chat_api_test.rs     # 集成测试（Mock LLM）
    ├── api_crud_test.rs     # CRUD API 测试
    ├── rate_limit_test.rs   # 速率限制和请求体大小限制测试
    └── server_e2e_test.rs   # E2E 测试（实际 HTTP 服务器）
```

## 开发

### 添加新的 API 端点

1. 在 `src/api/` 创建新模块
2. 实现路由和处理函数
3. 在 `src/api/mod.rs` 导出
4. 在 `src/lib.rs` 的 `create_router` 中注册

### 添加中间件

在 `src/lib.rs` 的 `create_router` 函数中添加：

```rust
Router::new()
    .merge(api::create_chat_router())
    .layer(your_middleware())  // 添加在这里
    .layer(TraceLayer::new_for_http())
    .layer(cors)
    .with_state(state)
```

## 许可证

Apache-2.0
