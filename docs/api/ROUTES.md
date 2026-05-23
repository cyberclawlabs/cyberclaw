# CyberClaw HTTP Route Inventory

> 自动生成参考。源：`grep -rE '\.route\("…", (get|post|put|delete|patch)' apps/cyberclaw-server/src/api/`。
> 共 **91 条 routes**（admin SPA assets、API、cluster、wizard、webhooks）。

更详细的 schema：
- 公共 API 子集见 [`openapi.yaml`](./openapi.yaml)
- 消费侧示例见 [`USER_MANUAL.md`](./USER_MANUAL.md)

## 鉴权矩阵

| 区段 | 鉴权 | 备注 |
|---|---|---|
| `/health`, `/ready`, `/metrics` | 公开 | 监控/LB 心跳 |
| `/admin`（HTML + dist 资源） | 公开 | SPA shell；客户端拿 JWT 后调 API |
| `/admin/login`, `/admin/onboarding/*`（部分） | 公开 / bootstrap-token | 首次启动用 bootstrap，已注册操作员 JWT |
| `/admin/me`, `/admin/dashboard`, `/admin/events`, `/admin/onboarding/status`, `/admin/seed-demo` | JWT | |
| `/api/v1/*` | JWT | 所有业务 API |
| `/internal/cluster/*` | shared-token (`CYBERCLAW_CLUSTER_SHARED_TOKEN`) + 独立 rate limit + body limit | 集群内部，不对外 |
| `/webhooks/*` | platform signature (HMAC-SHA256) | 不需 JWT，依赖签名校验 |
| `/wizard/*` | bootstrap-token / JWT | Onboarding Web UI |
| `/_dev/*` | feature flag (`CYBERCLAW_HANDOFF_ENABLED`) | dev only |
| `/v1/(agent/)?chat/completions` | JWT | OpenAI-compatible LLM proxy |

## 全量路由（按 surface area 分组）

### admin · 14 routes

| 方法 | 路径 |
|---|---|
| GET | `/admin/dashboard` |
| GET | `/admin/dist/:file` |
| GET | `/admin/events` |
| GET | `/admin/me` |
| GET | `/admin/onboarding/status` |
| GET | `/admin/src/:file` |
| POST | `/admin/login` |
| POST | `/admin/logout` |
| POST | `/admin/onboarding/complete` |
| POST | `/admin/onboarding/governance` |
| POST | `/admin/onboarding/llm-config` |
| POST | `/admin/onboarding/scan-mcp` |
| POST | `/admin/onboarding/test-llm` |
| POST | `/admin/seed-demo` |

### memory · 7 routes

| 方法 | 路径 |
|---|---|
| GET | `/api/v1/memory` |
| GET | `/api/v1/memory/:id` |
| GET | `/api/v1/memory/:id/trace` |
| GET | `/api/v1/memory/search` |
| POST | `/api/v1/memory` |
| POST | `/api/v1/memory/:id/edit` |
| DELETE | `/api/v1/memory/:id` |

### chat · 7 routes

| 方法 | 路径 |
|---|---|
| GET | `/api/v1/chat/clarify/:clarify_id` |
| GET | `/api/v1/chat/clarify/all` |
| GET | `/api/v1/chat/handoff` |
| GET | `/api/v1/chat/handoff/:id` |
| POST | `/api/v1/chat/approval` |
| POST | `/api/v1/chat/handoff/:id/accept` |
| POST | `/api/v1/chat/handoff/:id/reject` |

### agents · 4 routes

| 方法 | 路径 |
|---|---|
| GET | `/api/v1/agents` |
| GET | `/api/v1/agents/:id/digest` |
| GET | `/api/v1/agents/:name/status` |
| POST | `/api/v1/agents/:name/invoke` |

### tasks · 5 routes

| 方法 | 路径 |
|---|---|
| GET | `/api/v1/tasks` |
| GET | `/api/v1/tasks/:id` |
| GET | `/api/v1/tasks/:id/status` |
| POST | `/api/v1/tasks` |
| DELETE | `/api/v1/tasks/:id` |

