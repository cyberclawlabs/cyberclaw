# 生态系统架构

**最后更新:** 2024-03-18
**目录:** `ecosystem/`, `crates/cyberclaw-core/src/manifests.rs`
**状态:** ✅ 类型定义完成，包实例规划中

## 生态系统概览

```
CyberClaw Ecosystem
├── Agents (谁来做)
│   └── 角色主体、行为边界、风险上限
│
├── Skills (怎么做)
│   └── 方法、知识、模板、playbook
│
├── Connectors (用什么做)
│   └── 外部系统、API、模型、渠道
│
└── Platform Plugins (平台怎么增强)
    └── 事件监听、拦截器、审计增强
```

## 包类型系统

**文件:** `crates/cyberclaw-core/src/manifests.rs`

### PackageKind (包类型)

```rust
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PackageKind {
    Agent,           // 智能体包
    Skill,           // 技能包
    Connector,       // 连接器包
    PlatformPlugin,  // 平台插件包
}
```

### 通用 Manifest 结构

```rust
pub struct PackageManifest {
    pub api_version: String,      // API 版本 (v1)
    pub kind: PackageKind,         // 包类型
    pub id: String,                // 唯一标识符
    pub version: String,           // 语义化版本
    pub name: String,              // 包名
    pub display_name: Option<String>,
    pub summary: String,           // 简短描述
    pub owner: Option<String>,     // 所有者
    pub license: Option<String>,   // 许可证
    pub homepage: Option<String>,  // 主页
    pub repository: Option<String>,// 仓库地址
    pub tags: Vec<String>,         // 标签
    pub compatibility: Compatibility,
    pub dependencies: Dependencies,
    pub artifacts: Artifacts,
    pub config_schema: Option<String>,
    pub spec: PackageSpec,         // 类型特定规范
}
```

## 1. Agent 包

### Agent Class (角色类别)

```rust
pub enum AgentClass {
    Master,              // 主控 Agent
    Business,            // 业务 Agent
    SecuritySupervisor,  // 安全监督 Agent
    SubagentTemplate,    // 子代理模板
}
```

### AgentSpec

```rust
pub struct AgentSpec {
    pub role: String,                      // 角色描述
    pub class: AgentClass,                 // 角色类别
    pub description: String,               // 详细描述
    pub persona_file: Option<String>,      // 人设文件
    pub policy_file: Option<String>,       // 策略文件
    pub memory_file: Option<String>,       // 记忆文件
    pub system_prompt_file: String,        // 系统提示词
    pub default_skills: Vec<String>,       // 默认技能
    pub default_connectors: Vec<String>,   // 默认连接器
    pub default_workflow: Option<String>,  // 默认工作流
    pub spawn_policy: SpawnPolicy,         // 生成策略
}

pub struct SpawnPolicy {
    pub can_spawn: bool,               // 是否可生成子代理
    pub max_depth: Option<u32>,        // 最大深度
    pub allowed_children: Vec<String>, // 允许的子类型
}
```

### Agent Manifest 示例

```yaml
apiVersion: v1
kind: Agent
metadata:
  id: security-scanner
  version: 1.0.0
  name: Security Scanner Agent
  displayName: Security Scanner
  summary: Automated security scanning agent for code and infrastructure

owner: security-team@example.com
license: Apache-2.0
tags:
  - security
  - scanning
  - appsec

compatibility:
  platform: cyberclaw-v2
  runtime:
    - rust-1.75
  os:
    - linux
    - macos

dependencies:
  skills:
    - static-analysis
    - dependency-audit
  connectors:
    - github
    - slack

artifacts:
  entry: main.rs
  files:
    - persona.md
    - policy.yaml
    - system-prompt.md

spec:
  role: Security Scanner
  class: business
  description: Performs automated security scans on code repositories
  personaFile: persona.md
  policyFile: policy.yaml
  systemPromptFile: system-prompt.md

  defaultSkills:
    - static-analysis
    - dependency-audit
    - report-summary

  defaultConnectors:
    - github
    - slack

  spawnPolicy:
    canSpawn: false
    maxDepth: 0
```

## 2. Skill 包

### SkillSpec

```rust
pub struct SkillSpec {
    pub format: SkillFormat,              // 格式 (claude-compatible)
    pub entry_file: String,               // 入口文件
    pub scripts_dir: Option<String>,      // 脚本目录
    pub references_dir: Option<String>,   // 参考资料目录
    pub assets_dir: Option<String>,       // 资源目录
    pub suggested_agents: Vec<String>,    // 建议使用的 Agent
    pub required_capabilities: Vec<String>, // 必需的能力
    pub suggested_connectors: Vec<String>,  // 建议的 Connector
    pub workflow_templates: Vec<String>,    // 工作流模板
}

pub enum SkillFormat {
    ClaudeCompatible,  // Claude 兼容格式
}
```

