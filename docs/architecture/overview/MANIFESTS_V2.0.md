# CyberClaw Manifest / Schema 草案 v2.0

## 1. 目标

本文档定义 CyberClaw 四类生态对象的 `manifest` 与 `schema` 草案：

1. `Agent`
2. `Skill`
3. `Connector`
4. `Platform Plugin`

目标：

1. 为 Registry、Loader、Resolver 提供统一的元数据模型
2. 为生态对象安装、校验、签名、升级提供稳定约束
3. 保持 Skill 对 `Claude/Codex Skill` 的兼容性
4. 控制字段边界，避免把平台运行态信息塞进对象 manifest

---

## 2. 设计原则

### 2.1 一套包模型
四类对象统一使用**包级 manifest**，避免每种对象都有不同顶层格式。

### 2.2 manifest 只描述静态元数据
manifest 负责描述：

- 身份
- 版本
- 类型
- 兼容性
- 依赖
- 权限声明
- 能力声明
- 配置 schema

manifest 不负责描述：

- 运行时状态
- 执行结果
- Trace
- 审批记录
- 租户实例配置值
- 会话级记忆内容

### 2.3 Skill 保持原生兼容
Skill 的核心内容仍然是：

- `SKILL.md`
- `scripts/`
- `references/`
- `assets/`

CyberClaw 只在包根目录增加 sidecar `manifest.yaml`，不把平台私有字段塞进 `SKILL.md`。

### 2.4 Connector 以 Capability 为中心
Connector manifest 不以“厂商接入”作为治理单元，而以 `Capability` 作为治理、授权和审计单元。

### 2.5 Platform Plugin 只做平台扩展
Platform Plugin manifest 只能声明：

- 监听哪些平台事件
- 在哪些生命周期阶段执行
- 需要哪些平台 API 权限

不能承载：

- 业务角色定义
- 外部能力接入
- Skill 内容本身

---

## 3. 通用包模型

所有生态对象统一采用：

```yaml
apiVersion: cyberclaw.io/v2
kind: Agent | Skill | Connector | PlatformPlugin
id: string
version: semver
name: string
displayName: string
summary: string
owner: string
license: string
homepage: string
repository: string
tags: []
compatibility:
  platform: string
  runtime: []
  os: []
  arch: []
dependencies:
  agents: []
  skills: []
  connectors: []
  plugins: []
  packages: []
artifacts:
  entry: string
  files: []
configSchema: string
spec: {}
```

### 3.1 通用字段说明

| 字段 | 说明 | 必填 |
|---|---|---|
| `apiVersion` | manifest 版本 | 是 |
| `kind` | 对象类型 | 是 |
| `id` | 包唯一标识，建议小写短横线 | 是 |
| `version` | semver 版本 | 是 |
| `name` | 内部名称 | 是 |
| `displayName` | 展示名称 | 否 |
| `summary` | 简介 | 是 |
| `owner` | 发布者或组织 | 否 |
| `license` | 许可证 | 否 |
| `homepage` | 文档主页 | 否 |
| `repository` | 源码仓库 | 否 |
| `tags` | 标签 | 否 |
| `compatibility` | 平台兼容矩阵 | 是 |
| `dependencies` | 包依赖关系 | 否 |
| `artifacts.entry` | 主入口文件 | 否 |
| `artifacts.files` | 关键文件列表 | 否 |
| `configSchema` | 配置 JSON Schema 路径 | 否 |
| `spec` | 类型专属字段 | 是 |

### 3.2 `id` 规则

建议规则：

- 小写字母、数字、短横线
- 允许命名空间前缀
- 推荐格式：`org/name` 或 `name`

示例：

- `cyberclaw/master-agent`
- `github-reviewer`
- `xsoar-connector`
- `review-notifier`

---

## 4. 通用 Schema 草案

