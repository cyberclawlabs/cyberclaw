# CyberClaw P2 Architecture Design - Extensibility & Automation

- Status: Draft
- Scope: Architecture Design
- Owner: CyberClaw Development Team
- Date: 2026-03-23
- Phase: P2 - Extensibility & Automation
- Target Milestone: Release Candidate

## 执行摘要

**阶段目标**: 将 CyberClaw 从"本地可控平台 (Beta)"推进到"可扩展、可持续运行的平台 (Release Candidate)"

**开发模式**: **10-Agent 高速并行开发**

**预计时长**: 5-6 周

**核心交付物**:
1. ✅ Platform Plugin 运行时系统
2. ✅ MCP (Model Context Protocol) Connector
3. ✅ 外部系统 Connector 生态 (GitHub, Database, Slack)
4. ✅ Heartbeat + Cron 自动化调度系统
5. ✅ Skill 格式兼容层 (Claude Code, Codex, OpenClaw)

---

## 1. P2 完成标准

根据 DEVELOPMENT_ROADMAP_V2.0.md，P2 阶段需要满足以下标准：

### 1.1 功能标准

- [x] `Platform Plugin` 运行时落地
- [x] `HookDispatcher + failurePolicy` 完成
- [x] `McpConnector` 落地
- [x] 外部系统 connector 可接入
- [x] `heartbeat + cron` 双环落地
- [x] 自动化任务统一进入 `Task -> Execution` 主链路
- [x] `Skill` 兼容主流 skill 格式的加载和适配规则稳定

### 1.2 质量标准

- [ ] 测试覆盖率 > 75%
- [ ] 0 编译警告
- [ ] 所有 CRITICAL/HIGH 安全问题修复
- [ ] 性能基准测试通过
- [ ] 所有新增 API 有文档注释

### 1.3 文档标准

- [ ] 所有新增模块有 README.md
- [ ] 至少 3 个 End-to-End 示例
- [ ] 开发者指南完整
- [ ] API 参考文档完整

### 1.4 生态标准

- [ ] 至少 3 个生产级 External Connectors
- [ ] 至少 2 种 Skill 格式兼容
- [ ] 至少 1 个社区示例 Plugin

---

## 2. 10-Agent 并行开发计划

### 2.1 Agent 分配表

| Agent ID | 职责 | 模型 | 主要交付物 | 预计工作量 |
|----------|------|------|------------|------------|
| **Agent 1** | Platform Plugin - 核心运行时 | Opus | PluginLoader, PluginSandbox, PluginLifecycle | 高 |
| **Agent 2** | Platform Plugin - Hook 系统 | Sonnet | HookDispatcher, FailurePolicy, EventBus | 高 |
| **Agent 3** | MCP Connector | Sonnet | McpConnector, Protocol 封装, Tool/Resource 映射 | 中 |
| **Agent 4** | GitHub Connector | Sonnet | GitHubConnector, OAuth 集成, Webhook 处理 | 中 |
| **Agent 5** | Database Connector | Sonnet | DatabaseConnector, Connection Pool, 多DB支持 | 中 |
| **Agent 6** | Slack Connector | Sonnet | SlackConnector, Bot 集成, Message 模板 | 中 |
| **Agent 7** | Heartbeat 监控系统 | Sonnet | HeartbeatMonitor, 健康检查, 异常检测 | 中 |
| **Agent 8** | Cron 调度系统 | Sonnet | CronScheduler, 任务队列, 执行历史 | 中 |
| **Agent 9** | Skill Loader | Sonnet | Multi-format Loader, UnifiedAdapter, 热重载 | 中 |
| **Agent 10** | 集成测试 + 文档 | Opus | E2E 测试套件, 开发者文档, 示例项目 | 高 |

### 2.2 模块依赖关系

```
┌─────────────────────────────────────────────────────────┐
│                   P2 架构全景图                          │
└─────────────────────────────────────────────────────────┘

┌──────────────────────┐
│  Platform Plugins    │  ← Agent 1, 2
│  ┌────────────────┐  │
│  │ PluginLoader   │  │
│  │ HookDispatcher │  │
│  │ PluginSandbox  │  │
│  └────────────────┘  │
└──────────────────────┘
         ↓ 使用
┌──────────────────────┐  ┌──────────────────────┐
│   Connectors         │  │   Scheduler          │
│  ┌────────────────┐  │  │  ┌────────────────┐  │
│  │ McpConnector   │◄─┼──┼──│ HeartbeatMon   │  │ ← Agent 7, 8
│  │ (Agent 3)      │  │  │  │ CronScheduler  │  │
│  ├────────────────┤  │  │  └────────────────┘  │
│  │ GitHubConn     │  │  └──────────────────────┘
│  │ (Agent 4)      │  │           ↓ 触发
│  ├────────────────┤  │  ┌──────────────────────┐
│  │ DatabaseConn   │  │  │   Task Executor      │
│  │ (Agent 5)      │  │  │  (统一执行入口)       │
│  ├────────────────┤  │  └──────────────────────┘
│  │ SlackConn      │  │
│  │ (Agent 6)      │  │
│  └────────────────┘  │
└──────────────────────┘
         ↑ 加载
┌──────────────────────┐
│   Skill Runtime      │  ← Agent 9
│  ┌────────────────┐  │
│  │ SkillLoader    │  │
│  │ UnifiedAdapter │  │
│  └────────────────┘  │
└──────────────────────┘
         ↑
┌──────────────────────┐
│ Integration & Tests  │  ← Agent 10
│ (E2E, Docs, Examples)│
└──────────────────────┘
```

