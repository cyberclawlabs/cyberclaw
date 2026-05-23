# 故障排查手册

CyberClaw 部署中常见问题的诊断和解决方案。

---

## 目录

1. [启动问题](#启动问题)
2. [网络和连接问题](#网络和连接问题)
3. [性能问题](#性能问题)
4. [日志和监控](#日志和监控)
5. [安全问题](#安全问题)
6. [数据和存储问题](#数据和存储问题)

---

## 启动问题

### 问题: 服务无法启动，出现 "JWT_SECRET must be at least 32 characters"

**症状**:
```
Error: JWT_SECRET must be at least 32 characters long for security
```

**原因**: JWT_SECRET 环境变量未设置或长度不足

**解决方案**:

```bash
# 1. 检查当前设置
echo $JWT_SECRET
echo ${#JWT_SECRET}  # 查看长度

# 2. 生成安全的 JWT_SECRET
SECURE_SECRET=$(openssl rand -base64 32)
echo "新的 JWT_SECRET: $SECURE_SECRET"

# 3. 设置环境变量
export JWT_SECRET="$SECURE_SECRET"

# 或者在 .env 文件中配置
cat > .env << EOF
JWT_SECRET=$SECURE_SECRET
LLM_API_KEY=your-api-key
EOF

# 4. 启动服务
cargo run -p cyberclaw-server
# 或
docker-compose up -d
```

---

### 问题: 端口已被占用

**症状**:
```
error: binding to 0.0.0.0:3000: Address already in use (os error 48)
```

**原因**: 指定的端口已被其他服务占用

**解决方案**:

```bash
# 1. 查看占用端口的进程
lsof -i :3000
# 或
netstat -tulpn | grep :3000

# 2. 查看具体的进程信息
ps aux | grep <PID>

# 3. 方案 A: 停止占用端口的服务
kill -9 <PID>

# 方案 B: 使用不同的端口
export CYBERCLAW_ADDR=127.0.0.1:3001
cargo run -p cyberclaw-server

# 方案 C: 在 docker-compose.yml 中更改端口映射
# ports:
#   - "3001:3000"
```

---

### 问题: LLM_API_KEY 未设置

**症状**:
```
panicked at 'LLM_API_KEY must be set'
```

**原因**: 缺少 LLM API 密钥

**解决方案**:

```bash
# 1. 获取 API 密钥
# OpenAI: https://platform.openai.com/api-keys
# Anthropic: https://console.anthropic.com/
# ARK: https://console.volcengine.com/
# 火山方舟: https://ark.cn-beijing.volces.com/

# 2. 设置环境变量
export LLM_API_KEY="sk-your-actual-api-key"

# 3. 验证设置
echo "API Key set: ${LLM_API_KEY:0:10}..."

# 4. 启动服务
cargo run -p cyberclaw-server

# 5. 如果仍失败，检查 API 密钥有效性
curl https://api.openai.com/v1/models \
  -H "Authorization: Bearer $LLM_API_KEY"
```

---

### 问题: 容器启动后立即退出

**症状**:
```bash
$ docker-compose logs cyberclaw-server
# 日志中显示瞬间启动和退出，没有错误信息
```

**诊断**:

```bash
# 1. 查看容器状态
docker-compose ps

# 2. 查看完整日志
docker-compose logs cyberclaw-server

# 3. 运行时留在前台查看输出
docker-compose up cyberclaw-server  # 不使用 -d

# 4. 进入容器检查环境
docker run -it cyberclaw-server:latest /bin/bash
echo $LLM_API_KEY
echo $JWT_SECRET
```

**常见原因**:
- 环境变量未正确传递
- 配置文件缺失
- 依赖库缺失

**解决方案**:

```bash
# 检查 docker-compose.yml 的 environment 配置
cat docker-compose.yml | grep -A 10 "environment:"

# 确保所有必需的环境变量都已设置
docker-compose config | grep -A 20 "environment"

# 重新构建镜像
docker-compose build --no-cache
docker-compose up -d
```

---

## 网络和连接问题

### 问题: LLM API 连接失败

**症状**:
```
error: Failed to create LLM client
error: Connection refused
error: Network is unreachable
```

**诊断**:

```bash
# 1. 测试网络连接
ping api.openai.com
ping api.anthropic.com

# 2. 测试 DNS 解析
nslookup api.openai.com
dig api.openai.com

# 3. 测试 HTTP 连接
curl -v https://api.openai.com/v1/models

# 4. 测试带认证的连接
curl https://api.openai.com/v1/models \
  -H "Authorization: Bearer $LLM_API_KEY"

# 5. Docker 容器内部测试
docker-compose exec cyberclaw-server curl https://api.openai.com/
```

**常见原因和解决方案**:

| 原因 | 诊断 | 解决 |
|------|------|------|
| 网络断开 | `ping 8.8.8.8` | 检查网络连接 |
| DNS 失败 | `nslookup google.com` | 检查 DNS 配置，改用 8.8.8.8 |
| 防火墙阻止 | `curl -v` 看卡住 | 检查防火墙规则 |
| API URL 错误 | 检查 `LLM_BASE_URL` | 确保 URL 正确 |
| API 密钥无效 | API 返回 401 | 更新或验证 API 密钥 |

**防火墙配置示例**:

```bash
# Linux (使用 UFW)
sudo ufw allow out to any port 443  # HTTPS
sudo ufw allow out to any port 80   # HTTP

# macOS (使用 pfctl)
# 默认允许所有出站连接

# Windows Firewall
# 通过 GUI 或 PowerShell 配置
```

---

### 问题: 健康检查失败

**症状**:
```
Health check failed: No such file or directory
Health check failed: Connection refused
```

**诊断**:

```bash
# 1. 检查服务是否在运行
curl http://localhost:3000/health

# 2. 检查服务日志
docker logs cyberclaw-server

# 3. 进入容器手动执行健康检查
docker exec cyberclaw-server curl http://localhost:3000/health

# 4. 验证服务已启动
docker exec cyberclaw-server ps aux | grep cyberclaw-server
```

**解决方案**:

```bash
# 增加启动延迟
# 在 docker-compose.yml 中配置
healthcheck:
  test: ["CMD", "curl", "-f", "http://localhost:3000/health"]
  interval: 30s
  timeout: 10s
  retries: 3
  start_period: 30s  # 增加此值

# 或使用脚本健康检查
healthcheck:
  test: |
    bash -c 'curl -f http://localhost:3000/health || exit 1'
  interval: 30s
```

---

## 性能问题

### 问题: 服务响应缓慢

**症状**:
- API 请求响应时间超过 30 秒
- CPU/内存使用率高

**诊断**:

```bash
# 1. 检查资源使用
docker stats cyberclaw-server

# 2. 检查 CPU 使用
top -p $(docker inspect -f '{{.State.Pid}}' cyberclaw-server)

# 3. 检查内存使用
ps aux | grep cyberclaw-server | awk '{print $6}'

# 4. 测试响应时间
time curl http://localhost:3000/health

# 5. 检查日志中是否有错误
docker logs cyberclaw-server | grep -i error

# 6. 监控日志
docker logs -f cyberclaw-server
```

**常见原因和解决方案**:

| 原因 | 症状 | 解决 |
|------|------|------|
| 内存不足 | OOM Kill, 进程重启 | 增加内存限制或优化代码 |
| CPU 限制 | CPU 使用率 100% | 增加 CPU 限制，检查并发请求 |
| 磁盘 I/O 慢 | 日志输出卡顿 | 检查磁盘状态 |
| LLM API 慢 | 第一个 token 延迟高 | 检查 LLM 服务状态 |

**资源调优**:

```yaml
# docker-compose.yml
services:
  cyberclaw-server:
    deploy:
      resources:
        limits:
          cpus: '4'          # 最多使用 4 CPU
          memory: 8G         # 最多使用 8GB 内存
        reservations:
          cpus: '2'          # 预留 2 CPU
          memory: 4G         # 预留 4GB 内存
```

---

### 问题: 内存泄漏

**症状**:
- 内存使用持续上升
- 最终被 OOM Kill

**诊断**:

```bash
# 1. 监控内存增长趋势
watch -n 5 'docker stats cyberclaw-server'

# 2. 收集内存快照（需要支持）
docker exec cyberclaw-server /memory-profiler

# 3. 分析容器内存使用
docker inspect cyberclaw-server | grep -A 5 Memory
```

**短期解决方案**:

```bash
# 定期重启服务
# 在 crontab 中添加
0 3 * * * docker-compose -f /opt/cyberclaw/docker-compose.yml restart cyberclaw-server
```

**长期解决方案**:

- 报告到开发团队
- 检查是否有已知的内存泄漏问题
- 考虑使用内存分析工具 (valgrind, heaptrack)

---

## 日志和监控

### 问题: 无法查看日志

**症状**:
```
No such file or directory
Permission denied
```

**解决方案**:

```bash
# 1. 查看容器日志
docker-compose logs cyberclaw-server

# 2. 查看日志文件位置
docker inspect cyberclaw-server | grep LogPath

# 3. 直接查看日志文件
tail -f /var/lib/docker/containers/<container-id>/<container-id>-json.log

# 4. 查看主机上的日志
tail -f ./logs/app/cyberclaw-server.log

# 5. 设置日志级别查看更详细的日志
export RUST_LOG=debug
docker-compose up -d
```

---

### 问题: 日志文件过大

**症状**:
- 磁盘空间耗尽
- 日志查询变慢

**解决方案**:

```bash
# 配置日志轮转
# docker-compose.yml
logging:
  driver: "json-file"
  options:
    max-size: "10m"      # 单个文件最大 10MB
    max-file: "3"        # 最多保留 3 个文件

# 手动清理旧日志
sudo du -sh /var/lib/docker/containers/*/

# 删除过期日志
find ./logs -name "*.log" -mtime +7 -delete  # 删除 7 天前的日志

# 配置系统日志轮转 (logrotate)
sudo cat > /etc/logrotate.d/cyberclaw << 'EOF'
/opt/cyberclaw/logs/*.log {
    daily
    rotate 7
    compress
    delaycompress
    missingok
    notifempty
}
EOF
```

---

## 安全问题

### 问题: TLS 证书错误

**症状**:
```
error: Unable to get local issuer certificate
error: certificate verify failed
```

**诊断**:

```bash
# 1. 检查证书文件存在
ls -la /opt/cyberclaw/certs/

# 2. 验证证书格式
openssl x509 -in server.crt -text -noout

# 3. 检查证书有效期
openssl x509 -enddate -noout -in server.crt

# 4. 验证证书链
openssl verify server.crt

# 5. 检查私钥匹配
openssl x509 -noout -modulus -in server.crt | openssl md5
openssl rsa -noout -modulus -in server.key | openssl md5
```

**解决方案**:

```bash
# 更新过期的证书
# 使用 Let's Encrypt
sudo certbot renew --force-renewal

# 或生成新的自签名证书
openssl req -x509 -newkey rsa:4096 \
  -keyout server.key \
  -out server.crt \
  -days 365 -nodes

# 更新 docker-compose.yml 中的路径
# 重启服务
docker-compose restart cyberclaw-server
```

---

### 问题: 认证失败

**症状**:
```
401 Unauthorized
Invalid token
```

**诊断**:

```bash
# 1. 检查 JWT_SECRET 是否相同
echo $JWT_SECRET

# 2. 测试 API 调用
curl -H "Authorization: Bearer token" http://localhost:3000/v1/chat/completions

# 3. 检查令牌有效性
# （需要解析 JWT）
```

**解决方案**:

```bash
# 确保所有实例使用相同的 JWT_SECRET
# 在所有节点上设置相同的值
export JWT_SECRET="same-secret-everywhere"

# 重启所有实例
docker-compose restart
```

---

## 数据和存储问题

### 问题: 磁盘空间不足

**症状**:
```
No space left on device
Disk quota exceeded
```

**诊断**:

```bash
# 1. 检查磁盘使用
df -h /

# 2. 查找大文件
du -sh /opt/cyberclaw/*
du -sh /var/lib/docker/*

# 3. 查看日志大小
du -sh ./logs/*

# 4. Docker 镜像和容器大小
docker images
docker ps -a --size
```

**解决方案**:

```bash
# 1. 清理日志
rm -rf ./logs/*
# 或配置日志轮转

# 2. 清理 Docker 镜像
docker image prune -a

# 3. 清理 Docker 容器
docker container prune

# 4. 清理未使用的卷
docker volume prune

# 5. 增加磁盘空间 (如果是虚拟机)
# 扩展分区或挂载新的卷
```

---

## 获取帮助

### 提交问题时需要提供的信息

1. **环境信息**
```bash
# 操作系统
uname -a

# Docker 版本
docker --version
docker-compose --version

# 磁盘和内存
df -h
free -h
```

2. **完整错误日志**
```bash
docker-compose logs cyberclaw-server > logs.txt
# 提交 logs.txt (去掉敏感信息)
```

3. **配置信息 (去掉密钥)**
```bash
cat docker-compose.yml
cat .env | grep -v "KEY\|SECRET\|TOKEN"
```

4. **问题重现步骤**
- 描述执行的操作
- 预期结果
- 实际结果

---

## 速查表

| 问题 | 快速诊断 | 快速修复 |
|------|---------|---------|
| 无法启动 | `docker logs` | 检查环境变量 |
| 网络错误 | `curl https://api.openai.com/` | 检查防火墙和 DNS |
| 性能差 | `docker stats` | 增加资源限制 |
| 磁盘满 | `df -h` | 清理日志和 Docker 缓存 |
| 证书过期 | `openssl x509 -enddate` | 更新证书 |
| 内存泄漏 | 监控内存趋势 | 定期重启或报告 |

---

## 相关文档

- [部署指南](README.md)
- [Docker 部署](docker.md)
- [安全检查清单](security-checklist.md)
- [环境变量配置](../ENVIRONMENT_VARIABLES.md)