### 4.1 包级 manifest 顶层 JSON Schema

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://schemas.cyberclaw.io/package-manifest.v2.schema.json",
  "type": "object",
  "required": ["apiVersion", "kind", "id", "version", "name", "summary", "compatibility", "spec"],
  "properties": {
    "apiVersion": { "const": "cyberclaw.io/v2" },
    "kind": {
      "type": "string",
      "enum": ["Agent", "Skill", "Connector", "PlatformPlugin"]
    },
    "id": {
      "type": "string",
      "pattern": "^[a-z0-9][a-z0-9-/]*$"
    },
    "version": { "type": "string" },
    "name": { "type": "string", "minLength": 1 },
    "displayName": { "type": "string" },
    "summary": { "type": "string", "minLength": 1 },
    "owner": { "type": "string" },
    "license": { "type": "string" },
    "homepage": { "type": "string" },
    "repository": { "type": "string" },
    "tags": {
      "type": "array",
      "items": { "type": "string" },
      "uniqueItems": true
    },
    "compatibility": { "$ref": "#/$defs/compatibility" },
    "dependencies": { "$ref": "#/$defs/dependencies" },
    "artifacts": { "$ref": "#/$defs/artifacts" },
    "configSchema": { "type": "string" },
    "spec": { "type": "object" }
  },
  "$defs": {
    "compatibility": {
      "type": "object",
      "required": ["platform"],
      "properties": {
        "platform": { "type": "string" },
        "runtime": {
          "type": "array",
          "items": { "type": "string" }
        },
        "os": {
          "type": "array",
          "items": { "type": "string" }
        },
        "arch": {
          "type": "array",
          "items": { "type": "string" }
        }
      },
      "additionalProperties": false
    },
    "dependencies": {
      "type": "object",
      "properties": {
        "agents": { "$ref": "#/$defs/stringArray" },
        "skills": { "$ref": "#/$defs/stringArray" },
        "connectors": { "$ref": "#/$defs/stringArray" },
        "plugins": { "$ref": "#/$defs/stringArray" },
        "packages": { "$ref": "#/$defs/stringArray" }
      },
      "additionalProperties": false
    },
    "artifacts": {
      "type": "object",
      "properties": {
        "entry": { "type": "string" },
        "files": { "$ref": "#/$defs/stringArray" }
      },
      "additionalProperties": false
    },
    "stringArray": {
      "type": "array",
      "items": { "type": "string" },
      "uniqueItems": true
    }
  },
  "additionalProperties": false
}
```

---

## 5. Agent Manifest 草案

## 5.1 角色定位
Agent 是**声明式角色包**，不是任意代码运行时。

它回答：

- 这个角色是谁
- 默认如何工作
- 默认能访问哪些 Skill / Connector
- 默认使用什么 workspace / memory / isolation 策略

## 5.2 推荐文件结构

```text
ecosystem/agents/<agent-id>/
├── manifest.yaml
├── AGENT.md
├── PERSONA.md
├── POLICY.md
├── MEMORY.md
└── templates/
```

## 5.3 Agent manifest 示例

```yaml
apiVersion: cyberclaw.io/v2
kind: Agent
id: cyberclaw/grc-agent
version: 2.0.0
name: grc-agent
displayName: GRC Agent
summary: 面向治理、风险与合规工作的角色化 Agent
owner: cyberclaw
license: Apache-2.0
tags: [grc, compliance, audit]
compatibility:
  platform: ">=2.0.0 <3.0.0"
  runtime: [native]
artifacts:
  entry: AGENT.md
  files: [AGENT.md, PERSONA.md, POLICY.md, MEMORY.md]
configSchema: schemas/config.schema.json
spec:
  role: grc
  class: business
  description: 负责合规映射、控制检查、证据整理与报告协作
  personaFile: PERSONA.md
  policyFile: POLICY.md
  memoryFile: MEMORY.md
  systemPromptFile: AGENT.md
  defaultSkills:
    - compliance-mapping
    - evidence-collection
  defaultConnectors:
    - jira
    - confluence
    - github
  defaultWorkflow: grc-review
  spawnPolicy:
    canSpawn: true
    maxDepth: 2
    allowedChildren: [report-agent, evidence-agent]
  workspace:
    mode: isolated
    writableRoots: [workspace/, output/]
  memory:
    scope: case
    allowWriteBack: true
  governance:
    riskCeiling: medium
    approvalProfile: standard-review
  outputs:
    defaultSchema: schemas/report.output.json
