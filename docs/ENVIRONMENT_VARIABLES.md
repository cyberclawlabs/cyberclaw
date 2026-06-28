# Environment Variables

本文档列出了 CyberClaw Server 的所有环境变量配置。

## 安全相关 (Security)

### JWT_SECRET
- **必需性**: 必需（所有环境）
- **默认值**: 无（必须显式设置）
- **描述**: JWT 签名密钥，用于 API 认证。服务器启动时若未设置将立即 panic。
- **要求**: 至少 32 字符长度
- **生成方法**: `openssl rand -base64 48`
- **示例**: `export JWT_SECRET="your-very-long-and-secure-secret-key-at-least-32-characters"`

### ALLOWED_ORIGINS
- **必需性**: 生产环境强烈建议
- **默认值**: `http://localhost:3000,http://localhost:5173` (仅开发环境)
- **描述**: 允许的 CORS 来源，逗号分隔
- **示例**: `export ALLOWED_ORIGINS="https://app.example.com,https://admin.example.com"`

### USE_TLS
- **必需性**: 可选
- **默认值**: `false`
- **描述**: 是否启用 TLS/HTTPS
- **示例**: `export USE_TLS=true`

### TLS_CERT_PATH
- **必需性**: 当 `USE_TLS=true` 时必需
- **描述**: TLS 证书文件路径
- **示例**: `export TLS_CERT_PATH="/etc/ssl/certs/server.crt"`

### TLS_KEY_PATH
- **必需性**: 当 `USE_TLS=true` 时必需
- **描述**: TLS 私钥文件路径
- **示例**: `export TLS_KEY_PATH="/etc/ssl/private/server.key"`

### ENVIRONMENT
- **必需性**: 可选
- **默认值**: `development`
- **描述**: 服务器运行环境，设置为 `production` 时启用额外安全特性（如 HSTS 和 TLS 强制）
- **示例**: `export ENVIRONMENT=production`

## 服务器配置 (Server Configuration)

### SERVER_PORT
- **必需性**: 可选
- **默认值**: `8080`
- **描述**: 服务器监听端口
- **示例**: `export SERVER_PORT=443`

### SERVER_HOST
- **必需性**: 可选
- **默认值**: `0.0.0.0`
- **描述**: 服务器监听地址
- **示例**: `export SERVER_HOST=127.0.0.1`

## LLM 配置 (LLM Configuration)

### LLM_PROVIDER
- **必需性**: 可选
- **默认值**: `openai`
- **描述**: LLM 提供商，可选值: `openai`, `anthropic`, `ark`, `generic`
- **示例**: `export LLM_PROVIDER=anthropic`

### LLM_API_KEY
- **必需性**: 必需
- **描述**: LLM API 密钥
- **示例**: `export LLM_API_KEY="sk-..."`

### LLM_BASE_URL
- **必需性**: 可选（`generic` provider 时必需）
- **默认值**: 根据 provider 自动设置
- **描述**: LLM API 基础 URL
- **示例**: `export LLM_BASE_URL="https://api.anthropic.com/v1"`

## 运行时执行 (Runtime Execution)

### CYBERCLAW_CMD_TIMEOUT_MS
- **必需性**: 可选
- **默认值**: `30000`（30 秒）
- **描述**: `cmd.exec` / `cmd.run` / `cmd.run_powershell` 命令工具在模型未显式指定超时时使用的默认超时（毫秒）。用于放宽重型产物生成命令（如容器内 `pip install python-pptx && python build_deck.py`）的时间预算。每次调用显式传入的 `timeout_ms` 优先级更高。有效值（无论来自本变量还是逐调用）会被钳制到上限 600000 毫秒（10 分钟）。流式命令 `cmd.run_streaming` 有独立的 60 秒默认，不受本变量影响。
- **示例**: `export CYBERCLAW_CMD_TIMEOUT_MS=120000`

## 生产环境配置示例

```bash
#!/bin/bash

# 安全配置
export JWT_SECRET="your-very-long-and-secure-secret-key-at-least-32-characters-long"
export ALLOWED_ORIGINS="https://app.cyberclaw.io,https://admin.cyberclaw.io"
export USE_TLS=true
export TLS_CERT_PATH="/etc/ssl/certs/cyberclaw.crt"
export TLS_KEY_PATH="/etc/ssl/private/cyberclaw.key"
export ENVIRONMENT=production

# 服务器配置
export SERVER_PORT=443

# LLM 配置
export LLM_PROVIDER=openai
export LLM_API_KEY="sk-prod-..."

# 启动服务器
./cyberclaw-server
```

## 开发环境配置示例

```bash
#!/bin/bash

# 最小配置（开发环境）
export JWT_SECRET="development-secret-key-at-least-32-characters-long"
export LLM_API_KEY="sk-dev-..."

# 启动服务器
cargo run --bin cyberclaw-server
```

## 安全最佳实践

1. **生产环境必须设置**:
   - `JWT_SECRET`: 使用强随机密钥（至少 32 字符）
   - `ALLOWED_ORIGINS`: 只允许信任的域名
   - `USE_TLS=true`: 启用 HTTPS
   - `ENVIRONMENT=production`: 启用生产环境安全特性

2. **密钥管理**:
   - 不要在代码中硬编码密钥
   - 使用密钥管理服务（如 AWS Secrets Manager, HashiCorp Vault）
   - 定期轮换密钥

3. **TLS 证书**:
   - 使用可信 CA 签发的证书
   - 定期更新证书
   - 考虑使用 Let's Encrypt 自动化证书管理

4. **环境隔离**:
   - 开发、测试、生产环境使用不同的密钥
   - 避免在版本控制中存储 `.env` 文件

## 故障排查

### JWT 密钥错误
```
Error: JWT_SECRET must be at least 32 characters long for security
```
**解决方案**: 设置更长的 JWT_SECRET

### TLS 配置错误
```
Error: TLS_CERT_PATH must be set when USE_TLS=true
```
**解决方案**: 设置 TLS_CERT_PATH 和 TLS_KEY_PATH

### CORS 错误
```
Access-Control-Allow-Origin header missing
```
**解决方案**: 在 ALLOWED_ORIGINS 中添加请求来源

## 相关文档

- [安全政策](../SECURITY.md)
- [开发指南](../DEVELOPMENT.md)
- [部署指南](./DEPLOYMENT.md)