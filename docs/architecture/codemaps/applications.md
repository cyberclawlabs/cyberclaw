# 应用层架构

**最后更新:** 2024-03-18
**目录:** `apps/`
**状态:** 🚧 规划中

## 应用概览

```
Applications Layer
├── cyberclaw-server  - HTTP API 服务器
└── cyberclaw-cli     - 命令行工具
```

## 1. CyberClaw Server (API 服务器)

**目录:** `apps/cyberclaw-server/`
**状态:** 脚手架阶段

### 设计目标

```
功能职责:
├── HTTP API Gateway
│   ├── RESTful API
│   ├── GraphQL (可选)
│   └── WebSocket (实时通信)
│
├── 请求路由
│   ├── /api/v1/tasks      - 任务管理
│   ├── /api/v1/cases      - 案例管理
│   ├── /api/v1/agents     - Agent 管理
│   ├── /api/v1/workflows  - 工作流管理
│   └── /api/v1/review     - 审批管理
│
├── 认证与授权
│   ├── JWT Token
│   ├── API Key
│   ├── OAuth 2.0
│   └── RBAC 权限
│
└── 集成层
    ├── Control Plane
    ├── Execution Service
    └── Governance Gate
```

### API 端点设计（规划）

#### 任务管理 API

```
POST   /api/v1/tasks              - 创建任务
GET    /api/v1/tasks              - 列出任务
GET    /api/v1/tasks/:id          - 获取任务详情
PATCH  /api/v1/tasks/:id          - 更新任务
DELETE /api/v1/tasks/:id          - 取消任务
GET    /api/v1/tasks/:id/status   - 获取任务状态
GET    /api/v1/tasks/:id/logs     - 获取任务日志
```

**请求示例:**
```json
POST /api/v1/tasks
{
  "title": "Security Scan",
  "description": "Scan repository for vulnerabilities",
  "agent": "security-scanner",
  "input": {
    "repository": "https://github.com/org/repo",
    "branch": "main"
  },
  "priority": "high"
}
```

**响应示例:**
```json
{
  "id": "task-123",
  "status": "pending",
  "createdAt": "2024-03-18T10:00:00Z",
  "executionId": "exec-456",
  "estimatedDuration": 300
}
```

#### 案例管理 API

```
POST   /api/v1/cases              - 创建案例
GET    /api/v1/cases              - 列出案例
GET    /api/v1/cases/:id          - 获取案例详情
PATCH  /api/v1/cases/:id          - 更新案例
POST   /api/v1/cases/:id/tasks    - 添加任务到案例
GET    /api/v1/cases/:id/timeline - 获取案例时间线
```

#### Agent 管理 API

```
GET    /api/v1/agents             - 列出所有 Agent
GET    /api/v1/agents/:id         - 获取 Agent 详情
POST   /api/v1/agents/:id/invoke  - 直接调用 Agent
GET    /api/v1/agents/:id/metrics - 获取 Agent 指标
```

#### 审批管理 API

```
GET    /api/v1/reviews            - 列出待审批项
GET    /api/v1/reviews/:id        - 获取审批详情
POST   /api/v1/reviews/:id/approve - 批准
POST   /api/v1/reviews/:id/reject  - 拒绝
POST   /api/v1/reviews/:id/comment - 添加评论
```

### 服务器架构（规划）

```rust
// 主入口
pub struct CyberClawServer {
    config: ServerConfig,
    router: Router,
    control_plane: Arc<ControlPlaneOrchestrator>,
    auth_service: Arc<AuthService>,
}

// 服务器配置
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub tls: Option<TlsConfig>,
    pub cors: CorsConfig,
    pub rate_limit: RateLimitConfig,
}

// 路由定义
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .nest("/api/v1/tasks", task_routes())
        .nest("/api/v1/cases", case_routes())
        .nest("/api/v1/agents", agent_routes())
        .nest("/api/v1/reviews", review_routes())
        .layer(AuthMiddleware::new())
        .layer(RateLimitMiddleware::new())
        .with_state(state)
}
```

### 中间件栈

```
请求 →
  ├→ CORS Middleware
  ├→ Rate Limiting
  ├→ Authentication
  ├→ Authorization
  ├→ Request Logging
  ├→ Tracing
  │
  └→ 路由处理
      ├→ 请求验证
      ├→ 业务逻辑
      ├→ 响应序列化
      └→ 错误处理
```

