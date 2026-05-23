# CyberClaw API · 用户手册

> 给 API 消费者（CLI、第三方集成、自建 Web UI）。运维/部署细节见 `docs/implementation/deploy/`。

## 0. 30 秒快速开始

```bash
# 0. staging 跑起来
./scripts/deploy/staging-podman.sh build
./scripts/deploy/staging-podman.sh up
# → http://127.0.0.1:38090

# 1. 健康
curl -sf http://127.0.0.1:38090/health
# → OK

# 2. 拿 token
JWT=$(curl -s -X POST http://127.0.0.1:38090/admin/login \
  -H 'Content-Type: application/json' \
  -d '{"user_id":"op_ada","password":"any"}' | jq -r .jwt)

# 3. 验证身份
curl -s -H "Authorization: Bearer $JWT" \
  http://127.0.0.1:38090/admin/me

# 4. 调一个 API
curl -s -H "Authorization: Bearer $JWT" \
  http://127.0.0.1:38090/api/v1/memory
```

## 1. 鉴权模型

- **算法**：HS256
- **Secret**：`JWT_SECRET` 环境变量（≥ 32 字节）；server 端必须与 client 用同一 secret
- **过期**：默认 1 小时；过期后调 `/admin/login` 重发
- **载荷**：`{ "sub": "<user_id>", "iat": <unix>, "exp": <unix> }`

**生产环境**：登录走 `/admin/login` + 操作员密码。MVP 阶段任意 password 通过（仅 staging）。

## 2. 核心工作流

### 2.1 跑一次 Agent execution

```bash
# 列出 agents
curl -s -H "Authorization: Bearer $JWT" \
  http://127.0.0.1:38090/api/v1/agents

# invoke
EXEC_ID=$(curl -s -X POST \
  -H "Authorization: Bearer $JWT" \
  -H 'Content-Type: application/json' \
  -d '{"input":{"prompt":"hello"},"execution_mode":"Normal"}' \
  http://127.0.0.1:38090/api/v1/agents/example-agent/invoke | jq -r .execution_id)

# 看 trace
curl -s -H "Authorization: Bearer $JWT" \
  http://127.0.0.1:38090/api/v1/executions/$EXEC_ID/trace
```

**execution_mode**：
| 值 | 行为 |
|---|---|
| `Normal` | 默认，每个 capability 独立审批 |
| `Autopilot` | 危险 capability 被剥离 + circuit breaker（3 次连续失败强制退出） |
| `Persistent` | Ralph-style 持久 loop，story-driven 执行 |

### 2.2 长期记忆 CRUD

```bash
# 写
curl -s -X POST -H "Authorization: Bearer $JWT" \
  -H 'Content-Type: application/json' \
  -d '{"agent_id":"my-agent","level":"L1","content":"meeting notes…"}' \
  http://127.0.0.1:38090/api/v1/memory

# 列
curl -s -H "Authorization: Bearer $JWT" \
  'http://127.0.0.1:38090/api/v1/memory?agent_id=my-agent&level=L1&limit=20'

# 搜
curl -s -G -H "Authorization: Bearer $JWT" \
  --data-urlencode 'q=meeting' \
  http://127.0.0.1:38090/api/v1/memory/search

# 删
curl -s -X DELETE -H "Authorization: Bearer $JWT" \
  http://127.0.0.1:38090/api/v1/memory/<id>
```

**Memory Level 语义**：
- `L0` — short-term, scoped 到单次 execution，会被压缩
- `L1` — agent-scoped 半持久，会按 trust 升降
- `L2` — 长期事实，需高 trust + 操作员确认

### 2.3 人审 (Review) 流

当 Agent 触发 medium/high-risk capability，会产生 review：

```bash
# 列待审
curl -s -H "Authorization: Bearer $JWT" \
  'http://127.0.0.1:38090/api/v1/reviews?status=pending'

# 批准
curl -s -X POST -H "Authorization: Bearer $JWT" \
  -H 'Content-Type: application/json' \
  -d '{"comment":"OK to proceed"}' \
  http://127.0.0.1:38090/api/v1/reviews/<review_id>/approve

# 拒
curl -s -X POST -H "Authorization: Bearer $JWT" \
  -H 'Content-Type: application/json' \
  -d '{"comment":"unsafe target"}' \
  http://127.0.0.1:38090/api/v1/reviews/<review_id>/reject
```

