# CyberClaw Workspace 与 Crate 设计 v2.0

## 1. 目标

本设计定义 CyberClaw 的 workspace 与 crate 组织方式，目标是：

1. 明确平台代码、入口程序、生态内容的边界
2. 把控制平面提升为一级内核模块
3. 避免生态对象和平台内核源码混杂
4. 为后续 Agent / Skill / Connector / Platform Plugin 生态扩展预留稳定结构

---

## 2. 顶层目录结构

推荐使用三分结构：

```text
cyberclaw/
├── apps/
├── crates/
├── ecosystem/
├── workflows/
├── policies/
├── state/
└── docs/
```

### `apps/`
运行程序入口。

### `crates/`
平台内核与运行时能力。

### `ecosystem/`
生态内容对象。

这样可以清晰地区分：

1. 用户运行什么程序
2. 平台本身有哪些内核能力
3. 生态对象如何被安装和加载

---

## 3. 推荐目录树

```text
cyberclaw/
├── apps/
│   ├── cyberclaw-server/              # HTTP / Web / ACP server
│   ├── cyberclaw-cli/                 # CLI
│   └── cyberclaw-worker/              # 后台任务、scheduler、job runner（可选）
│
├── crates/
│   ├── cyberclaw-core/                # 核心类型与抽象
│   ├── cyberclaw-control-plane/       # 控制平面
│   ├── cyberclaw-agent-runtime/       # Agent runtime / subagent runtime
│   ├── cyberclaw-workflow/            # Workflow engine
│   ├── cyberclaw-governance/          # Governance gate
│   ├── cyberclaw-observability/       # audit / trace / provenance
│   ├── cyberclaw-safety/              # prompt scan / runtime detection / trust
│   ├── cyberclaw-memory/              # memory / compaction
│   ├── cyberclaw-automation/          # scheduler / heartbeat
│   ├── cyberclaw-connectors/          # connector runtime / capability registry
│   ├── cyberclaw-skill-runtime/       # skill loader / resolver
│   ├── cyberclaw-platform-plugins/    # platform plugin runtime
│   ├── cyberclaw-vault/               # secrets / credential lifecycle
│   └── cyberclaw-store/               # state store / review queue / persistence
│
├── ecosystem/
│   ├── agents/
│   ├── skills/
│   ├── connectors/
│   └── platform-plugins/
│
├── workflows/
├── policies/
├── state/
└── docs/
```

---

## 4. 分层职责

## 4.1 `apps`
只负责入口和装配，不承载平台中心逻辑。

### `cyberclaw-server`
职责：

- 启动 HTTP / Web / ACP server
- 暴露 API 和远程客户端入口
- 组装 Control Plane 和内核服务

### `cyberclaw-cli`
职责：

- 提供本地 CLI 入口
- 将命令请求送入 Control Plane
- 显示执行结果和 review 状态

### `cyberclaw-worker`
职责：

- 运行后台作业
- 承接 scheduler / automation / heartbeat
- 处理长任务和异步队列

说明：
`cyberclaw-worker` 用于承载后台作业；在简化部署形态下，这部分能力也可以与 `cyberclaw-server` 组合部署。

---

## 4.2 `crates`
这是平台内核。

### `cyberclaw-core`
职责：

- 定义核心类型
- 定义 trait 和对象模型
- 定义错误类型、共享协议和最小抽象

建议包含：

- `Task`
- `Case`
- `WorkflowRef`
- `Execution`
- `ExecutionTree`
- `Capability`
- `Artifact`
- `Review`
- `Trace`
- `Provenance`
- `SpawnRequest`
- `ResultEnvelope`

### `cyberclaw-control-plane`
职责：

- Gateway / Session Router
- Task / Case Manager
- Resolver
- Registry / Package / Trust 入口
- Review Queue 入口
- Subagent Scheduler
- Automation 入口协调

这是平台中心，不是 `api` 或 `cli`。

### `cyberclaw-agent-runtime`
职责：

- Agent runtime
- Subagent runtime
- 上下文装配
- Agent 生命周期管理
- 结构化执行与结果回收

### `cyberclaw-workflow`
职责：

- Workflow 定义与解析
- 顺序 / 并行 / 分支 / 重试
- Join / fanout / fanin

### `cyberclaw-governance`
职责：

- Governance Gate
- Risk Policy
- Approval / Review
- Capability 级权限控制
- Policy 决策

### `cyberclaw-observability`
职责：

- Audit
- Trace
- Correlation
- Provenance
- Metrics / logs / lineage

### `cyberclaw-safety`
职责：

- Prompt Security Scanner
- Skill / Package Trust Scanner
- Runtime Detection Engine
- Security Event 汇聚

### `cyberclaw-memory`
职责：

- Memory
- Compaction
- 长期记忆与短期记忆边界
- Session / Workspace 记忆读写策略

### `cyberclaw-automation`
职责：

- Scheduler
- Heartbeat
- Automation runtime
- 后台周期任务协调

### `cyberclaw-connectors`
职责：

- Connector runtime
- Capability registry
- 外部系统调用适配
- RemoteAgentConnector 抽象

### `cyberclaw-skill-runtime`
职责：

- Skill loader
- Skill resolver
- Skill metadata
- 标准 Skill 兼容装配

### `cyberclaw-platform-plugins`
职责：

- Platform Plugin runtime
- Hook 生命周期管理
- 平台级事件监听与拦截

