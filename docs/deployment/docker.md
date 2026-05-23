# Docker 部署详细指南

本文档提供 CyberClaw 服务的完整 Docker 部署说明。

---

## 目录

1. [Dockerfile 说明](#dockerfile-说明)
2. [镜像构建](#镜像构建)
3. [容器运行](#容器运行)
4. [Docker Compose](#docker-compose)
5. [网络配置](#网络配置)
6. [数据卷管理](#数据卷管理)
7. [多容器编排](#多容器编排)
8. [性能优化](#性能优化)

---

## Dockerfile 说明

### 标准 Dockerfile 结构

```dockerfile
# apps/cyberclaw-server/Dockerfile

# 构建阶段
FROM rust:1.70 as builder

WORKDIR /app

# 复制依赖声明
COPY Cargo.* ./
COPY crates ./crates
COPY apps/cyberclaw-server ./apps/cyberclaw-server

# 构建二进制
RUN cargo build --release -p cyberclaw-server

# 运行阶段
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/cyberclaw-server /usr/local/bin/

EXPOSE 3000 443

# 健康检查
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:3000/health || exit 1

ENTRYPOINT ["cyberclaw-server"]
```

### 注意事项

- **多阶段构建**: 最终镜像仅包含运行时依赖，大小更小
- **健康检查**: Docker 容器编排系统使用此信息自动重启失败的容器
- **EXPOSE**: 记录容器暴露的端口 (documentation only)

---

## 镜像构建

### 方式 1: 从源码构建

```bash
# 进入项目根目录
cd /path/to/cyberclaw

# 构建镜像 (tag 为 latest)
docker build -t cyberclaw-server:latest \
  -f apps/cyberclaw-server/Dockerfile .

# 指定版本号
docker build -t cyberclaw-server:2.0.0 \
  -f apps/cyberclaw-server/Dockerfile .

# 查看构建结果
docker images | grep cyberclaw-server
```

### 方式 2: 使用 Docker Compose 自动构建

```bash
# docker-compose.yml 中定义 build
docker-compose build cyberclaw-server

# 强制重新构建 (跳过缓存)
docker-compose build --no-cache cyberclaw-server
```

### 构建优化

**使用 .dockerignore 加速构建**

```bash
# 创建 .dockerignore 文件
cat > .dockerignore << 'EOF'
.git
.github
.omc
target
node_modules
*.md
docs
tmp
.DS_Store
.env
.env.*.local
EOF
```

**启用 Docker BuildKit (更快的并行构建)**

```bash
# 启用 BuildKit
export DOCKER_BUILDKIT=1

# 或在 docker-compose.yml 中配置
version: '3.8'
services:
  cyberclaw-server:
    build:
      context: .
      dockerfile: apps/cyberclaw-server/Dockerfile
```

---

## 容器运行

### 基本运行 (开发环境)

```bash
# 最简单的运行方式
docker run -d \
  --name cyberclaw-server \
  -p 3000:3000 \
  -e LLM_API_KEY="sk-your-api-key" \
  -e JWT_SECRET="dev-secret-at-least-32-characters-long" \
  cyberclaw-server:latest
```

### 完整的运行配置 (生产环境)

```bash
docker run -d \
  --name cyberclaw-server \
  \
  # 网络和端口配置
  -p 443:443 \
  -p 80:80 \
  --hostname cyberclaw.example.com \
  --dns 8.8.8.8 \
  \
  # 资源限制
  --memory 4g \
  --cpus 2 \
  --restart always \
  \
  # 日志配置
  --log-driver json-file \
  --log-opt max-size=10m \
  --log-opt max-file=3 \
  \
  # 环境变量
  -e LLM_API_KEY="sk-prod-key" \
  -e JWT_SECRET="prod-secret-at-least-32-chars" \
  -e USE_TLS=true \
  -e TLS_CERT_PATH=/etc/ssl/certs/server.crt \
  -e TLS_KEY_PATH=/etc/ssl/private/server.key \
  -e RUST_LOG=info \
  \
  # 卷挂载
  -v /etc/ssl/certs:/etc/ssl/certs:ro \
  -v /etc/ssl/private:/etc/ssl/private:ro \
  -v /opt/cyberclaw/logs:/var/log/cyberclaw \
  \
  # 健康检查
  --health-cmd "curl -f http://localhost:3000/health || exit 1" \
  --health-interval 30s \
  --health-timeout 10s \
  --health-retries 3 \
  \
  cyberclaw-server:latest
```

### 运行参数说明

| 参数 | 说明 | 示例 |
|------|------|------|
| `-d` | 后台运行 | - |
| `--name` | 容器名称 | `cyberclaw-server` |
| `-p` | 端口映射 | `-p 3000:3000` |
| `-e` | 环境变量 | `-e LLM_API_KEY=xxx` |
| `-v` | 卷挂载 | `-v /local:/container` |
| `--memory` | 内存限制 | `--memory 4g` |
| `--cpus` | CPU 限制 | `--cpus 2` |
| `--restart` | 重启策略 | `--restart always` |
| `--log-driver` | 日志驱动 | `--log-driver json-file` |

### 常用命令

```bash
# 查看运行的容器
docker ps -a | grep cyberclaw

# 查看容器日志
docker logs cyberclaw-server
docker logs -f cyberclaw-server  # 实时日志
docker logs --tail 50 cyberclaw-server  # 最后 50 行

# 进入容器 shell
docker exec -it cyberclaw-server /bin/bash

# 查看容器资源使用
docker stats cyberclaw-server

# 停止容器
docker stop cyberclaw-server

# 重启容器
docker restart cyberclaw-server

# 删除容器
docker rm cyberclaw-server

# 查看容器详细信息
docker inspect cyberclaw-server
```

---

## Docker Compose

### 基本 docker-compose.yml

```yaml
version: '3.8'

services:
  cyberclaw-server:
    image: cyberclaw-server:latest

    # 自动重启
    restart: always

    # 容器名称
    container_name: cyberclaw-server

    # 端口映射
    ports:
      - "3000:3000"
      - "443:443"

    # 环境变量
    environment:
      LLM_API_KEY: ${LLM_API_KEY}
      LLM_PROVIDER: openai
      JWT_SECRET: ${JWT_SECRET}
      RUST_LOG: info
      USE_TLS: "false"

    # 卷挂载
    volumes:
      - ./logs:/var/log/cyberclaw

    # 健康检查
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:3000/health"]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 10s

    # 日志配置
    logging:
      driver: "json-file"
      options:
        max-size: "10m"
        max-file: "3"

# 网络配置 (可选)
networks:
  default:
    name: cyberclaw-net
    driver: bridge

# 卷管理 (可选)
volumes:
  logs:
    driver: local
```

### 运行 Docker Compose

```bash
# 启动服务
docker-compose up -d

# 查看服务状态
docker-compose ps

# 查看日志
docker-compose logs -f cyberclaw-server

# 停止服务
docker-compose down

# 停止并删除卷
docker-compose down -v

# 查看服务信息
docker-compose describe cyberclaw-server
```

### 使用 .env 文件管理环境变量

```bash
# 创建 .env 文件
cat > .env << 'EOF'
# LLM 配置
LLM_API_KEY=sk-your-api-key
LLM_PROVIDER=openai

# 安全配置
JWT_SECRET=your-secret-at-least-32-characters
USE_TLS=false

# 服务器配置
CYBERCLAW_ADDR=0.0.0.0:3000
RUST_LOG=info
EOF

# docker-compose 自动加载 .env
docker-compose up -d
```

---

## 网络配置

### 使用自定义网络

```yaml
version: '3.8'

services:
  cyberclaw-server:
    image: cyberclaw-server:latest
    networks:
      - cyberclaw-net
    ports:
      - "3000:3000"

  # 如果需要配套服务 (如 Redis, PostgreSQL)
  # redis:
  #   image: redis:7-alpine
  #   networks:
  #     - cyberclaw-net

networks:
  cyberclaw-net:
    driver: bridge
    ipam:
      config:
        - subnet: 172.20.0.0/16
```

### 容器间通信

```bash
# 容器可以通过服务名通信
# 例如，如果有 redis 服务
# 在 cyberclaw-server 中可以访问 http://redis:6379
```

---

## 数据卷管理

### 配置卷 (用于持久化配置)

```yaml
services:
  cyberclaw-server:
    volumes:
      # 配置文件
      - ./config:/etc/cyberclaw:ro

      # TLS 证书
      - ./certs:/etc/ssl/certs:ro

      # 日志输出
      - ./logs:/var/log/cyberclaw

      # 具名卷
      - cyberclaw-data:/opt/cyberclaw/data

volumes:
  cyberclaw-data:
    driver: local
```

### 卷操作

```bash
# 列出所有卷
docker volume ls

# 检查卷内容
docker volume inspect cyberclaw-data

# 清理未使用的卷
docker volume prune

# 备份卷内容
docker run --rm -v cyberclaw-data:/data -v $(pwd):/backup \
  ubuntu tar czf /backup/cyberclaw-data.tar.gz -C /data .

# 恢复卷内容
docker run --rm -v cyberclaw-data:/data -v $(pwd):/backup \
  ubuntu bash -c "cd /data && tar xzf /backup/cyberclaw-data.tar.gz"
```

---

## 多容器编排

### 完整的生产级 docker-compose.yml

```yaml
version: '3.8'

services:
  # 反向代理 (Nginx)
  nginx:
    image: nginx:latest
    restart: always
    container_name: cyberclaw-nginx
    ports:
      - "80:80"
      - "443:443"
    volumes:
      - ./nginx.conf:/etc/nginx/nginx.conf:ro
      - ./certs:/etc/nginx/certs:ro
      - ./logs/nginx:/var/log/nginx
    depends_on:
      cyberclaw-server:
        condition: service_healthy
    networks:
      - cyberclaw-net
    logging:
      driver: "json-file"
      options:
        max-size: "10m"
        max-file: "3"

  # CyberClaw 服务器
  cyberclaw-server:
    build:
      context: .
      dockerfile: apps/cyberclaw-server/Dockerfile
    image: cyberclaw-server:latest
    restart: always
    container_name: cyberclaw-server
    environment:
      LLM_API_KEY: ${LLM_API_KEY}
      JWT_SECRET: ${JWT_SECRET}
      RUST_LOG: info
      USE_TLS: "false"  # Nginx 负责 TLS
      CYBERCLAW_ADDR: 0.0.0.0:3000
    volumes:
      - ./logs/app:/var/log/cyberclaw
    ports:
      - "3000:3000"  # 仅用于内部通信
    networks:
      - cyberclaw-net
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:3000/health"]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 10s
    logging:
      driver: "json-file"
      options:
        max-size: "10m"
        max-file: "3"

  # 日志收集 (可选)
  # filebeat:
  #   image: docker.elastic.co/beats/filebeat:latest
  #   volumes:
  #     - ./logs:/var/log/cyberclaw:ro
  #     - ./filebeat.yml:/usr/share/filebeat/filebeat.yml:ro
  #   networks:
  #     - cyberclaw-net
  #   command: filebeat -e

networks:
  cyberclaw-net:
    driver: bridge

volumes:
  logs:
    driver: local
```

---

## 性能优化

### 资源限制配置

```yaml
services:
  cyberclaw-server:
    deploy:
      resources:
        limits:
          cpus: '2'
          memory: 4G
        reservations:
          cpus: '1'
          memory: 2G
```

### 日志输出优化

```yaml
services:
  cyberclaw-server:
    logging:
      driver: "json-file"
      options:
        max-size: "10m"      # 单个日志文件最大 10MB
        max-file: "3"        # 保留最多 3 个日志文件
        labels: "service=cyberclaw"
```

### 镜像大小优化

```dockerfile
# 使用更小的基础镜像
FROM debian:bookworm-slim

# 或者使用 Alpine
FROM alpine:latest

# 安装必要的依赖
RUN apk add --no-cache \
    ca-certificates \
    curl

# 清理包管理器缓存
RUN rm -rf /var/cache/apk/*
```

---

## 安全最佳实践

### 非 root 用户运行

```dockerfile
# 创建非 root 用户
RUN useradd -m -u 1000 cyberclaw

# 切换用户
USER cyberclaw

ENTRYPOINT ["cyberclaw-server"]
```

### 只读文件系统

```yaml
services:
  cyberclaw-server:
    read_only: true
    tmpfs:
      - /tmp
      - /var/run
```

### 环境变量安全

```bash
# 使用 .env 文件但不要提交到 git
echo ".env.local" >> .gitignore

# 或使用 Docker secrets (Swarm/Kubernetes)
# 不在示例中展示，因为 compose 不原生支持
```

---

## 故障排查

### 容器无法启动

```bash
# 查看详细日志
docker logs cyberclaw-server

# 常见原因：
# 1. 环境变量未设置
# 2. 端口被占用
# 3. 镜像构建失败

# 查看端口占用
docker ps -a | grep 3000
lsof -i :3000
```

### 容器运行缓慢

```bash
# 查看资源使用
docker stats cyberclaw-server

# 检查限制设置
docker inspect cyberclaw-server | grep -A 10 "MemorySwap"

# 增加资源限制
docker update --memory 4g --cpus 2 cyberclaw-server
```

### 网络问题

```bash
# 检查网络连接
docker exec cyberclaw-server ping 8.8.8.8

# 检查 DNS
docker exec cyberclaw-server cat /etc/resolv.conf

# 测试 LLM 连接
docker exec cyberclaw-server curl https://api.openai.com/
```

---

## 相关命令速查表

| 命令 | 说明 |
|------|------|
| `docker build` | 构建镜像 |
| `docker run` | 运行容器 |
| `docker ps` | 列出容器 |
| `docker logs` | 查看日志 |
| `docker exec` | 执行命令 |
| `docker-compose up` | 启动服务 |
| `docker-compose down` | 停止服务 |
| `docker-compose ps` | 查看服务状态 |
| `docker-compose logs` | 查看日志 |

---

## 相关文档

- [部署指南](README.md)
- [故障排查](troubleshooting.md)
- [安全检查清单](security-checklist.md)