### 2.4 OpenAI-compatible Chat Completions

兼容 OpenAI `/v1/chat/completions` 格式，可被任意 OpenAI SDK 直接消费：

```bash
curl -s -X POST -H "Authorization: Bearer $JWT" \
  -H 'Content-Type: application/json' \
  -d '{
    "model":"cyberclaw-default",
    "messages":[{"role":"user","content":"hi"}]
  }' \
  http://127.0.0.1:38090/v1/chat/completions
```

差别：response 中除 standard fields 外含 `cyberclaw_execution_id`，可用于 audit。

## 3. 速率限制

| 区段 | 默认 | 注解 |
|---|---|---|
| 应用 routes | 1 r/s + burst 60 | 通过 `RATE_LIMIT_PER_SECOND` / `RATE_LIMIT_BURST_SIZE` 调 |
| 内部 cluster | 独立配额（同上 envs，但 layer 独立） | |
| 429 返回头 | `Retry-After: <秒>` | |

E2E / 压测请提调到 ≥ 100 r/s + burst 500（staging 脚本默认）。

## 4. 错误约定

| HTTP | 含义 | Body |
|---|---|---|
| 200/201/204 | 成功 | varies |
| 400 | 请求格式错（schema 不符） | `{"error":"<msg>"}` |
| 401 | JWT 缺失/过期/签名错 | `{"error":"unauthorized"}` |
| 403 | 角色无权（如 viewer 调 mutation） | `{"error":"forbidden"}` |
| 404 | 资源不存在 / route 未匹配 | `{"error":"not_found"}` |
| 409 | 状态冲突（如 cancel 一个已完成的 execution） | `{"error":"conflict"}` |
| 429 | 速率限制 | `{"error":"too_many_requests"}` + `Retry-After` |
| 500 | 服务器内部错（生产环境消息会被脱敏） | `{"error":"internal_server_error"}` |

> 生产 `ENVIRONMENT=production`：500/4xx 错误会脱敏（不泄漏 stacktrace）。Staging `ENVIRONMENT=staging` 保留更多细节。

## 5. SSE 事件流（live updates）

`/admin/events` 提供 Server-Sent Events，admin SPA 用它做实时更新：

```bash
curl -N -H "Authorization: Bearer $JWT" \
  http://127.0.0.1:38090/admin/events
```

EventSource 客户端（浏览器）：

```js
const es = new EventSource('/admin/events?token=' + jwt);
es.onmessage = (ev) => console.log(JSON.parse(ev.data));
```

> EventSource 不能 set headers，所以 `?token=` query 是允许的（处理器自验，参考 `apps/cyberclaw-server/src/api/admin/events.rs`）。

## 6. Webhook 接入（消息平台）

`POST /webhooks/im/:platform` 接收 IM 消息（飞书/钉钉/Slack 等）。**不需 JWT**，依赖平台签名校验：

```
Authorization: HMAC-SHA256 <hex>
X-Platform-Signature: sha256=<hex>
```

支持的 platforms 由 `CYBERCLAW_WEBHOOK_SECRET_<PLATFORM>` env 决定。无 secret 配置 → reject 403。

## 7. 限制 / 已知坑

- **Body size limit**：默认 1 MB；超过 413
- **WebSocket**：暂未实现，使用 SSE
- **Pagination**：当前所有列表接口用 `limit=N` 单页，无 cursor。下一版加 `cursor` 字段
- **Bulk operations**：暂无 batch endpoint，逐个调用

## 8. SDK / 工具

无官方 SDK。推荐：
- TypeScript：`fetch` + `openapi-typescript-codegen` 生成 client（基于 `docs/api/openapi.yaml`）
- Python：`openapi-python-client generate --path docs/api/openapi.yaml`
- Rust：`utoipa-codegen` 或手写 reqwest wrapper

## 9. 完整 schema

- 91 routes 全量清单：[`ROUTES.md`](./ROUTES.md)
- OpenAPI 3.0 spec（部分覆盖）：[`openapi.yaml`](./openapi.yaml)
- 体系架构：[`docs/architecture/overview/ARCHITECTURE_V2.0.md`](../architecture/overview/ARCHITECTURE_V2.0.md)
