# 运行时层架构

**最后更新:** 2024-03-18
**包路径:** `crates/cyberclaw-{agent,skill,workflow,connectors}-runtime/`
**状态:** 🚧 规划中

## 模块概览

```
Runtime Layers
├── Agent Runtime    - Agent 执行引擎
├── Skill Runtime    - Skill 加载与执行
├── Workflow Engine  - 工作流编排
└── Connectors      - 外部系统集成
```

## 1. Agent Runtime (智能体运行时)

**包路径:** `crates/cyberclaw-agent-runtime/`
**状态:** 脚手架阶段

### 设计目标

```
功能职责：
├── Agent 生命周期管理
│   ├── 初始化 (persona, policy, memory 加载)
│   ├── 会话管理
│   └── 状态持久化
│
├── 上下文管理
│   ├── 工作区隔离
│   ├── 工具访问控制
│   └── 内存边界
│
├── 子代理生成
│   ├── 深度控制 (max_depth)
│   ├── 预算验证 (steps, tokens, duration)
│   └── 亲子关系追踪
│
└── 集成点
    ├── Skill 加载
    ├── Connector 调用
    └── Workflow 执行
```

### 关键类型（规划）

```rust
pub struct AgentContext {
    pub agent_id: String,
    pub role: String,
    pub class: AgentClass,
    pub workspace: WorkspaceRef,
    pub available_skills: Vec<SkillRef>,
    pub available_connectors: Vec<ConnectorRef>,
    pub spawn_policy: SpawnPolicy,
}

pub struct AgentSession {
    pub session_id: String,
    pub agent_context: AgentContext,
    pub memory: SessionMemory,
    pub execution_tree: ExecutionTree,
}

pub trait AgentRuntime {
    async fn initialize(&mut self, spec: &AgentSpec) -> Result<AgentContext>;
    async fn execute(&self, task: Task) -> Result<ExecutionResult>;
    async fn spawn_child(&self, template: &str, task: Task) -> Result<AgentSession>;
}
```

### 数据流

```
Task → AgentRuntime
         │
         ├→ 加载 AgentSpec (manifest.yaml)
         ├→ 初始化 Context (persona, policy)
         ├→ 加载 Skills/Connectors
         │
         ├→ 执行循环
         │   ├→ LLM 调用
         │   ├→ Skill 执行
         │   ├→ Connector 调用
         │   └→ 子代理生成
         │
         └→ 结果 + Artifacts
```

## 2. Skill Runtime (技能运行时)

**包路径:** `crates/cyberclaw-skill-runtime/`
**状态:** 脚手架阶段

### 设计目标

```
功能职责：
├── Skill 加载
│   ├── 格式验证 (claude-compatible)
│   ├── 依赖解析
│   └── 沙箱准备
│
├── 执行引擎
│   ├── 提示词注入
│   ├── 脚本执行
│   └── 资源隔离
│
└── 兼容性
    ├── Claude Skills 格式
    ├── Codex Skills 格式 (未来)
    └── 自定义格式扩展
```

### Skill 格式 (Claude 兼容)

```markdown
---
name: example-skill
description: Example skill description
---

# Skill Content

<skill-content>
Prompt content that gets injected into agent context.
Can reference {variables} and include logic.
</skill-content>

<skill-scripts>
Optional bash/python scripts for automation.
</skill-scripts>
```

### 关键类型（规划）

```rust
pub struct SkillContext {
    pub skill_id: String,
    pub format: SkillFormat,
    pub entry_content: String,
    pub scripts: HashMap<String, Script>,
    pub references: HashMap<String, String>,
}

pub trait SkillRuntime {
    async fn load_skill(&self, spec: &SkillSpec) -> Result<SkillContext>;
    async fn inject_prompt(&self, context: &SkillContext) -> Result<String>;
    async fn execute_script(&self, script: &Script) -> Result<ScriptOutput>;
}
```

## 3. Workflow Engine (工作流引擎)

