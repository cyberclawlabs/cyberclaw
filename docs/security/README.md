# Security & Governance

CyberClaw 的差异化不在于让模型更自由，而在于让高风险自动化仍然可治理。

## 这一组文档回答什么

- CyberClaw 如何理解治理
- 平台如何看待审批与策略
- 为什么 Capability 是最小治理单元
- 为什么审计和追踪是平台主链的一部分

## 适合谁先读

- 想理解平台为什么强调 governance 的使用者
- 想评估边界是否清晰的 builder
- 想检查公开说法是否和实现现实一致的维护者

## 代表性工作流

这组文档面向的不是抽象“安全能力”，而是可以直接落到团队职责上的工作流：

- 代码审计与 PR 风险检查
- 安全运营中的告警分诊、升级和跟踪
- 安全事件处置中的隔离、升级、补丁协同和审计留痕
- DevOps / 运维中的发布门禁、回滚提案和变更审批
- 数据库写入、事务、迁移等高风险内部操作
- Web3 中金库、多签、Signer 相关高风险动作

## 场景链路示例

在真实安全与运营流程里，CyberClaw 更适合承载下面这些工作流：

| 场景 | 示例链路 | 当前仓库支撑 |
|------|------|------|
| 代码审计 / PR 风险检查 | `Agent` 收集 PR、变更文件、相关规范和历史上下文；`Skill` 组织审计方法；GitHub `Connector` 提供仓库协同面；MCP `mcp.prompt.code_review` 这类能力可生成受控的审查模板；治理链限制是否允许创建评论、Issue 或后续任务。 | GitHub Connector、MCP README 中的 `mcp.prompt.code_review` |
| 安全运营 / 告警分诊 | `Agent` 接收告警后先拉取 trace、日志、相关代码与历史记录；`Skill` 输出分诊结论；Slack `Connector` 负责升级和通知，GitHub `Connector` 负责创建跟踪工单；治理链确保“收集信息”和“执行后续动作”被明确区分。 | Observability、Slack Connector、GitHub Connector |
| 安全事件处置 | `Agent` 在分诊后提出隔离、升级、补丁协同或调查步骤；治理链对外部写入、运行任务或生产变更做门控；平台保留完整审计证据，便于复盘与合规。 | 审计文档、trace 能力、运行时隔离模式 |
| 发布门禁 / 变更审批 | `Agent` 可以组织发布清单、收集检查结果、生成回滚提案并通知值班人；但真实变更仍要经过 GitHub、Slack 和部署相关接入面进入治理链，而不是由模型直接执行。 | GitHub Connector、Slack Connector、部署文档 |
| 数据库变更门禁 | `Agent` 先分析 SQL、迁移计划和影响范围，再把动作交给 Database `Connector`；`db.query`、`db.execute`、`db.transaction`、`db.migrate` 的风险不同，迁移类动作应匹配更强审批和隔离。 | Database Connector 中的分级能力定义 |
| Web3 高风险执行 | `Agent` 汇总钱包、交易、治理和风险上下文；`Skill` 形成提案；审批与策略决定是否放行；只有被允许的 `Connector` / `Capability` 可以继续执行；最终保留审批、追踪和执行产物。 | Web3 Guide、治理与审计文档 |

## 平台如何把这些流程做得可控

关键不在口号，而在几个明确边界：

- `Connector` 是唯一代码级能力接入面，避免把外部系统直接裸露给模型
- `Capability` 是最小治理单元，便于把“可读取”和“可写入”分开
- Runtime 隔离模式区分 `Native`、`Process`、`Container`，高风险动作应匹配更强隔离
- 审计与 trace 是执行链一部分，而不是事后补丁

## 阅读顺序

1. [Governance Model](governance-model.md)
2. [Approvals and Policies](approvals-and-policies.md)
3. [Audit and Traceability](audit-and-traceability.md)

## 阅读提醒

这一组文档说明的是平台安全与治理方向，不应替代：

- 当前代码现实
- 当前测试结果
- 当前实施复核结论

## 深入材料

- [治理架构 README](../architecture/governance/README.md)
- [上线前终检报告](../implementation/reports/pre-launch-review-2026-04-15.md)