### Skill Manifest 示例

```yaml
apiVersion: v1
kind: Skill
metadata:
  id: static-analysis
  version: 1.0.0
  name: Static Analysis Skill
  summary: Performs static code analysis for security vulnerabilities

owner: security-team@example.com
license: Apache-2.0
tags:
  - security
  - static-analysis
  - code-quality

compatibility:
  platform: cyberclaw-v2

dependencies:
  connectors:
    - semgrep
    - codeql

artifacts:
  entry: skill.md
  files:
    - scripts/run-analysis.sh
    - references/cwe-patterns.json

spec:
  format: claude-compatible
  entryFile: skill.md
  scriptsDir: scripts
  referencesDir: references

  suggestedAgents:
    - security-scanner
    - code-reviewer

  requiredCapabilities:
    - file-read
    - process-spawn

  suggestedConnectors:
    - github
    - semgrep
```

## 3. Connector 包

### ConnectorSpec

```rust
pub struct ConnectorSpec {
    pub subtype: String,                  // 子类型 (api, database, mq)
    pub runtime: ConnectorRuntime,        // 运行时模式
    pub auth: Option<ConnectorAuth>,      // 认证配置
    pub capabilities: Vec<CapabilityContract>, // 能力列表
}

pub enum ConnectorRuntime {
    Native,    // 进程内
    Remote,    // 远程 HTTP/gRPC
    Process,   // 子进程
    Container, // 容器化
}

pub struct ConnectorAuth {
    pub modes: Vec<String>,         // 认证模式
    pub secret_refs: Vec<String>,   // 密钥引用
}

pub struct CapabilityContract {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub input_schema: String,       // JSON Schema
    pub output_schema: String,      // JSON Schema
    pub risk: RiskLevel,            // 风险等级
    pub effects: Vec<CapabilityEffect>,
    pub placement: Option<CapabilityPlacement>,
    pub timeouts: CapabilityTimeouts,
}
```

### Connector Manifest 示例

```yaml
apiVersion: v1
kind: Connector
metadata:
  id: github-connector
  version: 1.0.0
  name: GitHub API Connector
  summary: Integrates with GitHub API for repository operations

owner: platform-team@example.com
license: Apache-2.0
tags:
  - github
  - git
  - version-control

compatibility:
  platform: cyberclaw-v2
  runtime:
    - rust-1.75

artifacts:
  entry: connector.rs
  files:
    - schemas/create-issue.json
    - schemas/list-prs.json

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
      description: Creates a new issue in a repository
      inputSchema: schemas/create-issue.json
      outputSchema: schemas/issue-output.json
      risk: medium
      effects:
        - external-write
      timeouts:
        requestMs: 5000

    - id: list-pull-requests
      title: List Pull Requests
      inputSchema: schemas/list-prs.json
      outputSchema: schemas/pr-list-output.json
      risk: low
      effects:
        - external-read
      timeouts:
        requestMs: 3000
```

## 4. Platform Plugin 包

### PlatformPluginSpec

```rust
pub struct PlatformPluginSpec {
    pub runtime: String,                  // 运行时类型
    pub hooks: Vec<PlatformHook>,         // 钩子列表
    pub permissions: BTreeMap<String, serde_json::Value>,
    pub failure_policy: PlatformPluginFailurePolicy,
}

pub struct PlatformHook {
    pub event: String,      // 事件名称
    pub phase: HookPhase,   // 钩子阶段
    pub handler: String,    // 处理器函数
}

pub enum HookPhase {
    Before,  // 事件前
    After,   // 事件后
    Around,  // 环绕
}

pub struct PlatformPluginFailurePolicy {
    pub on_error: PluginErrorPolicy,   // 错误策略
    pub emit_security_event: bool,     // 是否发出安全事件
}

pub enum PluginErrorPolicy {
    Continue,  // 继续执行
    Fail,      // 失败
    Disable,   // 禁用插件
}
```

### Platform Plugin Manifest 示例

```yaml
apiVersion: v1
kind: PlatformPlugin
metadata:
  id: audit-enricher
  version: 1.0.0
  name: Audit Enrichment Plugin
  summary: Enriches audit logs with additional context

owner: security-team@example.com
license: Apache-2.0
tags:
  - audit
  - security
  - compliance

compatibility:
  platform: cyberclaw-v2

artifacts:
  entry: plugin.rs

spec:
  runtime: native

  hooks:
    - event: task.created
      phase: after
      handler: enrich_task_audit

    - event: capability.invoked
      phase: around
      handler: log_capability_use

    - event: approval.requested
      phase: before
      handler: validate_approval

  permissions:
    audit.read: true
    audit.write: true
    state.read: true

  failurePolicy:
    onError: continue
    emitSecurityEvent: true
```