### 2.3 开发时间线

**Week 1: 架构设计 + 技术调研**
- All Agents: 架构设计评审
- Agent 1-2: Plugin 动态加载技术选型 (libloading vs WASM)
- Agent 3: MCP 协议规范研究
- Agent 4-6: 外部 API 集成方案
- Agent 7-8: Cron 表达式库选型
- Agent 9: Skill 格式标准对比
- Agent 10: 测试框架设计

**Week 2: 核心开发 I**
- Agent 1: PluginLoader + PluginRegistry
- Agent 2: HookDispatcher 基础
- Agent 3: MCP Protocol 封装
- Agent 4: GitHub API 集成
- Agent 5: Database Connection Pool
- Agent 6: Slack Bot 集成
- Agent 7: Heartbeat 基础框架
- Agent 8: Cron Parser 集成
- Agent 9: Claude Code Skill Loader
- Agent 10: 单元测试模板

**Week 3: 核心开发 II**
- Agent 1: PluginSandbox + 隔离执行
- Agent 2: FailurePolicy + EventBus
- Agent 3: Tool/Resource 映射
- Agent 4: GitHub Webhook 处理
- Agent 5: 多数据库支持 (PostgreSQL, MySQL, SQLite)
- Agent 6: Slack Message 模板
- Agent 7: 健康检查与异常检测
- Agent 8: 任务队列管理
- Agent 9: Codex/OpenClaw Loader
- Agent 10: 集成测试用例

**Week 4: 集成与优化**
- Agent 1-2: Plugin 集成测试
- Agent 3: MCP 协议兼容性测试
- Agent 4-6: External Connectors 集成测试
- Agent 7-8: Scheduler 压力测试
- Agent 9: Skill 热重载测试
- Agent 10: E2E 测试套件

**Week 5: 文档与交付**
- All Agents: Bug 修复与优化
- Agent 10: 开发者文档编写
- Agent 10: 示例项目创建
- All Agents: Code Review 互审

---

## 3. 技术架构设计

### 3.1 Platform Plugin 系统 (Agent 1-2)

#### 3.1.1 核心组件

**PluginLoader** (`crates/cyberclaw-plugin-runtime/src/loader.rs`)

```rust
use libloading::Library;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Plugin 动态加载器
pub struct PluginLoader {
    /// 已加载的 Plugin 库
    loaded: Arc<RwLock<HashMap<PluginId, LoadedPlugin>>>,
    /// Plugin 搜索路径
    search_paths: Vec<PathBuf>,
    /// 安全策略
    security: Arc<PluginSecurityPolicy>,
}

/// 已加载的 Plugin
struct LoadedPlugin {
    id: PluginId,
    library: Library,
    metadata: PluginMetadata,
    hooks: Vec<HookRegistration>,
    state: PluginState,
}

#[derive(Debug, Clone, PartialEq)]
enum PluginState {
    Loaded,
    Initialized,
    Enabled,
    Disabled,
    Failed(String),
}

impl PluginLoader {
    /// 加载 Plugin
    pub async fn load_plugin(&self, manifest_path: PathBuf) -> Result<PluginId> {
        // 1. 读取 manifest
        let manifest = PluginManifest::from_file(&manifest_path).await?;

        // 2. 安全验证
        self.security.verify_plugin(&manifest).await?;

        // 3. 加载动态库
        let lib_path = manifest_path.parent()
            .ok_or(Error::InvalidPath)?
            .join(&manifest.library_path);

        let library = unsafe { Library::new(&lib_path)? };

        // 4. 提取符号
        let init_fn: libloading::Symbol<PluginInitFn> =
            unsafe { library.get(b"cyberclaw_plugin_init")? };

        // 5. 调用初始化
        let plugin_api = init_fn(&manifest)?;

        // 6. 注册到运行时
        let plugin_id = PluginId::new();
        let loaded = LoadedPlugin {
            id: plugin_id.clone(),
            library,
            metadata: manifest.metadata,
            hooks: plugin_api.hooks,
            state: PluginState::Loaded,
        };

        self.loaded.write().await.insert(plugin_id.clone(), loaded);

        Ok(plugin_id)
    }

    /// 卸载 Plugin
    pub async fn unload_plugin(&self, plugin_id: &PluginId) -> Result<()> {
        // 1. 移除 hooks
        // 2. 调用 cleanup
        // 3. 卸载动态库
        // 4. 清理资源
        todo!()
    }
}
```

**HookDispatcher** (`crates/cyberclaw-plugin-runtime/src/hooks.rs`)

