# 生产安全检查清单

部署 CyberClaw 到生产环境前必须完成的安全检查项。

- **清单版本**: 1.0
- **适用范围**: 生产环境部署
- **完成时间**: 部署前至少 24 小时

---

## 部署前检查 (Pre-Deployment)

### 1. 认证和密钥管理

- [ ] **JWT_SECRET 已设置且强度足够**
  - [ ] 长度 ≥ 32 字符
  - [ ] 使用强随机生成器生成
  - [ ] 未在代码或版本控制中存储
  - [ ] 验证方式:
    ```bash
    echo ${#JWT_SECRET}  # 应输出 ≥ 32
    grep -r "JWT_SECRET" . --exclude-dir=.git  # 不应有硬编码值
    ```

- [ ] **LLM API 密钥已安全存储**
  - [ ] 使用密钥管理服务 (AWS Secrets Manager, HashiCorp Vault 等)
  - [ ] 不在 `.env` 文件或代码中存储
  - [ ] 定期轮换密钥
  - [ ] 只授予必要的权限

- [ ] **凭证访问控制**
  - [ ] `.env` 文件权限设置为 600 (仅所有者可读)
  - [ ] 证书文件权限设置为 600
  - [ ] 私钥文件权限设置为 400
  - [ ] 验证方式:
    ```bash
    ls -la .env | grep "rw-------"
    ls -la /opt/cyberclaw/certs/ | grep "^-.*400"
    ```

### 2. TLS/HTTPS 配置

- [ ] **TLS 已启用**
  - [ ] `USE_TLS=true` 在生产环境配置中
  - [ ] `CYBERCLAW_ADDR` 设置为 `0.0.0.0:443`
  - [ ] 验证方式:
    ```bash
    curl -k https://localhost/health
    ```

- [ ] **证书有效且未过期**
  - [ ] 从信任的 CA 获取 (不使用自签名)
  - [ ] 已验证证书链完整性
  - [ ] 检查过期日期在 90 天以上
  - [ ] 验证方式:
    ```bash
    openssl x509 -enddate -noout -in /opt/cyberclaw/certs/server.crt
    # 应显示 90 天后的日期
    ```

- [ ] **证书和私钥匹配**
  - [ ] 验证方式:
    ```bash
    diff <(openssl x509 -noout -modulus -in server.crt | openssl md5) \
         <(openssl rsa -noout -modulus -in server.key | openssl md5)
    # 两个 MD5 应相同
    ```

- [ ] **HSTS (HTTP Strict Transport Security) 已配置**
  - [ ] 在响应头中设置 `Strict-Transport-Security: max-age=31536000`
  - [ ] 初始值可用较短的 `max-age` (如 300)，经过测试后增加

- [ ] **支持现代 TLS 版本**
  - [ ] TLS 1.2+ 已启用
  - [ ] TLS 1.0/1.1 已禁用
  - [ ] 强密码套件已配置
  - [ ] 验证方式:
    ```bash
    nmap --script ssl-enum-ciphers -p 443 localhost
    ```

### 3. CORS 和跨域配置

- [ ] **CORS 已显式配置**
  - [ ] 不使用 `*` (通配符)
  - [ ] 只允许信任的域名
  - [ ] 配置中列出所有允许的来源
  - [ ] 验证方式:
    ```bash
    grep -E "CORS_ORIGINS|AllowedOrigins" docker-compose.yml
    # 应显示具体域名，不是 *
    ```

- [ ] **CORS 预检请求已配置**
  - [ ] OPTIONS 请求返回正确的 CORS 头
  - [ ] `Access-Control-Allow-Methods` 已设置
  - [ ] `Access-Control-Allow-Headers` 已设置
  - [ ] `Access-Control-Max-Age` 已设置

### 4. 网络和防火墙

- [ ] **防火墙规则已配置**
  - [ ] 仅允许必要的入站端口
    - [ ] 443 (HTTPS) - 允许所有
    - [ ] 80 (HTTP) - 可选，仅用于重定向
  - [ ] SSH (22) - 仅允许特定 IP
  - [ ] 其他端口 - 全部关闭
  - [ ] 验证方式:
    ```bash
    sudo ufw status numbered
    sudo iptables -L -n | grep ACCEPT
    ```

- [ ] **出站连接受限**
  - [ ] 仅允许到 LLM API 的出站连接
  - [ ] 禁止其他不必要的出站连接
  - [ ] 特别是禁止向公网发送敏感数据

