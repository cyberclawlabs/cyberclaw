# Platform Modules

这一页同时回答两个问题：

1. CyberClaw 的平台对象分别是什么，应该怎么使用
2. 仓库里的 `apps/`、`crates/`、`ecosystem/` 各自负责什么

如果你只需要一个“先理解整体，再决定往哪读”的入口，这一页就是最短路径。

## 平台对象

### `Agent`

**负责什么**

- 角色主体
- 任务编排
- 执行预算和上下文组织

**什么时候先看它**

- 你想理解“谁来做”
- 你在看多步骤任务如何被组织

**不负责什么**

- 不直接替代底层执行器
- 不直接绕过治理去调用外部系统

**继续阅读**

- [User Guide / Agents](../user-guide/agents.md)
- [Architecture Overview](../architecture/overview/ARCHITECTURE_V2.0.md)

### `Skill`

**负责什么**

- 方法、知识、提示词、模板、参考资料
- 让 `Agent` 知道“怎么做”

**什么时候先看它**

- 你要沉淀操作方法
- 你要复用提示词、规则、参考材料
- 你要做生态分发和共享

**不负责什么**

- 不直接承载平台执行权限
- 不替代 `Connector`

**继续阅读**

- [User Guide / Skills](../user-guide/skills.md)
- [Build a Skill](../builders/build-a-skill.md)
- [Skill/Tool 兼容性架构设计](../architecture/overview/SKILL_TOOL_COMPATIBILITY_V1.md)

### `Connector`

**负责什么**

- 外部系统和运行时接入
- 受控能力暴露
- 把真实动作组织成可治理 `Capability`

**什么时候先看它**

- 你要接 GitHub、Slack、数据库、链上系统
- 你要为外部系统定义执行边界

**不负责什么**

- 不绕开治理链
- 不把大而混乱的动作直接暴露给模型

**继续阅读**

- [User Guide / Connectors](../user-guide/connectors.md)
- [Build a Connector](../builders/build-a-connector.md)

### `Capability`

**负责什么**

- 最小治理动作单元
- 明确读写边界和风险等级

**什么时候先看它**

- 你在定义“能做什么”和“允许做到什么程度”
- 你在设计审批、策略和风险分级

**不负责什么**

- 不是面向访客的一级生态对象
- 不承载复杂编排逻辑

**继续阅读**

- [Security & Governance](../security/README.md)
- [Governance Model](../security/governance-model.md)

### `Platform Plugin`

**负责什么**

- 平台级生命周期增强
- 横切能力和集成钩子

**什么时候先看它**

- 你要增强平台行为本身，而不是接外部系统

**不负责什么**

- 不承担业务角色
- 不替代 `Connector`
- 不绕过治理、审计和追踪

**继续阅读**

- [Build a Platform Plugin](../builders/build-a-plugin.md)

## 统一执行链

平台主链固定为：

`Task/Case -> Resolver -> Execution -> Governance -> Connector -> Capability -> Artifact/Trace`

这条链决定了 CyberClaw 的边界：

- `Skill` 负责方法，不负责直接执行
- `Connector` 是唯一代码级能力接入面
- `Capability` 是最小治理单元
- 审计与追踪不是附属功能，而是执行链的一部分

## 仓库模块

### 应用入口

| 路径 | 作用 | 什么时候读 |
| --- | --- | --- |
| `apps/` | 可运行应用入口，例如 CLI、server | 你要启动程序、看路由或看顶层装配 |

### 核心 crates

| 路径 | 作用 | 什么时候读 |
| --- | --- | --- |
| `crates/cyberclaw-core` | 核心类型、trait、协议、manifest 抽象 | 你要理解对象模型和共享类型 |
| `crates/cyberclaw-control-plane` | 加载、注册、解析、编排、执行主链 | 你要看执行链和控制平面 |
| `crates/cyberclaw-connectors` | Connector 实现与能力分发 | 你要加接入面或治理动作 |
| `crates/cyberclaw-governance` | 审批、权限、策略、风险 | 你要加治理规则或审批链 |
| `crates/cyberclaw-observability` | 事件、trace、metrics | 你要看审计和可观测性 |
| `crates/cyberclaw-agent-runtime` | Agent 运行时 | 你要改 Agent 执行逻辑 |
| `crates/cyberclaw-skill-runtime` | Skill 运行时 | 你要处理 Skill 装载与运行 |
| `crates/cyberclaw-store` | 存储层 | 你要改状态持久化 |
| `crates/cyberclaw-workflow` | 工作流层 | 你要看复杂流程编排 |

### 生态与文档

| 路径 | 作用 | 什么时候读 |
| --- | --- | --- |
| `ecosystem/` | Agent / Skill / Connector / Platform Plugin 示例包 | 你要看生态包结构或示例 |
| `schemas/` | JSON Schema | 你要校验 manifest 或元信息 |
| `docs/` | 正式文档 | 你要找公开入口、架构说明或实施记录 |

## 典型阅读路径

### 想先跑起来

1. [Getting Started](../getting-started/README.md)
2. [Quickstart](../getting-started/quickstart.md)
3. [CLI Reference](cli.md)

### 想先理解平台边界

1. 本页
2. [User Guide](../user-guide/README.md)
3. [Security & Governance](../security/README.md)

### 想开始做扩展

1. 本页
2. [Builder Guide](../builders/README.md)
3. [Skill Hub Repository Guide](../builders/skill-hub-repository.md)
4. [Manifests](manifests.md)