```rust
use tokio::sync::mpsc;
use std::sync::Arc;

/// Hook 事件分发器
pub struct HookDispatcher {
    /// Hook 注册表
    registry: Arc<RwLock<HookRegistry>>,
    /// 事件总线
    event_bus: mpsc::UnboundedSender<HookEvent>,
    /// 失败策略
    failure_policy: FailurePolicy,
}

/// Hook 注册
#[derive(Debug, Clone)]
pub struct HookRegistration {
    pub plugin_id: PluginId,
    pub hook_type: HookType,
    pub handler: Arc<dyn HookHandler>,
    pub priority: i32,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookType {
    BeforeExecution,
    AfterExecution,
    OnFailure,
    OnReview,
    BeforeCapability,
    AfterCapability,
}

#[derive(Debug, Clone)]
pub enum FailurePolicy {
    /// 忽略失败,继续执行
    Ignore,
    /// 重试 N 次
    Retry { max_attempts: u32 },
    /// 中止执行
    Abort,
}

impl HookDispatcher {
    /// 分发 Hook 事件
    pub async fn dispatch(
        &self,
        hook_type: HookType,
        context: &HookContext,
    ) -> Result<HookResult> {
        // 1. 获取注册的 Hooks (按优先级排序)
        let hooks = self.registry.read().await
            .get_hooks_for_type(hook_type)
            .sorted_by_key(|h| h.priority)
            .collect::<Vec<_>>();

        // 2. 依次调用 Hooks
        let mut results = Vec::new();
        for hook in hooks {
            match self.call_hook(hook, context).await {
                Ok(result) => results.push(result),
                Err(e) => {
                    // 应用失败策略
                    match &self.failure_policy {
                        FailurePolicy::Ignore => {
                            tracing::warn!("Hook failed but ignored: {}", e);
                            continue;
                        }
                        FailurePolicy::Retry { max_attempts } => {
                            // 重试逻辑
                            for attempt in 1..=*max_attempts {
                                match self.call_hook(hook, context).await {
                                    Ok(r) => {
                                        results.push(r);
                                        break;
                                    }
                                    Err(retry_err) if attempt == *max_attempts => {
                                        return Err(retry_err);
                                    }
                                    _ => continue,
                                }
                            }
                        }
                        FailurePolicy::Abort => {
                            return Err(e);
                        }
                    }
                }
            }
        }

        Ok(HookResult::combine(results))
    }

    async fn call_hook(
        &self,
        hook: &HookRegistration,
        context: &HookContext,
    ) -> Result<HookOutput> {
        // 带超时的 Hook 调用
        tokio::time::timeout(
            Duration::from_millis(hook.timeout_ms),
            hook.handler.handle(context),
        )
        .await
        .map_err(|_| Error::HookTimeout)?
    }
}
```

#### 3.1.2 Plugin 安全模型

```rust
/// Plugin 安全策略
pub struct PluginSecurityPolicy {
    /// 允许的 capability 白名单
    allowed_capabilities: HashSet<String>,
    /// 资源配额限制
    resource_limits: ResourceLimits,
    /// 签名验证
    signature_verification: bool,
}

#[derive(Debug, Clone)]
pub struct ResourceLimits {
    /// 最大内存使用 (bytes)
    pub max_memory: usize,
    /// 最大 CPU 时间 (ms)
    pub max_cpu_ms: u64,
    /// 最大文件打开数
    pub max_file_handles: usize,
    /// 最大网络连接数
    pub max_network_connections: usize,
}

impl PluginSecurityPolicy {
    /// 验证 Plugin 是否可信
    pub async fn verify_plugin(&self, manifest: &PluginManifest) -> Result<()> {
        // 1. 检查签名
        if self.signature_verification {
            self.verify_signature(manifest)?;
        }

        // 2. 检查 capabilities
        for cap in &manifest.required_capabilities {
            if !self.allowed_capabilities.contains(cap) {
                return Err(Error::UnauthorizedCapability(cap.clone()));
            }
        }

        // 3. 检查资源请求
        if manifest.resource_request.memory > self.resource_limits.max_memory {
            return Err(Error::ExcessiveResourceRequest);
        }

        Ok(())
    }
}
```

#### 3.1.3 Plugin Manifest 格式

```toml
# ecosystem/plugins/example-plugin/cyberclaw-plugin.toml

[plugin]
id = "example-plugin"
name = "Example Plugin"
version = "0.1.0"
description = "An example CyberClaw plugin"
authors = ["Your Name <you@example.com>"]

[plugin.library]
path = "target/release/libexample_plugin.so"
entry_point = "cyberclaw_plugin_init"

[plugin.hooks]
# 注册的 Hook 列表
before_execution = { handler = "before_exec_hook", priority = 100, timeout_ms = 5000 }
after_execution = { handler = "after_exec_hook", priority = 100, timeout_ms = 5000 }

[plugin.capabilities]
# Plugin 需要的 capabilities
required = ["fs.read", "network.http"]

[plugin.resources]
# 资源请求
memory = 104857600  # 100 MB
cpu_ms = 10000      # 10 seconds
file_handles = 100
network_connections = 10

[plugin.metadata]
homepage = "https://example.com"
repository = "https://github.com/example/plugin"
license = "Apache-2.0"
```

---

### 3.2 MCP Connector (Agent 3)

#### 3.2.1 MCP 协议概述

Model Context Protocol (MCP) 是一个标准化的协议,用于 AI 模型访问外部上下文和工具。

**核心概念**:
- **Tools**: 可调用的函数 (类似 CyberClaw Capability)
- **Resources**: 可读取的数据源 (文件、API、数据库等)
- **Prompts**: 预定义的提示模板

**协议栈**:
```
JSON-RPC 2.0 (传输层)
    ↓
MCP Protocol (应用层)
    ↓
Tools / Resources / Prompts (语义层)
```

#### 3.2.2 McpConnector 实现

