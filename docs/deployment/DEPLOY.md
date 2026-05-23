# CyberClaw 生产部署指南

本文档是面向运维人员的生产部署参考，涵盖快速启动、环境变量、反向代理、备份和升级流程。

- **文档版本**: 2.0 (Sprint 20)
- **更新时间**: 2026-04-24
- **适用版本**: CyberClaw 0.1+

---

## 目录

1. [环境变量必填清单](#环境变量必填清单)
2. [Docker 快速启动](#docker-快速启动)
3. [反向代理 TLS 配置](#反向代理-tls-配置)
4. [Volume 备份策略](#volume-备份策略)
5. [健康检查端点](#健康检查端点)
6. [升级流程](#升级流程)
7. [故障排查](#故障排查)

---

## 环境变量必填清单

以下变量**必须**在启动前通过 `.env` 文件或外部密钥系统注入：

| 变量名 | 必填 | 默认值 | 说明 |
|--------|------|--------|------|
| `JWT_SECRET` | **是** | 无 | JWT 签名密钥，至少 32 字符随机字符串 |
| `LLM_API_KEY` | **是** | 无 | LLM 服务 API Key（OpenAI / 兼容 API） |
| `LLM_BASE_URL` | 否 | `https://api.openai.com/v1` | LLM API 基础 URL |
| `LLM_DEFAULT_MODEL` | 否 | `gpt-4o-mini` | 默认使用的模型名称 |
| `CYBERCLAW_ADDR` | 否 | `0.0.0.0:3000` | 服务监听地址和端口 |
| `ENVIRONMENT` | 否 | `production` | 运行环境标识（影响安全策略） |
| `CYBERCLAW_MEMORY_DB` | 否 | `/var/lib/cyberclaw/memory.db` | SQLite 记忆数据库路径 |
| `CYBERCLAW_MEMORY_RETENTION_DAYS` | 否 | `7` | 记忆保留天数 |
| `CYBERCLAW_AUTO_COMPRESS_THRESHOLD` | 否 | `24000` | 自动压缩触发字符数阈值 |
| `CYBERCLAW_HANDOFF_ENABLED` | 否 | `false` | Sprint 21 多 agent handoff 开关；设为 `true` 或 `1` 启用 `agent.handoff` capability + Govern Handoffs tab |
| `RUST_LOG` | 否 | `info` | 日志级别（info/debug/warn/error） |

### 生成安全密钥

```bash
# 生成 JWT_SECRET（64 字符随机字符串）
openssl rand -hex 32

# 或使用 base64
openssl rand -base64 48
```

### .env 文件示例

```bash
# .env（不要提交到版本控制）
JWT_SECRET=your-64-char-random-secret-here
LLM_API_KEY=sk-your-api-key
LLM_BASE_URL=https://api.openai.com/v1
LLM_DEFAULT_MODEL=gpt-4o-mini
```

---

## Docker 快速启动

### 前置要求

- Docker 20.10+
- Docker Compose 2.0+

### 使用 docker-compose（推荐）

```bash
# 1. 克隆仓库
git clone https://github.com/cyberclawlabs/cyberclaw.git
cd cyberclaw

# 2. 创建 .env 文件
cp .env.example .env
# 编辑 .env，填入必填变量

# 3. 构建并启动
docker compose -f docker-compose.prod.yml up -d

# 4. 检查服务状态
docker compose -f docker-compose.prod.yml ps
docker compose -f docker-compose.prod.yml logs -f
```

### 使用 docker run（单容器）

```bash
docker run -d \
  --name cyberclaw \
  --restart unless-stopped \
  -p 3000:3000 \
  -v cyberclaw-data:/var/lib/cyberclaw \
  -e JWT_SECRET="$(openssl rand -hex 32)" \
  -e LLM_API_KEY="sk-your-key" \
  -e LLM_BASE_URL="https://api.openai.com/v1" \
  -e CYBERCLAW_MEMORY_DB="/var/lib/cyberclaw/memory.db" \
  -e ENVIRONMENT=production \
  cyberclaw-server:latest
```

### 构建镜像

```bash
# 从源码构建（需要 Docker BuildKit）
DOCKER_BUILDKIT=1 docker build -t cyberclaw-server:latest .

# 指定版本标签
docker build -t cyberclaw-server:0.1.0 .
```

---

## 反向代理 TLS 配置

生产环境推荐由反向代理处理 TLS，CyberClaw 内部使用 HTTP。

### Nginx 配置示例

```nginx
# /etc/nginx/sites-available/cyberclaw
server {
    listen 80;
    server_name cyberclaw.example.com;
    return 301 https://$host$request_uri;
}

server {
    listen 443 ssl http2;
    server_name cyberclaw.example.com;

    ssl_certificate     /etc/ssl/certs/cyberclaw.crt;
    ssl_certificate_key /etc/ssl/private/cyberclaw.key;
    ssl_protocols       TLSv1.2 TLSv1.3;
    ssl_ciphers         ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256;

    # 安全头
    add_header Strict-Transport-Security "max-age=31536000; includeSubDomains" always;
    add_header X-Frame-Options DENY always;
    add_header X-Content-Type-Options nosniff always;

    location / {
        proxy_pass         http://localhost:3000;
        proxy_set_header   Host $host;
        proxy_set_header   X-Real-IP $remote_addr;
        proxy_set_header   X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header   X-Forwarded-Proto $scheme;
        proxy_read_timeout 300s;
        proxy_send_timeout 300s;
    }
}
```

```bash
# 验证配置并重载
nginx -t && nginx -s reload
```

### Caddy 配置示例（自动 TLS）

```caddyfile
# /etc/caddy/Caddyfile
cyberclaw.example.com {
    reverse_proxy localhost:3000

    # Caddy 自动申请和续期 Let's Encrypt 证书
    tls {
        protocols tls1.2 tls1.3
    }

    header {
        Strict-Transport-Security "max-age=31536000; includeSubDomains"
        X-Frame-Options DENY
        X-Content-Type-Options nosniff
    }
}
```

```bash
# 重载 Caddy
caddy reload --config /etc/caddy/Caddyfile
```

---

## Volume 备份策略

CyberClaw 的持久化数据存储在 `/var/lib/cyberclaw/memory.db`（SQLite 文件）。

### 手动备份

```bash
# 备份 Docker volume 到本地
docker run --rm \
  -v cyberclaw-data:/data:ro \
  -v $(pwd)/backups:/backup \
  debian:bookworm-slim \
  sh -c "cp /data/memory.db /backup/memory-$(date +%Y%m%d-%H%M%S).db"

# 验证备份文件
ls -lh backups/
sqlite3 backups/memory-*.db ".tables"
```

### 自动定时备份（cron）

```bash
# 编辑 crontab
crontab -e

# 每天凌晨 2 点备份，保留 30 天
0 2 * * * docker run --rm \
  -v cyberclaw-data:/data:ro \
  -v /opt/backups/cyberclaw:/backup \
  debian:bookworm-slim \
  sh -c "cp /data/memory.db /backup/memory-\$(date +\%Y\%m\%d).db && \
         find /backup -name 'memory-*.db' -mtime +30 -delete"
```

### 从备份恢复

```bash
# 停止服务
docker compose -f docker-compose.prod.yml stop

# 恢复数据
docker run --rm \
  -v cyberclaw-data:/data \
  -v $(pwd)/backups:/backup:ro \
  debian:bookworm-slim \
  sh -c "cp /backup/memory-20260424.db /data/memory.db && \
         chown 1000:1000 /data/memory.db"

# 重启服务
docker compose -f docker-compose.prod.yml start
```

---

## 健康检查端点

### GET /health

服务就绪检查端点，无需认证。

```bash
# 检查服务状态
curl -f http://localhost:3000/health

# 预期响应（HTTP 200）
# {"status":"ok"}
```

### 监控集成

```bash
# Prometheus 抓取（若已启用 metrics 端点）
# 目前版本仅提供基础 health 端点，metrics 推 v2

# Docker healthcheck 查看
docker inspect --format='{{json .State.Health}}' cyberclaw | jq
```

---

## 升级流程

### 使用 docker-compose

```bash
# 1. 拉取新镜像
docker compose -f docker-compose.prod.yml pull

# 2. 滚动重启（start-first 策略，零停机）
docker compose -f docker-compose.prod.yml up -d --no-deps cyberclaw-server

# 3. 确认新版本运行
docker compose -f docker-compose.prod.yml ps
curl -f http://localhost:3000/health

# 4. 清理旧镜像
docker image prune -f
```

### 从源码重新构建

```bash
# 1. 拉取最新代码
git pull origin main

# 2. 重新构建镜像
docker build -t cyberclaw-server:latest .

# 3. 重启服务
docker compose -f docker-compose.prod.yml up -d --force-recreate
```

### 回滚

```bash
# 使用指定版本标签
docker compose -f docker-compose.prod.yml \
  --env-file .env \
  -e IMAGE_TAG=0.1.0 \
  up -d --no-deps cyberclaw-server
```

---

## 故障排查

### Bootstrap Token 问题

首次启动时会生成 Bootstrap Admin Token，查看方式：

```bash
# 查看启动日志获取 bootstrap token
docker compose -f docker-compose.prod.yml logs cyberclaw-server | grep -i "bootstrap\|token\|admin"

# 或实时等待
docker compose -f docker-compose.prod.yml logs -f cyberclaw-server | grep -i token
```

### LLM 连接问题

```bash
# 容器内测试 LLM API 连通性
docker exec cyberclaw curl -s \
  -H "Authorization: Bearer $LLM_API_KEY" \
  "${LLM_BASE_URL}/models" | jq '.data[0].id'

# 常见原因：
# - LLM_BASE_URL 末尾不需要斜杠
# - API Key 无效或额度耗尽
# - 网络策略阻止出站请求
```

### 记忆数据库迁移

SQLite 数据库采用自动 schema 迁移，升级时自动执行：

```bash
# 检查数据库文件
docker run --rm \
  -v cyberclaw-data:/data:ro \
  debian:bookworm-slim \
  sqlite3 /data/memory.db ".tables"

# 若数据库损坏，可删除后重建（数据会丢失）
docker run --rm \
  -v cyberclaw-data:/data \
  debian:bookworm-slim \
  rm -f /data/memory.db

docker compose -f docker-compose.prod.yml restart
```

### 端口被占用

```bash
# 检查 3000 端口占用
lsof -i :3000
# 或
ss -tlnp | grep 3000

# 修改端口：编辑 docker-compose.prod.yml
# ports:
#   - "8080:3000"  # 宿主机 8080 -> 容器 3000
```

### 容器内存溢出

```bash
# 查看内存使用
docker stats cyberclaw --no-stream

# 调整内存限制（docker-compose.prod.yml deploy.resources）
# limits:
#   memory: 4G

# 检查 OOM 日志
docker inspect cyberclaw | jq '.[0].State.OOMKilled'
```

### 日志级别调整

```bash
# 临时调整（重启后失效）
docker exec cyberclaw sh -c "kill -USR1 1"  # 若支持信号

# 持久调整：在 .env 中设置
RUST_LOG=debug,cyberclaw=trace
```

---

## 相关文档

- [Docker 详细说明](docker.md)
- [安全检查清单](security-checklist.md)
- [故障排查详情](troubleshooting.md)
- [性能基准数据](BENCHMARKS.md)
- [部署目录 README](README.md)