```

## 5.4 Agent spec 字段草案

| 字段 | 说明 | 必填 |
|---|---|---|
| `spec.role` | 角色标识 | 是 |
| `spec.class` | `master` / `business` / `security-supervisor` / `subagent-template` | 是 |
| `spec.description` | 角色描述 | 是 |
| `spec.personaFile` | 人格文件 | 否 |
| `spec.policyFile` | 角色策略文件 | 否 |
| `spec.memoryFile` | 初始记忆文件 | 否 |
| `spec.systemPromptFile` | 角色主说明文件 | 是 |
| `spec.defaultSkills` | 默认 Skill | 否 |
| `spec.defaultConnectors` | 默认 Connector | 否 |
| `spec.defaultWorkflow` | 默认 workflow | 否 |
| `spec.spawnPolicy` | 派生 subagent 规则 | 否 |
| `spec.workspace` | workspace 策略 | 否 |
| `spec.memory` | memory 策略 | 否 |
| `spec.governance` | 治理约束 | 否 |
| `spec.outputs.defaultSchema` | 默认输出 schema | 否 |

## 5.5 Agent 不应承载的字段

不要在 Agent manifest 中放：

- 实时审批状态
- Trace ID
- 当前租户的密钥值
- 当前会话历史
- 实时工具调用记录
- LLM Provider 细节实现

---

## 6. Skill Manifest 草案

## 6.1 角色定位
Skill 是**标准技能包**，回答“怎么做”。

Skill 必须保持对 `Claude/Codex Skill` 的兼容性。CyberClaw 只增加 sidecar manifest，不改写 `SKILL.md` 协议。

## 6.2 推荐文件结构

### Loose Skill 形态

```text
ecosystem/skills/<skill-id>/
├── SKILL.md
├── scripts/
├── references/
└── assets/
```

### Packaged Skill 形态

```text
ecosystem/skills/<skill-id>/
├── manifest.yaml
├── SKILL.md
├── scripts/
├── references/
├── assets/
└── schemas/
```

说明：

- 本地加载可只依赖 `SKILL.md`
- 进入 Registry、Package、Trust 体系时，`manifest.yaml` 是必需的

## 6.3 Skill manifest 示例

```yaml
apiVersion: cyberclaw.io/v2
kind: Skill
id: cyberclaw/alert-triage
version: 2.0.0
name: alert-triage
displayName: Alert Triage
summary: 告警研判技能包
owner: cyberclaw
license: Apache-2.0
tags: [triage, alerts, investigation]
compatibility:
  platform: ">=2.0.0 <3.0.0"
artifacts:
  entry: SKILL.md
  files:
    - SKILL.md
    - scripts/
    - references/
    - assets/
configSchema: schemas/config.schema.json
spec:
  format: claude-compatible
  entryFile: SKILL.md
  scriptsDir: scripts
  referencesDir: references
  assetsDir: assets
  suggestedAgents:
    - alert-triage-agent
    - security-supervisor-agent
  requiredCapabilities:
    - siem.search
    - case.comment.append
  suggestedConnectors:
    - splunk
    - elastic
    - jira
  workflowTemplates:
    - workflows/alert-triage.yaml
  outputs:
    defaultSchema: schemas/triage.output.json
  reviewHints:
    recommendedReview: medium
```

## 6.4 Skill spec 字段草案

| 字段 | 说明 | 必填 |
|---|---|---|
| `spec.format` | 技能格式，建议 `claude-compatible` | 是 |
| `spec.entryFile` | 技能入口，默认 `SKILL.md` | 是 |
| `spec.scriptsDir` | 脚本目录 | 否 |
| `spec.referencesDir` | 参考资料目录 | 否 |
| `spec.assetsDir` | 资源目录 | 否 |
| `spec.suggestedAgents` | 建议搭配的 Agent | 否 |
| `spec.requiredCapabilities` | 所需 capability | 否 |
| `spec.suggestedConnectors` | 建议 connector | 否 |
| `spec.workflowTemplates` | workflow 模板 | 否 |
| `spec.outputs.defaultSchema` | 输出 schema | 否 |
| `spec.reviewHints.recommendedReview` | review 建议 | 否 |

## 6.5 Skill 不应承载的字段

不要在 Skill manifest 中放：

- 平台内部 Hook 逻辑
- 租户密钥
- 审批流规则本体
- 具体 connector 实例地址
- 实时执行状态
- 角色人格和 workspace 策略

---

## 7. Connector Manifest 草案

## 7.1 角色定位
Connector 是能力接入对象，回答“用什么做”。

Connector 的治理中心不是“厂商名”，而是 `Capability`。

## 7.2 推荐文件结构

```text
ecosystem/connectors/<connector-id>/
├── manifest.yaml
├── connector.yaml
├── schemas/
│   ├── config.schema.json
│   ├── capabilities/
│   │   ├── search.input.json
│   │   ├── search.output.json
│   │   ├── create_issue.input.json
│   │   └── create_issue.output.json
└── docs/
```

## 7.3 Connector manifest 示例

```yaml
apiVersion: cyberclaw.io/v2
kind: Connector
id: cyberclaw/github
version: 2.0.0
name: github
displayName: GitHub Connector
summary: 提供 GitHub 仓库、PR、Issue 和代码检索能力
owner: cyberclaw
license: Apache-2.0
tags: [github, scm, code]
compatibility:
  platform: ">=2.0.0 <3.0.0"
  runtime: [native, remote]
