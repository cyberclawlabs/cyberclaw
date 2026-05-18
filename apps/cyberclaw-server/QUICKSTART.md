# CyberClaw Server 快速开始指南

本指南帮助您快速配置和启动 CyberClaw Server，特别是使用 **MiniMax** 模型。

## 🚀 5 分钟快速启动（MiniMax）

### 步骤 1: 准备 API Key

前往 [MiniMax 控制台](https://platform.minimaxi.com/) 获取您的 API Key。

### 步骤 2: 配置环境变量

```bash
cd apps/cyberclaw-server

# 复制配置模板
cp .env.example .env

# 编辑 .env 文件，填入您的 API Key
echo "LLM_PROVIDER=minimax" > .env
echo "LLM_API_KEY=YOUR_MINIMAX_API_KEY" >> .env
echo "LLM_BASE_URL=https://api.minimax.chat/v1" >> .env
echo "LLM_DEFAULT_MODEL=minimax-m2.5" >> .env
echo "SERVER_PORT=8080" >> .env
```

### 步骤 3: 启动服务器

```bash
# 使用 MiniMax 启动脚本
./start-minimax.sh
```

### 步骤 4: 测试 API

```bash
# 健康检查
curl http://localhost:8080/health

# 测试 Chat Completions
curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "minimax-m2.5",
    "messages": [
      {"role": "user", "content": "你好，介绍一下你自己"}
    ],
    "temperature": 0.7,
    "max_tokens": 200,
    "stream": false
  }'
```

## 📋 配置文件说明

### config.toml

包含服务器和 LLM 的所有配置选项。默认配置已设置为使用 MiniMax。

**关键配置项：**
```toml
[llm]
provider = "minimax"
model = "minimax-m2.5"
base_url = "https://api.minimax.chat/v1"

[llm.minimax]
default_model = "minimax-m2.5"
models = [
    "minimax-m2.5",
    "abab6.5-chat",
    "abab6.5s-chat",
    "abab5.5-chat",
    "abab5.5s-chat"
]
```

### .env 文件

用于存储敏感配置（如 API Key）。此文件已在 `.gitignore` 中，不会提交到版本控制。

**示例：**
```bash
LLM_PROVIDER=minimax
LLM_API_KEY=your-minimax-api-key-here
LLM_BASE_URL=https://api.minimax.chat/v1
LLM_DEFAULT_MODEL=minimax-m2.5
SERVER_PORT=8080
```

## 🔧 其他 LLM 提供商配置

### 火山引擎 ARK

```bash
export LLM_PROVIDER=ark
export LLM_API_KEY=your-ark-api-key
export LLM_BASE_URL=https://ark.cn-beijing.volces.com/api/v3
export LLM_DEFAULT_MODEL=doubao-pro-32k
```

### OpenAI

```bash
export LLM_PROVIDER=openai
export LLM_API_KEY=sk-your-openai-key
export LLM_BASE_URL=https://api.openai.com/v1
export LLM_DEFAULT_MODEL=gpt-4
```

### Anthropic

```bash
export LLM_PROVIDER=anthropic
export LLM_API_KEY=sk-ant-your-key
export LLM_BASE_URL=https://api.anthropic.com/v1
export LLM_DEFAULT_MODEL=claude-3-5-sonnet-20241022
```

### 本地模型（Ollama）

```bash
export LLM_PROVIDER=generic
export LLM_API_KEY=not-required
export LLM_BASE_URL=http://localhost:11434/v1
export LLM_DEFAULT_MODEL=llama2
```

## 🧪 运行测试

### 单元测试

```bash
cd apps/cyberclaw-server
cargo test
```

### E2E 测试

```bash
# 从项目根目录运行
./run-e2e-tests.sh
```

## 📝 API 使用示例

### 非流式响应

```bash
curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "minimax-m2.5",
    "messages": [
      {"role": "system", "content": "你是一个有帮助的AI助手"},
      {"role": "user", "content": "什么是机器学习？"}
    ],
    "temperature": 0.7,
    "max_tokens": 500,
    "stream": false
  }'
```

### 流式响应

```bash
curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "minimax-m2.5",
    "messages": [
      {"role": "user", "content": "写一首关于春天的诗"}
    ],
    "stream": true
  }'
```

## ❓ 常见问题

### Q1: 服务器启动失败

**A:** 检查以下几点：
- 确认 API Key 已正确设置
- 检查端口 8080 是否被占用
- 查看日志文件：`cat /tmp/cyberclaw-server.log`

### Q2: API 返回 404 错误

**A:** 可能是模型名称不正确。请确认：
- 模型名称拼写正确
- 您的账户有权访问该模型
- 对于火山引擎 ARK，可能需要使用 Endpoint ID（`ep-xxxxx` 格式）

### Q3: 如何切换到其他模型？

**A:** 修改 `.env` 文件中的 `LLM_DEFAULT_MODEL`：
```bash
# 切换到 abab6.5-chat
LLM_DEFAULT_MODEL=abab6.5-chat
```

### Q4: 如何查看服务器日志？

**A:** 服务器日志输出到标准输出。如果使用启动脚本，日志会保存到 `/tmp/`:
```bash
tail -f /tmp/cyberclaw-server-minimax.log
```

## 🔐 安全提示

1. **永远不要**将 `.env` 文件提交到版本控制
2. **使用环境变量**存储 API Key，而不是硬编码在配置文件中
3. **定期轮换** API Key
4. **限制权限**：为服务器账户配置最小权限

## 📚 更多资源

- [完整 README](README.md)
- [E2E 测试报告](../../E2E_TEST_REPORT.md)
- [MiniMax 官方文档](https://platform.minimaxi.com/document)
- [OpenAI API 兼容性说明](https://platform.openai.com/docs/api-reference)

## 💡 下一步

1. **生产部署**: 考虑使用 Docker 容器化部署
2. **负载均衡**: 配置 Nginx 或 HAProxy
3. **监控告警**: 集成 Prometheus + Grafana
4. **日志聚合**: 使用 ELK Stack 或 Loki

---

**需要帮助？** 请查看 [README.md](README.md) 或提交 Issue。