```rust
// crates/cyberclaw-connectors/src/mcp_connector.rs

use jsonrpc_core::{IoHandler, Params};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;

/// MCP Connector
pub struct McpConnector {
    /// 服务器地址
    server_url: String,
    /// JSON-RPC 客户端
    rpc_client: Arc<JsonRpcClient>,
    /// Tool/Resource 缓存
    cache: Arc<RwLock<McpCache>>,
    /// 超时配置
    timeout: Duration,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    pub mime_type: Option<String>,
}

impl Connector for McpConnector {
    fn id(&self) -> &str {
        "mcp-connector"
    }

    fn manifest(&self) -> ConnectorManifest {
        ConnectorManifest {
            id: "mcp-connector".into(),
            name: "MCP Connector".into(),
            version: "1.0.0".into(),
            capabilities: self.list_capabilities(),
        }
    }

    async fn execute(
        &self,
        capability: &str,
        request: CapabilityRequest,
    ) -> Result<CapabilityResponse> {
        // 动态路由到 MCP Tool
        match capability {
            cap if cap.starts_with("mcp.tool.") => {
                let tool_name = cap.strip_prefix("mcp.tool.").unwrap();
                self.call_mcp_tool(tool_name, request).await
            }
            cap if cap.starts_with("mcp.resource.") => {
                let resource_uri = cap.strip_prefix("mcp.resource.").unwrap();
                self.read_mcp_resource(resource_uri, request).await
            }
            _ => Err(Error::UnknownCapability(capability.to_string())),
        }
    }
}

impl McpConnector {
    /// 调用 MCP Tool
    async fn call_mcp_tool(
        &self,
        tool_name: &str,
        request: CapabilityRequest,
    ) -> Result<CapabilityResponse> {
        // 1. 构造 JSON-RPC 请求
        let rpc_request = json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": {
                "name": tool_name,
                "arguments": request.params,
            },
            "id": uuid::Uuid::new_v4().to_string(),
        });

        // 2. 发送请求
        let response: serde_json::Value = self.rpc_client
            .call(rpc_request)
            .await?;

        // 3. 解析响应
        Ok(CapabilityResponse {
            output: response["result"].clone(),
            metadata: Default::default(),
        })
    }

    /// 读取 MCP Resource
    async fn read_mcp_resource(
        &self,
        resource_uri: &str,
        _request: CapabilityRequest,
    ) -> Result<CapabilityResponse> {
        // 1. 检查缓存
        if let Some(cached) = self.cache.read().await.get(resource_uri) {
            return Ok(cached.clone());
        }

        // 2. 请求 Resource
        let rpc_request = json!({
            "jsonrpc": "2.0",
            "method": "resources/read",
            "params": {
                "uri": resource_uri,
            },
            "id": uuid::Uuid::new_v4().to_string(),
        });

        let response: serde_json::Value = self.rpc_client
            .call(rpc_request)
            .await?;

        // 3. 缓存并返回
        let result = CapabilityResponse {
            output: response["result"].clone(),
            metadata: Default::default(),
        };

        self.cache.write().await.insert(resource_uri.to_string(), result.clone());

        Ok(result)
    }

    /// 动态发现 Capabilities
    async fn discover_capabilities(&self) -> Result<Vec<CapabilityContract>> {
        // 1. 列举 Tools
        let tools: Vec<McpTool> = self.rpc_client
            .call(json!({
                "jsonrpc": "2.0",
                "method": "tools/list",
                "id": "discovery",
            }))
            .await?;

        // 2. 映射为 Capabilities
        let mut capabilities = Vec::new();
        for tool in tools {
            capabilities.push(CapabilityContract {
                id: format!("mcp.tool.{}", tool.name),
                description: tool.description,
                risk: RiskLevel::Medium,
                requires_review: false,
                input_schema: tool.input_schema,
            });
        }

        // 3. 列举 Resources (同理)

        Ok(capabilities)
    }
}
```

#### 3.2.3 MCP Server 配置

```yaml
# ecosystem/connectors/mcp-example/mcp-server.yaml

servers:
  - name: "filesystem"
    url: "stdio://mcp-server-filesystem"
    command: "npx"
    args: ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]

  - name: "database"
    url: "http://localhost:8080/mcp"
    type: "http"

  - name: "custom"
    url: "stdio://./custom-mcp-server"
    command: "./custom-mcp-server"
```

---

### 3.3 External Connectors (Agent 4-6)

#### 3.3.1 统一 Connector 模式

所有外部 Connector 遵循统一模式:

```rust
// 统一认证接口
#[async_trait]
pub trait Authenticator: Send + Sync {
    async fn authenticate(&self) -> Result<Credentials>;
    async fn refresh(&self) -> Result<Credentials>;
}

// 统一速率限制
pub struct RateLimiter {
    permits_per_second: u32,
    bucket: Arc<Mutex<TokenBucket>>,
}

// 统一重试策略
pub struct RetryPolicy {
    max_retries: u32,
    backoff: BackoffStrategy,
}

// Connector 模板
pub struct ExternalConnectorTemplate {
    auth: Arc<dyn Authenticator>,
    rate_limiter: Arc<RateLimiter>,
    retry_policy: RetryPolicy,
    http_client: reqwest::Client,
}
```

#### 3.3.2 GitHub Connector (Agent 4)

**Capabilities**:
- `github.create_issue`: 创建 Issue
- `github.create_pr`: 创建 Pull Request
- `github.review_code`: 代码审查
- `github.list_repos`: 列举仓库
- `github.search_code`: 搜索代码

**实现**:

```rust
// crates/cyberclaw-connectors/src/github_connector.rs

pub struct GitHubConnector {
    auth: Arc<GitHubAuth>,
    client: octocrab::Octocrab,
    rate_limiter: Arc<RateLimiter>,
}

impl Connector for GitHubConnector {
    async fn execute(
        &self,
        capability: &str,
        request: CapabilityRequest,
    ) -> Result<CapabilityResponse> {
        // 速率限制
        self.rate_limiter.acquire().await?;

        match capability {
            "github.create_issue" => self.create_issue(request).await,
            "github.create_pr" => self.create_pr(request).await,
            "github.review_code" => self.review_code(request).await,
            _ => Err(Error::UnknownCapability(capability.to_string())),
        }
    }
}

impl GitHubConnector {
    async fn create_issue(&self, req: CapabilityRequest) -> Result<CapabilityResponse> {
        let owner = req.params["owner"].as_str()?;
        let repo = req.params["repo"].as_str()?;
        let title = req.params["title"].as_str()?;
        let body = req.params["body"].as_str()?;

        let issue = self.client
            .issues(owner, repo)
            .create(title)
            .body(body)
            .send()
            .await?;

        Ok(CapabilityResponse {
            output: serde_json::to_value(&issue)?,
            metadata: Default::default(),
        })
    }
}
```

