# Governance Model

CyberClaw 的治理不是外围补丁，而是执行链的一部分。

## 治理关注什么

- 谁可以执行
- 在什么上下文下执行
- 经过什么策略判断
- 是否需要审批
- 执行后如何留痕

## 为什么 Capability 是核心

因为它是平台里最小、最适合治理和授权的动作单元。

## 具体治理什么

在真实团队里，治理通常不是针对一个抽象“Agent”，而是针对具体动作：

- 代码审计过程中，是否允许创建评论、Issue 或后续修复任务
- 告警分诊过程中，是否只允许读取 trace、日志和仓库上下文
- 安全事件处置过程中，是否允许发送升级通知、触发后续 runbook 或修改外部系统状态
- DevOps / 发布过程中，是否允许执行回滚、变更单推进或生产环境相关动作
- 数据库操作中，是否只允许 `db.query`，还是允许 `db.execute`、`db.transaction`、`db.migrate`

## 一个简化判断方式

可以把治理理解成三个问题：

1. 这个动作是读、写，还是高风险变更
2. 这个动作是否需要审批、策略或人工确认
3. 这个动作应该运行在什么隔离级别下

在 CyberClaw 里，这些判断最终都应落到 `Capability` 和执行边界上，而不是停留在 prompt 层。

## 相关文档

- [架构总览](../architecture/overview/ARCHITECTURE_V2.0.md)
- [治理架构 README](../architecture/governance/README.md)
