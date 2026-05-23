# Deployment

把 CyberClaw 从开发机搬到生产。覆盖：环境变量契约、单机 systemd、Podman/Docker 单容器、K8s 清单、监控、备份恢复。

## 0. 部署形态选择

| 形态 | 何时用 | 入口文档 |
|---|---|---|
| 单机 systemd | 内部工具 / 单租户 PoC | 本文 §2 |
| Podman / Docker 单容器 | 一台机器，需要进程隔离 | [`STAGING_PODMAN.md`](../implementation/deploy/STAGING_PODMAN.md) |
| K8s 多副本 | 生产，多 agent 并发 | 本文 §4 + `deploy/k8s/` |
| Raft 集群 | 高可用 / 跨可用区 | `docs/architecture/runtime/RAFT_*.md` |

## 1. 环境变量契约

### 必需（缺则启动失败）

| Var | 示例 | 来源 |
|---|---|---|
| `JWT_SECRET` | `openssl rand -hex 32` | 32+ 字符。不能放代码 / git。 |

### 路径配置（建议显式设）

| Var | 默认 | 说明 |
|---|---|---|
| `CYBERCLAW_WORKSPACE` | `~/.cyberclaw/workspace` | agent 工作目录（每 agent 子目录） |
| `CYBERCLAW_ECOSYSTEM_DIR` | `./ecosystem` | skills / connectors / platform-plugins 加载位置 |
| `CYBERCLAW_USERS_FILE` | `~/.cyberclaw/users.toml` | admin 用户 + role |
| `CYBERCLAW_AUDIT_DB` | `~/.cyberclaw/audit.db` | append-only audit + hash chain |
| `CYBERCLAW_AUDIT_ARCHIVE_DIR` | `<audit-db>/archive` | VACUUM INTO 快照存档 |

### 治理 / 安全

| Var | 默认 | 说明 |
|---|---|---|
| `CYBERCLAW_POLICY_REVIEW_THRESHOLD` | `Medium` | 高于此阈值的 capability 进入 review 队列 |
| `CYBERCLAW_AUDIT_ARCHIVE_GPG_KEY` | (off) | audit 归档 GPG 签名 keyid |
| `CYBERCLAW_WEBHOOK_SECRET_<PLATFORM>` | (off) | 各平台 webhook HMAC-SHA256 密钥 |
| `CYBERCLAW_HSTS` | `production` | `production` 启用 HSTS，`disabled` 关闭 |

### LLM / 工具后端

| Var | 默认 | 说明 |
|---|---|---|
| `OPENAI_API_KEY` | (off) | OpenAI / 兼容端点（含 MiniMax / vLLM） |
| `OPENAI_BASE_URL` | OpenAI 官方 | 自部署模型时改这里 |
| `WEB_SEARCH_PROVIDER` | `duckduckgo` | `exa` (BT-06 推荐) / `tavily` / `brave` |
| `EXA_API_KEY` | (off) | 当 provider=exa 时 |
| `WEB_SEARCH_API_KEY` | (off) | 当 provider=tavily/brave 时 |

### 集群（多节点才需要）

| Var | 默认 | 说明 |
|---|---|---|
| `CYBERCLAW_CLUSTER_MODE` | `single` | `raft` 启用集群 |
| `CYBERCLAW_NODE_ID` | `local-server-node` | 必须在集群内唯一 |
| `CYBERCLAW_RAFT_PEERS` | `""` | 逗号分隔的 `<node_id>=<host>:<port>` |
| `CYBERCLAW_RAFT_BIND_ADDR` | `127.0.0.1:7700` | 集群内部 RPC 端口 |
| `CYBERCLAW_CLUSTER_SHARED_TOKEN` | (off) | 集群内部 RPC 鉴权 |

## 2. 单机 systemd（最简）

```bash
# 1. 把发布产物 + ecosystem 拷到目标机
scp target/release/cyberclaw-server prod-host:/usr/local/bin/
rsync -a ecosystem/ prod-host:/opt/cyberclaw/ecosystem/

# 2. 在目标机生成密钥 + 用户
ssh prod-host
sudo useradd --system --home /var/lib/cyberclaw cyberclaw
sudo -u cyberclaw mkdir -p /var/lib/cyberclaw/workspace
sudo -u cyberclaw bash -c 'cat > /var/lib/cyberclaw/users.toml <<EOF
[[users]]
user_id = "admin"
password_hash = "<bcrypt-hash>"
role = "admin"
EOF'

# 3. 写 systemd unit
sudo tee /etc/systemd/system/cyberclaw.service <<'EOF'
[Unit]
Description=CyberClaw Server
After=network.target

[Service]
User=cyberclaw
Group=cyberclaw
EnvironmentFile=/etc/cyberclaw/cyberclaw.env
ExecStart=/usr/local/bin/cyberclaw-server
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

# 4. 写 env 文件（受保护）
sudo install -m 0640 -o root -g cyberclaw /dev/null /etc/cyberclaw/cyberclaw.env
sudo tee /etc/cyberclaw/cyberclaw.env <<EOF
JWT_SECRET=$(openssl rand -hex 32)
CYBERCLAW_WORKSPACE=/var/lib/cyberclaw/workspace
CYBERCLAW_ECOSYSTEM_DIR=/opt/cyberclaw/ecosystem
CYBERCLAW_USERS_FILE=/var/lib/cyberclaw/users.toml
CYBERCLAW_POLICY_REVIEW_THRESHOLD=High
EOF

# 5. 启动
sudo systemctl enable --now cyberclaw
sudo systemctl status cyberclaw
journalctl -u cyberclaw -f
```