### WebSocket 支持（规划）

```
WS /api/v1/stream/tasks/:id
  ├→ 实时任务状态更新
  ├→ 日志流式输出
  └→ 事件推送

WS /api/v1/stream/events
  ├→ 全局事件订阅
  ├→ 过滤器支持
  └→ 多客户端广播
```

## 2. CyberClaw CLI (命令行工具)

**目录:** `apps/cyberclaw-cli/`
**状态:** 脚手架阶段

### 设计目标

```
功能职责:
├── 任务管理
│   ├── task create   - 创建任务
│   ├── task list     - 列出任务
│   ├── task get      - 查看任务
│   ├── task logs     - 查看日志
│   └── task cancel   - 取消任务
│
├── 包管理
│   ├── pkg list      - 列出包
│   ├── pkg search    - 搜索包
│   ├── pkg install   - 安装包
│   ├── pkg update    - 更新包
│   └── pkg remove    - 移除包
│
├── Agent 交互
│   ├── agent list    - 列出 Agent
│   ├── agent run     - 运行 Agent
│   └── agent chat    - 交互式对话
│
└── 运维工具
    ├── cluster nodes - 查看节点
    ├── cluster health- 健康检查
    ├── logs stream   - 日志流
    └── config show   - 查看配置
```

### CLI 命令结构（规划）

```bash
# 任务管理
cyberclaw task create --agent security-scanner --input repo.json
cyberclaw task list --status running
cyberclaw task get task-123
cyberclaw task logs task-123 --follow
cyberclaw task cancel task-123

# 案例管理
cyberclaw case create --title "Security Audit Q1"
cyberclaw case add-task case-456 task-123
cyberclaw case timeline case-456

# Agent 管理
cyberclaw agent list
cyberclaw agent run security-scanner --input '{"repo": "..."}'
cyberclaw agent chat master-agent

# 包管理
cyberclaw pkg list --kind agent
cyberclaw pkg search security
cyberclaw pkg install security-scanner@1.0.0
cyberclaw pkg update security-scanner
cyberclaw pkg remove security-scanner

# 工作流
cyberclaw workflow run security-scan.yaml
cyberclaw workflow list
cyberclaw workflow status workflow-789

# 审批
cyberclaw review list --pending
cyberclaw review approve review-101
cyberclaw review reject review-102 --reason "..."

# 集群管理
cyberclaw cluster nodes
cyberclaw cluster health
cyberclaw cluster members

# 配置
cyberclaw config show
cyberclaw config set api.endpoint https://api.example.com
cyberclaw config get api.endpoint

# 日志
cyberclaw logs stream --filter agent=security-scanner
cyberclaw logs search "error" --last 1h
```

### CLI 架构（规划）

```rust
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "cyberclaw")]
#[command(about = "CyberClaw Platform CLI")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    #[arg(long, global = true)]
    pub config: Option<String>,

    #[arg(long, global = true)]
    pub endpoint: Option<String>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Task management
    #[command(subcommand)]
    Task(TaskCommands),

    /// Case management
    #[command(subcommand)]
    Case(CaseCommands),

    /// Agent operations
    #[command(subcommand)]
    Agent(AgentCommands),

    /// Package management
    #[command(subcommand)]
    Pkg(PkgCommands),

    /// Workflow operations
    #[command(subcommand)]
    Workflow(WorkflowCommands),

    /// Review operations
    #[command(subcommand)]
    Review(ReviewCommands),

    /// Cluster management
    #[command(subcommand)]
    Cluster(ClusterCommands),

    /// Configuration
    #[command(subcommand)]
    Config(ConfigCommands),

    /// Logs
    #[command(subcommand)]
    Logs(LogsCommands),
}
```

### 交互式模式（规划）

```bash
# 进入交互式 shell
cyberclaw shell

cyberclaw> task create
? Select agent: ❯ security-scanner
               code-reviewer
               report-agent

? Repository URL: https://github.com/org/repo
? Branch: main
? Priority: ❯ high
            medium
            low

✓ Task created: task-123
  Status: pending
  Estimated duration: 5 minutes

cyberclaw> task logs task-123 --follow
[2024-03-18 10:00:00] Initializing agent...
[2024-03-18 10:00:01] Loading skills: static-analysis
[2024-03-18 10:00:02] Scanning repository...
```

### 配置文件（规划）