**OAuth 认证**:

```rust
pub struct GitHubAuth {
    client_id: String,
    client_secret: String,
    token_cache: Arc<RwLock<Option<AccessToken>>>,
}

#[async_trait]
impl Authenticator for GitHubAuth {
    async fn authenticate(&self) -> Result<Credentials> {
        // 1. 检查缓存
        if let Some(token) = self.token_cache.read().await.as_ref() {
            if !token.is_expired() {
                return Ok(Credentials::Bearer(token.access_token.clone()));
            }
        }

        // 2. OAuth 流程
        let auth_url = format!(
            "https://github.com/login/oauth/authorize?client_id={}&scope=repo",
            self.client_id
        );

        // 3. 用户授权 (浏览器跳转或 device flow)
        // 4. 换取 access token
        // 5. 缓存 token

        todo!()
    }
}
```

#### 3.3.3 Database Connector (Agent 5)

**Capabilities**:
- `db.query`: 执行查询
- `db.execute`: 执行命令
- `db.transaction`: 事务执行
- `db.migrate`: 数据库迁移

**多数据库支持**:

```rust
pub struct DatabaseConnector {
    pool: Arc<DatabasePool>,
    db_type: DatabaseType,
}

pub enum DatabaseType {
    PostgreSQL,
    MySQL,
    SQLite,
}

pub enum DatabasePool {
    PostgreSQL(sqlx::PgPool),
    MySQL(sqlx::MySqlPool),
    SQLite(sqlx::SqlitePool),
}

impl DatabaseConnector {
    async fn query(&self, req: CapabilityRequest) -> Result<CapabilityResponse> {
        let sql = req.params["sql"].as_str()?;

        let rows = match &self.pool.as_ref() {
            DatabasePool::PostgreSQL(pool) => {
                sqlx::query(sql).fetch_all(pool).await?
            }
            DatabasePool::MySQL(pool) => {
                sqlx::query(sql).fetch_all(pool).await?
            }
            DatabasePool::SQLite(pool) => {
                sqlx::query(sql).fetch_all(pool).await?
            }
        };

        Ok(CapabilityResponse {
            output: serde_json::to_value(&rows)?,
            metadata: Default::default(),
        })
    }
}
```

#### 3.3.4 Slack Connector (Agent 6)

**Capabilities**:
- `slack.send_message`: 发送消息
- `slack.create_channel`: 创建频道
- `slack.upload_file`: 上传文件
- `slack.react_emoji`: 添加 emoji 反应

**Message 模板**:

```rust
pub struct SlackConnector {
    client: slack::WebClient,
    templates: Arc<MessageTemplates>,
}

pub struct MessageTemplates {
    templates: HashMap<String, HandlebarsTemplate>,
}

impl SlackConnector {
    async fn send_message(&self, req: CapabilityRequest) -> Result<CapabilityResponse> {
        let channel = req.params["channel"].as_str()?;
        let template_name = req.params["template"].as_str()?;
        let data = &req.params["data"];

        // 渲染模板
        let message = self.templates.render(template_name, data)?;

        // 发送消息
        let response = self.client
            .chat_post_message()
            .channel(channel)
            .text(&message)
            .send()
            .await?;

        Ok(CapabilityResponse {
            output: serde_json::to_value(&response)?,
            metadata: Default::default(),
        })
    }
}
```

---

### 3.4 Scheduler System (Agent 7-8)

#### 3.4.1 Heartbeat Monitor (Agent 7)

**职责**:
- 节点健康检查
- 资源使用率上报
- 异常检测与告警

**实现**:

```rust
// crates/cyberclaw-scheduler/src/heartbeat.rs

pub struct HeartbeatMonitor {
    /// 监控间隔
    interval: Duration,
    /// 节点注册表
    nodes: Arc<RwLock<HashMap<NodeId, NodeInfo>>>,
    /// 健康检查器
    health_checker: Arc<HealthChecker>,
    /// 异常检测器
    anomaly_detector: Arc<AnomalyDetector>,
}

#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub id: NodeId,
    pub last_heartbeat: Instant,
    pub cpu_usage: f64,
    pub memory_usage: f64,
    pub disk_usage: f64,
    pub status: NodeStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NodeStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Offline,
}

impl HeartbeatMonitor {
    /// 启动心跳监控
    pub async fn start(&self) -> Result<()> {
        let mut interval = tokio::time::interval(self.interval);

        loop {
            interval.tick().await;

            // 1. 收集所有节点心跳
            let nodes = self.nodes.read().await.clone();

            for (node_id, node_info) in nodes {
                // 2. 检查心跳超时
                if node_info.last_heartbeat.elapsed() > self.interval * 3 {
                    self.mark_node_offline(&node_id).await?;
                    continue;
                }

                // 3. 健康检查
                let health = self.health_checker.check(&node_info).await?;

                // 4. 异常检测
                if let Some(anomaly) = self.anomaly_detector.detect(&node_info).await? {
                    self.handle_anomaly(&node_id, anomaly).await?;
                }

                // 5. 更新状态
                self.update_node_status(&node_id, health).await?;
            }
        }
    }

    /// 处理异常
    async fn handle_anomaly(
        &self,
        node_id: &NodeId,
        anomaly: Anomaly,
    ) -> Result<()> {
        match anomaly {
            Anomaly::HighCpuUsage(usage) => {
                tracing::warn!("Node {} high CPU usage: {}%", node_id, usage);
                // 触发告警
            }
            Anomaly::HighMemoryUsage(usage) => {
                tracing::warn!("Node {} high memory usage: {}%", node_id, usage);
                // 触发告警
            }
            Anomaly::HeartbeatIrregular => {
                tracing::warn!("Node {} heartbeat irregular", node_id);
                // 触发告警
            }
        }
        Ok(())
    }
}
```