## 3. Podman / Docker 单容器

完整指南见 [`docs/implementation/deploy/STAGING_PODMAN.md`](../implementation/deploy/STAGING_PODMAN.md)。最小命令：

```bash
podman run -d --name cyberclaw \
  -p 38090:38090 \
  -e JWT_SECRET=$(openssl rand -hex 32) \
  -e CYBERCLAW_POLICY_REVIEW_THRESHOLD=High \
  -v $HOME/.cyberclaw:/var/lib/cyberclaw \
  -v $(pwd)/ecosystem:/opt/cyberclaw/ecosystem:ro \
  ghcr.io/cyberclaw/cyberclaw:latest
```

## 4. Kubernetes

清单见 [`deploy/k8s/`](../../deploy/k8s/) 目录。核心 manifest：

| 文件 | 作用 |
|---|---|
| `base/configmap.yaml` | 非密钥环境变量（review threshold、HSTS、cluster mode 等） |
| `base/secret.template.yaml` | JWT_SECRET / API key 模板（生产从 Vault 注入） |
| `base/deployment.yaml` | server Deployment + readiness/liveness 探针 |
| `base/service.yaml` | ClusterIP + Ingress |
| `base/cronjob-audit-archive.yaml` | 每小时 VACUUM INTO + GPG 签名快照 |

最小部署：

```bash
# 1. 先注入密钥
kubectl -n cyberclaw create secret generic cyberclaw-secrets \
  --from-literal=jwt-secret=$(openssl rand -hex 32) \
  --from-literal=openai-api-key="$OPENAI_API_KEY" \
  --from-literal=exa-api-key="$EXA_API_KEY"

# 2. 应用 manifest
kubectl apply -f deploy/k8s/base/

# 3. 等就绪 + 看日志
kubectl -n cyberclaw rollout status deploy/cyberclaw-server
kubectl -n cyberclaw logs -l app=cyberclaw-server --tail=200 -f
```

## 5. Web UI 部署

Server 默认会从 `web/dist/` 目录加载 SPA 资源。容器化部署的两种方式：

**方式 A — 镜像内 bake**：在 Dockerfile 加 `RUN npm install && npm run build:web`，把 `web/dist/` 复制到 `/app/web/dist/`。

**方式 B — 镜像外挂**：
```bash
podman run ... -v ./web/dist:/app/web/dist:ro ...
```

如果 `web/dist/` 不存在，admin SPA 路由返回 404，但 `/api/v1/*` API 正常工作。运营人员可以纯 CLI 操作。

## 6. 监控 + 告警

模板：[`deploy/monitoring/`](../../deploy/monitoring/)。

- `/metrics` 暴露 Prometheus 格式（绕过 rate limiting + JWT）
- `/health` 暴露 K8s readinessProbe（同上）
- Grafana 面板 JSON 在 `deploy/monitoring/grafana/`

关键指标：

| Metric | 阈值 | 告警 |
|---|---|---|
| `cyberclaw_audit_chain_corrupted_total` | > 0 | P0 立即停服 + 走 RB-11 |
| `cyberclaw_review_queue_pending` | > 100 持续 30min | P1 审批积压 |
| `cyberclaw_request_duration_p99` | > 5s | P2 性能退化 |

## 7. 备份 / 灾难恢复

完整 runbook：[`RB-11 Audit / Memory DB 备份 + 灾难恢复`](../implementation/deploy/RUNBOOKS.md#rb-11-audit-memory-db-备份-灾难恢复)。

- audit.db: hash chain，每小时归档 + GPG 签名，丢失数据上限 = 备份周期
- memory.db: 每日备份即可，丢失允许 24 小时（agent 重新学习）

## 8. CHAOS 演习（发布前必跑）

每次推 production 前跑一次 [RB-12 CHAOS](../implementation/deploy/RUNBOOKS.md#rb-12-chaos-故障注入--发布前演习)：

| 场景 | 验证 |
|---|---|
| Server SIGKILL | 无僵尸任务 + audit chain 完整 |
| SQLite 写锁模拟 | 503 降级而非 panic |
| PolicyEngine 强制拦截 | cmd.exec 不被执行 |
| 节点隔离（多节点） | 任务重新分配 |
| OOM Kill | 60s 内恢复 |

## 9. 零停机升级

```bash
# K8s rolling update（推荐）
kubectl -n cyberclaw set image deploy/cyberclaw-server \
  cyberclaw-server=ghcr.io/cyberclaw/cyberclaw:vX.Y.Z

# 单机蓝绿（systemd）
sudo cp target/release/cyberclaw-server /usr/local/bin/cyberclaw-server.new
sudo mv /usr/local/bin/cyberclaw-server.new /usr/local/bin/cyberclaw-server
sudo systemctl restart cyberclaw       # ≤2s 中断
```

audit chain 跨重启保持完整 — restart 不会丢任何 audit row。

## 下一步

- [RB-01 ~ RB-12 Runbooks](../implementation/deploy/RUNBOOKS.md) — on-call 第一响应手册
- [Quickstart](quickstart.md) — 本机调试场景
- [Reference](../reference/README.md) — 全部 HTTP API + CLI 命令
