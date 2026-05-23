# CyberClaw 部署指南

本指南涵盖 CyberClaw 服务在开发、测试和生产环境中的部署流程。

- **文档版本**: 1.0
- **更新时间**: 2026-03-28
- **适用版本**: CyberClaw 2.0+

## 快速导航

- [本地开发部署](#本地开发部署)
- [生产环境部署](#生产环境部署)
- [Docker 部署](#docker-部署)
- [TLS/HTTPS 配置](#tlshttps-配置)
- [健康检查](#健康检查)
- [故障排查](#故障排查)
- [安全检查清单](#安全检查清单)

---

## 前置要求

### 最小要求

- **Rust**: 1.70+ (仅源码构建需要)
- **Docker**: 20.10+ (容器部署)
- **Docker Compose**: 2.0+ (容器编排)

### 运行时要求

- **有效的 LLM API 密钥** (OpenAI, Anthropic, ARK, 或 Generic)
- **JWT_SECRET** (生产环境强制)
- **网络连接** (访问 LLM API 服务)

### 推荐配置

- **生产环境**: 2+ CPU 核心, 4GB+ 内存
- **开发环境**: 1+ CPU 核心, 2GB+ 内存

---

## 本地开发部署

### 方式 1: 直接运行 (推荐用于快速开发)

**1. 配置环境变量**

```bash
# 最小化配置
export LLM_API_KEY="sk-your-api-key"
export JWT_SECRET="dev-secret-at-least-32-characters-long"

# 可选配置
export LLM_PROVIDER="openai"          # 默认: openai
export CYBERCLAW_ADDR="127.0.0.1:3000"  # 默认: 127.0.0.1:3000
export RUST_LOG="info"                # 日志级别
export ENVIRONMENT="development"      # 运行环境
```

**2. 启动服务**

```bash
# 进入项目根目录
cd /path/to/cyberclaw

# 构建并运行
cargo run -p cyberclaw-server

# 或者先构建后运行（推荐用于反复测试）
cargo build -p cyberclaw-server
./target/debug/cyberclaw-server
```

**3. 验证部署**

```bash
# 检查服务健康状态
curl http://localhost:3000/health

# 测试 Chat API
curl -X POST http://localhost:3000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-3.5-turbo",
    "messages": [{"role": "user", "content": "Hello"}],
    "stream": false
  }'
```

### 方式 2: 使用 .env 文件 (推荐用于复杂配置)

**1. 创建 .env 文件**

```bash
# 在项目根目录创建
cat > .env << 'EOF'
# LLM 配置
LLM_PROVIDER=openai
LLM_API_KEY=sk-your-api-key
LLM_BASE_URL=https://api.openai.com/v1

# 服务器配置
CYBERCLAW_ADDR=127.0.0.1:3000
RUST_LOG=info

# 安全配置
JWT_SECRET=dev-secret-at-least-32-characters-long
EOF
```

**2. 加载 .env 文件并启动**

```bash
# 使用 direnv (推荐)
direnv allow

# 或者手动加载
set -a
source .env
set +a
cargo run -p cyberclaw-server
```

---

## Docker 部署

详细的 Docker 部署指南请查看 [docker.md](docker.md)。

### 快速启动 (开发环境)

```bash
# 构建镜像
docker build -t cyberclaw-server:latest \
  -f apps/cyberclaw-server/Dockerfile .

# 运行容器
docker run -d \
  --name cyberclaw-server \
  -p 3000:3000 \
  -e LLM_API_KEY="sk-your-api-key" \
  -e JWT_SECRET="dev-secret-at-least-32-characters-long" \
  cyberclaw-server:latest

# 查看日志
docker logs -f cyberclaw-server

# 测试服务
curl http://localhost:3000/health
```

### Docker Compose (推荐用于完整部署)

```bash
# 在项目根目录
docker-compose -f docs/deployment/docker-compose.yml up -d

# 查看服务状态
docker-compose -f docs/deployment/docker-compose.yml ps

# 查看日志
docker-compose -f docs/deployment/docker-compose.yml logs -f cyberclaw-server

# 停止服务
docker-compose -f docs/deployment/docker-compose.yml down
```

---

## 生产环境部署

### 预部署检查

```bash
# 1. 验证系统要求
docker --version
docker-compose --version

# 2. 验证可用资源
free -h              # 内存
df -h               # 磁盘空间
nproc               # CPU 核心数

# 3. 检查网络连接
ping api.openai.com  # 或其他 LLM API 域名
```

### 环境准备

**1. 创建部署目录**

```bash
# 推荐使用独立目录
mkdir -p /opt/cyberclaw/{config,logs,certs}
cd /opt/cyberclaw
```

**2. 准备配置文件**

```bash
# 复制示例配置
cp /path/to/cyberclaw/docs/deployment/examples/.env.production.example .env.production

# 编辑配置文件
vim .env.production

# 验证配置
cat .env.production | grep -v "^#" | grep -v "^$"
```

**3. 获取或生成 TLS 证书**

参考 [TLS/HTTPS 配置](#tlshttps-配置) 章节。

### 启动生产服务

**1. 准备 docker-compose.yml**

```bash
# 复制到部署目录
cp /path/to/cyberclaw/docs/deployment/docker-compose.yml .

# 编辑为生产配置（启用 TLS、日志等）
vim docker-compose.yml
```

**2. 启动服务**

```bash
# 前台运行（用于验证）
docker-compose up

# 后台运行（生产环境）
docker-compose up -d

# 验证服务
docker-compose ps
docker-compose logs cyberclaw-server | tail -20
```

**3. 配置反向代理 (可选但推荐)**

使用 Nginx 反向代理以增强安全性和性能：

```bash
# 复制示例配置
cp docs/deployment/examples/nginx.conf.example /etc/nginx/sites-available/cyberclaw

# 编辑配置
sudo vim /etc/nginx/sites-available/cyberclaw

# 启用配置
sudo ln -s /etc/nginx/sites-available/cyberclaw /etc/nginx/sites-enabled/

# 测试 Nginx 配置
sudo nginx -t

# 重新加载 Nginx
sudo systemctl reload nginx
```

---

## TLS/HTTPS 配置

### 方式 1: 自签名证书 (仅限测试)

```bash
# 生成自签名证书（有效期 365 天）
openssl req -x509 -newkey rsa:4096 \
  -keyout /opt/cyberclaw/certs/server.key \
  -out /opt/cyberclaw/certs/server.crt \
  -days 365 -nodes \
  -subj "/CN=localhost/O=CyberClaw/C=US"

# 验证证书
openssl x509 -in /opt/cyberclaw/certs/server.crt -text -noout
```

### 方式 2: Let's Encrypt 证书 (生产环境推荐)

**使用 Certbot (需要对域名的 DNS 控制权)**

```bash
# 安装 Certbot
sudo apt-get install certbot python3-certbot-nginx

# 获取证书
sudo certbot certonly --standalone \
  -d cyberclaw.example.com \
  -d api.cyberclaw.example.com \
  --email admin@example.com \
  --agree-tos \
  --no-eff-email

# 证书位置
sudo ls -la /etc/letsencrypt/live/cyberclaw.example.com/
```

**配置自动续期**

```bash
# 查看现有续期任务
sudo systemctl list-timers

# 创建续期脚本
cat > /opt/cyberclaw/renew-cert.sh << 'EOF'
#!/bin/bash
certbot renew --quiet
docker-compose -f /opt/cyberclaw/docker-compose.yml restart cyberclaw-server
EOF

chmod +x /opt/cyberclaw/renew-cert.sh

# 添加到 crontab (每月 1 号凌晨 2 点)
echo "0 2 1 * * /opt/cyberclaw/renew-cert.sh" | sudo crontab -
```

### 方式 3: 企业级证书 (需要商业 CA)

```bash
# 生成证书签名请求 (CSR)
openssl req -new -newkey rsa:4096 \
  -keyout /opt/cyberclaw/certs/server.key \
  -out /opt/cyberclaw/certs/server.csr \
  -subj "/CN=cyberclaw.example.com/O=YourOrg/C=US"

# 提交到 CA 获取证书
# CA 会返回 .crt 和可能的 .ca-bundle 文件

# 验证证书链
openssl verify -CAfile /opt/cyberclaw/certs/ca-bundle.crt \
  /opt/cyberclaw/certs/server.crt
```

### 配置服务使用 TLS

在 `.env.production` 中配置：

```bash
# 启用 TLS
USE_TLS=true
TLS_CERT_PATH=/opt/cyberclaw/certs/server.crt
TLS_KEY_PATH=/opt/cyberclaw/certs/server.key

# 监听 HTTPS 端口
CYBERCLAW_ADDR=0.0.0.0:443
```

### 验证 TLS 配置

```bash
# 测试 HTTPS 连接（忽略自签名证书警告）
curl -k https://localhost/health

# 测试证书有效性
openssl s_client -connect localhost:443 -servername localhost

# 查看证书过期日期
openssl x509 -enddate -noout -in /opt/cyberclaw/certs/server.crt
```

---

## 环境变量配置

### 必需变量

| 变量名 | 说明 | 生产环境 | 示例 |
|--------|------|---------|------|
| `LLM_API_KEY` | LLM API 密钥 | 必需 | `sk-...` |
| `JWT_SECRET` | JWT 签名密钥 (≥32 字符) | **强制** | 见下方 |

### 生产环境推荐配置

```bash
# 安全配置
JWT_SECRET="your-very-long-and-random-secret-at-least-32-chars"
USE_TLS=true
TLS_CERT_PATH=/opt/cyberclaw/certs/server.crt
TLS_KEY_PATH=/opt/cyberclaw/certs/server.key
ENVIRONMENT=production

# LLM 配置
LLM_PROVIDER=openai
LLM_API_KEY=sk-prod-your-api-key
LLM_BASE_URL=https://api.openai.com/v1

# 服务器配置
CYBERCLAW_ADDR=0.0.0.0:443

# 日志配置
RUST_LOG=info
```

### 生成安全的 JWT_SECRET

```bash
# 使用 OpenSSL (推荐)
openssl rand -base64 32

# 或使用 Python
python3 -c "import secrets; print(secrets.token_urlsafe(32))"

# 或使用 Ruby
ruby -e "require 'securerandom'; puts SecureRandom.base64(32)"
```

更多详情参考 [docs/ENVIRONMENT_VARIABLES.md](../ENVIRONMENT_VARIABLES.md)。

---

## 健康检查

### 健康检查端点

```bash
# 活跃性探针 (Liveness Probe)
curl http://localhost:3000/health

# 响应 (200 OK)
{"status":"ok"}
```

### 就绪性探针 (Readiness Probe)

```bash
# 检查服务是否已准备好接收请求
curl http://localhost:3000/ready

# 响应 (200 OK)
{"status":"ready"}
```

### Kubernetes 配置示例

```yaml
livenessProbe:
  httpGet:
    path: /health
    port: 3000
  initialDelaySeconds: 10
  periodSeconds: 10

readinessProbe:
  httpGet:
    path: /ready
    port: 3000
  initialDelaySeconds: 5
  periodSeconds: 5
```

---

## 故障排查

详细的故障排查手册请查看 [troubleshooting.md](troubleshooting.md)。

### 常见问题快速检查

**1. 服务无法启动**

```bash
# 检查错误日志
docker-compose logs cyberclaw-server

# 常见原因：
# - JWT_SECRET 长度不足 (< 32 字符)
# - 端口被占用
# - LLM_API_KEY 无效

# 检查端口占用
lsof -i :3000

# 释放端口
kill -9 <PID>
```

**2. LLM 连接失败**

```bash
# 验证 API 密钥和 URL
curl https://api.openai.com/v1/models \
  -H "Authorization: Bearer $LLM_API_KEY"

# 检查网络连接
ping api.openai.com
```

**3. TLS/HTTPS 错误**

```bash
# 验证证书文件存在且可读
ls -l /opt/cyberclaw/certs/

# 验证证书有效性
openssl x509 -in /opt/cyberclaw/certs/server.crt -text -noout

# 检查证书是否过期
openssl x509 -enddate -noout -in /opt/cyberclaw/certs/server.crt
```

---

## 安全检查清单

部署到生产环境前，请完成 [security-checklist.md](security-checklist.md) 中的所有检查项。

### 快速检查

- [ ] **JWT_SECRET**: 长度 ≥ 32 字符
- [ ] **TLS 启用**: USE_TLS=true
- [ ] **证书有效**: 证书未过期
- [ ] **CORS 配置**: 只允许受信任的域
- [ ] **防火墙**: 仅开放必要端口 (443, 80)
- [ ] **日志监控**: 已配置日志收集和监控
- [ ] **备份**: 已备份配置和密钥

更多详情见 [security-checklist.md](security-checklist.md)。

---

## 监控和日志

### 查看日志

```bash
# Docker 容器日志
docker-compose logs -f cyberclaw-server

# 日志级别配置
export RUST_LOG=debug  # 更详细的日志
export RUST_LOG=warn   # 仅警告和错误

# 本地日志文件（如配置）
tail -f /opt/cyberclaw/logs/cyberclaw-server.log
```

### 监控指标

```bash
# 查看容器资源使用
docker stats cyberclaw-server

# 检查服务可用性
watch -n 5 'curl -s http://localhost:3000/health'
```

---

## 性能调优

### 速率限制配置

```bash
# 每秒请求数 (默认: 1)
RATE_LIMIT_PER_SECOND=100

# 允许的突发请求数 (默认: 60)
RATE_LIMIT_BURST_SIZE=200
```

### 请求体大小限制

```bash
# 最大请求体大小 (默认: 10MB)
MAX_REQUEST_BODY_SIZE=52428800  # 50MB
```

### 超时配置

```bash
# 请求超时时间 (秒)
CYBERCLAW_TIMEOUT=30
```

---

## 升级部署

### 安全升级步骤

```bash
# 1. 备份配置和数据
cp -r /opt/cyberclaw /opt/cyberclaw.backup.$(date +%Y%m%d-%H%M%S)

# 2. 获取新版本
git pull origin main
git checkout v2.1.0  # 或最新版本

# 3. 构建新镜像
docker build -t cyberclaw-server:2.1.0 .

# 4. 停止旧服务
docker-compose down

# 5. 更新镜像标签在 docker-compose.yml 中
# 修改: image: cyberclaw-server:2.1.0

# 6. 启动新服务
docker-compose up -d

# 7. 验证新服务
docker-compose ps
curl http://localhost:3000/health

# 8. 监控日志
docker-compose logs -f cyberclaw-server
```

---

## 问题反馈

如遇到部署问题，请提供以下信息：

1. 部署环境 (OS, Docker 版本)
2. 完整的错误日志
3. 环境变量配置 (去掉敏感信息)
4. 使用的部署方式 (Docker/源码/Kubernetes)

---

## 相关文档

- [Docker 部署指南](docker.md)
- [故障排查手册](troubleshooting.md)
- [安全检查清单](security-checklist.md)
- [环境变量配置](../ENVIRONMENT_VARIABLES.md)
- [CyberClaw 服务器 README](../../apps/cyberclaw-server/README.md)