- [ ] **内部网络隔离**
  - [ ] Docker 容器运行在隔离网络中
  - [ ] 仅暴露必要的端口
  - [ ] 容器间通信使用内部网络

### 5. 访问控制 (AAA - Authentication/Authorization/Audit)

- [ ] **API 认证已启用**
  - [ ] 所有 API 端点都需要认证
  - [ ] JWT 令牌验证已实现
  - [ ] 令牌过期时间已设置 (建议 1 小时)

- [ ] **授权检查已实现**
  - [ ] 用户只能访问自己的资源
  - [ ] 管理操作受限于管理员
  - [ ] 速率限制已配置以防止滥用

- [ ] **审计日志已启用**
  - [ ] 所有认证尝试都被记录
  - [ ] 所有授权决策都被记录
  - [ ] 日志包含时间戳和用户标识
  - [ ] 验证方式:
    ```bash
    docker logs cyberclaw-server | grep -i "auth\|access"
    ```

### 6. 输入验证和注入防护

- [ ] **请求体验证**
  - [ ] 最大请求体大小已限制 (默认 10MB)
  - [ ] JSON 验证已实现
  - [ ] 参数类型检查已实现

- [ ] **SQL 注入防护** (如果适用)
  - [ ] 使用参数化查询
  - [ ] 不构造动态 SQL 字符串

- [ ] **命令注入防护** (如果适用)
  - [ ] 避免使用 `shell=True`
  - [ ] 验证所有用户输入
  - [ ] 不将用户输入直接传递给系统命令

### 7. 日志和监控

- [ ] **日志收集已配置**
  - [ ] 所有日志都被转发到中央系统
  - [ ] 日志包含足够的上下文信息
  - [ ] 敏感信息 (API 密钥, 密码) 已掩蔽
  - [ ] 验证方式:
    ```bash
    docker logs cyberclaw-server | head -20 | grep -v "KEY\|SECRET"
    ```

- [ ] **监控告警已配置**
  - [ ] 高错误率告警
  - [ ] 长响应时间告警
  - [ ] 内存/CPU 使用告警
  - [ ] 认证失败告警
  - [ ] TLS 证书过期告警 (提前 30 天)

- [ ] **日志保留策略**
  - [ ] 日志保留至少 30 天
  - [ ] 日志轮转已配置
  - [ ] 存档和备份已配置

### 8. 依赖和补丁

- [ ] **依赖版本已检查**
  - [ ] 运行 `cargo audit` 检查已知漏洞
  - [ ] 使用最新的稳定版本
  - [ ] 记录所有依赖版本
  - [ ] 验证方式:
    ```bash
    cargo audit
    cargo tree --depth 2
    ```

- [ ] **操作系统补丁已更新**
  - [ ] 所有安全补丁已应用
  - [ ] 内核已更新
  - [ ] 软件包已更新

- [ ] **Docker 镜像安全**
  - [ ] 基础镜像来自官方仓库
  - [ ] 使用最新的稳定标签 (不使用 `latest`)
  - [ ] 已扫描镜像的已知漏洞
  - [ ] 验证方式:
    ```bash
    docker scout cves cyberclaw-server:2.0.0
    ```

---

## 部署时检查 (Deployment-Time)

### 9. 部署流程

- [ ] **部署者身份已验证**
  - [ ] 部署由授权人员执行
  - [ ] 部署请求已获批准
  - [ ] 审计日志已记录部署者

- [ ] **环境隔离已确认**
  - [ ] 不会部署到错误的环境
  - [ ] 生产环境配置与开发/测试不同
  - [ ] 验证方式:
    ```bash
    grep ENVIRONMENT docker-compose.yml
    # 应显示 ENVIRONMENT=production
    ```

- [ ] **备份已创建**
  - [ ] 完整的配置备份
  - [ ] 证书和密钥备份
  - [ ] 数据库备份 (如适用)
  - [ ] 验证方式:
    ```bash
    ls -la /opt/cyberclaw.backup.*/
    ```

- [ ] **回滚计划已准备**
  - [ ] 上一版本的镜像已保留
  - [ ] 回滚步骤已文档化
  - [ ] 回滚已在测试环境验证

- [ ] **部署验证清单**
  - [ ] 服务成功启动
  - [ ] 健康检查通过
  - [ ] 日志中无错误
  - [ ] 所有 API 端点可访问
  - [ ] 验证方式:
    ```bash
    curl https://your-domain/health
    curl https://your-domain/ready
    ```