#### 3.4.2 Cron Scheduler (Agent 8)

**职责**:
- Cron 表达式解析
- 定时任务调度
- 执行历史记录

**实现**:

```rust
// crates/cyberclaw-scheduler/src/cron_scheduler.rs

use cron::Schedule;
use std::str::FromStr;

pub struct CronScheduler {
    /// 调度任务列表
    tasks: Arc<RwLock<HashMap<TaskId, ScheduledTask>>>,
    /// 任务执行器
    executor: Arc<TaskExecutor>,
    /// 执行历史
    history: Arc<RwLock<ExecutionHistory>>,
}

#[derive(Debug, Clone)]
pub struct ScheduledTask {
    pub id: TaskId,
    pub name: String,
    pub cron_expr: String,
    pub schedule: Schedule,
    pub action: TaskAction,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub enum TaskAction {
    /// 执行 Capability
    ExecuteCapability {
        connector_id: String,
        capability: String,
        params: serde_json::Value,
    },
    /// 执行 Agent
    ExecuteAgent {
        agent_id: String,
        task: String,
    },
}

impl CronScheduler {
    /// 添加定时任务
    pub async fn schedule(
        &self,
        name: String,
        cron_expr: String,
        action: TaskAction,
    ) -> Result<TaskId> {
        // 1. 解析 Cron 表达式
        let schedule = Schedule::from_str(&cron_expr)
            .map_err(|e| Error::InvalidCronExpression(e.to_string()))?;

        // 2. 创建任务
        let task_id = TaskId::new();
        let task = ScheduledTask {
            id: task_id.clone(),
            name,
            cron_expr,
            schedule,
            action,
            enabled: true,
        };

        // 3. 注册任务
        self.tasks.write().await.insert(task_id.clone(), task);

        Ok(task_id)
    }

    /// 启动调度器
    pub async fn start(&self) -> Result<()> {
        // 每分钟检查一次
        let mut interval = tokio::time::interval(Duration::from_secs(60));

        loop {
            interval.tick().await;

            let now = chrono::Utc::now();
            let tasks = self.tasks.read().await.clone();

            for (task_id, task) in tasks {
                if !task.enabled {
                    continue;
                }

                // 检查是否到了执行时间
                if let Some(next) = task.schedule.upcoming(chrono::Utc).next() {
                    if next.timestamp() <= now.timestamp() + 60 {
                        // 执行任务
                        self.execute_task(&task).await?;
                    }
                }
            }
        }
    }

    /// 执行任务
    async fn execute_task(&self, task: &ScheduledTask) -> Result<()> {
        let execution_id = ExecutionId::new();

        // 1. 转换为 Execution
        let execution = match &task.action {
            TaskAction::ExecuteCapability { connector_id, capability, params } => {
                Execution {
                    id: execution_id.clone(),
                    connector_id: connector_id.clone(),
                    capability: capability.clone(),
                    params: params.clone(),
                    // ...
                }
            }
            TaskAction::ExecuteAgent { agent_id, task: task_description } => {
                // Agent 执行逻辑
                todo!()
            }
        };

        // 2. 提交到 TaskExecutor (统一执行入口)
        let result = self.executor.execute(execution).await;

        // 3. 记录执行历史
        self.history.write().await.record(ExecutionRecord {
            task_id: task.id.clone(),
            execution_id,
            timestamp: chrono::Utc::now(),
            result: result.clone(),
        });

        result
    }
}
```

**Cron 表达式示例**:

```
# 每天凌晨 2 点执行数据库备份
0 2 * * * backup_database

# 每小时执行健康检查
0 * * * * health_check

# 每 5 分钟同步状态
*/5 * * * * sync_state

# 每周一上午 9 点发送报告
0 9 * * MON send_weekly_report
```

---

### 3.5 Skill Loader (Agent 9)

#### 3.5.1 多格式支持

**目标格式**:
1. **Claude Code Skill** (`.claude/skills/`)
2. **Codex Skill** (`.codex/skills/`)
3. **OpenClaw Skill** (`.openclaw/skills/`)

**统一 Skill 接口**:

```rust
// crates/cyberclaw-skill-runtime/src/skill.rs

#[async_trait]
pub trait Skill: Send + Sync {
    /// Skill ID
    fn id(&self) -> &str;

    /// Skill 元数据
    fn metadata(&self) -> &SkillMetadata;

    /// Skill 提供的 Capabilities
    fn capabilities(&self) -> Vec<CapabilityContract>;

    /// 执行 Skill
    async fn execute(
        &self,
        capability: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value>;
}

#[derive(Debug, Clone)]
pub struct SkillMetadata {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: Option<String>,
    pub homepage: Option<String>,
}
```

#### 3.5.2 Skill Loader 实现

