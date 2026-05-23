# CyberClaw

- Status: Active
- Scope: Repository
- Owner: CyberClaw Maintainers
- Last Updated: 2026-04-18

<div align="center">

**面向高风险真实业务系统的可治理 Agent 基础设施**

[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Docs](https://img.shields.io/badge/docs-portal-blue.svg)](docs/README.md)

[English](README.md) | [简体中文](README.zh-CN.md)

</div>

---

### CyberClaw 是什么？

CyberClaw 不是为了做一个更会聊天的 Agent，而是为了让 Agent 在高风险、真实业务系统中参与分析、协同和执行时，仍然保持安全、可控、可审计。

它面向已经开始把 AI 接入真实流程的团队，但不能接受“让模型直接触碰生产系统”。安全团队可以用它组织代码审计、告警分诊、事件处置和审计追踪；AI 团队可以用它把 Agent 从 Demo 带到生产；小团队也可以借此形成接近“一人安全中心”的工作方式，在有限人力下提升研判、响应和运营效率。

CyberClaw 的核心不是多接几个工具，而是把推理、执行、治理、审计和外部系统接入拆成清晰边界，让自动化建立在受控执行、策略约束、审批链路和可追踪产物之上。

CyberClaw 是通用可治理 Agent 平台，`Web3` 是当前最强落地场景。在钱包、Signer、金库、多签、链上运营和异常处置等环境中，治理和执行边界不是附加能力，而是前提条件。

### 核心场景

#### Web3 核心场景

CyberClaw 重点面向这类 Web3 工作流：

- 金库与多签流程：执行前需要上下文汇总、风险判断、审批和留痕
- Signer 门禁与链上 runbook：把策略、执行和审计拆开，而不是直接发起交易
- 协议运营与治理协同：统一链上动作、外部系统和内部审批
- 链上异常处置：组织告警、升级、响应和后续审计

#### 其他高价值场景

同样的控制模型也适合：

- 代码审计与 PR 风险检查
- 告警分诊、升级和安全事件处置
- 发布门禁、回滚提案和变更审批
- 数据库查询、写入、事务和迁移门禁

#### 当前仓库中的现有支撑

当前仓库已经包含与这些场景直接相关的连接器和文档入口：

- GitHub Connector 示例：Issue、PR、代码审查、仓库查询
- Slack Connector 示例：消息发送、频道创建、文件上传
- Database Connector 示例：查询、写入、事务、迁移
- 部署文档：部署、健康检查、生产配置
- 审计与安全基础设施：audit 插件、security event、observability 文档

### 典型治理链路示例

CyberClaw 适合承载的不是“模型直接调一堆工具”，而是这类有明确边界的真实流程：

| 场景 | 示例链路 |
|------|------|
| Web3 金库 / 多签流程 | `Agent` 汇总余额、请求、Signer 上下文和策略输入；`Skill` 生成执行提案；治理链做审批和策略判断；钱包相关 `Connector` 只暴露被允许的 `Capability`；最后沉淀审批记录、追踪和执行产物。 |
| 代码审计 / PR 风险检查 | `Agent` 收集 PR 上下文、变更文件和相关规范；`Skill` 组织审计方法；GitHub `Connector` 提供仓库协同面；MCP `mcp.prompt.code_review` 这类能力可生成审查模板；治理链限制后续写动作。 |
| 告警分诊 / 升级 | `Agent` 拉取 trace、日志和相关仓库上下文；`Skill` 输出分诊结论；Slack `Connector` 负责升级通知，GitHub `Connector` 负责创建跟踪 Issue；治理链约束后续高风险动作。 |
| 安全事件处置 | `Agent` 在分诊后提出隔离、升级、补丁协同或后续排查步骤；治理链对外部写入、运行任务和系统变更做门控；平台把告警、审批、动作和产物串成完整审计链路。 |
| 发布门禁 / 变更审批 | `Agent` 组织发布清单、检查结果和回滚提案，并通过 GitHub + Slack 协同；涉及回滚、变更执行或生产动作时，治理链要求显式边界和审批。 |
| 数据库变更门禁 | `Agent` 先分析 SQL、迁移计划和影响范围，再把执行请求交给 Database `Connector`；`db.query`、`db.execute`、`db.transaction`、`db.migrate` 风险不同，迁移类动作需要更强审批、隔离和留痕。 |

### 从这里开始

- [文档门户](docs/README.md)
- [快速开始](docs/getting-started/README.md)
- [开发者指引](docs/builders/README.md)
- [安全与治理](docs/security/README.md)
- [Web3 指引](docs/web3/README.md)
- [Skill Hub MVP](docs/business/brand/SKILL_HUB_MVP.md)
- [I18N 内容策略](docs/business/brand/I18N_CONTENT_STRATEGY.md)

### 语言支持

当前仓库入口层语言支持：

- `en` - 默认开源入口语言
- `zh-CN` - 维护中的中文入口语言

公开站点近期扩展语种：

- `ja`
- `ko`
- `es`

### 适合谁？

#### 使用者 / 集成者

- 运行具有明确执行边界的 Agent
- 接入 Skill 和 Connector，同时保持治理链不被绕过
- 在 Web3 和其他高风险自动化场景中落地

#### 生态构建者

- 构建和发布 Skill、Connector、Platform Plugin
- 复用平台的受控执行模型
- 在不破坏五对象边界的前提下扩展平台

### 快速开始

```bash
git clone https://github.com/cyberclawlabs/cyberclaw.git
cd cyberclaw
cargo build
cargo run -p cyberclaw-cli -- --help
```

查看本地平台表面：

```bash
cargo run -p cyberclaw-cli -- status
```

### 为什么是 CyberClaw？

- 强调可治理执行，而不是无边界 agent 行为
- 用 `Connector` 和 `Capability` 约束真实动作
- 把审计、追踪、可观测性放进主链路
- 提供可扩展的 Skill / Connector / Platform Plugin 生态面
- 特别适合 Web3 及其他高风险环境

### 平台构件

CyberClaw 围绕五个核心对象组织：

| 对象 | 职责 |
|------|------|
| `Agent` | 角色、编排、执行预算 |
| `Skill` | 知识、方法、提示词、参考资料 |
| `Connector` | 运行时与外部系统接入 |
| `Capability` | 最小治理动作单元 |
| `Platform Plugin` | 平台级增强钩子 |

对外接入时，最容易理解的仍然是：

- `Skill`：怎么做
- `Tool` 表面：平台如何对外暴露受控能力

但平台内部执行链保持不变：

`Task/Case -> Resolver -> Execution -> Governance -> Connector -> Capability -> Artifact/Trace`

### Web3 现阶段定位

CyberClaw 是通用平台，Web3 是当前最能体现其治理、安全、审计和风险控制价值的落地方向。

查看 [Web3 指引](docs/web3/README.md)。

### 文档入口

- [文档总索引](docs/INDEX.md)
- [架构文档](docs/architecture/README.md)
- [实施文档](docs/implementation/README.md)
- [业务文档](docs/business/README.md)
- [贡献指南](CONTRIBUTING.md)
- [安全策略](SECURITY.md)
- [变更记录](CHANGELOG.md)

### 当前状态

#### 已实现

- 核心平台 crate 与运行时基础层
- 治理、可观测性、隔离执行基础能力
- CLI 与 Server 入口
- 较完整的架构与实施文档基础

#### 正在建设

- 面向开源访客的产品化文档层
- 开源门面与外部叙事整理
- Skill Hub 发现入口
- 公开内容 i18n 化

#### 路线图

- `cyberclawlabs.ai` GitHub Pages 首页
- 独立 Skill Hub 体验
- 更完整的 builder 生态工作流
- 更多公开语种覆盖

### 联系方式

- 公共邮箱：`info@cyberclawlabs.ai`

不要只根据本 README 判断实现状态。当前事实应结合代码、测试结果、实现报告和评审记录共同判断。