### `cyberclaw-vault`
职责：

- Secret 管理
- Credential lifecycle
- Token refresh / ownership / boundary injection

### `cyberclaw-store`
职责：

- 状态持久化
- Review Queue 存储
- Execution / Artifact / PolicyDecision / SecurityEvent 存储

---

## 4.3 `ecosystem`
这是生态内容目录，不是平台源码目录。

### `ecosystem/agents/`
存放声明式 Agent 角色包。

### `ecosystem/skills/`
存放标准 Skill 包。

### `ecosystem/connectors/`
存放 Connector manifest 和分发元数据。

### `ecosystem/platform-plugins/`
存放 Platform Plugin 包和 hook 元数据。

---

## 5. crate 依赖原则

建议遵循以下依赖方向：

```text
apps -> control-plane -> runtime services -> core
```

更具体地：

```text
apps/*
  -> cyberclaw-control-plane
  -> cyberclaw-observability

cyberclaw-control-plane
  -> cyberclaw-core
  -> cyberclaw-agent-runtime
  -> cyberclaw-workflow
  -> cyberclaw-governance
  -> cyberclaw-connectors
  -> cyberclaw-skill-runtime
  -> cyberclaw-store

cyberclaw-agent-runtime
  -> cyberclaw-core
  -> cyberclaw-observability
  -> cyberclaw-memory

cyberclaw-governance
  -> cyberclaw-core
  -> cyberclaw-observability
  -> cyberclaw-safety

cyberclaw-connectors
  -> cyberclaw-core
  -> cyberclaw-vault
  -> cyberclaw-observability

cyberclaw-skill-runtime
  -> cyberclaw-core

cyberclaw-platform-plugins
  -> cyberclaw-core
  -> cyberclaw-observability
```

约束：

1. `core` 不依赖其他业务 crate
2. `control-plane` 可以依赖其他内核 crate，但不能反向被它们依赖
3. `apps` 只做装配和入口
4. `ecosystem` 不直接依赖 Rust crate，而是通过 manifest/runtime 加载

---

## 6. 最小核心 crate 集合

平台最小可用形态可由以下核心 crate 构成：

1. `cyberclaw-core`
2. `cyberclaw-control-plane`
3. `cyberclaw-agent-runtime`
4. `cyberclaw-workflow`
5. `cyberclaw-governance`
6. `cyberclaw-observability`
7. `cyberclaw-connectors`
8. `cyberclaw-skill-runtime`
9. `cyberclaw-store`

### 可组合实现的模块

以下模块在逻辑边界上应保持独立，但在简化实现中可以与其他 crate 组合：

1. `cyberclaw-safety`
可与 `cyberclaw-governance` 组合实现

2. `cyberclaw-memory`
可与 `cyberclaw-agent-runtime` 组合实现

3. `cyberclaw-automation`
可与 `cyberclaw-control-plane` 组合实现

4. `cyberclaw-platform-plugins`
可与 `cyberclaw-control-plane` 或 `cyberclaw-observability` 组合实现

5. `cyberclaw-vault`
可与 `cyberclaw-connectors` 组合实现

这样既保证边界正确，也避免早期实现出现过多碎片化 crate。

---

## 7. 推荐 Cargo workspace 草案

根 `Cargo.toml` 可采用如下结构：

```toml
[workspace]
resolver = "2"
members = [
  "apps/cyberclaw-server",
  "apps/cyberclaw-cli",
  "apps/cyberclaw-worker",
  "crates/cyberclaw-core",
  "crates/cyberclaw-control-plane",
  "crates/cyberclaw-agent-runtime",
  "crates/cyberclaw-workflow",
  "crates/cyberclaw-governance",
  "crates/cyberclaw-observability",
  "crates/cyberclaw-safety",
  "crates/cyberclaw-memory",
  "crates/cyberclaw-automation",
  "crates/cyberclaw-connectors",
  "crates/cyberclaw-skill-runtime",
  "crates/cyberclaw-platform-plugins",
  "crates/cyberclaw-vault",
  "crates/cyberclaw-store",
]

[workspace.package]
version = "0.1.0"
edition = "2021"
authors = ["CyberClaw Team"]
license = "Apache-2.0"

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
async-trait = "0.1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml = "0.9"
thiserror = "1"
anyhow = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
```

在极简部署形态下，可以只保留上述 9 个核心 crate。

---

## 8. 推荐实现顺序

### 核心闭环
优先实现：

1. `cyberclaw-core`
2. `cyberclaw-control-plane`
3. `cyberclaw-agent-runtime`
4. `cyberclaw-governance`
5. `cyberclaw-store`
6. `apps/cyberclaw-server`
7. `apps/cyberclaw-cli`

### 扩展能力
随后补充：

1. `cyberclaw-workflow`
2. `cyberclaw-observability`
3. `cyberclaw-connectors`
4. `cyberclaw-skill-runtime`

### 平台增强
继续补充：

1. `cyberclaw-safety`
2. `cyberclaw-memory`
3. `cyberclaw-automation`
4. `cyberclaw-platform-plugins`
5. `cyberclaw-vault`
6. `apps/cyberclaw-worker`

---

## 9. 一句话结论

> **CyberClaw 采用 `apps / crates / ecosystem` 三分结构，并把 `cyberclaw-control-plane` 设为平台中心。平台源码、程序入口和生态内容分别位于清晰的边界之内。**