---

## 部署后检查 (Post-Deployment)

### 10. 功能验证

- [ ] **API 功能测试**
  - [ ] Chat Completions 端点正常工作
  - [ ] 健康检查端点返回 200 OK
  - [ ] 错误处理正常 (返回正确的 HTTP 状态码)

- [ ] **安全功能验证**
  - [ ] 未认证请求被拒绝
  - [ ] 超过速率限制的请求被拒绝
  - [ ] CORS 预检请求返回正确的头

- [ ] **性能验证**
  - [ ] 响应时间在可接受范围内 (< 30s)
  - [ ] 并发请求处理正常
  - [ ] CPU/内存使用合理

### 11. 安全验证

- [ ] **HTTPS 强制**
  - [ ] HTTP 请求被重定向到 HTTPS
  - [ ] 验证方式:
    ```bash
    curl -v http://your-domain:80/health
    # 应返回 301/302 重定向
    ```

- [ ] **安全头已设置**
  - [ ] `Strict-Transport-Security` 已设置
  - [ ] `X-Content-Type-Options: nosniff` 已设置
  - [ ] `X-Frame-Options: DENY` 已设置
  - [ ] `Content-Security-Policy` 已设置 (如适用)
  - [ ] 验证方式:
    ```bash
    curl -I https://your-domain/health
    ```

- [ ] **证书链验证**
  - [ ] 浏览器信任此证书
  - [ ] 证书信息显示正确的域名
  - [ ] 无警告或错误

- [ ] **SSL/TLS 配置验证**
  - [ ] 使用 SSL Labs 或 nmap 扫描
  - [ ] 等级为 A 或更高
  - [ ] 所有已知漏洞已修补
  - [ ] 验证方式:
    ```bash
    # 使用 testssl.sh
    ./testssl.sh https://your-domain

    # 或在线检查
    # https://www.ssllabs.com/ssltest/
    ```

### 12. 监控验证

- [ ] **日志收集验证**
  - [ ] 日志出现在中央日志系统中
  - [ ] 日志格式正确
  - [ ] 敏感信息已掩蔽

- [ ] **监控告警验证**
  - [ ] 告警规则已激活
  - [ ] 测试告警通知是否工作
  - [ ] 联系人信息已更新

- [ ] **度量收集验证**
  - [ ] Prometheus/Grafana (如适用) 接收度量
  - [ ] 仪表板显示数据
  - [ ] 告警规则已配置

### 13. 运维交接

- [ ] **文档已更新**
  - [ ] 运维手册已更新
  - [ ] 告警规则已文档化
  - [ ] 恢复流程已文档化
  - [ ] 联系人列表已更新

- [ ] **培训已完成**
  - [ ] 运维团队已接收培训
  - [ ] 所有队员理解部署架构
  - [ ] 故障排查流程已讲解

- [ ] **访问权限已配置**
  - [ ] 所有运维人员有适当的访问权限
  - [ ] SSH 密钥已配置
  - [ ] 日志系统访问已配置
  - [ ] 监控系统访问已配置

---

## 月度/季度检查 (Periodic)

### 14. 定期安全审计

- [ ] **每月检查**
  - [ ] 审查访问日志 (是否有异常访问)
  - [ ] 检查证书过期日期
  - [ ] 运行 `cargo audit` 检查依赖
  - [ ] 审查告警日志 (是否有异常)
  - [ ] 验证备份完整性

- [ ] **每季度检查**
  - [ ] 渗透测试
  - [ ] 依赖更新和升级
  - [ ] 密钥轮换
  - [ ] 访问权限审计
  - [ ] 灾难恢复测试

- [ ] **每年检查**
  - [ ] 完整的安全审计
  - [ ] 合规性验证
  - [ ] 架构评估

---

## 清单完成记录

请在文件开头记录完成时间和负责人:

```
部署日期: [DATE]
部署环境: [PROD/STAGING]
负责人: [NAME]
审批人: [NAME]
完成日期: [DATE]
备注: [NOTES]
```

---

## 相关文档

- [部署指南](README.md)
- [Docker 部署](docker.md)
- [故障排查](troubleshooting.md)
- [环境变量配置](../ENVIRONMENT_VARIABLES.md)
- [安全架构](../../docs/architecture/SECURITY_ARCHITECTURE.md)