```rust
// crates/cyberclaw-skill-runtime/src/loaders/mod.rs

pub struct UnifiedSkillLoader {
    loaders: Vec<Box<dyn FormatLoader>>,
    cache: Arc<RwLock<HashMap<String, Arc<dyn Skill>>>>,
}

#[async_trait]
pub trait FormatLoader: Send + Sync {
    /// 检测格式
    fn can_load(&self, path: &Path) -> bool;

    /// 加载 Skill
    async fn load(&self, path: &Path) -> Result<Arc<dyn Skill>>;
}

impl UnifiedSkillLoader {
    pub fn new() -> Self {
        Self {
            loaders: vec![
                Box::new(ClaudeCodeSkillLoader::new()),
                Box::new(CodexSkillLoader::new()),
                Box::new(OpenClawSkillLoader::new()),
            ],
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 自动检测并加载 Skill
    pub async fn load_skill(&self, path: &Path) -> Result<Arc<dyn Skill>> {
        // 1. 检查缓存
        let cache_key = path.to_string_lossy().to_string();
        if let Some(cached) = self.cache.read().await.get(&cache_key) {
            return Ok(cached.clone());
        }

        // 2. 尝试所有 Loader
        for loader in &self.loaders {
            if loader.can_load(path) {
                let skill = loader.load(path).await?;
                self.cache.write().await.insert(cache_key, skill.clone());
                return Ok(skill);
            }
        }

        Err(Error::UnsupportedSkillFormat)
    }
}
```

#### 3.5.3 Claude Code Skill Loader

```rust
// crates/cyberclaw-skill-runtime/src/loaders/claude_code.rs

pub struct ClaudeCodeSkillLoader;

impl FormatLoader for ClaudeCodeSkillLoader {
    fn can_load(&self, path: &Path) -> bool {
        // 检测 SKILL.md 文件
        path.join("SKILL.md").exists()
    }

    async fn load(&self, path: &Path) -> Result<Arc<dyn Skill>> {
        // 1. 读取 SKILL.md
        let skill_md = tokio::fs::read_to_string(path.join("SKILL.md")).await?;

        // 2. 解析 frontmatter
        let metadata = self.parse_frontmatter(&skill_md)?;

        // 3. 查找 scripts/
        let scripts_dir = path.join("scripts");
        let scripts = if scripts_dir.exists() {
            self.load_scripts(&scripts_dir).await?
        } else {
            vec![]
        };

        // 4. 构造 Skill
        Ok(Arc::new(ClaudeCodeSkill {
            path: path.to_path_buf(),
            metadata,
            scripts,
        }))
    }
}
```

**热重载支持**:

```rust
pub struct HotReloadWatcher {
    watcher: notify::RecommendedWatcher,
    skill_loader: Arc<UnifiedSkillLoader>,
}

impl HotReloadWatcher {
    pub async fn watch(&mut self, skill_dirs: Vec<PathBuf>) -> Result<()> {
        use notify::Watcher;

        for dir in skill_dirs {
            self.watcher.watch(&dir, notify::RecursiveMode::Recursive)?;
        }

        let (tx, mut rx) = tokio::sync::mpsc::channel(100);

        // 监听文件变化
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    notify::Event::Modify(_) | notify::Event::Create(_) => {
                        // 重新加载 Skill
                        self.skill_loader.reload_skill(&event.path).await;
                    }
                    _ => {}
                }
            }
        });

        Ok(())
    }
}
```

---

### 3.6 集成测试策略 (Agent 10)

#### 3.6.1 测试层级

```
E2E 测试 (End-to-End)
    ↓
集成测试 (Integration)
    ↓
单元测试 (Unit)
```

#### 3.6.2 测试用例设计

**Plugin 系统测试** (30+ tests):
- Plugin 加载/卸载
- Hook 分发与顺序
- FailurePolicy 验证
- 资源隔离
- 安全验证

**MCP Connector 测试** (20+ tests):
- 协议兼容性
- Tool/Resource 映射
- 错误处理
- 超时与重试

**External Connectors 测试** (每个 15+ tests):
- API 集成
- 认证流程
- 速率限制
- 幂等性

**Scheduler 测试** (25+ tests):
- Cron 解析
- 调度精度
- 并发执行
- 故障恢复

**Skill Loader 测试** (20+ tests):
- 格式检测
- 多格式加载
- 热重载
- 缓存一致性

**总计**: 150+ 测试用例

#### 3.6.3 测试工具链

```rust
// tests/common/mod.rs

/// 测试辅助函数
pub mod helpers {
    use tempfile::TempDir;

    /// 创建临时 Plugin
    pub async fn create_test_plugin(name: &str) -> (TempDir, PathBuf) {
        let temp_dir = TempDir::new().unwrap();
        let plugin_dir = temp_dir.path().join(name);
        tokio::fs::create_dir_all(&plugin_dir).await.unwrap();

        // 写入 manifest
        let manifest = format!(r#"
[plugin]
id = "{}"
name = "Test Plugin"
version = "0.1.0"
"#, name);

        tokio::fs::write(
            plugin_dir.join("cyberclaw-plugin.toml"),
            manifest,
        ).await.unwrap();

        (temp_dir, plugin_dir)
    }

    /// Mock MCP Server
    pub struct MockMcpServer {
        server: axum::Server,
    }

    impl MockMcpServer {
        pub async fn start() -> Self {
            // 启动 Mock JSON-RPC Server
            todo!()
        }
    }
}
```

---

## 4. 里程碑与验收标准

### 4.1 Milestone P2.1: Plugin Runtime (Week 2-3)