**位置:** `~/.cyberclaw/config.yaml`

```yaml
api:
  endpoint: https://api.cyberclaw.example.com
  token: eyJhbGciOiJIUzI1NiIs...

defaults:
  agent: security-scanner
  priority: medium
  format: table  # table, json, yaml

aliases:
  scan: task create --agent security-scanner
  review: task list --status pending-review

output:
  color: true
  verbose: false
```

## 应用集成架构

```
┌─────────────────────────────────────────────┐
│            Client Applications               │
│  ┌──────────────┐      ┌──────────────┐    │
│  │   Browser    │      │   CLI Tool   │    │
│  │   (Future)   │      │              │    │
│  └──────┬───────┘      └──────┬───────┘    │
│         │                     │             │
└─────────┼─────────────────────┼─────────────┘
          │                     │
          │  HTTP/WebSocket     │  HTTP/gRPC
          │                     │
┌─────────▼─────────────────────▼─────────────┐
│          CyberClaw Server                    │
│  ┌────────────────────────────────────┐     │
│  │      API Gateway + Router          │     │
│  └────────┬───────────────────────────┘     │
│           │                                  │
│  ┌────────▼───────────────────────────┐     │
│  │    Control Plane Orchestrator      │     │
│  │  • TaskManager                     │     │
│  │  • CaseManager                     │     │
│  │  • ReviewQueue                     │     │
│  └────────────────────────────────────┘     │
└─────────────────────────────────────────────┘
```

## 部署模式

### 1. 单机模式

```
cyberclaw-server --mode standalone --port 8080

包含：
├── API Server
├── Control Plane
├── Execution Runtime
└── Local Storage
```

### 2. 分布式模式（规划）

```
# API Server 层（多实例）
cyberclaw-server --mode api --cluster-endpoint consul://...

# Control Plane 层（多实例）
cyberclaw-server --mode control-plane --cluster-endpoint consul://...

# Execution Runtime 层（多实例）
cyberclaw-server --mode runtime --cluster-endpoint consul://...
```

## 监控与可观测

### 健康检查端点

```
GET /health
{
  "status": "healthy",
  "version": "2.0.0",
  "uptime": 3600,
  "components": {
    "controlPlane": "healthy",
    "database": "healthy",
    "eventBus": "healthy"
  }
}

GET /metrics
# Prometheus 格式指标
cyberclaw_tasks_total{status="completed"} 42
cyberclaw_tasks_total{status="failed"} 3
cyberclaw_agents_active 5
cyberclaw_api_requests_total{endpoint="/tasks",method="POST"} 100
```

### 日志结构

```json
{
  "timestamp": "2024-03-18T10:00:00Z",
  "level": "info",
  "target": "cyberclaw_server::api",
  "message": "Task created",
  "fields": {
    "task_id": "task-123",
    "agent": "security-scanner",
    "user": "user@example.com"
  }
}
```

## 安全特性

### 认证

```
支持的认证方式：
├── JWT Token (短期访问令牌)
├── API Key (长期服务密钥)
└── OAuth 2.0 (第三方集成)
```

### 授权

```
RBAC 角色：
├── admin      - 全部权限
├── operator   - 运维权限
├── developer  - 开发权限
└── viewer     - 只读权限

权限示例：
- tasks:create
- tasks:read
- tasks:update
- tasks:delete
- agents:invoke
- reviews:approve
```

### 速率限制

```
限流策略：
├── 匿名: 100 req/hour
├── 认证: 1000 req/hour
└── 高级: 10000 req/hour

限流响应：
HTTP 429 Too Many Requests
{
  "error": "Rate limit exceeded",
  "retryAfter": 3600
}
```

## 未来扩展

### v2.1 规划
- [ ] HTTP API Server MVP
- [ ] CLI 基础命令
- [ ] JWT 认证

### v2.2 规划
- [ ] WebSocket 实时推送
- [ ] 交互式 CLI
- [ ] GraphQL API

### v2.3 规划
- [ ] Web Dashboard
- [ ] 分布式部署
- [ ] 高可用架构

## 相关文档

- [控制平面](./control-plane.md) - 后端服务
- [运行时层](./runtime-layers.md) - Agent 执行
- [治理层](./governance.md) - 权限和审批
- [可观测层](./observability.md) - 日志和指标

---

**维护说明:** 应用层目前处于脚手架阶段，本文档描述设计目标和 API 规划。