**包路径:** `crates/cyberclaw-workflow/`
**状态:** 脚手架阶段

### 设计目标

```
功能职责：
├── 工作流定义
│   ├── YAML 格式
│   ├── 步骤编排
│   └── 条件分支
│
├── 执行引擎
│   ├── 步骤调度
│   ├── 状态机管理
│   ├── 错误恢复
│   └── 并发控制
│
└── 集成能力
    ├── Agent 调用
    ├── Skill 执行
    ├── Connector 触发
    └── 审批流程
```

### 工作流定义示例

```yaml
apiVersion: v1
kind: Workflow
metadata:
  id: security-scan-workflow
  name: Security Scanning Workflow

steps:
  - id: code-scan
    type: agent
    agent: security-scanner
    skills:
      - static-analysis
      - dependency-audit

  - id: review
    type: approval
    requires: code-scan
    reviewers:
      - security-team

  - id: report
    type: skill
    requires: review
    skill: report-summary
    connectors:
      - slack
      - jira
```

### 关键类型（规划）

```rust
pub struct WorkflowDefinition {
    pub id: String,
    pub steps: Vec<WorkflowStep>,
    pub triggers: Vec<Trigger>,
}

pub enum WorkflowStep {
    AgentTask(AgentTask),
    SkillExecution(SkillExecution),
    Approval(ApprovalStep),
    Parallel(Vec<WorkflowStep>),
}

pub trait WorkflowEngine {
    async fn execute(&self, workflow: &WorkflowDefinition) -> Result<WorkflowResult>;
    async fn pause(&self, workflow_id: &str) -> Result<()>;
    async fn resume(&self, workflow_id: &str) -> Result<()>;
}
```

## 4. Connectors (连接器层)

**包路径:** `crates/cyberclaw-connectors/`
**状态:** 脚手架阶段

### 设计目标

```
功能职责：
├── 连接器类型
│   ├── API 连接器 (HTTP/REST/GraphQL)
│   ├── 数据库连接器 (PostgreSQL, MongoDB)
│   ├── 消息队列 (Kafka, RabbitMQ)
│   ├── 协作工具 (Slack, GitHub, Jira)
│   └── AI 模型 (OpenAI, Anthropic, 本地模型)
│
├── 运行时模式
│   ├── Native   - 进程内调用
│   ├── Remote   - HTTP/gRPC 远程调用
│   ├── Process  - 子进程执行
│   └── Container - 容器化隔离
│
└── 安全机制
    ├── 认证 (API Keys, OAuth, mTLS)
    ├── 授权 (权限检查)
    ├── 速率限制
    └── 审计日志
```

### Connector 示例（GitHub）

```yaml
apiVersion: v1
kind: Connector
metadata:
  id: github-connector
  version: 1.0.0
  name: GitHub API Connector

spec:
  subtype: git-platform
  runtime: remote

  auth:
    modes:
      - personal-access-token
      - oauth-app
      - github-app
    secretRefs:
      - github-token

  capabilities:
    - id: create-issue
      title: Create GitHub Issue
      inputSchema: schemas/create-issue-input.json
      outputSchema: schemas/issue-output.json
      risk: medium
      effects:
        - external-write

    - id: list-prs
      title: List Pull Requests
      inputSchema: schemas/list-prs-input.json
      outputSchema: schemas/pr-list-output.json
      risk: low
      effects:
        - external-read
```

### 关键类型（规划）

```rust
pub struct ConnectorContext {
    pub connector_id: String,
    pub runtime: ConnectorRuntime,
    pub auth: Option<AuthContext>,
    pub capabilities: Vec<CapabilityContract>,
}

pub trait Connector {
    async fn initialize(&mut self, config: ConnectorConfig) -> Result<()>;
    async fn invoke(&self, capability: &str, input: Value) -> Result<Value>;
    async fn health_check(&self) -> Result<HealthStatus>;
}

pub struct ConnectorRegistry {
    connectors: HashMap<String, Box<dyn Connector>>,
}
```

## 运行时层集成