## 包依赖关系

```
┌─────────────────────────────────────────────┐
│              Agent Package                   │
│  • role, class, spawn_policy                │
│  • default_skills: [skill-1, skill-2]       │
│  • default_connectors: [conn-1]             │
└──────────┬───────────────┬──────────────────┘
           │               │
           ▼               ▼
┌──────────────────┐  ┌──────────────────┐
│  Skill Package   │  │Connector Package │
│  • format        │  │  • runtime       │
│  • entry_file    │  │  • capabilities  │
│  • required_caps │  │  • auth          │
└──────────────────┘  └──────────────────┘
           │
           ▼
┌──────────────────────────────────────────────┐
│       Platform Plugin Package                │
│  • hooks (before/after/around)               │
│  • permissions                               │
└──────────────────────────────────────────────┘
```

## 现有生态包

### Agents

```
ecosystem/agents/
├── master-agent/
│   └── manifest.yaml
└── report-agent/
    └── manifest.yaml
```

### Skills

```
ecosystem/skills/
└── report-summary/
    └── manifest.yaml
```

### Connectors

```
ecosystem/connectors/
└── github/
    └── manifest.yaml
```

### Platform Plugins

```
ecosystem/platform-plugins/
└── audit-enricher/
    └── manifest.yaml
```

## 包加载流程

```
启动 → EcosystemScanner
         │
         ├→ 扫描 ecosystem/ 目录
         │   ├→ agents/
         │   ├→ skills/
         │   ├→ connectors/
         │   └→ platform-plugins/
         │
         ├→ ManifestLoader
         │   ├→ 读取 manifest.yaml
         │   ├→ 验证 schema
         │   ├→ 解析 spec
         │   └→ 验证依赖
         │
         └→ Registry
             ├→ 注册 Agent
             ├→ 注册 Skill
             ├→ 注册 Connector
             └→ 注册 Platform Plugin
```

## 依赖解析

```
Resolver 接收 Task
    │
    ├→ 选择 Agent (基于 role, capability)
    │   ├→ 加载 AgentSpec
    │   └→ 获取 default_skills, default_connectors
    │
    ├→ 解析 Skills
    │   ├→ 从 Agent.default_skills
    │   ├→ 检查 required_capabilities
    │   └→ 验证 Skill 可用性
    │
    ├→ 解析 Connectors
    │   ├→ 从 Agent.default_connectors
    │   ├→ 从 Skill.suggested_connectors
    │   ├→ 验证 auth
    │   └→ 检查 capabilities
    │
    └→ 加载 Platform Plugins
        ├→ 匹配 hooks
        └→ 设置 interceptors
```

## 版本管理

### 语义化版本

```
version: "MAJOR.MINOR.PATCH"

MAJOR: 破坏性变更 (API 不兼容)
MINOR: 新增功能 (向后兼容)
PATCH: Bug 修复 (向后兼容)
```

### 兼容性检查

```rust
pub struct Compatibility {
    pub platform: String,      // "cyberclaw-v2"
    pub runtime: Vec<String>,  // ["rust-1.75", "python-3.11"]
    pub os: Vec<String>,       // ["linux", "macos"]
    pub arch: Vec<String>,     // ["x86_64", "aarch64"]
}
```

## 安全考量

### Manifest 验证

```
加载时验证：
├── Schema 合规性
├── 文件路径安全 (路径遍历)
├── 依赖完整性
├── 签名验证 (未来)
└── 大小限制 (1MB)
```

### 运行时隔离

```
Agent Runtime:
├── 工作区隔离
├── 权限边界
└── 资源配额

Skill Execution:
├── 沙箱执行
├── 脚本限制
└── 超时控制

Connector:
├── 认证验证
├── 速率限制
└── 审计日志
```

## 生态系统扩展

### 发布流程（规划）

```
1. 开发包 (本地 ecosystem/)
2. 验证 manifest.yaml
3. 测试集成
4. 打包 (tar.gz)
5. 发布到 Registry (未来: 中心化包仓库)
6. 安装到目标环境
```

### 包发现（规划）

```
cyberclaw search "security scanner"
cyberclaw install security-scanner@1.0.0
cyberclaw list --installed
cyberclaw update security-scanner
```

## 相关文档

- [控制平面](./control-plane.md) - Registry, Resolver, Scanner
- [运行时层](./runtime-layers.md) - Agent, Skill, Connector 执行
- [核心引擎](./core.md) - 核心类型定义
- [项目总览](./INDEX.md) - 整体架构

---

**维护说明:** Manifest 类型已完成，包实例和生态系统工具链规划中。