configSchema: schemas/config.schema.json
spec:
  subtype: repository
  runtime: native
  auth:
    modes: [oauth2, pat]
    secretRefs:
      - github.token
  capabilities:
    - id: repo.read_file
      title: 读取仓库文件
      inputSchema: schemas/capabilities/read_file.input.json
      outputSchema: schemas/capabilities/read_file.output.json
      risk: low
      effects: [read]
    - id: issue.create
      title: 创建 issue
      inputSchema: schemas/capabilities/create_issue.input.json
      outputSchema: schemas/capabilities/create_issue.output.json
      risk: medium
      effects: [write, ticket]
    - id: pr.comment
      title: 评论 Pull Request
      inputSchema: schemas/capabilities/pr_comment.input.json
      outputSchema: schemas/capabilities/pr_comment.output.json
      risk: medium
      effects: [write]
  network:
    outboundHosts:
      - api.github.com
      - github.com
  governance:
    defaultApprovalProfile: connector-default
  observability:
    emitRawRequestMetadata: true
```

## 7.4 Connector spec 字段草案

| 字段 | 说明 | 必填 |
|---|---|---|
| `spec.subtype` | `channel` / `repository` / `siem` / `soar` / `scanner` / `llm` / `remote-agent` 等 | 是 |
| `spec.runtime` | `native` / `remote` / `process` / `container` | 是 |
| `spec.auth.modes` | 支持的认证方式 | 否 |
| `spec.auth.secretRefs` | 所需 secret 引用名 | 否 |
| `spec.capabilities` | 能力列表 | 是 |
| `spec.network.outboundHosts` | 允许访问的外部域名 | 否 |
| `spec.governance.defaultApprovalProfile` | 默认审批配置 | 否 |
| `spec.observability.emitRawRequestMetadata` | 是否输出原始请求元数据 | 否 |

## 7.5 CapabilityContract JSON Schema 草案

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://schemas.cyberclaw.io/capability-contract.v2.schema.json",
  "type": "object",
  "required": ["id", "title", "inputSchema", "outputSchema", "risk", "effects"],
  "properties": {
    "id": { "type": "string" },
    "title": { "type": "string" },
    "description": { "type": "string" },
    "inputSchema": { "type": "string" },
    "outputSchema": { "type": "string" },
    "risk": {
      "type": "string",
      "enum": ["low", "medium", "high", "critical"]
    },
    "effects": {
      "type": "array",
      "items": { "type": "string" },
      "minItems": 1
    },
    "timeouts": {
      "type": "object",
      "properties": {
        "requestMs": { "type": "integer", "minimum": 1 }
      },
      "additionalProperties": false
    }
  },
  "additionalProperties": false
}
```

## 7.6 Connector 不应承载的字段

不要在 Connector manifest 中放：

- 业务角色定义
- 具体审批结果
- Skill 内容
- 会话级上下文
- 当前执行树状态

---

## 8. Platform Plugin Manifest 草案

## 8.1 角色定位
Platform Plugin 是平台插件，负责运行时增强。

它回答：

- 监听哪些事件
- 在哪些生命周期阶段运行
- 需要哪些平台 API 权限
- 失败时如何处理

## 8.2 推荐文件结构

```text
ecosystem/platform-plugins/<plugin-id>/
├── manifest.yaml
├── plugin.yaml
├── schemas/
│   └── config.schema.json
└── docs/
```

## 8.3 Platform Plugin manifest 示例

```yaml
apiVersion: cyberclaw.io/v2
kind: PlatformPlugin
id: cyberclaw/review-notifier
version: 2.0.0
name: review-notifier
displayName: Review Notifier
summary: 在 review 队列产生新项目时发送通知
owner: cyberclaw
license: Apache-2.0
tags: [review, notifications]
compatibility:
  platform: ">=2.0.0 <3.0.0"
  runtime: [native]
configSchema: schemas/config.schema.json
spec:
  runtime: native
  hooks:
    - event: review.requested
      phase: after
      handler: review_notifier.on_review_requested
    - event: approval.completed
      phase: after
      handler: review_notifier.on_approval_completed
  permissions:
    platformApi:
      - review.read
      - notification.send
      - trace.read
    network:
      - webhook.company.internal
  failurePolicy:
    onError: continue
    emitSecurityEvent: true
  ordering:
    priority: 100
  observability:
    emitPluginTrace: true
```