### executions · 3 routes

| 方法 | 路径 |
|---|---|
| GET | `/api/v1/executions` |
| GET | `/api/v1/executions/:id/trace` |
| POST | `/api/v1/executions/:id/cancel` |

### reviews · 4 routes

| 方法 | 路径 |
|---|---|
| GET | `/api/v1/reviews` |
| GET | `/api/v1/reviews/:id` |
| POST | `/api/v1/reviews/:id/approve` |
| POST | `/api/v1/reviews/:id/reject` |

### skills · 6 routes

| 方法 | 路径 |
|---|---|
| GET | `/api/v1/skills` |
| GET | `/api/v1/skills/:id/content` |
| POST | `/api/v1/skills/create` |
| POST | `/api/v1/skills/install` |
| POST | `/api/v1/skills/install-remote` |
| DELETE | `/api/v1/skills/:id` |

### capabilities · 3 routes

| 方法 | 路径 |
|---|---|
| GET | `/api/v1/capabilities` |
| GET | `/api/v1/capabilities/:id` |
| GET | `/api/v1/capabilities/discover` |

### connectors · 1 route

| 方法 | 路径 |
|---|---|
| GET | `/api/v1/connectors` |

### tools · 1 route

| 方法 | 路径 |
|---|---|
| GET | `/api/v1/tools` |

### channels · 2 routes

| 方法 | 路径 |
|---|---|
| GET | `/api/v1/channels` |
| PUT | `/api/v1/channels/:id` |

### workflows · 5 routes

| 方法 | 路径 |
|---|---|
| GET | `/api/v1/workflows/:id` |
| POST | `/api/v1/workflows` |
| POST | `/api/v1/workflows/:id/cancel` |
| POST | `/api/v1/workflows/:id/pause` |
| POST | `/api/v1/workflows/:id/resume` |

### settings · 4 routes

| 方法 | 路径 |
|---|---|
| GET | `/api/v1/settings/about` |
| GET | `/api/v1/settings/config` |
| GET | `/api/v1/settings/env` |
| GET | `/api/v1/settings/policies` |

### audit · 3 routes

| 方法 | 路径 |
|---|---|
| GET | `/api/v1/audit/logs` |
| GET | `/api/v1/audit/logs/export` |
| GET | `/api/v1/audit/verify` |

### workbench · 3 routes

| 方法 | 路径 |
|---|---|
| GET | `/api/v1/workbench/diagnose` |
| GET | `/api/v1/workbench/inspect/:kind/:id` |
| POST | `/api/v1/workbench/dry-run` |

### governance · 1 route

| 方法 | 路径 |
|---|---|
| GET | `/api/v1/governance/policy-rules` |

### cluster · 2 routes

| 方法 | 路径 |
|---|---|
| GET | `/api/v1/cluster/nodes` |
| GET | `/api/v1/cluster/nodes/:id` |

### internal-cluster · 1 route

| 方法 | 路径 |
|---|---|
| POST | `/internal/cluster/assignments/pull` |

### wizard · 6 routes

| 方法 | 路径 |
|---|---|
| GET | `/wizard` |
| GET | `/wizard/events` |
| GET | `/wizard/status` |
| POST | `/wizard/cancel` |
| POST | `/wizard/next` |
| POST | `/wizard/start` |

### webhooks · 2 routes

| 方法 | 路径 |
|---|---|
| POST | `/webhooks/:platform` |
| POST | `/webhooks/im/:platform` |

### dev · 1 route

| 方法 | 路径 |
|---|---|
| POST | `/_dev/trigger_handoff` |

### meta · 6 routes

| 方法 | 路径 |
|---|---|
| GET | `/admin` |
| GET | `/health` |
| GET | `/metrics` |
| GET | `/ready` |
| POST | `/v1/agent/chat/completions` |
| POST | `/v1/chat/completions` |

## Stats

```
delete   3
get     52
post    35
put      1
total   91
```