```
┌─────────────────────────────────────────────────┐
│              Control Plane                       │
│  • Resolver → 选择 Agent + Skills + Connectors  │
└────────────────┬────────────────────────────────┘
                 │
┌────────────────▼────────────────────────────────┐
│           Agent Runtime                          │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐      │
│  │  Agent   │  │  Skill   │  │Workflow  │      │
│  │  Context │→ │ Executor │→ │  Engine  │      │
│  └──────────┘  └──────────┘  └──────────┘      │
│        │              │              │           │
│        └──────────────┼──────────────┘           │
│                       │                          │
└───────────────────────┼──────────────────────────┘
                        │
┌───────────────────────▼──────────────────────────┐
│           Connector Layer                        │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐      │
│  │   API    │  │   DB     │  │  Message │      │
│  │Connectors│  │Connectors│  │  Queue   │      │
│  └──────────┘  └──────────┘  └──────────┘      │
└─────────────────────────────────────────────────┘
```

## 执行流程示例

```
用户请求: "扫描代码并创建安全报告"
    │
    ▼
Control Plane: Resolver
    ├→ 选择 Agent: security-scanner
    ├→ 加载 Skills: static-analysis, report-summary
    └→ 加载 Connectors: github, slack
    │
    ▼
Agent Runtime: 初始化
    ├→ 加载 AgentSpec (persona, policy)
    ├→ 创建 AgentContext
    └→ 注入 Skills 到提示词
    │
    ▼
Skill Runtime: 执行 static-analysis
    ├→ 扫描代码
    ├→ 生成发现列表
    └→ 返回结果
    │
    ▼
Workflow Engine: 审批流程
    ├→ 创建 Review Request
    ├→ 等待审批
    └→ 继续执行
    │
    ▼
Skill Runtime: 执行 report-summary
    ├→ 生成报告
    └→ 格式化输出
    │
    ▼
Connector: GitHub + Slack
    ├→ 创建 Issue (github-connector)
    └→ 发送通知 (slack-connector)
```

## 性能考量

### Agent Runtime
- **并发:** 支持多个 Agent 会话并行
- **内存:** 每个 Agent 独立上下文，控制内存使用
- **超时:** 配置化超时 (max_duration_ms)

### Skill Runtime
- **加载:** 延迟加载，按需初始化
- **缓存:** 已加载 Skill 内容缓存
- **沙箱:** 脚本执行隔离

### Workflow Engine
- **持久化:** 工作流状态持久化到 Store
- **恢复:** 支持中断恢复
- **并发:** 并行步骤执行

### Connectors
- **连接池:** 复用 HTTP 连接
- **重试:** 指数退避重试
- **限流:** 客户端速率限制

## 测试策略

### 单元测试
- Agent 初始化和上下文管理
- Skill 加载和注入
- Workflow 步骤调度
- Connector 调用和错误处理

### 集成测试
- Agent → Skill → Connector 完整流程
- Workflow 多步骤编排
- 子代理生成和深度控制
- 并发 Agent 执行

### 端到端测试
- 真实场景工作流
- 外部系统集成
- 失败恢复测试

## 未来扩展

### v2.1 规划
- [ ] Agent Runtime MVP
- [ ] Skill Runtime (Claude 格式)
- [ ] 基础 Connector (HTTP, File)

### v2.2 规划
- [ ] Workflow Engine
- [ ] 更多 Connector (GitHub, Slack)
- [ ] 子代理生成

### v2.3 规划
- [ ] 高级工作流 (条件、循环)
- [ ] Connector 容器化运行
- [ ] 性能优化

## 相关文档

- [控制平面](./control-plane.md) - Agent/Skill/Connector 解析
- [核心引擎](./core.md) - ExecutionId, Task 定义
- [生态系统](./ecosystem.md) - Manifest 格式和包管理
- [治理层](./governance.md) - 权限和策略执行

---

**维护说明:** 运行时层目前处于脚手架阶段，本文档描述设计目标和规划。