## 8.4 Platform Plugin spec 字段草案

| 字段 | 说明 | 必填 |
|---|---|---|
| `spec.runtime` | 插件运行模式 | 是 |
| `spec.hooks` | 事件 hook 列表 | 是 |
| `spec.permissions.platformApi` | 可访问的平台 API 能力 | 否 |
| `spec.permissions.network` | 可访问的网络目标 | 否 |
| `spec.failurePolicy.onError` | `continue` / `fail` / `disable` | 是 |
| `spec.failurePolicy.emitSecurityEvent` | 是否产生安全事件 | 否 |
| `spec.ordering.priority` | 执行优先级 | 否 |
| `spec.observability.emitPluginTrace` | 是否输出插件 trace | 否 |

## 8.5 HookContract JSON Schema 草案

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://schemas.cyberclaw.io/platform-hook.v2.schema.json",
  "type": "object",
  "required": ["event", "phase", "handler"],
  "properties": {
    "event": { "type": "string" },
    "phase": {
      "type": "string",
      "enum": ["before", "after", "around"]
    },
    "handler": { "type": "string" }
  },
  "additionalProperties": false
}
```

## 8.6 Platform Plugin 不应承载的字段

不要在 Platform Plugin manifest 中放：

- 业务 Agent 角色定义
- Skill 内容或 prompt 正文
- 外部 connector capability 本体
- 审批决策状态
- 会话级 memory 数据

---

## 9. 四类对象边界总表

| 对象 | 回答的问题 | 主入口文件 | 典型职责 | 不该承载 |
|---|---|---|---|---|
| `Agent` | 谁来做 | `manifest.yaml` + `AGENT.md` | 角色主体、默认 skill/connectors、workspace/memory 策略 | 运行时状态、审批结果、trace |
| `Skill` | 怎么做 | `SKILL.md` + `manifest.yaml` | 方法、知识、模板、playbook | 平台 hooks、凭证、审批规则本体 |
| `Connector` | 用什么做 | `manifest.yaml` | capability 接入、认证、外部系统适配 | 业务角色、会话上下文 |
| `Platform Plugin` | 平台怎么增强 | `manifest.yaml` | hooks、平台增强、审计增强、自动化挂点 | 业务知识、外部 capability 本体 |

---

## 10. Loader / Registry 处理规则

### 10.1 Agent Loader

- 必须存在 `manifest.yaml`
- 必须至少存在 `AGENT.md`
- 可选加载 `PERSONA.md`、`POLICY.md`、`MEMORY.md`

### 10.2 Skill Loader

- 如果仅本地使用，可接受只有 `SKILL.md`
- 如果进入 Registry / Package / Trust 体系，必须存在 `manifest.yaml`
- `manifest.yaml` 只描述元数据，不复制 `SKILL.md` 内容

### 10.3 Connector Loader

- 必须存在 `manifest.yaml`
- 必须有 `spec.capabilities`
- 每个 capability 必须指向有效的输入输出 schema

### 10.4 Platform Plugin Loader

- 必须存在 `manifest.yaml`
- 必须声明 `hooks`
- 必须声明 `failurePolicy`

---

## 11. 推荐文件命名

统一建议：

- 包级入口：`manifest.yaml`
- 配置 schema：`schemas/config.schema.json`
- 输出 schema：`schemas/*.output.json`
- 输入 schema：`schemas/*.input.json`

说明：

- 不建议每种对象使用不同 manifest 文件名
- 统一使用 `manifest.yaml`，由 `kind` 区分类型
- 这样 Registry、Package Manager、Trust Scanner 只需要一套解析逻辑

---

## 12. 推荐的最小校验顺序

Registry / Loader 应按以下顺序校验：

1. 顶层包级 schema 校验
2. `kind` 对应的 `spec` schema 校验
3. 文件存在性校验
4. 依赖对象存在性校验
5. schema 文件可解析性校验
6. trust / signature 校验
7. compatibility 校验

---

## 13. 一句话结论

> **CyberClaw 采用统一的包级 manifest 模型：四类生态对象共享同一顶层结构，并通过 `kind + spec` 分化类型语义。Skill 保持 `Claude/Codex Skill` 原生兼容；Connector 以 Capability 为中心；Agent 表示声明式角色包；Platform Plugin 表示平台插件。**