**交付物**:
- [x] PluginLoader + PluginRegistry
- [x] HookDispatcher + FailurePolicy
- [x] PluginSandbox 隔离执行
- [x] 完整的 Plugin Manifest 规范
- [x] 至少 30 个单元测试

**验收标准**:
- [ ] 可以动态加载/卸载 Plugin
- [ ] Hook 分发按优先级正确执行
- [ ] FailurePolicy 所有模式正常工作
- [ ] 资源隔离生效 (memory/cpu 限制)
- [ ] 0 编译警告

### 4.2 Milestone P2.2: MCP + External Connectors (Week 2-4)

**交付物**:
- [x] MCP Connector (完整协议支持)
- [x] GitHub Connector (5+ capabilities)
- [x] Database Connector (3 数据库支持)
- [x] Slack Connector (4+ capabilities)
- [x] 至少 65 个集成测试

**验收标准**:
- [ ] MCP 通过官方协议测试套件
- [ ] GitHub/Database/Slack 可实际调用外部 API
- [ ] 认证流程正常 (OAuth, API Key)
- [ ] 速率限制生效
- [ ] 错误处理健壮

### 4.3 Milestone P2.3: Scheduler + Skill Loader (Week 3-4)

**交付物**:
- [x] HeartbeatMonitor 完整实现
- [x] CronScheduler 完整实现
- [x] UnifiedSkillLoader (3 格式支持)
- [x] 热重载机制
- [x] 至少 45 个测试

**验收标准**:
- [ ] Heartbeat 正确检测节点状态
- [ ] Cron 调度精度 ± 1分钟
- [ ] 支持 Claude Code / Codex / OpenClaw 格式
- [ ] 热重载不影响运行中的任务
- [ ] 任务统一进入 Execution 主链路

### 4.4 Milestone P2.4: Integration & Documentation (Week 4-5)

**交付物**:
- [x] 完整的 E2E 测试套件
- [x] 开发者文档 (Plugin/Connector/Skill 开发指南)
- [x] 至少 3 个示例项目
- [x] API 参考文档
- [x] 性能基准测试

**验收标准**:
- [ ] 所有 E2E 测试通过
- [ ] 文档覆盖所有新增 API
- [ ] 示例项目可运行
- [ ] 基准测试达标

---

## 5. 技术选型

### 5.1 依赖库

| 功能 | Crate | 版本 | 用途 |
|------|-------|------|------|
| 动态加载 | `libloading` | 0.8 | Plugin 动态库加载 |
| JSON-RPC | `jsonrpc-core` | 18.0 | MCP 协议实现 |
| HTTP 客户端 | `reqwest` | 0.11 | 外部 API 调用 |
| GitHub API | `octocrab` | 0.32 | GitHub 集成 |
| 数据库 | `sqlx` | 0.7 | 多数据库支持 |
| Cron 解析 | `cron` | 0.12 | Cron 表达式解析 |
| 模板引擎 | `handlebars` | 4.5 | Slack 消息模板 |
| 文件监控 | `notify` | 6.0 | 热重载支持 |
| 速率限制 | `governor` | 0.6 | API 速率控制 |

### 5.2 架构模式

- **Plugin 系统**: Dynamic Library Loading
- **MCP Connector**: JSON-RPC over stdio/HTTP
- **External Connectors**: Adapter Pattern + Template Method
- **Scheduler**: Event-Driven Architecture
- **Skill Loader**: Strategy Pattern + Factory Pattern

---

## 6. 风险与缓解

### 6.1 技术风险

| 风险 | 影响 | 概率 | 缓解策略 |
|------|------|------|----------|
| MCP 协议不稳定 | 高 | 中 | 实现适配器层隔离变化 |
| Plugin 沙箱复杂度 | 高 | 高 | 优先 Process 隔离,Container 作为后备 |
| 多格式 Skill 兼容 | 中 | 中 | 定义统一接口,各格式独立 Loader |
| 性能瓶颈 | 中 | 低 | 提前基准测试,优化热路径 |

### 6.2 进度风险

| 风险 | 影响 | 概率 | 缓解策略 |
|------|------|------|----------|
| Agent 协调复杂 | 中 | 中 | 清晰定义模块边界,减少依赖 |
| 外部 API 变更 | 低 | 低 | 使用官方 SDK,版本锁定 |
| 测试覆盖不足 | 高 | 中 | 专门 Agent 负责测试,并行开发 |

---

## 7. 下一步行动

### 7.1 立即执行 (本周)

1. ✅ 创建 P2 开发分支: `feature/p2-extensibility-automation`
2. ✅ 创建项目目录结构
3. ✅ 启动 10-Agent 并行开发:
   - Agent 1-2: Platform Plugin
   - Agent 3: MCP Connector
   - Agent 4-6: External Connectors
   - Agent 7-8: Scheduler
   - Agent 9: Skill Loader
   - Agent 10: Integration & Docs

### 7.2 Week 1 目标

- [ ] 所有 Agent 完成架构设计文档
- [ ] 技术选型确认
- [ ] 初始目录结构创建
- [ ] 依赖库添加到 Cargo.toml

### 7.3 P2 完成定义

当以下条件全部满足时,P2 阶段完成:

- [ ] 所有功能标准达成
- [ ] 所有质量标准达成
- [ ] 所有文档标准达成
- [ ] 所有生态标准达成
- [ ] 通过 P2 验收测试

**完成后**: CyberClaw 进入 **Release Candidate** 阶段 🎉

---

**文档版本**: v1.0
**最后更新**: 2026-03-23
**批准状态**: 待批准
**下次评审**: Week 1 结束
